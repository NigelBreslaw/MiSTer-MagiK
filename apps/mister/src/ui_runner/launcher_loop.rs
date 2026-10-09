// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

// The pipeline tests pin these markers as source text; each one also feeds the frame profile.
macro_rules! record_launcher_frame_phase {
    ($phase:expr) => {
        crate::ui_runner::phase_profile::mark($phase)
    };
}

mod frame_loop;
pub(super) use frame_loop::{Env, run_frame_loop};

use super::arcade_drawer::{ArcadeDrawerViewCache, arcade_filter_cache_token};
use super::crt_backdrop_controller::CrtBackdropController;
use super::launcher_confirmation::{
    DisplayConfirmation, OrientationConfirmation, display_confirmation_ui_enabled,
};
use super::launcher_frame_accounting::{
    FrameAnalyticsCpuStamp, FrameAnalyticsMode, LauncherCustomDrawTrace, LauncherFrameAccounting,
};
use super::launcher_pacing::{
    FB0_LATE_FRAME_START_HEADROOM_US, FrameProductionClass, FrameProductionTrace,
    LauncherFramePacingInput, LauncherFramePacingPolicy, LauncherPacingTrace,
    LauncherPhaseAlignment,
};
use super::launcher_screensaver::{ScreensaverRenderTrace, ScreensaverStartupTimeline};
use super::launcher_settings_pipeline::{SettingsCogSession, SettingsFrameRequest};
use super::launcher_transition_start::{
    SettingsInputs, TransitionInputs, begin_navigation_transition, begin_settings_transition,
};
use super::launcher_worker_intents::reset_media_progress_bridge;
use super::launcher_worker_intents::{
    LauncherWorkerUiIntent, apply_launcher_worker_ui_intent, catalog_scan_message,
};
use super::phase_profile::LauncherFramePhase;
use super::*;
#[path = "launcher_loop_startup.rs"]
mod startup;
use crate::input_event::{InputPhase, LogicalAction};
use crate::input_state::PadState;
use crate::launcher_presentation::SelectionFeedbackTarget;
use crate::launcher_ui_actions::{
    LauncherUiAction, LauncherUiActionsAdapter, apply_navigation_action,
};
use crate::preview_state::PreviewApplyTrace;
#[cfg(test)]
use mister_magik_fb::framebuffer::target::PhysicalLayerBacking;
use mister_magik_fb::process_config::ScreensaverStartMode;
use std::collections::{BTreeSet, VecDeque};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

const LIBRARY_RESET_REBOOT_TIMEOUT: Duration = Duration::from_secs(15);

enum LibraryResetState {
    Idle,
    Deleting(std::sync::mpsc::Receiver<Result<launcher::PurgeLibraryDataOutcome, String>>),
    RebootRequested { deadline: Instant },
}

impl LibraryResetState {
    /// True while ordinary launcher work must remain paused.
    fn poll(&mut self, now: Instant) -> Result<bool, String> {
        let error = match self {
            Self::Idle => return Ok(false),
            Self::RebootRequested { deadline } => {
                if now < *deadline {
                    return Ok(true);
                }
                "Database deleted, but MiSTer did not reboot. Restart MiSTer manually.".to_string()
            }
            Self::Deleting(worker) => match worker.try_recv() {
                Ok(Ok(outcome)) => {
                    crate::ui_logln!(
                        "library_reset_reboot_requested catalog_removed={} screenshot_removed={}",
                        outcome.catalog_artifacts_removed,
                        outcome.screenshot_artifacts_removed
                    );
                    *self = Self::RebootRequested {
                        deadline: now + LIBRARY_RESET_REBOOT_TIMEOUT,
                    };
                    return Ok(true);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(true),
                Ok(Err(error)) => error,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    "Database reset worker stopped unexpectedly".to_string()
                }
            },
        };
        *self = Self::Idle;
        Err(error)
    }
}

const DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY: Duration = Duration::from_secs(2);
const CATALOG_READY_STATIONARY_EDGE_SETTLE: Duration = Duration::from_millis(250);
const CATALOG_IDLE_BURST_SETTLE: Duration = Duration::from_millis(1_000);
fn card_direct_tile_damage(left: usize, level_trick: bool, split: usize) -> [DirtyRect; 2] {
    // Trick rendering clears from x=268, including root cards whose ordinary
    // carousel starts at x=296. Keep that width on the landing frame as well.
    [
        DirtyRect {
            x0: if level_trick { 268 } else { left },
            y0: 120,
            x1: split,
            y1: 495,
        },
        DirtyRect {
            x0: split,
            y0: 120,
            x1: 934,
            y1: 495,
        },
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CardDirectEligibility {
    custom_home_active: bool,
    custom_home_needs_render: bool,
    /// 960x540 landscape, or an HDMI portrait output (rotated into scanout).
    direct_geometry: bool,
    full_frame_present: bool,
    launching: bool,
    screensaver_active: bool,
    startup_intro_active: bool,
    startup_reveal_suppressed: bool,
    startup_intro_suppressed: bool,
    confirm_visible: bool,
    catalog_scan_visible: bool,
    navigation_transition_active: bool,
    orientation_transition_active: bool,
    composition_state: UiCompositionState,
    force_full_slint_raster: bool,
    force_full_slint_present: bool,
    transition_state: FullScreenTransitionState,
}

fn card_direct_hidden_eligible(input: CardDirectEligibility) -> bool {
    input.custom_home_active
        && input.custom_home_needs_render
        && input.direct_geometry
        && !input.full_frame_present
        && !input.launching
        && !input.screensaver_active
        && !input.startup_intro_active
        && !input.startup_reveal_suppressed
        && !input.startup_intro_suppressed
        && !input.confirm_visible
        && !input.catalog_scan_visible
        && !input.navigation_transition_active
        && !input.orientation_transition_active
        && input.composition_state == UiCompositionState::FullSlint
        && !input.force_full_slint_raster
        && !input.force_full_slint_present
        && input.transition_state == FullScreenTransitionState::Live
}

fn card_cached_frame_view(
    pixels: &[mister_magik_framebuffer_scenes::Rgb565Pixel],
    width: usize,
    height: usize,
) -> CachedFrameView<'_> {
    assert_eq!(pixels.len(), width.saturating_mul(height));
    CachedFrameView::new(card_pixels_as_slint(pixels), width, height)
}

pub(super) fn card_pixels_as_slint(
    pixels: &[mister_magik_framebuffer_scenes::Rgb565Pixel],
) -> &[Rgb565Pixel] {
    const {
        assert!(
            std::mem::size_of::<mister_magik_framebuffer_scenes::Rgb565Pixel>()
                == std::mem::size_of::<Rgb565Pixel>()
        );
        assert!(
            std::mem::align_of::<mister_magik_framebuffer_scenes::Rgb565Pixel>()
                == std::mem::align_of::<Rgb565Pixel>()
        );
    }
    // SAFETY: both RGB565 pixel types are transparent `u16` wrappers, have
    // compile-time-checked layout, and accept every `u16` bit pattern.
    unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<Rgb565Pixel>(), pixels.len()) }
}

pub(super) fn selected_device_reveal_image(
    preview: &crate::preview_state::PreviewState,
    backdrop: Option<&CrtBackdropController>,
    layout: crate::ui_display::UiLayoutGeometry,
) -> Option<mister_magik_framebuffer_scenes::device_card::RevealImage> {
    preview.selected_backdrop_source().map(|source| {
        mister_magik_framebuffer_scenes::device_card::RevealImage {
            pixels: source.words,
            width: source.source_width,
            height: source.source_height,
            stride: source.stride_pixels,
            reference_height: backdrop.map_or(layout.logical_h(), |b| b.reference_height()),
            integer_scale: backdrop.is_some_and(|b| b.reference_height() > b.physical_height()),
        }
    })
}

fn accepted_selection_feedback_input(event: Option<&crate::input_event::InputEvent>) -> bool {
    event.is_some_and(|event| event.phase == InputPhase::Pressed)
}

fn discrete_selection_feedback_target(
    nav: &LauncherNav,
    setup: &SetupNav,
    lifecycle: &LauncherLifecycle,
) -> Option<SelectionFeedbackTarget> {
    if setup.is_active() {
        return setup_selection_feedback_target(setup);
    }

    let lifecycle_view = lifecycle.view();
    if let Some(dialog) = lifecycle_view.catalog_recovery_dialog() {
        return Some(SelectionFeedbackTarget::new(
            format!("dialog:catalog-recovery:{}", dialog.title),
            if dialog.selected.selected_index() == 0 {
                "left"
            } else {
                "right"
            },
        ));
    }
    if lifecycle_view.launch_failure_dialog().is_some() {
        return Some(SelectionFeedbackTarget::new(
            "dialog:launch-failure",
            "back",
        ));
    }
    if let Some(action) = nav.confirm_action {
        return Some(SelectionFeedbackTarget::new(
            format!("dialog:{action:?}"),
            if nav.confirm_selected == 0 {
                "left"
            } else {
                "right"
            },
        ));
    }

    nav_selection_feedback_target(nav)
}

fn setup_selection_feedback_target(setup: &SetupNav) -> Option<SelectionFeedbackTarget> {
    let surface = format!("setup:{:?}:{:?}", setup.phase, setup.target_device);
    match setup.phase {
        SetupPhase::NewOrExisting => Some(SelectionFeedbackTarget::new(
            surface,
            if setup.list_index == 0 {
                "new"
            } else {
                "existing"
            },
        )),
        SetupPhase::PickExisting => Some(SelectionFeedbackTarget::new(
            surface,
            format!("saved:{}", setup.list_index),
        )),
        _ => None,
    }
}

fn nav_selection_feedback_target(nav: &LauncherNav) -> Option<SelectionFeedbackTarget> {
    match nav.screen {
        Screen::Home => SelectionFeedbackTarget::home(nav),
        Screen::Arcade if nav.is_system_hub() => Some(SelectionFeedbackTarget::new(
            "system-hub",
            ["games", "recent", "favorites", "info"]
                .get(nav.system_hub_selected)
                .copied()
                .unwrap_or("unknown"),
        )),
        Screen::Settings if nav.display_combo_open => Some(SelectionFeedbackTarget::new(
            "display-combo",
            format!("option:{}", nav.display_highlighted),
        )),
        Screen::Settings if nav.orientation_combo_open => Some(SelectionFeedbackTarget::new(
            "orientation-combo",
            format!("option:{}", nav.orientation_highlighted),
        )),
        Screen::Settings => Some(SelectionFeedbackTarget::new(
            "settings",
            [
                "display",
                "orientation",
                "reduce-motion",
                "screensaver-delay",
                "screensaver-preview",
                "exit",
                "rebuild",
                "about",
            ]
            .get(nav.settings_selected)
            .copied()
            .unwrap_or("unknown"),
        )),
        Screen::About => Some(SelectionFeedbackTarget::new("about", "licenses")),
        Screen::Licenses => Some(SelectionFeedbackTarget::new(
            "licenses",
            [
                "mister-magik",
                "ffmpeg",
                "slint",
                "press-start-2p",
                "commercial-fonts",
                "jersey-25",
                "jersey-15",
                "spleen",
                "terminus-font",
                "rust-standard-library",
                "zlib",
                "libpng",
            ]
            .get(nav.licenses_selected)
            .copied()
            .unwrap_or("unknown"),
        )),
        Screen::Arcade
            if nav.arcade_search.is_active(&nav.arcade_filter.active)
                && nav.arcade_search.pane == launcher::ArcadeSearchPane::Keyboard =>
        {
            Some(SelectionFeedbackTarget::new(
                format!(
                    "arcade-search-keyboard:{}",
                    nav.active_collection_id().unwrap_or("none")
                ),
                format!("key:{}", nav.arcade_search.selected_key),
            ))
        }
        // The game list and search results are fixed-selector velocity surfaces.
        // Their press-to-first-motion response remains latency-critical, but
        // continuous crossings do not create discrete acknowledgement pulses.
        Screen::Arcade | Screen::Controller | Screen::LicenseText => None,
    }
}

fn launcher_screen_input_focus(nav: &LauncherNav) -> FocusRequest {
    let (owner, directional_policy) = match nav.screen {
        Screen::Home => (1, DirectionalPolicy::HomeContinuous),
        Screen::Arcade if nav.is_system_hub() => (2, DirectionalPolicy::MenuRepeat),
        Screen::Controller => (3, DirectionalPolicy::EdgeOnly),
        Screen::Arcade if nav.arcade_uses_menu_repeat() => (4, DirectionalPolicy::MenuRepeat),
        Screen::Arcade => (4, DirectionalPolicy::ArcadeContinuous),
        Screen::Settings => (5, DirectionalPolicy::MenuRepeat),
        Screen::About => (7, DirectionalPolicy::MenuRepeat),
        Screen::Licenses => (8, DirectionalPolicy::MenuRepeat),
        Screen::LicenseText => (10, DirectionalPolicy::MenuRepeat),
    };
    FocusRequest {
        target: FocusTarget {
            kind: InputContextKind::Screen,
            owner,
        },
        directional_policy,
    }
}

fn launcher_input_focus(
    enabled: bool,
    screensaver: bool,
    lifecycle_dialog: bool,
    setup: bool,
    modal: bool,
    transition: bool,
    nav: &LauncherNav,
) -> FocusRequest {
    let (kind, owner, directional_policy) = if !enabled {
        (InputContextKind::Disabled, 0, DirectionalPolicy::EdgeOnly)
    } else if screensaver {
        (
            InputContextKind::Screensaver,
            1,
            DirectionalPolicy::EdgeOnly,
        )
    } else if lifecycle_dialog {
        (
            InputContextKind::LifecycleDialog,
            1,
            DirectionalPolicy::MenuRepeat,
        )
    } else if setup {
        (
            InputContextKind::ControllerSetup,
            1,
            DirectionalPolicy::MenuRepeat,
        )
    } else if modal && !nav.refresh_hold_owns_input() {
        (
            InputContextKind::LauncherModal,
            1,
            DirectionalPolicy::MenuRepeat,
        )
    } else if transition {
        (InputContextKind::Transition, 1, DirectionalPolicy::EdgeOnly)
    } else {
        return launcher_screen_input_focus(nav);
    };
    FocusRequest {
        target: FocusTarget { kind, owner },
        directional_policy,
    }
}

impl LauncherPresentBackend {
    fn from_config(config: &mister_magik_fb::process_config::PresentBackendConfig) -> Self {
        use mister_magik_fb::process_config::PresentBackendConfig;
        match config {
            PresentBackendConfig::FpgaVblankLatchHidden => Self::FpgaVblankLatchHidden,
            PresentBackendConfig::Fb0Dirty => Self::Fb0Dirty,
            PresentBackendConfig::Retired(retired) => {
                crate::ui_errln!(
                    "launcher_present_backend_retired value={retired}; using required latch backend"
                );
                boot_analytics::event(
                    "launcher_present_backend_retired",
                    format!("{retired} backend=fpga-vblank-latch-hidden"),
                );
                Self::FpgaVblankLatchHidden
            }
            PresentBackendConfig::Invalid(invalid) => {
                crate::ui_errln!(
                    "launcher_present_backend_invalid value={invalid}; using required latch backend"
                );
                Self::FpgaVblankLatchHidden
            }
        }
    }

    fn log_if_experimental(self) {
        match self {
            Self::None | Self::Fb0Dirty => {}
            Self::FpgaVblankLatchHidden => {
                crate::ui_logln!("launcher_present_backend=fpga-vblank-latch-hidden");
                boot_analytics::event("launcher_present_backend", "fpga-vblank-latch-hidden");
            }
        }
    }
}

fn present_mode_label_for_backend_status(
    backend: LauncherPresentBackend,
    status: LauncherPresentStatus,
) -> &'static str {
    match (backend, status) {
        (LauncherPresentBackend::FpgaVblankLatchHidden, LauncherPresentStatus::Ok) => "Mode=latch",
        (_, LauncherPresentStatus::Frozen) => "Mode=output frozen",
        _ => "Mode=/dev/fb0 diagnostic",
    }
}

#[cfg(feature = "tooling")]
fn duration_us(start: Instant, end: Instant) -> u64 {
    end.saturating_duration_since(start)
        .as_micros()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(feature = "tooling")]
fn u128_to_u64(value: u128) -> u64 {
    value.min(u128::from(u64::MAX)) as u64
}

struct PendingCollectionEntry {
    collection_id: String,
    requested_at: Instant,
    source: launcher::HomeViewState,
    open_game_list_directly: bool,
}

#[derive(Default)]
struct DeferredSettingsActivation {
    event: Option<crate::input_event::InputEvent>,
}

impl DeferredSettingsActivation {
    fn intercept_while_cards_move(
        &mut self,
        nav: &LauncherNav,
        card_home_animating: bool,
        event: &mut Option<crate::input_event::InputEvent>,
    ) -> bool {
        if self.event.is_some()
            || !card_home_animating
            || nav.screen != Screen::Home
            || nav.current_menu_id() != crate::launcher_taxonomy::ROOT_MENU_ID
            || nav.selected != 5
            || !event.as_ref().is_some_and(|event| {
                event.phase == InputPhase::Pressed && event.action == LogicalAction::Activate
            })
        {
            return false;
        }
        self.event = event.take();
        true
    }

    fn take_when_settled(
        &mut self,
        card_home_animating: bool,
    ) -> Option<crate::input_event::InputEvent> {
        if card_home_animating {
            None
        } else {
            self.event.take()
        }
    }

    fn is_pending(&self) -> bool {
        self.event.is_some()
    }
}

const NAVIGATION_STATUS_QUIESCE_LIMIT: Duration = Duration::from_millis(50);

fn should_defer_or_preserve_selected_preview(
    defer_selected_preview: bool,
    navigation_transition_active: bool,
    source_was_arcade: bool,
) -> bool {
    defer_selected_preview || (navigation_transition_active && source_was_arcade)
}

fn preview_work_allowed(
    background_work_allowed: bool,
    system_entry_in_progress: bool,
    arcade_scroll_active: bool,
    arcade_turbo_active: bool,
) -> bool {
    background_work_allowed
        || system_entry_in_progress
        || arcade_scroll_active
        || arcade_turbo_active
}

fn initial_system_entry_reader_required(
    capsule_seed_ready: bool,
    sharded_seed_ready: bool,
) -> bool {
    capsule_seed_ready || sharded_seed_ready
}

fn configure_arcade_list_renderer_geometry(
    renderer: &mut ArcadeListRenderer,
    nav: &LauncherNav,
    ui: &UiDisplay,
) {
    let (geometry, visible_height) = arcade_list_layout(nav, ui);
    renderer.set_geometry_for_visible_height(geometry, visible_height);
    renderer.set_favourite_launch_refs_if_changed(
        nav.favourite_launch_refs_revision(),
        nav.favourite_launch_refs(),
    );
}

fn navigation_home_endpoint_is_live(
    route: Option<NavigationTransitionRoute>,
    request: Option<NavigationTransitionRequest>,
    endpoint: Option<NavigationTransitionEndpoint>,
) -> bool {
    let Some(request) = request else {
        return false;
    };
    request.direction == NavigationTransitionDirection::Reverse
        && endpoint == Some(NavigationTransitionEndpoint::Destination)
        && matches!(
            (route, request.renderer_label()),
            (
                Some(NavigationTransitionRoute::HomeToSettings),
                "settings-cog"
            ) | (
                Some(
                    NavigationTransitionRoute::HomeToArcade
                        | NavigationTransitionRoute::ConsolesToSystem
                ),
                "device-card"
            )
        )
}

fn settings_navigation_source_candidate(
    nav: &LauncherNav,
    event: Option<&crate::input_event::InputEvent>,
) -> bool {
    nav.pending_settings_activation() || settings_navigation_input_candidate(nav.screen, event)
}

fn settings_navigation_input_candidate(
    screen: Screen,
    event: Option<&crate::input_event::InputEvent>,
) -> bool {
    let Some(event) = event.filter(|event| event.phase == crate::input_event::InputPhase::Pressed)
    else {
        return false;
    };
    let activated = event.action == crate::input_event::LogicalAction::Activate;
    let backed = event.action == crate::input_event::LogicalAction::Back;
    let went_home = event.action == crate::input_event::LogicalAction::Home;
    match screen {
        Screen::Home => activated || went_home,
        Screen::Settings | Screen::About | Screen::Licenses | Screen::LicenseText => {
            activated || backed || went_home
        }
        Screen::Controller | Screen::Arcade => false,
    }
}

fn route_lifecycle_dialog_input(
    event: Option<&crate::input_event::InputEvent>,
    launch_failure_visible: bool,
    recovery_dialog_visible: bool,
) -> Option<LauncherLifecycleInput> {
    let event = event.filter(|event| event.phase == crate::input_event::InputPhase::Pressed)?;

    if launch_failure_visible {
        matches!(
            event.action,
            crate::input_event::LogicalAction::Activate
                | crate::input_event::LogicalAction::Back
                | crate::input_event::LogicalAction::Home
        )
        .then_some(LauncherLifecycleInput::LaunchFailureAcknowledge)
    } else if recovery_dialog_visible {
        match event.action {
            crate::input_event::LogicalAction::Left => {
                Some(LauncherLifecycleInput::CatalogRecoveryLeft)
            }
            crate::input_event::LogicalAction::Right => {
                Some(LauncherLifecycleInput::CatalogRecoveryRight)
            }
            crate::input_event::LogicalAction::Activate => {
                Some(LauncherLifecycleInput::CatalogRecoveryConfirm)
            }
            crate::input_event::LogicalAction::Back | crate::input_event::LogicalAction::Home => {
                Some(LauncherLifecycleInput::CatalogRecoveryCancel)
            }
            _ => None,
        }
    } else {
        None
    }
}

fn lifecycle_dialog_ui_inputs(
    action: &LauncherUiAction,
    launch_failure_visible: bool,
    recovery_dialog_visible: bool,
) -> Option<Vec<LauncherLifecycleInput>> {
    if launch_failure_visible {
        return matches!(
            action,
            LauncherUiAction::ChooseConfirmation(_)
                | LauncherUiAction::DismissOverlay
                | LauncherUiAction::Back
                | LauncherUiAction::Home
        )
        .then(|| vec![LauncherLifecycleInput::LaunchFailureAcknowledge]);
    }
    if !recovery_dialog_visible {
        return None;
    }
    match action {
        LauncherUiAction::ChooseConfirmation(slint_ui::launcher::DialogChoice::Cancel) => {
            Some(vec![
                LauncherLifecycleInput::CatalogRecoveryLeft,
                LauncherLifecycleInput::CatalogRecoveryConfirm,
            ])
        }
        LauncherUiAction::ChooseConfirmation(slint_ui::launcher::DialogChoice::Confirm) => {
            Some(vec![
                LauncherLifecycleInput::CatalogRecoveryRight,
                LauncherLifecycleInput::CatalogRecoveryConfirm,
            ])
        }
        LauncherUiAction::DismissOverlay | LauncherUiAction::Back | LauncherUiAction::Home => {
            Some(vec![LauncherLifecycleInput::CatalogRecoveryCancel])
        }
        _ => None,
    }
}

fn sync_navigation_transition_active(
    app: &slint_ui::launcher::Launcher,
    transition: &NavigationTransitionRuntime,
) {
    let active = transition.is_active();
    let navigation = app.global::<slint_ui::launcher::NavigationView>();
    let state = crate::launcher_view_types::navigation_transition_state(active);
    if navigation.get_transition_state() != state {
        navigation.set_transition_state(state);
    }
}

fn set_launcher_clock_text(app: &slint_ui::launcher::Launcher, value: &str) {
    app.global::<slint_ui::launcher::NavigationView>()
        .set_clock_text(value.into());
}

fn set_launcher_update_available(app: &slint_ui::launcher::Launcher, available: bool) {
    app.global::<slint_ui::launcher::NavigationView>()
        .set_update_available(available);
}

fn set_launcher_present_mode_label(app: &slint_ui::launcher::Launcher, value: &str) {
    let value = slint::SharedString::from(value);
    app.global::<slint_ui::launcher::NavigationView>()
        .set_present_mode_label(value.clone());
    app.global::<slint_ui::launcher::InformationView>()
        .set_present_mode_label(value);
}

fn collection_has_resident_rows(catalog: &ArcadeCatalog, collection_id: &str) -> bool {
    catalog.system_game_count(collection_id) > 0
}

fn empty_collection_invariant_violated(catalog: &ArcadeCatalog, nav: &LauncherNav) -> bool {
    nav.screen == Screen::Arcade
        && active_system(catalog, nav).is_some_and(|system| {
            system.count > 0
                && !collection_has_resident_rows(catalog, &system.id)
                && !nav.catalog_system_hydration_is_loading(&system.id)
                && !nav.catalog_system_hydration_has_failed(&system.id)
                && !nav.collection_is_scanning(&system.id)
        })
}

fn commit_pending_collection_entry(
    pending: &mut Option<PendingCollectionEntry>,
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    start: Instant,
) -> bool {
    let Some(entry) = pending.as_ref() else {
        return false;
    };
    if !collection_has_resident_rows(catalog, &entry.collection_id) {
        return false;
    }
    let entry = pending.take().expect("pending collection entry");
    nav.catalog_system_hydration_finished(&entry.collection_id);
    if !nav.activate_collection(catalog, &entry.collection_id) {
        return false;
    }
    if entry.open_game_list_directly {
        nav.skip_system_page(catalog);
    }
    print_startup_event(
        start,
        "catalog_system_entry_committed",
        format!(
            "system={} resident_rows={} pending_us={}",
            entry.collection_id,
            catalog.system_game_count(&entry.collection_id),
            entry.requested_at.elapsed().as_micros()
        ),
    );
    true
}

fn restore_failed_pending_collection_entry(
    pending: &mut Option<PendingCollectionEntry>,
    nav: &mut LauncherNav,
    start: Instant,
) -> bool {
    let Some(entry) = pending
        .as_ref()
        .filter(|entry| nav.catalog_system_hydration_has_failed(&entry.collection_id))
    else {
        return false;
    };
    let collection_id = entry.collection_id.clone();
    let entry = pending.take().expect("failed pending collection entry");
    nav.restore_pending_home_view(entry.source);
    print_startup_event(
        start,
        "catalog_system_entry_failed",
        format!("system={collection_id}"),
    );
    true
}

fn cancel_pending_collection_entry_for_input(
    pending: &mut Option<PendingCollectionEntry>,
    nav: &mut LauncherNav,
    event: Option<&crate::input_event::InputEvent>,
    start: Instant,
) -> bool {
    if !event.is_some_and(|event| {
        event.phase == crate::input_event::InputPhase::Pressed
            && matches!(
                event.action,
                crate::input_event::LogicalAction::Back | crate::input_event::LogicalAction::Home
            )
    }) {
        return false;
    }
    let Some(entry) = pending.take() else {
        return false;
    };
    nav.catalog_system_hydration_finished(&entry.collection_id);
    print_startup_event(
        start,
        "catalog_system_entry_cancelled",
        format!("system={} reason=back-or-home", entry.collection_id),
    );
    true
}

/// One system entry, from the press that opens it to the first frame that shows its rows and
/// exact preview. Preview work stays off until that frame is out, so it cannot delay it.
#[derive(Default)]
struct SystemEntryAdoption {
    entered: bool,
    rows_ready: bool,
    preview_exact: bool,
    destination_prepared: bool,
    ready_presented: bool,
}

impl SystemEntryAdoption {
    fn cancel(&mut self) {
        *self = Self::default();
    }

    fn note_enter(&mut self) {
        self.entered = true;
    }

    fn note_rows_ready(&mut self) {
        if self.entered {
            self.rows_ready = true;
        }
    }

    fn note_preview(&mut self, nav: &LauncherNav, catalog: &ArcadeCatalog, preview: &PreviewState) {
        if !self.entered || self.preview_exact || !self.rows_ready {
            return;
        }
        self.preview_exact = if selected_arcade_game_has_preview(nav, catalog) {
            preview.trace_cache_state() == "exact"
        } else {
            preview.terminal_empty()
        };
    }

    /// The first frame carrying the rows and the exact preview.
    fn note_destination_frame(&mut self, screen: Screen, copied_rows: u32) {
        if self.entered
            && self.rows_ready
            && self.preview_exact
            && screen == Screen::Arcade
            && copied_rows > 0
        {
            self.destination_prepared = true;
        }
    }

    /// The same frame once the hardware confirms it.
    fn note_ready_frame(&mut self, screen: Screen, copied_rows: u32, main_active_confirmed: bool) {
        if self.destination_prepared
            && self.entered
            && self.rows_ready
            && self.preview_exact
            && screen == Screen::Arcade
            && copied_rows > 0
            && main_active_confirmed
        {
            self.ready_presented = true;
        }
    }

    fn preview_adoption_in_progress(&self) -> bool {
        self.entered && self.rows_ready && !self.ready_presented
    }
}

fn should_defer_arcade_overlay_bridge(
    launching: bool,
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
) -> bool {
    !launching
        && nav.screen == Screen::Arcade
        && !nav.arcade_search.is_active(&nav.arcade_filter.active)
        && !active_system_game_view(catalog, nav).is_empty()
}

pub(super) struct LauncherStatusTextSnapshot {
    pub(super) catalog_scan_message: SharedString,
    pub(super) catalog_scan_title: SharedString,
    pub(super) catalog_scan_detail: SharedString,
    pub(super) confirm_title: SharedString,
    pub(super) confirm_message: SharedString,
    pub(super) confirm_left_label: SharedString,
    pub(super) confirm_right_label: SharedString,
}

impl LauncherStatusTextSnapshot {
    fn from_views(
        catalog: &slint_ui::launcher::CatalogView<'_>,
        overlay: &slint_ui::launcher::OverlayView<'_>,
    ) -> Self {
        Self {
            catalog_scan_message: catalog.get_message(),
            catalog_scan_title: catalog.get_title(),
            catalog_scan_detail: catalog.get_detail(),
            confirm_title: overlay.get_confirmation_title(),
            confirm_message: overlay.get_confirmation_message(),
            confirm_left_label: overlay.get_cancel_label(),
            confirm_right_label: overlay.get_confirm_label(),
        }
    }

    fn bytes_len(&self) -> usize {
        self.catalog_scan_message.len()
            + self.catalog_scan_title.len()
            + self.catalog_scan_detail.len()
            + self.confirm_title.len()
            + self.confirm_message.len()
            + self.confirm_left_label.len()
            + self.confirm_right_label.len()
    }
}

#[cfg(test)]
fn pad_state_with(set: impl FnOnce(&mut PadState)) -> PadState {
    let mut state = PadState::default();
    set(&mut state);
    state
}

#[cfg(test)]
fn normalized_test_press(
    action: crate::input_event::LogicalAction,
) -> crate::input_event::InputEvent {
    crate::input_event::InputEvent {
        source: crate::input_event::InputSourceId {
            kind: crate::input_event::InputSourceKind::Preview,
            instance: 1,
        },
        source_epoch: crate::input_event::SourceEpoch(1),
        sequence: 1,
        press_id: crate::input_event::PressId(1),
        captured_at_us: 1,
        action,
        phase: crate::input_event::InputPhase::Pressed,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct LauncherRenderIntent {
    first_visible_copy_done: bool,
    startup_input_enabled: bool,
    wake_reasons: LauncherWakeReasons,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LauncherWakeReasons(u64);

impl LauncherWakeReasons {
    const REDRAW_PENDING: Self = Self(1 << 0);
    const LAUNCHING: Self = Self(1 << 1);
    const SETUP_ACTIVE: Self = Self(1 << 2);
    const TOOLING_SEQUENCE_ACTIVE: Self = Self(1 << 4);
    const ROUTE_FORCES_FULL_PRESENT: Self = Self(1 << 5);
    const BRIDGE_DIRTY: Self = Self(1 << 6);
    const CATALOG_MESSAGES_ACTIVE: Self = Self(1 << 7);
    const MEDIA_MESSAGE_SEEN: Self = Self(1 << 8);
    const SLINT_ANIMATION_ACTIVE: Self = Self(1 << 13);
    const HOME_PAN_PRESENT_ACTIVE: Self = Self(1 << 14);
    const ARCADE_VISUAL_CHANGED_THIS_LOOP: Self = Self(1 << 15);
    const ARCADE_SCROLL_ACTIVE: Self = Self(1 << 16);
    const ARCADE_FILTER_SCROLL_ACTIVE: Self = Self(1 << 17);
    const ARCADE_SEARCH_ACTIVE: Self = Self(1 << 18);
    const PREVIEW_DIRTY: Self = Self(1 << 19);
    const PREVIEW_SCHEDULED_THIS_LOOP: Self = Self(1 << 20);
    const COMPOSITION_FORCES_FULL_PRESENT: Self = Self(1 << 21);
    const COMPOSITION_CLEARS_DIRECT_LAYERS: Self = Self(1 << 22);
    const HOME_HORIZONTAL_INPUT_HELD: Self = Self(1 << 23);
    const LATENCY_CRITICAL_INPUT: Self = Self(1 << 25);
    const CRT_BACKDROP_PREPARED: Self = Self(1 << 26);

    #[inline]
    fn insert_if(&mut self, reason: Self, active: bool) {
        if active {
            self.0 |= reason.0;
        }
    }

    #[inline]
    fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for LauncherWakeReasons {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl LauncherRenderIntent {
    fn can_sleep(self) -> bool {
        self.first_visible_copy_done && self.startup_input_enabled && self.wake_reasons.is_empty()
    }
}

fn screensaver_pipeline_start_allowed(screensaver_active: bool, ram_pipeline_active: bool) -> bool {
    screensaver_active && !ram_pipeline_active
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherBridgeSyncPlan {
    None,
    Full,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupIntroLauncherUiPlan {
    Suppress,
    PrepareLiveFrame,
    Interactive,
}

fn startup_intro_launcher_ui_plan(
    intro_active: bool,
    reveal_state: StartupRevealState,
    live_frame_ready: bool,
) -> StartupIntroLauncherUiPlan {
    if !intro_active {
        StartupIntroLauncherUiPlan::Interactive
    } else if reveal_state == StartupRevealState::RevealLauncher && !live_frame_ready {
        StartupIntroLauncherUiPlan::PrepareLiveFrame
    } else {
        StartupIntroLauncherUiPlan::Suppress
    }
}

fn startup_catalog_ready_for_reveal(
    intro_active: bool,
    catalog_ready: bool,
    refresh_done: bool,
) -> bool {
    catalog_ready && (!intro_active || refresh_done)
}

fn launcher_bridge_sync_plan(
    launching: bool,
    full_bridge_dirty: bool,
    light_bridge_dirty: bool,
) -> LauncherBridgeSyncPlan {
    if launching {
        LauncherBridgeSyncPlan::None
    } else if full_bridge_dirty {
        LauncherBridgeSyncPlan::Full
    } else if light_bridge_dirty {
        LauncherBridgeSyncPlan::Light
    } else {
        LauncherBridgeSyncPlan::None
    }
}

const HOME_PAN_PRESENT_DURATION: Duration = Duration::from_millis(190);
const CATALOG_SCAN_BLINK_HALF_PERIOD: Duration = Duration::from_millis(500);
const HOME_LAYOUT_PADDING: usize = 18;
const HOME_HEADER_H: usize = 42;
const HOME_LAYOUT_SPACING: usize = 14;
const HOME_FOOTER_H: usize = 30;

fn update_home_pan_present_window(
    screen: Screen,
    scroll_x: i32,
    last_scroll_x: &mut i32,
    present_until: &mut Option<Instant>,
    now: Instant,
) -> bool {
    if screen != Screen::Home {
        *last_scroll_x = scroll_x;
        *present_until = None;
        return false;
    }

    if scroll_x != *last_scroll_x {
        *last_scroll_x = scroll_x;
        *present_until = Some(now + HOME_PAN_PRESENT_DURATION);
    }

    let active = present_until.is_some_and(|deadline| now <= deadline);
    if !active {
        *present_until = None;
    }
    active
}

fn home_pan_present_rect(ui: &UiDisplay) -> DirtyRect {
    let x0 = HOME_LAYOUT_PADDING;
    let y0 = HOME_LAYOUT_PADDING + HOME_HEADER_H + HOME_LAYOUT_SPACING;
    let x1 = ui.render_w().saturating_sub(HOME_LAYOUT_PADDING);
    let y1 = ui
        .render_h()
        .saturating_sub(HOME_LAYOUT_PADDING + HOME_LAYOUT_SPACING + HOME_FOOTER_H);
    DirtyRect {
        x0: x0.min(ui.render_w()),
        y0: y0.min(ui.render_h()),
        x1: x1.max(x0).min(ui.render_w()),
        y1: y1.max(y0).min(ui.render_h()),
    }
}

fn expand_home_pan_dirty_rect(
    dirty: Option<DirtyRect>,
    ui: &UiDisplay,
    home_pan_present_active: bool,
) -> Option<DirtyRect> {
    if !home_pan_present_active {
        return dirty;
    }
    let band = home_pan_present_rect(ui);
    Some(dirty.map_or(band, |rect| rect.union(band)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CatalogWorkMode {
    Cpu0,
    Paused,
    DualCoreBurst,
}

fn launcher_max_sleep_duration(frame_period_us: u64) -> Duration {
    Duration::from_micros(frame_period_us.max(1))
}

fn launcher_idle_sleep_duration(pacer: &VsyncPacer) -> Duration {
    let frame_period = launcher_max_sleep_duration(pacer.period_us());
    slint::platform::duration_until_next_timer_update()
        .map_or(frame_period, |timer| frame_period.min(timer))
}

fn launcher_catalog_work_mode(
    first_visible: bool,
    interaction_active: bool,
    visible_animation_active: bool,
    now: Instant,
    idle_candidate_since: &mut Option<Instant>,
) -> CatalogWorkMode {
    if interaction_active {
        *idle_candidate_since = None;
        return CatalogWorkMode::Paused;
    }
    // Before the launcher becomes interactive there is no scroll latency to
    // protect. Give incomplete first-run catalog work both A9 cores, even
    // while the intro is visible, so Arcade and the remaining systems become
    // usable as soon as possible. Once input is enabled, visible motion keeps
    // catalog work on CPU0 and actual interaction parks it completely.
    if !first_visible {
        *idle_candidate_since = None;
        return CatalogWorkMode::DualCoreBurst;
    }
    if visible_animation_active {
        *idle_candidate_since = None;
        return CatalogWorkMode::Cpu0;
    }
    let idle_since = idle_candidate_since.get_or_insert(now);
    if now.saturating_duration_since(*idle_since) >= CATALOG_IDLE_BURST_SETTLE {
        CatalogWorkMode::DualCoreBurst
    } else {
        CatalogWorkMode::Cpu0
    }
}

#[derive(Debug)]
struct CatalogWorkModeTelemetry {
    mode: CatalogWorkMode,
    changed_at: Instant,
    cpu0_us: u64,
    paused_us: u64,
    burst_us: u64,
    transitions: u64,
}

impl CatalogWorkModeTelemetry {
    fn new(now: Instant) -> Self {
        Self {
            mode: CatalogWorkMode::Cpu0,
            changed_at: now,
            cpu0_us: 0,
            paused_us: 0,
            burst_us: 0,
            transitions: 0,
        }
    }

    fn observe(&mut self, mode: CatalogWorkMode, now: Instant) -> bool {
        if mode == self.mode {
            return false;
        }
        self.account(now);
        self.mode = mode;
        self.changed_at = now;
        self.transitions = self.transitions.saturating_add(1);
        true
    }

    fn account(&mut self, now: Instant) {
        let elapsed = u64::try_from(now.saturating_duration_since(self.changed_at).as_micros())
            .unwrap_or(u64::MAX);
        match self.mode {
            CatalogWorkMode::Cpu0 => self.cpu0_us = self.cpu0_us.saturating_add(elapsed),
            CatalogWorkMode::Paused => self.paused_us = self.paused_us.saturating_add(elapsed),
            CatalogWorkMode::DualCoreBurst => self.burst_us = self.burst_us.saturating_add(elapsed),
        }
        self.changed_at = now;
    }
}

#[derive(Debug)]
struct CatalogScanBlink {
    dot_visible: bool,
    next_toggle_at: Option<Instant>,
}

impl Default for CatalogScanBlink {
    fn default() -> Self {
        Self {
            dot_visible: true,
            next_toggle_at: None,
        }
    }
}

impl CatalogScanBlink {
    fn update(&mut self, catalog_building: bool, now: Instant) -> Option<bool> {
        if !catalog_building {
            self.next_toggle_at = None;
            if !self.dot_visible {
                self.dot_visible = true;
                return Some(true);
            }
            return None;
        }

        if self.next_toggle_at.is_none() {
            self.next_toggle_at = Some(now + CATALOG_SCAN_BLINK_HALF_PERIOD);
            if !self.dot_visible {
                self.dot_visible = true;
                return Some(true);
            }
            return None;
        }

        if self.next_toggle_at.is_some_and(|deadline| now >= deadline) {
            self.dot_visible = !self.dot_visible;
            self.next_toggle_at = Some(now + CATALOG_SCAN_BLINK_HALF_PERIOD);
            return Some(self.dot_visible);
        }

        None
    }

    fn time_until_toggle(&self, now: Instant) -> Option<Duration> {
        self.next_toggle_at
            .map(|deadline| deadline.saturating_duration_since(now))
    }
}

#[allow(clippy::too_many_arguments)]
fn can_preempt_home_latch_wait(
    screen: Screen,
    feedback_frame_stamped: bool,
    transition_active: bool,
    screensaver_active: bool,
    direct_layer_state_active: bool,
    preview_commit_pending: bool,
    startup_intro_frame_posted: bool,
) -> bool {
    screen == Screen::Home
        && !feedback_frame_stamped
        && !transition_active
        && !screensaver_active
        && !direct_layer_state_active
        && !preview_commit_pending
        && !startup_intro_frame_posted
}

/// Loop restart after new input arrives between rendering and publication.
/// Waiting for a slot has not consumed Slint damage and must stay on the direct path.
pub(in crate::ui_runner) fn restart_unpublished_home_frame(
    completed: &mut Option<CompletedHiddenFrame>,
    slint_rasterized: bool,
    unpublished_cached_frame_present: &mut bool,
    discard: impl FnOnce(CompletedHiddenFrame),
) {
    if let Some(completed) = completed.take() {
        discard(completed);
    }
    *unpublished_cached_frame_present |= slint_rasterized;
}

#[allow(clippy::too_many_arguments)]
fn can_preempt_disposable_home_raster(
    screen: Screen,
    current_batch_empty: bool,
    latency_critical_frame_pending: bool,
    input_changed_since_drain: bool,
    transition_active: bool,
    screensaver_active: bool,
    direct_layer_state_active: bool,
    startup_intro_active: bool,
) -> bool {
    screen == Screen::Home
        && should_restart_for_urgent_input(
            current_batch_empty,
            latency_critical_frame_pending,
            input_changed_since_drain,
        )
        && !transition_active
        && !screensaver_active
        && !direct_layer_state_active
        && !startup_intro_active
}

fn should_restart_for_urgent_input(
    current_batch_empty: bool,
    latency_critical_frame_pending: bool,
    input_changed_since_drain: bool,
) -> bool {
    current_batch_empty && !latency_critical_frame_pending && input_changed_since_drain
}

fn pad_state_has_active_input(state: &PadState) -> bool {
    state.dpad_up
        || state.dpad_down
        || state.dpad_left
        || state.dpad_right
        || state.btn_a
        || state.btn_b
        || state.btn_x
        || state.btn_y
        || state.btn_l
        || state.btn_r
        || state.btn_zl
        || state.btn_zr
        || state.btn_select
        || state.btn_start
        || state.btn_l3
        || state.btn_r3
        || state.btn_home
        || state.btn_capture
}

fn direct_preview_requested(
    screen: Screen,
    memory_guard_active: bool,
    raw_transition_available: bool,
) -> bool {
    screen == Screen::Arcade && !memory_guard_active && raw_transition_available
}

fn pad_state_home_horizontal_held(state: &PadState) -> bool {
    state.dpad_left || state.dpad_right
}

fn home_frame_driven_redraw_active(
    screen: Screen,
    home_pan_present_active: bool,
    home_horizontal_input_held: bool,
) -> bool {
    screen == Screen::Home && (home_pan_present_active || home_horizontal_input_held)
}

fn frame_production_class(
    screensaver_active: bool,
    home_motion_active: bool,
    navigation_transition_active: bool,
) -> FrameProductionClass {
    if screensaver_active {
        FrameProductionClass::Prepared
    } else if home_motion_active || navigation_transition_active {
        FrameProductionClass::SynchronousAnimation
    } else {
        FrameProductionClass::EventDriven
    }
}

fn latch_late_start_wait_enabled(
    latch_backend_active: bool,
    production_class: FrameProductionClass,
    latency_critical_input: bool,
) -> bool {
    !latency_critical_input
        && !(latch_backend_active && production_class == FrameProductionClass::SynchronousAnimation)
}

fn retain_or_defer_screensaver_buffer(
    launcher_frame: &mut Option<Vec<Rgb565Pixel>>,
    recycle_after_present: &mut Option<Vec<Rgb565Pixel>>,
    displaced: Vec<Rgb565Pixel>,
) {
    if launcher_frame.is_none() {
        *launcher_frame = Some(displaced);
    } else {
        debug_assert!(recycle_after_present.is_none());
        *recycle_after_present = Some(displaced);
    }
}

fn visible_frame_was_presented(
    copied_rows: u32,
    status: LauncherPresentStatus,
    copy_path: &str,
) -> bool {
    copied_rows > 0
        || (status == LauncherPresentStatus::Ok
            && copy_path == LatchCopyPath::ExternalDirect.label())
}

fn read_sharded_registry_seed(
    root: &str,
    storage: &Path,
    start: Instant,
) -> Option<ShardedCatalogSeed> {
    let load_started = Instant::now();
    match load_sharded_registry_seed_at(root, storage) {
        Ok(seed) => {
            print_startup_event(
                start,
                "catalog_registry_load",
                format!(
                    "status=ready elapsed_us={} path={} generation={} systems={}",
                    load_started.elapsed().as_micros(),
                    storage.display(),
                    seed.generation,
                    seed.catalog.systems.len()
                ),
            );
            Some(seed)
        }
        Err(error) if error.status == "empty" => None,
        Err(error) => {
            print_startup_event(
                start,
                "catalog_registry_load",
                format!(
                    "status={} elapsed_us={} path={} error={error}",
                    error.status,
                    load_started.elapsed().as_micros(),
                    storage.display()
                ),
            );
            None
        }
    }
}

#[derive(Default)]
struct CatalogGenerationState {
    current: Option<String>,
    durable: Option<String>,
}

impl CatalogGenerationState {
    fn publish(&mut self, fingerprint: Option<String>, durable: bool) {
        self.current = fingerprint;
        self.durable = durable.then(|| self.current.clone()).flatten();
    }
}

fn initialize_catalog_generation(
    scheduler: &mut LauncherScheduler,
    fingerprint: Option<String>,
) -> CatalogGenerationState {
    let generation = CatalogGenerationState {
        current: fingerprint.clone(),
        durable: fingerprint,
    };
    let _ = scheduler.set_system_shard_generation(generation.current.as_deref());
    generation
}

fn request_system_shard_hydration(
    scheduler: &mut LauncherScheduler,
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    catalog_version: usize,
    system_id: &str,
    reason: &'static str,
    now: Instant,
) -> bool {
    if !scheduler.request_system_shard(
        system_id.to_string(),
        reason,
        catalog.clone(),
        catalog_version,
        now,
    ) {
        return false;
    }
    nav.catalog_system_hydration_started(system_id);
    true
}

fn retry_system_shard_hydration(
    scheduler: &mut LauncherScheduler,
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    catalog_version: usize,
    system_id: &str,
    reason: &'static str,
    now: Instant,
) -> bool {
    if !scheduler.retry_system_shard(
        system_id.to_string(),
        reason,
        catalog.clone(),
        catalog_version,
        now,
    ) {
        return false;
    }
    nav.catalog_system_hydration_started(system_id);
    true
}

struct ColdCollectionEntryStart {
    pending: Option<PendingCollectionEntry>,
    bridge_dirty: bool,
}

#[allow(clippy::too_many_arguments)]
fn begin_cold_collection_entry(
    scheduler: &mut LauncherScheduler,
    nav: &mut LauncherNav,
    preview: &mut PreviewState,
    catalog: &ArcadeCatalog,
    catalog_version: usize,
    collection_id: &str,
    requested_at: Instant,
    trace_source: &'static str,
    open_game_list_directly: bool,
    system_entry: &mut SystemEntryAdoption,
    start: Instant,
) -> ColdCollectionEntryStart {
    // The Arcade shell exists independently of installed games. Do not ask
    // the shard scheduler to read a file for an unregistered/zero-row library.
    if collection_id == arcade_catalog::MENU_ARCADE_SYSTEM_ID
        && nav.collection_declared_count(collection_id) == 0
    {
        return ColdCollectionEntryStart {
            pending: None,
            bridge_dirty: true,
        };
    }
    let hydration_failed = nav.catalog_system_hydration_has_failed(collection_id);
    let already_loading = nav.catalog_system_hydration_is_loading(collection_id);
    let preview_dispatch = (!already_loading).then(|| {
        let (requests, generation) = preview.reserve_system_entry_preview();
        SystemEntryPreviewDispatch {
            generation,
            requests,
        }
    });
    let hydration_requested = if already_loading {
        false
    } else if hydration_failed {
        scheduler.retry_system_shard_with_preview(
            collection_id.to_string(),
            "explicit-retry",
            catalog.clone(),
            catalog_version,
            requested_at,
            preview_dispatch,
        )
    } else {
        scheduler.request_system_shard_with_preview(
            collection_id.to_string(),
            "open-collection",
            catalog.clone(),
            catalog_version,
            requested_at,
            preview_dispatch,
        )
    };
    if !hydration_requested && !already_loading {
        preview.cancel_system_entry_preview();
    }
    if hydration_requested {
        nav.catalog_system_hydration_started(collection_id);
    }
    if collection_id == arcade_catalog::MENU_ARCADE_SYSTEM_ID {
        if !hydration_requested && !already_loading && !nav.collection_is_scanning(collection_id) {
            nav.catalog_system_hydration_failed(collection_id);
        }
        // Navigation commits now; hydration only updates the open screen.
        // In particular a late result must never reopen Arcade after Back.
        return ColdCollectionEntryStart {
            pending: None,
            bridge_dirty: true,
        };
    }
    let pending = (hydration_requested || nav.catalog_system_hydration_is_loading(collection_id))
        .then(|| {
            system_entry.note_enter();
            print_startup_event(
                start,
                "catalog_system_entry_pending",
                format!("system={collection_id} source={trace_source}"),
            );
            PendingCollectionEntry {
                collection_id: collection_id.to_string(),
                requested_at,
                source: nav.home_view_state(),
                open_game_list_directly,
            }
        });
    ColdCollectionEntryStart {
        pending,
        bridge_dirty: hydration_failed && hydration_requested,
    }
}

fn request_pending_launch_return_shard(
    pending: Option<&launcher::LaunchReturnState>,
    catalog: &ArcadeCatalog,
    catalog_version: usize,
    nav: &mut LauncherNav,
    scheduler: &mut LauncherScheduler,
    now: Instant,
    start: Instant,
) -> bool {
    let Some(state) = pending else {
        return false;
    };
    let collection_id = state.collection_id().unwrap_or_else(|| state.system_id());
    if catalog
        .system_game_view(collection_id)
        .iter()
        .any(|game| game.mra_path.as_ref() == state.game_path())
    {
        return false;
    }
    let system_id = state.system_id();
    if !catalog.systems.iter().any(|system| system.id == system_id) {
        return false;
    }
    if !request_system_shard_hydration(
        scheduler,
        nav,
        catalog,
        catalog_version,
        system_id,
        "launch-return",
        now,
    ) {
        return false;
    }
    print_startup_event(
        start,
        "launch_return_system_shard_requested",
        format!("system={system_id}"),
    );
    true
}

fn startup_intro_catalog_worker_request(request: CatalogWorkerRequest) -> CatalogWorkerRequest {
    if request == CatalogWorkerRequest::FreshBuild {
        CatalogWorkerRequest::FreshBuild
    } else {
        // Missing-cache planning maps CheckStamp to InitialBuild, preserving
        // first-visible Arcade publication before the authoritative full scan.
        CatalogWorkerRequest::CheckStamp
    }
}

fn catalog_taxonomy_sync_required(catalog_ready: bool, source: CatalogSource) -> bool {
    !(catalog_ready && source == CatalogSource::NavigationProjection)
}

fn catalog_for_ready_source(
    nav: &mut LauncherNav,
    catalog: ArcadeCatalog,
    source: CatalogSource,
) -> ArcadeCatalog {
    if source == CatalogSource::ShardedRegistry {
        nav.catalog_build_finished(&catalog);
        catalog
    } else {
        nav.catalog_with_build_shells(catalog)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectiveLauncherView {
    Launching,
    Screensaver,
    Navigation(Screen),
}

impl EffectiveLauncherView {
    fn resolve(
        lifecycle: &LauncherLifecycle,
        screensaver_active: bool,
        return_screen: Screen,
    ) -> Self {
        Self::resolve_state(lifecycle.state(), screensaver_active, return_screen)
    }

    fn resolve_state(
        lifecycle: &LauncherLifecycleState,
        screensaver_active: bool,
        return_screen: Screen,
    ) -> Self {
        if matches!(
            lifecycle,
            LauncherLifecycleState::Launching { .. } | LauncherLifecycleState::Handoff { .. }
        ) {
            Self::Launching
        } else if screensaver_active {
            Self::Screensaver
        } else {
            Self::Navigation(return_screen)
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Launching => "launching",
            Self::Screensaver => "screensaver",
            Self::Navigation(screen) => screen_label(screen),
        }
    }

    const fn launch_active(self) -> bool {
        matches!(self, Self::Launching)
    }

    const fn accepts_application_input(self) -> bool {
        matches!(self, Self::Screensaver | Self::Navigation(_))
    }

    pub(super) const fn return_screen(self) -> Option<Screen> {
        match self {
            Self::Navigation(screen) => Some(screen),
            Self::Launching | Self::Screensaver => None,
        }
    }
}

#[cfg(test)]
fn screensaver_start_mode(
    idle_when_ready: bool,
    preview_when_ready: bool,
    legacy_start_active: bool,
) -> ScreensaverStartMode {
    if preview_when_ready {
        ScreensaverStartMode::PreviewWhenReady
    } else if idle_when_ready {
        ScreensaverStartMode::IdleWhenReady
    } else if legacy_start_active {
        ScreensaverStartMode::PreviewWhenReady
    } else {
        ScreensaverStartMode::Inactive
    }
}

fn screensaver_preview_start_ready(
    content_ready: bool,
    wait_for_analytics: bool,
    analytics_mode: FrameAnalyticsMode,
) -> bool {
    content_ready && (!wait_for_analytics || analytics_mode == FrameAnalyticsMode::Process)
}

#[derive(Debug)]
struct ScreensaverControl {
    last_activity: Instant,
    active: bool,
    start_mode: ScreensaverStartMode,
    preview_active: bool,
    waiting_for_input_release: bool,
    restore_full_frame: bool,
    /// Frames the preview fade has been shown for; `None` when no fade runs.
    preview_fade_frames: Option<u32>,
    reactivation_suppressed: bool,
    timeline: ScreensaverStartupTimeline,
}

impl ScreensaverControl {
    fn new(now: Instant, start_mode: ScreensaverStartMode) -> Self {
        Self {
            last_activity: now,
            active: false,
            start_mode,
            preview_active: false,
            waiting_for_input_release: false,
            restore_full_frame: false,
            preview_fade_frames: None,
            reactivation_suppressed: false,
            timeline: ScreensaverStartupTimeline::default(),
        }
    }

    fn update(
        &mut self,
        now: Instant,
        enabled: bool,
        delay: Duration,
        catalog_busy: bool,
        preview_ready: bool,
    ) {
        match self.start_mode {
            ScreensaverStartMode::PreviewWhenReady => {
                if preview_ready {
                    self.preview(now);
                } else {
                    self.last_activity = now;
                    self.active = false;
                }
            }
            ScreensaverStartMode::IdleWhenReady => {
                if catalog_busy {
                    self.last_activity = now;
                    self.active = false;
                } else {
                    self.active = true;
                    self.start_mode = ScreensaverStartMode::Inactive;
                    self.waiting_for_input_release = false;
                }
            }
            ScreensaverStartMode::Inactive => {
                if catalog_busy && !self.preview_active {
                    self.restore_full_frame |= self.active;
                    self.last_activity = now;
                    self.active = false;
                    self.preview_fade_frames = None;
                } else if enabled
                    && !self.reactivation_suppressed
                    && now.saturating_duration_since(self.last_activity) >= delay
                {
                    self.active = true;
                    self.waiting_for_input_release = false;
                }
            }
        }
    }

    fn preview(&mut self, now: Instant) {
        self.active = true;
        self.start_mode = ScreensaverStartMode::Inactive;
        self.preview_active = true;
        self.waiting_for_input_release = true;
        self.last_activity = now;
        self.preview_fade_frames = Some(0);
        self.reactivation_suppressed = false;
        self.timeline.begin(now);
    }

    #[cfg(test)]
    fn is_preview(&self) -> bool {
        self.preview_active
    }

    fn input_held_for_control(&self, screensaver_wake: bool, physical_input_held: bool) -> bool {
        screensaver_wake || (self.preview_active && physical_input_held)
    }

    fn cancel_for_exclusive_view(&mut self, now: Instant) -> bool {
        let was_active = self.active || self.start_mode != ScreensaverStartMode::Inactive;
        self.restore_full_frame |= self.active;
        self.active = false;
        self.start_mode = ScreensaverStartMode::Inactive;
        self.preview_active = false;
        self.waiting_for_input_release = false;
        self.preview_fade_frames = None;
        self.last_activity = now;
        was_active
    }

    /// Returns true when this input frame is consumed by screensaver control.
    fn handle_input(&mut self, now: Instant, input_held: bool, user_activity: bool) -> bool {
        if self.active && self.waiting_for_input_release {
            if !input_held {
                self.waiting_for_input_release = false;
            }
            return true;
        }
        if self.active && user_activity {
            self.active = false;
            self.preview_active = false;
            self.restore_full_frame = true;
            self.last_activity = now;
            self.preview_fade_frames = None;
            return true;
        }
        if user_activity {
            self.last_activity = now;
            self.reactivation_suppressed = false;
        }
        false
    }

    fn fail_current_activation(&mut self, now: Instant) {
        self.restore_full_frame |= self.active;
        self.active = false;
        self.start_mode = ScreensaverStartMode::Inactive;
        self.preview_active = false;
        self.waiting_for_input_release = false;
        self.preview_fade_frames = None;
        self.reactivation_suppressed = true;
        self.last_activity = now;
    }

    fn take_restore_full_frame(&mut self) -> bool {
        std::mem::take(&mut self.restore_full_frame)
    }

    /// Fade opacity for the frame being produced. Call once per produced
    /// frame: each call is one display period further into the fade.
    fn preview_fade_alpha(&mut self, frame_period: Duration) -> Option<u8> {
        const PREVIEW_FADE_DURATION: Duration = Duration::from_millis(200);
        let frames = self.preview_fade_frames.as_mut()?;
        let elapsed = frame_period.saturating_mul(*frames);
        *frames = frames.saturating_add(1);
        Some(
            (elapsed.as_micros().min(PREVIEW_FADE_DURATION.as_micros()) * 255
                / PREVIEW_FADE_DURATION.as_micros()) as u8,
        )
    }
}

const fn screensaver_catalog_busy(worker_running: bool, refresh_done: bool) -> bool {
    worker_running || !refresh_done
}

fn replace_layout(
    layout: &mut UiLayoutGeometry,
    layout_epoch: &mut u64,
    next_layout: UiLayoutGeometry,
) -> bool {
    if next_layout == *layout {
        return false;
    }
    *layout_epoch = layout_epoch
        .checked_add(1)
        .expect("physical layout epoch exhausted");
    *layout = next_layout;
    true
}

fn sync_license_viewport(nav: &mut LauncherNav, layout: UiLayoutGeometry) {
    let content = layout.content_rect();
    let safe_x = content
        .x
        .max(layout.logical_w().saturating_sub(content.x + content.width));
    let safe_y = content.y.max(
        layout
            .logical_h()
            .saturating_sub(content.y + content.height),
    );
    nav.set_license_viewport_geometry(layout.logical_w(), layout.logical_h(), safe_x, safe_y);
}

#[allow(clippy::too_many_arguments)]
fn apply_orientation_layout(
    app: &slint_ui::launcher::Launcher,
    window: &Rc<MisterSoftwareWindow>,
    ui: &UiDisplay,
    orientation: ScreenOrientation,
    nav: &mut LauncherNav,
    layout: &mut UiLayoutGeometry,
    layout_epoch: &mut u64,
    navigation_transition: &mut NavigationTransitionRuntime,
) {
    nav.settings.screen_orientation = orientation;
    nav.sync_orientation_selection();
    let next_layout = UiLayoutGeometry::for_display(ui, orientation);
    replace_layout(layout, layout_epoch, next_layout);
    nav.set_portrait_layout(layout.is_portrait());
    sync_license_viewport(nav, *layout);
    if ui.output_route().is_crt() {
        let metrics = crate::ui_display::CrtUiMetrics::for_display(ui);
        nav.set_arcade_row_height(crt_arcade_row_height(
            metrics.game_row_height,
            layout.is_portrait(),
        ));
    }
    let mister_ui = app.global::<slint_ui::launcher::MisterUi>();
    mister_ui.set_window_width(layout.logical_w() as i32);
    mister_ui.set_window_height(layout.logical_h() as i32);
    mister_ui.set_screen_orientation(match orientation {
        ScreenOrientation::Normal => slint_ui::launcher::ScreenOrientation::Normal,
        ScreenOrientation::MonitorClockwise => {
            slint_ui::launcher::ScreenOrientation::MonitorClockwise
        }
        ScreenOrientation::MonitorCounterclockwise => {
            slint_ui::launcher::ScreenOrientation::MonitorCounterclockwise
        }
    });
    if ui.output_route().is_crt() {
        let content = layout.content_rect();
        mister_ui.set_crt_content_x(content.x as i32);
        mister_ui.set_crt_content_y(content.y as i32);
        mister_ui.set_crt_content_width(content.width as i32);
        mister_ui.set_crt_content_height(content.height as i32);
    }
    configure_window_layout(layout, window);
    navigation_transition.set_enabled(
        layout.logical_w(),
        layout.logical_h(),
        !nav.settings.reduce_motion,
    );
    window.request_redraw();
}

/// Stop the screensaver's render-ahead pipeline and keep it until it has
/// stopped, so a replacement never starts while the old one still runs.
fn retire_screensaver_pipeline(
    pipeline: &mut Option<ScreensaverRenderAhead>,
    retiring: &mut Vec<ScreensaverRenderAhead>,
) {
    if let Some(pipeline) = pipeline.take() {
        pipeline.cancel();
        retiring.push(pipeline);
    }
}

#[allow(clippy::too_many_arguments)]
fn begin_orientation_transition(
    app: &slint_ui::launcher::Launcher,
    window: &Rc<MisterSoftwareWindow>,
    ui: &UiDisplay,
    target: &UiFrameTarget,
    from: ScreenOrientation,
    to: ScreenOrientation,
    now: Instant,
    reduce_motion: bool,
    nav: &mut LauncherNav,
    layout: &mut UiLayoutGeometry,
    layout_epoch: &mut u64,
    director: &mut PresentationDirector,
    orientation_preparation_trace: &mut OrientationPreparationTrace,
    intent: OrientationIntent,
) -> bool {
    let begin_started = Instant::now();
    let source_snapshot_started = Instant::now();
    let Some(animated) =
        director.begin_orientation(from, to, target.cached_565(), now, reduce_motion, intent)
    else {
        return false;
    };
    let source_snapshot_us = source_snapshot_started.elapsed().as_micros();
    let layout_started = Instant::now();
    apply_orientation_layout(
        app,
        window,
        ui,
        to,
        nav,
        layout,
        layout_epoch,
        &mut director.navigation,
    );
    *orientation_preparation_trace = OrientationPreparationTrace {
        begin_us: begin_started.elapsed().as_micros(),
        source_snapshot_us,
        layout_us: layout_started.elapsed().as_micros(),
        source_snapshot_bytes: target.cached_565().len().saturating_mul(2) as u64,
    };
    if !animated {
        director.end_orientation();
    }
    animated
}

#[derive(Clone, Copy, Default)]
struct OrientationPreparationTrace {
    begin_us: u128,
    source_snapshot_us: u128,
    layout_us: u128,
    source_snapshot_bytes: u64,
}

fn render_immediate_launcher_frame(
    window: &MisterSoftwareWindow,
    target: &mut UiFrameTarget,
    layout: UiLayoutGeometry,
) -> Option<DirtyRect> {
    let mut layer_target = LayerTarget::new_oriented(target, layout);
    let (dirty, mut damage) = layer_target.render_slint_base(window);
    if damage.is_empty() {
        damage.push_if_some(dirty);
    }
    damage.iter().reduce(DirtyRect::union)
}

/// What the wait hands to the confirmed-present accounting.
struct LatchWaitOutcome {
    finish_timing: LauncherFrameFinishTraceTiming,
    #[cfg(feature = "tooling")]
    wait_start: Instant,
    pace: mister_magik_fb::framebuffer::vsync::VsyncPace,
    wait_done: Instant,
    readiness_post: Option<crate::ui_runner::launcher_readiness::ConfirmedLatchPost>,
}

#[cfg(feature = "tooling")]
/// What the tooling presentation metrics read and update for one confirmed present.
struct ToolingPresentation<'a> {
    app: &'a slint_ui::launcher::Launcher,
    card_direct_frame_rendered: bool,
    card_direct_measurement: &'a mut Option<(u64, u64, u64, u64)>,
    card_presentation_measurement_enabled: bool,
    card_work_timing: Option<mister_magik_tooling_support::measurement::FrameWorkTiming>,
    custom_draw_done: Instant,
    custom_draw_start: Instant,
    director: &'a PresentationDirector,
    f: &'a mut Fpga,
    frame_start_phase_us: u64,
    frame_t1: Instant,
    frame_t2: Instant,
    frame_t3: Instant,
    frame_t4: Instant,
    home_horizontal_input_held: bool,
    launcher_card_home: &'a Option<crate::ui_runner::launcher_card_home::LauncherCardHomeSession>,
    nav: &'a LauncherNav,
    navigation_endpoint_rendered: bool,
    navigation_transition_composition_active: bool,
    navigation_transition_renderer: &'static str,
    navigation_transition_route: &'static str,
    pacer: &'a VsyncPacer,
    post_timing: &'a Option<(Instant, Instant)>,
    pre_render_wait_us: u128,
    presented_frame: &'a LauncherPresentedFrame,
    run_start: Instant,
    screensaver: &'a ScreensaverControl,
    tooling: &'a mut Option<mister_magik_tooling_support::Session>,
    tooling_animation_active: bool,
    tooling_attempt_id: u64,
    tooling_drop_baseline: &'a mut Option<ToolingPresentationObservation>,
    tooling_frame_begin: Instant,
    tooling_frame_evidence:
        &'a mut Option<mister_magik_tooling_support::frame_evidence::FrameEvidence>,
    tooling_tick_us: u64,
    wait_done: Instant,
    wait_start: Instant,
}

#[cfg(feature = "tooling")]
/// Records the confirmed present into the tooling session's counters, drop accounting and
/// frame evidence.
fn record_tooling_presentation(ctx: ToolingPresentation<'_>) {
    let ToolingPresentation {
        app,
        card_direct_frame_rendered,
        card_direct_measurement,
        card_presentation_measurement_enabled,
        card_work_timing,
        custom_draw_done,
        custom_draw_start,
        director,
        f,
        frame_start_phase_us,
        frame_t1,
        frame_t2,
        frame_t3,
        frame_t4,
        home_horizontal_input_held,
        launcher_card_home,
        nav,
        navigation_endpoint_rendered,
        navigation_transition_composition_active,
        navigation_transition_renderer,
        navigation_transition_route,
        pacer,
        post_timing,
        pre_render_wait_us,
        presented_frame,
        run_start,
        screensaver,
        tooling,
        tooling_animation_active,
        tooling_attempt_id,
        tooling_drop_baseline,
        tooling_frame_begin,
        tooling_frame_evidence,
        tooling_tick_us,
        wait_done,
        wait_start,
    } = ctx;
    if let Some(session) = tooling.as_mut() {
        let metrics = &mut session.metrics;
        metrics.counters.presentations += 1;
        if screensaver.active {
            metrics.counters.screensaver_presentations += 1;
        }
        metrics.counters.posts += 1;
        metrics.counters.flips += 1;
        let render_us = frame_t2.saturating_duration_since(frame_t1).as_micros() as u64;
        metrics.last_render_us = render_us;
        metrics.counters.render_us += render_us;
        if metrics.window_start.is_some() && metrics.window.is_none() {
            metrics.frame_timings_us.push([
                render_us,
                presented_frame.main_present_hidden_copy_us as u64,
                Instant::now()
                    .saturating_duration_since(frame_t1)
                    .as_micros() as u64,
            ]);
        }
        metrics.counters.render_to_present_us += Instant::now()
            .saturating_duration_since(frame_t1)
            .as_micros() as u64;
        if let Some((copy_us, source_timestamp_us, source_generation, age_us)) =
            card_direct_measurement.take()
        {
            metrics.counters.card_hidden_copy_us =
                metrics.counters.card_hidden_copy_us.saturating_add(copy_us);
            metrics.counters.card_source_age_us =
                metrics.counters.card_source_age_us.saturating_add(age_us);
            metrics.last_card_source_timestamp_us = source_timestamp_us;
            let requested_generation = launcher_card_home
                .as_ref()
                .expect("a direct card presentation retains its card session")
                .current_request()
                .generation;
            if !metrics.note_card_delivery(requested_generation, source_generation) {
                if let Some(frame) = tooling_frame_evidence.as_mut() {
                    frame.missing_fresh_pose += 1;
                }
                metrics.record_dropped_frame(
                    mister_magik_tooling_support::measurement::DroppedFrameRecord {
                        reason: "delivered artwork does not match the requested pose",
                        workload: mister_magik_tooling_support::measurement::FrameWorkload::Card,
                        dropped_frames: 1,
                        source_generation,
                        source_age_us: age_us,
                        ..Default::default()
                    },
                );
            }
        } else if card_presentation_measurement_enabled {
            metrics.counters.card_synchronous_presentations += 1;
        }
        if nav.home_horizontal_repeat_active() {
            metrics.counters.card_continuous_presentations += 1;
        }
        let evidence_read_before = tooling_frame_evidence.as_ref().map(|_| Instant::now());
        match f.read_magik_presentation_telemetry() {
            Ok(telemetry) => {
                let observed_at = Instant::now();
                let animation_active = tooling_animation_active;
                if let Some(previous_observation) = tooling_drop_baseline {
                    let previous = previous_observation.telemetry;
                    let at = previous_observation.at;
                    let was_animating = previous_observation.motion;
                    match mister_magik_latch_contract::validate_presentation_telemetry_window(
                        previous,
                        telemetry,
                        observed_at.saturating_duration_since(at).as_micros().max(1) as u64,
                        8_333,
                    ) {
                        Ok(delta) => {
                            metrics.counters.owned_vblanks += u64::from(delta.owned_vblank_delta);
                            metrics.counters.presented_vblanks +=
                                u64::from(delta.presented_vblank_delta);
                            let repeated = u64::from(delta.repeated_vblank_delta);
                            let dropped = if !was_animating && animation_active {
                                // A first frame's baseline restarts at its render, so
                                // this spans that frame's work up to its post.
                                let work_us = post_timing
                                    .map_or(frame_t4, |(posted, _)| posted)
                                    .saturating_duration_since(at)
                                    .as_micros()
                                    as u64;
                                let dropped =
                                    mister_magik_tooling_support::measurement::first_frame_drops(
                                        repeated,
                                        work_us,
                                        pacer.period_us(),
                                    );
                                metrics.counters.motion_starts += 1;
                                metrics.counters.first_frame_wait_refreshes += repeated - dropped;
                                dropped
                            } else if animation_active || was_animating {
                                repeated
                            } else {
                                0
                            };
                            metrics.counters.drops += dropped;
                            if dropped != 0 || tooling_frame_evidence.is_some() {
                                let record = mister_magik_tooling_support::measurement::DroppedFrameRecord {
                                    reason: "owned refresh repeated during motion; see observation interval and phase timeline",
                                    workload: if screensaver.active {
                                        mister_magik_tooling_support::measurement::FrameWorkload::Screensaver
                                    } else if navigation_transition_composition_active {
                                        mister_magik_tooling_support::measurement::FrameWorkload::SystemTransition
                                    } else if card_work_timing.is_some() {
                                        mister_magik_tooling_support::measurement::FrameWorkload::Card
                                    } else {
                                        mister_magik_tooling_support::measurement::FrameWorkload::Slint
                                    },
                                    transition_route: navigation_transition_route,
                                    transition_renderer: navigation_transition_renderer,
                                    timeline: Some(mister_magik_tooling_support::measurement::FramePhaseTimeline {
                                        previous_observation_us: duration_us(run_start, at),
                                        frame_begin_us: duration_us(run_start, tooling_frame_begin),
                                        render_start_us: duration_us(run_start, frame_t1),
                                        render_end_us: duration_us(run_start, frame_t2),
                                        custom_draw_start_us: duration_us(run_start, custom_draw_start),
                                        custom_draw_end_us: duration_us(run_start, custom_draw_done),
                                        present_start_us: duration_us(run_start, frame_t3),
                                        post_returned_us: duration_us(run_start, frame_t4),
                                        post_request_start_us: post_timing.map(|(at,_)|duration_us(run_start,at)),
                                        post_verified_us: post_timing.map(|(_,at)|duration_us(run_start,at)),
                                        confirmation_wait_start_us: duration_us(run_start, wait_start),
                                        active_observed_us: duration_us(run_start, wait_done),
                                        telemetry_observed_us: duration_us(run_start, observed_at),
                                        refresh_period_us: pacer.period_us(),
                                        frame_start_phase_us,
                                        present_start_phase_us: u128_to_u64(presented_frame.present_phase_us),
                                        tooling_tick_us,
                                        pre_render_wait_us: u128_to_u64(pre_render_wait_us),
                                        hidden_copy_us: u128_to_u64(presented_frame.main_present_hidden_copy_us),
                                        hidden_publish_us: u128_to_u64(presented_frame.main_present_hidden_publish_us),
                                        latch_request_us: u128_to_u64(presented_frame.main_present_request_us),
                                        post_status_us: presented_frame.main_present_wait_us,
                                        completion_poll_us: presented_frame.main_present_completion_poll_wall_us,
                                        previous_active_sequence: previous.active_sequence,
                                        posted_sequence: presented_frame.main_present_sequence,
                                        post_active_sequence: presented_frame.main_present_post_active_sequence,
                                        post_pending_sequence: presented_frame.main_present_post_pending_sequence,
                                        post_pending: presented_frame.main_present_post_pending,
                                        previous_owned_refresh: previous.owned_vblank_count,
                                        owned_refresh_delta: delta.owned_vblank_delta,
                                        repeated_refresh_delta: delta.repeated_vblank_delta,
                                    }),
                                    work:card_work_timing,
                                    dropped_frames: dropped,
                                    owned_refresh_observed: Some(telemetry.owned_vblank_count),
                                    active_sequence: Some(telemetry.active_sequence), ui_render_us: render_us,
                                    ..Default::default()
                                };
                                if dropped != 0 {
                                    metrics.record_dropped_frame(record);
                                }
                                if let Some(frame) = tooling_frame_evidence.as_mut() {
                                    frame.record = record;
                                    frame.telemetry_valid = true;
                                    frame.telemetry_before_us =
                                        duration_us(run_start, evidence_read_before.unwrap());
                                    frame.previous_read_bracket_us =
                                        previous_observation.read_bracket_us;
                                    frame.previous_observation_attempt_id =
                                        Some(previous_observation.attempt_id);
                                    frame.refresh_counter = Some(telemetry.owned_vblank_count);
                                    frame.ownership_loss_count =
                                        Some(telemetry.ownership_loss_count);
                                    frame.raw_presented_count =
                                        Some(telemetry.presented_vblank_count);
                                    frame.raw_repeat_count = Some(telemetry.repeated_vblank_count);
                                    frame.telemetry_flags = Some(telemetry.flags);
                                }
                            }
                        }
                        Err(error) => metrics.error = Some(error.to_string()),
                    }
                }
                let card_motion_active = nav.screen == Screen::Home
                    && launcher_card_home
                        .as_ref()
                        .is_some_and(|card| card.is_animating());
                let other_motion_active = card_motion_active
                    || nav.home_scroll_active()
                    || nav.screen == Screen::Home && nav.home_horizontal_repeat_active()
                    || home_horizontal_input_held
                    || screensaver.active
                    || director.navigation.is_active()
                    || director.orientation.is_active()
                    || nav.screen == Screen::Arcade && nav.arcade.is_scroll_active()
                    || app.window().has_active_animations();
                let endpoint_rendered = navigation_endpoint_rendered
                    || card_direct_frame_rendered && !card_motion_active;
                let motion_continues =
                    animation_active && (!endpoint_rendered || other_motion_active);
                if animation_active && !motion_continues {
                    metrics.counters.motion_endpoint_resets += 1;
                }
                if let Some(frame) = tooling_frame_evidence.as_mut() {
                    frame.motion_continues_after_present = Some(motion_continues);
                }
                *tooling_drop_baseline = Some(
                    super::launcher_frame_accounting::ToolingPresentationObservation::new(
                        telemetry,
                        observed_at,
                        motion_continues,
                        tooling_attempt_id,
                        evidence_read_before,
                        run_start,
                    ),
                );
                metrics.last_physical_drop_count = Some(presented_frame.main_present_drop_count);
            }
            Err(error) => metrics.error = Some(format!("presentation telemetry: {error}")),
        }
    }
}

fn should_desire_direct_layer(wants_layer: bool, composition_allows_layer: bool) -> bool {
    wants_layer && composition_allows_layer
}

fn shield_base_damage_under_publication(
    damage: DirtyRectList,
    publication: &mut Option<PhysicalLayerPublication>,
) -> DirtyRectList {
    let Some(current) = publication.as_ref() else {
        return damage;
    };
    let rect = current.state().rect;
    if !damage
        .iter()
        .any(|damaged| damaged.intersection(rect).is_some())
    {
        return damage;
    }
    let Some(reapply) = current.for_frame(current.state(), Some(PhysicalLayerUpdate::Full(rect)))
    else {
        return damage;
    };
    *publication = Some(reapply);
    subtract_dirty_rects(damage, &DirtyRectList::from_one(rect))
}

fn should_start_preview_compositor(
    wants_preview: bool,
    hdmi_preview_route: bool,
    composition_allows_preview: bool,
    memory_guard_active: bool,
    start_attempted: bool,
) -> bool {
    wants_preview
        && hdmi_preview_route
        && composition_allows_preview
        && !memory_guard_active
        && !start_attempted
}

fn should_desire_preview_direct_layer(
    wants_layer: bool,
    composition_allows_layer: bool,
    route_wants_preview: bool,
    compositor_pending: bool,
    has_preview_backing: bool,
    has_direct_preview_update: bool,
) -> bool {
    should_desire_direct_layer(
        wants_layer
            || has_direct_preview_update
            || (route_wants_preview && compositor_pending && has_preview_backing),
        composition_allows_layer,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewRoutePolicy {
    kind: PreviewRouteKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewRouteKind {
    Hdmi,
    CrtBackdrop,
}

fn crt_backdrop_frame_is_presented(
    navigation_transition_active: bool,
    full_damage: bool,
    work_active: bool,
    exact_preview: bool,
    raw_frame_ready: bool,
    backdrop_transitioning: bool,
) -> bool {
    !navigation_transition_active
        && full_damage
        && !work_active
        && exact_preview
        && raw_frame_ready
        && !backdrop_transitioning
}

impl PreviewRoutePolicy {
    const fn for_output_route(route: ResolvedOutputRoute) -> Self {
        Self {
            kind: match route {
                ResolvedOutputRoute::Hdmi => PreviewRouteKind::Hdmi,
                ResolvedOutputRoute::Crt240p60
                | ResolvedOutputRoute::Crt288p50
                | ResolvedOutputRoute::Crt480p60
                | ResolvedOutputRoute::Crt576p50 => PreviewRouteKind::CrtBackdrop,
            },
        }
    }

    const fn allows_hdmi_preview(self) -> bool {
        matches!(self.kind, PreviewRouteKind::Hdmi)
    }

    const fn allows_crt_backdrop(self) -> bool {
        matches!(self.kind, PreviewRouteKind::CrtBackdrop)
    }
}

fn catalog_build_media_gate(
    catalog_refresh_done: bool,
    base: MediaInteractionGate,
) -> MediaInteractionGate {
    if catalog_refresh_done {
        base
    } else {
        MediaInteractionGate {
            active: true,
            reason: "catalog-build",
        }
    }
}

fn apply_catalog_system_scanning_presentation(
    nav: &mut LauncherNav,
    catalog: &mut ArcadeCatalog,
    system_id: &str,
    defer_bridge_ui: bool,
) -> bool {
    nav.catalog_system_scanning(system_id);
    if defer_bridge_ui {
        return false;
    }
    *catalog = catalog.with_system_placeholder(system_id);
    true
}

fn retain_startup_intro_catalog_ui_intent(
    replay: &mut Option<LauncherWorkerUiIntent>,
    intent: LauncherWorkerUiIntent,
) {
    if intent.is_catalog_presentation() {
        *replay = Some(intent);
    }
}

/// The catalog-side state the worker-message and effect handlers update together.
struct CatalogDomain<'a> {
    nav: &'a mut LauncherNav,
    catalog: &'a mut ArcadeCatalog,
    catalog_ready: &'a mut bool,
    catalog_version: &'a mut usize,
    return_capsule_active: &'a mut bool,
    catalog_generation: &'a mut CatalogGenerationState,
    launch_return_session: &'a mut LaunchReturnSession,
    preview: &'a mut PreviewState,
    scheduler: &'a mut LauncherScheduler,
    catalog_session: &'a mut LauncherCatalogSession,
    lifecycle: &'a mut LauncherLifecycle,
    lifecycle_effects: &'a mut LifecycleEffects,
    full_bridge_dirty: &'a mut bool,
    startup_intro_catalog_ui_replay: &'a mut Option<LauncherWorkerUiIntent>,
    startup_intro_catalog_shells_pending: &'a mut bool,
}

#[allow(clippy::too_many_arguments)]
fn process_catalog_worker_message(
    message: CatalogWorkerMessage,
    prepare_trace: &mut LauncherPrepareTrace,
    loop_start: Instant,
    app: &slint_ui::launcher::Launcher,
    domain: CatalogDomain<'_>,
    defer_bridge_ui: bool,
    start: Instant,
) {
    let CatalogDomain {
        nav,
        catalog,
        catalog_ready,
        catalog_version,
        return_capsule_active,
        catalog_generation,
        launch_return_session,
        preview,
        scheduler,
        catalog_session,
        lifecycle,
        lifecycle_effects,
        full_bridge_dirty,
        startup_intro_catalog_ui_replay,
        startup_intro_catalog_shells_pending,
    } = domain;
    prepare_trace.catalog_message_count = prepare_trace.catalog_message_count.saturating_add(1);
    let effects = catalog_session.handle_worker_message(
        CatalogWorkerMessageContext {
            catalog_ready: *catalog_ready,
            catalog_partial: *return_capsule_active,
        },
        message,
    );
    apply_catalog_session_effects(
        effects,
        app,
        CatalogDomain {
            nav,
            catalog,
            catalog_ready,
            catalog_version,
            return_capsule_active,
            catalog_generation,
            launch_return_session,
            preview,
            scheduler,
            catalog_session,
            lifecycle,
            lifecycle_effects,
            full_bridge_dirty,
            startup_intro_catalog_ui_replay,
            startup_intro_catalog_shells_pending,
        },
        defer_bridge_ui,
        loop_start,
        start,
    );
}

fn should_defer_catalog_message(
    message: &CatalogWorkerMessage,
    catalog_ready: bool,
    nav: &LauncherNav,
    stationary_edge_since: Option<Instant>,
    now: Instant,
) -> bool {
    if matches!(
        message,
        CatalogWorkerMessage::Ready {
            source: CatalogSource::NavigationProjection,
            ..
        }
    ) {
        return false;
    }
    if !catalog_ready
        || nav.screen != Screen::Arcade
        || !matches!(message, CatalogWorkerMessage::Ready { .. })
    {
        return false;
    }
    if nav.arcade.has_scroll_motion_or_queue() {
        return true;
    }
    nav.arcade.is_scroll_active()
        && stationary_edge_since.is_none_or(|since| {
            now.saturating_duration_since(since) < CATALOG_READY_STATIONARY_EDGE_SETTLE
        })
}

fn should_defer_launcher_background_work(
    input_event_count: usize,
    navigation_transition_active: bool,
    orientation_transition_active: bool,
    directional_input_held: bool,
) -> bool {
    input_event_count > 0
        || navigation_transition_active
        || orientation_transition_active
        || directional_input_held
}

fn catalog_messages_need_polling(
    pending_catalog_ready: bool,
    refresh_done: bool,
    worker_running: bool,
) -> bool {
    pending_catalog_ready || !refresh_done || worker_running
}

fn catalog_poll_scope(
    background_work_allowed: bool,
    full_screen_transition_owned: bool,
    system_entry_handoff_only: bool,
) -> Option<CatalogPollScope> {
    if full_screen_transition_owned {
        return Some(CatalogPollScope::Transition {
            system_entry_handoff: system_entry_handoff_only,
        });
    }
    if background_work_allowed {
        Some(CatalogPollScope::Idle)
    } else {
        Some(CatalogPollScope::Interactive {
            system_entry_handoff: system_entry_handoff_only,
        })
    }
}

fn should_poll_system_entry_handoff(
    background_work_allowed: bool,
    collection_entry_pending: bool,
    launch_return_hydrating: bool,
    system_entry_prepare_active: bool,
) -> bool {
    !background_work_allowed
        && system_entry_prepare_active
        && (collection_entry_pending || launch_return_hydrating)
}

fn update_catalog_ready_stationary_edge_since(
    nav: &LauncherNav,
    current: Option<Instant>,
    now: Instant,
) -> Option<Instant> {
    (nav.screen == Screen::Arcade
        && nav.arcade.is_scroll_active()
        && !nav.arcade.has_scroll_motion_or_queue())
    .then_some(current.unwrap_or(now))
}

fn launcher_return_to_launcher_requested() -> bool {
    return_to_launcher_env_is_set(
        std::env::var("MISTER_MAGIK_RETURN_TO_LAUNCHER")
            .ok()
            .as_deref(),
    )
}

fn return_black_timeout_requires_home_fallback(
    return_was_waiting: bool,
    effects: &LifecycleEffects,
) -> bool {
    return_was_waiting && effects.has_startup_event("return_black_screen_timeout")
}

fn return_to_launcher_env_is_set(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true") | Some("yes"))
}

#[derive(Debug)]
pub(super) struct LaunchReturnSession {
    state: Option<launcher::LaunchReturnState>,
    pub(super) source: &'static str,
    pub(super) phase: &'static str,
    pub(super) fallback_reason: String,
    pub(super) exact_context_monotonic_us: u64,
    pub(super) preview_ready_monotonic_us: u64,
    pub(super) first_correct_present_monotonic_us: u64,
    authoritative_catalog_ready: bool,
    complete: bool,
}

impl LaunchReturnSession {
    fn new(state: Option<launcher::LaunchReturnState>) -> Self {
        Self {
            phase: if state.is_some() { "requested" } else { "none" },
            state,
            source: "none",
            fallback_reason: String::new(),
            exact_context_monotonic_us: 0,
            preview_ready_monotonic_us: 0,
            first_correct_present_monotonic_us: 0,
            authoritative_catalog_ready: false,
            complete: false,
        }
    }

    fn requested(&self) -> bool {
        self.state.is_some()
    }

    fn protects_hydrating_collection(&self, nav: &LauncherNav) -> bool {
        self.state.as_ref().is_some_and(|state| {
            state.collection_id().is_some_and(|collection_id| {
                nav.active_collection_id() == Some(collection_id)
                    && nav.catalog_system_hydration_is_loading(state.system_id())
            })
        })
    }

    fn state(&self) -> Option<&launcher::LaunchReturnState> {
        self.state.as_ref()
    }

    fn note_capsule_failure(&mut self, error: String) {
        self.source = "capsule-rejected";
        self.phase = "hydrate-system-shard";
        self.fallback_reason = error;
    }

    fn apply(
        &mut self,
        nav: &mut LauncherNav,
        catalog: &ArcadeCatalog,
        source: CatalogSource,
    ) -> bool {
        let Some(state) = self.state.as_ref().cloned() else {
            return false;
        };
        if !launcher::apply_launch_return_state(nav, catalog, state) {
            return false;
        }
        if self.exact_context_monotonic_us == 0 {
            self.source = source.label();
            self.exact_context_monotonic_us = monotonic_clock_us().unwrap_or(0);
        }
        if matches!(
            source,
            CatalogSource::ShardedRegistry
                | CatalogSource::NavigationProjection
                | CatalogSource::FullSqlite
                | CatalogSource::FreshBuild
        ) {
            self.authoritative_catalog_ready = true;
        }
        self.phase = if self.complete {
            "complete"
        } else if self.authoritative_catalog_ready {
            "authoritative-context-restored"
        } else {
            "context-restored"
        };
        true
    }

    fn reapply(&mut self, nav: &mut LauncherNav, catalog: &ArcadeCatalog) -> bool {
        let Some(state) = self.state.as_ref().cloned() else {
            return false;
        };
        if !launcher::apply_launch_return_state(nav, catalog, state) {
            return false;
        }
        self.phase = if self.complete {
            "complete"
        } else if self.authoritative_catalog_ready {
            "authoritative-context-restored"
        } else {
            "context-restored"
        };
        true
    }

    fn mark_system_shard_authoritative(&mut self) {
        self.authoritative_catalog_ready = true;
        self.source = "system-shard";
        self.phase = if self.complete {
            "complete"
        } else {
            "authoritative-context-restored"
        };
    }

    fn context_matches(&self, nav: &LauncherNav, catalog: &ArcadeCatalog) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        if nav.screen != Screen::Arcade
            || state
                .collection_id()
                .is_some_and(|collection_id| nav.active_collection_id() != Some(collection_id))
            || nav.arcade.selected != state.game_index()
            || !nav.arcade.is_settled_at_selected()
        {
            return false;
        }
        nav.active_arcade_game_at(
            catalog,
            nav.active_collection_scope_id(catalog),
            nav.arcade.selected,
        )
        .is_some_and(|game| game.mra_path.as_ref() == state.game_path())
    }

    fn mark_preview_ready(&mut self) {
        if self.preview_ready_monotonic_us == 0 {
            self.preview_ready_monotonic_us = monotonic_clock_us().unwrap_or(0);
        }
        self.phase = "preview-ready";
    }

    fn mark_correct_present(&mut self, nav: &LauncherNav, catalog: &ArcadeCatalog) {
        if !self.context_matches(nav, catalog) || self.preview_ready_monotonic_us == 0 {
            return;
        }
        if self.first_correct_present_monotonic_us == 0 {
            self.first_correct_present_monotonic_us = monotonic_clock_us().unwrap_or(0);
        }
        self.phase = if self.authoritative_catalog_ready {
            "complete"
        } else {
            "presented-awaiting-authoritative-catalog"
        };
        if self.authoritative_catalog_ready {
            self.complete = true;
        }
    }

    fn release_if_complete(&mut self) {
        if self.complete {
            // Catalog/taxonomy replacement may reapply the saved state after the
            // correct frame was presented. Reapplication must not make a completed
            // return look incomplete to status consumers.
            self.phase = "complete";
            self.state = None;
        }
    }

    fn fallback_to_home(&mut self, nav: &mut LauncherNav) {
        nav.go_root();
        self.phase = "fallback-home";
        if self.fallback_reason.is_empty() {
            self.fallback_reason = "return restoration exceeded five-second deadline".to_string();
        }
        self.state = None;
    }
}

fn apply_pending_launch_return_state(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    pending: &mut LaunchReturnSession,
    source: CatalogSource,
) -> bool {
    pending.apply(nav, catalog, source)
}

#[allow(clippy::too_many_arguments)]
fn apply_or_request_pending_launch_return_state(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    catalog_version: usize,
    pending: &mut LaunchReturnSession,
    scheduler: &mut LauncherScheduler,
    source: CatalogSource,
    now: Instant,
    start: Instant,
) -> bool {
    let restored = apply_pending_launch_return_state(nav, catalog, pending, source);
    if !restored {
        let _ = request_pending_launch_return_shard(
            pending.state(),
            catalog,
            catalog_version,
            nav,
            scheduler,
            now,
            start,
        );
    }
    restored
}

fn reapply_pending_launch_return_state(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    pending: &mut LaunchReturnSession,
) -> bool {
    pending.reapply(nav, catalog)
}

fn emit_return_context_restored(
    lifecycle: &mut LauncherLifecycle,
    effects: &mut LifecycleEffects,
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
    preview: &PreviewState,
    return_session: &mut LaunchReturnSession,
    restored_at: Instant,
) {
    let startup_status = lifecycle.startup_status();
    if startup_status.mode != StartupMode::ReturnFromGame || startup_status.input_enabled {
        return;
    }
    let system_id = active_system(catalog, nav)
        .map(|system| system.legacy_system_id.clone())
        .unwrap_or_default();
    let game_path = active_system(catalog, nav)
        .and_then(|system| nav.active_arcade_game_at(catalog, &system.id, nav.arcade.selected))
        .map(|game| game.mra_path.to_string())
        .unwrap_or_default();
    lifecycle.handle(
        LauncherLifecycleInput::StartupReturnContextRestored {
            screen: screen_label(nav.screen),
            system_id,
            filter: arcade_filter_cache_token(&nav.arcade_filter.active),
            game_path,
            game_index: nav.arcade.selected,
            visual_index: nav.arcade.visual_index,
            preview_expected: selected_arcade_game_has_preview(nav, catalog),
            restored_at,
        },
        effects,
    );
    if return_preview_ready(return_session, nav, catalog, preview) {
        return_session.mark_preview_ready();
        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnPreviewReady {
                preview_state: preview.trace_cache_state(),
            },
            effects,
        );
    }
}

fn maybe_mark_return_preview_ready(
    lifecycle: &mut LauncherLifecycle,
    effects: &mut LifecycleEffects,
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
    preview: &PreviewState,
    return_session: &mut LaunchReturnSession,
) {
    let status = lifecycle.startup_status();
    if status.mode != StartupMode::ReturnFromGame
        || status.state != StartupRevealState::WaitRelevantPreview
        || !return_preview_ready(return_session, nav, catalog, preview)
    {
        return;
    }
    return_session.mark_preview_ready();
    lifecycle.handle(
        LauncherLifecycleInput::StartupReturnPreviewReady {
            preview_state: preview.trace_cache_state(),
        },
        effects,
    );
}

fn return_preview_ready(
    return_session: &LaunchReturnSession,
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
    preview: &PreviewState,
) -> bool {
    if !return_session.context_matches(nav, catalog) {
        return false;
    }
    if !selected_arcade_game_has_preview(nav, catalog) {
        return true;
    }
    preview.trace_cache_state() == "exact"
}

fn selected_arcade_game_has_preview(nav: &LauncherNav, catalog: &ArcadeCatalog) -> bool {
    active_system(catalog, nav)
        .and_then(|system| nav.active_arcade_game_at(catalog, &system.id, nav.arcade.selected))
        .is_some_and(|game| game.has_preview)
}

fn apply_lifecycle_effects(
    effects: &mut LifecycleEffects,
    scheduler: &mut LauncherScheduler,
    start: Instant,
) {
    for effect in effects.drain() {
        match effect {
            LauncherEffect::StartupEvent { name, detail } => {
                if name == "return_black_screen_timeout" {
                    crate::ui_errln!("return black-screen watchdog expired: {detail}");
                }
                print_startup_event(start, name, detail);
            }
            LauncherEffect::BeginLoadingFrame { launch_ref } => {
                print_startup_event(
                    start,
                    "launcher_lifecycle_loading_frame_requested",
                    format!("launch_ref={launch_ref}"),
                );
            }
            LauncherEffect::BeginLaunchHandoff { launch_ref } => {
                scheduler.complete_loading_frame();
                print_startup_event(
                    start,
                    "launcher_lifecycle_handoff_requested",
                    format!("launch_ref={launch_ref}"),
                );
            }
            LauncherEffect::PresentRecoveryFrame => {
                print_startup_event(
                    start,
                    "launcher_lifecycle_recovery_requested",
                    "reason=launch",
                );
            }
            LauncherEffect::ReturnToIdle => {
                print_startup_event(start, "launcher_lifecycle_recovered", "state=idle");
            }
            LauncherEffect::StartCatalogRetry { root } => {
                print_startup_event(start, "catalog_retry_started", &root);
                scheduler.start_catalog_worker(
                    root,
                    CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                    CatalogWorkerInitialCache::AlreadyProbedMissing,
                    CatalogExecutionMode::ForegroundExclusive,
                );
            }
            LauncherEffect::StartCatalogRebuild { root } => {
                print_startup_event(start, "catalog_rebuild_started", &root);
                scheduler.start_catalog_worker(
                    root,
                    CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                    CatalogWorkerInitialCache::AlreadyLoadedReady,
                    CatalogExecutionMode::BackgroundInteractive,
                );
            }
            LauncherEffect::StartFreshCatalogBuild { root } => {
                print_startup_event(start, "catalog_fresh_build_started", &root);
                scheduler.start_catalog_worker(
                    root,
                    CatalogWorkerRequest::FreshBuild,
                    CatalogWorkerInitialCache::AlreadyProbedMissing,
                    CatalogExecutionMode::ForegroundExclusive,
                );
            }
            LauncherEffect::ExitToMister => {
                print_startup_event(start, "catalog_recovery_exit_requested", "target=mister");
                match launcher::exit_to_mister() {
                    Ok(()) => std::process::exit(0),
                    Err(error) => {
                        crate::ui_errln!("catalog recovery exit to MiSTer failed: {error}");
                    }
                }
            }
        }
    }
}

fn apply_catalog_session_effects(
    effects: CatalogSessionEffects,
    app: &slint_ui::launcher::Launcher,
    domain: CatalogDomain<'_>,
    defer_bridge_ui: bool,
    now: Instant,
    start: Instant,
) {
    let CatalogDomain {
        nav,
        catalog,
        catalog_ready,
        catalog_version,
        return_capsule_active,
        catalog_generation,
        launch_return_session,
        preview,
        scheduler,
        catalog_session: _,
        lifecycle,
        lifecycle_effects,
        full_bridge_dirty,
        startup_intro_catalog_ui_replay,
        startup_intro_catalog_shells_pending,
    } = domain;
    for effect in effects.into_effects() {
        match effect {
            CatalogSessionEffect::StartupEvent(event) => {
                print_startup_event(start, &event.name, event.detail);
            }
            CatalogSessionEffect::UseCatalog {
                catalog: ready_catalog,
                source,
                durable,
                generation_fingerprint,
                publication_ack,
            } => {
                let taxonomy_sync_required = catalog_taxonomy_sync_required(*catalog_ready, source);
                *catalog = catalog_for_ready_source(nav, ready_catalog, source);
                *catalog_version = (*catalog_version).wrapping_add(1);
                *catalog_ready = true;
                *return_capsule_active = false;
                nav.set_arcade_exit_locked(false);
                catalog_generation.publish(generation_fingerprint, durable);
                if scheduler.set_system_shard_generation(catalog_generation.current.as_deref()) {
                    nav.catalog_hydration_reset();
                    if catalog_generation.current.is_some() {
                        match scheduler.open_system_entry_reader() {
                            Ok(elapsed_us) => print_startup_event(
                                start,
                                "system_entry_reader_reopened",
                                format!(
                                    "generation={} elapsed_us={} cpu=0 reason=catalog-publication",
                                    catalog_generation.current.as_deref().unwrap_or("unknown"),
                                    elapsed_us,
                                ),
                            ),
                            Err(error) => print_startup_event(
                                start,
                                "system_entry_reader_reopen_failed",
                                format!("error={}", error.replace('\t', " ")),
                            ),
                        }
                    }
                }
                if let Some(publication_ack) = publication_ack {
                    let _ = publication_ack.send(());
                }
                if taxonomy_sync_required {
                    nav.sync_launcher_taxonomy(catalog);
                }
                apply_forced_arcade_selected(nav, catalog);
                let return_restored = apply_or_request_pending_launch_return_state(
                    nav,
                    catalog,
                    *catalog_version,
                    launch_return_session,
                    scheduler,
                    source,
                    now,
                    start,
                );
                if return_restored {
                    emit_return_context_restored(
                        lifecycle,
                        lifecycle_effects,
                        nav,
                        catalog,
                        preview,
                        launch_return_session,
                        now,
                    );
                    lifecycle.tick_startup_reveal(now, true, lifecycle_effects);
                }
                lifecycle.handle(
                    LauncherLifecycleInput::CatalogReady {
                        source,
                        validating: false,
                    },
                    lifecycle_effects,
                );
                apply_lifecycle_effects(lifecycle_effects, scheduler, start);
            }
            CatalogSessionEffect::DiscardPartialCatalog => {
                let root = catalog.root.to_string_lossy().into_owned();
                *catalog = empty_arcade_catalog(&root);
                *catalog_version = (*catalog_version).wrapping_add(1);
                *catalog_ready = false;
                *return_capsule_active = false;
                *catalog_generation = CatalogGenerationState::default();
                let _ = scheduler.set_system_shard_generation(None);
                nav.catalog_hydration_reset();
                nav.set_arcade_exit_locked(false);
                nav.sync_launcher_taxonomy(catalog);
                let _ = reapply_pending_launch_return_state(nav, catalog, launch_return_session);
                let bridge = app.global::<slint_ui::launcher::ArcadeView>();
                preview.clear(&bridge);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::ApplySearchResult { request, result } => {
                if request.catalog_version == *catalog_version {
                    let timing = result.timing;
                    if nav.apply_arcade_search_result(catalog, &request, result) {
                        print_startup_event(
                            start,
                            "arcade_search_query_ready",
                            format!(
                                "request={} collection={} rust_prepare_us={} sqlite_us={} rust_finalize_us={} total_us={}",
                                request.request_id,
                                request.collection_id,
                                timing.rust_prepare_us,
                                timing.sqlite_us,
                                timing.rust_finalize_us,
                                timing.total_us
                            ),
                        );
                        let return_restored = reapply_pending_launch_return_state(
                            nav,
                            catalog,
                            launch_return_session,
                        );
                        if return_restored {
                            emit_return_context_restored(
                                lifecycle,
                                lifecycle_effects,
                                nav,
                                catalog,
                                preview,
                                launch_return_session,
                                now,
                            );
                            lifecycle.tick_startup_reveal(now, true, lifecycle_effects);
                        }
                        *full_bridge_dirty = true;
                    }
                }
            }
            CatalogSessionEffect::FailSearchRequest { request, error } => {
                if request.catalog_version == *catalog_version
                    && nav.fail_arcade_search_request(&request)
                {
                    print_startup_event(
                        start,
                        "arcade_search_query_failed",
                        format!(
                            "request={} collection={} error={}",
                            request.request_id,
                            request.collection_id,
                            error.replace('\t', " ")
                        ),
                    );
                    *full_bridge_dirty = true;
                }
            }
            CatalogSessionEffect::SyncCatalogBridge => {
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogPlanReady {
                system_ids,
                all_published_systems,
            } => {
                // The first-run intro needs only the authoritative Arcade
                // projection used for its live launcher frame. Rebuilding
                // navigation shells here clones the resident Arcade rows on
                // CPU1 once per scan milestone, despite the launcher being
                // dormant. The final published catalog will install the same
                // taxonomy authoritatively.
                nav.catalog_reconciliation_plan(catalog, &system_ids, all_published_systems);
                if defer_bridge_ui {
                    *startup_intro_catalog_shells_pending = true;
                    continue;
                }
                *catalog = nav.catalog_with_build_shells(catalog.clone());
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                let _ = reapply_pending_launch_return_state(nav, catalog, launch_return_session);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogSystemScanning { system_id } => {
                if !apply_catalog_system_scanning_presentation(
                    nav,
                    catalog,
                    &system_id,
                    defer_bridge_ui,
                ) {
                    *startup_intro_catalog_shells_pending = true;
                    continue;
                }
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                let _ = reapply_pending_launch_return_state(nav, catalog, launch_return_session);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogSystemPrepared {
                system_id,
                generation,
            } => {
                nav.catalog_system_prepared(&system_id);
                if defer_bridge_ui {
                    *startup_intro_catalog_shells_pending = true;
                } else {
                    *catalog_version = (*catalog_version).wrapping_add(1);
                    *full_bridge_dirty = true;
                }
                print_startup_event(
                    start,
                    "catalog_system_prepared",
                    format!("system={system_id} generation={generation}"),
                );
            }
            CatalogSessionEffect::CatalogManifestPublished {
                generation,
                rebuilt,
                removed,
            } => {
                print_startup_event(
                    start,
                    "catalog_manifest_published",
                    format!(
                        "generation={generation} rebuilt={} removed={}",
                        rebuilt.join(","),
                        removed.join(",")
                    ),
                );
            }
            CatalogSessionEffect::CatalogSystemUpdateFailed { system_id } => {
                nav.catalog_system_update_failed(&system_id);
                *catalog = catalog.with_system_placeholder(&system_id);
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                let _ = reapply_pending_launch_return_state(nav, catalog, launch_return_session);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogSystemHydrationFailed { system_id } => {
                nav.catalog_system_hydration_failed(&system_id);
                *catalog_version = (*catalog_version).wrapping_add(1);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::PersistCatalogFailure {
                detail,
                mode,
                has_stale_catalog,
                system_id,
            } => {
                let (expected, actual) = crate::catalog_failure_report::schema_versions(&detail);
                let report_path = crate::catalog_failure_report::enqueue(
                    crate::catalog_failure_report::CatalogFailureReport {
                        code: mode.diagnostic_code().to_string(),
                        stage: mode.diagnostic_stage().to_string(),
                        operation: mode.diagnostic_operation().to_string(),
                        detail,
                        expected,
                        actual,
                        system_id,
                        generation: catalog_generation.current.clone(),
                        usable_catalog: has_stale_catalog && *catalog_ready,
                        games: catalog.len(),
                        systems: catalog.systems.len(),
                        durable_generation: catalog_generation.durable.clone(),
                        recovery_actions: vec![
                            mode.label(has_stale_catalog, CatalogRecoveryChoice::Left)
                                .to_string(),
                            mode.label(has_stale_catalog, CatalogRecoveryChoice::Right)
                                .to_string(),
                        ],
                    },
                );
                print_startup_event(
                    start,
                    "catalog_failure_report_queued",
                    format!(
                        "code={} stage={} operation={} path={}",
                        mode.diagnostic_code(),
                        mode.diagnostic_stage(),
                        mode.diagnostic_operation(),
                        report_path.display()
                    ),
                );
            }
            CatalogSessionEffect::CatalogBuildFinished => {
                *catalog = catalog.without_empty_system_placeholders();
                nav.catalog_build_finished(catalog);
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                let return_restored = apply_pending_launch_return_state(
                    nav,
                    catalog,
                    launch_return_session,
                    CatalogSource::FreshBuild,
                );
                if return_restored {
                    emit_return_context_restored(
                        lifecycle,
                        lifecycle_effects,
                        nav,
                        catalog,
                        preview,
                        launch_return_session,
                        now,
                    );
                }
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::Ui(intent) => {
                if defer_bridge_ui {
                    retain_startup_intro_catalog_ui_intent(startup_intro_catalog_ui_replay, intent);
                    *full_bridge_dirty = true;
                } else {
                    apply_launcher_worker_ui_intent(app, intent, full_bridge_dirty);
                }
            }
            CatalogSessionEffect::CatalogValidationFinished => {
                lifecycle.handle(
                    LauncherLifecycleInput::CatalogValidationFinished,
                    lifecycle_effects,
                );
                apply_lifecycle_effects(lifecycle_effects, scheduler, start);
            }
            CatalogSessionEffect::ApplySystemShard {
                system_id,
                catalog: prepared_catalog,
                base_catalog_version,
                game_count,
                prepare_us,
                preview_prelude,
            } => {
                if base_catalog_version != *catalog_version {
                    preview.cancel_system_entry_preview();
                    let _ = retry_system_shard_hydration(
                        scheduler,
                        nav,
                        catalog,
                        *catalog_version,
                        &system_id,
                        "stale-prepared-catalog",
                        now,
                    );
                    print_startup_event(
                        start,
                        "catalog_system_shard_stale",
                        format!(
                            "system={system_id} base_version={base_catalog_version} current_version={}",
                            *catalog_version
                        ),
                    );
                    continue;
                }
                let adoption_started = Instant::now();
                nav.catalog_system_hydration_finished(&system_id);
                let retired_catalog = std::mem::replace(catalog, prepared_catalog);
                *catalog_version = (*catalog_version).wrapping_add(1);
                if let Some(prelude) = preview_prelude
                    && let Some(game) = catalog.system_game_at(&system_id, 0)
                {
                    preview.adopt_system_entry_preview(game, prelude);
                }
                scheduler.retire_catalog(retired_catalog);
                nav.sync_launcher_taxonomy(catalog);
                let return_restored =
                    reapply_pending_launch_return_state(nav, catalog, launch_return_session);
                if return_restored {
                    launch_return_session.mark_system_shard_authoritative();
                    emit_return_context_restored(
                        lifecycle,
                        lifecycle_effects,
                        nav,
                        catalog,
                        preview,
                        launch_return_session,
                        now,
                    );
                    lifecycle.tick_startup_reveal(now, true, lifecycle_effects);
                }
                *full_bridge_dirty = true;
                let adoption_us = adoption_started.elapsed().as_micros();
                print_startup_event(
                    start,
                    "catalog_system_shard_ready",
                    format!(
                        "system={system_id} games={game_count} prepare_us={prepare_us} adoption_us={}",
                        adoption_us
                    ),
                );
            }
            CatalogSessionEffect::RequestLibraryRebuildOnNextBoot => {
                match launcher::request_library_rebuild_on_next_boot() {
                    Ok(()) => {
                        print_startup_event(start, "library_rebuild_deferred", "marker=written");
                    }
                    Err(e) => {
                        crate::ui_errln!("failed to defer library rebuild: {e}");
                        print_startup_event(start, "library_rebuild_defer_failed", e);
                    }
                }
            }
            CatalogSessionEffect::Confirm(action) => {
                nav.confirm_action = Some(action);
                nav.confirm_selected = 0;
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::Lifecycle(input) => {
                lifecycle.handle(input, lifecycle_effects);
                apply_lifecycle_effects(lifecycle_effects, scheduler, start);
                launch_return_session.release_if_complete();
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::StartCatalogWorker(worker) => {
                print_startup_event(start, "catalog_worker_start", &worker.root);
                lifecycle.handle(
                    LauncherLifecycleInput::CatalogBuilding {
                        mode: if worker.request == CatalogWorkerRequest::FreshBuild {
                            CatalogBuildMode::FreshRecovery
                        } else if *catalog_ready {
                            CatalogBuildMode::Update
                        } else {
                            CatalogBuildMode::FirstBuild
                        },
                        foreground: worker.execution_mode
                            == CatalogExecutionMode::ForegroundExclusive,
                        has_stale_catalog: *catalog_ready,
                    },
                    lifecycle_effects,
                );
                apply_lifecycle_effects(lifecycle_effects, scheduler, start);
                scheduler.start_catalog_worker(
                    worker.root,
                    worker.request,
                    worker.initial_cache,
                    worker.execution_mode,
                );
            }
        }
    }
}

fn apply_screenshot_media_update_effects(
    effects: ScreenshotMediaUpdateEffects,
    app: &slint_ui::launcher::Launcher,
    catalog: &mut ArcadeCatalog,
    scheduler: &mut LauncherScheduler,
    mut preview: Option<&mut PreviewState>,
    full_bridge_dirty: &mut bool,
    start: Instant,
) {
    for effect in effects.into_effects() {
        match effect {
            ScreenshotMediaUpdateEffect::StartupEvent(event) => {
                print_startup_event(start, &event.name, event.detail);
            }
            ScreenshotMediaUpdateEffect::Ui(intent) => {
                apply_launcher_worker_ui_intent(app, intent, full_bridge_dirty);
            }
            ScreenshotMediaUpdateEffect::EnsureWorker { mode } => {
                scheduler.ensure_media_worker_started(start, mode);
            }
            ScreenshotMediaUpdateEffect::EnsureSystem { system_id } => {
                scheduler.ensure_media_system(&system_id);
            }
            ScreenshotMediaUpdateEffect::DropWorker => {
                scheduler.drop_media_worker();
            }
            ScreenshotMediaUpdateEffect::MarkWorkerUnavailable => {
                scheduler.mark_media_worker_unavailable();
            }
            ScreenshotMediaUpdateEffect::ClearPreviewFailures => {
                if let Some(preview) = preview.as_deref_mut() {
                    preview.clear_failed_preview_cache();
                }
            }
            ScreenshotMediaUpdateEffect::ApplyPreviewAvailability { system_id, games } => {
                *catalog = catalog_with_preview_availability(catalog, &system_id, &games);
                if let Some(preview) = preview.as_deref_mut() {
                    preview.clear_failed_preview_cache();
                }
                *full_bridge_dirty = true;
                print_startup_event(
                    start,
                    "screenshot_media_catalog_live_applied",
                    format!("system={system_id} games={}", games.len()),
                );
            }
            ScreenshotMediaUpdateEffect::SetInteractionActive { active, reason } => {
                scheduler.set_media_interaction_active(active, reason);
            }
        }
    }
}

fn catalog_with_preview_availability(
    catalog: &ArcadeCatalog,
    system_id: &str,
    games: &[mister_magik_catalog::system_shard::SystemGame],
) -> ArcadeCatalog {
    let (replacement, launch_plans) = arcade_rows_from_persisted_shard(system_id, games);
    let collection = std::sync::Arc::new(arcade_catalog::SystemCollection::new(
        system_id,
        replacement,
        launch_plans,
        catalog.platform_kind(system_id),
    ));
    let mut updated =
        catalog.with_system_collection_for_id(system_id, std::sync::Arc::clone(&collection));
    if system_id == "arcade" {
        updated = updated
            .with_system_collection_for_id(arcade_catalog::MENU_ARCADE_SYSTEM_ID, collection);
    }
    updated
}

fn catalog_background_validation_delay() -> Duration {
    std::env::var("MISTER_CATALOG_BACKGROUND_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogStartupWithoutSummaryPlan {
    DeferredWorker {
        request: CatalogWorkerRequest,
        initial_cache: CatalogWorkerInitialCache,
        execution_mode: CatalogExecutionMode,
    },
    NoCatalog,
}

fn catalog_startup_without_registry_plan(
    catalog_worker_enabled: bool,
) -> CatalogStartupWithoutSummaryPlan {
    if catalog_worker_enabled {
        return CatalogStartupWithoutSummaryPlan::DeferredWorker {
            // No registry means there is no refresh manifest to reconcile.
            // Select the build operation before intro/latch eligibility so
            // every cold-start route can create the first fast catalog.
            request: CatalogWorkerRequest::FreshBuild,
            initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
            execution_mode: CatalogExecutionMode::ForegroundExclusive,
        };
    }
    CatalogStartupWithoutSummaryPlan::NoCatalog
}

fn startup_intro_is_eligible(
    startup_mode: StartupMode,
    predecessor_catalog_migration_required: bool,
    screensaver_start_mode: ScreensaverStartMode,
    portrait: bool,
) -> bool {
    startup_mode == StartupMode::ColdNoCatalog
        && (predecessor_catalog_migration_required
            || (screensaver_start_mode == ScreensaverStartMode::Inactive && !portrait))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DeferredCatalogWorkerStartPolicy {
    allowed: bool,
    delay: Duration,
    foreground: bool,
}

fn deferred_catalog_worker_start_policy(
    catalog_ready: bool,
    first_visible_copy_done: bool,
    startup_return_waiting_for_catalog: bool,
    startup_waiting_for_initial_catalog: bool,
    background_delay: Duration,
) -> DeferredCatalogWorkerStartPolicy {
    if catalog_ready {
        DeferredCatalogWorkerStartPolicy {
            allowed: true,
            delay: background_delay,
            foreground: false,
        }
    } else {
        DeferredCatalogWorkerStartPolicy {
            allowed: first_visible_copy_done
                || startup_return_waiting_for_catalog
                || startup_waiting_for_initial_catalog,
            delay: Duration::ZERO,
            foreground: true,
        }
    }
}

fn deferred_catalog_worker_lifecycle_input(
    execution_mode: CatalogExecutionMode,
    request: CatalogWorkerRequest,
) -> LauncherLifecycleInput {
    if execution_mode == CatalogExecutionMode::ForegroundExclusive {
        LauncherLifecycleInput::CatalogBuilding {
            mode: if request == CatalogWorkerRequest::FreshBuild {
                CatalogBuildMode::FreshRecovery
            } else {
                CatalogBuildMode::FirstBuild
            },
            foreground: matches!(
                request,
                CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS | CatalogWorkerRequest::FreshBuild
            ),
            has_stale_catalog: false,
        }
    } else {
        LauncherLifecycleInput::CatalogValidationStarted
    }
}

fn initial_catalog_scan_visible(
    catalog_ready: bool,
    catalog_worker_enabled: bool,
    foreground_update: bool,
    startup_waiting_for_initial_catalog: bool,
) -> bool {
    catalog_worker_enabled
        && !startup_waiting_for_initial_catalog
        && (foreground_update || !catalog_ready)
}

fn should_draw_arcade_overlay(
    nav: &LauncherNav,
    launching: bool,
    active_arcade_games_available: bool,
) -> bool {
    !launching
        && nav.screen == Screen::Arcade
        && !nav.is_system_hub()
        && active_arcade_games_available
}

fn update_arcade_physical_layer_tracking(
    version: &mut u64,
    content_offset: &mut LayerOffset,
    update: Option<ArcadeListUpdate>,
    publication_tracks_content_generation: bool,
) {
    match update {
        Some(ArcadeListUpdate::Full(_)) if !publication_tracks_content_generation => {
            *version = version.wrapping_add(1).max(1);
        }
        Some(ArcadeListUpdate::Scroll {
            delta_x, delta_y, ..
        }) => {
            content_offset.x = content_offset.x.saturating_add(delta_x as i64);
            content_offset.y = content_offset.y.saturating_add(delta_y as i64);
        }
        _ => {}
    }
}

fn ready_catalog_worker_request(refresh_policy: CatalogRefreshPolicy) -> CatalogWorkerRequest {
    if refresh_policy.force_requested() {
        CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS
    } else {
        CatalogWorkerRequest::LoadOnly
    }
}

fn defer_warm_registry_hydration(
    capsule_seed_ready: bool,
    startup_return_requested: bool,
    manifest_slots_present: bool,
    forced_refresh_requested: bool,
) -> bool {
    !capsule_seed_ready
        && !startup_return_requested
        && manifest_slots_present
        && !forced_refresh_requested
}

fn summary_seed_catalog_worker_request(
    refresh_policy: CatalogRefreshPolicy,
    deferred_library_rebuild: bool,
    return_catalog_hydration_needed: bool,
) -> Option<CatalogWorkerRequest> {
    if deferred_library_rebuild {
        return Some(CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS);
    }
    let request = ready_catalog_worker_request(refresh_policy);
    if return_catalog_hydration_needed {
        return Some(if request == CatalogWorkerRequest::LoadOnly {
            CatalogWorkerRequest::StrictLoad
        } else {
            request
        });
    }
    (request != CatalogWorkerRequest::LoadOnly && refresh_policy.worker_enabled())
        .then_some(request)
}

fn summary_seed_catalog_worker_starts_immediately(
    request: CatalogWorkerRequest,
    return_catalog_hydration_needed: bool,
) -> bool {
    request == CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS || return_catalog_hydration_needed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_event::InputSourceKind;

    #[test]
    fn queued_settings_activation_retains_its_transition_source_on_the_settling_tick() {
        let catalog = empty_arcade_catalog("/tmp");
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        let start = Instant::now();
        nav.selected = 4;
        nav.handle_held_tick_with_navigation_intents(&PadState::default(), start, &catalog);
        let direction = LauncherUiAction::Navigate(slint_ui::launcher::NavigationDirection::Right)
            .input_pulse(1, 16_000)
            .unwrap();
        nav.handle_action_with_navigation_intents(
            &direction[0],
            start + Duration::from_millis(16),
            &catalog,
        );
        nav.handle_action_with_navigation_intents(
            &direction[1],
            start + Duration::from_millis(32),
            &catalog,
        );
        assert_eq!(nav.selected, 5);
        let activation = LauncherUiAction::Activate.input_pulse(2, 48_000).unwrap();
        nav.handle_action_with_navigation_intents(
            &activation[0],
            start + Duration::from_millis(48),
            &catalog,
        );
        nav.handle_action_with_navigation_intents(
            &activation[1],
            start + Duration::from_millis(64),
            &catalog,
        );
        assert_eq!(nav.screen, Screen::Home);
        assert!(nav.pending_settings_activation());
        assert!(!settings_navigation_input_candidate(nav.screen, None));
        let mut opened = false;
        for frame in 5..180 {
            let source = settings_navigation_source_candidate(&nav, None)
                .then(|| (nav.screen, nav.navigation_transition_state()));
            nav.handle_held_tick_with_navigation_intents(
                &PadState::default(),
                start + Duration::from_millis(frame * 16),
                &catalog,
            );
            if nav.screen == Screen::Settings {
                let (source_screen, _) =
                    source.expect("queued activation lost its source on an idle tick");
                assert_eq!(
                    settings_page_transition(source_screen, nav.screen),
                    Some((
                        NavigationTransitionRoute::HomeToSettings,
                        NavigationTransitionDirection::Forward
                    ))
                );
                assert!(!nav.pending_settings_activation());
                opened = true;
                break;
            }
        }
        assert!(opened, "queued Settings activation never completed");
    }

    #[test]
    fn settings_activation_waits_for_the_moving_card_then_fires_once() {
        let mut nav = LauncherNav::new();
        nav.selected = 5;
        let activation = normalized_test_press(LogicalAction::Activate);
        let mut routed = Some(activation);
        let mut deferred = DeferredSettingsActivation::default();

        assert!(deferred.intercept_while_cards_move(&nav, true, &mut routed));
        assert!(routed.is_none());
        assert!(deferred.is_pending());
        assert!(deferred.take_when_settled(true).is_none());
        let settled_activation = deferred
            .take_when_settled(false)
            .expect("settled card should release the queued activation");
        assert_eq!(settled_activation, activation);
        assert!(!deferred.is_pending());
        assert!(deferred.take_when_settled(false).is_none());

        let catalog = empty_arcade_catalog("/tmp");
        assert!(
            nav.handle_action_with_navigation_intents(
                &settled_activation,
                Instant::now(),
                &catalog,
            )
            .is_none()
        );
        assert_eq!(nav.screen, Screen::Settings);
    }

    #[test]
    fn reverse_card_and_cog_home_endpoints_are_live_handoffs() {
        let reverse_cog =
            NavigationTransitionRequest::settings_cog(NavigationTransitionDirection::Reverse);
        assert!(navigation_home_endpoint_is_live(
            Some(NavigationTransitionRoute::HomeToSettings),
            Some(reverse_cog),
            Some(NavigationTransitionEndpoint::Destination),
        ));
        assert!(navigation_home_endpoint_is_live(
            Some(NavigationTransitionRoute::HomeToArcade),
            Some(NavigationTransitionRequest::device_card(
                NavigationTransitionDirection::Reverse,
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionGeometry::default(),
                mister_magik_framebuffer_scenes::device_card::DeviceCardReveal::cabinet(false),
            )),
            Some(NavigationTransitionEndpoint::Destination),
        ));
        assert!(!navigation_home_endpoint_is_live(
            Some(NavigationTransitionRoute::HomeToSettings),
            Some(NavigationTransitionRequest::settings_cog(
                NavigationTransitionDirection::Forward,
            )),
            Some(NavigationTransitionEndpoint::Destination),
        ));
        assert!(!navigation_home_endpoint_is_live(
            Some(NavigationTransitionRoute::HomeToSettings),
            Some(reverse_cog),
            Some(NavigationTransitionEndpoint::Source),
        ));
        assert!(!navigation_home_endpoint_is_live(
            Some(NavigationTransitionRoute::HomeToSettings),
            Some(NavigationTransitionRequest::device_card(
                NavigationTransitionDirection::Reverse,
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionGeometry::default(),
                mister_magik_framebuffer_scenes::device_card::DeviceCardReveal::cabinet(false),
            )),
            Some(NavigationTransitionEndpoint::Destination),
        ));
    }

    #[test]
    fn reverse_arcade_live_handoff_consumes_retained_redraw_without_release_raster() {
        let mut transition = FullScreenTransitionStateChart::default();
        let generation = transition
            .begin(FullScreenTransitionOwner::Navigation)
            .unwrap();
        transition.retain_redraw(generation).unwrap();
        assert!(transition.take_controlled_capture(generation).unwrap());
        transition.capture_completed(generation).unwrap();
        transition.release(generation).unwrap();

        let live_endpoint = navigation_home_endpoint_is_live(
            Some(NavigationTransitionRoute::HomeToArcade),
            Some(NavigationTransitionRequest::device_card(
                NavigationTransitionDirection::Reverse,
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionGeometry::default(),
                mister_magik_framebuffer_scenes::device_card::DeviceCardReveal::cabinet(false),
            )),
            Some(NavigationTransitionEndpoint::Destination),
        );
        let retained_redraw = transition.live_frame_presented(generation).unwrap();

        assert!(live_endpoint);
        assert!(retained_redraw);
        assert!(!(retained_redraw && !live_endpoint));
        assert_eq!(transition.state(), FullScreenTransitionState::Live);
    }

    fn eligible_card_direct_input() -> CardDirectEligibility {
        CardDirectEligibility {
            custom_home_active: true,
            custom_home_needs_render: true,
            direct_geometry: true,
            full_frame_present: false,
            launching: false,
            screensaver_active: false,
            startup_intro_active: false,
            startup_reveal_suppressed: false,
            startup_intro_suppressed: false,
            confirm_visible: false,
            catalog_scan_visible: false,
            navigation_transition_active: false,
            orientation_transition_active: false,
            composition_state: UiCompositionState::FullSlint,
            force_full_slint_raster: false,
            force_full_slint_present: false,
            transition_state: FullScreenTransitionState::Live,
        }
    }

    #[test]
    fn card_direct_hidden_requires_unobstructed_native_home() {
        assert!(card_direct_hidden_eligible(eligible_card_direct_input()));
        for blocked in [
            CardDirectEligibility {
                confirm_visible: true,
                ..eligible_card_direct_input()
            },
            CardDirectEligibility {
                catalog_scan_visible: true,
                ..eligible_card_direct_input()
            },
            CardDirectEligibility {
                navigation_transition_active: true,
                ..eligible_card_direct_input()
            },
            CardDirectEligibility {
                orientation_transition_active: true,
                ..eligible_card_direct_input()
            },
            CardDirectEligibility {
                direct_geometry: false,
                ..eligible_card_direct_input()
            },
            CardDirectEligibility {
                full_frame_present: true,
                ..eligible_card_direct_input()
            },
            CardDirectEligibility {
                composition_state: UiCompositionState::Recovering,
                ..eligible_card_direct_input()
            },
            CardDirectEligibility {
                force_full_slint_raster: true,
                ..eligible_card_direct_input()
            },
        ] {
            assert!(!card_direct_hidden_eligible(blocked));
        }
    }

    #[test]
    fn base_damage_is_shielded_only_by_a_full_reapply_publication() {
        let full = DirtyRect {
            x0: 0,
            y0: 0,
            x1: 8,
            y1: 6,
        };
        let layer = DirtyRect {
            x0: 2,
            y0: 1,
            x1: 7,
            y1: 5,
        };
        let backing = PhysicalLayerBacking::new(layer, Rgb565Pixel(0x1234))
            .expect("test layer has nonempty geometry");
        let mut publication = PhysicalLayerPublication::capture_owned(
            PhysicalLayerRole::Preview,
            3,
            1,
            9,
            PhysicalLayerState::new(layer, 4),
            None,
            backing,
        );

        let shielded =
            shield_base_damage_under_publication(DirtyRectList::from_one(full), &mut publication);

        assert!(
            shielded
                .iter()
                .all(|rect| rect.intersection(layer).is_none())
        );
        assert_eq!(
            publication.and_then(|publication| publication.update()),
            Some(PhysicalLayerUpdate::Full(layer))
        );

        let damage = DirtyRectList::from_one(full);
        assert_eq!(
            shield_base_damage_under_publication(damage, &mut None),
            damage
        );
    }

    #[test]
    fn discrete_feedback_targets_cover_included_and_excluded_surfaces() {
        let mut nav = LauncherNav::new();

        nav.screen = Screen::Arcade;
        nav.system_page_mode = launcher::SystemPageMode::Hub;
        nav.system_hub_selected = 2;
        assert_eq!(
            nav_selection_feedback_target(&nav),
            Some(SelectionFeedbackTarget::new("system-hub", "favorites"))
        );

        nav.screen = Screen::Settings;
        nav.settings_selected = 2;
        assert_eq!(
            nav_selection_feedback_target(&nav),
            Some(SelectionFeedbackTarget::new("settings", "reduce-motion"))
        );
        nav.display_combo_open = true;
        nav.display_highlighted = 4;
        assert_eq!(
            nav_selection_feedback_target(&nav),
            Some(SelectionFeedbackTarget::new("display-combo", "option:4"))
        );
        nav.display_combo_open = false;

        nav.screen = Screen::About;
        assert_eq!(
            nav_selection_feedback_target(&nav),
            Some(SelectionFeedbackTarget::new("about", "licenses"))
        );

        nav.screen = Screen::Licenses;
        nav.licenses_selected = 2;
        assert_eq!(
            nav_selection_feedback_target(&nav),
            Some(SelectionFeedbackTarget::new("licenses", "slint"))
        );
        nav.screen = Screen::Arcade;
        nav.system_page_mode = launcher::SystemPageMode::List;
        assert_eq!(nav_selection_feedback_target(&nav), None);
        nav.arcade_filter.drawer_open = true;
        nav.arcade_filter.selected = 3;
        assert_eq!(nav_selection_feedback_target(&nav), None);
        nav.arcade_filter.drawer_open = false;
        nav.arcade_filter.active = arcade_catalog::ArcadeFilter::Search;
        nav.arcade_search.pane = launcher::ArcadeSearchPane::Keyboard;
        nav.arcade_search.selected_key = 9;
        assert_eq!(
            nav_selection_feedback_target(&nav)
                .expect("search keyboard target")
                .item,
            "key:9"
        );
        nav.arcade_search.pane = launcher::ArcadeSearchPane::Results;
        assert_eq!(nav_selection_feedback_target(&nav), None);

        nav.screen = Screen::Controller;
        assert_eq!(nav_selection_feedback_target(&nav), None);
        nav.screen = Screen::LicenseText;
        assert_eq!(nav_selection_feedback_target(&nav), None);
    }

    #[test]
    fn controller_setup_feedback_is_limited_to_discrete_choices() {
        let mut setup = SetupNav::new();
        setup.phase = SetupPhase::NewOrExisting;
        setup.list_index = 1;
        assert_eq!(
            setup_selection_feedback_target(&setup)
                .expect("new-or-existing target")
                .item,
            "existing"
        );
        setup.phase = SetupPhase::PickExisting;
        setup.list_index = 5;
        assert_eq!(
            setup_selection_feedback_target(&setup)
                .expect("saved controller target")
                .item,
            "saved:5"
        );
        setup.phase = SetupPhase::Configure;
        assert_eq!(setup_selection_feedback_target(&setup), None);
    }

    #[test]
    fn feedback_registration_requires_an_accepted_pressed_dispatch() {
        let pressed = normalized_test_press(LogicalAction::Right);
        let mut released = pressed;
        released.phase = InputPhase::Released;

        assert!(accepted_selection_feedback_input(Some(&pressed)));
        assert!(!accepted_selection_feedback_input(Some(&released)));
        assert!(!accepted_selection_feedback_input(None));
    }

    #[test]
    fn interactive_frames_defer_launcher_background_work() {
        assert!(!should_defer_launcher_background_work(
            0, false, false, false
        ));
        assert!(should_defer_launcher_background_work(
            1, false, false, false
        ));
        assert!(should_defer_launcher_background_work(0, true, false, false));
        assert!(should_defer_launcher_background_work(0, false, true, false));
        assert!(should_defer_launcher_background_work(0, false, false, true));
    }

    #[test]
    fn catalog_poll_scope_preserves_control_liveness_across_launcher_states() {
        let scopes = [
            catalog_poll_scope(true, false, false),
            catalog_poll_scope(false, false, false),
            catalog_poll_scope(false, false, true),
            catalog_poll_scope(false, true, true),
            catalog_poll_scope(false, true, false),
        ];

        assert_eq!(
            scopes,
            [
                Some(CatalogPollScope::Idle),
                Some(CatalogPollScope::Interactive {
                    system_entry_handoff: false,
                }),
                Some(CatalogPollScope::Interactive {
                    system_entry_handoff: true,
                }),
                Some(CatalogPollScope::Transition {
                    system_entry_handoff: true,
                }),
                Some(CatalogPollScope::Transition {
                    system_entry_handoff: false,
                }),
            ]
        );
    }

    #[test]
    fn only_disposable_home_frames_yield_the_latch_wait_to_input() {
        assert!(can_preempt_home_latch_wait(
            Screen::Home,
            false,
            false,
            false,
            false,
            false,
            false,
        ));
        for blocked in 0..6 {
            let mut conditions = [false; 6];
            conditions[blocked] = true;
            assert!(!can_preempt_home_latch_wait(
                Screen::Home,
                conditions[0],
                conditions[1],
                conditions[2],
                conditions[3],
                conditions[4],
                conditions[5],
            ));
        }
        assert!(!can_preempt_home_latch_wait(
            Screen::Settings,
            false,
            false,
            false,
            false,
            false,
            false,
        ));
    }

    #[test]
    fn only_disposable_home_rasters_yield_to_new_input() {
        assert!(can_preempt_disposable_home_raster(
            Screen::Home,
            true,
            false,
            true,
            false,
            false,
            false,
            false,
        ));
        for blocked in 0..4 {
            let mut conditions = [false; 4];
            conditions[blocked] = true;
            assert!(!can_preempt_disposable_home_raster(
                Screen::Home,
                true,
                false,
                true,
                conditions[0],
                conditions[1],
                conditions[2],
                conditions[3],
            ));
        }
        assert!(!can_preempt_disposable_home_raster(
            Screen::Settings,
            true,
            false,
            true,
            false,
            false,
            false,
            false,
        ));
        assert!(!can_preempt_disposable_home_raster(
            Screen::Home,
            false,
            false,
            true,
            false,
            false,
            false,
            false,
        ));
        assert!(!can_preempt_disposable_home_raster(
            Screen::Home,
            true,
            true,
            true,
            false,
            false,
            false,
            false,
        ));
        assert!(!can_preempt_disposable_home_raster(
            Screen::Home,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
        ));
    }

    #[test]
    fn urgent_input_restarts_only_an_empty_noninteractive_loop() {
        assert!(should_restart_for_urgent_input(true, false, true));
        assert!(!should_restart_for_urgent_input(false, false, true));
        assert!(!should_restart_for_urgent_input(true, true, true));
        assert!(!should_restart_for_urgent_input(true, false, false));
    }

    #[test]
    fn startup_intro_preserves_first_visible_build_planning() {
        assert_eq!(
            startup_intro_catalog_worker_request(CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS),
            CatalogWorkerRequest::CheckStamp
        );
        assert_eq!(
            startup_intro_catalog_worker_request(CatalogWorkerRequest::FreshBuild),
            CatalogWorkerRequest::FreshBuild
        );
    }

    fn crt_240_display() -> UiDisplay {
        let plan = UiDisplayPlan::from_mister_ini_text(
            "[MiSTer]\ndirect_video=1\nmenu_pal=0\nforced_scandoubler=0\n",
        )
        .expect("CRT240 display plan");
        UiDisplay::for_plan(plan)
    }

    #[test]
    fn navigation_destination_uses_crt_240_arcade_geometry() {
        let ui = crt_240_display();
        let metrics = CrtUiMetrics::for_display(&ui);
        let nav = LauncherNav::for_crt_layout_with_row_height(true, metrics.game_row_height);
        let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &ui);

        configure_arcade_list_renderer_geometry(&mut renderer, &nav, &ui);

        assert_eq!(
            renderer.dirty_rect(),
            DirtyRect {
                x0: 66,
                y0: 133,
                x1: 428,
                y1: 392,
            }
        );
    }

    #[test]
    fn crt_240_arcade_composition_leaves_header_and_footer_bands_untouched() {
        let ui = crt_240_display();
        let metrics = CrtUiMetrics::for_display(&ui);
        let nav = LauncherNav::for_crt_layout_with_row_height(true, metrics.game_row_height);
        let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &ui);
        configure_arcade_list_renderer_geometry(&mut renderer, &nav, &ui);
        let games = (0..20)
            .map(|index| arcade_game(format!("Game {index}")).build())
            .collect::<Vec<_>>();
        let sentinel = <Rgb565Pixel as TargetPixel>::from_rgb(255, 0, 255);
        let mut target = UiFrameTarget::cached(frame_target_geometry(&ui));
        target.cached_565_mut().fill(sentinel);

        let update = renderer
            .draw(ArcadeGameView::contiguous(&games), 0, 0.0, true)
            .expect("forced Arcade list composition");
        let _ = compose_arcade_list_update(&mut target, &mut renderer, update);

        let pixels = target.cached_frame_view().pixels();
        for band in [56..104, 416..448] {
            assert!(
                band.flat_map(|y| &pixels[y * ui.render_w()..(y + 1) * ui.render_w()])
                    .all(|pixel| *pixel == sentinel)
            );
        }
    }

    #[test]
    fn shared_arcade_geometry_preserves_hdmi_and_crt_search_layouts() {
        let hdmi = UiDisplay::for_framebuffer(960, 540);
        let hdmi_nav = LauncherNav::new();
        let mut hdmi_renderer = ArcadeListRenderer::new();
        configure_arcade_list_renderer_geometry(&mut hdmi_renderer, &hdmi_nav, &hdmi);
        assert_eq!(
            hdmi_renderer.dirty_rect(),
            DirtyRect {
                x0: 26,
                y0: 88,
                x1: 488,
                y1: 484,
            }
        );
        assert_eq!(
            (hdmi_renderer.selection_rect().y0 - hdmi_renderer.dirty_rect().y0)
                / ARCADE_ROW_HEIGHT as usize,
            3
        );

        let crt = crt_240_display();
        let metrics = CrtUiMetrics::for_display(&crt);
        let mut crt_nav =
            LauncherNav::for_crt_layout_with_row_height(true, metrics.game_row_height);
        crt_nav.arcade_filter.active = arcade_catalog::ArcadeFilter::Search;
        let mut crt_renderer = ArcadeListRenderer::new_for_crt_display(metrics, &crt);
        configure_arcade_list_renderer_geometry(&mut crt_renderer, &crt_nav, &crt);
        assert_eq!(
            crt_renderer.dirty_rect(),
            DirtyRect {
                x0: 331,
                y0: 133,
                x1: 574,
                y1: 392,
            }
        );
    }

    #[test]
    fn crt_routes_use_roomier_rows_in_normal_and_search_layouts() {
        for (pal, scandoubler, expected_row_height, expected_full_rows) in
            [(0, 0, 32, 8), (1, 0, 19, 8), (0, 1, 32, 10), (1, 1, 39, 10)]
        {
            let ini = format!(
                "[MiSTer]\ndirect_video=1\nmenu_pal={pal}\nforced_scandoubler={scandoubler}\n"
            );
            let display = UiDisplay::for_plan(
                UiDisplayPlan::from_mister_ini_text(&ini).expect("CRT display plan"),
            );
            let metrics = CrtUiMetrics::for_display(&display);
            assert_eq!(metrics.game_row_height, expected_row_height);
            let mut nav =
                LauncherNav::for_crt_layout_with_row_height(true, metrics.game_row_height);

            for search in [false, true] {
                nav.arcade_filter.active = if search {
                    arcade_catalog::ArcadeFilter::Search
                } else {
                    arcade_catalog::ArcadeFilter::All
                };
                let (geometry, visible_height) = arcade_list_layout(&nav, &display);
                assert_eq!(
                    visible_height / metrics.game_row_height as usize,
                    expected_full_rows,
                    "pal={pal} scandoubler={scandoubler} search={search}"
                );
                let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &display);
                renderer.set_geometry_for_visible_height(geometry, visible_height);
                assert_eq!(
                    (renderer.selection_rect().y0 - renderer.dirty_rect().y0)
                        / metrics.game_row_height as usize,
                    expected_full_rows.saturating_sub(1).min(3),
                    "pal={pal} scandoubler={scandoubler} search={search}"
                );
            }
        }

        let hdmi = ArcadeListRenderer::new();
        assert_eq!(
            hdmi.selection_rect().y1 - hdmi.selection_rect().y0,
            ARCADE_ROW_HEIGHT as usize
        );
    }

    #[test]
    fn settings_page_routes_use_depth_for_forward_and_reverse_motion() {
        assert_eq!(
            settings_page_transition(Screen::Home, Screen::Settings),
            Some((
                NavigationTransitionRoute::HomeToSettings,
                NavigationTransitionDirection::Forward
            ))
        );
        assert_eq!(
            settings_page_transition(Screen::Settings, Screen::About),
            Some((
                NavigationTransitionRoute::SettingsToAbout,
                NavigationTransitionDirection::Forward
            ))
        );
        assert_eq!(
            settings_page_transition(Screen::About, Screen::Licenses),
            Some((
                NavigationTransitionRoute::AboutToLicenses,
                NavigationTransitionDirection::Forward
            ))
        );
        assert_eq!(
            settings_page_transition(Screen::Licenses, Screen::About),
            Some((
                NavigationTransitionRoute::AboutToLicenses,
                NavigationTransitionDirection::Reverse
            ))
        );
        assert_eq!(
            settings_page_transition(Screen::Licenses, Screen::LicenseText),
            Some((
                NavigationTransitionRoute::LicensesToLicenseText,
                NavigationTransitionDirection::Forward
            ))
        );
        assert_eq!(
            settings_page_transition(Screen::LicenseText, Screen::Licenses),
            Some((
                NavigationTransitionRoute::LicensesToLicenseText,
                NavigationTransitionDirection::Reverse
            ))
        );
        assert_eq!(
            settings_page_transition(Screen::LicenseText, Screen::Home),
            Some((
                NavigationTransitionRoute::NestedToHome,
                NavigationTransitionDirection::Reverse
            ))
        );
        assert_eq!(settings_page_transition(Screen::Home, Screen::Arcade), None);
        assert_eq!(
            settings_page_transition(Screen::Settings, Screen::LicenseText),
            None
        );
    }

    #[test]
    fn catalog_recovery_consumes_a_until_release() {
        let catalog = catalog_for_media_systems(&["arcade"]);
        let mut nav = LauncherNav::new();
        let now = Instant::now();

        let event = normalized_test_press(crate::input_event::LogicalAction::Activate);
        let input = route_lifecycle_dialog_input(Some(&event), false, true);
        assert!(matches!(
            input,
            Some(LauncherLifecycleInput::CatalogRecoveryConfirm)
        ));
        assert_eq!(nav.screen, Screen::Home);

        let event = nav
            .handle_action_with_navigation_intents(
                &normalized_test_press(crate::input_event::LogicalAction::Activate),
                now + Duration::from_millis(32),
                &catalog,
            )
            .expect("fresh A should reach the selected Arcade tile");
        assert_eq!(event.action, LauncherAction::OpenCollection);
        assert_eq!(event.path.as_deref(), Some("menu:arcade"));
    }

    #[test]
    fn library_reset_reboot_wait_expires_and_resumes_input_without_retrying() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut reset = LibraryResetState::Deleting(receiver);
        let now = Instant::now();
        assert_eq!(reset.poll(now), Ok(true));
        sender
            .send(Ok(launcher::PurgeLibraryDataOutcome::default()))
            .unwrap();
        assert_eq!(reset.poll(now), Ok(true));
        assert!(matches!(reset, LibraryResetState::RebootRequested { .. }));
        assert_eq!(
            reset.poll(now + LIBRARY_RESET_REBOOT_TIMEOUT - Duration::from_millis(1)),
            Ok(true)
        );

        let error = reset
            .poll(now + LIBRARY_RESET_REBOOT_TIMEOUT)
            .expect_err("missing reboot must time out");
        assert!(error.contains("MiSTer did not reboot"));
        assert!(matches!(reset, LibraryResetState::Idle));
        assert_eq!(reset.poll(now + LIBRARY_RESET_REBOOT_TIMEOUT), Ok(false));

        let catalog = empty_arcade_catalog("/tmp");
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Settings;
        nav.show_library_reset_error(error);
        let press = normalized_test_press(LogicalAction::Activate);
        assert!(
            nav.handle_action_with_navigation_intents(&press, now, &catalog)
                .is_none()
        );
        assert_eq!(nav.confirm_action, None);
    }

    #[test]
    fn library_reset_worker_failure_and_disconnect_resume_launcher() {
        for disconnect in [false, true] {
            let (sender, receiver) = std::sync::mpsc::channel();
            let mut reset = LibraryResetState::Deleting(receiver);
            if !disconnect {
                sender.send(Err("delete failed".into())).unwrap();
            }
            drop(sender);
            let error = reset.poll(Instant::now()).expect_err("failed worker");
            assert_eq!(
                error,
                if disconnect {
                    "Database reset worker stopped unexpectedly"
                } else {
                    "delete failed"
                }
            );
            assert!(matches!(reset, LibraryResetState::Idle));
            assert_eq!(reset.poll(Instant::now()), Ok(false));
        }
    }

    #[test]
    fn sequential_dispatch_recomputes_focus_after_modal_opens() {
        let catalog = empty_arcade_catalog("/tmp");
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Settings;
        nav.settings_selected = 5;
        let initial_focus = launcher_input_focus(true, false, false, false, false, false, &nav);
        let mut router = InputRouter::new(initial_focus);
        let now = Instant::now();

        let activate = normalized_test_press(LogicalAction::Activate);
        let InputOutcome::Dispatch { event, .. } = router.route_event(activate, initial_focus, now)
        else {
            panic!("activate should dispatch to settings");
        };
        assert!(
            nav.handle_action_with_navigation_intents(&event, now, &catalog)
                .is_none()
        );
        assert!(nav.confirm_action.is_some());

        let modal_focus = launcher_input_focus(true, false, false, false, true, false, &nav);
        let mut right = normalized_test_press(LogicalAction::Right);
        right.sequence = 2;
        right.press_id = crate::input_event::PressId(2);
        let InputOutcome::Dispatch { event, context, .. } =
            router.route_event(right, modal_focus, now)
        else {
            panic!("right should dispatch to the newly opened modal");
        };
        assert_eq!(context.target.kind, InputContextKind::LauncherModal);
        assert!(
            nav.handle_action_with_navigation_intents(&event, now, &catalog)
                .is_none()
        );
        assert_eq!(nav.confirm_selected, 1);
    }

    #[test]
    fn rapid_second_back_is_swallowed_after_settings_exit() {
        let catalog = empty_arcade_catalog("/tmp");
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Settings;
        let settings_focus = launcher_screen_input_focus(&nav);
        let mut router = InputRouter::new(settings_focus);
        let now = Instant::now();

        let first_back = normalized_test_press(LogicalAction::Back);
        let InputOutcome::Dispatch { event, .. } =
            router.route_event(first_back, settings_focus, now)
        else {
            panic!("the first Back should leave Settings");
        };
        assert!(
            nav.handle_action_with_navigation_intents(&event, now, &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::Home);
        assert!(settings_page_transition(Screen::Settings, nav.screen).is_some());

        let transition_focus = launcher_input_focus(true, false, false, false, false, true, &nav);
        let mut first_release = first_back;
        first_release.sequence = 2;
        first_release.phase = InputPhase::Released;
        assert!(matches!(
            router.route_event(first_release, transition_focus, now),
            InputOutcome::Released { context, .. } if context.target == settings_focus.target
        ));

        let mut second_back = normalized_test_press(LogicalAction::Back);
        second_back.sequence = 3;
        second_back.press_id = crate::input_event::PressId(2);
        assert!(matches!(
            router.route_event(second_back, transition_focus, now),
            InputOutcome::Consumed {
                reason: ConsumedReason::TransitionActive,
                ..
            }
        ));

        let destination_focus = launcher_screen_input_focus(&nav);
        router.set_focus(destination_focus);
        assert!(!router.action_held(LogicalAction::Back));
        assert!(router.tick_repeat(now + Duration::from_secs(1)).is_none());
        let mut second_release = second_back;
        second_release.sequence = 4;
        second_release.phase = InputPhase::Released;
        assert!(matches!(
            router.route_event(second_release, destination_focus, now),
            InputOutcome::Released { context, .. }
                if context.target.kind == InputContextKind::Transition
        ));
        assert_eq!(nav.screen, Screen::Home);
    }

    #[test]
    fn rapid_second_back_is_swallowed_after_arcade_exit() {
        let catalog = catalog_for_media_systems(&["arcade"]);
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_default_arcade(&catalog));
        let arcade_focus = launcher_screen_input_focus(&nav);
        let mut router = InputRouter::new(arcade_focus);
        let now = Instant::now();

        let first_back = normalized_test_press(LogicalAction::Back);
        let InputOutcome::Dispatch { event, .. } =
            router.route_event(first_back, arcade_focus, now)
        else {
            panic!("the first Back should leave Arcade");
        };
        let navigation = nav
            .handle_action_with_navigation_intents(&event, now, &catalog)
            .expect("Arcade Back should produce a navigation intent");
        assert_eq!(navigation.action, LauncherAction::NavigateBack);
        assert!(navigation_transition_for_intent(&nav, &navigation, false).is_some());
        assert!(nav.commit_navigation_intent(&navigation, &catalog));
        let destination_screen = nav.screen;
        assert_ne!(destination_screen, Screen::Arcade);

        let transition_focus = launcher_input_focus(true, false, false, false, false, true, &nav);
        let mut first_release = first_back;
        first_release.sequence = 2;
        first_release.phase = InputPhase::Released;
        assert!(matches!(
            router.route_event(first_release, transition_focus, now),
            InputOutcome::Released { context, .. } if context.target == arcade_focus.target
        ));

        let mut second_back = normalized_test_press(LogicalAction::Back);
        second_back.sequence = 3;
        second_back.press_id = crate::input_event::PressId(2);
        assert!(matches!(
            router.route_event(second_back, transition_focus, now),
            InputOutcome::Consumed {
                reason: ConsumedReason::TransitionActive,
                ..
            }
        ));

        let destination_focus = launcher_screen_input_focus(&nav);
        router.set_focus(destination_focus);
        assert!(!router.action_held(LogicalAction::Back));
        assert!(router.tick_repeat(now + Duration::from_secs(1)).is_none());
        let mut second_release = second_back;
        second_release.sequence = 4;
        second_release.phase = InputPhase::Released;
        assert!(matches!(
            router.route_event(second_release, destination_focus, now),
            InputOutcome::Released { context, .. }
                if context.target.kind == InputContextKind::Transition
        ));
        assert_eq!(nav.screen, destination_screen);
    }

    #[test]
    fn launch_failure_consumes_every_acknowledgement_button() {
        for action in [
            crate::input_event::LogicalAction::Activate,
            crate::input_event::LogicalAction::Back,
            crate::input_event::LogicalAction::Home,
        ] {
            let nav = LauncherNav::new();
            let event = normalized_test_press(action);
            let input = route_lifecycle_dialog_input(Some(&event), true, false);
            assert!(matches!(
                input,
                Some(LauncherLifecycleInput::LaunchFailureAcknowledge)
            ));
            assert_eq!(nav.screen, Screen::Home);
        }
    }

    #[test]
    fn in_flight_arcade_preview_result_is_deferred_for_the_whole_transition() {
        assert!(should_defer_or_preserve_selected_preview(false, true, true,));
        assert!(!should_defer_or_preserve_selected_preview(
            false, false, true,
        ));
        assert!(!should_defer_or_preserve_selected_preview(
            false, true, false,
        ));
        assert!(should_defer_or_preserve_selected_preview(
            true, false, false,
        ));
    }

    #[test]
    fn selected_preview_work_remains_live_during_normal_and_turbo_scroll() {
        assert!(preview_work_allowed(false, false, true, false));
        assert!(preview_work_allowed(false, false, true, true));
        assert!(preview_work_allowed(false, true, false, false));
        assert!(preview_work_allowed(true, false, false, false));
        assert!(!preview_work_allowed(false, false, false, false));
    }

    #[test]
    fn return_capsule_seed_opens_the_generation_reader_before_input() {
        assert!(initial_system_entry_reader_required(true, false));
        assert!(initial_system_entry_reader_required(false, true));
        assert!(!initial_system_entry_reader_required(false, false));
    }

    #[test]
    fn committed_navigation_can_restore_its_exact_source_menu() {
        let catalog = catalog_for_media_systems(&["psx"]);
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        let enter = launcher::LauncherEvent {
            action: LauncherAction::OpenMenu,
            path: Some(crate::launcher_taxonomy::CONSOLES_MENU_ID.to_string()),
            settings: None,
        };
        let root_state = nav.navigation_transition_state();

        assert!(nav.commit_navigation_intent(&enter, &catalog));
        assert_eq!(
            nav.current_menu_id(),
            crate::launcher_taxonomy::CONSOLES_MENU_ID
        );
        nav.restore_navigation_transition_state(root_state);
        assert_eq!(
            nav.current_menu_id(),
            crate::launcher_taxonomy::ROOT_MENU_ID
        );

        assert!(nav.commit_navigation_intent(&enter, &catalog));
        let consoles_state = nav.navigation_transition_state();
        let leave = launcher::LauncherEvent {
            action: LauncherAction::NavigateBack,
            path: None,
            settings: None,
        };
        assert!(nav.commit_navigation_intent(&leave, &catalog));
        assert_eq!(
            nav.current_menu_id(),
            crate::launcher_taxonomy::ROOT_MENU_ID
        );
        nav.restore_navigation_transition_state(consoles_state);
        assert_eq!(
            nav.current_menu_id(),
            crate::launcher_taxonomy::CONSOLES_MENU_ID
        );
    }

    #[test]
    fn screensaver_retains_launcher_then_defers_recycling_until_after_present() {
        let mut launcher_frame = None;
        let mut recycle_after_present = None;

        retain_or_defer_screensaver_buffer(
            &mut launcher_frame,
            &mut recycle_after_present,
            vec![Rgb565Pixel(1)],
        );
        assert_eq!(launcher_frame.as_deref(), Some(&[Rgb565Pixel(1)][..]));
        assert!(recycle_after_present.is_none());

        retain_or_defer_screensaver_buffer(
            &mut launcher_frame,
            &mut recycle_after_present,
            vec![Rgb565Pixel(2)],
        );
        assert_eq!(launcher_frame.as_deref(), Some(&[Rgb565Pixel(1)][..]));
        assert_eq!(
            recycle_after_present.as_deref(),
            Some(&[Rgb565Pixel(2)][..])
        );
    }

    #[test]
    fn copied_and_external_direct_frames_count_as_visible_presentations() {
        assert!(visible_frame_was_presented(
            720,
            LauncherPresentStatus::Ok,
            LatchCopyPath::IdentityFull.label(),
        ));
        assert!(visible_frame_was_presented(
            0,
            LauncherPresentStatus::Ok,
            LatchCopyPath::ExternalDirect.label(),
        ));
        assert!(visible_frame_was_presented(
            0,
            LauncherPresentStatus::Ok,
            LatchCopyPath::ExternalDirect.label(),
        ));
    }

    use crate::test_support::{arcade_catalog, arcade_game, arcade_system};

    #[test]
    fn crt_route_policy_is_fixed_to_the_supported_backdrop_matrix() {
        let hdmi = PreviewRoutePolicy::for_output_route(ResolvedOutputRoute::Hdmi);
        assert!(hdmi.allows_hdmi_preview());
        assert!(!hdmi.allows_crt_backdrop());

        for route in [
            ResolvedOutputRoute::Crt240p60,
            ResolvedOutputRoute::Crt288p50,
            ResolvedOutputRoute::Crt480p60,
            ResolvedOutputRoute::Crt576p50,
        ] {
            let crt = PreviewRoutePolicy::for_output_route(route);
            assert!(!crt.allows_hdmi_preview());
            assert!(crt.allows_crt_backdrop());
        }
    }

    #[test]
    fn crt_backdrop_acknowledgement_requires_a_settled_full_frame() {
        assert!(crt_backdrop_frame_is_presented(
            false, true, false, true, true, false
        ));
        assert!(!crt_backdrop_frame_is_presented(
            true, true, false, true, true, false
        ));
        assert!(!crt_backdrop_frame_is_presented(
            false, false, false, true, true, false
        ));
        assert!(!crt_backdrop_frame_is_presented(
            false, true, true, true, true, false
        ));
        assert!(!crt_backdrop_frame_is_presented(
            false, true, false, false, true, false
        ));
        assert!(!crt_backdrop_frame_is_presented(
            false, true, false, true, false, false
        ));
        assert!(!crt_backdrop_frame_is_presented(
            false, true, false, true, true, true
        ));
    }

    #[test]
    fn full_present_during_crt_arcade_keeps_same_frame_list_repaint_ownership() {
        let mut composition = UiCompositionController::new();
        let input = UiCompositionInput {
            screensaver_active: false,
            navigation_transition_active: false,
            navigation_destination_committed: false,
            navigation_destination_ready: false,
            navigation_destination_layers_ready: false,
            return_screen: Some(Screen::Arcade),
            confirm_visible: false,
            fullscreen_overlay_visible: false,
            arcade_ready: true,
            route_ok: true,
            wants_arcade_list: true,
            wants_preview: false,
            preview_cache_exact: false,
            preview_frame_ready: false,
        };
        let first = composition.tick(input);
        let full_present = composition.tick(input);
        let renderer = ArcadeListRenderer::new_for_crt(24);

        assert!(first.allow_arcade_list_blit);
        assert!(full_present.allow_arcade_list_blit);
        assert!(arcade_list_needs_forced_redraw(&renderer, None, true));
    }

    #[test]
    fn landscape_full_arcade_update_advances_layer_identity_for_both_slots() {
        let rect = DirtyRect {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 3,
        };
        let mut landscape_version = 1;
        let mut landscape_offset = LayerOffset::ZERO;

        update_arcade_physical_layer_tracking(
            &mut landscape_version,
            &mut landscape_offset,
            Some(ArcadeListUpdate::Full(rect)),
            false,
        );
        assert_eq!(landscape_version, 2);
        assert_eq!(landscape_offset, LayerOffset::ZERO);

        update_arcade_physical_layer_tracking(
            &mut landscape_version,
            &mut landscape_offset,
            Some(ArcadeListUpdate::Scroll {
                delta_x: -2,
                delta_y: 3,
                rect,
                repair_rect: None,
            }),
            false,
        );
        assert_eq!(landscape_version, 2);
        assert_eq!(landscape_offset, LayerOffset::new(-2, 3));

        let mut portrait_version = 1;
        let mut portrait_offset = LayerOffset::ZERO;
        update_arcade_physical_layer_tracking(
            &mut portrait_version,
            &mut portrait_offset,
            Some(ArcadeListUpdate::Full(rect)),
            true,
        );
        assert_eq!(portrait_version, 1);
        assert_eq!(portrait_offset, LayerOffset::ZERO);
    }

    #[test]
    fn media_stays_gated_through_ready_and_opens_after_completion() {
        let mut session = LauncherCatalogSession::new(false);
        let idle = MediaInteractionGate {
            active: false,
            reason: "idle",
        };
        let ready = CatalogWorkerMessage::Ready {
            catalog: catalog_for_media_systems(&["arcade"]),
            load_us: 0,
            source: CatalogSource::FreshBuild,
            durable_save_pending: true,
            generation_fingerprint: None,
            publication_ack: None,
        };
        session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                catalog_partial: false,
            },
            ready,
        );
        let gated = catalog_build_media_gate(session.refresh_done(), idle);
        assert!(gated.active);
        assert_eq!(gated.reason, "catalog-build");

        session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
            },
            CatalogWorkerMessage::Done,
        );
        assert_eq!(catalog_build_media_gate(session.refresh_done(), idle), idle);
    }

    #[test]
    fn startup_intro_consumes_the_existing_launcher_reveal_transition() {
        assert_eq!(
            startup_intro_launcher_ui_plan(true, StartupRevealState::CatalogProgressVisible, false,),
            StartupIntroLauncherUiPlan::Suppress
        );
        assert_eq!(
            startup_intro_launcher_ui_plan(true, StartupRevealState::RevealLauncher, false),
            StartupIntroLauncherUiPlan::PrepareLiveFrame
        );
        assert_eq!(
            startup_intro_launcher_ui_plan(true, StartupRevealState::RevealLauncher, true),
            StartupIntroLauncherUiPlan::Suppress
        );
        assert_eq!(
            startup_intro_launcher_ui_plan(false, StartupRevealState::InputEnabled, true),
            StartupIntroLauncherUiPlan::Interactive
        );
    }

    #[test]
    fn startup_intro_waits_for_full_refresh_or_the_hard_deadline() {
        assert!(!startup_catalog_ready_for_reveal(true, true, false));
        assert!(startup_catalog_ready_for_reveal(true, true, true));
        assert!(startup_catalog_ready_for_reveal(false, true, false));
        assert!(!startup_catalog_ready_for_reveal(true, false, true));
    }

    #[test]
    fn catalog_publication_syncs_before_startup_input_is_enabled() {
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                catalog_partial: false,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_for_media_systems(&["arcade", "amiga"]),
                load_us: 0,
                source: CatalogSource::FreshBuild,
                durable_save_pending: false,
                generation_fingerprint: None,
                publication_ack: None,
            },
        );
        let mut use_catalog_seen = false;
        let mut full_bridge_dirty = false;
        for effect in effects.into_effects() {
            match effect {
                CatalogSessionEffect::UseCatalog { .. } => use_catalog_seen = true,
                CatalogSessionEffect::SyncCatalogBridge => {
                    assert!(
                        use_catalog_seen,
                        "bridge sync must follow catalog installation"
                    );
                    full_bridge_dirty = true;
                }
                _ => {}
            }
        }
        assert!(use_catalog_seen);
        assert!(full_bridge_dirty);
        assert_eq!(
            launcher_bridge_sync_plan(false, full_bridge_dirty, false),
            LauncherBridgeSyncPlan::Full
        );
    }

    fn catalog_for_media_systems(system_ids: &[&str]) -> ArcadeCatalog {
        let mut games = Vec::new();
        let mut systems = Vec::new();
        for system_id in system_ids {
            games.push(
                arcade_game(format!("{system_id} game"))
                    .path(format!("/media/fat/_Arcade/{system_id}.mra"))
                    .preview(format!("{system_id}.raw565"))
                    .system_id(*system_id)
                    .build(),
            );
            systems.push(arcade_system(*system_id, 1));
        }
        arcade_catalog(games, systems)
    }

    #[test]
    fn startup_registry_fingerprint_enables_system_shard_requests() {
        let mut scheduler = LauncherScheduler::new();
        let generation =
            initialize_catalog_generation(&mut scheduler, Some("generation-a".to_string()));

        assert_eq!(generation.current.as_deref(), Some("generation-a"));
        assert_eq!(generation.durable.as_deref(), Some("generation-a"));
        assert!(scheduler.request_system_shard(
            "c64".to_string(),
            "startup-regression-test",
            empty_arcade_catalog("/tmp"),
            1,
            Instant::now()
        ));
    }

    #[test]
    fn shard_request_state_changes_only_after_scheduler_acceptance() {
        let mut nav = LauncherNav::new();
        let mut scheduler = LauncherScheduler::new();
        let catalog = empty_arcade_catalog("/tmp");

        assert!(!request_system_shard_hydration(
            &mut scheduler,
            &mut nav,
            &catalog,
            0,
            "c64",
            "rejected-without-generation",
            Instant::now()
        ));
        assert!(!nav.catalog_system_hydration_is_loading("c64"));

        nav.catalog_system_hydration_failed("c64");
        assert!(!retry_system_shard_hydration(
            &mut scheduler,
            &mut nav,
            &catalog,
            0,
            "c64",
            "rejected-retry-without-generation",
            Instant::now()
        ));
        assert!(nav.catalog_system_hydration_has_failed("c64"));

        let _ = initialize_catalog_generation(&mut scheduler, Some("generation-a".to_string()));
        assert!(retry_system_shard_hydration(
            &mut scheduler,
            &mut nav,
            &catalog,
            0,
            "c64",
            "accepted-retry",
            Instant::now()
        ));
        assert!(nav.catalog_system_hydration_is_loading("c64"));
    }

    #[test]
    fn pending_launch_return_deduplicates_a_second_registry_shard_request() {
        let full_catalog = catalog_for_media_systems(&["c64"]);
        let mut launched_nav = LauncherNav::new();
        assert!(launched_nav.open_system_game_list(&full_catalog, "c64"));
        let state = launcher::capture_launch_return_state(
            &launched_nav,
            &full_catalog,
            "/media/fat/_Arcade/c64.mra",
        )
        .expect("return state");
        let registry = arcade_catalog(Vec::new(), vec![arcade_system("c64", 1)]);
        let mut restored_nav = LauncherNav::new();
        restored_nav.sync_launcher_taxonomy(&registry);
        let mut scheduler = LauncherScheduler::new();
        let _ = initialize_catalog_generation(&mut scheduler, Some("generation-a".to_string()));
        let now = Instant::now();

        assert!(request_pending_launch_return_shard(
            Some(&state),
            &registry,
            0,
            &mut restored_nav,
            &mut scheduler,
            now,
            now,
        ));
        assert!(scheduler.system_shard_attempted("c64"));
        assert!(!scheduler.request_system_shard(
            "c64".to_string(),
            "duplicate-request",
            registry.clone(),
            0,
            now,
        ));
    }

    #[test]
    fn pending_launch_return_requests_its_shard_when_other_collection_rows_are_resident() {
        let full_catalog = arcade_catalog(
            vec![
                arcade_game("first")
                    .path("/media/fat/_Arcade/first.mra")
                    .system_id("arcade")
                    .build(),
                arcade_game("saved")
                    .path("/media/fat/_Arcade/saved.mra")
                    .system_id("arcade")
                    .build(),
            ],
            vec![arcade_system("arcade", 2)],
        );
        let mut launched_nav = LauncherNav::new();
        assert!(launched_nav.open_system(&full_catalog, "arcade"));
        let state = launcher::capture_launch_return_state(
            &launched_nav,
            &full_catalog,
            "/media/fat/_Arcade/saved.mra",
        )
        .expect("return state");
        let partial_catalog = arcade_catalog(
            vec![
                arcade_game("first")
                    .path("/media/fat/_Arcade/first.mra")
                    .system_id("arcade")
                    .build(),
            ],
            vec![arcade_system("arcade", 2)],
        );
        let mut restored_nav = LauncherNav::new();
        restored_nav.sync_launcher_taxonomy(&partial_catalog);
        let mut scheduler = LauncherScheduler::new();
        let _ = initialize_catalog_generation(&mut scheduler, Some("generation-a".to_string()));
        let now = Instant::now();

        assert!(request_pending_launch_return_shard(
            Some(&state),
            &partial_catalog,
            0,
            &mut restored_nav,
            &mut scheduler,
            now,
            now,
        ));
        assert!(scheduler.system_shard_attempted("arcade"));
    }

    #[test]
    fn return_session_reapplies_exact_context_until_authoritative_present() {
        let catalog = arcade_catalog(
            (0..3)
                .map(|index| {
                    arcade_game(format!("c64 game {index}"))
                        .path(format!("/media/fat/_Arcade/c64-{index}.mra"))
                        .preview(format!("c64-{index}.raw565"))
                        .system_id("c64")
                        .build()
                })
                .collect(),
            vec![arcade_system("c64", 3)],
        );
        let mut launched_nav = LauncherNav::new();
        assert!(launched_nav.open_system_game_list(&catalog, "c64"));
        launched_nav
            .arcade
            .restore_position(2, 2 * launched_nav.arcade.row_height(), 3);
        let state = launcher::capture_launch_return_state(
            &launched_nav,
            &catalog,
            "/media/fat/_Arcade/c64-2.mra",
        )
        .expect("return state");
        let mut session = LaunchReturnSession::new(Some(state));
        let mut restored_nav = LauncherNav::new();

        assert!(session.apply(&mut restored_nav, &catalog, CatalogSource::ReturnCapsule));
        assert!(session.context_matches(&restored_nav, &catalog));
        session.mark_preview_ready();
        session.mark_correct_present(&restored_nav, &catalog);
        assert!(
            session.requested(),
            "capsule present is not authoritative hydration"
        );

        restored_nav.go_root();
        assert!(!session.context_matches(&restored_nav, &catalog));
        assert!(session.apply(&mut restored_nav, &catalog, CatalogSource::FullSqlite));
        assert!(session.context_matches(&restored_nav, &catalog));
        assert_eq!(session.source, "return-capsule");
        session.mark_correct_present(&restored_nav, &catalog);
        assert!(
            session.requested(),
            "state is retained through catalog validation"
        );
        assert_eq!(session.phase, "complete");
        restored_nav.go_root();
        assert!(session.apply(&mut restored_nav, &catalog, CatalogSource::FullSqlite));
        assert!(session.context_matches(&restored_nav, &catalog));
        assert_eq!(session.phase, "complete");
        session.release_if_complete();
        assert!(!session.requested());
        assert_eq!(session.phase, "complete");
    }

    #[test]
    fn registry_replacement_preserves_return_list_while_requesting_its_shard() {
        let full_catalog = arcade_catalog(
            (0..3)
                .map(|index| {
                    arcade_game(format!("SNES game {index}"))
                        .path(format!("/media/fat/games/SNES/game-{index}.sfc"))
                        .system_id("snes")
                        .build()
                })
                .collect(),
            vec![arcade_system("snes", 3)],
        );
        let mut launched_nav = LauncherNav::new();
        assert!(launched_nav.open_system(&full_catalog, "snes"));
        assert_eq!(launched_nav.screen, Screen::Arcade);
        launched_nav.set_arcade_user_list_mode(&full_catalog, launcher::ArcadeUserListMode::Games);
        launched_nav.screen = Screen::Arcade;
        launched_nav.arcade.restore_position(
            2,
            2 * launched_nav.arcade.row_height(),
            full_catalog.system_game_count("snes"),
        );
        let state = launcher::capture_launch_return_state(
            &launched_nav,
            &full_catalog,
            "/media/fat/games/SNES/game-2.sfc",
        )
        .expect("return state");
        let mut session = LaunchReturnSession::new(Some(state));
        let mut restored_nav = LauncherNav::new();
        assert!(session.apply(
            &mut restored_nav,
            &full_catalog,
            CatalogSource::ReturnCapsule
        ));
        assert_eq!(restored_nav.screen, Screen::Arcade);
        assert_eq!(restored_nav.arcade.selected, 2);
        assert!(restored_nav.arcade.is_settled_at_selected());

        let registry = arcade_catalog(Vec::new(), vec![arcade_system("snes", 3)]);
        restored_nav.sync_launcher_taxonomy(&registry);
        let mut scheduler = LauncherScheduler::new();
        let _ = initialize_catalog_generation(&mut scheduler, Some("generation-a".to_string()));
        let now = Instant::now();

        assert!(!apply_or_request_pending_launch_return_state(
            &mut restored_nav,
            &registry,
            2,
            &mut session,
            &mut scheduler,
            CatalogSource::ShardedRegistry,
            now,
            now,
        ));
        assert_eq!(restored_nav.screen, Screen::Arcade);
        assert_eq!(restored_nav.active_collection_id(), Some("snes"));
        assert_eq!(restored_nav.arcade.selected, 2);
        assert!(restored_nav.arcade.is_settled_at_selected());
        assert!(scheduler.system_shard_attempted("snes"));
        assert!(restored_nav.catalog_system_hydration_is_loading("snes"));

        assert!(session.apply(
            &mut restored_nav,
            &full_catalog,
            CatalogSource::NavigationProjection,
        ));
        assert_eq!(restored_nav.screen, Screen::Arcade);
        assert_eq!(restored_nav.arcade.selected, 2);
        assert!(restored_nav.arcade.is_settled_at_selected());
    }

    #[test]
    fn three_consecutive_return_sessions_restore_their_settled_row() {
        let catalog = arcade_catalog(
            (0..3)
                .map(|index| {
                    arcade_game(format!("arcade game {index}"))
                        .path(format!("/media/fat/_Arcade/arcade-{index}.mra"))
                        .system_id("arcade")
                        .build()
                })
                .collect(),
            vec![arcade_system("arcade", 3)],
        );
        for index in 0..3 {
            let mut launched_nav = LauncherNav::new();
            assert!(launched_nav.open_system(&catalog, "arcade"));
            launched_nav.arcade.restore_position(
                index,
                index as i32 * launched_nav.arcade.row_height(),
                3,
            );
            let path = format!("/media/fat/_Arcade/arcade-{index}.mra");
            let state = launcher::capture_launch_return_state(&launched_nav, &catalog, &path)
                .expect("return state");
            let mut session = LaunchReturnSession::new(Some(state));
            let mut restored_nav = LauncherNav::new();

            assert!(session.apply(&mut restored_nav, &catalog, CatalogSource::FullSqlite));
            assert!(session.context_matches(&restored_nav, &catalog));
            assert_eq!(restored_nav.arcade.selected, index);
            assert_eq!(
                restored_nav.arcade.scroll_y,
                index as i32 * restored_nav.arcade.row_height()
            );
        }
    }

    #[test]
    fn return_session_timeout_explicitly_falls_back_to_root_home() {
        let catalog = catalog_for_media_systems(&["c64"]);
        let mut launched_nav = LauncherNav::new();
        assert!(launched_nav.open_system_game_list(&catalog, "c64"));
        let state = launcher::capture_launch_return_state(
            &launched_nav,
            &catalog,
            "/media/fat/_Arcade/c64.mra",
        )
        .expect("return state");
        let mut session = LaunchReturnSession::new(Some(state));

        session.note_capsule_failure("capsule checksum mismatch".to_string());
        session.fallback_to_home(&mut launched_nav);

        assert_eq!(launched_nav.screen, Screen::Home);
        assert_eq!(
            launched_nav.current_menu_id(),
            crate::launcher_taxonomy::ROOT_MENU_ID
        );
        assert_eq!(session.phase, "fallback-home");
        assert_eq!(session.fallback_reason, "capsule checksum mismatch");
        assert!(!session.requested());
    }

    #[test]
    fn return_preview_timeout_falls_back_even_when_exact_context_was_restored() {
        let catalog = catalog_for_media_systems(&["c64"]);
        let mut nav = LauncherNav::new();
        assert!(nav.open_system_game_list(&catalog, "c64"));
        let state =
            launcher::capture_launch_return_state(&nav, &catalog, "/media/fat/_Arcade/c64.mra")
                .expect("return state");
        let mut session = LaunchReturnSession::new(Some(state));
        assert!(session.apply(&mut nav, &catalog, CatalogSource::ReturnCapsule));
        assert!(session.context_matches(&nav, &catalog));
        let mut effects = LifecycleEffects::new();
        effects.startup_event("return_black_screen_timeout", "preview never ready");

        assert!(return_black_timeout_requires_home_fallback(true, &effects));
        session.fallback_to_home(&mut nav);

        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(session.phase, "fallback-home");
        assert!(!session.requested());
    }

    #[test]
    fn rejected_capsule_restores_from_the_urgent_system_shard() {
        let full_catalog = catalog_for_media_systems(&["c64"]);
        let mut launched_nav = LauncherNav::new();
        assert!(launched_nav.open_system_game_list(&full_catalog, "c64"));
        let state = launcher::capture_launch_return_state(
            &launched_nav,
            &full_catalog,
            "/media/fat/_Arcade/c64.mra",
        )
        .expect("return state");
        let mut session = LaunchReturnSession::new(Some(state));
        session.note_capsule_failure("capsule generation mismatch".to_string());
        let registry = arcade_catalog(Vec::new(), vec![arcade_system("c64", 1)]);
        let mut restored_nav = LauncherNav::new();

        assert!(!session.reapply(&mut restored_nav, &registry));
        assert!(session.reapply(&mut restored_nav, &full_catalog));
        session.mark_system_shard_authoritative();
        assert!(session.context_matches(&restored_nav, &full_catalog));
        assert_eq!(session.source, "system-shard");
    }

    #[test]
    fn rejected_capsule_restores_immediately_from_validated_registry_rows() {
        let catalog = catalog_for_media_systems(&["c64"]);
        let mut launched_nav = LauncherNav::new();
        assert!(launched_nav.open_system_game_list(&catalog, "c64"));
        let state = launcher::capture_launch_return_state(
            &launched_nav,
            &catalog,
            "/media/fat/_Arcade/c64.mra",
        )
        .expect("return state");
        let mut session = LaunchReturnSession::new(Some(state));
        session.note_capsule_failure("capsule missing".to_string());
        let mut restored_nav = LauncherNav::new();

        assert!(session.apply(&mut restored_nav, &catalog, CatalogSource::ShardedRegistry));
        assert!(session.context_matches(&restored_nav, &catalog));
        assert_eq!(session.source, "sharded-registry");
        assert_eq!(session.phase, "authoritative-context-restored");

        assert!(session.apply(&mut restored_nav, &catalog, CatalogSource::FreshBuild));
        assert_eq!(
            session.source, "sharded-registry",
            "later catalogue publications must not rewrite the restoration origin"
        );
    }

    #[test]
    fn pending_return_shard_protects_exact_arcade_context_from_empty_list_recovery() {
        let capsule = catalog_for_media_systems(&["arcade"]);
        let mut launched_nav = LauncherNav::new();
        assert!(launched_nav.open_default_arcade(&capsule));
        let state = launcher::capture_launch_return_state(
            &launched_nav,
            &capsule,
            "/media/fat/_Arcade/arcade.mra",
        )
        .expect("return state");
        assert_eq!(state.collection_id(), Some("menu:arcade"));
        assert_eq!(state.system_id(), "arcade");
        let session = LaunchReturnSession::new(Some(state));
        let mut restored_nav = LauncherNav::new();
        assert!(launcher::apply_launch_return_state(
            &mut restored_nav,
            &capsule,
            session.state().expect("pending return").clone(),
        ));
        restored_nav.catalog_system_hydration_started("arcade");
        let registry = summary_catalog_for_media_systems(&["arcade"]);

        assert!(!empty_collection_invariant_violated(
            &registry,
            &restored_nav,
        ));
        assert!(session.protects_hydrating_collection(&restored_nav));
        assert!(should_poll_system_entry_handoff(
            false,
            false,
            session.protects_hydrating_collection(&restored_nav),
            true,
        ));

        restored_nav.catalog_system_hydration_failed("arcade");
        assert!(!session.protects_hydrating_collection(&restored_nav));
        assert!(!should_poll_system_entry_handoff(
            false,
            false,
            session.protects_hydrating_collection(&restored_nav),
            true,
        ));
    }

    #[test]
    fn authoritative_registry_reconciles_discovery_shells_before_taxonomy_sync() {
        let mut nav = LauncherNav::new();
        nav.catalog_system_discovered("snes");
        nav.catalog_system_discovered("3do");
        let authoritative = catalog_for_media_systems(&["snes"]);

        let catalog =
            catalog_for_ready_source(&mut nav, authoritative, CatalogSource::ShardedRegistry);
        nav.sync_launcher_taxonomy(&catalog);

        assert!(catalog.systems.iter().any(|system| system.id == "snes"));
        assert!(catalog.systems.iter().all(|system| system.id != "3do"));
        assert!(nav.open_menu(crate::launcher_taxonomy::CONSOLES_MENU_ID));
        assert!(
            nav.current_menu_items()
                .iter()
                .any(|item| item.id == "snes")
        );
        assert!(nav.current_menu_items().iter().all(|item| item.id != "3do"));
    }

    #[test]
    fn progressive_catalog_retains_discovery_shells_until_registry_publish() {
        let mut nav = LauncherNav::new();
        nav.catalog_system_discovered("snes");
        let bootstrap = catalog_for_media_systems(&["arcade"]);

        let catalog =
            catalog_for_ready_source(&mut nav, bootstrap, CatalogSource::NavigationProjection);

        assert!(catalog.systems.iter().any(|system| system.id == "snes"));
    }

    #[test]
    fn intro_deferred_scanning_replays_one_system_shell_after_handoff() {
        let mut nav = LauncherNav::new();
        let mut catalog = catalog_for_media_systems(&["arcade"]);

        assert!(!apply_catalog_system_scanning_presentation(
            &mut nav,
            &mut catalog,
            "snes",
            true,
        ));
        assert!(catalog.systems.iter().all(|system| system.id != "snes"));

        catalog = nav.catalog_with_build_shells(catalog);
        nav.sync_launcher_taxonomy(&catalog);

        assert!(catalog.systems.iter().any(|system| system.id == "snes"));
        assert!(nav.open_menu(crate::launcher_taxonomy::CONSOLES_MENU_ID));
        assert!(
            nav.current_menu_items()
                .iter()
                .any(|item| item.id == "snes")
        );
    }

    #[test]
    fn intro_catalog_ui_replay_retains_the_latest_presentation() {
        let mut replay = None;

        retain_startup_intro_catalog_ui_intent(
            &mut replay,
            LauncherWorkerUiIntent::ShowCatalogBackgroundScan,
        );
        retain_startup_intro_catalog_ui_intent(
            &mut replay,
            LauncherWorkerUiIntent::ClearCatalogScan,
        );

        assert!(matches!(
            replay,
            Some(LauncherWorkerUiIntent::ClearCatalogScan)
        ));
    }

    fn summary_catalog_for_media_systems(system_ids: &[&str]) -> ArcadeCatalog {
        let systems = system_ids
            .iter()
            .map(|system_id| arcade_system(*system_id, 1))
            .collect();
        arcade_catalog(Vec::new(), systems)
    }

    #[test]
    fn cold_collection_sequence_keeps_home_until_populated_commit() {
        let empty = ArcadeCatalog::new(
            std::path::PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![crate::test_support::arcade_system("c64", 1)],
        );
        let hydrated = crate::test_support::arcade_catalog(
            vec![
                crate::test_support::arcade_game("C64 Game")
                    .system_id("c64")
                    .build(),
            ],
            vec![crate::test_support::arcade_system("c64", 1)],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&empty);
        let mut pending = Some(PendingCollectionEntry {
            collection_id: "c64".to_string(),
            requested_at: Instant::now(),
            source: nav.home_view_state(),
            open_game_list_directly: false,
        });
        let source_bridge = LauncherProjectionKey::from_nav(&nav);

        assert!(!commit_pending_collection_entry(
            &mut pending,
            &mut nav,
            &empty,
            Instant::now()
        ));
        assert_eq!(nav.screen, Screen::Home);
        assert!(pending.is_some());
        assert_eq!(LauncherProjectionKey::from_nav(&nav).screen, Screen::Home);
        assert_eq!(
            LauncherProjectionKey::from_nav(&nav).menu_id,
            source_bridge.menu_id
        );

        assert!(commit_pending_collection_entry(
            &mut pending,
            &mut nav,
            &hydrated,
            Instant::now()
        ));
        // Every console, computer and handheld opens its own page first.
        assert_eq!(nav.screen, Screen::Arcade);
        assert!(pending.is_none());
        assert_eq!(active_system_game_view(&hydrated, &nav).len(), 1);
        assert!(!empty_collection_invariant_violated(&hydrated, &nav));
        assert_eq!(LauncherProjectionKey::from_nav(&nav).screen, Screen::Arcade);
    }

    #[test]
    fn cold_snes_commit_opens_the_hub_except_for_direct_benchmarks() {
        let registry = ArcadeCatalog::new(
            std::path::PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![crate::test_support::arcade_system("snes", 1)],
        );
        let hydrated = crate::test_support::arcade_catalog(
            vec![
                crate::test_support::arcade_game("F-Zero")
                    .system_id("snes")
                    .build(),
            ],
            vec![crate::test_support::arcade_system("snes", 1)],
        );

        for (open_game_list_directly, expected_mode) in [
            (false, launcher::SystemPageMode::Hub),
            (true, launcher::SystemPageMode::List),
        ] {
            let mut nav = LauncherNav::new();
            nav.sync_launcher_taxonomy(&registry);
            let mut pending = Some(PendingCollectionEntry {
                collection_id: "snes".to_string(),
                requested_at: Instant::now(),
                source: nav.home_view_state(),
                open_game_list_directly,
            });

            assert!(commit_pending_collection_entry(
                &mut pending,
                &mut nav,
                &hydrated,
                Instant::now(),
            ));
            assert_eq!(nav.screen, Screen::Arcade);
            assert_eq!(nav.system_page_mode, expected_mode);
        }
    }

    #[test]
    fn failed_pending_collection_restores_home_without_clearing_load_failure() {
        let catalog = ArcadeCatalog::new(
            std::path::PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![crate::test_support::arcade_system("c64", 1)],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        let source = nav.home_view_state();
        let mut pending = Some(PendingCollectionEntry {
            collection_id: "c64".to_string(),
            requested_at: Instant::now(),
            source: source.clone(),
            open_game_list_directly: false,
        });
        nav.catalog_system_hydration_failed("c64");

        assert!(restore_failed_pending_collection_entry(
            &mut pending,
            &mut nav,
            Instant::now(),
        ));
        assert!(pending.is_none());
        assert_eq!(nav.home_view_state(), source);
        assert!(nav.catalog_system_hydration_has_failed("c64"));
    }

    #[test]
    fn back_at_home_root_cancels_pending_entry_even_without_navigation_change() {
        let catalog = ArcadeCatalog::new(
            std::path::PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![crate::test_support::arcade_system("c64", 1)],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        nav.catalog_system_hydration_started("c64");
        let mut pending = Some(PendingCollectionEntry {
            collection_id: "c64".to_string(),
            requested_at: Instant::now(),
            source: nav.home_view_state(),
            open_game_list_directly: false,
        });
        let event = normalized_test_press(crate::input_event::LogicalAction::Back);

        assert!(cancel_pending_collection_entry_for_input(
            &mut pending,
            &mut nav,
            Some(&event),
            Instant::now()
        ));
        assert!(pending.is_none());
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(
            nav.current_menu_id(),
            crate::launcher_taxonomy::ROOT_MENU_ID
        );
    }

    #[test]
    fn populated_collection_with_no_resident_rows_violates_presentation_invariant() {
        let catalog = ArcadeCatalog::new(
            std::path::PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![crate::test_support::arcade_system("c64", 18_851)],
        );
        let mut nav = LauncherNav::new();
        assert!(nav.open_system_game_list(&catalog, "c64"));

        assert!(empty_collection_invariant_violated(&catalog, &nav));
        nav.recover_empty_collection_to_home();
        assert!(!empty_collection_invariant_violated(&catalog, &nav));
    }

    fn ready_catalog_message() -> CatalogWorkerMessage {
        CatalogWorkerMessage::Ready {
            catalog: catalog_for_media_systems(&["arcade"]),
            load_us: 42,
            source: CatalogSource::FullSqlite,
            durable_save_pending: false,
            generation_fingerprint: None,
            publication_ack: None,
        }
    }

    #[test]
    pub(super) fn catalog_ready_swap_defers_while_arcade_scroll_is_active() {
        let now = Instant::now();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade.handle_direction_input(1, 0, now, 2);

        assert!(should_defer_catalog_message(
            &ready_catalog_message(),
            true,
            &nav,
            None,
            now
        ));
    }

    #[test]
    pub(super) fn catalog_ready_swap_does_not_defer_first_usable_catalog() {
        let now = Instant::now();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade.handle_direction_input(1, 0, now, 2);

        assert!(!should_defer_catalog_message(
            &ready_catalog_message(),
            false,
            &nav,
            None,
            now
        ));
    }

    #[test]
    pub(super) fn deferred_search_catalog_publishes_during_arcade_motion() {
        let now = Instant::now();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade.handle_direction_input(1, 0, now, 2);
        let source = catalog_for_media_systems(&["arcade"]);
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            source.root.clone(),
            source.games.as_ref().clone(),
            source.systems.clone(),
            Vec::new(),
        );
        assert!(!catalog.text_indexes_ready());
        let message = CatalogWorkerMessage::Ready {
            catalog,
            load_us: 42,
            source: CatalogSource::NavigationProjection,
            durable_save_pending: false,
            generation_fingerprint: None,
            publication_ack: None,
        };

        assert!(!should_defer_catalog_message(
            &message, true, &nav, None, now
        ));
    }

    #[test]
    pub(super) fn catalog_ready_swap_briefly_defers_while_direction_is_held_at_edge() {
        let now = Instant::now();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade.handle_direction_input(1, 0, now, 1);
        let edge_since = update_catalog_ready_stationary_edge_since(&nav, None, now);

        assert!(should_defer_catalog_message(
            &ready_catalog_message(),
            true,
            &nav,
            edge_since,
            now + CATALOG_READY_STATIONARY_EDGE_SETTLE / 2
        ));
    }

    #[test]
    pub(super) fn catalog_ready_swap_applies_after_stationary_edge_settles() {
        let now = Instant::now();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade.handle_direction_input(1, 0, now, 1);
        let edge_since = update_catalog_ready_stationary_edge_since(&nav, None, now);

        assert!(!should_defer_catalog_message(
            &ready_catalog_message(),
            true,
            &nav,
            edge_since,
            now + CATALOG_READY_STATIONARY_EDGE_SETTLE
        ));
    }

    #[test]
    pub(super) fn catalog_terminal_messages_are_not_defer_candidates() {
        let now = Instant::now();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade.handle_direction_input(1, 0, now, 2);

        let message = CatalogWorkerMessage::Done;

        assert!(!should_defer_catalog_message(
            &message, true, &nav, None, now
        ));
    }

    #[test]
    pub(super) fn recovery_worker_is_polled_after_startup_refresh_finished() {
        assert!(catalog_messages_need_polling(false, true, true));
        assert!(!catalog_messages_need_polling(false, true, false));
    }

    #[test]
    pub(super) fn launch_return_restore_requires_volatile_main_flag() {
        assert!(!return_to_launcher_env_is_set(None));
        assert!(!return_to_launcher_env_is_set(Some("0")));
        assert!(!return_to_launcher_env_is_set(Some("false")));
        assert!(return_to_launcher_env_is_set(Some("1")));
        assert!(return_to_launcher_env_is_set(Some("true")));
        assert!(return_to_launcher_env_is_set(Some("yes")));
    }

    #[test]
    pub(super) fn layout_epoch_advances_for_each_directed_orientation_change() {
        let ui = UiDisplay::for_framebuffer(1280, 720);
        let mut layout = UiLayoutGeometry::for_display(&ui, ScreenOrientation::Normal);
        let mut epoch = 1;

        for (expected_epoch, orientation) in [
            (2, ScreenOrientation::MonitorClockwise),
            (3, ScreenOrientation::MonitorCounterclockwise),
            (4, ScreenOrientation::Normal),
            (5, ScreenOrientation::MonitorCounterclockwise),
            (6, ScreenOrientation::MonitorClockwise),
            (7, ScreenOrientation::Normal),
        ] {
            assert!(replace_layout(
                &mut layout,
                &mut epoch,
                UiLayoutGeometry::for_display(&ui, orientation),
            ));
            assert_eq!(epoch, expected_epoch);
            assert_eq!(layout.orientation(), orientation);
        }

        assert!(!replace_layout(
            &mut layout,
            &mut epoch,
            UiLayoutGeometry::for_display(&ui, ScreenOrientation::Normal),
        ));
        assert_eq!(epoch, 7);
    }

    #[test]
    pub(super) fn arcade_overlay_draws_for_closed_arcade_list() {
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;

        assert!(should_draw_arcade_overlay(&nav, false, true));
    }

    #[test]
    pub(super) fn arcade_overlay_draws_filter_list_while_filter_view_is_open() {
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade_filter.drawer_open = true;

        assert!(should_draw_arcade_overlay(&nav, false, true));
    }

    #[test]
    pub(super) fn arcade_overlay_stays_hidden_while_unavailable_or_launching() {
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;

        assert!(!should_draw_arcade_overlay(&nav, true, false));
        assert!(!should_draw_arcade_overlay(&nav, false, false));
        assert!(should_draw_arcade_overlay(&nav, false, true));
    }

    #[test]
    pub(super) fn launcher_present_backend_defaults_to_fpga_latch() {
        use mister_magik_fb::process_config::PresentBackendConfig;
        assert_eq!(
            LauncherPresentBackend::from_config(&PresentBackendConfig::FpgaVblankLatchHidden),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
        assert_eq!(
            LauncherPresentBackend::from_config(&PresentBackendConfig::Fb0Dirty),
            LauncherPresentBackend::Fb0Dirty
        );
    }

    #[test]
    pub(super) fn launcher_present_backend_retired_values_use_required_latch_backend() {
        use mister_magik_fb::process_config::PresentBackendConfig;
        assert_eq!(
            LauncherPresentBackend::from_config(&PresentBackendConfig::Retired(
                ["main", "flip-v1"].join("-")
            )),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
        assert_eq!(
            LauncherPresentBackend::from_config(&PresentBackendConfig::Retired(
                ["main", "vsync-hidden"].join("-")
            )),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
        assert_eq!(
            LauncherPresentBackend::from_config(&PresentBackendConfig::Retired(
                ["plugin", "main", "vsync-hidden"].join("-")
            )),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
        assert_eq!(
            LauncherPresentBackend::from_config(&PresentBackendConfig::FpgaVblankLatchHidden),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
    }

    #[test]
    pub(super) fn present_mode_label_reports_only_proven_latch_as_latch() {
        assert_eq!(
            present_mode_label_for_backend_status(
                LauncherPresentBackend::FpgaVblankLatchHidden,
                LauncherPresentStatus::Ok,
            ),
            "Mode=latch"
        );
        assert_eq!(
            present_mode_label_for_backend_status(
                LauncherPresentBackend::FpgaVblankLatchHidden,
                LauncherPresentStatus::Frozen,
            ),
            "Mode=output frozen"
        );
        assert_eq!(
            present_mode_label_for_backend_status(
                LauncherPresentBackend::Fb0Dirty,
                LauncherPresentStatus::None,
            ),
            "Mode=/dev/fb0 diagnostic"
        );
        assert_eq!(
            present_mode_label_for_backend_status(
                LauncherPresentBackend::None,
                LauncherPresentStatus::None,
            ),
            "Mode=/dev/fb0 diagnostic"
        );
    }

    #[test]
    pub(super) fn arcade_drawer_view_cache_reuses_rows_until_identity_changes() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("Alpha")
                    .path("/media/fat/_Arcade/alpha.mra")
                    .year(1986)
                    .manufacturer("Capcom")
                    .control("Shooter")
                    .build(),
                arcade_game("Beta")
                    .path("/media/fat/_Arcade/beta.mra")
                    .year(1991)
                    .manufacturer("Namco")
                    .control("Maze")
                    .build(),
            ],
            vec![arcade_system("arcade", 2)],
        );
        let mut nav = LauncherNav::new();
        assert!(nav.open_default_arcade(&catalog));
        nav.arcade_filter.drawer_open = true;
        let mut cache = ArcadeDrawerViewCache::default();

        let top_items = cache.items(&catalog, &nav, 7).to_vec();
        assert_eq!(cache.rebuilds, 1);
        assert_eq!(cache.items(&catalog, &nav, 7), top_items.as_slice());
        assert_eq!(cache.rebuilds, 1);

        nav.arcade_filter.level = launcher::ArcadeFilterLevel::Manufacturers;
        let manufacturer_items = cache.items(&catalog, &nav, 7).to_vec();
        assert_eq!(cache.rebuilds, 2);
        assert_eq!(
            manufacturer_items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Capcom", "Namco"]
        );
        assert_eq!(
            cache.items(&catalog, &nav, 7),
            manufacturer_items.as_slice()
        );
        assert_eq!(cache.rebuilds, 2);

        nav.arcade_filter.active = arcade_catalog::ArcadeFilter::Manufacturer("Capcom".into());
        let first_item_active = cache.items(&catalog, &nav, 7)[0].active;
        assert_eq!(cache.rebuilds, 3);
        assert!(first_item_active);
    }

    #[test]
    pub(super) fn home_boot_with_ready_catalog_hides_catalog_popup() {
        assert!(!initial_catalog_scan_visible(true, true, false, false));
        assert!(initial_catalog_scan_visible(true, true, true, false));
    }

    #[test]
    fn display_transactions_rearm_vsync_after_every_stable_boundary() {
        let source = include_str!("launcher_loop/frame_loop.rs");
        let call = ["pacer", ".rearm_after_display_mode_change()"].concat();
        assert_eq!(source.matches(&call).count(), 3);
    }

    #[test]
    fn navigation_motion_suppresses_full_stream_refinement() {
        let source = include_str!("launcher_loop.rs");
        assert!(
            source.contains("let stream_motion_before_render = navigation_transition.is_active()")
        );
    }

    #[test]
    pub(super) fn missing_catalog_shows_catalog_popup_on_home_or_arcade_boot() {
        assert!(initial_catalog_scan_visible(false, true, false, false));
        assert!(!initial_catalog_scan_visible(true, true, false, false));
        assert!(!initial_catalog_scan_visible(false, false, false, false));
        assert!(!initial_catalog_scan_visible(false, true, false, true));
    }

    #[test]
    pub(super) fn launcher_idle_wait_requires_first_visible_copy_and_no_redraw() {
        let mut intent = LauncherRenderIntent {
            first_visible_copy_done: true,
            startup_input_enabled: true,
            wake_reasons: LauncherWakeReasons::default(),
        };

        assert!(intent.can_sleep());
        intent.first_visible_copy_done = false;
        assert!(!intent.can_sleep());
        intent.first_visible_copy_done = true;
        intent
            .wake_reasons
            .insert_if(LauncherWakeReasons::REDRAW_PENDING, true);
        assert!(!intent.can_sleep());
        intent.wake_reasons = LauncherWakeReasons::default();
        intent.startup_input_enabled = false;
        assert!(!intent.can_sleep());
    }

    #[test]
    pub(super) fn launcher_sleep_is_capped_to_one_physical_frame() {
        assert_eq!(
            launcher_max_sleep_duration(16_667),
            Duration::from_micros(16_667)
        );
        assert_eq!(
            launcher_max_sleep_duration(20_000),
            Duration::from_micros(20_000)
        );
    }

    #[test]
    pub(super) fn catalog_work_pauses_for_interaction_and_waits_one_second_to_burst() {
        let started = Instant::now();
        let mut idle_since = None;
        assert_eq!(CATALOG_IDLE_BURST_SETTLE, Duration::from_millis(1_000));
        assert_eq!(
            launcher_catalog_work_mode(false, false, true, started, &mut idle_since),
            CatalogWorkMode::DualCoreBurst
        );
        assert_eq!(
            launcher_catalog_work_mode(true, true, false, started, &mut idle_since),
            CatalogWorkMode::Paused
        );
        assert_eq!(
            launcher_catalog_work_mode(true, false, false, started, &mut idle_since),
            CatalogWorkMode::Cpu0
        );
        assert_eq!(
            launcher_catalog_work_mode(
                true,
                false,
                false,
                started + CATALOG_IDLE_BURST_SETTLE - Duration::from_millis(1),
                &mut idle_since,
            ),
            CatalogWorkMode::Cpu0
        );
        assert_eq!(
            launcher_catalog_work_mode(
                true,
                false,
                false,
                started + CATALOG_IDLE_BURST_SETTLE,
                &mut idle_since,
            ),
            CatalogWorkMode::DualCoreBurst
        );
        assert_eq!(
            launcher_catalog_work_mode(
                true,
                false,
                true,
                started + CATALOG_IDLE_BURST_SETTLE,
                &mut idle_since,
            ),
            CatalogWorkMode::Cpu0
        );
        assert!(idle_since.is_none());
    }

    #[test]
    pub(super) fn catalog_work_telemetry_accounts_each_mode_without_overlap() {
        let started = Instant::now();
        let mut telemetry = CatalogWorkModeTelemetry::new(started);
        assert!(telemetry.observe(CatalogWorkMode::Paused, started + Duration::from_millis(2)));
        assert!(telemetry.observe(
            CatalogWorkMode::DualCoreBurst,
            started + Duration::from_millis(5)
        ));
        telemetry.account(started + Duration::from_millis(11));

        assert_eq!(telemetry.cpu0_us, 2_000);
        assert_eq!(telemetry.paused_us, 3_000);
        assert_eq!(telemetry.burst_us, 6_000);
        assert_eq!(telemetry.transitions, 2);
    }

    #[test]
    pub(super) fn launcher_wake_reasons_combine_without_allocations() {
        let mut reasons = LauncherWakeReasons::default();
        assert!(reasons.is_empty());

        reasons.insert_if(LauncherWakeReasons::LAUNCHING, true);
        reasons.insert_if(LauncherWakeReasons::PREVIEW_DIRTY, true);
        reasons.insert_if(LauncherWakeReasons::MEDIA_MESSAGE_SEEN, false);

        assert_eq!(
            reasons,
            LauncherWakeReasons::LAUNCHING | LauncherWakeReasons::PREVIEW_DIRTY
        );
        assert!(!reasons.is_empty());
    }

    #[test]
    pub(super) fn stable_static_views_have_no_preview_intent_or_wake_reason() {
        for screen in [
            Screen::Home,
            Screen::Controller,
            Screen::Settings,
            Screen::About,
            Screen::Licenses,
            Screen::LicenseText,
        ] {
            let mut preview = PreviewState::new();
            preview.set_route(PreviewRoute::Unavailable);
            let mut reasons = LauncherWakeReasons::default();
            reasons.insert_if(
                LauncherWakeReasons::PREVIEW_DIRTY,
                preview.frame_intent().is_actionable(),
            );

            assert_eq!(
                preview.frame_intent(),
                PreviewFrameIntent::None,
                "{screen:?}"
            );
            assert!(reasons.is_empty(), "{screen:?}");
            assert!(
                LauncherRenderIntent {
                    first_visible_copy_done: true,
                    startup_input_enabled: true,
                    wake_reasons: reasons,
                }
                .can_sleep(),
                "{screen:?}"
            );
        }
    }

    #[test]
    pub(super) fn active_screensaver_starts_only_without_an_existing_pipeline() {
        assert!(screensaver_pipeline_start_allowed(true, false));
        assert!(!screensaver_pipeline_start_allowed(true, true));
        assert!(!screensaver_pipeline_start_allowed(false, false));
    }

    #[test]
    pub(super) fn launcher_domain_wake_reasons_match_current_behavior() {
        let home = LauncherWakeReasons::HOME_PAN_PRESENT_ACTIVE
            | LauncherWakeReasons::HOME_HORIZONTAL_INPUT_HELD;
        let arcade = LauncherWakeReasons::ARCADE_VISUAL_CHANGED_THIS_LOOP
            | LauncherWakeReasons::ARCADE_SCROLL_ACTIVE
            | LauncherWakeReasons::ARCADE_FILTER_SCROLL_ACTIVE;
        let search_preview = LauncherWakeReasons::ARCADE_SEARCH_ACTIVE
            | LauncherWakeReasons::PREVIEW_DIRTY
            | LauncherWakeReasons::PREVIEW_SCHEDULED_THIS_LOOP;
        let composition = LauncherWakeReasons::COMPOSITION_FORCES_FULL_PRESENT
            | LauncherWakeReasons::COMPOSITION_CLEARS_DIRECT_LAYERS;

        for reasons in [home, arcade, search_preview, composition] {
            assert!(
                !LauncherRenderIntent {
                    first_visible_copy_done: true,
                    startup_input_enabled: true,
                    wake_reasons: reasons,
                }
                .can_sleep()
            );
        }
    }

    #[test]
    pub(super) fn home_frame_driven_redraw_tracks_home_motion_only() {
        assert!(home_frame_driven_redraw_active(Screen::Home, true, false));
        assert!(home_frame_driven_redraw_active(Screen::Home, false, true));
        assert!(home_frame_driven_redraw_active(Screen::Home, true, true));
        assert!(!home_frame_driven_redraw_active(Screen::Home, false, false));
        assert!(!home_frame_driven_redraw_active(Screen::Arcade, true, true));
        assert!(!home_frame_driven_redraw_active(
            Screen::Settings,
            true,
            true
        ));
    }

    #[test]
    pub(super) fn frame_production_class_distinguishes_prepared_and_synchronous_frames() {
        assert_eq!(
            frame_production_class(false, false, false),
            FrameProductionClass::EventDriven
        );
        assert_eq!(
            frame_production_class(false, true, false),
            FrameProductionClass::SynchronousAnimation
        );
        assert_eq!(
            frame_production_class(false, false, true),
            FrameProductionClass::SynchronousAnimation
        );
        assert_eq!(
            frame_production_class(true, true, true),
            FrameProductionClass::Prepared
        );
    }

    #[test]
    pub(super) fn home_horizontal_held_matches_left_or_right_only() {
        assert!(!pad_state_home_horizontal_held(&PadState::default()));
        assert!(pad_state_home_horizontal_held(&pad_state_with(|state| {
            state.dpad_left = true;
        })));
        assert!(pad_state_home_horizontal_held(&pad_state_with(|state| {
            state.dpad_right = true;
        })));
        assert!(!pad_state_home_horizontal_held(&pad_state_with(|state| {
            state.dpad_up = true;
        })));
    }

    #[test]
    pub(super) fn latch_late_start_wait_is_disabled_for_interactive_frames_and_latch_animation() {
        assert!(latch_late_start_wait_enabled(
            false,
            FrameProductionClass::EventDriven,
            false,
        ));
        assert!(latch_late_start_wait_enabled(
            false,
            FrameProductionClass::SynchronousAnimation,
            false,
        ));
        assert!(latch_late_start_wait_enabled(
            true,
            FrameProductionClass::EventDriven,
            false,
        ));
        assert!(latch_late_start_wait_enabled(
            true,
            FrameProductionClass::Prepared,
            false,
        ));
        assert!(!latch_late_start_wait_enabled(
            true,
            FrameProductionClass::SynchronousAnimation,
            false,
        ));
        assert!(!latch_late_start_wait_enabled(
            false,
            FrameProductionClass::EventDriven,
            true,
        ));
        assert!(!latch_late_start_wait_enabled(
            true,
            FrameProductionClass::Prepared,
            true,
        ));
    }

    #[test]
    pub(super) fn home_pan_present_window_follows_scroll_changes() {
        let now = Instant::now();
        let mut last_scroll_x = 0;
        let mut present_until = None;

        assert!(!update_home_pan_present_window(
            Screen::Home,
            0,
            &mut last_scroll_x,
            &mut present_until,
            now,
        ));
        assert!(update_home_pan_present_window(
            Screen::Home,
            220,
            &mut last_scroll_x,
            &mut present_until,
            now,
        ));
        assert!(update_home_pan_present_window(
            Screen::Home,
            220,
            &mut last_scroll_x,
            &mut present_until,
            now + HOME_PAN_PRESENT_DURATION - Duration::from_millis(1),
        ));
        assert!(!update_home_pan_present_window(
            Screen::Home,
            220,
            &mut last_scroll_x,
            &mut present_until,
            now + HOME_PAN_PRESENT_DURATION + Duration::from_millis(1),
        ));
        assert!(present_until.is_none());
    }

    #[test]
    pub(super) fn home_pan_present_window_clears_off_home() {
        let now = Instant::now();
        let mut last_scroll_x = 0;
        let mut present_until = None;

        assert!(update_home_pan_present_window(
            Screen::Home,
            220,
            &mut last_scroll_x,
            &mut present_until,
            now,
        ));
        assert!(!update_home_pan_present_window(
            Screen::Arcade,
            220,
            &mut last_scroll_x,
            &mut present_until,
            now,
        ));
        assert!(present_until.is_none());
    }

    #[test]
    fn catalog_scan_blink_only_toggles_while_building() {
        let now = Instant::now();
        let mut blink = CatalogScanBlink::default();

        assert_eq!(blink.update(false, now), None);
        assert_eq!(blink.time_until_toggle(now), None);

        assert_eq!(blink.update(true, now), None);
        assert_eq!(
            blink.time_until_toggle(now),
            Some(CATALOG_SCAN_BLINK_HALF_PERIOD)
        );
        assert_eq!(
            blink.update(
                true,
                now + CATALOG_SCAN_BLINK_HALF_PERIOD - Duration::from_millis(1)
            ),
            None
        );
        assert_eq!(
            blink.update(true, now + CATALOG_SCAN_BLINK_HALF_PERIOD),
            Some(false)
        );
        assert_eq!(
            blink.update(true, now + CATALOG_SCAN_BLINK_HALF_PERIOD * 2),
            Some(true)
        );
    }

    #[test]
    fn catalog_scan_blink_disarms_and_resets_visible() {
        let now = Instant::now();
        let mut blink = CatalogScanBlink::default();

        assert_eq!(blink.update(true, now), None);
        assert_eq!(
            blink.update(true, now + CATALOG_SCAN_BLINK_HALF_PERIOD),
            Some(false)
        );
        assert_eq!(
            blink.update(false, now + CATALOG_SCAN_BLINK_HALF_PERIOD),
            Some(true)
        );
        assert_eq!(blink.time_until_toggle(now), None);
        assert_eq!(blink.update(false, now), None);

        assert_eq!(blink.update(true, now), None);
        assert_eq!(
            blink.time_until_toggle(now),
            Some(CATALOG_SCAN_BLINK_HALF_PERIOD)
        );
    }

    #[test]
    pub(super) fn home_pan_present_rect_matches_home_list_band() {
        let ui = UiDisplay::for_framebuffer(960, 540);
        assert_eq!(
            home_pan_present_rect(&ui),
            DirtyRect {
                x0: 18,
                y0: 74,
                x1: 942,
                y1: 478,
            }
        );
    }

    #[test]
    pub(super) fn home_pan_present_expands_dirty_rect_to_rail_band_only() {
        let ui = UiDisplay::for_framebuffer(960, 540);
        let dirty = DirtyRect {
            x0: 100,
            y0: 120,
            x1: 200,
            y1: 220,
        };

        assert_eq!(
            expand_home_pan_dirty_rect(Some(dirty), &ui, false),
            Some(dirty)
        );
        assert_eq!(
            expand_home_pan_dirty_rect(Some(dirty), &ui, true),
            Some(DirtyRect {
                x0: 18,
                y0: 74,
                x1: 942,
                y1: 478,
            })
        );
        assert_eq!(
            expand_home_pan_dirty_rect(None, &ui, true),
            Some(DirtyRect {
                x0: 18,
                y0: 74,
                x1: 942,
                y1: 478,
            })
        );
    }

    #[test]
    pub(super) fn ready_catalog_loads_without_refresh_unless_explicitly_forced() {
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Default),
            CatalogWorkerRequest::LoadOnly
        );
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Force),
            CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS
        );
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Off),
            CatalogWorkerRequest::LoadOnly
        );
    }

    #[test]
    pub(super) fn warm_registry_hydration_is_deferred_only_for_normal_boot() {
        assert!(defer_warm_registry_hydration(false, false, true, false));
        assert!(!defer_warm_registry_hydration(true, false, true, false));
        assert!(!defer_warm_registry_hydration(false, true, true, false));
        assert!(!defer_warm_registry_hydration(false, false, false, false));
        assert!(!defer_warm_registry_hydration(false, false, true, true));
    }

    #[test]
    pub(super) fn summary_seed_skips_refresh_but_preserves_return_hydration() {
        assert_eq!(
            summary_seed_catalog_worker_request(CatalogRefreshPolicy::Off, false, false),
            None
        );
        assert_eq!(
            summary_seed_catalog_worker_request(CatalogRefreshPolicy::Default, false, false),
            None
        );
        assert_eq!(
            summary_seed_catalog_worker_request(CatalogRefreshPolicy::Off, false, true),
            Some(CatalogWorkerRequest::StrictLoad)
        );
        assert_eq!(
            summary_seed_catalog_worker_request(CatalogRefreshPolicy::Default, false, true),
            Some(CatalogWorkerRequest::StrictLoad)
        );
        assert_eq!(
            summary_seed_catalog_worker_request(CatalogRefreshPolicy::Off, true, true),
            Some(CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS)
        );
    }

    #[test]
    pub(super) fn summary_warm_validation_defers_non_return_hydration() {
        assert!(!summary_seed_catalog_worker_starts_immediately(
            CatalogWorkerRequest::CheckStamp,
            false
        ));
        assert!(summary_seed_catalog_worker_starts_immediately(
            CatalogWorkerRequest::CheckStamp,
            true
        ));
        assert!(summary_seed_catalog_worker_starts_immediately(
            CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
            false
        ));
    }

    #[test]
    pub(super) fn cold_catalog_worker_starts_after_first_copy_without_delay() {
        let before_copy = deferred_catalog_worker_start_policy(
            false,
            false,
            false,
            false,
            Duration::from_secs(2),
        );
        assert!(!before_copy.allowed);
        assert_eq!(before_copy.delay, Duration::ZERO);
        assert!(before_copy.foreground);

        let after_copy =
            deferred_catalog_worker_start_policy(false, true, false, false, Duration::from_secs(2));
        assert!(after_copy.allowed);
        assert_eq!(after_copy.delay, Duration::ZERO);
        assert!(matches!(
            deferred_catalog_worker_lifecycle_input(
                CatalogExecutionMode::ForegroundExclusive,
                CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
            ),
            LauncherLifecycleInput::CatalogBuilding {
                foreground: true,
                has_stale_catalog: false,
                ..
            }
        ));
    }

    #[test]
    pub(super) fn return_hydration_can_start_before_a_visible_copy() {
        let policy =
            deferred_catalog_worker_start_policy(false, false, true, false, Duration::from_secs(2));
        assert!(policy.allowed);
        assert_eq!(policy.delay, Duration::ZERO);
        assert!(policy.foreground);
    }

    #[test]
    pub(super) fn initial_hydration_can_start_before_a_visible_copy() {
        let policy =
            deferred_catalog_worker_start_policy(false, false, false, true, Duration::from_secs(2));
        assert!(policy.allowed);
        assert_eq!(policy.delay, Duration::ZERO);
        assert!(policy.foreground);
    }

    #[test]
    pub(super) fn warm_catalog_worker_starts_without_an_interaction_gate() {
        let delay = Duration::from_secs(2);
        let allowed = deferred_catalog_worker_start_policy(true, true, false, false, delay);
        assert!(allowed.allowed);
        assert_eq!(allowed.delay, delay);
        assert!(matches!(
            deferred_catalog_worker_lifecycle_input(
                CatalogExecutionMode::BackgroundInteractive,
                CatalogWorkerRequest::CheckStamp,
            ),
            LauncherLifecycleInput::CatalogValidationStarted
        ));
    }

    #[test]
    pub(super) fn catalog_interaction_idle_ignores_resting_stick_noise() {
        let mut resting = PadState {
            left_x: 0.5,
            right_y: -1.0,
            ..PadState::default()
        };
        assert!(!pad_state_has_active_input(&resting));

        resting.dpad_right = true;
        assert!(pad_state_has_active_input(&resting));

        resting.dpad_right = false;
        resting.btn_a = true;
        assert!(pad_state_has_active_input(&resting));
    }

    #[test]
    pub(super) fn direct_preview_request_is_scoped_to_the_arcade_screen() {
        assert!(direct_preview_requested(Screen::Arcade, false, true));
        assert!(!direct_preview_requested(Screen::Settings, false, true));
        assert!(!direct_preview_requested(Screen::Home, false, true));
        assert!(!direct_preview_requested(Screen::Arcade, true, true));
        assert!(!direct_preview_requested(Screen::Arcade, false, false));
    }

    #[test]
    fn catalog_generation_is_capsule_eligible_only_when_published_durable() {
        let mut generation = CatalogGenerationState::default();
        generation.publish(Some("new".to_string()), false);
        assert!(generation.durable.is_none());

        generation.publish(Some("new".to_string()), true);
        assert_eq!(generation.durable.as_deref(), Some("new"));

        generation.publish(Some("next".to_string()), false);
        assert!(generation.durable.is_none());
    }

    #[test]
    fn warm_navigation_projection_reuses_seeded_taxonomy() {
        assert!(!catalog_taxonomy_sync_required(
            true,
            CatalogSource::NavigationProjection
        ));
        assert!(catalog_taxonomy_sync_required(
            false,
            CatalogSource::NavigationProjection
        ));
        assert!(catalog_taxonomy_sync_required(
            true,
            CatalogSource::FreshBuild
        ));
    }

    #[test]
    fn screensaver_idle_timer_resets_for_activity_and_catalog_work() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);
        let delay = Duration::from_secs(300);

        assert!(!saver.handle_input(start + Duration::from_secs(250), false, true));
        saver.update(start + Duration::from_secs(500), true, delay, false, true);
        assert!(!saver.active);
        saver.update(start + Duration::from_secs(551), true, delay, false, true);
        assert!(saver.active);

        saver.update(start + Duration::from_secs(552), true, delay, true, true);
        assert!(!saver.active);
        assert!(saver.take_restore_full_frame());
        saver.update(start + Duration::from_secs(851), true, delay, false, true);
        assert!(!saver.active);
        saver.update(start + Duration::from_secs(852), true, delay, false, true);
        assert!(saver.active);
    }

    #[test]
    fn direct_layers_are_never_desired_without_both_intent_and_permission() {
        assert!(should_desire_direct_layer(true, true));
        assert!(!should_desire_direct_layer(false, true));
        assert!(!should_desire_direct_layer(true, false));
        assert!(!should_desire_direct_layer(false, false));
    }

    #[test]
    fn preview_layer_stays_owned_while_replacement_is_pending() {
        assert!(should_desire_preview_direct_layer(
            false, true, true, true, true, false
        ));
        assert!(!should_desire_preview_direct_layer(
            false, true, false, true, true, false
        ));
        assert!(!should_desire_preview_direct_layer(
            false, true, true, false, true, false
        ));
        assert!(!should_desire_preview_direct_layer(
            true, false, true, true, true, false
        ));
        assert!(should_desire_preview_direct_layer(
            false, true, false, false, true, true
        ));
    }

    #[test]
    fn preview_layer_retires_when_route_stops_wanting_preview() {
        assert!(!should_desire_preview_direct_layer(
            false, true, false, true, true, false
        ));
    }

    #[test]
    fn preview_compositor_starts_once_only_for_an_active_hdmi_preview() {
        assert!(should_start_preview_compositor(
            true, true, true, false, false
        ));
        assert!(!should_start_preview_compositor(
            true, false, true, false, false
        ));
        assert!(!should_start_preview_compositor(
            false, true, true, false, false
        ));
        assert!(!should_start_preview_compositor(
            true, true, true, false, true
        ));
        assert!(!should_start_preview_compositor(
            true, true, true, true, false
        ));
    }

    #[test]
    fn screensaver_idle_start_keeps_waiting_for_startup_catalog_work() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::IdleWhenReady);
        let delay = Duration::from_secs(300);

        saver.update(start, true, delay, true, false);
        assert!(!saver.active);
        assert_eq!(saver.start_mode, ScreensaverStartMode::IdleWhenReady);
        saver.update(start + Duration::from_secs(1), true, delay, true, true);
        assert!(!saver.active);
        saver.update(start + Duration::from_secs(2), true, delay, false, true);
        assert!(saver.active);
        assert_eq!(saver.start_mode, ScreensaverStartMode::Inactive);
    }

    #[test]
    fn legacy_screensaver_start_active_uses_preview_semantics() {
        assert_eq!(
            screensaver_start_mode(false, false, true),
            ScreensaverStartMode::PreviewWhenReady
        );
        assert_eq!(
            screensaver_start_mode(true, false, true),
            ScreensaverStartMode::IdleWhenReady
        );
        assert_eq!(
            screensaver_start_mode(true, true, true),
            ScreensaverStartMode::PreviewWhenReady
        );
    }

    #[test]
    fn benchmark_preview_waits_for_process_analytics_after_content_is_ready() {
        assert!(!screensaver_preview_start_ready(
            false,
            false,
            FrameAnalyticsMode::Process
        ));
        assert!(screensaver_preview_start_ready(
            true,
            false,
            FrameAnalyticsMode::Off
        ));
        assert!(!screensaver_preview_start_ready(
            true,
            true,
            FrameAnalyticsMode::Wall
        ));
        assert!(screensaver_preview_start_ready(
            true,
            true,
            FrameAnalyticsMode::Process
        ));
    }

    #[test]
    fn screensaver_preview_start_waits_for_content_then_uses_preview_input_semantics() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::PreviewWhenReady);
        let delay = Duration::from_secs(300);

        saver.update(start, true, delay, true, false);
        assert!(!saver.active);
        assert_eq!(saver.start_mode, ScreensaverStartMode::PreviewWhenReady);

        let ready = start + Duration::from_millis(16);
        saver.update(ready, true, delay, true, true);
        assert!(saver.active);
        assert!(saver.is_preview());
        assert_eq!(saver.start_mode, ScreensaverStartMode::Inactive);
        assert!(saver.handle_input(ready, true, true));
        assert!(saver.active);
        assert!(saver.handle_input(ready + Duration::from_millis(16), false, true));
        assert!(saver.active);
    }

    #[test]
    fn screenshot_screensaver_waits_for_catalog_work() {
        assert!(screensaver_catalog_busy(true, false));
        assert!(!screensaver_catalog_busy(false, true));
    }

    #[test]
    fn preview_is_preserved_for_pipeline_start() {
        let start = Instant::now();
        let next_frame = start + Duration::from_millis(16);
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);

        saver.preview(start);
        saver.update(next_frame, false, Duration::from_secs(300), true, true);

        assert!(saver.active);
        assert!(saver.preview_active);
        assert!(!saver.restore_full_frame);
        assert!(screensaver_pipeline_start_allowed(saver.active, false));
    }

    #[test]
    fn settings_screensaver_preview_waits_for_activation_release_then_consumes_next_input() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);
        let mut physical_input = PadState {
            btn_a: true,
            ..PadState::default()
        };
        assert!(!saver.input_held_for_control(false, true));

        saver.preview(start);
        saver.update(start, true, Duration::from_secs(300), true, true);
        assert!(saver.active);
        let period = Duration::from_millis(20);
        let fade: Vec<_> = (0..12).map(|_| saver.preview_fade_alpha(period)).collect();
        assert_eq!(fade[0], Some(0));
        assert_eq!(fade[5], Some(127));
        assert_eq!(fade[10], Some(255));
        assert_eq!(fade[11], Some(255));
        let activation_held =
            saver.input_held_for_control(false, pad_state_has_active_input(&physical_input));
        assert!(saver.handle_input(start, activation_held, true));
        assert!(saver.active);
        let activation_still_held =
            saver.input_held_for_control(false, pad_state_has_active_input(&physical_input));
        assert!(saver.handle_input(
            start + Duration::from_millis(16),
            activation_still_held,
            true
        ));
        assert!(saver.active);

        physical_input.btn_a = false;
        let activation_released =
            saver.input_held_for_control(false, pad_state_has_active_input(&physical_input));
        assert!(saver.handle_input(start + Duration::from_millis(32), activation_released, true));
        assert!(saver.active);

        assert!(!saver.handle_input(start + Duration::from_millis(48), false, false));
        assert!(saver.active);
        let next_input = saver.input_held_for_control(true, true);
        assert!(saver.handle_input(start + Duration::from_secs(1), next_input, true));
        assert!(!saver.active);
        assert!(saver.take_restore_full_frame());
        assert!(!saver.take_restore_full_frame());
        assert!(!saver.handle_input(start + Duration::from_secs(2), true, true));
    }

    #[test]
    fn idle_screensaver_view_always_routes_activity_to_dismissal() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);
        saver.update(
            start + Duration::from_secs(301),
            true,
            Duration::from_secs(300),
            false,
            true,
        );
        let view = EffectiveLauncherView::resolve_state(
            &LauncherLifecycleState::Idle,
            saver.active,
            Screen::Settings,
        );

        assert_eq!(view, EffectiveLauncherView::Screensaver);
        assert!(view.accepts_application_input());
        assert!(saver.handle_input(start + Duration::from_secs(302), true, true));
        assert!(!saver.active);
        assert!(saver.take_restore_full_frame());
    }

    #[test]
    fn genuine_launch_wins_over_screensaver_and_releases_its_resources() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::IdleWhenReady);
        saver.update(start, true, Duration::from_secs(300), false, true);
        assert!(saver.active);

        let launch_state = LauncherLifecycleState::Launching {
            phase: LaunchingPhase::HandoffPending,
        };
        let view =
            EffectiveLauncherView::resolve_state(&launch_state, saver.active, Screen::Arcade);
        assert_eq!(view, EffectiveLauncherView::Launching);
        assert!(saver.cancel_for_exclusive_view(start + Duration::from_millis(1)));
        assert!(!saver.active);
        assert!(saver.take_restore_full_frame());
    }

    #[test]
    fn disabled_screensaver_never_activates_but_preview_still_can() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);

        saver.update(
            start + Duration::from_secs(600),
            false,
            Duration::from_secs(60),
            false,
            true,
        );
        assert!(!saver.active);
        saver.preview(start + Duration::from_secs(601));
        assert!(saver.active);
    }

    #[test]
    fn failed_screensaver_waits_for_fresh_activity_before_reactivation() {
        let start = Instant::now();
        let delay = Duration::from_secs(300);
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);
        saver.update(start + delay, true, delay, false, true);
        assert!(saver.active);

        saver.fail_current_activation(start + delay);
        saver.update(start + delay + delay, true, delay, false, true);
        assert!(!saver.active);

        saver.handle_input(start + delay + delay, false, true);
        saver.update(start + delay + delay + delay, true, delay, false, true);
        assert!(saver.active);
    }

    #[test]
    fn arcade_preview_availability_updates_artifact_and_menu_aliases() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/fixture"),
            Vec::new(),
            vec![
                arcade_catalog::GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1,
                },
                arcade_catalog::GameSystemEntry {
                    id: arcade_catalog::MENU_ARCADE_SYSTEM_ID.into(),
                    title: "Arcade".into(),
                    count: 1,
                },
                arcade_catalog::GameSystemEntry {
                    id: "snes".into(),
                    title: "Super Nintendo".into(),
                    count: 10,
                },
            ],
        );
        let game = mister_magik_catalog::system_shard::SystemGame {
            stable_key: "arcade\u{1f}1943 kai".into(),
            title: "1943- Kai Midway Kaisen (JP)".into(),
            launch_ref: "/media/fat/_Arcade/1943 Kai.mra".into(),
            preview_archive_path: "/media/fat/mister-magik-dev/assets/arcade-screenshots.mmlz4b"
                .into(),
            preview_asset_key: "1943kai".into(),
            has_preview: true,
            ..Default::default()
        };

        let updated = catalog_with_preview_availability(&catalog, "arcade", &[game]);

        for system_id in ["arcade", arcade_catalog::MENU_ARCADE_SYSTEM_ID] {
            let row = updated.system_game_at(system_id, 0).unwrap();
            assert!(row.has_preview);
            assert_eq!(row.preview_asset_key.as_ref(), "1943kai");
        }
        assert_eq!(
            updated
                .systems
                .iter()
                .find(|system| system.id == "snes")
                .map(|system| system.count),
            Some(10)
        );
    }

    #[test]
    pub(super) fn startup_without_registry_starts_the_fast_builder() {
        assert_eq!(
            catalog_startup_without_registry_plan(true),
            CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::FreshBuild,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            }
        );
        assert_eq!(
            catalog_startup_without_registry_plan(false),
            CatalogStartupWithoutSummaryPlan::NoCatalog
        );
    }

    #[test]
    pub(super) fn first_build_survives_non_intro_startup_sequences() {
        for (startup_mode, screensaver_start_mode, portrait) in [
            (
                StartupMode::ColdNoCatalog,
                ScreensaverStartMode::IdleWhenReady,
                false,
            ),
            (
                StartupMode::ColdNoCatalog,
                ScreensaverStartMode::Inactive,
                true,
            ),
            (
                StartupMode::ReturnFromGame,
                ScreensaverStartMode::Inactive,
                false,
            ),
        ] {
            assert!(!startup_intro_is_eligible(
                startup_mode,
                false,
                screensaver_start_mode,
                portrait,
            ));

            let CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request,
                initial_cache,
                execution_mode,
            } = catalog_startup_without_registry_plan(true)
            else {
                panic!("cold startup must schedule a catalog worker");
            };
            let mut session = LauncherCatalogSession::new(false);
            let now = Instant::now();
            session.defer_catalog_worker(
                "/media/fat/_Arcade".to_string(),
                request,
                initial_cache,
                execution_mode,
            );
            assert!(
                session
                    .maybe_start_deferred_worker(false, false, true, now, Duration::ZERO)
                    .is_none()
            );
            let worker = session
                .maybe_start_deferred_worker(false, true, true, now, Duration::ZERO)
                .expect("first visible copy starts the first build");
            assert_eq!(worker.request, CatalogWorkerRequest::FreshBuild);
            assert_eq!(
                worker.initial_cache,
                CatalogWorkerInitialCache::AlreadyProbedMissing
            );
            assert_eq!(
                worker.execution_mode,
                CatalogExecutionMode::ForegroundExclusive
            );
        }
    }

    #[test]
    pub(super) fn predecessor_migration_keeps_the_particle_intro_eligible() {
        assert!(startup_intro_is_eligible(
            StartupMode::ColdNoCatalog,
            true,
            ScreensaverStartMode::IdleWhenReady,
            true,
        ));
        assert!(!startup_intro_is_eligible(
            StartupMode::WarmCatalog,
            true,
            ScreensaverStartMode::Inactive,
            false,
        ));
    }

    #[test]
    fn main_proxy_press_moves_root_card_after_idle() {
        let catalog = empty_arcade_catalog("/tmp");
        let mut nav = LauncherNav::new();
        let focus = launcher_screen_input_focus(&nav);
        let mut router = InputRouter::new(focus);
        let start = Instant::now();
        let press_at = start + Duration::from_secs(2);
        nav.handle_held_tick_with_navigation_intents(&PadState::default(), start, &catalog);
        nav.handle_held_tick_with_navigation_intents(
            &PadState::default(),
            press_at - Duration::from_millis(16),
            &catalog,
        );

        let mut press = normalized_test_press(LogicalAction::Right);
        press.source.kind = InputSourceKind::MainProxy;
        let InputOutcome::Dispatch { event, .. } = router.route_event(press, focus, press_at)
        else {
            panic!("root card press should dispatch");
        };
        nav.handle_action_with_navigation_intents(&event, press_at, &catalog);
        let mut held = PadState::default();
        held.set_logical_action(
            LogicalAction::Right,
            router.action_held(LogicalAction::Right),
        );
        nav.handle_held_tick_with_navigation_intents(&held, press_at, &catalog);
        assert_eq!(nav.selected, 1);

        let release_at = press_at + Duration::from_millis(80);
        let mut release = press;
        release.sequence += 1;
        release.phase = InputPhase::Released;
        assert!(matches!(
            router.route_event(release, focus, release_at),
            InputOutcome::Released { .. }
        ));
        nav.handle_action_with_navigation_intents(&release, release_at, &catalog);
        nav.handle_held_tick_with_navigation_intents(&PadState::default(), release_at, &catalog);
        for frame in 1..=120 {
            nav.handle_held_tick_with_navigation_intents(
                &PadState::default(),
                release_at + Duration::from_millis(frame * 16),
                &catalog,
            );
        }
        assert_eq!(nav.selected, 1);
    }

    #[test]
    fn refresh_hold_keeps_initial_press_capture_through_confirmation() {
        let catalog = empty_arcade_catalog("/tmp");
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Settings;
        nav.settings_selected = 6;
        let initial_focus = launcher_screen_input_focus(&nav);
        let mut router = InputRouter::new(initial_focus);
        let now = Instant::now();
        let mut press = normalized_test_press(LogicalAction::Activate);
        press.source.kind = InputSourceKind::MainProxy;
        let InputOutcome::Dispatch { event, .. } = router.route_event(press, initial_focus, now)
        else {
            panic!("refresh press should dispatch");
        };
        assert!(
            nav.handle_action_with_navigation_intents(&event, now, &catalog)
                .is_none()
        );
        assert_eq!(
            nav.confirm_action,
            Some(launcher::ConfirmAction::RefreshDatabase)
        );
        assert_eq!(nav.confirm_selected, 0);

        let focus = launcher_input_focus(true, false, false, false, true, false, &nav);
        router.set_focus(focus);
        assert_eq!(focus, initial_focus);
        let held = PadState {
            btn_a: router.action_held(LogicalAction::Activate),
            ..PadState::default()
        };
        assert!(held.btn_a);
        assert!(
            nav.handle_held_tick_with_navigation_intents(
                &held,
                now + Duration::from_millis(6999),
                &catalog
            )
            .is_none()
        );
        let reset = nav
            .handle_held_tick_with_navigation_intents(&held, now + Duration::from_secs(7), &catalog)
            .expect("continuous initial press should reset");
        assert_eq!(reset.action, LauncherAction::PurgeLibraryData);
        assert_eq!(nav.confirm_action, None);
    }

    #[test]
    fn system_entry_destination_needs_rows_and_preview_in_one_frame() {
        let mut ready = SystemEntryAdoption {
            entered: true,
            rows_ready: true,
            preview_exact: true,
            ..SystemEntryAdoption::default()
        };
        ready.note_destination_frame(Screen::Arcade, 0);
        assert!(!ready.destination_prepared);
        ready.note_destination_frame(Screen::Arcade, 240);
        assert!(ready.destination_prepared);

        let mut waiting = SystemEntryAdoption {
            entered: true,
            rows_ready: true,
            ..SystemEntryAdoption::default()
        };
        waiting.note_destination_frame(Screen::Arcade, 240);
        assert!(!waiting.destination_prepared);
    }

    #[test]
    fn system_entry_ready_frame_needs_main_active_confirmation() {
        let mut entry = SystemEntryAdoption {
            entered: true,
            rows_ready: true,
            preview_exact: true,
            destination_prepared: true,
            ..SystemEntryAdoption::default()
        };
        entry.note_ready_frame(Screen::Arcade, 240, false);
        assert!(!entry.ready_presented);
        assert!(entry.preview_adoption_in_progress());
        entry.note_ready_frame(Screen::Arcade, 240, true);
        assert!(entry.ready_presented);
        assert!(!entry.preview_adoption_in_progress());
    }

    #[test]
    fn system_entry_holds_preview_work_only_between_rows_and_the_ready_frame() {
        let mut entry = SystemEntryAdoption::default();
        assert!(!entry.preview_adoption_in_progress());
        entry.note_rows_ready();
        assert!(
            !entry.preview_adoption_in_progress(),
            "rows without an entry press"
        );
        entry.note_enter();
        entry.note_rows_ready();
        assert!(entry.preview_adoption_in_progress());
        entry.cancel();
        assert!(!entry.preview_adoption_in_progress());
    }

    #[test]
    fn launcher_idle_wait_rejects_active_work() {
        for reason in [
            LauncherWakeReasons::REDRAW_PENDING,
            LauncherWakeReasons::LAUNCHING,
            LauncherWakeReasons::SETUP_ACTIVE,
            LauncherWakeReasons::TOOLING_SEQUENCE_ACTIVE,
            LauncherWakeReasons::ROUTE_FORCES_FULL_PRESENT,
            LauncherWakeReasons::BRIDGE_DIRTY,
            LauncherWakeReasons::CATALOG_MESSAGES_ACTIVE,
            LauncherWakeReasons::MEDIA_MESSAGE_SEEN,
            LauncherWakeReasons::SLINT_ANIMATION_ACTIVE,
            LauncherWakeReasons::HOME_PAN_PRESENT_ACTIVE,
            LauncherWakeReasons::HOME_HORIZONTAL_INPUT_HELD,
            LauncherWakeReasons::ARCADE_VISUAL_CHANGED_THIS_LOOP,
            LauncherWakeReasons::ARCADE_SCROLL_ACTIVE,
            LauncherWakeReasons::ARCADE_FILTER_SCROLL_ACTIVE,
            LauncherWakeReasons::ARCADE_SEARCH_ACTIVE,
            LauncherWakeReasons::PREVIEW_DIRTY,
            LauncherWakeReasons::PREVIEW_SCHEDULED_THIS_LOOP,
            LauncherWakeReasons::CRT_BACKDROP_PREPARED,
            LauncherWakeReasons::COMPOSITION_FORCES_FULL_PRESENT,
            LauncherWakeReasons::COMPOSITION_CLEARS_DIRECT_LAYERS,
            LauncherWakeReasons::LATENCY_CRITICAL_INPUT,
        ] {
            assert!(
                !LauncherRenderIntent {
                    first_visible_copy_done: true,
                    startup_input_enabled: true,
                    wake_reasons: reason,
                }
                .can_sleep()
            );
        }
    }
}
