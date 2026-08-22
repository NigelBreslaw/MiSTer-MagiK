// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::arcade_drawer::{ArcadeDrawerViewCache, arcade_filter_cache_token};
use super::crt_backdrop_controller::CrtBackdropController;
use super::launcher_frame_accounting::{
    FrameAnalyticsCpuStamp, FrameAnalyticsMode, LauncherCustomDrawTrace, LauncherFrameAccounting,
    LauncherFrameCpuTrace, LauncherFrameIdentity, LauncherFrameRenderData,
    LauncherFrameSnapshotBuilder, LauncherFrameStatusData, LauncherFrameTiming,
};
#[cfg(test)]
use super::launcher_frame_pipeline::{LauncherFramePhase, LauncherFramePhaseObserver};
use super::launcher_pacing::{
    FB0_LATE_FRAME_START_HEADROOM_US, FrameProductionClass, FrameProductionTrace,
    LauncherFramePacingInput, LauncherFramePacingPolicy, LauncherPacingTrace,
    LauncherPhaseAlignment,
};
use super::launcher_screensaver::ScreensaverRenderTrace;
use super::launcher_worker_intents::{
    LauncherWorkerUiIntent, apply_launcher_worker_ui_intent, catalog_scan_message,
};
#[cfg(test)]
use super::launcher_worker_intents::{
    catalog_background_scan_progress_visible, catalog_scan_progress_visible,
};
use super::*;
use crate::input_event::{InputPhase, InputSourceKind, LogicalAction};
use crate::input_state::PadState;
use crate::launcher_presentation::SelectionFeedbackTarget;
use crate::preview_state::PreviewApplyTrace;
use crate::preview_worker;
use mister_magik_catalog::builder_service::CatalogWorkMode;
#[cfg(test)]
use mister_magik_catalog::catalog_summary;
#[cfg(test)]
use mister_magik_fb::framebuffer::target::PhysicalLayerBacking;
use mister_magik_fb::process_config::{ScreensaverStartMode, ScriptedInputConfig};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};

const DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY: Duration = Duration::from_secs(2);
const CATALOG_READY_STATIONARY_EDGE_SETTLE: Duration = Duration::from_millis(250);
const CATALOG_IDLE_BURST_SETTLE: Duration = Duration::from_millis(150);
const CATALOG_IDLE_BURST_SLEEP_LIMIT: Duration = Duration::from_millis(250);
const LIBRARY_CHANGED_TEST_ACTION_SETTLE: Duration = Duration::from_millis(1200);
const LAUNCHER_INPUT_SCRIPT_PRESS_FRAMES: usize = 2;
const LAUNCHER_INPUT_SCRIPT_RELEASE_FRAMES: usize = 6;
const SYSTEM_ENTRY_BENCHMARK_SETTLE_MS: u64 = 2_000;
const SETTINGS_NAVIGATION_STATUS_DRAIN_MIN: Duration = Duration::from_millis(500);
const SETTINGS_NAVIGATION_STATUS_DRAIN_LIMIT: Duration = Duration::from_secs(2);
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
const MODAL_INPUT_TEST_ROOT: &str = "/tmp/mister-magik/modal-input-benchmark";

fn navigation_geometry_to_composition(
    layout: UiLayoutGeometry,
    mut geometry: NavigationTransitionGeometry,
) -> NavigationTransitionGeometry {
    fn map_rect(
        layout: UiLayoutGeometry,
        rect: NavigationTransitionRect,
    ) -> NavigationTransitionRect {
        if rect.width == 0 || rect.height == 0 {
            return rect;
        }
        let mapped = layout.logical_rect_to_composition(DirtyRect {
            x0: rect.x as usize,
            y0: rect.y as usize,
            x1: rect.right() as usize,
            y1: rect.bottom() as usize,
        });
        NavigationTransitionRect {
            x: mapped.x0.min(u16::MAX as usize) as u16,
            y: mapped.y0.min(u16::MAX as usize) as u16,
            width: mapped.width().min(u16::MAX as usize) as u16,
            height: mapped.rows().min(u32::from(u16::MAX)) as u16,
        }
    }

    geometry.source_card = map_rect(layout, geometry.source_card);
    geometry.source_label = map_rect(layout, geometry.source_label);
    geometry.source_detail = map_rect(layout, geometry.source_detail);
    geometry.destination_title = map_rect(layout, geometry.destination_title);
    geometry.destination_detail = map_rect(layout, geometry.destination_detail);
    geometry.destination_list = map_rect(layout, geometry.destination_list);
    geometry.destination_selected_row = map_rect(layout, geometry.destination_selected_row);
    geometry.destination_preview = map_rect(layout, geometry.destination_preview);
    geometry.destination_footer = map_rect(layout, geometry.destination_footer);
    geometry
}

fn accepted_selection_feedback_input(event: Option<&crate::input_event::InputEvent>) -> bool {
    event.is_some_and(|event| event.phase == InputPhase::Pressed)
}

fn system_entry_benchmark_settled(elapsed_ms: u64, input_enabled_ms: u64) -> bool {
    elapsed_ms.saturating_sub(input_enabled_ms) >= SYSTEM_ENTRY_BENCHMARK_SETTLE_MS
}

fn navigation_capture_source_carrier_required(
    policy: FullScreenTransitionPolicy,
    owner: Option<FullScreenTransitionOwner>,
    phase: NavigationTransitionPhase,
    settings_physical_space: bool,
) -> bool {
    policy.controlled_capture
        && owner == Some(FullScreenTransitionOwner::Navigation)
        && phase == NavigationTransitionPhase::Capture
        && settings_physical_space
}

fn orientation_capture_source_carrier_required(
    policy: FullScreenTransitionPolicy,
    owner: Option<FullScreenTransitionOwner>,
    transition_active: bool,
    destination_ready: bool,
) -> bool {
    policy.controlled_capture
        && owner == Some(FullScreenTransitionOwner::Orientation)
        && transition_active
        && !destination_ready
}

fn settings_navigation_status_drain_complete(elapsed: Duration, status_current: bool) -> bool {
    elapsed >= SETTINGS_NAVIGATION_STATUS_DRAIN_LIMIT
        || (elapsed >= SETTINGS_NAVIGATION_STATUS_DRAIN_MIN && status_current)
}

fn settings_navigation_status_drain_plan(
    sequence_before_frame: u64,
    sequence_after_frame: u64,
) -> (u64, bool) {
    if sequence_after_frame > sequence_before_frame {
        (sequence_before_frame, false)
    } else {
        (sequence_after_frame, true)
    }
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
        Screen::SystemHub => Some(SelectionFeedbackTarget::new(
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
                "screensaver",
                "reduce-motion",
                "exit",
                "rebuild",
                "about",
            ]
            .get(nav.settings_selected)
            .copied()
            .unwrap_or("unknown"),
        )),
        Screen::Screensaver => Some(SelectionFeedbackTarget::new(
            "screensaver-settings",
            ["enabled", "delay", "preview"]
                .get(nav.screensaver_selected)
                .copied()
                .unwrap_or("unknown"),
        )),
        Screen::About => Some(SelectionFeedbackTarget::new(
            "about",
            ["info", "licenses"]
                .get(nav.about_selected)
                .copied()
                .unwrap_or("unknown"),
        )),
        Screen::Licenses if !nav.licenses_expanded => Some(SelectionFeedbackTarget::new(
            "licenses",
            [
                "mister-magik",
                "ffmpeg",
                "press-start-2p",
                "commercial-fonts",
                "jersey-25",
                "jersey-15",
                "terminus-font",
                "spleen",
                "arcade-cabinet",
                "slint",
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
        Screen::Arcade | Screen::Controller | Screen::Info | Screen::Licenses => None,
    }
}
const ORIENTATION_TRANSITION_BENCHMARK_EVIDENCE_ENV: &str =
    "MISTER_ORIENTATION_TRANSITIONS_EVIDENCE_DIR";
const SETTINGS_NAVIGATION_BENCHMARK_EVIDENCE_ENV: &str = "MISTER_SETTINGS_NAVIGATION_EVIDENCE_DIR";

fn launcher_screen_input_focus(nav: &LauncherNav) -> FocusRequest {
    let (owner, directional_policy) = match nav.screen {
        Screen::Home => (1, DirectionalPolicy::HomeContinuous),
        Screen::SystemHub => (2, DirectionalPolicy::MenuRepeat),
        Screen::Controller => (3, DirectionalPolicy::EdgeOnly),
        Screen::Arcade if nav.arcade_uses_menu_repeat() => (4, DirectionalPolicy::MenuRepeat),
        Screen::Arcade => (4, DirectionalPolicy::ArcadeContinuous),
        Screen::Settings => (5, DirectionalPolicy::MenuRepeat),
        Screen::Screensaver => (6, DirectionalPolicy::EdgeOnly),
        Screen::About => (7, DirectionalPolicy::MenuRepeat),
        Screen::Licenses => (8, DirectionalPolicy::MenuRepeat),
        Screen::Info => (9, DirectionalPolicy::MenuRepeat),
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
    } else if modal {
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

fn orientation_transition_benchmark_evidence_dir() -> Option<std::path::PathBuf> {
    std::env::var_os(ORIENTATION_TRANSITION_BENCHMARK_EVIDENCE_ENV)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

fn settings_navigation_benchmark_evidence_dir() -> Option<std::path::PathBuf> {
    std::env::var_os(SETTINGS_NAVIGATION_BENCHMARK_EVIDENCE_ENV)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

fn settings_navigation_presentation_snapshot_json(
    capture: SettingsNavigationPresentationCapture,
) -> serde_json::Value {
    let telemetry = capture.telemetry;
    serde_json::json!({
        "owned_vblank_count": telemetry.owned_vblank_count,
        "presented_vblank_count": telemetry.presented_vblank_count,
        "repeated_vblank_count": telemetry.repeated_vblank_count,
        "ownership_loss_count": telemetry.ownership_loss_count,
        "active_sequence": telemetry.active_sequence,
        "magik_ownership": telemetry.magik_ownership(),
        "pending": telemetry.pending(),
        "lifetime_invariant_valid": telemetry.lifetime_invariant_valid(),
    })
}

fn orientation_transition_presentation_snapshot_json(
    capture: OrientationTransitionPresentationCapture,
) -> serde_json::Value {
    let telemetry = capture.telemetry;
    serde_json::json!({
        "owned_vblank_count": telemetry.owned_vblank_count,
        "presented_vblank_count": telemetry.presented_vblank_count,
        "repeated_vblank_count": telemetry.repeated_vblank_count,
        "ownership_loss_count": telemetry.ownership_loss_count,
        "active_sequence": telemetry.active_sequence,
        "magik_ownership": telemetry.magik_ownership(),
        "pending": telemetry.pending(),
        "lifetime_invariant_valid": telemetry.lifetime_invariant_valid(),
    })
}

fn write_settings_navigation_benchmark_completion(
    directory: &Path,
    benchmark: &SettingsNavigationBenchmark,
    frames: u64,
) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let records = benchmark
        .records()
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let presentation_start = record
                .presentation_start
                .map(settings_navigation_presentation_snapshot_json);
            let presentation_end = record
                .presentation_end
                .map(settings_navigation_presentation_snapshot_json);
            serde_json::json!({
                "leg": index + 1,
                "orientation": record.orientation.id(),
                "route": record.leg.route.label(),
                "renderer": record.leg.route.renderer(),
                "direction": record.leg.direction.label(),
                "source": screen_label(record.leg.source),
                "destination": screen_label(record.leg.destination),
                "start_frame": record.start_frame,
                "rendered_endpoint_frame": record.rendered_endpoint_frame,
                "presented_endpoint_frame": record.presented_endpoint_frame,
                "presented_sequence": record.presented_sequence,
                "presentation_window": {
                    "schema": "mister-magik-settings-navigation-presentation-window-v1",
                    "source": "fpga-owned-vblank-telemetry",
                    "start": presentation_start,
                    "end": presentation_end,
                    "elapsed_us": record.presentation_elapsed_us,
                    "error": record.presentation_error,
                },
            })
        })
        .collect::<Vec<_>>();
    let document = serde_json::json!({
        "schema": "mister-magik-settings-navigation-transition-v4",
        "state": if benchmark.complete() { "complete" } else { "failed" },
        "failure": benchmark.failure(),
        "orientations": SETTINGS_NAVIGATION_ORIENTATIONS.map(ScreenOrientation::id),
        "frames": frames,
        "route": ["home", "settings", "about", "info", "about", "settings", "home", "settings", "about", "info", "about", "settings", "home"],
        "records": records,
    });
    let temporary = directory.join("completion.json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(&document).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(temporary, directory.join("completion.json"))
}

fn write_orientation_transition_benchmark_completion(
    directory: &Path,
    benchmark: &OrientationTransitionBenchmark,
    frames: u64,
) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let records = benchmark
        .records()
        .iter()
        .map(|record| {
            let presentation_start = record
                .presentation_start
                .map(orientation_transition_presentation_snapshot_json);
            let presentation_end = record
                .presentation_end
                .map(orientation_transition_presentation_snapshot_json);
            serde_json::json!({
                "leg": record.leg.index + 1,
                "effect": record.leg.effect.id(),
                "label": record.leg.label(),
                "from": record.leg.from.id(),
                "to": record.leg.to.id(),
                "start_frame": record.start_frame,
                "rendered_endpoint_frame": record.rendered_endpoint_frame,
                "presented_endpoint_frame": record.presented_endpoint_frame,
                "presented_sequence": record.presented_sequence,
                "presentation_window": {
                    "schema": "mister-magik-orientation-transition-presentation-window-v1",
                    "source": "fpga-owned-vblank-telemetry",
                    "start": presentation_start,
                    "end": presentation_end,
                    "elapsed_us": record.presentation_elapsed_us,
                    "error": record.presentation_error,
                },
            })
        })
        .collect::<Vec<_>>();
    let document = serde_json::json!({
        "schema": "mister-magik-orientation-transition-v2",
        "state": if benchmark.complete() { "complete" } else { "failed" },
        "failure": benchmark.failure(),
        "effect": benchmark.effect().id(),
        "frames": frames,
        "route": ORIENTATION_TRANSITION_BENCHMARK_ROUTE
            .iter()
            .map(|orientation| orientation.id())
            .collect::<Vec<_>>(),
        "records": records,
    });
    let temporary = directory.join("completion.json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(&document).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(temporary, directory.join("completion.json"))
}

fn write_orientation_transition_pmu_completion(
    path: Option<&str>,
    effect: OrientationTransitionEffect,
) -> std::io::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let path = std::path::PathBuf::from(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let profile = mister_magik_perf_events::take_thread_profile();
    let document = serde_json::json!({
        "schema": "mister-magik-orientation-transition-pmu-v2",
        "state": if profile.enabled && profile.failure.is_none() && !profile.records.is_empty() && profile.dropped_spans == 0 {
            "complete"
        } else {
            "failed"
        },
        "route": ORIENTATION_TRANSITION_BENCHMARK_ROUTE
            .iter()
            .map(|orientation| orientation.id())
            .collect::<Vec<_>>(),
        "effect": effect.id(),
        "profile": profile,
    });
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&document).map_err(std::io::Error::other)?,
    )
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

struct ArcadeEntryLatencyTrace {
    writer: Option<std::io::BufWriter<std::fs::File>>,
    run_id: String,
    profile_path: Option<String>,
}

impl ArcadeEntryLatencyTrace {
    fn from_config(config: &mister_magik_fb::process_config::LauncherEntryTraceConfig) -> Self {
        let run_id = config.run_id().to_owned();
        let writer = config
            .trace_path()
            .and_then(|path| {
                let file = std::fs::File::create(path)
                    .map_err(|e| crate::ui_errln!("arcade entry trace: create {path} failed: {e}"))
                    .ok()?;
                let mut writer = std::io::BufWriter::with_capacity(16 * 1024, file);
                writer
                    .write_all(
                        b"event\trun_id\telapsed_ms\tdelta_ms\tsince_input_enabled_ms\taccepted\tsystem\tselected\tframe\tprepare_us\tpreview_state\tasset_key\tdetail\n",
                    )
                    .map_err(|e| crate::ui_errln!("arcade entry trace: header write failed: {e}"))
                    .ok()?;
                crate::ui_logln!("arcade_entry_trace={path} run_id={run_id}");
                Some(writer)
            });
        Self {
            writer,
            run_id,
            profile_path: config.profile_path().map(str::to_owned),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        start: Instant,
        event: &str,
        at: Instant,
        reference: Option<Instant>,
        input_enabled_ms: u64,
        accepted: bool,
        system: &str,
        selected: usize,
        frame: Option<u64>,
        prepare_us: Option<u128>,
        preview_state: &str,
        asset_key: &str,
        detail: impl std::fmt::Display,
    ) {
        let elapsed_ms = at.saturating_duration_since(start).as_millis();
        let delta_ms = reference
            .map(|reference| at.saturating_duration_since(reference).as_millis() as i128)
            .unwrap_or(-1);
        let since_input_enabled_ms = (elapsed_ms as i128 - input_enabled_ms as i128).max(0);
        let detail = detail.to_string();
        print_startup_event(
            start,
            event,
            format!(
                "delta_ms={} since_input_enabled_ms={} accepted={} system={} selected={} frame={} prepare_us={} preview_state={} asset_key={} {}",
                delta_ms,
                since_input_enabled_ms,
                u8::from(accepted),
                system,
                selected,
                frame
                    .map(|frame| frame.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                prepare_us
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                preview_state,
                asset_key,
                detail
            ),
        );
        if let Some(writer) = self.writer.as_mut() {
            let _ = writeln!(
                writer,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                event,
                self.run_id,
                elapsed_ms,
                delta_ms,
                since_input_enabled_ms,
                u8::from(accepted),
                system,
                selected,
                frame
                    .map(|frame| frame.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                prepare_us
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                preview_state,
                asset_key,
                detail.replace('\t', " ")
            );
            let _ = writer.flush();
        }
    }
}

struct ArcadeEntryLatencyTracker {
    trace: ArcadeEntryLatencyTrace,
    enter_input_at: Option<Instant>,
    destination_prepared: bool,
    enter_presented: bool,
    rows_ready: bool,
    preview_exact: bool,
    ready_presented: bool,
    first_nav_input_at: Option<Instant>,
    first_nav_presented: bool,
    catalog_resident_at_input: Option<bool>,
    presentation_start: Option<SystemEntryPresentationStart>,
}

#[derive(Clone, Copy)]
struct SystemEntryPresentationStart {
    telemetry: mister_magik_latch_contract::PresentationTelemetry,
    latch_drop_count: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SystemEntryPublicationPhases {
    bridge_model_assembly_us: u64,
    bridge_updates_us: u64,
    list_projection_us: u64,
    slint_raster_us: u64,
    overlay_composition_us: u64,
    latch_copy_us: u64,
    post_us: u64,
    confirmation_wait_wall_us: u64,
    confirmation_poll_cpu_us: u64,
}

impl SystemEntryPublicationPhases {
    fn from_presented_frame(frame: &LauncherPresentedFrame) -> Self {
        let bridge_total_us = u128_to_u64(frame.prepare_trace.bridge_sync_us);
        let bridge_model_assembly_us = u128_to_u64(frame.prepare_trace.bridge_model_projection_us);
        let list_projection_us = u128_to_u64(frame.custom_draw_trace.arcade_list_update_us);
        let custom_draw_us = duration_us(frame.custom_draw_start, frame.custom_draw_done);
        Self {
            bridge_model_assembly_us,
            bridge_updates_us: bridge_total_us.saturating_sub(bridge_model_assembly_us),
            list_projection_us,
            slint_raster_us: duration_us(frame.frame_t1, frame.frame_t2),
            overlay_composition_us: custom_draw_us.saturating_sub(list_projection_us),
            latch_copy_us: u128_to_u64(frame.main_present_hidden_copy_us),
            post_us: u128_to_u64(frame.main_present_request_us),
            confirmation_wait_wall_us: u128_to_u64(frame.post_present_wait_us),
            confirmation_poll_cpu_us: frame.main_present_completion_poll_cpu_us,
        }
    }

    fn json(self) -> serde_json::Value {
        serde_json::json!({
            "clock_domain": "CLOCK_MONOTONIC",
            "bridge_model_assembly": self.bridge_model_assembly_us,
            "bridge_updates": self.bridge_updates_us,
            "list_projection": self.list_projection_us,
            "slint_raster": self.slint_raster_us,
            "overlay_composition": self.overlay_composition_us,
            "latch_copy": self.latch_copy_us,
            "post": self.post_us,
            "confirmation_wait_wall": self.confirmation_wait_wall_us,
            "confirmation_poll_cpu": self.confirmation_poll_cpu_us,
        })
    }
}

fn duration_us(start: Instant, end: Instant) -> u64 {
    end.saturating_duration_since(start)
        .as_micros()
        .min(u128::from(u64::MAX)) as u64
}

fn u128_to_u64(value: u128) -> u64 {
    value.min(u128::from(u64::MAX)) as u64
}

struct PendingCollectionEntry {
    collection_id: String,
    requested_at: Instant,
    source: launcher::HomeViewState,
    open_game_list_directly: bool,
}

struct PendingNavigationTransition {
    event: launcher::LauncherEvent,
    source_state: launcher::NavigationTransitionState,
    source_was_arcade: bool,
    committed: bool,
    status_quiesce_started_at: Option<Instant>,
}

const NAVIGATION_STATUS_QUIESCE_LIMIT: Duration = Duration::from_millis(50);

fn system_entry_preview_terminal(
    selected_has_preview: bool,
    preview_state: &str,
    terminal_empty: bool,
) -> bool {
    if selected_has_preview {
        preview_state == "exact"
    } else {
        terminal_empty
    }
}

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

fn full_screen_transition_owns_cpu1(state: FullScreenTransitionState) -> bool {
    state != FullScreenTransitionState::Live
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

fn navigation_transition_for_intent(
    nav: &LauncherNav,
    event: &launcher::LauncherEvent,
) -> Option<(NavigationTransitionEdge, NavigationTransitionDirection)> {
    use crate::launcher_taxonomy::ROOT_MENU_ID;

    match event.action {
        LauncherAction::OpenMenu => Some((
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
        )),
        LauncherAction::OpenCollection if nav.current_menu_id() == ROOT_MENU_ID => Some((
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
        )),
        LauncherAction::OpenCollection => Some((
            NavigationTransitionEdge::ConsolesToSystem,
            NavigationTransitionDirection::Forward,
        )),
        LauncherAction::NavigateBack if nav.screen == Screen::Home => Some((
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Reverse,
        )),
        LauncherAction::NavigateBack
            if nav.screen == Screen::Arcade && nav.current_menu_id() == ROOT_MENU_ID =>
        {
            Some((
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionDirection::Reverse,
            ))
        }
        LauncherAction::NavigateBack if nav.screen == Screen::Arcade => Some((
            NavigationTransitionEdge::ConsolesToSystem,
            NavigationTransitionDirection::Reverse,
        )),
        LauncherAction::NavigateHome if nav.screen == Screen::Home => Some((
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Reverse,
        )),
        LauncherAction::NavigateHome
            if nav.screen == Screen::Arcade && nav.current_menu_id() == ROOT_MENU_ID =>
        {
            Some((
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionDirection::Reverse,
            ))
        }
        LauncherAction::NavigateHome if nav.screen == Screen::Arcade => Some((
            NavigationTransitionEdge::ConsolesToSystem,
            NavigationTransitionDirection::Reverse,
        )),
        _ => None,
    }
}

fn settings_page_transition(
    source: Screen,
    destination: Screen,
) -> Option<(NavigationTransitionRoute, NavigationTransitionDirection)> {
    let source_depth = settings_page_depth(source)?;
    let destination_depth = settings_page_depth(destination)?;
    let route = match (source, destination) {
        (Screen::Home, Screen::Settings) | (Screen::Settings, Screen::Home) => {
            Some(NavigationTransitionRoute::HomeToSettings)
        }
        (Screen::Settings, Screen::Screensaver) | (Screen::Screensaver, Screen::Settings) => {
            Some(NavigationTransitionRoute::SettingsToScreensaver)
        }
        (Screen::Settings, Screen::About) | (Screen::About, Screen::Settings) => {
            Some(NavigationTransitionRoute::SettingsToAbout)
        }
        (Screen::About, Screen::Info) | (Screen::Info, Screen::About) => {
            Some(NavigationTransitionRoute::AboutToInfo)
        }
        (Screen::About, Screen::Licenses) | (Screen::Licenses, Screen::About) => {
            Some(NavigationTransitionRoute::AboutToLicenses)
        }
        (source, Screen::Home) if source != Screen::Home => {
            Some(NavigationTransitionRoute::NestedToHome)
        }
        _ => None,
    }?;
    let adjacent = matches!(
        (source, destination),
        (Screen::Home, Screen::Settings)
            | (Screen::Settings, Screen::Home)
            | (Screen::Settings, Screen::Screensaver | Screen::About)
            | (Screen::Screensaver | Screen::About, Screen::Settings)
            | (Screen::About, Screen::Info | Screen::Licenses)
            | (Screen::Info | Screen::Licenses, Screen::About)
    );
    let direct_home = source != Screen::Home && destination == Screen::Home;
    (adjacent || direct_home).then_some((
        route,
        if destination_depth > source_depth {
            NavigationTransitionDirection::Forward
        } else {
            NavigationTransitionDirection::Reverse
        },
    ))
}

const fn settings_page_depth(screen: Screen) -> Option<u8> {
    match screen {
        Screen::Home => Some(0),
        Screen::Settings => Some(1),
        Screen::Screensaver | Screen::About => Some(2),
        Screen::Info | Screen::Licenses => Some(3),
        Screen::Controller | Screen::Arcade | Screen::SystemHub => None,
    }
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
        Screen::Settings
        | Screen::Screensaver
        | Screen::About
        | Screen::Info
        | Screen::Licenses => activated || backed || went_home,
        Screen::Controller | Screen::Arcade | Screen::SystemHub => false,
    }
}

fn route_lifecycle_dialog_input(
    event: Option<&crate::input_event::InputEvent>,
    launch_failure_visible: bool,
    recovery_dialog_visible: bool,
) -> Option<LauncherLifecycleInput> {
    let event = event.filter(|event| event.phase == crate::input_event::InputPhase::Pressed)?;
    let input = if launch_failure_visible {
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
    };
    input
}

fn sync_navigation_transition_active(
    app: &slint_ui::launcher::Launcher,
    transition: &NavigationTransitionRuntime,
) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let active = transition.is_active();
    if bridge.get_navigation_transition_active() != active {
        bridge.set_navigation_transition_active(active);
    }
}

fn begin_navigation_full_screen_transition(
    chart: &mut FullScreenTransitionStateChart,
    generation: &mut Option<FullScreenTransitionGeneration>,
) -> bool {
    match chart.begin(FullScreenTransitionOwner::Navigation) {
        Ok(started) => {
            *generation = Some(started);
            true
        }
        Err(error) => {
            crate::ui_errln!("navigation full-screen transition begin rejected: {error:?}");
            false
        }
    }
}

fn release_full_screen_transition(
    chart: &mut FullScreenTransitionStateChart,
    generation: Option<FullScreenTransitionGeneration>,
) {
    if let Some(generation) = generation
        && let Err(error) = chart.release(generation)
    {
        crate::ui_errln!("navigation full-screen transition release rejected: {error:?}");
    }
}

fn collection_has_resident_rows(catalog: &ArcadeCatalog, collection_id: &str) -> bool {
    catalog.system_game_count(collection_id) > 0
}

fn system_entry_collection_id(system_id: &str) -> &str {
    if system_id == "arcade" {
        arcade_catalog::MENU_ARCADE_SYSTEM_ID
    } else {
        system_id
    }
}

fn empty_collection_invariant_violated(catalog: &ArcadeCatalog, nav: &LauncherNav) -> bool {
    nav.screen == Screen::Arcade
        && active_system(catalog, nav).is_some_and(|system| {
            system.count > 0 && !collection_has_resident_rows(catalog, &system.id)
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
    if entry.open_game_list_directly && nav.screen == Screen::SystemHub {
        nav.set_arcade_user_list_mode(catalog, launcher::ArcadeUserListMode::Games);
        nav.screen = Screen::Arcade;
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

impl ArcadeEntryLatencyTracker {
    fn from_config(config: &mister_magik_fb::process_config::LauncherEntryTraceConfig) -> Self {
        Self {
            trace: ArcadeEntryLatencyTrace::from_config(config),
            enter_input_at: None,
            destination_prepared: false,
            enter_presented: false,
            rows_ready: false,
            preview_exact: false,
            ready_presented: false,
            first_nav_input_at: None,
            first_nav_presented: false,
            catalog_resident_at_input: None,
            presentation_start: None,
        }
    }

    fn input_enabled_ms(lifecycle: &LauncherLifecycle) -> u64 {
        lifecycle.startup_status().input_enabled_ms
    }

    fn cancel_enter(&mut self) {
        self.enter_input_at = None;
        self.destination_prepared = false;
        self.enter_presented = false;
        self.rows_ready = false;
        self.preview_exact = false;
        self.ready_presented = false;
        self.first_nav_input_at = None;
        self.first_nav_presented = false;
        self.catalog_resident_at_input = None;
        self.presentation_start = None;
    }

    fn capture_presentation_start(
        &mut self,
        telemetry: Option<mister_magik_latch_contract::PresentationTelemetry>,
        latch_drop_count: u16,
    ) {
        self.presentation_start = telemetry.map(|telemetry| SystemEntryPresentationStart {
            telemetry,
            latch_drop_count,
        });
    }

    fn preview_adoption_in_progress(&self) -> bool {
        self.enter_input_at.is_some() && self.rows_ready && !self.ready_presented
    }

    fn active_system_id(catalog: &ArcadeCatalog, nav: &LauncherNav) -> String {
        active_system(catalog, nav)
            .map(|system| system.legacy_system_id.clone())
            .unwrap_or_default()
    }

    fn selected_asset_key(catalog: &ArcadeCatalog, nav: &LauncherNav) -> String {
        active_system(catalog, nav)
            .and_then(|system| nav.active_arcade_game_at(catalog, &system.id, nav.arcade.selected))
            .map(|game| game.preview_asset_key.to_string())
            .unwrap_or_default()
    }

    fn record_enter_input(
        &mut self,
        start: Instant,
        at: Instant,
        lifecycle: &LauncherLifecycle,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
    ) {
        if self.enter_input_at.is_some() {
            return;
        }
        self.enter_input_at = Some(at);
        self.catalog_resident_at_input = Some(true);
        let system = Self::active_system_id(catalog, nav);
        let asset_key = Self::selected_asset_key(catalog, nav);
        self.trace.record(
            start,
            "arcade_enter_input",
            at,
            None,
            Self::input_enabled_ms(lifecycle),
            true,
            &system,
            nav.arcade.selected,
            None,
            None,
            "",
            &asset_key,
            "source=launcher_input",
        );
    }

    fn record_collection_enter_input(
        &mut self,
        start: Instant,
        at: Instant,
        lifecycle: &LauncherLifecycle,
        collection_id: &str,
        source: &'static str,
        catalog_resident: bool,
    ) {
        if self.enter_input_at.is_some() {
            return;
        }
        self.enter_input_at = Some(at);
        self.catalog_resident_at_input = Some(catalog_resident);
        self.trace.record(
            start,
            "arcade_enter_input",
            at,
            None,
            Self::input_enabled_ms(lifecycle),
            true,
            collection_id,
            0,
            None,
            None,
            "",
            "",
            format!(
                "source={source} catalog_resident={}",
                u8::from(catalog_resident)
            ),
        );
    }

    fn record_first_nav_input(
        &mut self,
        start: Instant,
        at: Instant,
        lifecycle: &LauncherLifecycle,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
    ) {
        if self.enter_input_at.is_none() || self.first_nav_input_at.is_some() {
            return;
        }
        self.first_nav_input_at = Some(at);
        let system = Self::active_system_id(catalog, nav);
        let asset_key = Self::selected_asset_key(catalog, nav);
        self.trace.record(
            start,
            "arcade_first_nav_input",
            at,
            self.enter_input_at,
            Self::input_enabled_ms(lifecycle),
            true,
            &system,
            nav.arcade.selected,
            None,
            None,
            "",
            &asset_key,
            "source=launcher_input",
        );
    }

    fn record_rows_ready(
        &mut self,
        start: Instant,
        at: Instant,
        lifecycle: &LauncherLifecycle,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
    ) {
        if self.enter_input_at.is_none() || self.rows_ready {
            return;
        }
        self.rows_ready = true;
        let system = Self::active_system_id(catalog, nav);
        let asset_key = Self::selected_asset_key(catalog, nav);
        self.trace.record(
            start,
            "arcade_rows_ready",
            at,
            self.enter_input_at,
            Self::input_enabled_ms(lifecycle),
            true,
            &system,
            nav.arcade.selected,
            None,
            None,
            "",
            &asset_key,
            format!(
                "games={} catalog_resident_at_input={}",
                catalog.system_game_count(&system),
                u8::from(self.catalog_resident_at_input.unwrap_or(false))
            ),
        );
    }

    fn record_preview_exact(
        &mut self,
        start: Instant,
        at: Instant,
        lifecycle: &LauncherLifecycle,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
        preview: &PreviewState,
    ) {
        if self.enter_input_at.is_none() || self.preview_exact || !self.rows_ready {
            return;
        }
        let preview_state = preview.trace_cache_state();
        let selected_has_preview = selected_arcade_game_has_preview(nav, catalog);
        if !system_entry_preview_terminal(
            selected_has_preview,
            preview_state,
            preview.terminal_empty(),
        ) {
            return;
        }
        self.preview_exact = true;
        let system = Self::active_system_id(catalog, nav);
        let asset_key = Self::selected_asset_key(catalog, nav);
        let timing = if selected_has_preview {
            preview.selected_preview_timing()
        } else {
            crate::preview_state::SelectedPreviewTiming::default()
        };
        self.trace.record(
            start,
            "arcade_preview_exact",
            at,
            self.enter_input_at,
            Self::input_enabled_ms(lifecycle),
            true,
            &system,
            nav.arcade.selected,
            None,
            None,
            preview_state,
            &asset_key,
            format!(
                "source=preview_state selected_has_preview={} provenance={} request_age_us={} read_us={} decode_us={} raw565_parse_us={} resize_us={} total_us={} encoded_bytes={} decoded_bytes={}",
                u8::from(selected_has_preview),
                if selected_has_preview {
                    preview.visible_preview_load_source()
                } else {
                    "terminal-empty"
                },
                timing.request_age_us,
                timing.read_us,
                timing.decode_us,
                timing.raw565_parse_us,
                timing.resize_us,
                timing.total_us,
                timing.encoded_bytes,
                timing.decoded_bytes,
            ),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_destination_prepared_frame(
        &mut self,
        start: Instant,
        at: Instant,
        lifecycle: &LauncherLifecycle,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
        preview: &PreviewState,
        frame: u64,
        prepare_us: u128,
        copied_rows: u32,
        catalog_generation: usize,
    ) {
        if !system_entry_destination_frame_eligible(
            self.enter_input_at.is_some(),
            self.rows_ready,
            self.preview_exact,
            self.destination_prepared,
            nav.screen,
            copied_rows,
        ) {
            return;
        }
        self.destination_prepared = true;
        let system = Self::active_system_id(catalog, nav);
        let asset_key = Self::selected_asset_key(catalog, nav);
        self.trace.record(
            start,
            "system_entry_destination_prepared",
            at,
            self.enter_input_at,
            Self::input_enabled_ms(lifecycle),
            true,
            &system,
            nav.arcade.selected,
            Some(frame),
            Some(prepare_us),
            preview.trace_cache_state(),
            &asset_key,
            format!(
                "copied_rows={copied_rows} catalog_generation={} preview_generation={}",
                catalog_generation,
                preview.presentation_generation(),
            ),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_presented_frame(
        &mut self,
        start: Instant,
        at: Instant,
        lifecycle: &LauncherLifecycle,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
        preview: &PreviewState,
        frame: u64,
        prepare_us: u128,
        copied_rows: u32,
    ) {
        if self.enter_input_at.is_none() || copied_rows == 0 || nav.screen != Screen::Arcade {
            return;
        }
        let system = Self::active_system_id(catalog, nav);
        let asset_key = Self::selected_asset_key(catalog, nav);
        if !self.enter_presented {
            self.enter_presented = true;
            self.trace.record(
                start,
                "arcade_enter_presented",
                at,
                self.enter_input_at,
                Self::input_enabled_ms(lifecycle),
                true,
                &system,
                nav.arcade.selected,
                Some(frame),
                Some(prepare_us),
                preview.trace_cache_state(),
                &asset_key,
                format!("copied_rows={copied_rows}"),
            );
        }
        if self.first_nav_input_at.is_some() && !self.first_nav_presented {
            self.first_nav_presented = true;
            self.trace.record(
                start,
                "arcade_first_nav_presented",
                at,
                self.first_nav_input_at,
                Self::input_enabled_ms(lifecycle),
                true,
                &system,
                nav.arcade.selected,
                Some(frame),
                Some(prepare_us),
                preview.trace_cache_state(),
                &asset_key,
                format!("copied_rows={copied_rows}"),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_ready_presented_frame(
        &mut self,
        start: Instant,
        at: Instant,
        lifecycle: &LauncherLifecycle,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
        preview: &PreviewState,
        frame: u64,
        prepare_us: u128,
        copied_rows: u32,
        main_active_confirmed: bool,
        catalog_generation: usize,
        main_sequence: u16,
        presentation_end: Option<mister_magik_latch_contract::PresentationTelemetry>,
        latch_drop_count: u16,
        publication: SystemEntryPublicationPhases,
    ) -> bool {
        if !self.destination_prepared
            || !system_entry_ready_frame_eligible(
                self.enter_input_at.is_some(),
                self.rows_ready,
                self.preview_exact,
                self.ready_presented,
                nav.screen,
                copied_rows,
                main_active_confirmed,
            )
        {
            return false;
        }
        self.ready_presented = true;
        let system = Self::active_system_id(catalog, nav);
        let asset_key = Self::selected_asset_key(catalog, nav);
        let selected_has_preview = selected_arcade_game_has_preview(nav, catalog);
        let cadence = self
            .presentation_start
            .zip(presentation_end)
            .filter(|(start, end)| {
                start.telemetry.magik_ownership()
                    && end.magik_ownership()
                    && start.telemetry.lifetime_invariant_valid()
                    && end.lifetime_invariant_valid()
            })
            .map(|(start, end)| {
                (
                    end.repeated_vblank_count
                        .wrapping_sub(start.telemetry.repeated_vblank_count),
                    latch_drop_count.wrapping_sub(start.latch_drop_count),
                )
            });
        // Publish the benchmark-owned phase record before the ready marker. The host treats
        // that marker as the point at which every correlated artifact is complete.
        write_system_entry_publication_profile(self.trace.profile_path.as_deref(), publication);
        self.trace.record(
            start,
            "system_entry_ready_presented",
            at,
            self.enter_input_at,
            Self::input_enabled_ms(lifecycle),
            true,
            &system,
            nav.arcade.selected,
            Some(frame),
            Some(prepare_us),
            preview.trace_cache_state(),
            &asset_key,
            format!(
                "copied_rows={copied_rows} confirmation=main-active-sequence selected_has_preview={} catalog_generation={} preview_generation={} main_sequence={} cadence_authoritative={} repeated_vblank_delta={} latch_drop_delta={} bridge_model_assembly_us={} bridge_updates_us={} list_projection_us={} slint_raster_us={} overlay_composition_us={} latch_copy_us={} post_us={} confirmation_wait_wall_us={} confirmation_poll_cpu_us={}",
                u8::from(selected_has_preview),
                catalog_generation,
                preview.presentation_generation(),
                main_sequence,
                u8::from(cadence.is_some()),
                cadence
                    .map(|(repeated, _)| repeated.to_string())
                    .unwrap_or_else(|| "unavailable".to_string()),
                cadence
                    .map(|(_, dropped)| dropped.to_string())
                    .unwrap_or_else(|| "unavailable".to_string()),
                publication.bridge_model_assembly_us,
                publication.bridge_updates_us,
                publication.list_projection_us,
                publication.slint_raster_us,
                publication.overlay_composition_us,
                publication.latch_copy_us,
                publication.post_us,
                publication.confirmation_wait_wall_us,
                publication.confirmation_poll_cpu_us,
            ),
        );
        true
    }
}

fn write_system_entry_publication_profile(
    path: Option<&str>,
    publication: SystemEntryPublicationPhases,
) {
    let Some(path) = path else {
        return;
    };
    let path = std::path::Path::new(path);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(mut evidence) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    evidence["first_frame_publication_us"] = publication.json();
    if let Err(error) = std::fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&evidence).unwrap_or_default()
        ),
    ) {
        crate::ui_errln!("system-entry publication profile write failed: {error}");
    }
}

fn system_entry_destination_frame_eligible(
    entered: bool,
    rows_ready: bool,
    preview_terminal: bool,
    already_recorded: bool,
    screen: Screen,
    copied_rows: u32,
) -> bool {
    entered
        && rows_ready
        && preview_terminal
        && !already_recorded
        && screen == Screen::Arcade
        && copied_rows > 0
}

fn system_entry_ready_frame_eligible(
    entered: bool,
    rows_ready: bool,
    preview_terminal: bool,
    already_recorded: bool,
    screen: Screen,
    copied_rows: u32,
    main_active_confirmed: bool,
) -> bool {
    entered
        && rows_ready
        && preview_terminal
        && !already_recorded
        && screen == Screen::Arcade
        && copied_rows > 0
        && main_active_confirmed
}

fn should_defer_arcade_overlay_bridge(
    dirty_opt: bool,
    launching: bool,
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
) -> bool {
    dirty_opt
        && !launching
        && nav.screen == Screen::Arcade
        && !nav.arcade_search.is_active(&nav.arcade_filter.active)
        && !active_system_game_view(catalog, nav).is_empty()
}

struct LauncherStatusTextSnapshot {
    catalog_scan_message: SharedString,
    catalog_scan_title: SharedString,
    catalog_scan_detail: SharedString,
    confirm_title: SharedString,
    confirm_message: SharedString,
    confirm_left_label: SharedString,
    confirm_right_label: SharedString,
}

impl LauncherStatusTextSnapshot {
    fn from_bridge(bridge: &slint_ui::launcher::MisterBridge<'_>) -> Self {
        Self {
            catalog_scan_message: bridge.get_catalog_scan_message(),
            catalog_scan_title: bridge.get_catalog_scan_title(),
            catalog_scan_detail: bridge.get_catalog_scan_detail(),
            confirm_title: bridge.get_confirm_title(),
            confirm_message: bridge.get_confirm_message(),
            confirm_left_label: bridge.get_confirm_left_label(),
            confirm_right_label: bridge.get_confirm_right_label(),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LibraryChangedDialogTestPhase {
    Waiting,
    ContinueReleaseA,
    RebuildReleaseRight,
    RebuildPressA,
    RebuildReleaseA,
    Done,
}

struct LibraryChangedDialogTestDriver {
    choice: Option<launcher::LibraryChangedTestDialogChoice>,
    dialog_seen_at: Option<Instant>,
    phase: LibraryChangedDialogTestPhase,
    next_sequence: u64,
    next_press_id: u64,
    active_press: Option<(
        crate::input_event::LogicalAction,
        crate::input_event::PressId,
    )>,
}

const INPUT_INTEGRITY_TRACE_PATH: &str = "/tmp/mister-magik/input-integrity-trace.json";
const INPUT_INTEGRITY_TRACE_LIMIT: usize = 512;
const LAUNCHER_RESPONSE_TRACE_PATH: &str = "/tmp/mister-magik/launcher-response-trace.json";
const LAUNCHER_RESPONSE_PARTIAL_FLUSH_INTERVAL: Duration = Duration::from_secs(1);
const LAUNCHER_RESPONSE_TRACE_LIMIT: usize = 256;

struct InputIntegrityTrace {
    enabled: bool,
    records: VecDeque<serde_json::Value>,
    initial_presses: u64,
    releases: u64,
    repeats: u64,
    queue_high_water: usize,
    dispatch_latencies_us: Vec<u64>,
    dirty: bool,
    last_write: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LauncherResponseState {
    screen: String,
    menu_id: String,
    selected_item_id: String,
    selected_index: usize,
    arcade_visual_index_milli: Option<i64>,
}

impl LauncherResponseState {
    fn capture(nav: &LauncherNav) -> Self {
        let feedback_target = nav_selection_feedback_target(nav);
        Self {
            screen: screen_label(nav.screen).to_string(),
            menu_id: feedback_target
                .as_ref()
                .map(|target| target.surface.clone())
                .unwrap_or_else(|| nav.current_menu_id().to_string()),
            selected_item_id: feedback_target
                .map(|target| target.item)
                .unwrap_or_else(|| nav.current_menu_selected_item_id().to_string()),
            selected_index: match nav.screen {
                Screen::Arcade => nav.arcade.selected,
                Screen::SystemHub => nav.system_hub_selected,
                Screen::Settings => nav.settings_selected,
                Screen::Screensaver => nav.screensaver_selected,
                Screen::About => nav.about_selected,
                Screen::Licenses => nav.licenses_selected,
                _ => nav.selected,
            },
            arcade_visual_index_milli: (nav.screen == Screen::Arcade)
                .then(|| (f64::from(nav.arcade.visual_index) * 1_000.0).round() as i64),
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "screen": self.screen,
            "menu_id": self.menu_id,
            "selected_item_id": self.selected_item_id,
            "selected_index": self.selected_index,
            "arcade_visual_index_milli": self.arcade_visual_index_milli,
        })
    }

    fn matches_presented(&self, before: &Self, presented: &Self) -> bool {
        if self.screen != "arcade" {
            return self == presented;
        }
        self.screen == presented.screen
            && self.selected_index == presented.selected_index
            && before.arcade_visual_index_milli != presented.arcade_visual_index_milli
    }
}

#[derive(Clone)]
struct LauncherResponseFeedbackRecord {
    phase: &'static str,
    event_id: u64,
    surface: String,
    item: String,
    confirmed_at_us: u64,
    confirmed_frame: u64,
    confirmed_sequence: u16,
    dwell_us: Option<u64>,
}

#[derive(Clone, Copy)]
struct LauncherResponsePresentationSnapshot {
    owned_vblank_count: u32,
    presented_vblank_count: u32,
    repeated_vblank_count: u32,
    ownership_loss_count: u32,
    latch_drop_count: u32,
    magik_ownership: bool,
}

#[derive(Clone)]
struct LauncherResponseRecord {
    action: LogicalAction,
    trigger: DispatchKind,
    press_id: u64,
    proxy_sequence: Option<u32>,
    proxy_kernel_at_us: Option<u64>,
    input_reader: Option<crate::input_hub::InputReaderEventEvidence>,
    captured_at_us: u64,
    published_at_us: Option<u64>,
    drained_at_us: Option<u64>,
    drained_execution: Option<ThreadExecutionStamp>,
    dispatch_at_us: u64,
    dispatch_execution: Option<ThreadExecutionStamp>,
    state_applied_at_us: Option<u64>,
    state_applied_execution: Option<ThreadExecutionStamp>,
    before: LauncherResponseState,
    after: Option<LauncherResponseState>,
    disposition: &'static str,
    frame: Option<LauncherResponseFrameEvidence>,
    confirmed_at_us: Option<u64>,
    confirmed_execution: Option<ThreadExecutionStamp>,
    confirmed_frame: Option<u64>,
    confirmed_sequence: Option<u16>,
}

#[derive(Clone)]
struct LauncherResponseFrameEvidence {
    selected: LauncherResponseState,
    projected_at_us: u64,
    projected_execution: Option<ThreadExecutionStamp>,
    raster_started_at_us: u64,
    raster_started_execution: Option<ThreadExecutionStamp>,
    raster_completed_at_us: u64,
    raster_completed_execution: Option<ThreadExecutionStamp>,
    slint_damage_rects: Vec<(usize, usize, usize, usize)>,
    post_accepted_at_us: u64,
    post_accepted_execution: Option<ThreadExecutionStamp>,
    dirty_rect: Option<(usize, usize, usize, usize)>,
    present_bytes: usize,
    wasted_present_bytes: usize,
    cached_present_us: u64,
    hidden_compose_us: u64,
    hidden_copy_us: u64,
    hidden_publish_us: u64,
    hidden_invalid_bytes: usize,
    hidden_rect_count: u32,
    hidden_catchup_bytes: usize,
    hidden_full_copy: bool,
    hidden_copy_path: &'static str,
    present_request_us: u64,
    set_vga_fb_us: u64,
    present_wait_us: u64,
    posted_sequence: u16,
    post_active_sequence: u16,
    post_pending_sequence: u16,
    post_pending: bool,
    first_eligible_vblank: Option<bool>,
}

#[derive(Clone)]
struct LauncherResponseFrameStamp {
    record_index: usize,
    selected: LauncherResponseState,
    projected_at_us: u64,
    projected_execution: Option<ThreadExecutionStamp>,
    raster_started_at_us: u64,
    raster_started_execution: Option<ThreadExecutionStamp>,
    raster_completed_at_us: u64,
    raster_completed_execution: Option<ThreadExecutionStamp>,
    slint_damage_rects: Vec<(usize, usize, usize, usize)>,
}

#[derive(Clone, Copy, Default)]
struct LauncherResponsePresentReceipt {
    post_accepted_at_us: u64,
    post_accepted_execution: Option<ThreadExecutionStamp>,
    dirty_rect: Option<(usize, usize, usize, usize)>,
    present_bytes: usize,
    wasted_present_bytes: usize,
    cached_present_us: u64,
    hidden_compose_us: u64,
    hidden_copy_us: u64,
    hidden_publish_us: u64,
    hidden_invalid_bytes: usize,
    hidden_rect_count: u32,
    hidden_catchup_bytes: usize,
    hidden_full_copy: bool,
    hidden_copy_path: &'static str,
    present_request_us: u64,
    set_vga_fb_us: u64,
    present_wait_us: u64,
    posted_sequence: u16,
    post_active_sequence: u16,
    post_pending_sequence: u16,
    post_pending: bool,
    refresh_period_us: u64,
}

struct LauncherResponseTrace {
    enabled: bool,
    execution_enabled: bool,
    pmu_enabled: bool,
    pmu_active: bool,
    completion_path: Option<String>,
    pmu_completion_path: Option<String>,
    system_entry_profile_path: Option<String>,
    records: Vec<LauncherResponseRecord>,
    feedback_records: Vec<LauncherResponseFeedbackRecord>,
    pending_dispatches: VecDeque<usize>,
    pending_confirmations: VecDeque<usize>,
    published_at_us: HashMap<u64, u64>,
    proxy_sequences: HashMap<u64, u32>,
    proxy_kernel_at_us: HashMap<u64, u64>,
    input_reader: HashMap<u64, crate::input_hub::InputReaderEventEvidence>,
    input_reader_policy: Option<mister_magik_catalog::runtime_thread::RuntimeThreadPolicyReport>,
    drained_at_us: HashMap<u64, u64>,
    drained_execution: HashMap<u64, ThreadExecutionStamp>,
    state: LauncherResponseState,
    queue_high_water: usize,
    refresh_period_us: u64,
    presentation_start: Option<LauncherResponsePresentationSnapshot>,
    presentation_end: Option<LauncherResponsePresentationSnapshot>,
    run_id: String,
    expected_confirmed: usize,
    expected_feedback_hidden: usize,
    hidden_feedback_count: usize,
    cancelled_feedback_count: usize,
    outstanding_feedback: HashSet<u64>,
    complete: bool,
    frame_trace_finalize_pending: bool,
    writer: Option<Sender<LauncherResponseTraceWrite>>,
    input_probe: Option<crate::input_hub::InputObservationProbe>,
    catalog_phases: Vec<serde_json::Value>,
    scheduler_phases: Vec<serde_json::Value>,
    lab_records: Vec<serde_json::Value>,
    last_partial_flush_at: Instant,
    partial_confirmed_sent: usize,
    partial_feedback_sent: usize,
    partial_lab_sent: usize,
    dirty: bool,
}

fn launcher_response_u64(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn launcher_response_execution_interval(
    started: Option<ThreadExecutionStamp>,
    finished: Option<ThreadExecutionStamp>,
    started_at_us: Option<u64>,
    finished_at_us: Option<u64>,
) -> Option<serde_json::Value> {
    started
        .zip(finished)
        .zip(started_at_us.zip(finished_at_us))
        .map(|((started, finished), (started_at, finished_at))| {
            started.interval_json(finished, finished_at.saturating_sub(started_at))
        })
}

struct LauncherResponseTraceWrite {
    snapshot: LauncherResponseTraceSnapshot,
    completion_path: Option<String>,
}

struct LauncherResponseTraceSnapshot {
    records: Vec<LauncherResponseRecord>,
    feedback_records: Vec<LauncherResponseFeedbackRecord>,
    refresh_period_us: u64,
    presentation_start: Option<LauncherResponsePresentationSnapshot>,
    presentation_end: Option<LauncherResponsePresentationSnapshot>,
    run_id: String,
    expected_confirmed: usize,
    expected_feedback_hidden: usize,
    hidden_feedback_count: usize,
    cancelled_feedback_count: usize,
    outstanding_feedback_count: usize,
    complete: bool,
    execution_enabled: bool,
    queue_high_water: usize,
    catalog_phases: Vec<serde_json::Value>,
    scheduler_phases: Vec<serde_json::Value>,
    lab_records: Vec<serde_json::Value>,
    input_reader_policy: Option<mister_magik_catalog::runtime_thread::RuntimeThreadPolicyReport>,
}

struct LauncherResponseCatalogPhaseStart {
    label: &'static str,
    started_at_us: u64,
    input_generation: Option<u64>,
}

#[derive(Clone, Copy, Default)]
struct LauncherResponseSchedulerBoundary {
    wall_at_us: u64,
    input_generation: Option<u64>,
    execution: Option<ThreadExecutionStamp>,
}

impl LauncherResponseTrace {
    fn from_config(
        config: &mister_magik_fb::process_config::LauncherResponseTraceConfig,
        entry_config: &mister_magik_fb::process_config::LauncherEntryTraceConfig,
        nav: &LauncherNav,
        input_probe: Option<crate::input_hub::InputObservationProbe>,
    ) -> Self {
        let enabled = config.enabled();
        let execution_enabled = config.execution_enabled();
        let pmu_enabled = config.pmu_enabled();
        let pmu_completion_path = response_trace_volatile_path(config.pmu_completion_path());
        let run_id = config.run_id().to_owned();
        let expected_confirmed = config.expected_confirmed();
        let expected_feedback_hidden = config.expected_feedback_hidden();
        let completion_path = response_trace_volatile_path(config.completion_path());
        if enabled {
            let _ = std::fs::remove_file(LAUNCHER_RESPONSE_TRACE_PATH);
            for path in [
                config.completion_path(),
                config.frame_completion_path(),
                config.pmu_completion_path(),
            ] {
                if let Some(path) = response_trace_volatile_path(path) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        if pmu_enabled {
            mister_magik_perf_events::clear_process_profiles();
        }
        let writer = enabled.then(|| spawn_launcher_response_trace_writer(completion_path.clone()));
        Self {
            enabled,
            execution_enabled,
            pmu_enabled,
            pmu_active: false,
            completion_path,
            pmu_completion_path,
            system_entry_profile_path: entry_config.profile_path().map(str::to_owned),
            records: Vec::with_capacity(LAUNCHER_RESPONSE_TRACE_LIMIT),
            feedback_records: Vec::with_capacity(LAUNCHER_RESPONSE_TRACE_LIMIT),
            pending_dispatches: VecDeque::new(),
            pending_confirmations: VecDeque::new(),
            published_at_us: HashMap::new(),
            proxy_sequences: HashMap::new(),
            proxy_kernel_at_us: HashMap::new(),
            input_reader: HashMap::new(),
            input_reader_policy: None,
            drained_at_us: HashMap::new(),
            drained_execution: HashMap::new(),
            state: LauncherResponseState::capture(nav),
            queue_high_water: 0,
            refresh_period_us: 0,
            presentation_start: None,
            presentation_end: None,
            run_id,
            expected_confirmed,
            expected_feedback_hidden,
            hidden_feedback_count: 0,
            cancelled_feedback_count: 0,
            outstanding_feedback: HashSet::new(),
            complete: false,
            frame_trace_finalize_pending: false,
            writer,
            input_probe,
            catalog_phases: Vec::new(),
            scheduler_phases: Vec::new(),
            lab_records: Vec::new(),
            last_partial_flush_at: Instant::now(),
            partial_confirmed_sent: 0,
            partial_feedback_sent: 0,
            partial_lab_sent: 0,
            dirty: enabled,
        }
    }

    #[cfg(test)]
    fn enabled_for_test(nav: &LauncherNav) -> Self {
        Self {
            enabled: true,
            execution_enabled: false,
            pmu_enabled: false,
            pmu_active: false,
            completion_path: None,
            pmu_completion_path: None,
            system_entry_profile_path: None,
            records: Vec::with_capacity(LAUNCHER_RESPONSE_TRACE_LIMIT),
            feedback_records: Vec::with_capacity(LAUNCHER_RESPONSE_TRACE_LIMIT),
            pending_dispatches: VecDeque::new(),
            pending_confirmations: VecDeque::new(),
            published_at_us: HashMap::new(),
            proxy_sequences: HashMap::new(),
            proxy_kernel_at_us: HashMap::new(),
            input_reader: HashMap::new(),
            input_reader_policy: None,
            drained_at_us: HashMap::new(),
            drained_execution: HashMap::new(),
            state: LauncherResponseState::capture(nav),
            queue_high_water: 0,
            refresh_period_us: 0,
            presentation_start: None,
            presentation_end: None,
            run_id: "test-run".to_string(),
            expected_confirmed: 0,
            expected_feedback_hidden: 0,
            hidden_feedback_count: 0,
            cancelled_feedback_count: 0,
            outstanding_feedback: HashSet::new(),
            complete: false,
            frame_trace_finalize_pending: false,
            writer: None,
            input_probe: None,
            catalog_phases: Vec::new(),
            scheduler_phases: Vec::new(),
            lab_records: Vec::new(),
            last_partial_flush_at: Instant::now()
                .checked_sub(LAUNCHER_RESPONSE_PARTIAL_FLUSH_INTERVAL)
                .unwrap_or_else(Instant::now),
            partial_confirmed_sent: 0,
            partial_feedback_sent: 0,
            partial_lab_sent: 0,
            dirty: false,
        }
    }

    #[cfg(test)]
    fn configured_for_test(
        nav: &LauncherNav,
        expected_confirmed: usize,
        expected_feedback_hidden: usize,
    ) -> Self {
        let mut trace = Self::enabled_for_test(nav);
        trace.expected_confirmed = expected_confirmed;
        trace.expected_feedback_hidden = expected_feedback_hidden;
        trace
    }

    #[cfg(test)]
    fn enable_execution_for_test(&mut self) {
        self.execution_enabled = true;
    }

    fn execution_stamp(&self) -> Option<ThreadExecutionStamp> {
        self.execution_enabled.then(ThreadExecutionStamp::capture)
    }

    fn input_pmu_span(
        &self,
        relevant: bool,
        name: &'static str,
    ) -> Option<mister_magik_perf_events::SampledSpan> {
        (self.pmu_active && relevant)
            .then(|| mister_magik_perf_events::sampled_span(name))
            .flatten()
    }

    fn observe_drained_input(&mut self, drained: &crate::input_hub::DrainedInput) {
        if self.enabled {
            let batch = &drained.batch;
            self.queue_high_water = self.queue_high_water.max(batch.health.queue_high_water);
            let drained_at_us = crate::input_hub::monotonic_us();
            let drained_execution = self.execution_stamp();
            for publication in &drained.publications {
                self.published_at_us
                    .insert(publication.sequence, publication.published_at_us);
                if let Some(proxy_sequence) = publication.proxy_sequence {
                    self.proxy_sequences
                        .insert(publication.sequence, proxy_sequence);
                }
                if let Some(proxy_kernel_at_us) = publication.proxy_kernel_at_us {
                    self.proxy_kernel_at_us
                        .insert(publication.sequence, proxy_kernel_at_us);
                }
                if let Some(reader) = publication.reader {
                    self.input_reader.insert(publication.sequence, reader);
                }
            }
            self.input_reader_policy.clone_from(&drained.reader_policy);
            for event in &batch.events {
                self.drained_at_us.insert(event.sequence, drained_at_us);
                if let Some(execution) = drained_execution {
                    self.drained_execution.insert(event.sequence, execution);
                }
            }
        }
    }

    fn record_route(&mut self, event: crate::input_event::InputEvent, outcome: InputOutcome) {
        if !self.enabled
            || event.source.kind != InputSourceKind::MainProxy
            || event.phase != InputPhase::Pressed
            || self.records.len() == LAUNCHER_RESPONSE_TRACE_LIMIT
        {
            return;
        }
        let dispatch_at_us = crate::input_hub::monotonic_us();
        let dispatch_execution = self.execution_stamp();
        let (trigger, disposition) = match outcome {
            InputOutcome::Dispatch { kind, .. } => (kind, "dispatched"),
            InputOutcome::Consumed {
                reason: ConsumedReason::TransitionActive,
                ..
            } => (DispatchKind::Initial, "transition-swallowed"),
            _ => return,
        };
        let index = self.records.len();
        self.records.push(LauncherResponseRecord {
            action: event.action,
            trigger,
            press_id: event.press_id.0,
            proxy_sequence: self.proxy_sequences.remove(&event.sequence),
            proxy_kernel_at_us: self.proxy_kernel_at_us.remove(&event.sequence),
            input_reader: self.input_reader.remove(&event.sequence),
            captured_at_us: event.captured_at_us,
            published_at_us: self.published_at_us.remove(&event.sequence),
            drained_at_us: self.drained_at_us.remove(&event.sequence),
            drained_execution: self.drained_execution.remove(&event.sequence),
            dispatch_at_us,
            dispatch_execution,
            state_applied_at_us: None,
            state_applied_execution: None,
            before: self.state.clone(),
            after: None,
            disposition,
            frame: None,
            confirmed_at_us: None,
            confirmed_execution: None,
            confirmed_frame: None,
            confirmed_sequence: None,
        });
        if disposition == "dispatched" {
            self.pending_dispatches.push_back(index);
        }
        self.dirty = true;
    }

    fn observe_state(&mut self, nav: &LauncherNav, transition_active: bool) {
        if !self.enabled {
            return;
        }
        let state = LauncherResponseState::capture(nav);
        if state != self.state {
            self.state = state.clone();
            if let Some(index) = self.pending_dispatches.pop_front() {
                self.records[index].after = Some(state);
                self.records[index].state_applied_at_us = Some(crate::input_hub::monotonic_us());
                self.records[index].state_applied_execution = self.execution_stamp();
                self.records[index].disposition = "state-changed";
                self.pending_confirmations.push_back(index);
            }
            self.dirty = true;
        } else if !transition_active && let Some(index) = self.pending_dispatches.pop_front() {
            self.records[index].disposition = "no-change";
            self.dirty = true;
        }
    }

    fn frame_stamp(
        &self,
        nav: &LauncherNav,
        projected_at_us: u64,
        projected_execution: Option<ThreadExecutionStamp>,
        raster_started_at_us: u64,
        raster_started_execution: Option<ThreadExecutionStamp>,
        raster_completed_at_us: u64,
        raster_completed_execution: Option<ThreadExecutionStamp>,
    ) -> Option<LauncherResponseFrameStamp> {
        if !self.enabled {
            return None;
        }
        let state = LauncherResponseState::capture(nav);
        let Some(position) = self.pending_confirmations.iter().rposition(|index| {
            let record = &self.records[*index];
            record
                .state_applied_at_us
                .is_some_and(|applied_at_us| applied_at_us <= projected_at_us)
                && record
                    .after
                    .as_ref()
                    .is_some_and(|after| after.matches_presented(&record.before, &state))
        }) else {
            return None;
        };
        Some(LauncherResponseFrameStamp {
            record_index: self.pending_confirmations[position],
            selected: state,
            projected_at_us,
            projected_execution,
            raster_started_at_us,
            raster_started_execution,
            raster_completed_at_us,
            raster_completed_execution,
            slint_damage_rects: Vec::new(),
        })
    }

    fn confirm(
        &mut self,
        stamp: Option<&LauncherResponseFrameStamp>,
        receipt: LauncherResponsePresentReceipt,
        frame: u64,
        sequence: u16,
    ) {
        if !self.enabled {
            return;
        }
        let Some(stamp) = stamp else {
            return;
        };
        let Some(position) = self
            .pending_confirmations
            .iter()
            .position(|index| *index == stamp.record_index)
        else {
            return;
        };
        let index = self
            .pending_confirmations
            .remove(position)
            .expect("stamped response index");
        let confirmed_at_us = crate::input_hub::monotonic_us();
        let confirmed_execution = self.execution_stamp();
        let first_eligible_vblank = (receipt.refresh_period_us > 0).then(|| {
            confirmed_at_us.saturating_sub(receipt.post_accepted_at_us)
                <= receipt.refresh_period_us.saturating_add(3_000)
        });
        let record = &mut self.records[index];
        record.disposition = "confirmed";
        record.frame = Some(LauncherResponseFrameEvidence {
            selected: stamp.selected.clone(),
            projected_at_us: stamp.projected_at_us,
            projected_execution: stamp.projected_execution,
            raster_started_at_us: stamp.raster_started_at_us,
            raster_started_execution: stamp.raster_started_execution,
            raster_completed_at_us: stamp.raster_completed_at_us,
            raster_completed_execution: stamp.raster_completed_execution,
            slint_damage_rects: stamp.slint_damage_rects.clone(),
            post_accepted_at_us: receipt.post_accepted_at_us,
            post_accepted_execution: receipt.post_accepted_execution,
            dirty_rect: receipt.dirty_rect,
            present_bytes: receipt.present_bytes,
            wasted_present_bytes: receipt.wasted_present_bytes,
            cached_present_us: receipt.cached_present_us,
            hidden_compose_us: receipt.hidden_compose_us,
            hidden_copy_us: receipt.hidden_copy_us,
            hidden_publish_us: receipt.hidden_publish_us,
            hidden_invalid_bytes: receipt.hidden_invalid_bytes,
            hidden_rect_count: receipt.hidden_rect_count,
            hidden_catchup_bytes: receipt.hidden_catchup_bytes,
            hidden_full_copy: receipt.hidden_full_copy,
            hidden_copy_path: receipt.hidden_copy_path,
            present_request_us: receipt.present_request_us,
            set_vga_fb_us: receipt.set_vga_fb_us,
            present_wait_us: receipt.present_wait_us,
            posted_sequence: receipt.posted_sequence,
            post_active_sequence: receipt.post_active_sequence,
            post_pending_sequence: receipt.post_pending_sequence,
            post_pending: receipt.post_pending,
            first_eligible_vblank,
        });
        record.confirmed_at_us = Some(confirmed_at_us);
        record.confirmed_execution = confirmed_execution;
        record.confirmed_frame = Some(frame);
        record.confirmed_sequence = Some(sequence);
        self.update_completion();
        self.dirty = true;
    }

    fn record_feedback_confirmation(
        &mut self,
        confirmation: &crate::launcher_presentation::SelectionFeedbackConfirmation,
        frame: u64,
        sequence: u16,
    ) {
        if !self.enabled || self.feedback_records.len() == LAUNCHER_RESPONSE_TRACE_LIMIT {
            return;
        }
        let (phase, event_id, target, dwell_us) = match confirmation {
            crate::launcher_presentation::SelectionFeedbackConfirmation::Visible {
                event_id,
                target,
                ..
            } => {
                self.outstanding_feedback.insert(*event_id);
                ("visible", *event_id, target, None)
            }
            crate::launcher_presentation::SelectionFeedbackConfirmation::Hidden {
                event_id,
                target,
                visible_for,
                ..
            } => {
                self.outstanding_feedback.remove(event_id);
                self.hidden_feedback_count = self.hidden_feedback_count.saturating_add(1);
                (
                    "hidden",
                    *event_id,
                    target,
                    Some(u64::try_from(visible_for.as_micros()).unwrap_or(u64::MAX)),
                )
            }
            crate::launcher_presentation::SelectionFeedbackConfirmation::Cancelled {
                event_id,
                target,
                ..
            } => {
                self.outstanding_feedback.remove(event_id);
                self.cancelled_feedback_count = self.cancelled_feedback_count.saturating_add(1);
                ("cancelled", *event_id, target, None)
            }
        };
        self.feedback_records.push(LauncherResponseFeedbackRecord {
            phase,
            event_id,
            surface: target.surface.clone(),
            item: target.item.clone(),
            confirmed_at_us: crate::input_hub::monotonic_us(),
            confirmed_frame: frame,
            confirmed_sequence: sequence,
            dwell_us,
        });
        self.update_completion();
        self.dirty = true;
    }

    fn begin_catalog_phase(
        &self,
        label: &'static str,
    ) -> Option<LauncherResponseCatalogPhaseStart> {
        self.enabled.then(|| LauncherResponseCatalogPhaseStart {
            label,
            started_at_us: crate::input_hub::monotonic_us(),
            input_generation: self
                .input_probe
                .as_ref()
                .map(|probe| probe.observe().generation()),
        })
    }

    fn catalog_boundary(&self) -> (u64, Option<u64>) {
        (
            crate::input_hub::monotonic_us(),
            self.input_probe
                .as_ref()
                .map(|probe| probe.observe().generation()),
        )
    }

    fn scheduler_boundary(&self) -> LauncherResponseSchedulerBoundary {
        if self.enabled {
            LauncherResponseSchedulerBoundary {
                wall_at_us: crate::input_hub::monotonic_us(),
                input_generation: self
                    .input_probe
                    .as_ref()
                    .map(|probe| probe.observe().generation()),
                execution: self.execution_stamp(),
            }
        } else {
            LauncherResponseSchedulerBoundary::default()
        }
    }

    fn record_scheduler_interval(
        &mut self,
        label: &'static str,
        start: LauncherResponseSchedulerBoundary,
    ) -> LauncherResponseSchedulerBoundary {
        let end = self.scheduler_boundary();
        let duration_us = end.wall_at_us.saturating_sub(start.wall_at_us);
        let input_changed_during = start
            .input_generation
            .zip(end.input_generation)
            .is_some_and(|(before, after)| before != after);
        if self.enabled && (input_changed_during || duration_us >= 20_000) {
            let execution = start
                .execution
                .zip(end.execution)
                .map(|(before, after)| before.interval_json(after, duration_us));
            self.scheduler_phases.push(serde_json::json!({
                "label": label,
                "started_at_us": start.wall_at_us,
                "completed_at_us": end.wall_at_us,
                "duration_us": duration_us,
                "input_generation_before": start.input_generation,
                "input_generation_after": end.input_generation,
                "input_changed_during": input_changed_during,
                "execution": execution,
            }));
            self.dirty = true;
        }
        end
    }

    fn record_catalog_interval(
        &mut self,
        label: &'static str,
        start: (u64, Option<u64>),
        end: (u64, Option<u64>),
        measured_duration_us: u128,
    ) {
        self.catalog_phases.push(serde_json::json!({
            "label": label,
            "started_at_us": start.0,
            "completed_at_us": end.0,
            "duration_us": end.0.saturating_sub(start.0),
            "measured_duration_us": measured_duration_us.min(u128::from(u64::MAX)) as u64,
            "input_generation_before": start.1,
            "input_generation_after": end.1,
            "input_changed_during": start.1.zip(end.1)
                .is_some_and(|(before, after)| before != after),
        }));
        self.dirty = true;
    }

    fn end_catalog_phase(&mut self, phase: Option<LauncherResponseCatalogPhaseStart>) {
        let Some(phase) = phase else {
            return;
        };
        let completed_at_us = crate::input_hub::monotonic_us();
        let input_generation_after = self
            .input_probe
            .as_ref()
            .map(|probe| probe.observe().generation());
        self.catalog_phases.push(serde_json::json!({
            "label": phase.label,
            "started_at_us": phase.started_at_us,
            "completed_at_us": completed_at_us,
            "duration_us": completed_at_us.saturating_sub(phase.started_at_us),
            "input_generation_before": phase.input_generation,
            "input_generation_after": input_generation_after,
            "input_changed_during": phase.input_generation.zip(input_generation_after)
                .is_some_and(|(before, after)| before != after),
        }));
        self.dirty = true;
    }

    fn record_lab(&mut self, record: Option<serde_json::Value>) {
        if self.enabled
            && let Some(record) = record
        {
            self.lab_records.push(record);
            self.dirty = true;
        }
    }

    fn update_completion(&mut self) {
        if self.complete || self.expected_confirmed == 0 {
            return;
        }
        let confirmed = self
            .records
            .iter()
            .filter(|record| record.disposition == "confirmed")
            .count();
        if confirmed >= self.expected_confirmed
            && self
                .hidden_feedback_count
                .saturating_add(self.cancelled_feedback_count)
                >= self.expected_feedback_hidden
            && self.outstanding_feedback.is_empty()
        {
            self.complete = true;
            self.frame_trace_finalize_pending = true;
        }
    }

    fn take_frame_trace_finalize_pending(&mut self) -> bool {
        std::mem::take(&mut self.frame_trace_finalize_pending)
    }

    fn launcher_profile_start_ready(&self) -> bool {
        if !self.enabled {
            return false;
        }
        let mut confirmed = self
            .records
            .iter()
            .filter(|record| record.disposition == "confirmed");
        confirmed.next().is_some_and(|record| {
            confirmed.next().is_none()
                && record.after.as_ref().is_some_and(|state| {
                    state.menu_id == "menu:computers"
                        && state.selected_item_id == "menu:computers:acorn"
                })
        })
    }

    fn start_pmu_if_ready(&mut self) {
        if self.pmu_enabled && self.launcher_profile_start_ready() {
            self.pmu_active = true;
        }
    }

    fn finish_pmu(&mut self) -> Result<(), String> {
        if !self.pmu_enabled {
            return Ok(());
        }
        self.pmu_active = false;
        let profile = mister_magik_perf_events::take_thread_profile();
        let passed = profile.enabled
            && profile.failure.is_none()
            && profile.dropped_spans == 0
            && !profile.records.is_empty();
        let payload = serde_json::json!({
            "schema": "mister-magik-launcher-response-pmu-v1",
            "run_id": self.run_id,
            "state": if passed { "complete" } else { "failed" },
            "profile": profile,
        });
        let path = self
            .pmu_completion_path
            .as_deref()
            .ok_or_else(|| "MISTER_LAUNCHER_RESPONSE_PMU_COMPLETE is missing".to_owned())?;
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(path, format!("{payload}\n")).map_err(|error| error.to_string())?;
        if passed {
            Ok(())
        } else {
            Err("launcher response PMU profile is incomplete".into())
        }
    }

    fn observe_presentation(
        &mut self,
        telemetry: mister_magik_latch_contract::PresentationTelemetry,
        refresh_period_us: u64,
        latch_drop_count: u32,
    ) {
        if !self.enabled {
            return;
        }
        let snapshot = LauncherResponsePresentationSnapshot {
            owned_vblank_count: telemetry.owned_vblank_count,
            presented_vblank_count: telemetry.presented_vblank_count,
            repeated_vblank_count: telemetry.repeated_vblank_count,
            ownership_loss_count: telemetry.ownership_loss_count,
            latch_drop_count,
            magik_ownership: telemetry.magik_ownership(),
        };
        self.presentation_start.get_or_insert(snapshot);
        self.presentation_end = Some(snapshot);
        self.refresh_period_us = refresh_period_us;
        self.dirty = true;
    }

    fn snapshot(&self) -> LauncherResponseTraceSnapshot {
        LauncherResponseTraceSnapshot {
            records: self.records.clone(),
            feedback_records: self.feedback_records.clone(),
            refresh_period_us: self.refresh_period_us,
            presentation_start: self.presentation_start,
            presentation_end: self.presentation_end,
            run_id: self.run_id.clone(),
            expected_confirmed: self.expected_confirmed,
            expected_feedback_hidden: self.expected_feedback_hidden,
            hidden_feedback_count: self.hidden_feedback_count,
            cancelled_feedback_count: self.cancelled_feedback_count,
            outstanding_feedback_count: self.outstanding_feedback.len(),
            complete: self.complete,
            execution_enabled: self.execution_enabled,
            queue_high_water: self.queue_high_water,
            catalog_phases: self
                .complete
                .then(|| self.catalog_phases.clone())
                .unwrap_or_default(),
            scheduler_phases: self
                .complete
                .then(|| self.scheduler_phases.clone())
                .unwrap_or_default(),
            lab_records: self
                .complete
                .then(|| self.lab_records.clone())
                .unwrap_or_default(),
            input_reader_policy: self.input_reader_policy.clone(),
        }
    }

    fn partial_snapshot(&self) -> (LauncherResponseTraceSnapshot, usize, usize, usize) {
        let confirmed_count = self
            .records
            .iter()
            .filter(|record| record.disposition == "confirmed")
            .count();
        let records = self
            .records
            .iter()
            .filter(|record| record.disposition == "confirmed")
            .skip(self.partial_confirmed_sent)
            .cloned()
            .collect();
        let feedback_count = self.feedback_records.len();
        let feedback_records = self.feedback_records[self.partial_feedback_sent..].to_vec();
        let lab_count = self.lab_records.len();
        let lab_records = self.lab_records[self.partial_lab_sent..].to_vec();
        (
            LauncherResponseTraceSnapshot {
                records,
                feedback_records,
                refresh_period_us: self.refresh_period_us,
                presentation_start: self.presentation_start,
                presentation_end: self.presentation_end,
                run_id: self.run_id.clone(),
                expected_confirmed: self.expected_confirmed,
                expected_feedback_hidden: self.expected_feedback_hidden,
                hidden_feedback_count: self.hidden_feedback_count,
                cancelled_feedback_count: self.cancelled_feedback_count,
                outstanding_feedback_count: self.outstanding_feedback.len(),
                complete: false,
                execution_enabled: self.execution_enabled,
                queue_high_water: self.queue_high_water,
                catalog_phases: Vec::new(),
                scheduler_phases: Vec::new(),
                lab_records,
                input_reader_policy: self.input_reader_policy.clone(),
            },
            confirmed_count,
            feedback_count,
            lab_count,
        )
    }

    fn flush(&mut self) {
        if !self.enabled || !self.dirty {
            return;
        }
        if !self.complete
            && self.last_partial_flush_at.elapsed() < LAUNCHER_RESPONSE_PARTIAL_FLUSH_INTERVAL
        {
            return;
        }
        let (snapshot, partial_counts) = if self.complete {
            (self.snapshot(), None)
        } else {
            let (snapshot, confirmed_count, feedback_count, lab_count) = self.partial_snapshot();
            (snapshot, Some((confirmed_count, feedback_count, lab_count)))
        };
        if self.writer.as_ref().is_some_and(|writer| {
            writer
                .send(LauncherResponseTraceWrite {
                    snapshot,
                    completion_path: self.completion_path.clone(),
                })
                .is_ok()
        }) || self.writer.is_none()
        {
            if let Some((confirmed_count, feedback_count, lab_count)) = partial_counts {
                self.partial_confirmed_sent = confirmed_count;
                self.partial_feedback_sent = feedback_count;
                self.partial_lab_sent = lab_count;
            }
            self.dirty = false;
            self.last_partial_flush_at = Instant::now();
        }
    }
}

impl LauncherResponseTraceSnapshot {
    fn merge_partial(&mut self, next: Self) {
        debug_assert!(!self.complete && !next.complete);
        debug_assert_eq!(self.run_id, next.run_id);
        debug_assert_eq!(self.execution_enabled, next.execution_enabled);
        self.records.extend(next.records);
        self.feedback_records.extend(next.feedback_records);
        self.lab_records.extend(next.lab_records);
        self.refresh_period_us = next.refresh_period_us;
        self.presentation_start = self.presentation_start.or(next.presentation_start);
        self.presentation_end = next.presentation_end;
        self.expected_confirmed = next.expected_confirmed;
        self.expected_feedback_hidden = next.expected_feedback_hidden;
        self.hidden_feedback_count = next.hidden_feedback_count;
        self.cancelled_feedback_count = next.cancelled_feedback_count;
        self.outstanding_feedback_count = next.outstanding_feedback_count;
        self.queue_high_water = next.queue_high_water;
        if next.input_reader_policy.is_some() {
            self.input_reader_policy = next.input_reader_policy;
        }
    }

    fn payload(&self) -> String {
        let records = self
            .records
            .iter()
            .map(|record| {
                let frame_evidence = record.frame.as_ref().map(|frame| {
                    let slint_damage_rects = frame
                        .slint_damage_rects
                        .iter()
                        .map(|(x0, y0, x1, y1)| {
                            serde_json::json!({
                                "x0": x0,
                                "y0": y0,
                                "x1": x1,
                                "y1": y1,
                            })
                        })
                        .collect::<Vec<_>>();
                    let present_cost = serde_json::json!({
                        "present_bytes": frame.present_bytes,
                        "wasted_present_bytes": frame.wasted_present_bytes,
                        "cached_present_us": frame.cached_present_us,
                        "hidden_compose_us": frame.hidden_compose_us,
                        "hidden_copy_us": frame.hidden_copy_us,
                        "hidden_publish_us": frame.hidden_publish_us,
                        "hidden_invalid_bytes": frame.hidden_invalid_bytes,
                        "hidden_rect_count": frame.hidden_rect_count,
                        "hidden_catchup_bytes": frame.hidden_catchup_bytes,
                        "hidden_full_copy": frame.hidden_full_copy,
                        "hidden_copy_path": frame.hidden_copy_path,
                        "present_request_us": frame.present_request_us,
                        "set_vga_fb_us": frame.set_vga_fb_us,
                        "present_wait_us": frame.present_wait_us,
                    });
                    let execution = self.execution_enabled.then(|| serde_json::json!({
                        "stamps": {
                            "projected": frame.projected_execution.map(ThreadExecutionStamp::json),
                            "raster_started": frame.raster_started_execution.map(ThreadExecutionStamp::json),
                            "raster_completed": frame.raster_completed_execution.map(ThreadExecutionStamp::json),
                            "post_accepted": frame.post_accepted_execution.map(ThreadExecutionStamp::json),
                        },
                        "intervals": {
                            "projection_to_raster": launcher_response_execution_interval(
                                frame.projected_execution,
                                frame.raster_started_execution,
                                Some(frame.projected_at_us),
                                Some(frame.raster_started_at_us),
                            ),
                            "raster": launcher_response_execution_interval(
                                frame.raster_started_execution,
                                frame.raster_completed_execution,
                                Some(frame.raster_started_at_us),
                                Some(frame.raster_completed_at_us),
                            ),
                            "raster_to_post": launcher_response_execution_interval(
                                frame.raster_completed_execution,
                                frame.post_accepted_execution,
                                Some(frame.raster_completed_at_us),
                                Some(frame.post_accepted_at_us),
                            ),
                        },
                    }));
                    serde_json::json!({
                        "selected": frame.selected.json(),
                        "projected_at_us": frame.projected_at_us,
                        "raster_started_at_us": frame.raster_started_at_us,
                        "raster_completed_at_us": frame.raster_completed_at_us,
                        "slint_damage_rects": slint_damage_rects,
                        "post_accepted_at_us": frame.post_accepted_at_us,
                        "dirty_rect": frame.dirty_rect.map(|(x0, y0, x1, y1)| serde_json::json!({
                            "x0": x0,
                            "y0": y0,
                            "x1": x1,
                            "y1": y1,
                        })),
                        "present_cost": present_cost,
                        "posted_sequence": frame.posted_sequence,
                        "post_active_sequence": frame.post_active_sequence,
                        "post_pending_sequence": frame.post_pending_sequence,
                        "post_pending": frame.post_pending,
                        "first_eligible_vblank": frame.first_eligible_vblank,
                        "execution": execution,
                    })
                });
                let execution = self.execution_enabled.then(|| serde_json::json!({
                    "stamps": {
                        "drained": record.drained_execution.map(ThreadExecutionStamp::json),
                        "dispatched": record.dispatch_execution.map(ThreadExecutionStamp::json),
                        "state_applied": record.state_applied_execution.map(ThreadExecutionStamp::json),
                        "confirmed": record.confirmed_execution.map(ThreadExecutionStamp::json),
                    },
                    "intervals": {
                        "drain_to_dispatch": launcher_response_execution_interval(
                            record.drained_execution,
                            record.dispatch_execution,
                            record.drained_at_us,
                            Some(record.dispatch_at_us),
                        ),
                        "dispatch_to_state": launcher_response_execution_interval(
                            record.dispatch_execution,
                            record.state_applied_execution,
                            Some(record.dispatch_at_us),
                            record.state_applied_at_us,
                        ),
                        "state_to_projection": record.frame.as_ref().and_then(|frame| {
                            launcher_response_execution_interval(
                                record.state_applied_execution,
                                frame.projected_execution,
                                record.state_applied_at_us,
                                Some(frame.projected_at_us),
                            )
                        }),
                        "post_to_confirmation": record.frame.as_ref().and_then(|frame| {
                            launcher_response_execution_interval(
                                frame.post_accepted_execution,
                                record.confirmed_execution,
                                Some(frame.post_accepted_at_us),
                                record.confirmed_at_us,
                            )
                        }),
                    },
                }));
                serde_json::json!({
                    "action": format!("{:?}", record.action).to_ascii_lowercase(),
                    "trigger": match record.trigger {
                        DispatchKind::Initial => "initial",
                        DispatchKind::Repeat => "repeat",
                    },
                    "press_id": record.press_id,
                    "proxy_sequence": record.proxy_sequence,
                    "proxy_kernel_at_us": record.proxy_kernel_at_us,
                    "input_reader": record.input_reader.map(|reader| serde_json::json!({
                        "poll_returned_at_us": reader.poll_returned_at_us,
                        "poll_thread_cpu_us": reader.poll_thread_cpu_us,
                        "poll_cpu": reader.poll_cpu,
                        "read_started_at_us": reader.read_started_at_us,
                        "captured_thread_cpu_us": reader.captured_thread_cpu_us,
                        "captured_cpu": reader.captured_cpu,
                        "poll_runtime_delta_us": reader.poll_runtime_delta_us,
                        "poll_run_delay_delta_us": reader.poll_run_delay_delta_us,
                        "poll_timeslice_delta": reader.poll_timeslice_delta,
                    })),
                    "captured_at_us": record.captured_at_us,
                    "published_at_us": record.published_at_us,
                    "drained_at_us": record.drained_at_us,
                    "dispatch_at_us": record.dispatch_at_us,
                    "capture_to_publish_us": record.published_at_us.map(|at| at.saturating_sub(record.captured_at_us)),
                    "publish_to_drain_us": record.published_at_us.zip(record.drained_at_us).map(|(published, drained)| drained.saturating_sub(published)),
                    "dispatch_latency_us": record.dispatch_at_us.saturating_sub(record.captured_at_us),
                    "state_applied_at_us": record.state_applied_at_us,
                    "before": record.before.json(),
                    "after": record.after.as_ref().map(LauncherResponseState::json),
                    "disposition": record.disposition,
                    "frame": frame_evidence,
                    "confirmed_at_us": record.confirmed_at_us,
                    "confirmed_latency_us": record.confirmed_at_us.map(|at| at.saturating_sub(record.captured_at_us)),
                    "confirmed_frame": record.confirmed_frame,
                    "confirmed_sequence": record.confirmed_sequence,
                    "execution": execution,
                })
            })
            .collect::<Vec<_>>();
        let feedback_records = self
            .feedback_records
            .iter()
            .map(|record| {
                serde_json::json!({
                    "phase": record.phase,
                    "event_id": record.event_id,
                    "surface": record.surface,
                    "item": record.item,
                    "confirmed_at_us": record.confirmed_at_us,
                    "confirmed_frame": record.confirmed_frame,
                    "confirmed_sequence": record.confirmed_sequence,
                    "dwell_us": record.dwell_us,
                })
            })
            .collect::<Vec<_>>();
        let presentation = self
            .presentation_start
            .zip(self.presentation_end)
            .map(|(start, end)| {
                serde_json::json!({
                    "source": "fpga-owned-vblank-telemetry",
                    "refresh_period_us": self.refresh_period_us,
                    "start": {
                        "owned_vblank_count": start.owned_vblank_count,
                        "presented_vblank_count": start.presented_vblank_count,
                        "repeated_vblank_count": start.repeated_vblank_count,
                        "ownership_loss_count": start.ownership_loss_count,
                        "latch_drop_count": start.latch_drop_count,
                        "magik_ownership": start.magik_ownership,
                    },
                    "end": {
                        "owned_vblank_count": end.owned_vblank_count,
                        "presented_vblank_count": end.presented_vblank_count,
                        "repeated_vblank_count": end.repeated_vblank_count,
                        "ownership_loss_count": end.ownership_loss_count,
                        "latch_drop_count": end.latch_drop_count,
                        "magik_ownership": end.magik_ownership,
                    },
                    "repeated_vblank_delta": end.repeated_vblank_count.wrapping_sub(start.repeated_vblank_count),
                    "ownership_loss_delta": end.ownership_loss_count.wrapping_sub(start.ownership_loss_count),
                    "latch_drop_delta": end.latch_drop_count.wrapping_sub(start.latch_drop_count),
                })
            });
        let build_identity = crate::build_identity::BuildIdentity::current();
        format!(
            "{}\n",
            serde_json::json!({
                "schema": "mister-magik-launcher-response-trace-v6",
                "run_id": self.run_id,
                "completion": {
                    "state": if self.complete { "complete" } else { "running" },
                    "expected_confirmed": self.expected_confirmed,
                    "expected_feedback_hidden": self.expected_feedback_hidden,
                    "confirmed": self.records.iter().filter(|record| record.disposition == "confirmed").count(),
                    "feedback_hidden": self.hidden_feedback_count,
                    "feedback_cancelled": self.cancelled_feedback_count,
                    "outstanding_feedback": self.outstanding_feedback_count,
                },
                "runtime": build_identity,
                "latch_protocol": 5,
                "queue_high_water": self.queue_high_water,
                "execution_attribution": {
                    "enabled": self.execution_enabled,
                    "source": "clock-thread-cputime-getrusage-thread-sched-getcpu",
                    "on_cpu_tolerance_us": 250,
                },
                "input_reader_policy": self.input_reader_policy.as_ref().map(|policy| serde_json::json!({
                    "role": policy.role,
                    "intended_nice": policy.intended_nice,
                    "actual_nice": policy.actual_nice,
                    "intended_affinity": policy.intended_affinity,
                    "allowed_cpus": policy.allowed_cpus,
                    "processor": policy.processor,
                    "scheduler_policy": policy.scheduler_policy,
                    "scheduler_priority": policy.scheduler_priority,
                    "thread_id": policy.thread_id,
                    "nice_status": policy.nice_status,
                    "affinity_status": policy.affinity_status,
                    "intended_scheduler": policy.intended_scheduler,
                    "scheduler_status": policy.scheduler_status,
                })),
                "records": records,
                "feedback_records": feedback_records,
                "catalog_phases": &self.catalog_phases,
                "scheduler_phases": &self.scheduler_phases,
                "lab_records": &self.lab_records,
                "presentation": presentation,
            })
        )
    }
}

fn response_trace_volatile_path(path: Option<&str>) -> Option<String> {
    path.filter(|path| path.starts_with("/tmp/") && path.len() > "/tmp/".len())
        .map(str::to_owned)
}

fn spawn_launcher_response_trace_writer(
    completion_path: Option<String>,
) -> Sender<LauncherResponseTraceWrite> {
    let (sender, receiver) = channel::<LauncherResponseTraceWrite>();
    let _ = std::thread::Builder::new()
        .name("launcher-response-trace".to_string())
        .spawn(move || {
            let trace_path = Path::new(LAUNCHER_RESPONSE_TRACE_PATH);
            let temporary_path = trace_path.with_extension("json.pending");
            let mut partial_snapshot: Option<LauncherResponseTraceSnapshot> = None;
            if let Some(parent) = trace_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            while let Ok(write) = receiver.recv() {
                let LauncherResponseTraceWrite {
                    snapshot,
                    completion_path: write_completion_path,
                } = write;
                let complete = snapshot.complete;
                if complete {
                    partial_snapshot = Some(snapshot);
                } else if let Some(accumulated) = partial_snapshot.as_mut() {
                    accumulated.merge_partial(snapshot);
                } else {
                    partial_snapshot = Some(snapshot);
                }
                let wrote = std::fs::write(
                    &temporary_path,
                    partial_snapshot
                        .as_ref()
                        .expect("response trace snapshot")
                        .payload(),
                )
                .and_then(|()| std::fs::rename(&temporary_path, trace_path))
                .is_ok();
                if wrote && complete {
                    if let Some(path) = write_completion_path.or_else(|| completion_path.clone()) {
                        let _ = std::fs::write(path, b"complete\n");
                    }
                }
            }
        });
    sender
}

impl InputIntegrityTrace {
    fn new(enabled: bool, now: Instant) -> Self {
        if enabled {
            let _ = std::fs::remove_file(INPUT_INTEGRITY_TRACE_PATH);
        }
        Self {
            enabled,
            records: VecDeque::new(),
            initial_presses: 0,
            releases: 0,
            repeats: 0,
            queue_high_water: 0,
            dispatch_latencies_us: Vec::new(),
            dirty: enabled,
            last_write: now,
        }
    }

    fn observe_batch(&mut self, batch: &crate::input_event::InputBatch) {
        if !self.enabled {
            return;
        }
        self.queue_high_water = self.queue_high_water.max(batch.health.queue_high_water);
        self.dirty = true;
    }

    fn record_outcome(&mut self, outcome: InputOutcome) {
        if !self.enabled {
            return;
        }
        match outcome {
            InputOutcome::Dispatch { event, kind, .. } => self.record_dispatch(event, kind),
            InputOutcome::Released { event, .. } => self.record_event(event, "release"),
            _ => {}
        }
    }

    fn record_dispatch(&mut self, event: crate::input_event::InputEvent, kind: DispatchKind) {
        if event.source.kind != InputSourceKind::MainProxy {
            return;
        }
        match kind {
            DispatchKind::Initial => self.initial_presses = self.initial_presses.saturating_add(1),
            DispatchKind::Repeat => self.repeats = self.repeats.saturating_add(1),
        }
        let kind = match kind {
            DispatchKind::Initial => "initial",
            DispatchKind::Repeat => "repeat",
        };
        self.record_event(event, kind);
    }

    fn record_event(&mut self, event: crate::input_event::InputEvent, kind: &'static str) {
        if event.source.kind != InputSourceKind::MainProxy {
            return;
        }
        if event.phase == InputPhase::Released {
            self.releases = self.releases.saturating_add(1);
        }
        let dispatch_at_us = crate::input_hub::monotonic_us();
        let dispatch_latency_us = dispatch_at_us.saturating_sub(event.captured_at_us);
        if kind != "repeat" {
            self.dispatch_latencies_us.push(dispatch_latency_us);
        }
        if self.records.len() == INPUT_INTEGRITY_TRACE_LIMIT {
            self.records.pop_front();
        }
        self.records.push_back(serde_json::json!({
            "sequence": event.sequence,
            "press_id": event.press_id.0,
            "source_epoch": event.source_epoch.0,
            "action": format!("{:?}", event.action).to_ascii_lowercase(),
            "phase": match event.phase {
                InputPhase::Pressed => "pressed",
                InputPhase::Released => "released",
            },
            "kind": kind,
            "captured_at_us": event.captured_at_us,
            "dispatch_at_us": dispatch_at_us,
            "dispatch_latency_us": dispatch_latency_us,
        }));
        self.dirty = true;
    }

    fn flush_if_due(&mut self, now: Instant, router: &InputRouter) {
        if !self.enabled
            || !self.dirty
            || now.saturating_duration_since(self.last_write) < Duration::from_millis(50)
        {
            return;
        }
        let mut latencies = self.dispatch_latencies_us.clone();
        latencies.sort_unstable();
        let p99_index = latencies.len().saturating_sub(1) * 99 / 100;
        let payload = serde_json::json!({
            "schema": "mister-magik-input-integrity-trace-v1",
            "initial_presses": self.initial_presses,
            "releases": self.releases,
            "repeats": self.repeats,
            "final_down_held": router.action_held(LogicalAction::Down),
            "queue_high_water": self.queue_high_water,
            "dispatch_p99_us": latencies.get(p99_index).copied().unwrap_or(0),
            "dispatch_max_us": latencies.last().copied().unwrap_or(0),
            "records": self.records,
        });
        if let Some(parent) = std::path::Path::new(INPUT_INTEGRITY_TRACE_PATH).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(INPUT_INTEGRITY_TRACE_PATH, format!("{}\n", payload)).is_ok() {
            self.dirty = false;
            self.last_write = now;
        }
    }
}

impl LibraryChangedDialogTestDriver {
    fn from_config(
        config: &mister_magik_fb::process_config::LauncherTestConfig,
        start: Instant,
    ) -> Self {
        let choice = library_changed_test_dialog_choice_from_value(
            config.library_changed_dialog_choice(),
            start,
        );
        Self {
            choice,
            dialog_seen_at: None,
            phase: LibraryChangedDialogTestPhase::Waiting,
            next_sequence: 0,
            next_press_id: 0,
            active_press: None,
        }
    }

    fn event_for(
        &mut self,
        nav: &LauncherNav,
        now: Instant,
        start: Instant,
    ) -> Option<crate::input_event::InputEvent> {
        let choice = self.choice?;
        if nav.confirm_action != Some(launcher::ConfirmAction::LibraryChanged) {
            self.dialog_seen_at = None;
            return None;
        }
        let seen_at = *self.dialog_seen_at.get_or_insert(now);
        if now.duration_since(seen_at) < LIBRARY_CHANGED_TEST_ACTION_SETTLE {
            return None;
        }

        match choice {
            launcher::LibraryChangedTestDialogChoice::Continue => match self.phase {
                LibraryChangedDialogTestPhase::Waiting => {
                    self.phase = LibraryChangedDialogTestPhase::ContinueReleaseA;
                    print_startup_event(
                        start,
                        "library_changed_test_dialog_input",
                        "choice=continue button=a",
                    );
                    Some(self.press(crate::input_event::LogicalAction::Activate, now, start))
                }
                LibraryChangedDialogTestPhase::ContinueReleaseA => {
                    self.phase = LibraryChangedDialogTestPhase::Done;
                    self.release(now, start)
                }
                _ => None,
            },
            launcher::LibraryChangedTestDialogChoice::Rebuild => match self.phase {
                LibraryChangedDialogTestPhase::Waiting => {
                    self.phase = LibraryChangedDialogTestPhase::RebuildReleaseRight;
                    print_startup_event(
                        start,
                        "library_changed_test_dialog_input",
                        "choice=rebuild button=right",
                    );
                    Some(self.press(crate::input_event::LogicalAction::Right, now, start))
                }
                LibraryChangedDialogTestPhase::RebuildReleaseRight => {
                    self.phase = LibraryChangedDialogTestPhase::RebuildPressA;
                    self.release(now, start)
                }
                LibraryChangedDialogTestPhase::RebuildPressA => {
                    self.phase = LibraryChangedDialogTestPhase::RebuildReleaseA;
                    print_startup_event(
                        start,
                        "library_changed_test_dialog_input",
                        "choice=rebuild button=a",
                    );
                    Some(self.press(crate::input_event::LogicalAction::Activate, now, start))
                }
                LibraryChangedDialogTestPhase::RebuildReleaseA => {
                    self.phase = LibraryChangedDialogTestPhase::Done;
                    self.release(now, start)
                }
                LibraryChangedDialogTestPhase::ContinueReleaseA
                | LibraryChangedDialogTestPhase::Done => None,
            },
        }
    }

    fn press(
        &mut self,
        action: crate::input_event::LogicalAction,
        now: Instant,
        start: Instant,
    ) -> crate::input_event::InputEvent {
        self.next_press_id = self.next_press_id.saturating_add(1).max(1);
        let press_id = crate::input_event::PressId((1_u64 << 60) | self.next_press_id);
        self.active_press = Some((action, press_id));
        self.make_event(
            action,
            press_id,
            crate::input_event::InputPhase::Pressed,
            now,
            start,
        )
    }

    fn release(&mut self, now: Instant, start: Instant) -> Option<crate::input_event::InputEvent> {
        let (action, press_id) = self.active_press.take()?;
        Some(self.make_event(
            action,
            press_id,
            crate::input_event::InputPhase::Released,
            now,
            start,
        ))
    }

    fn make_event(
        &mut self,
        action: crate::input_event::LogicalAction,
        press_id: crate::input_event::PressId,
        phase: crate::input_event::InputPhase,
        now: Instant,
        start: Instant,
    ) -> crate::input_event::InputEvent {
        self.next_sequence = self.next_sequence.saturating_add(1).max(1);
        crate::input_event::InputEvent {
            source: crate::input_event::InputSourceId {
                kind: crate::input_event::InputSourceKind::Automation,
                instance: 4,
            },
            source_epoch: crate::input_event::SourceEpoch(1),
            sequence: self.next_sequence,
            press_id,
            captured_at_us: now
                .saturating_duration_since(start)
                .as_micros()
                .min(u64::MAX as u128) as u64,
            action,
            phase,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherInputScriptButton {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherInputScriptStep {
    Button(LauncherInputScriptButton),
    Wait(usize),
}

impl LauncherInputScriptStep {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(frames) = value
            .strip_prefix("wait:")
            .or_else(|| value.strip_prefix("wait="))
            .and_then(|frames| frames.parse::<usize>().ok())
        {
            return Some(Self::Wait(frames.min(600)));
        }
        LauncherInputScriptButton::parse(value).map(Self::Button)
    }

    fn label(self) -> String {
        match self {
            Self::Button(button) => button.label().to_string(),
            Self::Wait(frames) => format!("wait:{frames}"),
        }
    }
}

impl LauncherInputScriptButton {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "a" => Some(Self::A),
            "b" | "back" => Some(Self::B),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::A => "a",
            Self::B => "b",
        }
    }
}

struct LauncherInputScriptDriver {
    steps: Vec<LauncherInputScriptStep>,
    step_idx: usize,
    frame_in_step: usize,
    wait_frames: usize,
    event_sequence: u64,
    press_sequence: u64,
    active_press: Option<(LauncherInputScriptButton, crate::input_event::PressId)>,
}

impl LauncherInputScriptDriver {
    fn from_config(config: &ScriptedInputConfig, start: Instant) -> Self {
        match config.script() {
            Some(value) => Self::from_script_with_wait_frames(value, start, config.wait_frames()),
            None => Self::empty(),
        }
    }

    #[cfg(test)]
    fn from_script(value: &str, start: Instant) -> Self {
        Self::from_script_with_wait_frames(value, start, 60)
    }

    fn from_script_with_wait_frames(value: &str, start: Instant, wait_frames: usize) -> Self {
        let mut steps = Vec::new();
        for token in value.split([',', ';', ' ']) {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match LauncherInputScriptStep::parse(token) {
                Some(step) => steps.push(step),
                None => print_startup_event(
                    start,
                    "launcher_input_script_invalid_token",
                    format!("token={token}"),
                ),
            }
        }
        if !steps.is_empty() {
            let labels = steps
                .iter()
                .map(|step| step.label())
                .collect::<Vec<_>>()
                .join(",");
            print_startup_event(
                start,
                "launcher_input_script_loaded",
                format!("buttons={labels}"),
            );
        }
        Self {
            steps,
            step_idx: 0,
            frame_in_step: 0,
            wait_frames,
            event_sequence: 0,
            press_sequence: 0,
            active_press: None,
        }
    }

    fn empty() -> Self {
        Self {
            steps: Vec::new(),
            step_idx: 0,
            frame_in_step: 0,
            wait_frames: 0,
            event_sequence: 0,
            press_sequence: 0,
            active_press: None,
        }
    }

    fn event_for(&mut self, captured_at_us: u64) -> Option<crate::input_event::InputEvent> {
        let step = *self.steps.get(self.step_idx)?;
        if self.frame_in_step < self.wait_frames {
            self.frame_in_step += 1;
            return None;
        }
        let local_frame = self.frame_in_step - self.wait_frames;
        self.frame_in_step += 1;
        if let LauncherInputScriptStep::Wait(frames) = step {
            if local_frame >= frames {
                self.step_idx += 1;
                self.frame_in_step = 0;
            }
            return None;
        }
        let LauncherInputScriptStep::Button(button) = step else {
            unreachable!();
        };
        let (phase, press_id) = if local_frame == 0 {
            self.press_sequence = self.press_sequence.saturating_add(1).max(1);
            let press_id = crate::input_event::PressId((1_u64 << 62) | self.press_sequence);
            self.active_press = Some((button, press_id));
            (crate::input_event::InputPhase::Pressed, press_id)
        } else if local_frame == LAUNCHER_INPUT_SCRIPT_PRESS_FRAMES {
            let (_, press_id) = self.active_press.take()?;
            (crate::input_event::InputPhase::Released, press_id)
        } else {
            if local_frame
                >= LAUNCHER_INPUT_SCRIPT_PRESS_FRAMES + LAUNCHER_INPUT_SCRIPT_RELEASE_FRAMES
            {
                self.step_idx += 1;
                self.frame_in_step = 0;
            }
            return None;
        };
        self.event_sequence = self.event_sequence.saturating_add(1).max(1);
        let action = match button {
            LauncherInputScriptButton::Up => crate::input_event::LogicalAction::Up,
            LauncherInputScriptButton::Down => crate::input_event::LogicalAction::Down,
            LauncherInputScriptButton::Left => crate::input_event::LogicalAction::Left,
            LauncherInputScriptButton::Right => crate::input_event::LogicalAction::Right,
            LauncherInputScriptButton::A => crate::input_event::LogicalAction::Activate,
            LauncherInputScriptButton::B => crate::input_event::LogicalAction::Back,
        };
        Some(crate::input_event::InputEvent {
            source: crate::input_event::InputSourceId {
                kind: crate::input_event::InputSourceKind::Automation,
                instance: 2,
            },
            source_epoch: crate::input_event::SourceEpoch(1),
            sequence: self.event_sequence,
            press_id,
            captured_at_us,
            action,
            phase,
        })
    }

    fn active(&self) -> bool {
        self.step_idx < self.steps.len()
    }
}

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
    const BENCHMARK_ACTIVE: Self = Self(1 << 3);
    const SCRIPTED_INPUT_ACTIVE: Self = Self(1 << 4);
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
    const FB0_ROUTE_RECOVERY_PENDING: Self = Self(1 << 24);
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

    #[inline]
    fn bits(self) -> u64 {
        self.0
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

fn launcher_presentation_recovery_wake_reasons(presenter_needs_frame: bool) -> LauncherWakeReasons {
    let mut reasons = LauncherWakeReasons::default();
    reasons.insert_if(
        LauncherWakeReasons::FB0_ROUTE_RECOVERY_PENDING,
        presenter_needs_frame,
    );
    reasons
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

fn launcher_bridge_sync_plan(
    launching: bool,
    _startup_input_enabled: bool,
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

fn launcher_idle_sleep_duration(pacer: &VsyncPacer, work_mode: CatalogWorkMode) -> Duration {
    let frame_period = if work_mode == CatalogWorkMode::DualCoreBurst {
        CATALOG_IDLE_BURST_SLEEP_LIMIT
    } else {
        Duration::from_micros(pacer.period_us().max(1))
    };
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

fn can_preempt_home_latch_wait(
    screen: Screen,
    response_frame_stamped: bool,
    feedback_frame_stamped: bool,
    transition_active: bool,
    screensaver_active: bool,
    direct_layer_state_active: bool,
    preview_commit_pending: bool,
    startup_intro_frame_posted: bool,
) -> bool {
    screen == Screen::Home
        && !response_frame_stamped
        && !feedback_frame_stamped
        && !transition_active
        && !screensaver_active
        && !direct_layer_state_active
        && !preview_commit_pending
        && !startup_intro_frame_posted
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EarlyInputChangeCheckpoint {
    label: &'static str,
    observed_at_us: u64,
}

fn note_early_input_change(
    enabled: bool,
    probe: Option<&crate::input_hub::InputObservationProbe>,
    drained_observation: crate::input_hub::InputObservation,
    checkpoint: &mut Option<EarlyInputChangeCheckpoint>,
    label: &'static str,
) {
    if enabled
        && checkpoint.is_none()
        && probe.is_some_and(|probe| probe.changed_since(drained_observation))
    {
        *checkpoint = Some(EarlyInputChangeCheckpoint {
            label,
            observed_at_us: crate::input_hub::monotonic_us(),
        });
    }
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

fn home_repeat_benchmark_active(scenario: Option<LauncherBenchScenario>) -> bool {
    scenario == Some(LauncherBenchScenario::HomeRepeatHold)
}

#[cfg(test)]
fn catalog_from_summary(
    root: &str,
    summary: &catalog_summary::CatalogSummaryProjection,
) -> ArcadeCatalog {
    let systems = summary
        .systems
        .iter()
        .map(|system| arcade_catalog::GameSystemEntry {
            id: system.id.clone(),
            title: system.title.clone(),
            count: system.count,
        })
        .collect();
    let hot_games = summary
        .hot_games
        .iter()
        .map(arcade_catalog::ArcadeGameEntry::from)
        .collect();
    ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
        PathBuf::from(root),
        hot_games,
        systems,
        Vec::new(),
        summary.platform_kinds(),
    )
}

#[cfg(test)]
fn catalog_from_sharded_registry_and_summary(
    root: &str,
    sharded: ArcadeCatalog,
    summary: &catalog_summary::CatalogSummaryProjection,
) -> ArcadeCatalog {
    let hot_games = if sharded.games.is_empty() {
        summary
            .hot_games
            .iter()
            .map(arcade_catalog::ArcadeGameEntry::from)
            .collect()
    } else {
        sharded.games.iter().cloned().collect()
    };
    ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
        PathBuf::from(root),
        hot_games,
        sharded.systems,
        Vec::new(),
        summary.platform_kinds(),
    )
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
                "catalog_v3_registry_load",
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
                "catalog_v3_registry_load",
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

#[cfg(test)]
fn legacy_summary_seed_needed(capsule_ready: bool, sharded_ready: bool) -> bool {
    !capsule_ready && !sharded_ready
}

#[cfg(test)]
fn read_catalog_summary_seed(
    sqlite_path: &Path,
    summary_path: &Path,
    start: Instant,
) -> Option<catalog_summary::CatalogSummaryProjection> {
    let summary_t = Instant::now();
    if !sqlite_path.exists() {
        print_startup_event(
            start,
            "catalog_summary_load",
            format!(
                "status=sqlite_missing elapsed_us={} sqlite_path={} path={} {}",
                summary_t.elapsed().as_micros(),
                sqlite_path.display(),
                summary_path.display(),
                library_db::catalog_load_counter_detail()
            ),
        );
        return None;
    }
    if !sqlite_file_has_valid_header(sqlite_path) {
        print_startup_event(
            start,
            "catalog_summary_load",
            format!(
                "status=sqlite_unusable elapsed_us={} sqlite_path={} path={} {}",
                summary_t.elapsed().as_micros(),
                sqlite_path.display(),
                summary_path.display(),
                library_db::catalog_load_counter_detail()
            ),
        );
        return None;
    }

    match catalog_summary::read_catalog_summary(summary_path) {
        Ok(Some(summary)) if !summary.systems.is_empty() => {
            if catalog_summary_seed_matches_sqlite(sqlite_path, &summary) {
                print_startup_event(
                    start,
                    "catalog_summary_load",
                    format!(
                        "status=ready systems={} games={} elapsed_us={} path={} {}",
                        summary.systems.len(),
                        summary.total_game_count,
                        summary_t.elapsed().as_micros(),
                        summary_path.display(),
                        library_db::catalog_load_counter_detail()
                    ),
                );
                Some(summary)
            } else {
                print_startup_event(
                    start,
                    "catalog_summary_load",
                    format!(
                        "status=missing_or_stale elapsed_us={} path={} {}",
                        summary_t.elapsed().as_micros(),
                        summary_path.display(),
                        library_db::catalog_load_counter_detail()
                    ),
                );
                None
            }
        }
        Ok(Some(_)) => {
            print_startup_event(
                start,
                "catalog_summary_load",
                format!(
                    "status=empty elapsed_us={} path={} {}",
                    summary_t.elapsed().as_micros(),
                    summary_path.display(),
                    library_db::catalog_load_counter_detail()
                ),
            );
            None
        }
        Ok(None) => {
            print_startup_event(
                start,
                "catalog_summary_load",
                format!(
                    "status=missing_or_stale elapsed_us={} path={} {}",
                    summary_t.elapsed().as_micros(),
                    summary_path.display(),
                    library_db::catalog_load_counter_detail()
                ),
            );
            None
        }
        Err(e) => {
            print_startup_event(
                start,
                "catalog_summary_load_failed",
                format!(
                    "elapsed_us={} path={} error={} {}",
                    summary_t.elapsed().as_micros(),
                    summary_path.display(),
                    e,
                    library_db::catalog_load_counter_detail()
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

    fn mark_durable(&mut self, fingerprint: Option<String>) {
        if fingerprint.is_some() && fingerprint == self.current {
            self.durable = fingerprint;
        }
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
    arcade_entry_latency: &mut ArcadeEntryLatencyTracker,
    lifecycle: &LauncherLifecycle,
    start: Instant,
) -> ColdCollectionEntryStart {
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
    let pending = (hydration_requested || nav.catalog_system_hydration_is_loading(collection_id))
        .then(|| {
            arcade_entry_latency.record_collection_enter_input(
                start,
                requested_at,
                lifecycle,
                collection_id,
                trace_source,
                false,
            );
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

fn catalog_hydration_execution_mode(_request: CatalogWorkerRequest) -> CatalogExecutionMode {
    CatalogExecutionMode::BackgroundInteractive
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

#[cfg(test)]
fn catalog_summary_seed_matches_sqlite(
    sqlite_path: &Path,
    summary: &catalog_summary::CatalogSummaryProjection,
) -> bool {
    let summary_stamp = mister_magik_catalog::catalog_stamp::CatalogStamp::from_lines(
        summary.catalog_stamp_lines.clone(),
    );
    match library_db::read_sqlite_catalog_stamp(sqlite_path) {
        Ok(Some(stored_stamp)) => {
            stored_stamp == summary_stamp
                && summary.catalog_stamp_fingerprint == stored_stamp.fingerprint_hex()
        }
        Ok(None) | Err(_) => false,
    }
}

fn sqlite_file_has_valid_header(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut header = [0u8; SQLITE_HEADER.len()];
    file.read_exact(&mut header).is_ok() && &header == SQLITE_HEADER
}

fn modal_input_test_paths_are_isolated<'a>(paths: impl IntoIterator<Item = &'a Path>) -> bool {
    let root = Path::new(MODAL_INPUT_TEST_ROOT);
    paths
        .into_iter()
        .all(|path| path != root && path.starts_with(root))
}

fn modal_input_catalog_recovery_test_requested(
    config: &mister_magik_fb::process_config::LauncherTestConfig,
    start: Instant,
) -> bool {
    if config.catalog_recovery_dialog() != Some("upgrade") {
        return false;
    }
    let paths = config.modal_path_inputs();
    let isolated =
        paths.len() == 7 && modal_input_test_paths_are_isolated(paths.iter().map(PathBuf::as_path));
    if !isolated {
        print_startup_event(
            start,
            "modal_input_test_rejected",
            "reason=catalog-paths-not-isolated",
        );
    }
    isolated
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
    preview_fade_started: Option<Instant>,
    reactivation_suppressed: bool,
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
            preview_fade_started: None,
            reactivation_suppressed: false,
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
                    self.preview_fade_started = None;
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

    fn set_qualification_particles(
        &mut self,
        now: Instant,
        qualification_enabled: bool,
        particles_requested: bool,
    ) {
        if !qualification_enabled {
            return;
        }
        if particles_requested {
            if !self.active {
                self.start_mode = ScreensaverStartMode::IdleWhenReady;
            }
        } else if self.active || self.start_mode != ScreensaverStartMode::Inactive {
            self.cancel_for_exclusive_view(now);
        }
    }

    fn preview(&mut self, now: Instant) {
        self.active = true;
        self.start_mode = ScreensaverStartMode::Inactive;
        self.preview_active = true;
        self.waiting_for_input_release = true;
        self.last_activity = now;
        self.preview_fade_started = Some(now);
        self.reactivation_suppressed = false;
    }

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
        self.preview_fade_started = None;
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
            self.preview_fade_started = None;
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
        self.preview_fade_started = None;
        self.reactivation_suppressed = true;
        self.last_activity = now;
    }

    fn take_restore_full_frame(&mut self) -> bool {
        std::mem::take(&mut self.restore_full_frame)
    }

    fn preview_fade_alpha(&self, now: Instant) -> Option<u8> {
        const PREVIEW_FADE_DURATION: Duration = Duration::from_millis(200);
        let started = self.preview_fade_started?;
        let elapsed = now.saturating_duration_since(started);
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
        ScreenOrientation::Normal => 0,
        ScreenOrientation::MonitorClockwise => 1,
        ScreenOrientation::MonitorCounterclockwise => 2,
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

fn arm_orientation_confirmation(nav: &mut LauncherNav) {
    nav.confirm_action = Some(launcher::ConfirmAction::ScreenOrientation);
    nav.confirm_selected = 0;
    nav.orientation_confirm_remaining = launcher::DISPLAY_CONFIRM_SECONDS;
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
    navigation_transition: &mut NavigationTransitionRuntime,
    full_screen_transition: &mut FullScreenTransitionStateChart,
    orientation_transition_generation: &mut Option<FullScreenTransitionGeneration>,
    orientation_transition: &mut OrientationTransitionRuntime,
    orientation_transition_intent: &mut Option<OrientationTransitionIntent>,
    orientation_preparation_trace: &mut OrientationPreparationTrace,
    intent: OrientationTransitionIntent,
) -> bool {
    let begin_started = Instant::now();
    let generation = match full_screen_transition.begin(FullScreenTransitionOwner::Orientation) {
        Ok(generation) => generation,
        Err(error) => {
            crate::ui_errln!("orientation full-screen transition begin rejected: {error:?}");
            return false;
        }
    };
    *orientation_transition_generation = Some(generation);
    let source_snapshot_started = Instant::now();
    let animated = orientation_transition.start(from, to, target.cached_565(), now, reduce_motion);
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
        navigation_transition,
    );
    *orientation_preparation_trace = OrientationPreparationTrace {
        begin_us: begin_started.elapsed().as_micros(),
        source_snapshot_us,
        layout_us: layout_started.elapsed().as_micros(),
        source_snapshot_bytes: target.cached_565().len().saturating_mul(2) as u64,
    };
    if animated {
        *orientation_transition_intent = Some(intent);
    } else {
        let _ = orientation_transition.take_completion();
        *orientation_transition_intent = None;
        release_full_screen_transition(full_screen_transition, Some(generation));
    }
    animated
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrientationTransitionIntent {
    Confirm,
    Rollback,
    Benchmark,
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

pub(super) fn run_launcher_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut MappedRgb565Framebuffer,
    f: &mut Fpga,
    display_session: &mut LauncherDisplaySession,
    window: &Rc<MisterSoftwareWindow>,
    target: &mut UiFrameTarget,
    mut pad: PadPool,
    app: slint_ui::launcher::Launcher,
    animation_clock: &AnimationClock,
    process_entry_cpu_profile: Option<cpu_profile::CpuProfiler>,
    launcher_config: mister_magik_fb::process_config::LauncherProcessConfig,
) {
    let start = Instant::now();
    let startup_monotonic_us = monotonic_clock_us().unwrap_or(0);
    let mut frames = 0u64;
    let profile_config = launcher_config.profiles().clone();
    let benchmark_config = launcher_config.benchmark().clone();
    let screensaver_start_mode = launcher_config.screensaver().start_mode();
    let screensaver_preview_waits_for_analytics =
        launcher_config.screensaver().preview_waits_for_analytics();
    let mut screensaver = ScreensaverControl::new(Instant::now(), screensaver_start_mode);
    let mut screensaver_pipeline: Option<ScreensaverRenderAhead> = None;
    let mut retiring_screensaver_pipelines: Vec<ScreensaverRenderAhead> = Vec::new();
    let mut screensaver_loader: Option<LauncherScreensaverLoader> = None;
    let mut screensaver_launcher_frame: Option<Vec<Rgb565Pixel>> = None;
    let mut screensaver_frame_visible = false;
    let mut screensaver_active_cards = 0usize;
    let mut screensaver_render_sequence = 0u64;
    let mut screensaver_starvation_count = 0u64;
    let mut screensaver_show_started: Option<Instant> = None;
    let mut screensaver_first_render_logged = false;
    let mut screensaver_first_present_logged = false;
    let mut screensaver_first_card_present_logged = false;
    let present_backend =
        LauncherPresentBackend::from_config(launcher_config.presentation_backend());
    present_backend.log_if_experimental();
    let mut launcher_presenter = LauncherPresenter::new(ui, present_backend);
    let mut launcher_readiness = super::launcher_readiness::LauncherReadiness::from_process_config(
        launcher_config.readiness().clone(),
    );
    let launcher_bench_scenario = benchmark_config.scenario();
    let orientation_benchmark_enabled =
        launcher_env_flag("MISTER_ORIENTATION_TRANSITIONS_BENCHMARK");
    let settings_navigation_benchmark_enabled =
        launcher_env_flag("MISTER_SETTINGS_NAVIGATION_BENCHMARK");
    let mut settings_navigation_benchmark =
        SettingsNavigationBenchmark::new(settings_navigation_benchmark_enabled);
    let mut settings_navigation_benchmark_completed_at = None;
    let mut settings_navigation_status_baseline = None;
    let orientation_benchmark_effect = std::env::var("MISTER_ORIENTATION_TRANSITION_EFFECT")
        .ok()
        .as_deref()
        .and_then(OrientationTransitionEffect::from_id);
    let mut orientation_benchmark = OrientationTransitionBenchmark::new(
        orientation_benchmark_enabled,
        orientation_benchmark_effect.unwrap_or(OrientationTransitionEffect::BrightnessFade),
    );
    if orientation_benchmark_enabled && orientation_benchmark_effect.is_none() {
        orientation_benchmark.fail("benchmark-effect-is-missing-or-invalid");
    }
    let mut orientation_benchmark_completed_at = None;
    let mut orientation_benchmark_terminal_status_requested = false;
    let orientation_benchmark_requires_analytics =
        launcher_env_flag("MISTER_ORIENTATION_TRANSITIONS_REQUIRE_ANALYTICS");
    let mut latch_v5_qualification =
        LatchV5Qualification::from_config(start, launcher_config.qualification());
    let mut latch_v5_bench_state = LauncherBenchState::default();
    let launcher_bench_after_input_script =
        launcher_bench_scenario.is_some() && benchmark_config.after_input_script();
    let launcher_bench_launch_handoff =
        launcher_bench_scenario == Some(LauncherBenchScenario::LaunchHandoff);
    let mut scheduler = LauncherScheduler::with_runtime_config(
        launcher_bench_launch_handoff,
        launcher_config.catalog_paths().clone(),
        launcher_config.archive_cache().clone(),
        launcher_config.media_worker().clone(),
        benchmark_config
            .launch_return_pmu_handoff_out()
            .map(str::to_owned),
    );
    let mut catalog_events = CatalogJobEventBuf::new();
    let mut deferred_catalog_events: VecDeque<CatalogWorkerMessage> = VecDeque::new();
    let mut pending_catalog_ready: Option<CatalogWorkerMessage> = None;
    let mut pending_collection_entry: Option<PendingCollectionEntry> = None;
    let mut pending_navigation_transition: Option<PendingNavigationTransition> = None;
    let mut deferred_navigation_hydration_finish: Option<String> = None;
    let mut catalog_ready_deferred_since: Option<Instant> = None;
    let mut catalog_ready_stationary_edge_since: Option<Instant> = None;
    let mut media_events = MediaJobEventBuf::new();
    let mut lifecycle_effects = LifecycleEffects::new();
    let mut preview_systems_entered = BTreeSet::new();
    let mut preview_initial_lists_ready = BTreeSet::new();
    let bench_starts_on_arcade = launcher_bench_scenario
        .is_some_and(|scenario| scenario.starts_on_arcade() && !launcher_bench_after_input_script);
    let media_benchmark_contention = media_benchmark_contention_enabled();
    let benchmark_media_interaction_active = benchmark_media_interaction_gate_active(
        launcher_bench_scenario.is_some()
            || orientation_benchmark.enabled()
            || settings_navigation_benchmark.enabled(),
        media_benchmark_contention,
    );
    let env_start_screen = benchmark_config.start_screen();
    let env_start_system = benchmark_config.start_system().map(str::to_owned);
    let system_entry_benchmark_system = benchmark_config.system_entry_system().map(str::to_owned);
    let mut pending_system_entry_benchmark = system_entry_benchmark_system
        .as_deref()
        .map(system_entry_collection_id)
        .map(str::to_string);
    let env_start_menu = launcher_bench_scenario
        .is_some()
        .then(|| benchmark_config.start_menu().map(str::to_owned))
        .flatten();
    let start_screen = orientation_benchmark
        .enabled()
        .then_some(Screen::Settings)
        .or_else(|| {
            settings_navigation_benchmark
                .enabled()
                .then_some(Screen::Home)
        })
        .or_else(|| latch_v5_qualification.enabled().then_some(Screen::Arcade))
        .or_else(|| system_entry_benchmark_system.as_ref().map(|_| Screen::Home))
        .or(env_start_screen)
        .or_else(|| env_start_system.as_ref().map(|_| Screen::Arcade))
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade))
        .unwrap_or(Screen::Home);
    let lock_screen = benchmark_config
        .lock_screen()
        .or_else(|| {
            env_start_system.as_ref().map(|_| {
                env_start_screen
                    .filter(|screen| *screen == Screen::SystemHub)
                    .unwrap_or(Screen::Arcade)
            })
        })
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade));
    let launch_return_restore_allowed = launcher_return_to_launcher_requested()
        && env_start_screen.is_none()
        && system_entry_benchmark_system.is_none()
        && launcher_bench_scenario.is_none()
        && lock_screen.is_none();
    let mut launch_return_session = LaunchReturnSession::new(
        launcher::take_launch_return_state().filter(|_| launch_return_restore_allowed),
    );
    if !launch_return_restore_allowed || !launch_return_session.requested() {
        return_catalog_capsule::remove_return_catalog_capsule();
    }
    let startup_return_requested = launch_return_session.requested();
    let mut launch_return_restored = false;
    let arcade_catalog_required_at_start =
        matches!(start_screen, Screen::Arcade | Screen::SystemHub)
            || matches!(lock_screen, Some(Screen::Arcade | Screen::SystemHub))
            || launcher_bench_after_input_script;
    let mut pending_start_system = env_start_system.clone();
    let mut pending_start_menu = env_start_system
        .is_none()
        .then(|| env_start_menu.clone())
        .flatten();
    let crt_layout = ui.output_route().is_crt();
    let crt_metrics = crate::ui_display::CrtUiMetrics::for_display(ui);
    let preview_route = PreviewRoutePolicy::for_output_route(ui.output_route());
    let mut nav =
        LauncherNav::for_crt_layout_with_row_height(crt_layout, crt_metrics.game_row_height);
    let settings_store =
        FileSettingsStore::new(launcher_config.device_paths().app_path("settings.json"));
    let orientation_store = ConfirmedOrientationStore::for_runtime(settings_store.clone());
    nav.settings = settings_store.load();
    if let Err(error) = orientation_store.reconcile_osd_rotation(nav.settings.screen_orientation) {
        crate::ui_errln!("settings: failed to reconcile MiSTer OSD rotation: {error}");
    }
    let arcade_benchmark_orientation = std::env::var("MISTER_ARCADE_BENCHMARK_ORIENTATION")
        .ok()
        .and_then(|value| ScreenOrientation::parse(&value));
    if let Some(orientation) = arcade_benchmark_orientation {
        nav.settings.screen_orientation = orientation;
    } else if orientation_benchmark.enabled() {
        nav.settings.screen_orientation = ScreenOrientation::Normal;
        nav.settings.reduce_motion = false;
    } else if settings_navigation_benchmark.enabled() {
        nav.settings.screen_orientation = settings_navigation_benchmark.orientation();
        nav.settings.reduce_motion = false;
    }
    let mut layout = UiLayoutGeometry::for_display(ui, nav.settings.screen_orientation);
    let mut layout_epoch = 1_u64;
    let mut preview_compositor = None;
    let mut preview_compositor_start_attempted = false;
    nav.set_portrait_layout(layout.is_portrait());
    if crt_layout {
        nav.set_arcade_row_height(crt_arcade_row_height(
            crt_metrics.game_row_height,
            layout.is_portrait(),
        ));
    }
    nav.sync_orientation_selection();
    let navigation_motion_enabled =
        !nav.settings.reduce_motion || profile_config.cpu().navigation_transition_requested();
    let mut navigation_transition = NavigationTransitionRuntime::new(
        layout.logical_w(),
        layout.logical_h(),
        navigation_motion_enabled,
    );
    let mut full_screen_transition = FullScreenTransitionStateChart::default();
    let mut navigation_transition_generation = None;
    nav.screen = start_screen;
    if orientation_benchmark.enabled() {
        nav.settings_selected = 1;
    }
    let mut display_confirm_deadline = None;
    let mut orientation_confirm_deadline = None;
    let mut orientation_previous = None;
    let mut orientation_full_redraw_pending = layout.is_portrait();
    let mut orientation_transition =
        OrientationTransitionRuntime::new(ui.render_w(), ui.render_h());
    let mut orientation_transition_intent = None;
    let mut orientation_transition_generation = None;
    let mut orientation_preparation_trace = OrientationPreparationTrace::default();
    let (display_confirm_tx, display_confirm_rx) =
        mpsc::channel::<Result<launcher::DisplayCommandState, String>>();
    let (orientation_confirm_tx, orientation_confirm_rx) = mpsc::channel::<Result<(), String>>();
    // Main owns the active display mode; the launcher only mirrors its reported state.
    if std::env::var_os("MISTER_MAGIK_PARENT").is_some() {
        if let Ok(state) = launcher::try_display_state() {
            let selected_id = state.pending.as_deref().unwrap_or(&state.active);
            if let Some(index) =
                mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
                    .iter()
                    .position(|mode| mode.id == selected_id)
            {
                nav.display_selected = index;
                nav.display_highlighted =
                    launcher::settings_display_selection_index(index).unwrap_or(0);
            }
            if state.return_to_settings {
                nav.screen = Screen::Settings;
                nav.settings_selected = 0;
                if let Some(error) = state.error.as_deref() {
                    nav.display_error = Some(format!(
                        "The previous resolution was restored after a display failure: {error}"
                    ));
                    nav.confirm_action = Some(launcher::ConfirmAction::DisplayResolutionError);
                    nav.confirm_selected = 0;
                }
            }
            display_confirm_deadline = apply_startup_pending_display(
                &mut nav,
                &state,
                display_confirmation_ui_enabled(
                    std::env::var_os("MISTER_MAGIK_DISPLAY_CONFIRM_UI").as_deref(),
                ),
                Instant::now(),
            );
        }
    }
    let mut setup = SetupNav::new();
    let mut input_router = InputRouter::new(launcher_input_focus(
        false, false, false, false, false, false, &nav,
    ));
    let mut input_fault_notice: Option<&'static str>;
    let mut setup_disconnect_notice = false;
    let mut input_integrity_stall = launcher_config.input().integrity_stall_ms();
    let mut input_integrity_trace =
        InputIntegrityTrace::new(launcher_config.input().integrity_trace(), Instant::now());
    let input_observation_probe = pad.input_observation_probe();
    let mut launcher_response_trace = LauncherResponseTrace::from_config(
        launcher_config.readiness().response_trace(),
        launcher_config.readiness().entry_trace(),
        &nav,
        input_observation_probe.clone(),
    );
    let mut gui_profiling = GuiProfilingController::from_config(profile_config.gui().clone());
    let mut input_latency_lab = InputLatencyLab::from_env(input_observation_probe.clone());
    let mut loading_title = String::new();
    let mut last_clock_update = Instant::now() - Duration::from_secs(2);
    let mut last_clock_text = launcher_clock_text();
    let mut launcher_bench_next_step: Instant;
    let mut launcher_bench_state = LauncherBenchState::default();
    let mut launcher_bench_active =
        launcher_bench_scenario.is_some() && !launcher_bench_after_input_script;
    let auto_launch_selected = benchmark_config.auto_launch_selected();
    let mut auto_launch_selected_done = false;
    let dirty_opt = launcher_dirty_opt_enabled();
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    crate::ui_logln!(
        "launcher running {label} — {} pad(s), D-pad to move, A to select, Home to go back...",
        pad.len()
    );
    crate::ui_logln!(
        "launcher_mode={} fb_format={}",
        "launcher",
        production_label()
    );
    if let Some(scenario) = launcher_bench_scenario {
        crate::ui_logln!("launcher_bench_scenario={}", scenario.label());
    }
    crate::ui_logln!(
        "launcher_start_screen={} launcher_lock_screen={}",
        screen_label(start_screen),
        lock_screen.map(screen_label).unwrap_or("none")
    );
    if let Some(system_id) = env_start_system.as_ref() {
        crate::ui_logln!("launcher_start_system={system_id}");
    }
    if let Some(menu_id) = env_start_menu.as_ref() {
        crate::ui_logln!("launcher_start_menu={menu_id}");
    }
    crate::ui_logln!(
        "launcher_dirty_opt={}",
        if dirty_opt { "on" } else { "off" }
    );
    boot_analytics::event(
        "launcher_loop_start",
        format!("label={label} pads={}", pad.len()),
    );
    if media_benchmark_contention {
        print_startup_event(
            start,
            "media_benchmark_contention",
            "active=1 benchmark_interaction_gate=disabled",
        );
    }
    if AUTO_CONTROLLER_SETUP_ENABLED {
        if let Some(device) = pad.device_needing_setup()
            && let Some(info) = pad.info_for_device(&device)
        {
            let status = pad.db().registry_status(info);
            crate::ui_errln!(
                "controller setup: {} generation {} needs setup ({status:?}) - showing prompt",
                device.plug_id,
                device.generation
            );
            setup.open_for(status, device);
        }
    }
    let mut pacer = ui
        .output_route()
        .nominal_period_us()
        .map(|period| {
            VsyncPacer::from_config_with_default_period(
                launcher_config.display_pacing().vsync(),
                period,
            )
        })
        .unwrap_or_else(|| VsyncPacer::from_config(launcher_config.display_pacing().vsync()));
    let pacing_policy = LauncherFramePacingPolicy::default();
    let mut phase_alignment = LauncherPhaseAlignment::default();
    let present_timing = launcher_config.display_pacing().present_timing();
    if preview_route.allows_preview_work()
        && launcher_bench_scenario.is_some()
        && !launcher_config.preview().archive_warm_skipped()
    {
        let warm_t = Instant::now();
        match preview_worker::warm_preview_archives_with_config(launcher_config.preview().worker())
        {
            Ok(loaded) => print_startup_event(
                start,
                "preview_archive_warm",
                format!(
                    "loaded={} elapsed_us={}",
                    if loaded { 1 } else { 0 },
                    warm_t.elapsed().as_micros()
                ),
            ),
            Err(e) => {
                crate::ui_errln!("preview archive warm failed before launcher benchmark: {e}");
                print_startup_event(start, "preview_archive_warm_failed", e);
                std::process::exit(13);
            }
        }
    } else if preview_route.allows_preview_work() && launcher_bench_scenario.is_some() {
        print_startup_event(start, "preview_archive_warm_skipped", "env=1");
    }
    let mut preview = PreviewState::new_with_config(start, launcher_config.preview().clone());
    let mut launcher_bench_waiting_for_initial_preview = launcher_bench_scenario
        .is_some_and(|scenario| scenario.starts_on_arcade() && !launcher_bench_after_input_script);
    let mut preview_transition = if preview_route.allows_preview_work() {
        PreviewTransitionDemo::from_config(launcher_config.preview_transition().clone())
    } else {
        PreviewTransitionDemo::disabled()
    };
    let transition_picker_enabled = preview_transition.picker_enabled();
    let mut arcade_list_renderer = if crt_layout {
        ArcadeListRenderer::new_for_crt_display(crt_metrics, ui)
    } else {
        ArcadeListRenderer::new()
    };
    arcade_list_renderer.set_crt_portrait_rows(layout.is_portrait());
    let mut crt_backdrop = CrtBackdropController::for_display(ui);
    let mut crt_arcade_overlay = CrtArcadeOverlayState::new();
    let mut launcher_preview_version = 1u64;
    let mut launcher_arcade_version = 1u64;
    let mut launcher_arcade_scroll_offset = LayerOffset::ZERO;
    let mut launcher_arcade_content_generation = 1u64;
    let mut launcher_preview_publication: Option<PhysicalLayerPublication> = None;
    let mut launcher_arcade_publication: Option<PhysicalLayerPublication> = None;
    let mut arcade_drawer_view_cache = ArcadeDrawerViewCache::default();
    let mut composition = UiCompositionController::new();
    let mut cpu = process_entry_cpu_profile.or_else(|| cpu_profile::start(profile_config.cpu()));
    let mut system_entry_cpu_profile = None;
    let mut screensaver_cpu_profile =
        cpu_profile::ScreensaverProfiler::from_config(profile_config.cpu());
    let mut bridge_models = LauncherBridgeModels::default();
    let mut catalog_version = 0usize;
    let user_state_session = UserStateSession::start(
        launcher_config
            .catalog_paths()
            .user_state_sqlite()
            .to_path_buf(),
        PathBuf::from("/media/fat"),
    );
    let mut user_state_catalog_version = None;
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    crate::ui_logln!(
        "preview_visual_pct={} preview_blitter=raw",
        launcher_config.preview().visual_pct()
    );
    crate::ui_logln!(
        "preview_transition={} segment_secs={} duration_ms={}",
        preview_transition.labels(),
        preview_transition.segment.as_secs(),
        preview_transition.duration.as_millis()
    );
    crate::ui_logln!(
        "fb_present_delay_us={} vsync_fresh_hit_max_age_us={}",
        present_timing.delay_us(),
        pacer.fresh_hit_max_age_us()
    );
    let return_capsule_target = launch_return_session.state().and_then(|state| {
        Some((
            state.collection_id()?.to_string(),
            state.game_path().to_string(),
        ))
    });
    let return_capsule = return_capsule_target.and_then(|(collection_id, game_path)| {
        let capsule_started = Instant::now();
        match return_catalog_capsule::take_return_catalog_capsule(
            Path::new(&arcade_root),
            &collection_id,
            &game_path,
        ) {
            Ok(capsule) => {
                print_startup_event(
                    start,
                    "return_catalog_capsule_decoded",
                    format!("elapsed_us={}", capsule_started.elapsed().as_micros()),
                );
                Some(capsule)
            }
            Err(error) => {
                print_startup_event(
                    start,
                    "return_catalog_capsule_rejected",
                    format!(
                        "elapsed_us={} error={}",
                        capsule_started.elapsed().as_micros(),
                        error.replace('\t', " ")
                    ),
                );
                launch_return_session.note_capsule_failure(error);
                None
            }
        }
    });
    let return_capsule_fingerprint = return_capsule
        .as_ref()
        .map(|capsule| capsule.durable_catalog_fingerprint.clone());
    let mut catalog = return_capsule
        .map(|capsule| capsule.catalog)
        .unwrap_or_else(|| empty_arcade_catalog(&arcade_root));
    let mut catalog_ready = !catalog.is_empty();
    let mut return_capsule_active = catalog_ready;
    let catalog_refresh_policy = catalog_refresh_policy();
    let catalog_refresh = catalog_refresh_policy.force_requested();
    let catalog_worker_enabled = catalog_refresh_policy.worker_enabled();
    let mut lifecycle = LauncherLifecycle::new(
        LauncherLifecycleConfig {
            catalog_worker_enabled,
        },
        start,
    );
    lifecycle.set_catalog_root(arcade_root.clone());
    let deferred_library_rebuild = consume_library_rebuild_marker(catalog_worker_enabled, start);
    // A forced replacement is not a foreground operation when a capsule,
    // sharded registry, summary, or existing database can seed the launcher.
    // First creation remains foreground through the !catalog_ready lifecycle.
    let mut catalog_session = LauncherCatalogSession::new(false);
    let mut catalog_publication_test =
        CatalogPublicationTestDriver::from_config(launcher_config.tests(), start);
    let mut media_session = ScreenshotMediaUpdateSession::default();
    let mut library_changed_dialog_test =
        LibraryChangedDialogTestDriver::from_config(launcher_config.tests(), start);
    let mut launcher_input_script =
        LauncherInputScriptDriver::from_config(launcher_config.input().scripted(), start);
    let mut launcher_automation = LauncherAutomation::new();
    let sqlite_path = mister_magik_catalog::catalog_state::path_for_root(
        launcher_config.catalog_paths().sharded_catalog_dir(),
    );
    let capsule_seed_ready = catalog_ready;
    let sharded_seed = (!capsule_seed_ready)
        .then(|| {
            read_sharded_registry_seed(
                &arcade_root,
                launcher_config.catalog_paths().sharded_catalog_dir(),
                start,
            )
        })
        .flatten();
    let sharded_seed_ready = sharded_seed.is_some();
    let sharded_catalog_fingerprint = sharded_seed
        .as_ref()
        .map(|seed| seed.catalog_fingerprint.clone());
    if let Some(seed) = sharded_seed {
        catalog = seed.catalog;
        catalog_ready = true;
    }
    let initial_catalog_fingerprint = return_capsule_fingerprint.or(sharded_catalog_fingerprint);
    let mut catalog_generation =
        initialize_catalog_generation(&mut scheduler, initial_catalog_fingerprint);
    if initial_system_entry_reader_required(capsule_seed_ready, sharded_seed_ready) {
        match scheduler.open_system_entry_reader() {
            Ok(elapsed_us) => print_startup_event(
                start,
                "system_entry_reader_opened",
                format!(
                    "generation={} elapsed_us={} cpu=0 preludes=on-demand",
                    catalog_generation.current.as_deref().unwrap_or("unknown"),
                    elapsed_us,
                ),
            ),
            Err(error) => print_startup_event(
                start,
                "system_entry_reader_open_failed",
                format!("error={}", error.replace('\t', " ")),
            ),
        }
    }
    let mut startup_ready_catalog_source = CatalogSource::FreshBuild;
    if capsule_seed_ready {
        startup_ready_catalog_source = CatalogSource::ReturnCapsule;
        catalog_session.note_summary_seed_ready();
        if preview_route.allows_preview_work() {
            media_session.request_catalog_seed();
        }
        catalog_version = catalog_version.wrapping_add(1);
        let request = summary_seed_catalog_worker_request(
            catalog_refresh_policy,
            deferred_library_rebuild,
            true,
        )
        .unwrap_or(CatalogWorkerRequest::LoadOnly);
        let initial_cache = summary_seed_catalog_worker_initial_cache(request, true);
        print_startup_event(
            start,
            "return_catalog_capsule_ready",
            format!(
                "root={} games={} request={}",
                arcade_root,
                catalog.len(),
                request.label()
            ),
        );
        let execution_mode = catalog_hydration_execution_mode(request);
        if catalog_publication_test.catalog_worker_allowed() {
            scheduler.start_catalog_worker(
                arcade_root.clone(),
                request,
                initial_cache,
                execution_mode,
            );
        }
    } else if sharded_seed_ready {
        startup_ready_catalog_source = CatalogSource::ShardedRegistry;
        catalog_session.note_summary_seed_ready();
        if preview_route.allows_preview_work() {
            media_session.request_catalog_seed();
        }
        catalog_version = catalog_version.wrapping_add(1);
        let return_catalog_hydration_needed = startup_return_requested;
        let request = summary_seed_catalog_worker_request(
            catalog_refresh_policy,
            deferred_library_rebuild,
            return_catalog_hydration_needed,
        );
        if let Some(request) = request {
            // Rich V3 rows are now the hydration authority. Validation may
            // inspect source facts, but it must not reopen the monolithic V2
            // navigation before a selected system asks for its mini-nav.
            let initial_cache = CatalogWorkerInitialCache::AlreadyLoadedReady;
            if summary_seed_catalog_worker_starts_immediately(
                request,
                return_catalog_hydration_needed,
            ) && catalog_publication_test.catalog_worker_allowed()
            {
                let execution_mode = catalog_hydration_execution_mode(request);
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                scheduler.start_catalog_worker(
                    arcade_root.clone(),
                    request,
                    initial_cache,
                    execution_mode,
                );
            } else {
                let delay = catalog_background_validation_delay();
                print_startup_event(
                    start,
                    "catalog_worker_deferred",
                    format!(
                        "root={} request={} delay_ms={} reason=sharded_registry_hydration",
                        arcade_root,
                        request.label(),
                        delay.as_millis()
                    ),
                );
                catalog_session.defer_catalog_worker(
                    arcade_root.clone(),
                    request,
                    initial_cache,
                    CatalogExecutionMode::BackgroundInteractive,
                );
            }
        } else {
            catalog_session.mark_refresh_done();
        }
    } else {
        let sqlite_state = catalog_startup_sqlite_state(&sqlite_path);
        match catalog_startup_without_summary_plan(
            sqlite_state,
            catalog_worker_enabled,
            catalog_refresh_policy,
            deferred_library_rebuild,
        ) {
            CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request,
                initial_cache,
                execution_mode,
            } => {
                print_startup_event(
                    start,
                    "catalog_worker_deferred",
                    format!(
                        "root={} request={} cache={} reason=first_visible_copy",
                        arcade_root,
                        request.label(),
                        sqlite_state.label()
                    ),
                );
                catalog_session.defer_catalog_worker(
                    arcade_root.clone(),
                    request,
                    initial_cache,
                    execution_mode,
                );
            }
            CatalogStartupWithoutSummaryPlan::NoCatalog => {
                print_startup_event(
                    start,
                    "catalog_refresh_decision",
                    format!(
                        "cache_state=missing refresh_policy={} background_validation=false plan=load_only",
                        catalog_refresh_policy.label()
                    ),
                );
                catalog_session.mark_refresh_done();
            }
        }
    }
    if catalog_publication_test.prepare_startup_catalog(
        &arcade_root,
        &mut catalog,
        &mut catalog_ready,
        start,
    ) {
        startup_ready_catalog_source = CatalogSource::FreshBuild;
        catalog_version = catalog_version.wrapping_add(1);
    }
    nav.sync_launcher_taxonomy(&catalog);
    if sharded_seed_ready && !capsule_seed_ready {
        launch_return_restored =
            launch_return_session.apply(&mut nav, &catalog, CatalogSource::ShardedRegistry);
    }
    if !capsule_seed_ready && !launch_return_restored {
        let _ = request_pending_launch_return_shard(
            launch_return_session.state(),
            &catalog,
            catalog_version,
            &mut nav,
            &mut scheduler,
            Instant::now(),
            start,
        );
    }
    if capsule_seed_ready {
        launch_return_restored =
            launch_return_session.apply(&mut nav, &catalog, CatalogSource::ReturnCapsule);
        if !launch_return_restored {
            crate::ui_errln!("return catalog capsule could not restore saved destination");
            catalog = empty_arcade_catalog(&arcade_root);
            catalog_ready = false;
            return_capsule_active = false;
            startup_ready_catalog_source = CatalogSource::FreshBuild;
            nav.sync_launcher_taxonomy(&catalog);
        }
    }
    nav.set_arcade_exit_locked(return_capsule_active);
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    apply_home_selected(&mut nav, &catalog, benchmark_config.home_selected(), start);
    let bridge_systems_t = Instant::now();
    let mut arcade_screen_pending = (start_screen == Screen::Arcade
        || lock_screen == Some(Screen::Arcade))
        && !arcade_navigation_ready(catalog_ready, &catalog);
    bridge.set_menu_title(nav.current_menu_title().into());
    bridge.set_menu_breadcrumb(nav.current_menu_breadcrumb().into());
    bridge.set_update_available(false);
    let menu_items = bridge_models.menu_items(&nav, catalog_version);
    bridge.set_menu_item_presentation(bridge_models.menu_item_presentation());
    bridge.set_menu_items(menu_items);
    let mut update_check = UpdateCheck::start(should_check_for_updates(
        launcher_bench_scenario.is_some(),
        bridge.get_dev_mode(),
    ));
    print_startup_event(
        start,
        "catalog_bridge_systems",
        format!(
            "catalog_ready={} systems={} elapsed_us={}",
            catalog_ready,
            catalog.systems.len(),
            bridge_systems_t.elapsed().as_micros()
        ),
    );
    let catalog_scan_title = if catalog_ready {
        if catalog_session.foreground_update() {
            "Indexing library".to_string()
        } else if catalog_refresh {
            "Validating library".to_string()
        } else {
            String::new()
        }
    } else if !catalog_worker_enabled {
        String::new()
    } else {
        "Indexing library".to_string()
    };
    let catalog_scan_detail = if catalog_ready {
        if catalog_session.foreground_update() {
            "Rebuilding catalog with latest games...".to_string()
        } else {
            format!("Using cached {} games", catalog.len())
        }
    } else if !catalog_worker_enabled {
        "Catalog worker disabled for benchmark restart".to_string()
    } else {
        "No cached catalog; scanning library...".to_string()
    };
    LauncherStatusPresenter::new(&bridge).sync_catalog_scan(CatalogScanBridgeStatus::new(
        initial_catalog_scan_visible(
            catalog_ready,
            arcade_catalog_required_at_start,
            catalog_worker_enabled,
            catalog_session.foreground_update(),
        ),
        false,
        catalog_scan_message(catalog_session.foreground_update()),
        catalog_scan_title,
        catalog_scan_detail,
        -1,
    ));
    let bridge_sync_t = Instant::now();
    sync_bridge_launcher(
        &app,
        &pad,
        &nav,
        &lifecycle,
        &setup,
        "",
        "",
        &catalog,
        &mut preview,
        &mut bridge_models,
        catalog_version,
        false,
        false,
        ui,
    );
    print_startup_event(
        start,
        "catalog_bridge_sync",
        format!(
            "catalog_ready={} games={} elapsed_us={}",
            catalog_ready,
            catalog.len(),
            bridge_sync_t.elapsed().as_micros()
        ),
    );
    lifecycle_effects.clear();
    let startup_catalog_state = if catalog_ready {
        StartupCatalogState::Ready {
            source: startup_ready_catalog_source,
            validation_scheduled: scheduler.catalog_worker_running()
                || !catalog_session.refresh_done(),
        }
    } else {
        StartupCatalogState::Building {
            mode: CatalogBuildMode::FirstBuild,
            foreground_catalog_update: catalog_session.foreground_update(),
            has_stale_catalog: false,
        }
    };
    let startup_mode = if startup_return_requested || launch_return_restored {
        StartupMode::ReturnFromGame
    } else if catalog_ready {
        StartupMode::WarmCatalog
    } else {
        StartupMode::ColdNoCatalog
    };
    lifecycle.begin_startup_reveal(startup_mode, start, &mut lifecycle_effects);
    if startup_return_requested && !launch_return_restored {
        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnCatalogHydrationNeeded,
            &mut lifecycle_effects,
        );
    }
    sync_startup_visibility(&app, &lifecycle);
    if launch_return_restored {
        emit_return_context_restored(
            &mut lifecycle,
            &mut lifecycle_effects,
            &nav,
            &catalog,
            &preview,
            &mut launch_return_session,
            start,
        );
    }
    let _ = lifecycle.after_boot_splash_presented(startup_catalog_state, &mut lifecycle_effects);
    apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
    let mut modal_input_test_dialog_pending =
        modal_input_catalog_recovery_test_requested(launcher_config.tests(), start);
    let auto_launch_gate = launcher_config
        .tests()
        .auto_launch_gate()
        .map(Path::to_path_buf);
    let mut modal_input_test_bridge_sync_pending = maybe_present_modal_input_test_dialog(
        &mut modal_input_test_dialog_pending,
        catalog_ready,
        &mut lifecycle,
        &mut lifecycle_effects,
        &mut scheduler,
        start,
    );
    window.request_redraw();
    let startup_intro_eligible = startup_mode == StartupMode::ColdNoCatalog
        && launcher_bench_scenario.is_none()
        && screensaver_start_mode == ScreensaverStartMode::Inactive
        && !layout.is_portrait();
    let mut startup_intro = if startup_intro_eligible
        && launcher_presenter.startup_intro_native_hidden_slots_available(ui)
    {
        match PreparedStartupIntro::new(ui) {
            Ok(prepared) => {
                print_startup_event(
                    start,
                    "startup_intro_started",
                    format!("width={} height={} fps=60", ui.fb_w(), ui.fb_h()),
                );
                Some(prepared.start())
            }
            Err(error) => {
                crate::ui_errln!("startup intro preparation failed: {error}");
                None
            }
        }
    } else {
        if startup_intro_eligible {
            print_startup_event(
                start,
                "startup_intro_skipped",
                "reason=direct-hidden-route-unavailable",
            );
        }
        None
    };
    // The particle scene owns the visible output. Keep Slint and its bridge
    // dormant until the existing launcher reveal transition fires, then build
    // exactly one off-screen launcher frame for the live morph target.
    let mut startup_intro_launcher_frame_ready = false;
    let mut startup_intro_bridge_dirty_pending = false;
    let mut startup_intro_catalog_ui_replay = None;
    let mut startup_intro_catalog_shells_pending = false;
    if startup_intro.is_some()
        && let Some(worker) = catalog_session.maybe_start_deferred_worker(
            scheduler.catalog_worker_running(),
            true,
            catalog_publication_test.catalog_worker_allowed(),
            Instant::now(),
            Duration::ZERO,
            catalog_builder_lock_available,
        )
    {
        print_startup_event(start, "catalog_worker_start", &worker.root);
        // A missing catalog always needs the first-visible Build operation,
        // even when a force-refresh request selected Reconcile before the
        // cache probe. The intro also owns CPU1, so override the ordinary cold
        // foreground mode at this boundary.
        let request = startup_intro_catalog_worker_request(worker.request);
        let execution_mode = CatalogExecutionMode::BackgroundInteractive;
        let lifecycle_input = deferred_catalog_worker_lifecycle_input(execution_mode, request);
        lifecycle.handle(lifecycle_input, &mut lifecycle_effects);
        apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
        scheduler.start_catalog_worker(worker.root, request, worker.initial_cache, execution_mode);
    }
    macro_rules! request_launcher_redraw {
        () => {{
            window.request_redraw();
        }};
    }
    let mut run_start =
        if arcade_catalog_required_at_start && arcade_navigation_ready(catalog_ready, &catalog) {
            Instant::now()
        } else {
            start
        };
    launcher_bench_next_step = run_start;
    // Post-navigation benchmarks do not begin until their target UI state is
    // active. A boot-time deadline would otherwise accept an inactive trace.
    let mut preview_scroll_exit_at = if launcher_bench_after_input_script {
        None
    } else {
        preview_scroll_exit_after_trace_deadline(run_start)
    };
    let mut first_render_logged = false;
    let mut first_vsync_logged = false;
    let mut first_launcher_frame_logged = false;
    let mut frame_accounting = LauncherFrameAccounting::new(
        run_start,
        ui.output_route().label(),
        ui.crt_font_experiment().label(),
        ui.fb_w(),
        ui.fb_h(),
        profile_config.frame().fps_log_enabled(),
    );
    if let Some(failure) = launcher_presenter.latch_failure() {
        frame_accounting.record_latch_failure(failure);
    }
    if launcher_bench_after_input_script {
        // Activation below replaces accounting and opens the measured trace.
        frame_accounting.close_preview_scroll_trace_for_restart();
    }
    let mut arcade_entry_latency =
        ArcadeEntryLatencyTracker::from_config(launcher_config.readiness().entry_trace());
    let mut memory_guard = crate::memory_pressure::MemoryPressureGuard::from_env();
    let catalog_contention_quiet_previews = matches!(
        std::env::var("MISTER_CATALOG_CONTENTION_QUIET_PREVIEWS")
            .ok()
            .as_deref(),
        Some("1") | Some("on") | Some("true") | Some("yes")
    );
    let mut last_home_pan_scroll_x = nav.scroll_x;
    let mut home_pan_present_until = None;
    let mut catalog_scan_blink = CatalogScanBlink::default();
    let mut navigation_source_bridge_sync_pending = false;
    let mut latency_critical_input_pending = false;
    let mut unpublished_cached_frame_present = false;
    let mut input_observation = input_observation_probe
        .as_ref()
        .map(crate::input_hub::InputObservationProbe::observe)
        .unwrap_or_default();
    let mut catalog_idle_candidate_since = None;
    let mut catalog_work_telemetry = CatalogWorkModeTelemetry::new(run_start);
    let mut reusable_early_input_events = VecDeque::new();
    #[cfg(test)]
    let mut launcher_frame_phase_observer = LauncherFramePhaseObserver::default();
    macro_rules! record_launcher_frame_phase {
        ($phase:expr) => {{
            #[cfg(test)]
            launcher_frame_phase_observer.record($phase);
        }};
    }
    macro_rules! run_launcher_input_phase {
        (
            $launcher:lifetime,
            $scheduler_phase:ident,
            $loop_start:ident,
            $route_input_early:ident,
            $pad_changed_for_input:ident,
            $setup_active:ident,
            $effective_view:ident,
            $full_bridge_dirty:ident,
            $light_bridge_dirty:ident,
            $early_input_change_checkpoint:ident
        ) => {{
'input_phase: {
            // Drain immediately before routing so catalog, timer, lifecycle,
            // and bridge housekeeping cannot sit between capture and dispatch.
            let drained_input = pad.drain_input_batch();
            record_launcher_frame_phase!(LauncherFramePhase::InputCaptured);
            input_observation = drained_input.observation;
            launcher_response_trace.observe_drained_input(&drained_input);
            if $route_input_early && !drained_input.batch.events.is_empty() {
                let first_publication = drained_input.publications.first().copied();
                launcher_response_trace.record_lab(Some(serde_json::json!({
                    "phase": "early-input-drain-attribution",
                    "first_changed_checkpoint": $early_input_change_checkpoint
                        .map(|checkpoint| checkpoint.label),
                    "first_changed_at_us": $early_input_change_checkpoint
                        .map(|checkpoint| checkpoint.observed_at_us),
                    "first_published_at_us": first_publication
                        .map(|publication| publication.published_at_us),
                    "first_proxy_sequence": first_publication
                        .and_then(|publication| publication.proxy_sequence),
                    "drained_at_us": crate::input_hub::monotonic_us(),
                    "observation_generation": drained_input.observation.generation(),
                })));
            }
            let input_batch = drained_input.batch;
            let input_batch_empty = input_batch.events.is_empty();
            let input_route_pmu = launcher_response_trace.input_pmu_span(
                !input_batch.events.is_empty(),
                "launcher-response.input-route",
            );
            input_integrity_trace.observe_batch(&input_batch);
            launcher_response_trace.record_lab(input_latency_lab.before_input_route());
            if !input_batch.events.is_empty()
                && let Some(stall_ms) = input_integrity_stall.take()
            {
                std::thread::sleep(Duration::from_millis(stall_ms));
            }
            let pad_changed = $pad_changed_for_input
                .take()
                .unwrap_or_else(|| pad.poll_with_debug_labels($setup_active));
            let frame_now = Instant::now();
            let mut current_input_events = VecDeque::new();
            let incoming_input_events = if $route_input_early {
                reusable_early_input_events.clear();
                &mut reusable_early_input_events
            } else {
                &mut current_input_events
            };
            let mut screensaver_wake = false;
            let input_batch_healthy = match input_router.accept_batch(&input_batch) {
                Ok(()) => {
                    input_fault_notice = None;
                    incoming_input_events.extend(input_batch.events.iter().copied());
                    true
                }
                Err(fault) => {
                    input_fault_notice = Some(fault.notice());
                    false
                }
            };
            let mut physical_for_automation = PadState::default();
            for action in crate::input_event::LogicalAction::ALL {
                physical_for_automation
                    .set_logical_action(action, input_batch.held_after_last.is_held(action));
            }
            if input_batch_healthy {
                incoming_input_events.extend(launcher_automation.poll_events(
                    &physical_for_automation,
                    $effective_view.accepts_application_input() && lifecycle.startup_input_enabled(),
                    setup.is_active(),
                    frame_now,
                ));
                if let Some(event) = settings_navigation_benchmark.event_for(
                    nav.screen,
                    nav.settings_focused,
                    nav.settings_selected,
                    full_screen_transition.state() == FullScreenTransitionState::Live,
                    frame_now
                        .saturating_duration_since(start)
                        .as_micros()
                        .min(u64::MAX as u128) as u64,
                ) {
                    incoming_input_events.push_back(event);
                }
                if let Some(event) = library_changed_dialog_test.event_for(&nav, frame_now, start) {
                    incoming_input_events.push_back(event);
                }
                if let Some(event) = launcher_input_script.event_for(
                    frame_now
                        .saturating_duration_since(start)
                        .as_micros()
                        .min(u64::MAX as u128) as u64,
                ) {
                    incoming_input_events.push_back(event);
                }
            }
            for event in incoming_input_events.iter().copied() {
                gui_profiling.observe_route_action(screen_label(nav.screen), event, frame_now);
                if nav.screen == Screen::Arcade
                    && event.action == LogicalAction::Down
                    && event.phase == InputPhase::Pressed
                    && gui_profiling.arcade_scroll_phase_started()
                {
                    screensaver_cpu_profile.begin_arcade_velocity_scroll(frames.saturating_add(1));
                }
            }
            if screensaver.active {
                while let Some(event) = incoming_input_events.pop_front() {
                    let focus = launcher_input_focus(true, true, false, false, false, false, &nav);
                    let outcome = input_router.route_event(event, focus, frame_now);
                    if matches!(outcome, InputOutcome::WakeScreensaver { .. }) {
                        latency_critical_input_pending = true;
                        screensaver_wake = true;
                        let _ = input_router.consume_remaining_batch(
                            incoming_input_events.drain(..),
                            ConsumedReason::ExclusiveBatch,
                        );
                        break;
                    }
                }
            }
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            let input_notice = input_fault_notice.or_else(|| {
                setup_disconnect_notice.then_some(
                    "Controller disconnected. Press a button after reconnecting to restart setup.",
                )
            });
            bridge.set_input_fault_notice(input_notice.unwrap_or_default().into());
            frame_accounting.set_automation_action_sequence(launcher_automation.action_sequence());

            let application_input_enabled =
                $effective_view.accepts_application_input() && lifecycle.startup_input_enabled();
            if !application_input_enabled {
                let disabled = launcher_input_focus(false, false, false, false, false, false, &nav);
                input_router.set_focus(disabled);
                for event in incoming_input_events.drain(..) {
                    let _ = input_router.route_event(event, disabled, frame_now);
                }
            }

            if application_input_enabled {
                if setup.is_active()
                    && setup
                        .target_device
                        .as_ref()
                        .is_none_or(|device| pad.info_for_device(device).is_none())
                {
                    crate::ui_errln!("controller setup: target disconnected; closing setup flow");
                    setup.cancel_disconnected();
                    setup_disconnect_notice = true;
                    bridge.set_input_fault_notice(
                    "Controller disconnected. Press a button after reconnecting to restart setup."
                        .into(),
                );
                    $full_bridge_dirty = true;
                }

                let raw_screensaver_input_activity =
                    pad.user_activity() || launcher_automation.active();
                let physical_input_held =
                    input_batch_healthy && pad_state_has_active_input(&physical_for_automation);
                let screensaver_input_held =
                    screensaver.input_held_for_control(screensaver_wake, physical_input_held);
                if screensaver.handle_input(
                    frame_now,
                    screensaver_input_held,
                    raw_screensaver_input_activity,
                ) {
                    record_launcher_frame_phase!(LauncherFramePhase::InputConsumed);
                    request_launcher_redraw!();
                    record_launcher_frame_phase!(LauncherFramePhase::Yielded);
                    break 'input_phase (true, input_batch_empty);
                }
                let active_device = pad.active_device();
                let info = pad.info().clone();
                let proxy_latest_captured_at_us =
                    input_batch.events.last().map(|event| event.captured_at_us);

                loop {
                    let lifecycle_view = lifecycle.view();
                    let focus = launcher_input_focus(
                        true,
                        false,
                        lifecycle_view.launch_failure_dialog().is_some()
                            || lifecycle_view.catalog_recovery_dialog().is_some(),
                        setup.is_active(),
                        nav.confirm_action.is_some(),
                        navigation_transition.is_active()
                            || orientation_transition.is_active()
                            || full_screen_transition.state() != FullScreenTransitionState::Live,
                        &nav,
                    );
                    input_router.set_focus(focus);
                    let mut final_input_tick = false;
                    let mut input_dispatch_now = frame_now;
                    let routed_event_this_loop = if let Some(event) =
                        incoming_input_events.pop_front()
                    {
                        if event.source.kind == InputSourceKind::MainProxy
                            && let Some(latest) = proxy_latest_captured_at_us
                        {
                            input_dispatch_now = frame_now
                                .checked_sub(Duration::from_micros(
                                    latest.saturating_sub(event.captured_at_us),
                                ))
                                .unwrap_or(frame_now);
                        }
                        let outcome = input_router.route_event(event, focus, frame_now);
                        input_integrity_trace.record_outcome(outcome);
                        launcher_response_trace.record_route(event, outcome);
                        latency_critical_input_pending |= matches!(
                            outcome,
                            InputOutcome::Dispatch { .. } | InputOutcome::WakeScreensaver { .. }
                        );
                        match outcome {
                            InputOutcome::Dispatch { event, .. } => Some(event),
                            InputOutcome::Released { event, context, .. }
                                if context == input_router.context() =>
                            {
                                Some(event)
                            }
                            InputOutcome::Released { .. } => None,
                            InputOutcome::WakeScreensaver { .. }
                            | InputOutcome::Consumed { .. } => None,
                        }
                    } else if focus.target.kind != InputContextKind::Transition
                        && let Some(outcome @ InputOutcome::Dispatch { event, .. }) =
                            input_router.tick_repeat(frame_now)
                    {
                        input_integrity_trace.record_outcome(outcome);
                        launcher_response_trace.record_route(event, outcome);
                        latency_critical_input_pending = true;
                        Some(event)
                    } else {
                        final_input_tick = true;
                        None
                    };
                    let mut launcher_state = PadState::default();
                    for action in crate::input_event::LogicalAction::ALL {
                        launcher_state.set_logical_action(action, input_router.action_held(action));
                    }
                    let selection_feedback_before =
                        discrete_selection_feedback_target(&nav, &setup, &lifecycle);
                    let selection_feedback_input =
                        accepted_selection_feedback_input(routed_event_this_loop.as_ref());

                    if launcher_bench_scenario.is_none()
                        && !settings_navigation_benchmark.enabled()
                        && setup.is_active()
                    {
                        let setup_before = SetupBridgeKey::from_setup(&setup);
                        let target_device = setup
                            .target_device
                            .clone()
                            .expect("active setup has an exact device identity");
                        let Some(setup_info) = pad.info_for_device(&target_device).cloned() else {
                            setup.cancel_disconnected();
                            setup_disconnect_notice = true;
                            $full_bridge_dirty = true;
                            continue;
                        };
                        let setup_action = routed_event_this_loop
                            .map_or(SetupAction::None, |event| {
                                setup.handle_action(&event, frame_now, &setup_info, pad.db())
                            });
                        match setup_action {
                            SetupAction::None => {}
                            SetupAction::RegisterNew => {
                                if let Err(e) = pad.register_new(&target_device) {
                                    crate::ui_errln!("controller setup: register new: {e}");
                                }
                            }
                            SetupAction::ClaimExisting { list_index } => {
                                if let Err(e) = pad.claim_existing(&target_device, list_index) {
                                    crate::ui_errln!("controller setup: claim existing: {e}");
                                }
                            }
                            SetupAction::SaveFinish { label, kind } => {
                                if let Err(e) = pad.finish_setup(&target_device, label, kind) {
                                    crate::ui_errln!("controller setup: save: {e}");
                                } else if let Some(info) = pad.info_for_device(&target_device) {
                                    crate::ui_errln!(
                                        "controller setup: saved \"{}\" ({})",
                                        pad.db().display_label(info),
                                        kind.as_str()
                                    );
                                }
                                setup.advance_to_next_pad(&pad);
                            }
                            SetupAction::Done => {
                                setup.advance_to_next_pad(&pad);
                            }
                        }
                        let setup_after = SetupBridgeKey::from_setup(&setup);
                        $full_bridge_dirty |= pad_changed || setup_before != setup_after;
                    } else if launcher_bench_scenario.is_none()
                        || launcher_bench_launch_handoff
                        || (launcher_bench_after_input_script && !launcher_bench_active)
                    {
                        if AUTO_CONTROLLER_SETUP_ENABLED && pad_changed {
                            let setup_before = SetupBridgeKey::from_setup(&setup);
                            setup.maybe_open(&info, active_device.clone(), pad.db(), true);
                            if setup.is_active() {
                                setup_disconnect_notice = false;
                            }
                            $full_bridge_dirty |= setup_before != SetupBridgeKey::from_setup(&setup);
                        }
                        if !setup.is_active() {
                            let nav_before = LauncherBridgeKey::from_nav(&nav);
                            let arcade_selected_before_input = nav.arcade.selected;
                            if transition_picker_enabled && nav.screen == Screen::Arcade {
                                let left = routed_event_this_loop.as_ref().is_some_and(|event| {
                                    event.phase == InputPhase::Pressed
                                        && event.action == LogicalAction::Left
                                });
                                let right = routed_event_this_loop.as_ref().is_some_and(|event| {
                                    event.phase == InputPhase::Pressed
                                        && event.action == LogicalAction::Right
                                });
                                let changed = if left {
                                    preview_transition.cycle_picker(-1)
                                } else if right {
                                    preview_transition.cycle_picker(1)
                                } else {
                                    false
                                };
                                if changed {
                                    crate::ui_logln!(
                                        "preview_transition_picker={}",
                                        preview_transition
                                            .current_label(frame_now.duration_since(run_start))
                                    );
                                    request_launcher_redraw!();
                                }
                            }
                            if let Some(orientation) = settings_navigation_benchmark
                                .take_orientation_change(
                                    nav.screen,
                                    full_screen_transition.state()
                                        == FullScreenTransitionState::Live,
                                )
                            {
                                apply_orientation_layout(
                                    &app,
                                    &window,
                                    ui,
                                    orientation,
                                    &mut nav,
                                    &mut layout,
                                    &mut layout_epoch,
                                    &mut navigation_transition,
                                );
                                $full_bridge_dirty = true;
                                request_launcher_redraw!();
                            }
                            let lifecycle_view = lifecycle.view();
                            let launch_failure_visible =
                                lifecycle_view.launch_failure_dialog().is_some();
                            let recovery_dialog_visible =
                                lifecycle_view.catalog_recovery_dialog().is_some();
                            let pending_collection_cancelled =
                                cancel_pending_collection_entry_for_input(
                                    &mut pending_collection_entry,
                                    &mut nav,
                                    routed_event_this_loop.as_ref(),
                                    start,
                                );
                            if pending_collection_cancelled {
                                preview.cancel_system_entry_preview();
                                arcade_entry_latency.cancel_enter();
                                if navigation_transition.is_active() {
                                    let now_us = frame_now
                                        .saturating_duration_since(start)
                                        .as_micros()
                                        .min(u64::MAX as u128)
                                        as u64;
                                    navigation_transition.request_reverse(now_us);
                                }
                            }
                            let settings_transition_source = (!launch_failure_visible
                                && !recovery_dialog_visible
                                && !navigation_transition.is_active()
                                && navigation_transition.enabled()
                                && settings_navigation_input_candidate(
                                    nav.screen,
                                    routed_event_this_loop.as_ref(),
                                ))
                            .then(|| (nav.screen, nav.navigation_transition_state()));
                            let event = if orientation_transition.is_active()
                                || full_screen_transition.owner()
                                    == Some(FullScreenTransitionOwner::Orientation)
                            {
                                None
                            } else if navigation_transition.is_active() {
                                None
                            } else if launch_failure_visible || recovery_dialog_visible {
                                if let Some(input) = route_lifecycle_dialog_input(
                                    routed_event_this_loop.as_ref(),
                                    launch_failure_visible,
                                    recovery_dialog_visible,
                                ) {
                                    lifecycle.handle(input, &mut lifecycle_effects);
                                    apply_lifecycle_effects(
                                        &mut lifecycle_effects,
                                        &mut scheduler,
                                        start,
                                    );
                                    $full_bridge_dirty = true;
                                }
                                None
                            } else if scheduler.should_request_benchmark_launch()
                                && catalog_ready
                                && !launcher_bench_waiting_for_initial_preview
                                && nav.screen == Screen::Arcade
                            {
                                active_system(&catalog, &nav)
                                    .and_then(|system| {
                                        nav.active_arcade_game_at(
                                            &catalog,
                                            &system.id,
                                            nav.arcade.selected,
                                        )
                                    })
                                    .map(|game| launcher::LauncherEvent {
                                        action: LauncherAction::LaunchGame,
                                        path: Some(game.mra_path.to_string()),
                                        settings: None,
                                    })
                            } else if auto_launch_selected
                                && !auto_launch_selected_done
                                && launcher_auto_launch_gate_ready(auto_launch_gate.as_deref())
                                && catalog_ready
                                && nav.screen == Screen::Arcade
                            {
                                let event = active_system(&catalog, &nav)
                                    .and_then(|system| {
                                        nav.active_arcade_game_at(
                                            &catalog,
                                            &system.id,
                                            nav.arcade.selected,
                                        )
                                    })
                                    .map(|game| launcher::LauncherEvent {
                                        action: LauncherAction::LaunchGame,
                                        path: Some(game.mra_path.to_string()),
                                        settings: None,
                                    });
                                auto_launch_selected_done = event.is_some();
                                event
                            } else if scheduler.launch_benchmark_enabled() {
                                None
                            } else if let Some(input_event) = routed_event_this_loop.as_ref() {
                                nav.handle_action_with_navigation_intents(
                                    input_event,
                                    input_dispatch_now,
                                    &catalog,
                                )
                            } else if final_input_tick {
                                nav.handle_held_tick_with_navigation_intents(
                                    &launcher_state,
                                    frame_now,
                                    &catalog,
                                )
                            } else {
                                None
                            };
                            let event = if !final_input_tick
                                && focus.target.kind == InputContextKind::Screen
                            {
                                event.or_else(|| {
                                    nav.handle_held_tick_with_navigation_intents(
                                        &launcher_state,
                                        input_dispatch_now,
                                        &catalog,
                                    )
                                })
                            } else {
                                event
                            };
                            if let Some((source_screen, source_state)) = settings_transition_source
                                && let Some((route, direction)) =
                                    settings_page_transition(source_screen, nav.screen)
                            {
                                let now_us = frame_now
                                    .saturating_duration_since(start)
                                    .as_micros()
                                    .min(u64::MAX as u128)
                                    as u64;
                                let source = target.cached_565();
                                let axis = match nav.settings.screen_orientation {
                                    ScreenOrientation::Normal => {
                                        SettingsPageTransitionAxis::Horizontal
                                    }
                                    ScreenOrientation::MonitorCounterclockwise => {
                                        SettingsPageTransitionAxis::Vertical
                                    }
                                    ScreenOrientation::MonitorClockwise => {
                                        SettingsPageTransitionAxis::VerticalReversed
                                    }
                                };
                                let started = navigation_transition.begin_settings_page_physical(
                                    route,
                                    direction,
                                    axis,
                                    ui.render_w(),
                                    ui.render_h(),
                                    source,
                                    now_us,
                                );
                                let started = started.unwrap_or(false);
                                if started
                                    && begin_navigation_full_screen_transition(
                                        &mut full_screen_transition,
                                        &mut navigation_transition_generation,
                                    )
                                {
                                    settings_navigation_benchmark.note_started(
                                        route,
                                        direction,
                                        source_screen,
                                        nav.screen,
                                        frames,
                                    );
                                    pending_navigation_transition =
                                        Some(PendingNavigationTransition {
                                            event: launcher::LauncherEvent {
                                                action: LauncherAction::NavigateBack,
                                                path: None,
                                                settings: None,
                                            },
                                            source_state,
                                            source_was_arcade: false,
                                            committed: true,
                                            status_quiesce_started_at: None,
                                        });
                                    $full_bridge_dirty = true;
                                    request_launcher_redraw!();
                                } else if started {
                                    navigation_transition.settle_at_destination();
                                    let _ = navigation_transition.complete();
                                }
                            }
                            if let Some(event) = event {
                                match event.action {
                                    LauncherAction::OpenMenu
                                    | LauncherAction::OpenCollection
                                    | LauncherAction::NavigateBack
                                    | LauncherAction::NavigateHome => {
                                        let collection_id = (event.action
                                            == LauncherAction::OpenCollection)
                                            .then(|| event.path.clone())
                                            .flatten();
                                        if let Some(collection_id) = collection_id.as_deref()
                                            && !collection_has_resident_rows(
                                                &catalog,
                                                collection_id,
                                            )
                                        {
                                            let requested_at = Instant::now();
                                            let entry = begin_cold_collection_entry(
                                                &mut scheduler,
                                                &mut nav,
                                                &mut preview,
                                                &catalog,
                                                catalog_version,
                                                collection_id,
                                                requested_at,
                                                "open-collection-intent",
                                                false,
                                                &mut arcade_entry_latency,
                                                &lifecycle,
                                                start,
                                            );
                                            $full_bridge_dirty |= entry.bridge_dirty;
                                            if entry.pending.is_some() {
                                                pending_collection_entry = entry.pending;
                                            }
                                        }

                                        let transition_spec =
                                            navigation_transition_for_intent(&nav, &event);
                                        if transition_spec.is_some()
                                            && nav.screen == Screen::Arcade
                                            && !crt_layout
                                        {
                                            if !layout.is_portrait() {
                                                arcade_list_renderer
                                                    .compose_layer_to_cached(target, true);
                                                let _ = target.compose_direct_preview_rect(
                                                    preview_screen_rect(ui),
                                                );
                                            }
                                        }
                                        let navigation_runtime_started = transition_spec
                                            .is_some_and(|(edge, direction)| {
                                                let geometry = match direction {
                                                    NavigationTransitionDirection::Forward => {
                                                        let root_menu = nav.current_menu_id()
                                                        == crate::launcher_taxonomy::ROOT_MENU_ID;
                                                        let selected_label = nav
                                                            .current_menu_items()
                                                            .get(nav.selected)
                                                            .map(|item| item.title.as_str())
                                                            .unwrap_or("");
                                                        Some(if crt_layout {
                                                            let content = layout.content_rect();
                                                            crt_navigation_geometry(
                                                                layout.logical_w(),
                                                                layout.logical_h(),
                                                                CrtNavigationLayout {
                                                                    content_x: content.x,
                                                                    content_y: content.y,
                                                                    content_width: content.width,
                                                                    content_height: content.height,
                                                                    grid_x: crt_metrics
                                                                        .grid_x
                                                                        .max(1)
                                                                        as usize,
                                                                    grid_y: crt_metrics
                                                                        .grid_y
                                                                        .max(1)
                                                                        as usize,
                                                                    header_height: crt_metrics
                                                                        .header_height
                                                                        .max(1)
                                                                        as usize,
                                                                    footer_height: crt_metrics
                                                                        .footer_height
                                                                        .max(1)
                                                                        as usize,
                                                                    heading_font_height: crt_metrics
                                                                        .heading_font
                                                                        .pixels()
                                                                        .max(1)
                                                                        as usize,
                                                                    title_font_height: crt_metrics
                                                                        .card_title_font
                                                                        .pixels()
                                                                        .max(1)
                                                                        as usize,
                                                                    detail_font_height: crt_metrics
                                                                        .card_detail_font
                                                                        .pixels()
                                                                        .max(1)
                                                                        as usize,
                                                                    game_row_height: crt_metrics
                                                                        .game_row_height
                                                                        .max(1)
                                                                        as usize,
                                                                },
                                                                nav.selected,
                                                                nav.current_menu_items().len(),
                                                                root_menu,
                                                                edge,
                                                                selected_label,
                                                            )
                                                        } else {
                                                            hdmi_navigation_geometry(
                                                                layout.logical_w(),
                                                                layout.logical_h(),
                                                                nav.selected,
                                                                nav.scroll_x,
                                                                root_menu,
                                                                edge,
                                                                selected_label,
                                                            )
                                                        })
                                                    }
                                                    NavigationTransitionDirection::Reverse => {
                                                        navigation_transition
                                                            .geometry_for_reverse(edge)
                                                    }
                                                };
                                                geometry.is_some_and(|geometry| {
                                                    let started = if layout.is_portrait() {
                                                        navigation_transition.begin_physical(
                                                            edge,
                                                            direction,
                                                            navigation_geometry_to_composition(
                                                                layout, geometry,
                                                            ),
                                                            geometry,
                                                            layout.composition_w(),
                                                            layout.composition_h(),
                                                            target.cached_565(),
                                                            frame_now
                                                                .saturating_duration_since(start)
                                                                .as_micros()
                                                                .min(u64::MAX as u128)
                                                                as u64,
                                                        )
                                                    } else {
                                                        navigation_transition.begin(
                                                            edge,
                                                            direction,
                                                            geometry,
                                                            target.cached_565(),
                                                            frame_now
                                                                .saturating_duration_since(start)
                                                                .as_micros()
                                                                .min(u64::MAX as u128)
                                                                as u64,
                                                        )
                                                    };
                                                    started.unwrap_or(false)
                                                })
                                            });
                                        let transition_started = navigation_runtime_started
                                            && begin_navigation_full_screen_transition(
                                                &mut full_screen_transition,
                                                &mut navigation_transition_generation,
                                            );
                                        if transition_started {
                                            let source_state = nav.navigation_transition_state();
                                            pending_navigation_transition =
                                                Some(PendingNavigationTransition {
                                                    event: event.clone(),
                                                    source_state,
                                                    source_was_arcade: nav.screen == Screen::Arcade,
                                                    committed: false,
                                                    status_quiesce_started_at: None,
                                                });
                                            $full_bridge_dirty = true;
                                            request_launcher_redraw!();
                                        } else if navigation_runtime_started {
                                            navigation_transition.settle_at_destination();
                                            let _ = navigation_transition.complete();
                                        } else if collection_id.is_none()
                                            || collection_id.as_deref().is_some_and(
                                                |collection_id| {
                                                    collection_has_resident_rows(
                                                        &catalog,
                                                        collection_id,
                                                    )
                                                },
                                            )
                                        {
                                            if nav.commit_navigation_intent(&event, &catalog) {
                                                if let Some(collection_id) =
                                                    collection_id.as_deref()
                                                {
                                                    print_startup_event(
                                                        start,
                                                        "catalog_system_entry_immediate",
                                                        format!(
                                                            "system={collection_id} resident_rows={}",
                                                            catalog
                                                                .system_game_count(collection_id)
                                                        ),
                                                    );
                                                }
                                                $full_bridge_dirty = true;
                                                request_launcher_redraw!();
                                            }
                                        }
                                    }
                                    LauncherAction::ExitToMister => {
                                        loading_title = "Exit to MiSTer".to_string();
                                        sync_bridge_launcher(
                                            &app,
                                            &pad,
                                            &nav,
                                            &lifecycle,
                                            &setup,
                                            scheduler.visible_loading_title(&loading_title),
                                            "Return to MiSTer MagiK after reboot",
                                            &catalog,
                                            &mut preview,
                                            &mut bridge_models,
                                            catalog_version,
                                            false,
                                            false,
                                            ui,
                                        );
                                        window.request_redraw();
                                        update_slint_animations(animation_clock);
                                        let _ =
                                            render_immediate_launcher_frame(window, target, layout);
                                        let _pace = pacer.wait();
                                        copy_cached_rows_565(
                                            disp,
                                            target.cached_frame_view(),
                                            0,
                                            ui.render_h(),
                                        );
                                        match launcher::exit_to_mister() {
                                            Ok(()) => std::process::exit(0),
                                            Err(e) => {
                                                crate::ui_errln!("exit to MiSTer failed: {e}");
                                                loading_title.clear();
                                            }
                                        }
                                    }
                                    LauncherAction::RebuildDatabase => {
                                        let effects =
                                            catalog_session.rebuild_database(arcade_root.clone());
                                        apply_catalog_session_effects(
                                            effects,
                                            preview_route,
                                            &mut launcher_response_trace,
                                            &app,
                                            &mut nav,
                                            &mut catalog,
                                            &mut catalog_ready,
                                            &mut catalog_version,
                                            &mut return_capsule_active,
                                            &mut catalog_generation,
                                            &mut launch_return_session,
                                            &mut preview,
                                            &mut media_session,
                                            &mut scheduler,
                                            &mut lifecycle,
                                            &mut lifecycle_effects,
                                            &mut $full_bridge_dirty,
                                            &mut startup_intro_catalog_ui_replay,
                                            &mut startup_intro_catalog_shells_pending,
                                            false,
                                            $loop_start,
                                            start,
                                        );
                                        request_launcher_redraw!();
                                        continue $launcher;
                                    }
                                    LauncherAction::Restart => {
                                        loading_title = "Shutting down…".to_string();
                                        sync_bridge_launcher(
                                            &app,
                                            &pad,
                                            &nav,
                                            &lifecycle,
                                            &setup,
                                            scheduler.visible_loading_title(&loading_title),
                                            "Restarting MiSTer",
                                            &catalog,
                                            &mut preview,
                                            &mut bridge_models,
                                            catalog_version,
                                            false,
                                            false,
                                            ui,
                                        );
                                        window.request_redraw();
                                        update_slint_animations(animation_clock);
                                        let _ =
                                            render_immediate_launcher_frame(window, target, layout);
                                        let _pace = pacer.wait();
                                        copy_cached_rows_565(
                                            disp,
                                            target.cached_frame_view(),
                                            0,
                                            ui.render_h(),
                                        );
                                        std::thread::sleep(Duration::from_millis(250));
                                        match launcher::reboot_mister() {
                                            Ok(()) => continue $launcher,
                                            Err(e) => {
                                                crate::ui_errln!("restart failed: {e}");
                                                loading_title.clear();
                                            }
                                        }
                                    }
                                    LauncherAction::ContinueWithStaleLibrary => {
                                        let effects = catalog_session.continue_with_stale_library();
                                        apply_catalog_session_effects(
                                            effects,
                                            preview_route,
                                            &mut launcher_response_trace,
                                            &app,
                                            &mut nav,
                                            &mut catalog,
                                            &mut catalog_ready,
                                            &mut catalog_version,
                                            &mut return_capsule_active,
                                            &mut catalog_generation,
                                            &mut launch_return_session,
                                            &mut preview,
                                            &mut media_session,
                                            &mut scheduler,
                                            &mut lifecycle,
                                            &mut lifecycle_effects,
                                            &mut $full_bridge_dirty,
                                            &mut startup_intro_catalog_ui_replay,
                                            &mut startup_intro_catalog_shells_pending,
                                            false,
                                            $loop_start,
                                            start,
                                        );
                                        request_launcher_redraw!();
                                        continue $launcher;
                                    }
                                    LauncherAction::RebuildLibrary => {
                                        let effects =
                                            catalog_session.rebuild_library(arcade_root.clone());
                                        apply_catalog_session_effects(
                                            effects,
                                            preview_route,
                                            &mut launcher_response_trace,
                                            &app,
                                            &mut nav,
                                            &mut catalog,
                                            &mut catalog_ready,
                                            &mut catalog_version,
                                            &mut return_capsule_active,
                                            &mut catalog_generation,
                                            &mut launch_return_session,
                                            &mut preview,
                                            &mut media_session,
                                            &mut scheduler,
                                            &mut lifecycle,
                                            &mut lifecycle_effects,
                                            &mut $full_bridge_dirty,
                                            &mut startup_intro_catalog_ui_replay,
                                            &mut startup_intro_catalog_shells_pending,
                                            false,
                                            $loop_start,
                                            start,
                                        );
                                        request_launcher_redraw!();
                                        continue $launcher;
                                    }
                                    LauncherAction::ApplyDisplayResolution => {
                                        if let Some(id) = event.path.as_deref() {
                                            let result = launcher::apply_display_resolution(id);
                                            pacer.rearm_after_display_mode_change();
                                            if let Err(error) = result {
                                                crate::ui_errln!("display apply failed: {error}");
                                                nav.display_error = Some(format!(
                                                    "Could not apply the selected resolution: {error}"
                                                ));
                                                nav.confirm_action = Some(
                                                    launcher::ConfirmAction::DisplayResolutionError,
                                                );
                                                nav.confirm_selected = 0;
                                            }
                                        }
                                    }
                                    LauncherAction::ConfirmDisplayResolution => {
                                        nav.display_confirm_busy = true;
                                        nav.display_error = None;
                                        nav.confirm_action =
                                            Some(launcher::ConfirmAction::DisplayResolution);
                                        let result_tx = display_confirm_tx.clone();
                                        std::thread::spawn(move || {
                                            let result =
                                                launcher::confirm_display_resolution_and_wait(
                                                    Duration::from_secs(12),
                                                );
                                            let _ = result_tx.send(result);
                                        });
                                    }
                                    LauncherAction::CancelDisplayResolution => {
                                        let result = launcher::cancel_display_resolution();
                                        pacer.rearm_after_display_mode_change();
                                        if let Err(error) = result {
                                            crate::ui_errln!("display rollback failed: {error}");
                                            nav.display_error = Some(format!(
                                                "Could not restore the previous resolution: {error}"
                                            ));
                                            nav.confirm_action = Some(
                                                launcher::ConfirmAction::DisplayResolutionError,
                                            );
                                            nav.confirm_selected = 0;
                                        }
                                    }
                                    LauncherAction::ApplyScreenOrientation => {
                                        if let Some(orientation) =
                                            event.path.as_deref().and_then(ScreenOrientation::parse)
                                            && orientation != nav.settings.screen_orientation
                                        {
                                            let previous = nav.settings.screen_orientation;
                                            orientation_previous = Some(previous);
                                            nav.orientation_confirm_busy = false;
                                            nav.orientation_error = None;
                                            arm_orientation_confirmation(&mut nav);
                                            orientation_confirm_deadline = None;
                                            let animated = begin_orientation_transition(
                                                &app,
                                                window,
                                                ui,
                                                target,
                                                previous,
                                                orientation,
                                                frame_now,
                                                nav.settings.reduce_motion,
                                                &mut nav,
                                                &mut layout,
                                                &mut layout_epoch,
                                                &mut navigation_transition,
                                                &mut full_screen_transition,
                                                &mut orientation_transition_generation,
                                                &mut orientation_transition,
                                                &mut orientation_transition_intent,
                                                &mut orientation_preparation_trace,
                                                OrientationTransitionIntent::Confirm,
                                            );
                                            if !animated {
                                                let _ = orientation_transition.take_completion();
                                                orientation_confirm_deadline = Some(
                                                    Instant::now()
                                                        + Duration::from_secs(u64::from(
                                                            launcher::DISPLAY_CONFIRM_SECONDS,
                                                        )),
                                                );
                                            }
                                            orientation_full_redraw_pending = true;
                                            $full_bridge_dirty = true;
                                        }
                                    }
                                    LauncherAction::ConfirmScreenOrientation => {
                                        orientation_confirm_deadline = None;
                                        nav.orientation_confirm_remaining = 0;
                                        nav.orientation_confirm_busy = true;
                                        nav.orientation_error = None;
                                        nav.confirm_action =
                                            Some(launcher::ConfirmAction::ScreenOrientation);
                                        nav.confirm_selected = 1;
                                        let confirmed = nav.settings.clone();
                                        let mut previous = confirmed.clone();
                                        previous.screen_orientation = orientation_previous
                                            .unwrap_or(confirmed.screen_orientation);
                                        let result_tx = orientation_confirm_tx.clone();
                                        let store = orientation_store.clone();
                                        std::thread::spawn(move || {
                                            let result = store
                                                .save_confirmed(&previous, &confirmed)
                                                .map_err(|error| error.to_string());
                                            let _ = result_tx.send(result);
                                        });
                                    }
                                    LauncherAction::CancelScreenOrientation => {
                                        if let Some(previous) = orientation_previous.take() {
                                            let from = nav.settings.screen_orientation;
                                            let animated = begin_orientation_transition(
                                                &app,
                                                window,
                                                ui,
                                                target,
                                                from,
                                                previous,
                                                frame_now,
                                                nav.settings.reduce_motion,
                                                &mut nav,
                                                &mut layout,
                                                &mut layout_epoch,
                                                &mut navigation_transition,
                                                &mut full_screen_transition,
                                                &mut orientation_transition_generation,
                                                &mut orientation_transition,
                                                &mut orientation_transition_intent,
                                                &mut orientation_preparation_trace,
                                                OrientationTransitionIntent::Rollback,
                                            );
                                            let _ = animated;
                                        }
                                        orientation_confirm_deadline = None;
                                        nav.orientation_confirm_remaining = 0;
                                        nav.orientation_confirm_busy = false;
                                        nav.orientation_error = None;
                                        orientation_full_redraw_pending = true;
                                        $full_bridge_dirty = true;
                                    }
                                    LauncherAction::PreviewScreensaver => {
                                        if !screensaver.preview_active {
                                            screensaver.preview(frame_now);
                                            screensaver_show_started = Some(frame_now);
                                            screensaver_first_render_logged = false;
                                            screensaver_first_present_logged = false;
                                            screensaver_first_card_present_logged = false;
                                            crate::ui_logln!(
                                                "screensaver_startup_timing milestone=show_pressed elapsed_us=0"
                                            );
                                        }
                                        request_launcher_redraw!();
                                        continue $launcher;
                                    }
                                    LauncherAction::PersistSettings => {
                                        if let Some(settings) = event.settings.as_ref() {
                                            navigation_transition.set_enabled(
                                                ui.render_w(),
                                                ui.render_h(),
                                                !settings.reduce_motion,
                                            );
                                            if let Err(error) = settings_store.save(settings) {
                                                crate::ui_errln!(
                                                    "settings: failed to save launcher settings: {error}"
                                                );
                                            }
                                        }
                                    }
                                    LauncherAction::AddFavourite
                                    | LauncherAction::RemoveFavourite => {
                                        let favourite =
                                            event.action == LauncherAction::AddFavourite;
                                        if let Some(launch_ref) = event.path.as_deref()
                                            && let Some(game) =
                                                catalog.user_game_identity_for_ref(launch_ref)
                                        {
                                            nav.reconcile_favourite_state(
                                                &catalog, launch_ref, favourite,
                                            );
                                            let now = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .ok()
                                                .and_then(|duration| {
                                                    i64::try_from(duration.as_secs()).ok()
                                                })
                                                .unwrap_or(0);
                                            user_state_session.set_favourite(game, favourite, now);
                                            $full_bridge_dirty = true;
                                            request_launcher_redraw!();
                                        }
                                    }
                                    LauncherAction::LaunchGame => {}
                                }
                                if event.action == LauncherAction::LaunchGame {
                                    let Some(mra) = event.path else {
                                        continue;
                                    };
                                    let lifecycle_step = lifecycle.handle(
                                        LauncherLifecycleInput::LaunchRequested {
                                            launch_ref: mra.clone(),
                                        },
                                        &mut lifecycle_effects,
                                    );
                                    if !matches!(
                                        lifecycle_step.state,
                                        LauncherLifecycleState::Launching {
                                            phase: LaunchingPhase::LoadingFramePending { ref launch_ref },
                                        } if launch_ref == &mra
                                    ) {
                                        apply_lifecycle_effects(
                                            &mut lifecycle_effects,
                                            &mut scheduler,
                                            start,
                                        );
                                        continue;
                                    }
                                    if !scheduler.begin_launch(
                                        &nav,
                                        &catalog,
                                        catalog_generation.durable.as_deref(),
                                        &mra,
                                        Instant::now(),
                                    ) {
                                        lifecycle.handle(
                                            LauncherLifecycleInput::LaunchFailed {
                                                title: launcher::game_title(&catalog, &mra),
                                                kind: launcher::LaunchFailureKind::Internal,
                                                detail: "launch scheduler rejected request"
                                                    .to_string(),
                                            },
                                            &mut lifecycle_effects,
                                        );
                                        apply_lifecycle_effects(
                                            &mut lifecycle_effects,
                                            &mut scheduler,
                                            start,
                                        );
                                        continue;
                                    }
                                    apply_lifecycle_effects(
                                        &mut lifecycle_effects,
                                        &mut scheduler,
                                        start,
                                    );
                                    sync_bridge_launcher(
                                        &app,
                                        &pad,
                                        &nav,
                                        &lifecycle,
                                        &setup,
                                        scheduler.launch_loading_title(),
                                        "",
                                        &catalog,
                                        &mut preview,
                                        &mut bridge_models,
                                        catalog_version,
                                        false,
                                        false,
                                        ui,
                                    );
                                    window.request_redraw();
                                    update_slint_animations(animation_clock);
                                    let _ = render_immediate_launcher_frame(window, target, layout);
                                    let _pace = pacer.wait();
                                    copy_cached_rows_565(
                                        disp,
                                        target.cached_frame_view(),
                                        0,
                                        ui.render_h(),
                                    );
                                    let loading_presented = Instant::now();
                                    lifecycle.loading_frame_presented(
                                        loading_presented,
                                        &mut lifecycle_effects,
                                    );
                                    apply_lifecycle_effects(
                                        &mut lifecycle_effects,
                                        &mut scheduler,
                                        start,
                                    );
                                    request_launcher_redraw!();
                                }
                            }
                            let nav_after = LauncherBridgeKey::from_nav(&nav);
                            if nav_before != nav_after {
                                if let Some(entry) = pending_collection_entry.take() {
                                    preview.cancel_system_entry_preview();
                                    nav.catalog_system_hydration_finished(&entry.collection_id);
                                    print_startup_event(
                                        start,
                                        "catalog_system_entry_cancelled",
                                        format!(
                                            "system={} reason=navigation-changed",
                                            entry.collection_id
                                        ),
                                    );
                                }
                                media_session.note_nav_change(
                                    &nav_before,
                                    &nav_after,
                                    Instant::now(),
                                );
                            }
                            if pad_changed && nav.screen == Screen::Controller {
                                $full_bridge_dirty = true;
                            } else if pad_changed && !dirty_opt {
                                $full_bridge_dirty = true;
                            }
                            if nav_before != nav_after {
                                if nav_before.screen == Screen::Home
                                    && nav_after.screen == Screen::Arcade
                                {
                                    arcade_entry_latency.record_enter_input(
                                        start, frame_now, &lifecycle, &catalog, &nav,
                                    );
                                    if !active_system_games_loading(&catalog, &nav) {
                                        if let Some(system) = active_system(&catalog, &nav) {
                                            if catalog.system_game_count(&system.id) > 0 {
                                                arcade_entry_latency.record_rows_ready(
                                                    start, frame_now, &lifecycle, &catalog, &nav,
                                                );
                                            }
                                        }
                                    }
                                } else if nav_before.screen == Screen::Arcade
                                    && nav_after.screen == Screen::Arcade
                                    && arcade_selected_before_input != nav.arcade.selected
                                {
                                    arcade_entry_latency.record_first_nav_input(
                                        start, frame_now, &lifecycle, &catalog, &nav,
                                    );
                                }
                                if !dirty_opt
                                    || nav_before.screen != nav_after.screen
                                    || nav_before.menu_id != nav_after.menu_id
                                {
                                    $full_bridge_dirty = true;
                                } else {
                                    $light_bridge_dirty = true;
                                }
                            }
                        }
                    }
                    let selection_feedback_after =
                        discrete_selection_feedback_target(&nav, &setup, &lifecycle);
                    let feedback_surface_changed = bridge_models
                        .sync_selection_feedback_surface(selection_feedback_after.as_ref());
                    let feedback_registered = selection_feedback_input
                        && bridge_models.note_selection_feedback_change(
                            selection_feedback_before.as_ref(),
                            selection_feedback_after.as_ref(),
                        );
                    if feedback_surface_changed || feedback_registered {
                        $full_bridge_dirty = true;
                        request_launcher_redraw!();
                    }
                    if final_input_tick {
                        break;
                    }
                    launcher_response_trace.observe_state(&nav, navigation_transition.is_active());
                }
                input_integrity_trace.flush_if_due(Instant::now(), &input_router);

                if let Some(screen) = effective_lock_screen(lock_screen, catalog_ready, &catalog) {
                    nav.screen = screen;
                }
            } else {
                if let Some(action) = scheduler.launch_runtime_action(Instant::now()) {
                    match action {
                        LaunchHandoffRuntimeAction::ArcadeCoreRunning => {
                            crate::ui_logln!("arcade core running — handing off to MiSTer");
                            std::process::exit(0);
                        }
                        LaunchHandoffRuntimeAction::TimedOut => {
                            crate::ui_errln!("game launch timed out");
                            lifecycle.handle(
                                LauncherLifecycleInput::LaunchTimedOut,
                                &mut lifecycle_effects,
                            );
                            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                            if scheduler.stop_spawned_mister_for_recovery() {
                                if let Err(e) =
                                    display_session.recover_after_launch_failure(frames, f)
                                {
                                    crate::ui_errln!(
                                        "failed to recover Slint framebuffer route after launch timeout: {e}"
                                    );
                                }
                            }
                            std::process::exit(1);
                        }
                    }
                }
            }

            launcher_response_trace.record_lab(input_latency_lab.arm_if_computers_ready(&nav));
            drop(input_route_pmu);
            record_launcher_frame_phase!(LauncherFramePhase::InputRouted);
            $scheduler_phase =
                launcher_response_trace.record_scheduler_interval("input-route", $scheduler_phase);
            (false, input_batch_empty)
        }
        }};
    }
    'launcher: while (secs == 0 || run_start.elapsed().as_secs() < secs)
        && preview_scroll_exit_at.is_none_or(|deadline| Instant::now() < deadline)
    {
        record_launcher_frame_phase!(LauncherFramePhase::Begin);
        gui_profiling.tick(Instant::now());
        let mut scheduler_phase = launcher_response_trace.scheduler_boundary();
        screensaver_cpu_profile.poll(frames);
        if catalog_publication_test.wait_for_first_frame_release(Instant::now(), start) {
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }
        let loop_start = Instant::now();
        let slint_timer_dispatch_started = Instant::now();
        let gui_timer_dispatch_pmu = gui_profiling.span("gui.timer-dispatch");
        let full_screen_transition_policy_at_loop_start = full_screen_transition.policy();
        let full_screen_transition_owned_at_loop_start =
            full_screen_transition_owns_cpu1(full_screen_transition.state());
        let current_pad_state = pad.state();
        let directional_input_held = current_pad_state.dpad_up
            || current_pad_state.dpad_down
            || current_pad_state.dpad_left
            || current_pad_state.dpad_right
            || launcher_automation.directional_input_held();
        let input_pending_before_route = input_observation_probe
            .as_ref()
            .is_some_and(|probe| probe.changed_since(input_observation));
        let mut background_work_allowed = !input_pending_before_route
            && !should_defer_launcher_background_work(
                0,
                navigation_transition.is_active(),
                orientation_transition.is_active(),
                directional_input_held,
            )
            && !full_screen_transition_owned_at_loop_start;
        let startup_intro_needs_live_launcher = startup_intro_launcher_ui_plan(
            startup_intro.is_some(),
            lifecycle.startup_status().state,
            startup_intro_launcher_frame_ready,
        ) == StartupIntroLauncherUiPlan::PrepareLiveFrame;
        if !input_pending_before_route
            && full_screen_transition_policy_at_loop_start.advance_slint_timers
            && (startup_intro.is_none() || startup_intro_needs_live_launcher)
        {
            slint::platform::update_timers_and_animations();
        }
        let slint_timer_dispatch_us = slint_timer_dispatch_started.elapsed().as_micros();
        drop(gui_timer_dispatch_pmu);
        if input_observation_probe
            .as_ref()
            .is_some_and(|probe| probe.changed_since(input_observation))
        {
            background_work_allowed = false;
        }
        let mut full_bridge_dirty = std::mem::take(&mut navigation_source_bridge_sync_pending)
            || std::mem::take(&mut modal_input_test_bridge_sync_pending);
        let route_input_early = benchmark_config.route_input_early();
        let mut effective_view =
            EffectiveLauncherView::resolve(&lifecycle, screensaver.active, nav.screen);
        let mut setup_active = setup.is_active();
        let mut light_bridge_dirty = false;
        let mut pad_changed_for_input = None;
        let mut early_input_phase_result = None;
        let mut early_input_change_checkpoint =
            input_pending_before_route.then(|| EarlyInputChangeCheckpoint {
                label: "after-timers",
                observed_at_us: crate::input_hub::monotonic_us(),
            });
        if route_input_early && input_pending_before_route {
            pad_changed_for_input = if effective_view.accepts_application_input()
                && lifecycle.startup_input_enabled()
            {
                Some(pad.poll_with_debug_labels(setup_active))
            } else {
                None
            };
            let result = run_launcher_input_phase!(
                'launcher,
                scheduler_phase,
                loop_start,
                route_input_early,
                pad_changed_for_input,
                setup_active,
                effective_view,
                full_bridge_dirty,
                light_bridge_dirty,
                early_input_change_checkpoint
            );
            if result.0 {
                continue;
            }
            if !result.1
                || input_observation_probe
                    .as_ref()
                    .is_some_and(|probe| probe.changed_since(input_observation))
            {
                background_work_allowed = false;
            }
            early_input_phase_result = Some(result);
        }
        if startup_intro.is_none() {
            #[cfg(test)]
            if startup_intro_catalog_shells_pending || startup_intro_catalog_ui_replay.is_some() {
                record_launcher_frame_phase!(LauncherFramePhase::StartupCatalogReplay);
            }
            if std::mem::take(&mut startup_intro_catalog_shells_pending) {
                catalog = nav.catalog_with_build_shells(catalog.clone());
                catalog_version = catalog_version.wrapping_add(1);
                nav.sync_launcher_taxonomy(&catalog);
                let _ = reapply_pending_launch_return_state(
                    &mut nav,
                    &catalog,
                    &mut launch_return_session,
                );
                full_bridge_dirty = true;
            }
            if let Some(intent) = startup_intro_catalog_ui_replay.take() {
                apply_launcher_worker_ui_intent(&app, intent, &mut full_bridge_dirty);
                window.request_redraw();
            }
        }
        let current_feedback_target = discrete_selection_feedback_target(&nav, &setup, &lifecycle);
        if bridge_models.sync_selection_feedback_surface(current_feedback_target.as_ref()) {
            full_bridge_dirty = true;
            request_launcher_redraw!();
        }
        if bridge_models.expire_selection_feedback(loop_start) {
            full_bridge_dirty = true;
            request_launcher_redraw!();
        }
        if (!orientation_benchmark_requires_analytics
            || frame_accounting.frame_analytics_mode() != FrameAnalyticsMode::Off)
            && let Some(leg) = orientation_benchmark.take_next_leg(
                nav.settings.screen_orientation,
                frames,
                loop_start,
            )
        {
            if !orientation_transition.set_effect(leg.effect) {
                orientation_benchmark.fail("benchmark-effect-changed-during-transition");
                continue;
            }
            let animated = begin_orientation_transition(
                &app,
                window,
                ui,
                target,
                leg.from,
                leg.to,
                loop_start,
                false,
                &mut nav,
                &mut layout,
                &mut layout_epoch,
                &mut navigation_transition,
                &mut full_screen_transition,
                &mut orientation_transition_generation,
                &mut orientation_transition,
                &mut orientation_transition_intent,
                &mut orientation_preparation_trace,
                OrientationTransitionIntent::Benchmark,
            );
            if animated {
                if leg.index == 0 {
                    screensaver_cpu_profile.begin_orientation_transitions(frames);
                }
                print_startup_event(
                    start,
                    "orientation_transition_benchmark_leg_started",
                    format!(
                        "leg={} effect={} label={} from={} to={} frame={frames}",
                        leg.index + 1,
                        leg.effect.id(),
                        leg.label(),
                        leg.from.id(),
                        leg.to.id(),
                    ),
                );
            } else {
                orientation_benchmark.fail("benchmark-transition-did-not-animate");
            }
            orientation_full_redraw_pending = true;
            full_bridge_dirty = true;
        }
        if let Some(collection_id) = deferred_navigation_hydration_finish.take() {
            nav.catalog_system_hydration_finished(&collection_id);
            full_bridge_dirty = true;
        }
        if let Some(deadline) = display_confirm_deadline {
            nav.display_confirm_remaining = if loop_start >= deadline {
                0
            } else {
                ((deadline - loop_start).as_millis().div_ceil(1000) as u8)
                    .min(launcher::DISPLAY_CONFIRM_SECONDS)
            };
        }
        if let Some(deadline) = orientation_confirm_deadline {
            nav.orientation_confirm_remaining = if loop_start >= deadline {
                0
            } else {
                ((deadline - loop_start).as_millis().div_ceil(1000) as u8)
                    .min(launcher::DISPLAY_CONFIRM_SECONDS)
            };
            if loop_start >= deadline
                && nav.confirm_action == Some(launcher::ConfirmAction::ScreenOrientation)
            {
                if let Some(previous) = orientation_previous.take() {
                    let from = nav.settings.screen_orientation;
                    let animated = begin_orientation_transition(
                        &app,
                        window,
                        ui,
                        target,
                        from,
                        previous,
                        loop_start,
                        nav.settings.reduce_motion,
                        &mut nav,
                        &mut layout,
                        &mut layout_epoch,
                        &mut navigation_transition,
                        &mut full_screen_transition,
                        &mut orientation_transition_generation,
                        &mut orientation_transition,
                        &mut orientation_transition_intent,
                        &mut orientation_preparation_trace,
                        OrientationTransitionIntent::Rollback,
                    );
                    let _ = animated;
                }
                orientation_confirm_deadline = None;
                nav.confirm_action = None;
                nav.confirm_selected = 0;
                nav.orientation_confirm_remaining = 0;
                orientation_full_redraw_pending = true;
                full_bridge_dirty = true;
            }
        }
        while let Ok(result) = orientation_confirm_rx.try_recv() {
            nav.orientation_confirm_busy = false;
            match result {
                Ok(()) => {
                    orientation_previous = None;
                    nav.confirm_action = None;
                    nav.confirm_selected = 0;
                    nav.orientation_error = None;
                    nav.orientation_confirm_remaining = 0;
                }
                Err(error) => {
                    nav.confirm_action = Some(launcher::ConfirmAction::ScreenOrientation);
                    nav.confirm_selected = 1;
                    nav.orientation_error = Some(error);
                }
            }
            full_bridge_dirty = true;
            request_launcher_redraw!();
        }
        while let Ok(result) = display_confirm_rx.try_recv() {
            pacer.rearm_after_display_mode_change();
            nav.display_confirm_busy = false;
            match result {
                Ok(state) => {
                    if state.phase == launcher::DisplayTransactionPhase::Failed {
                        nav.confirm_action = Some(launcher::ConfirmAction::DisplayResolution);
                        nav.confirm_selected = 0;
                        nav.display_error = Some(
                            state
                                .error
                                .unwrap_or_else(|| "display persistence failed".to_string()),
                        );
                        nav.display_confirm_remaining = state.remaining.max(1);
                        display_confirm_deadline = Some(
                            Instant::now() + Duration::from_secs(u64::from(state.remaining.max(1))),
                        );
                    } else {
                        nav.confirm_action = None;
                        nav.display_error = None;
                        display_confirm_deadline = None;
                        if let Some(index) =
                            mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
                                .iter()
                                .position(|mode| mode.id == state.active)
                        {
                            nav.display_selected = index;
                            nav.display_highlighted =
                                launcher::settings_display_selection_index(index).unwrap_or(0);
                        }
                    }
                }
                Err(error) => {
                    nav.confirm_action = Some(launcher::ConfirmAction::DisplayResolution);
                    nav.confirm_selected = 0;
                    nav.display_error = Some(error);
                }
            }
            full_bridge_dirty = true;
            request_launcher_redraw!();
        }
        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("pre-input-timers-feedback", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "timers-feedback",
        );
        let frame_analytics_mode = frame_accounting.frame_analytics_mode();
        let cpu_loop_start = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let arcade_visual_index_at_loop_start = nav.arcade.visual_index;
        let arcade_filter_visual_index_at_loop_start = nav.arcade_filter.visual_index;
        let prepare_trace_enabled =
            frame_accounting.preview_scroll_trace_enabled() || frame_analytics_mode.records_wall();
        let mut prepare_trace = LauncherPrepareTrace::default();
        prepare_trace.slint_timer_dispatch_us = slint_timer_dispatch_us;
        if background_work_allowed
            && catalog_ready
            && user_state_catalog_version != Some(catalog_version)
        {
            let games = catalog
                .games
                .iter()
                .filter(|game| game.system_id.eq_ignore_ascii_case("snes"))
                .filter_map(|game| catalog.user_game_identity_for_ref(&game.mra_path))
                .collect();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|duration| i64::try_from(duration.as_secs()).ok())
                .unwrap_or(0);
            user_state_session.refresh(games, now);
            user_state_catalog_version = Some(catalog_version);
        }
        while background_work_allowed && let Some(event) = user_state_session.poll() {
            match event {
                UserStateEvent::Snapshot(snapshot) => {
                    nav.set_user_game_refs(
                        &catalog,
                        snapshot.favourite_launch_refs,
                        snapshot.recent_launch_refs,
                    );
                    full_bridge_dirty = true;
                    request_launcher_redraw!();
                }
                UserStateEvent::Failed { error, rollback } => {
                    if let Some((launch_ref, favourite)) = rollback {
                        nav.reconcile_favourite_state(&catalog, &launch_ref, favourite);
                    }
                    crate::ui_errln!("user-state: {error}");
                    full_bridge_dirty = true;
                    request_launcher_redraw!();
                }
            }
        }
        let return_was_waiting = lifecycle.startup_status().mode == StartupMode::ReturnFromGame
            && !lifecycle.startup_can_present_frame();
        lifecycle.tick_startup_reveal(loop_start, catalog_ready, &mut lifecycle_effects);
        if return_black_timeout_requires_home_fallback(return_was_waiting, &lifecycle_effects) {
            launch_return_session.fallback_to_home(&mut nav);
            full_bridge_dirty = true;
            request_launcher_redraw!();
        }
        apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
        if startup_intro.is_none() || startup_intro_needs_live_launcher {
            sync_startup_visibility(&app, &lifecycle);
        }
        scheduler.record_loading_frame(loop_start);
        if launcher_presenter.retry_latch_automatically(ui) {
            runtime_status::event(
                "launcher_latch_recovery",
                &format!(
                    "action=automatic-retry attempt={}",
                    launcher_presenter.retry_attempts()
                ),
            );
            request_launcher_redraw!();
        }
        if launcher_presenter.take_supervised_restart_request() {
            match launcher::request_supervised_launcher_restart() {
                Ok(()) => runtime_status::event(
                    "launcher_latch_recovery",
                    "action=supervised-restart-requested",
                ),
                Err(error) => runtime_status::event(
                    "launcher_latch_recovery",
                    &format!("action=supervised-restart-failed error={error}"),
                ),
            }
        }
        frame_accounting.set_display_frozen(launcher_presenter.display_frozen());
        let lifecycle_launch_active = matches!(
            lifecycle.state(),
            LauncherLifecycleState::Launching { .. } | LauncherLifecycleState::Handoff { .. }
        );
        if scheduler.recover_stale_launch_transport(lifecycle_launch_active) {
            runtime_status::event(
                "launcher_state_invariant_recovered",
                "kind=stale-launch-transport lifecycle=interactive",
            );
        }
        if lifecycle_launch_active && screensaver.cancel_for_exclusive_view(loop_start) {
            runtime_status::event(
                "launcher_state_invariant_recovered",
                "kind=screensaver-during-launch action=cancel-screensaver",
            );
            request_launcher_redraw!();
        }
        effective_view = EffectiveLauncherView::resolve(&lifecycle, screensaver.active, nav.screen);
        let mut launching = effective_view.launch_active();
        setup_active = setup.is_active();
        let loop_elapsed_ms = loop_start
            .saturating_duration_since(start)
            .as_millis()
            .min(u64::MAX as u128) as u64;
        if catalog_ready
            && lifecycle.startup_input_enabled()
            && system_entry_benchmark_settled(
                loop_elapsed_ms,
                lifecycle.startup_status().input_enabled_ms,
            )
            && effective_view.accepts_application_input()
            && nav.screen == Screen::Home
            && pending_collection_entry.is_none()
            && let Some(collection_id) = pending_system_entry_benchmark.take()
        {
            let requested_at = Instant::now();
            mister_magik_perf_events::clear_process_profiles();
            system_entry_cpu_profile = cpu_profile::start_system_entry(profile_config.cpu());
            arcade_entry_latency.capture_presentation_start(
                f.read_magik_presentation_telemetry().ok(),
                frame_accounting.last_latch_drop_count(),
            );
            if collection_has_resident_rows(&catalog, &collection_id) {
                arcade_entry_latency.record_collection_enter_input(
                    start,
                    requested_at,
                    &lifecycle,
                    &collection_id,
                    "benchmark-direct",
                    true,
                );
                if nav.open_system(&catalog, &collection_id) {
                    if nav.screen == Screen::SystemHub {
                        nav.set_arcade_user_list_mode(
                            &catalog,
                            launcher::ArcadeUserListMode::Games,
                        );
                        nav.screen = Screen::Arcade;
                    }
                    arcade_entry_latency.record_rows_ready(
                        start,
                        requested_at,
                        &lifecycle,
                        &catalog,
                        &nav,
                    );
                    full_bridge_dirty = true;
                    request_launcher_redraw!();
                }
            } else {
                let entry = begin_cold_collection_entry(
                    &mut scheduler,
                    &mut nav,
                    &mut preview,
                    &catalog,
                    catalog_version,
                    &collection_id,
                    requested_at,
                    "benchmark-direct",
                    true,
                    &mut arcade_entry_latency,
                    &lifecycle,
                    start,
                );
                full_bridge_dirty |= entry.bridge_dirty;
                pending_collection_entry = entry.pending;
            }
        }
        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("pre-input-lifecycle-state", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "lifecycle-state",
        );
        if early_input_phase_result.is_none() {
            pad_changed_for_input = if effective_view.accepts_application_input()
                && lifecycle.startup_input_enabled()
            {
                Some(pad.poll_with_debug_labels(setup_active))
            } else {
                None
            };
        }
        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("pre-input-raw-device-poll", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "raw-device-poll",
        );
        if background_work_allowed && let Some(sample) = memory_guard.tick(loop_start) {
            if sample.changed {
                runtime_status::event(
                    "memory_pressure",
                    &format!(
                        "active={} available_kib={} threshold_kib={}",
                        u8::from(sample.active),
                        sample.available_kib,
                        sample.threshold_kib
                    ),
                );
                if sample.active {
                    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                    preview.clear(&bridge);
                    apply_screenshot_media_update_effects(
                        media_session.pause_for_low_memory(media_benchmark_contention),
                        &app,
                        &mut catalog,
                        &mut scheduler,
                        Some(&mut preview),
                        &mut full_bridge_dirty,
                        start,
                    );
                    full_bridge_dirty = true;
                }
            }
        }
        if background_work_allowed {
            apply_screenshot_media_update_effects(
                media_session.clear_progress_if_due(loop_start),
                &app,
                &mut catalog,
                &mut scheduler,
                Some(&mut preview),
                &mut full_bridge_dirty,
                start,
            );
        }
        launcher_readiness.poll();
        let mut route_action = display_session.begin_frame(frames, launching, f);
        route_action.force_full_present |= launcher_readiness.needs_full_present();
        // The catalog contention harness first proves one exact preview, then
        // freezes further selected-preview work so frame failures can be
        // attributed to the catalog rather than an independent image decode.
        let defer_selected_preview =
            catalog_contention_quiet_previews && preview.trace_cache_state() == "exact";
        let mut preview_scheduled_this_loop = false;
        let clock_update_due =
            background_work_allowed && last_clock_update.elapsed() >= Duration::from_secs(1);
        let clock_update_start = clock_update_due.then(Instant::now);
        if clock_update_due {
            if startup_intro.is_some() {
                startup_intro_bridge_dirty_pending = true;
            } else if dirty_opt {
                let clock_text = launcher_clock_text();
                if clock_text != last_clock_text {
                    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                    bridge.set_clock_text(clock_text.clone().into());
                    last_clock_text = clock_text;
                    light_bridge_dirty = true;
                }
            } else {
                let clock_text = launcher_clock_text();
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                bridge.set_clock_text(clock_text.clone().into());
                last_clock_text = clock_text;
                full_bridge_dirty = true;
            }
            last_clock_update = Instant::now();
        }
        let clock_update_us = clock_update_start
            .map(|started| started.elapsed().as_micros())
            .unwrap_or(0);
        if background_work_allowed && let Some(available) = update_check.try_recv() {
            if available {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                bridge.set_update_available(true);
                light_bridge_dirty = true;
                runtime_status::event("update_available", "source=downloader_mister_magik");
            }
        }

        if input_observation_probe
            .as_ref()
            .is_some_and(|probe| probe.changed_since(input_observation))
        {
            background_work_allowed = false;
        }

        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("pre-input-readiness-maintenance", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "readiness-maintenance",
        );

        let catalog_worker_trace_start = prepare_trace_enabled.then(Instant::now);
        let slint_animation_active = app.window().has_active_animations();
        let startup_return_waiting_for_catalog = lifecycle.startup_waiting_for_return_catalog();
        if scheduler.system_entry_prepare_active() {
            background_work_allowed = false;
        }
        let catalog_interaction_active = scheduler.system_entry_prepare_active()
            || !background_work_allowed
            || directional_input_held
            || latency_critical_input_pending
            || navigation_transition.is_active()
            || orientation_transition.is_active()
            || full_screen_transition_owned_at_loop_start
            || nav.arcade.is_scroll_active()
            || (nav.arcade_filter.drawer_open && nav.arcade_filter.is_scroll_active());
        let catalog_work_mode = launcher_catalog_work_mode(
            frame_accounting.first_visible_copy_done(),
            catalog_interaction_active,
            startup_intro.is_some() || slint_animation_active,
            loop_start,
            &mut catalog_idle_candidate_since,
        );
        let catalog_work_epoch =
            mister_magik_catalog::builder_service::set_catalog_work_mode(catalog_work_mode);
        if catalog_work_telemetry.observe(catalog_work_mode, loop_start) {
            let gate = mister_magik_catalog::builder_service::catalog_work_gate_snapshot();
            crate::ui_logln!(
                "catalog_work_mode_tsv\tmode={:?}\tepoch={}\tinteraction={}\tvisible_animation={}\tparked_threads={}\tpark_count={}\tcheckpoints={}",
                catalog_work_mode,
                catalog_work_epoch,
                u8::from(catalog_interaction_active),
                u8::from(startup_intro.is_some() || slint_animation_active),
                gate.parked_threads,
                gate.park_count,
                gate.checkpoints,
            );
        }
        let catalog_worker_work_allowed = catalog_work_mode != CatalogWorkMode::Paused;
        scheduler.tick_catalog_progress(catalog_worker_work_allowed, loop_start);
        if background_work_allowed
            && let Some(request) = nav.take_arcade_search_request(&catalog, catalog_version)
        {
            scheduler.request_arcade_search(request);
        }
        let deferred_worker_policy = deferred_catalog_worker_start_policy(
            catalog_ready,
            frame_accounting.first_visible_copy_done(),
            startup_return_waiting_for_catalog,
            lifecycle.catalog_worker_start_delay(catalog_background_validation_delay()),
        );
        if background_work_allowed
            && let Some(worker) = catalog_session.maybe_start_deferred_worker(
                scheduler.catalog_worker_running(),
                frame_accounting.first_visible_copy_done() || startup_return_waiting_for_catalog,
                deferred_worker_policy.allowed && catalog_publication_test.catalog_worker_allowed(),
                loop_start,
                deferred_worker_policy.delay,
                catalog_builder_lock_available,
            )
        {
            print_startup_event(start, "catalog_worker_start", &worker.root);
            let lifecycle_input =
                deferred_catalog_worker_lifecycle_input(worker.execution_mode, worker.request);
            lifecycle.handle(lifecycle_input, &mut lifecycle_effects);
            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
            scheduler.start_catalog_worker(
                worker.root,
                worker.request,
                worker.initial_cache,
                worker.execution_mode,
            );
        }

        if background_work_allowed
            && let Some(message) = catalog_publication_test.tick(loop_start, start)
        {
            deferred_catalog_events.push_back(message);
        }
        let system_entry_handoff_only = should_poll_system_entry_handoff(
            background_work_allowed,
            pending_collection_entry.is_some(),
            launch_return_session.protects_hydrating_collection(&nav),
            scheduler.system_entry_prepare_active(),
        );
        let catalog_poll_scope = catalog_poll_scope(
            background_work_allowed,
            full_screen_transition_owned_at_loop_start,
            system_entry_handoff_only,
        );
        if let Some(catalog_poll_scope) = catalog_poll_scope
            && catalog_messages_need_polling(
                pending_catalog_ready.is_some(),
                catalog_session.refresh_done(),
                scheduler.catalog_messages_running() || !deferred_catalog_events.is_empty(),
            )
        {
            let catalog_disconnected =
                scheduler.poll_catalog(&mut catalog_events, catalog_poll_scope);
            deferred_catalog_events.extend(catalog_events.drain());

            let mut catalog_messages_processed = 0usize;
            if let Some(message) = pending_catalog_ready.take() {
                catalog_ready_stationary_edge_since = update_catalog_ready_stationary_edge_since(
                    &nav,
                    catalog_ready_stationary_edge_since,
                    loop_start,
                );
                if should_defer_catalog_message(
                    &message,
                    catalog_ready,
                    &nav,
                    catalog_ready_stationary_edge_since,
                    loop_start,
                ) {
                    let deferred_since = *catalog_ready_deferred_since.get_or_insert(loop_start);
                    pending_catalog_ready = Some(message);
                    prepare_trace.catalog_ready_deferred = true;
                    prepare_trace.catalog_ready_deferred_age_us = loop_start
                        .saturating_duration_since(deferred_since)
                        .as_micros();
                } else {
                    catalog_ready_deferred_since = None;
                    catalog_ready_stationary_edge_since = None;
                    process_catalog_worker_message(
                        message,
                        preview_route,
                        &mut prepare_trace,
                        &mut launcher_response_trace,
                        frame_accounting.first_visible_copy_done(),
                        launching,
                        benchmark_media_interaction_active,
                        media_benchmark_contention,
                        loop_start,
                        &app,
                        &mut nav,
                        &mut catalog,
                        &mut catalog_ready,
                        &mut catalog_version,
                        &mut return_capsule_active,
                        &mut catalog_generation,
                        &mut launch_return_session,
                        &mut preview,
                        &mut media_session,
                        &mut scheduler,
                        &mut catalog_session,
                        &mut lifecycle,
                        &mut lifecycle_effects,
                        &mut full_bridge_dirty,
                        &mut startup_intro_catalog_ui_replay,
                        &mut startup_intro_catalog_shells_pending,
                        startup_intro.is_some(),
                        start,
                    );
                    catalog_messages_processed = catalog_messages_processed.saturating_add(1);
                }
            }

            while catalog_messages_processed < CATALOG_MESSAGES_PER_FRAME {
                let Some(message) = deferred_catalog_events.pop_front() else {
                    break;
                };
                catalog_ready_stationary_edge_since = update_catalog_ready_stationary_edge_since(
                    &nav,
                    catalog_ready_stationary_edge_since,
                    loop_start,
                );
                if should_defer_catalog_message(
                    &message,
                    catalog_ready,
                    &nav,
                    catalog_ready_stationary_edge_since,
                    loop_start,
                ) {
                    let deferred_since = *catalog_ready_deferred_since.get_or_insert(loop_start);
                    if pending_catalog_ready.is_none() {
                        pending_catalog_ready = Some(message);
                    } else {
                        deferred_catalog_events.push_front(message);
                        break;
                    }
                    prepare_trace.catalog_ready_deferred = true;
                    prepare_trace.catalog_ready_deferred_age_us = loop_start
                        .saturating_duration_since(deferred_since)
                        .as_micros();
                    continue;
                }
                process_catalog_worker_message(
                    message,
                    preview_route,
                    &mut prepare_trace,
                    &mut launcher_response_trace,
                    frame_accounting.first_visible_copy_done(),
                    launching,
                    benchmark_media_interaction_active,
                    media_benchmark_contention,
                    loop_start,
                    &app,
                    &mut nav,
                    &mut catalog,
                    &mut catalog_ready,
                    &mut catalog_version,
                    &mut return_capsule_active,
                    &mut catalog_generation,
                    &mut launch_return_session,
                    &mut preview,
                    &mut media_session,
                    &mut scheduler,
                    &mut catalog_session,
                    &mut lifecycle,
                    &mut lifecycle_effects,
                    &mut full_bridge_dirty,
                    &mut startup_intro_catalog_ui_replay,
                    &mut startup_intro_catalog_shells_pending,
                    startup_intro.is_some(),
                    start,
                );
                catalog_messages_processed = catalog_messages_processed.saturating_add(1);
            }
            let authoritative_ready_queued = pending_catalog_ready
                .as_ref()
                .is_some_and(|message| matches!(message, CatalogWorkerMessage::Ready { .. }))
                || deferred_catalog_events
                    .iter()
                    .any(|message| matches!(message, CatalogWorkerMessage::Ready { .. }));
            if catalog_disconnected && return_capsule_active && !authoritative_ready_queued {
                process_catalog_worker_message(
                    CatalogWorkerMessage::LoadFailed {
                        error: "catalog worker disconnected before authoritative hydration"
                            .to_string(),
                    },
                    preview_route,
                    &mut prepare_trace,
                    &mut launcher_response_trace,
                    frame_accounting.first_visible_copy_done(),
                    launching,
                    benchmark_media_interaction_active,
                    media_benchmark_contention,
                    loop_start,
                    &app,
                    &mut nav,
                    &mut catalog,
                    &mut catalog_ready,
                    &mut catalog_version,
                    &mut return_capsule_active,
                    &mut catalog_generation,
                    &mut launch_return_session,
                    &mut preview,
                    &mut media_session,
                    &mut scheduler,
                    &mut catalog_session,
                    &mut lifecycle,
                    &mut lifecycle_effects,
                    &mut full_bridge_dirty,
                    &mut startup_intro_catalog_ui_replay,
                    &mut startup_intro_catalog_shells_pending,
                    startup_intro.is_some(),
                    start,
                );
            }
            prepare_trace.catalog_backlog = deferred_catalog_events
                .len()
                .saturating_add(usize::from(pending_catalog_ready.is_some()))
                .min(u32::MAX as usize) as u32;
            if deferred_catalog_events.is_empty() && pending_catalog_ready.is_none() {
                catalog_ready_deferred_since = None;
                catalog_ready_stationary_edge_since = None;
            }
        }
        if let Some(trace_start) = catalog_worker_trace_start {
            prepare_trace.catalog_worker_us = trace_start.elapsed().as_micros();
        }
        scheduler_phase =
            launcher_response_trace.record_scheduler_interval("pre-input-catalog", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "catalog",
        );
        if maybe_present_modal_input_test_dialog(
            &mut modal_input_test_dialog_pending,
            catalog_ready,
            &mut lifecycle,
            &mut lifecycle_effects,
            &mut scheduler,
            start,
        ) {
            full_bridge_dirty = true;
            request_launcher_redraw!();
        }
        let media_worker_trace_start = prepare_trace_enabled.then(Instant::now);
        let mut media_message_seen = false;
        if preview_route.allows_preview_work() && background_work_allowed {
            scheduler.poll_media(&mut media_events);
            for message in media_events.drain() {
                media_message_seen = true;
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                let catalog_scan_visible = bridge.get_catalog_scan_visible();
                let effects =
                    media_session.handle_worker_message(message, catalog_scan_visible, loop_start);
                apply_screenshot_media_update_effects(
                    effects,
                    &app,
                    &mut catalog,
                    &mut scheduler,
                    Some(&mut preview),
                    &mut full_bridge_dirty,
                    start,
                );
            }
        }
        if let Some(trace_start) = media_worker_trace_start {
            prepare_trace.media_worker_us = trace_start.elapsed().as_micros();
        }
        scheduler_phase =
            launcher_response_trace.record_scheduler_interval("pre-input-media", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "media",
        );

        if let Some(completion) = scheduler.poll_launch_completion(Instant::now()) {
            match completion {
                LaunchHandoffCompletion::Success { benchmark_terminal } => {
                    let input = if benchmark_terminal {
                        LauncherLifecycleInput::BenchmarkLaunchCompleted
                    } else {
                        LauncherLifecycleInput::LaunchSucceeded {
                            spawned_mister: false,
                        }
                    };
                    lifecycle.handle(input, &mut lifecycle_effects);
                    apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                }
                LaunchHandoffCompletion::Failure { title, error } => {
                    lifecycle.handle(
                        LauncherLifecycleInput::LaunchFailed {
                            title,
                            kind: error.kind(),
                            detail: error.to_string(),
                        },
                        &mut lifecycle_effects,
                    );
                    apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                    if scheduler.stop_spawned_mister_for_recovery() {
                        if let Err(e) = display_session.recover_after_launch_failure(frames, f) {
                            crate::ui_errln!(
                                "failed to recover Slint framebuffer route after launch failure: {e}"
                            );
                        }
                    }
                    sync_bridge_launcher(
                        &app,
                        &pad,
                        &nav,
                        &lifecycle,
                        &setup,
                        "",
                        "",
                        &catalog,
                        &mut preview,
                        &mut bridge_models,
                        catalog_version,
                        false,
                        false,
                        ui,
                    );
                    update_slint_animations(animation_clock);
                    let recovery_rect = render_immediate_launcher_frame(window, target, layout);
                    if let Some(rect) = recovery_rect {
                        let _ = copy_cached_rect_565(disp, target.cached_frame_view(), rect);
                    } else {
                        copy_cached_rows_565(disp, target.cached_frame_view(), 0, ui.render_h());
                    }
                    let recovery_presented = Instant::now();
                    request_launcher_redraw!();
                    scheduler.finish_launch_failure_recovery(recovery_presented);
                    lifecycle.recovery_frame_presented(recovery_presented, &mut lifecycle_effects);
                    apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                    record_launcher_frame_phase!(LauncherFramePhase::LaunchRecoveryApplied);
                    crate::ui_errln!("game launch failed: {error}");
                }
            }
        }
        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("pre-input-launch-lifecycle", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "launch-lifecycle",
        );

        if arcade_screen_pending && arcade_navigation_ready(catalog_ready, &catalog) {
            let before = LauncherBridgeKey::from_nav(&nav);
            if nav.active_collection().is_none() {
                let _ = nav.open_default_arcade(&catalog);
            } else {
                nav.screen = Screen::Arcade;
            }
            arcade_screen_pending = false;
            full_bridge_dirty = true;
            let after = LauncherBridgeKey::from_nav(&nav);
            if before != after {
                media_session.note_nav_change(&before, &after, Instant::now());
            }
        }

        if !navigation_transition.is_active()
            && commit_pending_collection_entry(
                &mut pending_collection_entry,
                &mut nav,
                &catalog,
                start,
            )
        {
            arcade_entry_latency.record_rows_ready(start, loop_start, &lifecycle, &catalog, &nav);
            full_bridge_dirty = true;
            request_launcher_redraw!();
        } else if restore_failed_pending_collection_entry(
            &mut pending_collection_entry,
            &mut nav,
            start,
        ) {
            preview.cancel_system_entry_preview();
            arcade_entry_latency.cancel_enter();
            full_bridge_dirty = true;
            if navigation_transition.is_active() {
                let now_us = loop_start
                    .saturating_duration_since(start)
                    .as_micros()
                    .min(u64::MAX as u128) as u64;
                navigation_transition.request_reverse(now_us);
            }
        }

        if navigation_transition.is_active() {
            let now_us = loop_start
                .saturating_duration_since(start)
                .as_micros()
                .min(u64::MAX as u128) as u64;
            navigation_transition.tick(now_us);
            let should_commit = pending_navigation_transition
                .as_ref()
                .is_some_and(|pending| {
                    if pending.committed {
                        return false;
                    }
                    pending.event.action != LauncherAction::OpenCollection
                        || pending_collection_entry.as_ref().is_none_or(|entry| {
                            collection_has_resident_rows(&catalog, &entry.collection_id)
                        })
                });
            if should_commit {
                let navigation_commit_started = Instant::now();
                let event = pending_navigation_transition
                    .as_ref()
                    .map(|pending| pending.event.clone())
                    .expect("checked pending transition");
                let before = LauncherBridgeKey::from_nav(&nav);
                let committing_cold_collection = event.action == LauncherAction::OpenCollection
                    && pending_collection_entry.is_some();
                let committed = if committing_cold_collection {
                    commit_pending_collection_entry(
                        &mut pending_collection_entry,
                        &mut nav,
                        &catalog,
                        start,
                    )
                } else {
                    nav.commit_navigation_intent(&event, &catalog)
                };
                if committed {
                    if committing_cold_collection {
                        arcade_entry_latency
                            .record_rows_ready(start, loop_start, &lifecycle, &catalog, &nav);
                    }
                    if let Some(pending) = pending_navigation_transition.as_mut() {
                        pending.committed = true;
                    }
                    let after = LauncherBridgeKey::from_nav(&nav);
                    if before != after {
                        media_session.note_nav_change(&before, &after, Instant::now());
                    }
                    full_bridge_dirty = true;
                    request_launcher_redraw!();
                } else if event.action != LauncherAction::OpenCollection
                    || pending_collection_entry.is_none()
                {
                    navigation_transition.request_reverse(now_us);
                }
                prepare_trace.navigation_commit_us = prepare_trace
                    .navigation_commit_us
                    .saturating_add(navigation_commit_started.elapsed().as_micros());
            }
            request_launcher_redraw!();
        }

        if let Some(menu_id) = pending_start_menu.take() {
            if catalog_ready {
                let before = LauncherBridgeKey::from_nav(&nav);
                if nav.open_menu(&menu_id) {
                    print_startup_event(
                        start,
                        "launcher_start_menu_applied",
                        format!("menu={menu_id}"),
                    );
                    let after = LauncherBridgeKey::from_nav(&nav);
                    if before != after {
                        media_session.note_nav_change(&before, &after, Instant::now());
                        full_bridge_dirty = true;
                    }
                } else {
                    print_startup_event(
                        start,
                        "launcher_start_menu_fallback",
                        format!("menu={menu_id} reason=missing-or-empty"),
                    );
                    nav.go_root();
                    full_bridge_dirty = true;
                }
            } else {
                pending_start_menu = Some(menu_id);
            }
        }

        if let Some(system_id) = pending_start_system.take() {
            if arcade_navigation_ready(catalog_ready, &catalog) {
                let before = LauncherBridgeKey::from_nav(&nav);
                if apply_start_system_from_env(
                    &mut nav,
                    &catalog,
                    &system_id,
                    ui_frame_target::forced_arcade_selected_index(),
                ) {
                    print_startup_event(
                        start,
                        "launcher_start_system_applied",
                        format!("system={system_id}"),
                    );
                    let after = LauncherBridgeKey::from_nav(&nav);
                    if before != after {
                        media_session.note_nav_change(&before, &after, Instant::now());
                        full_bridge_dirty = true;
                    }
                } else {
                    print_startup_event(
                        start,
                        "launcher_start_system_fallback",
                        format!("system={system_id} reason=missing"),
                    );
                    nav.go_root();
                    full_bridge_dirty = true;
                }
            } else {
                pending_start_system = Some(system_id);
            }
        }

        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("pre-input-navigation", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "navigation",
        );

        latch_v5_qualification.poll_control(loop_start);
        latch_v5_qualification.observe_catalog_worker(
            scheduler.catalog_worker_running(),
            catalog_session.refresh_done(),
        );
        if latch_v5_qualification.take_catalog_request(scheduler.catalog_worker_running()) {
            let effects = catalog_session.qualification_fresh_rebuild(arcade_root.clone());
            apply_catalog_session_effects(
                effects,
                preview_route,
                &mut launcher_response_trace,
                &app,
                &mut nav,
                &mut catalog,
                &mut catalog_ready,
                &mut catalog_version,
                &mut return_capsule_active,
                &mut catalog_generation,
                &mut launch_return_session,
                &mut preview,
                &mut media_session,
                &mut scheduler,
                &mut lifecycle,
                &mut lifecycle_effects,
                &mut full_bridge_dirty,
                &mut startup_intro_catalog_ui_replay,
                &mut startup_intro_catalog_shells_pending,
                false,
                loop_start,
                start,
            );
            request_launcher_redraw!();
        }
        if latch_v5_qualification.enabled()
            && launcher_presenter.latch_failure().is_none()
            && arcade_navigation_ready(catalog_ready, &catalog)
            && let Some(scenario) = latch_v5_qualification.stress_class().bench_scenario()
        {
            let before = LauncherBridgeKey::from_nav(&nav);
            if launcher_bench_step(
                scenario,
                &benchmark_config,
                &mut nav,
                &catalog,
                None,
                &mut latch_v5_bench_state,
                loop_start,
            ) {
                latch_v5_bench_state.advance_if(true);
                let after = LauncherBridgeKey::from_nav(&nav);
                if before != after {
                    media_session.note_nav_change(&before, &after, loop_start);
                    full_bridge_dirty = true;
                }
                request_launcher_redraw!();
            }
        }

        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("pre-input-qualification", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "qualification",
        );

        if let Some(scenario) = launcher_bench_scenario {
            let latch_failure_active = launcher_presenter.latch_failure().is_some();
            let after_input_script_ready = match scenario {
                LauncherBenchScenario::ScreensaverShow => screensaver.active,
                _ => {
                    nav.screen == Screen::Arcade && arcade_navigation_ready(catalog_ready, &catalog)
                }
            };
            if launcher_bench_after_input_script
                && !launcher_bench_active
                && !launcher_input_script.active()
                && after_input_script_ready
            {
                run_start = Instant::now();
                frame_accounting.close_preview_scroll_trace_for_restart();
                frame_accounting = LauncherFrameAccounting::new(
                    run_start,
                    ui.output_route().label(),
                    ui.crt_font_experiment().label(),
                    ui.fb_w(),
                    ui.fb_h(),
                    profile_config.frame().fps_log_enabled(),
                );
                launcher_bench_active = true;
                launcher_bench_waiting_for_initial_preview = false;
                launcher_bench_next_step = run_start;
                preview_scroll_exit_at = preview_scroll_exit_after_trace_deadline(run_start);
                arcade_entry_latency
                    .record_first_nav_input(start, run_start, &lifecycle, &catalog, &nav);
                print_startup_event(
                    start,
                    "launcher_bench_after_input_script_start",
                    format!("scenario={}", scenario.label()),
                );
            }
            let catalog_ready_for_bench = if scenario.starts_on_arcade() {
                arcade_navigation_ready(catalog_ready, &catalog)
            } else {
                catalog_ready
            };
            if launcher_bench_active
                && !latch_failure_active
                && catalog_ready_for_bench
                && launcher_bench_waiting_for_initial_preview
            {
                let cache_state = preview.trace_cache_state();
                let selected_has_preview = selected_arcade_game_has_preview(&nav, &catalog);
                if launcher_bench_initial_preview_ready(scenario, cache_state, selected_has_preview)
                {
                    launcher_bench_waiting_for_initial_preview = false;
                    launcher_bench_next_step = Instant::now();
                    print_startup_event(
                        start,
                        "launcher_bench_preview_ready",
                        format!("cache_state={cache_state}"),
                    );
                }
            }
            if launcher_bench_active
                && !latch_failure_active
                && catalog_ready_for_bench
                && !launcher_bench_waiting_for_initial_preview
                && launcher_bench_next_step.elapsed() >= scenario.period()
            {
                let before = LauncherBridgeKey::from_nav(&nav);
                let bench_step_ran = launcher_bench_step(
                    scenario,
                    &benchmark_config,
                    &mut nav,
                    &catalog,
                    None,
                    &mut launcher_bench_state,
                    Instant::now(),
                );
                if bench_step_ran {
                    let after = LauncherBridgeKey::from_nav(&nav);
                    if before != after {
                        media_session.note_nav_change(&before, &after, Instant::now());
                        if !dirty_opt
                            || before.screen != after.screen
                            || before.menu_id != after.menu_id
                        {
                            full_bridge_dirty = true;
                        } else {
                            light_bridge_dirty = true;
                        }
                    }
                }
                launcher_bench_state.advance_if(bench_step_ran);
                launcher_bench_next_step = Instant::now();
            }
        }

        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("pre-input-benchmark", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "benchmark",
        );

        if let Some(screen) = effective_lock_screen(lock_screen, catalog_ready, &catalog) {
            nav.screen = screen;
        }

        let catalog_build_busy = screensaver_catalog_busy(
            scheduler.catalog_worker_running(),
            catalog_session.refresh_done(),
        );
        screensaver.set_qualification_particles(
            loop_start,
            latch_v5_qualification.enabled(),
            latch_v5_qualification.stress_class() == LatchV5StressClass::Particles,
        );
        let restore_before = screensaver.restore_full_frame;
        let preview_was_active = screensaver.is_preview();
        screensaver.update(
            Instant::now(),
            nav.settings.screensaver_enabled,
            Duration::from_secs(u64::from(nav.settings.screensaver_delay_minutes) * 60),
            catalog_build_busy,
            screensaver_preview_start_ready(
                catalog_ready,
                screensaver_preview_waits_for_analytics,
                frame_accounting.frame_analytics_mode(),
            ),
        );
        if !preview_was_active && screensaver.is_preview() {
            let started = Instant::now();
            screensaver_show_started = Some(started);
            screensaver_first_render_logged = false;
            screensaver_first_present_logged = false;
            screensaver_first_card_present_logged = false;
            crate::ui_logln!(
                "screensaver_startup_timing milestone=show_pressed elapsed_us=0 source=start-preview"
            );
        }
        if !restore_before && screensaver.restore_full_frame {
            request_launcher_redraw!();
        }
        effective_view = EffectiveLauncherView::resolve(&lifecycle, screensaver.active, nav.screen);
        launching = effective_view.launch_active();
        frame_accounting.set_effective_view(effective_view.label());
        frame_accounting.set_catalog_generation(catalog_generation.current.as_deref());
        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        if (startup_intro.is_none() || startup_intro_needs_live_launcher)
            && bridge.get_effective_view().as_str() != effective_view.label()
        {
            bridge.set_effective_view(effective_view.label().into());
        }

        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("pre-input-view-housekeeping", scheduler_phase);
        note_early_input_change(
            route_input_early,
            input_observation_probe.as_ref(),
            input_observation,
            &mut early_input_change_checkpoint,
            "view-housekeeping",
        );
        record_launcher_frame_phase!(LauncherFramePhase::PreInputMaintenance);
        let (input_phase_yielded, input_batch_empty) =
            if let Some(result) = early_input_phase_result.take() {
                result
            } else {
                run_launcher_input_phase!(
                    'launcher,
                    scheduler_phase,
                    loop_start,
                    route_input_early,
                    pad_changed_for_input,
                    setup_active,
                    effective_view,
                    full_bridge_dirty,
                    light_bridge_dirty,
                    early_input_change_checkpoint
                )
            };
        if input_phase_yielded {
            continue;
        }
        let interaction_projection_pmu = launcher_response_trace.input_pmu_span(
            latency_critical_input_pending,
            "launcher-response.interaction-projection",
        );

        if empty_collection_invariant_violated(&catalog, &nav)
            && !launch_return_session.protects_hydrating_collection(&nav)
        {
            if let Some(system) = active_system(&catalog, &nav) {
                crate::ui_errln!(
                    "catalog presentation invariant recovered: system={} registered_rows={} resident_rows=0",
                    system.id,
                    system.count
                );
                runtime_status::event(
                    "catalog_empty_list_invariant",
                    format!("system={} registered_rows={}", system.id, system.count),
                );
            }
            nav.recover_empty_collection_to_home();
            full_bridge_dirty = true;
            request_launcher_redraw!();
        }

        let startup_intro_launcher_ui_plan = startup_intro_launcher_ui_plan(
            startup_intro.is_some(),
            lifecycle.startup_status().state,
            startup_intro_launcher_frame_ready,
        );
        let startup_intro_prepare_live_launcher =
            startup_intro_launcher_ui_plan == StartupIntroLauncherUiPlan::PrepareLiveFrame;
        let startup_intro_suppress_launcher_ui =
            startup_intro_launcher_ui_plan == StartupIntroLauncherUiPlan::Suppress;
        if startup_intro_suppress_launcher_ui {
            startup_intro_bridge_dirty_pending |= full_bridge_dirty || light_bridge_dirty;
            full_bridge_dirty = false;
            light_bridge_dirty = false;
        } else {
            if std::mem::take(&mut startup_intro_bridge_dirty_pending)
                || startup_intro_prepare_live_launcher
            {
                full_bridge_dirty = true;
            }
            if startup_intro_prepare_live_launcher {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                LauncherStatusPresenter::new(&bridge).clear_catalog_scan();
                let clock_text = launcher_clock_text();
                bridge.set_clock_text(clock_text.clone().into());
                last_clock_text = clock_text;
                last_clock_update = Instant::now();
                window.request_redraw();
            }
            sync_settings_bridge(&app, &nav, &lifecycle);
        }
        let source_was_arcade = pending_navigation_transition
            .as_ref()
            .is_some_and(|pending| pending.source_was_arcade);
        let preserve_navigation_source_preview =
            navigation_transition.is_active() && source_was_arcade;
        let defer_or_preserve_selected_preview = should_defer_or_preserve_selected_preview(
            defer_selected_preview,
            navigation_transition.is_active(),
            source_was_arcade,
        );
        let bridge_sync_plan = launcher_bridge_sync_plan(
            launching,
            lifecycle.startup_input_enabled(),
            full_bridge_dirty,
            light_bridge_dirty,
        );
        let bridge_sync_started =
            (bridge_sync_plan != LauncherBridgeSyncPlan::None).then(Instant::now);
        let gui_bridge_phase = gui_bridge_profile_phase(
            bridge_sync_plan == LauncherBridgeSyncPlan::Full,
            bridge_sync_plan == LauncherBridgeSyncPlan::Light,
        );
        let gui_bridge_pmu = gui_profiling.phase_span(gui_bridge_phase.span_name());
        let mut bridge_model_projection_us = 0u128;
        match bridge_sync_plan {
            LauncherBridgeSyncPlan::Full => {
                bridge_model_projection_us = sync_bridge_launcher(
                    &app,
                    &pad,
                    &nav,
                    &lifecycle,
                    &setup,
                    scheduler.visible_loading_title(&loading_title),
                    "",
                    &catalog,
                    &mut preview,
                    &mut bridge_models,
                    catalog_version,
                    defer_or_preserve_selected_preview,
                    system_entry_cpu_profile.is_some(),
                    ui,
                )
                .model_projection_us;
                preview_scheduled_this_loop =
                    nav.screen == Screen::Arcade && preview_route.allows_hdmi_preview();
                request_launcher_redraw!();
            }
            LauncherBridgeSyncPlan::Light => {
                let active_games = if nav.screen == Screen::Arcade {
                    Some(active_system_game_view(&catalog, &nav))
                } else {
                    None
                };
                bridge_model_projection_us = sync_bridge_launcher_light(
                    &app,
                    &nav,
                    &lifecycle,
                    &mut bridge_models,
                    &setup,
                    scheduler.visible_loading_title(&loading_title),
                    "",
                    &catalog,
                    active_games,
                    &mut preview,
                    should_defer_arcade_overlay_bridge(dirty_opt, launching, &nav, &catalog),
                    defer_or_preserve_selected_preview,
                    system_entry_cpu_profile.is_some(),
                    ui,
                )
                .model_projection_us;
                preview_scheduled_this_loop =
                    nav.screen == Screen::Arcade && preview_route.allows_hdmi_preview();
                request_launcher_redraw!();
            }
            LauncherBridgeSyncPlan::None => {}
        }
        drop(gui_bridge_pmu);
        prepare_trace.bridge_sync_us = bridge_sync_started
            .map(|started| started.elapsed().as_micros())
            .unwrap_or(0);
        prepare_trace.bridge_model_projection_us = bridge_model_projection_us;
        let response_projected_at_us = crate::input_hub::monotonic_us();
        let response_projected_execution = launcher_response_trace.execution_stamp();
        drop(interaction_projection_pmu);
        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("interaction-projection", scheduler_phase);
        if !startup_intro_suppress_launcher_ui {
            sync_startup_visibility(&app, &lifecycle);
        }

        let media_gate_trace_start = prepare_trace_enabled.then(Instant::now);
        if background_work_allowed {
            let media_gate = media_session.current_gate(
                frame_accounting.first_visible_copy_done(),
                scheduler.has_pending_launch() || launching,
                benchmark_media_interaction_active,
                media_benchmark_contention,
                loop_start,
            );
            let media_gate = if nav.uses_crt_layout() {
                MediaInteractionGate {
                    active: true,
                    reason: "crt-no-screenshots",
                }
            } else if memory_guard.active() {
                MediaInteractionGate {
                    active: true,
                    reason: "low-memory",
                }
            } else {
                media_gate
            };
            let media_gate = catalog_build_media_gate(catalog_session.refresh_done(), media_gate);
            apply_screenshot_media_update_effects(
                media_session.sync_gate(media_gate),
                &app,
                &mut catalog,
                &mut scheduler,
                Some(&mut preview),
                &mut full_bridge_dirty,
                start,
            );
            apply_screenshot_media_update_effects(
                media_session.apply_gate(media_gate),
                &app,
                &mut catalog,
                &mut scheduler,
                Some(&mut preview),
                &mut full_bridge_dirty,
                start,
            );
            apply_screenshot_media_update_effects(
                media_session.sync_gate(media_gate),
                &app,
                &mut catalog,
                &mut scheduler,
                Some(&mut preview),
                &mut full_bridge_dirty,
                start,
            );
        }
        if let Some(trace_start) = media_gate_trace_start {
            prepare_trace.media_gate_us = trace_start.elapsed().as_micros();
        }

        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        let catalog_scan_visible = bridge.get_catalog_scan_visible();
        let catalog_scan_percent = bridge.get_catalog_scan_percent();
        let catalog_background_scan_visible = bridge.get_catalog_background_scan_visible();
        if let Some(dot_visible) = catalog_scan_blink.update(
            catalog_scan_visible || catalog_background_scan_visible,
            loop_start,
        ) {
            bridge.set_catalog_scan_dot_visible(dot_visible);
            request_launcher_redraw!();
        }
        let confirm_visible = bridge.get_confirm_visible();
        let confirm_selected = bridge.get_confirm_selected();
        let status_write_due = frame_accounting.status_write_due();
        let status_snapshot_due = status_write_due
            && !navigation_transition.is_active()
            && !full_screen_transition_owns_cpu1(full_screen_transition.state());
        let status_string_copy_start = (status_snapshot_due
            && frame_accounting.preview_scroll_trace_enabled())
        .then(Instant::now);
        let status_text =
            status_snapshot_due.then(|| LauncherStatusTextSnapshot::from_bridge(&bridge));
        let status_string_copy_us = status_string_copy_start
            .map(|start| start.elapsed().as_micros())
            .unwrap_or(0);
        prepare_trace.status_string_copy_us = status_string_copy_us;
        let status_string_copy_bytes = status_text
            .as_ref()
            .map(LauncherStatusTextSnapshot::bytes_len)
            .unwrap_or(0);
        if launching {
            request_launcher_redraw!();
        }
        let active_arcade_games = if !launching && nav.screen == Screen::Arcade {
            active_system_game_view(&catalog, &nav)
        } else {
            ArcadeGameView::empty()
        };
        let active_arcade_games_available = !active_arcade_games.is_empty();
        let arcade_search_active = nav.arcade_search.is_active(&nav.arcade_filter.active);
        if !launching && nav.screen == Screen::Arcade {
            if let Some(system) = active_system(&catalog, &nav) {
                let trace_system_id = &system.legacy_system_id;
                if preview_systems_entered.insert(trace_system_id.clone()) {
                    crate::ui_logln!(
                        "startup_timing\tpreview_system_entered\t{}ms\tsystem={}\tselected_index={}",
                        start.elapsed().as_millis(),
                        trace_system_id,
                        nav.arcade.selected
                    );
                }
                if active_arcade_games_available
                    && preview_initial_lists_ready.insert(trace_system_id.clone())
                {
                    arcade_entry_latency.record_rows_ready(
                        start,
                        Instant::now(),
                        &lifecycle,
                        &catalog,
                        &nav,
                    );
                    let selected = nav.arcade.selected.min(active_arcade_games.len() - 1);
                    if let Some(game) = active_arcade_games.get(selected) {
                        crate::ui_logln!(
                            "startup_timing\tpreview_initial_list_ready\t{}ms\tsystem={}\tselected_index={}\ttitle={}\thas_preview={}\tasset_key={}",
                            start.elapsed().as_millis(),
                            trace_system_id,
                            selected,
                            game.title,
                            if game.has_preview { 1 } else { 0 },
                            game.preview_asset_key
                        );
                    } else {
                        crate::ui_logln!(
                            "startup_timing\tpreview_initial_list_ready\t{}ms\tsystem={}\tselected_index={}\ttitle=\thas_preview=0\tasset_key=",
                            start.elapsed().as_millis(),
                            trace_system_id,
                            selected
                        );
                    }
                }
            }
        }
        let arcade_scroll_active = nav.screen == Screen::Arcade && nav.arcade.is_scroll_active();
        let arcade_turbo_active = nav.screen == Screen::Arcade && nav.arcade.is_turbo_active();
        let preview_work_allowed = preview_work_allowed(
            background_work_allowed,
            arcade_entry_latency.preview_adoption_in_progress(),
            arcade_scroll_active,
            arcade_turbo_active,
        );
        let preview_schedule_trace_start = prepare_trace_enabled.then(Instant::now);
        if dirty_opt
            && preview_work_allowed
            && !preview_scheduled_this_loop
            && !launching
            && preview_route.allows_preview_work()
            && nav.screen == Screen::Arcade
            && active_arcade_games_available
            && !arcade_search_active
            && !memory_guard.active()
        {
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            if schedule_arcade_preview_window(
                &bridge,
                active_arcade_games,
                nav.arcade.selected,
                &mut preview,
                defer_or_preserve_selected_preview,
                arcade_scroll_active,
                arcade_turbo_active,
            ) {
                request_launcher_redraw!();
            }
        }
        if let Some(trace_start) = preview_schedule_trace_start {
            prepare_trace.preview_schedule_us = trace_start.elapsed().as_micros();
        }
        let preview_apply_trace_start = prepare_trace_enabled.then(Instant::now);
        let mut preview_apply_trace = PreviewApplyTrace::default();
        let preview_apply_dirty = if !launching
            && preview_work_allowed
            && !arcade_search_active
            && !memory_guard.active()
            && preview_route.allows_preview_work()
        {
            let dirty = apply_ready_preview(
                &app,
                &mut preview,
                defer_or_preserve_selected_preview,
                arcade_turbo_active,
            );
            preview_apply_trace = preview.last_apply_trace();
            dirty
        } else {
            false
        };
        if preview_apply_dirty {
            request_launcher_redraw!();
        }
        if let Some(trace_start) = preview_apply_trace_start {
            prepare_trace.preview_apply_us = trace_start.elapsed().as_micros();
        }
        prepare_trace.preview_worker_drained = preview_apply_trace.worker_drained;
        prepare_trace.preview_ready_processed = preview_apply_trace.ready_processed;
        prepare_trace.preview_selected_processed = preview_apply_trace.selected_processed;
        prepare_trace.preview_prefetch_processed = preview_apply_trace.prefetch_processed;
        prepare_trace.preview_stale_results = preview_apply_trace.stale_results;
        prepare_trace.preview_cache_inserts = preview_apply_trace.cache_inserts;
        prepare_trace.preview_cache_evictions = preview.take_frame_cache_evictions();
        prepare_trace.preview_failed_results = preview_apply_trace.failed_results;
        prepare_trace.preview_backlog = preview_apply_trace.backlog_len;
        arcade_entry_latency.record_preview_exact(
            start,
            Instant::now(),
            &lifecycle,
            &catalog,
            &nav,
            &preview,
        );
        maybe_mark_return_preview_ready(
            &mut lifecycle,
            &mut lifecycle_effects,
            &nav,
            &catalog,
            &preview,
            &mut launch_return_session,
        );
        apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
        if !startup_intro_suppress_launcher_ui {
            sync_startup_visibility(&app, &lifecycle);
        }
        let startup_reveal_ready =
            lifecycle.startup_status().state == StartupRevealState::RevealLauncher;
        effective_view = EffectiveLauncherView::resolve(&lifecycle, screensaver.active, nav.screen);
        if effective_view.launch_active() && screensaver.cancel_for_exclusive_view(Instant::now()) {
            effective_view = EffectiveLauncherView::Launching;
            request_launcher_redraw!();
        }
        launching = effective_view.launch_active();
        frame_accounting.set_effective_view(effective_view.label());
        frame_accounting.set_catalog_generation(catalog_generation.current.as_deref());
        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        if !startup_intro_suppress_launcher_ui
            && bridge.get_effective_view().as_str() != effective_view.label()
        {
            bridge.set_effective_view(effective_view.label().into());
        }
        let mut full_frame_present = std::mem::take(&mut orientation_full_redraw_pending)
            || std::mem::take(&mut unpublished_cached_frame_present)
            || display_session.should_present_full_frame(launching, route_action)
            || startup_reveal_ready;
        let wants_arcade_list = !screensaver.active
            && should_draw_arcade_overlay(&nav, launching, active_arcade_games_available);
        let presentation_route = if preserve_navigation_source_preview {
            PreviewRoute::Occluded
        } else if preview_route.allows_preview_work()
            && nav.screen == Screen::Arcade
            && !memory_guard.active()
            && !screensaver.active
            && !confirm_visible
            && !catalog_scan_visible
            && !nav.arcade_search.is_active(&nav.arcade_filter.active)
        {
            PreviewRoute::Eligible
        } else {
            PreviewRoute::Unavailable
        };
        preview.set_route(presentation_route);
        let crt_backdrop_eligible = preview_route.allows_crt_backdrop()
            && presentation_route == PreviewRoute::Eligible
            && wants_arcade_list
            && !nav.arcade_filter.drawer_open;
        let crt_backdrop_was_eligible = crt_backdrop
            .as_ref()
            .is_some_and(CrtBackdropController::was_eligible);
        let crt_backdrop_leaving = crt_backdrop_was_eligible && !crt_backdrop_eligible;
        if crt_backdrop_leaving {
            full_frame_present = true;
            request_launcher_redraw!();
        }
        let preview_frame_intent = preview.frame_intent();
        let wants_preview_layer =
            preview_route.allows_hdmi_preview() && preview.direct_layer_desired();
        let wants_preview = preview_route.allows_hdmi_preview()
            && !screensaver.active
            && !nav.arcade_search.is_active(&nav.arcade_filter.active)
            && direct_preview_requested(
                nav.screen,
                memory_guard.active(),
                wants_preview_layer
                    || matches!(preview_frame_intent, PreviewFrameIntent::Present { .. }),
            );
        let preview_frame_status = preview.raw_frame_status();
        let preview_cache_state_before_composition = preview.trace_cache_state();
        if navigation_transition.is_active()
            && (effective_view == EffectiveLauncherView::Screensaver
                || confirm_visible
                || catalog_scan_visible)
        {
            let destination_committed = pending_navigation_transition
                .as_ref()
                .is_some_and(|pending| pending.committed);
            let endpoint = if destination_committed {
                navigation_transition.settle_at_destination();
                Some(NavigationTransitionEndpoint::Destination)
            } else {
                navigation_transition.cancel_for_exclusive_view()
            };
            let completion = navigation_transition.complete();
            if completion.is_some() {
                release_full_screen_transition(
                    &mut full_screen_transition,
                    navigation_transition_generation,
                );
            }
            if endpoint == Some(NavigationTransitionEndpoint::Source)
                && let Some(entry) = pending_collection_entry.take()
            {
                preview.cancel_system_entry_preview();
                deferred_navigation_hydration_finish = Some(entry.collection_id);
                arcade_entry_latency.cancel_enter();
            }
            pending_navigation_transition = None;
        }
        let navigation_destination_committed = pending_navigation_transition
            .as_ref()
            .is_some_and(|pending| pending.committed);
        // The list is the navigation destination. Preview media is asynchronous and
        // must never hold the full-screen transition closed after the list is ready.
        let navigation_destination_layers_ready = navigation_destination_committed
            && (nav.screen != Screen::Arcade || active_arcade_games_available);
        let composition_decision = composition.tick(UiCompositionInput {
            screensaver_active: effective_view == EffectiveLauncherView::Screensaver,
            navigation_transition_active: navigation_transition.is_active(),
            navigation_destination_committed,
            navigation_destination_ready: navigation_transition.destination_ready(),
            navigation_destination_layers_ready,
            return_screen: effective_view.return_screen(),
            confirm_visible,
            fullscreen_overlay_visible: catalog_scan_visible,
            arcade_ready: active_arcade_games_available,
            route_ok: display_session.route_ok(),
            wants_arcade_list,
            wants_preview: wants_preview_layer,
            preview_cache_exact: preview_cache_state_before_composition == "exact",
            preview_frame_ready: preview_frame_status == PreviewRawFrameStatus::Ready,
        });
        if screensaver.active {
            full_frame_present = true;
            request_launcher_redraw!();
        } else if screensaver.start_mode != ScreensaverStartMode::Inactive {
            request_launcher_redraw!();
        }
        for event in composition_decision.events.iter() {
            runtime_status::event(event.name, event.detail.as_str());
        }
        if !startup_intro_suppress_launcher_ui {
            sync_navigation_transition_active(&app, &navigation_transition);
        }
        launcher_response_trace.observe_state(&nav, navigation_transition.is_active());
        if composition_decision.force_full_slint_present {
            full_frame_present = true;
        }
        if composition_decision.force_full_slint_raster {
            request_launcher_redraw!();
        }
        if composition_decision.clears_arcade_layer() {
            arcade_list_renderer.invalidate_presented_layer();
            request_launcher_redraw!();
        }
        let startup_status = lifecycle.startup_status();
        let mut composition_status = composition_decision.status();
        composition_status.preview_state = preview.presentation_label();
        composition_status.preview_generation = preview.presentation_generation();
        let automation_frame_stamp = if launcher_automation.active() {
            let selected_system_id = nav.active_collection_scope_id(&catalog);
            let selected_game = (nav.screen == Screen::Arcade)
                .then(|| {
                    nav.active_arcade_game_at(&catalog, selected_system_id, nav.arcade.selected)
                })
                .flatten();
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            launcher_automation.observe_state(AutomationSemanticState {
                screen_orientation: nav.settings.screen_orientation.label().to_string(),
                effective_view: effective_view.label().to_string(),
                return_screen: screen_label(nav.screen).to_string(),
                menu_id: nav.current_menu_id().to_string(),
                selected_item_id: nav.current_menu_selected_item_id().to_string(),
                active_collection_id: nav.active_collection_id().unwrap_or("").to_string(),
                selected_system_id: selected_system_id.to_string(),
                selected_game_id: selected_game
                    .map_or("", |game| game.mra_path.as_ref())
                    .to_string(),
                selected_game_title: selected_game
                    .map_or("", |game| game.title.as_ref())
                    .to_string(),
                selected_index: if nav.screen == Screen::Arcade {
                    nav.arcade.selected
                } else {
                    nav.selected
                },
                selected_count: if nav.screen == Screen::Arcade {
                    nav.active_arcade_game_count(&catalog, selected_system_id)
                } else {
                    nav.current_menu_count()
                },
                overlay: if confirm_visible {
                    "confirm"
                } else if catalog_scan_visible {
                    "catalog-scan"
                } else if setup.is_active() {
                    "controller-setup"
                } else {
                    "none"
                }
                .to_string(),
                dialog_title: bridge.get_confirm_title().to_string(),
                dialog_message: bridge.get_confirm_message().to_string(),
                dialog_selected: confirm_selected,
                drawer_open: nav.arcade_filter.drawer_open,
                drawer_level: nav.arcade_filter.title().to_string(),
                drawer_selected: nav.arcade_filter.selected,
                search_active: nav.arcade_search.is_active(&nav.arcade_filter.active),
                search_status: match nav.arcade_search.status {
                    launcher::ArcadeSearchStatus::Idle => "idle",
                    launcher::ArcadeSearchStatus::Searching => "searching",
                    launcher::ArcadeSearchStatus::Ready => "ready",
                    launcher::ArcadeSearchStatus::Failed => "failed",
                }
                .to_string(),
                search_query: nav.arcade_search.query.clone(),
                search_results: nav.arcade_search_result_count(),
                preview_state: preview.trace_cache_state().to_string(),
                launch_state: if launching { "launching" } else { "idle" }.to_string(),
                loading_title: scheduler.visible_loading_title(&loading_title).to_string(),
                catalog_generation: catalog_generation.current.clone().unwrap_or_default(),
                catalog_ready,
                settings_selected: nav.settings_selected,
                composition_state: composition_status.state.to_string(),
                composition_recovery_count: composition_status.recovery_count,
                navigation_transition_active: navigation_transition.is_active(),
                input_enabled: startup_status.input_enabled,
            })
        } else {
            AutomationFrameStamp::default()
        };
        let home_pan_present_active = update_home_pan_present_window(
            nav.screen,
            nav.scroll_x,
            &mut last_home_pan_scroll_x,
            &mut home_pan_present_until,
            loop_start,
        );
        let home_repeat_bench_active = home_repeat_benchmark_active(launcher_bench_scenario);
        let home_horizontal_input_held = nav.screen == Screen::Home
            && (pad_state_home_horizontal_held(pad.state()) || home_repeat_bench_active);
        if home_frame_driven_redraw_active(
            nav.screen,
            home_pan_present_active,
            home_horizontal_input_held,
        ) {
            request_launcher_redraw!();
        }
        if nav.licenses_scroll_active() {
            request_launcher_redraw!();
        }
        let arcade_visual_changed_this_loop = nav.arcade.visual_index
            != arcade_visual_index_at_loop_start
            || nav.arcade_filter.visual_index != arcade_filter_visual_index_at_loop_start;
        let stream_motion_before_render = navigation_transition.is_active()
            || slint_animation_active
            || home_pan_present_active
            || home_horizontal_input_held
            || nav.licenses_scroll_active()
            || arcade_visual_changed_this_loop
            || (nav.screen == Screen::Arcade && nav.arcade.is_scroll_active())
            || (nav.screen == Screen::Arcade
                && nav.arcade_filter.drawer_open
                && nav.arcade_filter.is_scroll_active());
        if !stream_motion_before_render {
            let _ = launcher_presenter.publish_stream_refinement_if_due();
        }
        let crt_backdrop_prepared = crt_backdrop
            .as_mut()
            .is_some_and(CrtBackdropController::poll);
        let mut wake_reasons = LauncherWakeReasons::default();
        wake_reasons.insert_if(LauncherWakeReasons::REDRAW_PENDING, window.redraw_pending());
        wake_reasons.insert_if(LauncherWakeReasons::LAUNCHING, launching);
        wake_reasons.insert_if(LauncherWakeReasons::SETUP_ACTIVE, setup_active);
        wake_reasons.insert_if(LauncherWakeReasons::BENCHMARK_ACTIVE, launcher_bench_active);
        wake_reasons.insert_if(
            LauncherWakeReasons::SCRIPTED_INPUT_ACTIVE,
            launcher_input_script.active() || launcher_automation.active(),
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::ROUTE_FORCES_FULL_PRESENT,
            route_action.force_full_present,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::BRIDGE_DIRTY,
            full_bridge_dirty || light_bridge_dirty,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::LATENCY_CRITICAL_INPUT,
            latency_critical_input_pending,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::CATALOG_MESSAGES_ACTIVE,
            prepare_trace.catalog_message_count > 0
                || prepare_trace.catalog_backlog > 0
                || pending_catalog_ready.is_some(),
        );
        wake_reasons.insert_if(LauncherWakeReasons::MEDIA_MESSAGE_SEEN, media_message_seen);
        wake_reasons.insert_if(
            LauncherWakeReasons::SLINT_ANIMATION_ACTIVE,
            slint_animation_active,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::HOME_PAN_PRESENT_ACTIVE,
            home_pan_present_active,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::HOME_HORIZONTAL_INPUT_HELD,
            home_horizontal_input_held,
        );
        // Arcade list motion lives outside Slint's bridge key, so the final
        // visual tick still has to present before the launcher can idle.
        wake_reasons.insert_if(
            LauncherWakeReasons::ARCADE_VISUAL_CHANGED_THIS_LOOP,
            arcade_visual_changed_this_loop,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::ARCADE_SCROLL_ACTIVE,
            nav.screen == Screen::Arcade && nav.arcade.is_scroll_active(),
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::ARCADE_FILTER_SCROLL_ACTIVE,
            nav.screen == Screen::Arcade
                && nav.arcade_filter.drawer_open
                && nav.arcade_filter.is_scroll_active(),
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::ARCADE_SEARCH_ACTIVE,
            arcade_search_active,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::PREVIEW_DIRTY,
            preview_frame_intent.is_actionable(),
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::PREVIEW_SCHEDULED_THIS_LOOP,
            preview_scheduled_this_loop,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::CRT_BACKDROP_PREPARED,
            crt_backdrop_prepared,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::COMPOSITION_FORCES_FULL_PRESENT,
            composition_decision.force_full_slint_present,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::COMPOSITION_CLEARS_DIRECT_LAYERS,
            composition_decision.clear_direct_layers,
        );
        wake_reasons = wake_reasons
            | launcher_presentation_recovery_wake_reasons(launcher_presenter.needs_frame());
        let render_intent = LauncherRenderIntent {
            first_visible_copy_done: frame_accounting.first_visible_copy_done(),
            startup_input_enabled: startup_status.input_enabled,
            wake_reasons,
        };
        scheduler_phase = launcher_response_trace
            .record_scheduler_interval("post-projection-background", scheduler_phase);
        if should_restart_for_urgent_input(
            input_batch_empty,
            latency_critical_input_pending,
            input_observation_probe
                .as_ref()
                .is_some_and(|probe| probe.changed_since(input_observation)),
        ) {
            launcher_response_trace.record_lab(Some(serde_json::json!({
                "phase": "input-priority-restart",
                "checkpoint": "before-render",
                "at_us": crate::input_hub::monotonic_us(),
            })));
            let _ = launcher_response_trace
                .record_scheduler_interval("input-priority-restart", scheduler_phase);
            continue;
        }
        if render_intent.can_sleep() {
            if let Some(record) = input_latency_lab.cooperative_quantum(input_observation) {
                launcher_response_trace.record_lab(Some(record));
                continue;
            }
            frame_accounting.finish_idle_loop(
                frames,
                run_start,
                Instant::now(),
                &nav,
                &pad,
                &catalog,
                catalog_ready,
                catalog_session.refresh_done(),
                launching,
                scheduler.visible_loading_title(&loading_title),
                catalog_scan_visible,
                status_text
                    .as_ref()
                    .map(|text| text.catalog_scan_title.as_str())
                    .unwrap_or(""),
                status_text
                    .as_ref()
                    .map(|text| text.catalog_scan_detail.as_str())
                    .unwrap_or(""),
                catalog_scan_percent,
                catalog_background_scan_visible,
                status_text
                    .as_ref()
                    .map(|text| text.catalog_scan_message.as_str())
                    .unwrap_or(""),
                confirm_visible,
                status_text
                    .as_ref()
                    .map(|text| text.confirm_title.as_str())
                    .unwrap_or(""),
                status_text
                    .as_ref()
                    .map(|text| text.confirm_message.as_str())
                    .unwrap_or(""),
                confirm_selected,
                status_text
                    .as_ref()
                    .map(|text| text.confirm_left_label.as_str())
                    .unwrap_or(""),
                status_text
                    .as_ref()
                    .map(|text| text.confirm_right_label.as_str())
                    .unwrap_or(""),
                nav.arcade.selected,
                nav.arcade.visual_index,
                preview.trace_cache_state(),
                preview_transition.current_label(loop_start.duration_since(run_start)),
                1.0,
                &composition_status,
                launcher_bench_scenario,
                start_screen,
                lock_screen,
                display_session.reassert_count(),
                display_session.last_reassert_frame(),
                display_session.last_reassert_ok(),
                display_session.last_reassert_error(),
                startup_status,
                &launch_return_session,
            );
            scheduler_phase = launcher_response_trace
                .record_scheduler_interval("idle-accounting", scheduler_phase);
            record_launcher_frame_phase!(LauncherFramePhase::IdleWait);
            let idle_sleep = input_latency_lab.time_until_next_work().map_or_else(
                || launcher_idle_sleep_duration(&pacer, catalog_work_mode),
                |lab| launcher_idle_sleep_duration(&pacer, catalog_work_mode).min(lab),
            );
            let idle_sleep = catalog_scan_blink
                .time_until_toggle(loop_start)
                .map_or(idle_sleep, |blink| idle_sleep.min(blink));
            let _ = pad.wait_for_input(input_observation, idle_sleep);
            let _ = launcher_response_trace
                .record_scheduler_interval("idle-input-wait", scheduler_phase);
            record_launcher_frame_phase!(LauncherFramePhase::Yielded);
            continue;
        }

        let frame_start_phase_us = pacer.age_since_last_hit_us(loop_start);
        let redraw_pending_for_trace = window.redraw_pending();
        let wake_reasons_bits = wake_reasons.bits();
        let latch_backend_active = launcher_presenter.pacing_backend().is_latch();
        let home_motion_active = home_frame_driven_redraw_active(
            nav.screen,
            home_pan_present_active,
            home_horizontal_input_held,
        );
        let scheduled_frame_class = frame_production_class(
            screensaver.active,
            home_motion_active,
            navigation_transition.is_active(),
        );
        let late_frame_start_headroom_us = if latch_backend_active {
            phase_alignment.required_headroom_us()
        } else {
            FB0_LATE_FRAME_START_HEADROOM_US
        };
        let wait_before_render = latch_late_start_wait_enabled(
            latch_backend_active,
            scheduled_frame_class,
            latency_critical_input_pending,
        ) && pacing_policy
            .decide(LauncherFramePacingInput {
                first_visible_copy_done: frame_accounting.first_visible_copy_done(),
                frame_start_phase_us,
                period_us: pacer.period_us(),
                late_frame_start_headroom_us,
            })
            .wait_before_render;
        let cpu_t0 = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let frame_t0 = Instant::now();
        let prepare_us = (frame_t0 - loop_start).as_micros();
        scheduler_phase =
            launcher_response_trace.record_scheduler_interval("render-setup", scheduler_phase);
        let pre_render_pace = if wait_before_render {
            let wait_start = Instant::now();
            match pacer.wait_interruptible(|| {
                input_observation_probe
                    .as_ref()
                    .is_some_and(|probe| probe.changed_since(input_observation))
            }) {
                VsyncWaitOutcome::Pace(pace) => {
                    let wait_done = Instant::now();
                    Some((
                        pace,
                        wait_done,
                        wait_done.saturating_duration_since(wait_start).as_micros(),
                    ))
                }
                VsyncWaitOutcome::Interrupted => {
                    launcher_response_trace.record_lab(Some(serde_json::json!({
                        "phase": "pre-render-wait-interrupted-input",
                        "interrupted_at_us": crate::input_hub::monotonic_us(),
                    })));
                    let _ = launcher_response_trace.record_scheduler_interval(
                        "pre-render-wait-interrupted-input",
                        scheduler_phase,
                    );
                    request_launcher_redraw!();
                    continue 'launcher;
                }
            }
        } else {
            None
        };
        let pre_render_wait_us = pre_render_pace
            .as_ref()
            .map(|(_, _, wait_us)| *wait_us)
            .unwrap_or(0);
        scheduler_phase =
            launcher_response_trace.record_scheduler_interval("pre-render-pacing", scheduler_phase);
        let full_screen_transition_policy_before_render = full_screen_transition.policy();
        let navigation_snapshot_locked_before_render =
            full_screen_transition_policy_before_render.snapshot_locked;
        if full_screen_transition_policy_before_render.advance_slint_timers {
            update_slint_animations(animation_clock);
        }
        let mut layer_target = LayerTarget::new_oriented_with_epoch(target, layout, layout_epoch);
        let reclaimed_preview_publication =
            layer_target.reclaim_preview_publication(&mut launcher_preview_publication);
        let cpu_t1 = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let frame_t1 = Instant::now();
        retiring_screensaver_pipelines.retain_mut(|pipeline| !pipeline.poll_stopped());
        if screensaver.take_restore_full_frame() {
            if let Some(mut snapshot) = screensaver_launcher_frame.take() {
                if !layer_target.swap_presentation_cached(&mut snapshot) {
                    crate::ui_errln!(
                        "screensaver: launcher frame restore size mismatch snapshot={} cached={}",
                        snapshot.len(),
                        layer_target.cached_frame_view().pixels().len()
                    );
                }
            }
            if let Some(pipeline) = screensaver_pipeline.take() {
                pipeline.cancel();
                retiring_screensaver_pipelines.push(pipeline);
            }
            screensaver_frame_visible = false;
            screensaver_active_cards = 0;
            window.request_redraw();
            full_frame_present = true;
        }
        if screensaver_pipeline_start_allowed(screensaver.active, screensaver_pipeline.is_some()) {
            if screensaver_loader.is_none() {
                if let Some(started) = screensaver_show_started {
                    crate::ui_logln!(
                        "screensaver_startup_timing milestone=loader_started elapsed_us={}",
                        started.elapsed().as_micros()
                    );
                }
                screensaver_loader = Some(LauncherScreensaverLoader::start(
                    layout.output_layout(),
                    screensaver_show_started,
                    launcher_config
                        .catalog_paths()
                        .media_asset_dir()
                        .join("arcade-screenshots-320x320.mmlz4b"),
                    launcher_config.screensaver().seed(),
                ));
            }
            let loader = screensaver_loader.as_ref().expect("created above");
            if let Some(ready) = loader.try_ready() {
                if let Some(started) = screensaver_show_started {
                    crate::ui_logln!(
                        "screensaver_startup_timing milestone=renderer_ready elapsed_us={}",
                        started.elapsed().as_micros()
                    );
                }
                screensaver_pipeline = Some(ScreensaverRenderAhead::start(ready));
                screensaver_render_sequence = 0;
                screensaver_starvation_count = 0;
                screensaver_loader = None;
            }
        }
        if !screensaver.active {
            screensaver_loader = None;
            if let Some(pipeline) = screensaver_pipeline.take() {
                pipeline.cancel();
                retiring_screensaver_pipelines.push(pipeline);
            }
            screensaver_launcher_frame = None;
            screensaver_frame_visible = false;
            screensaver_active_cards = 0;
        }
        let screensaver_fade_alpha = screensaver.preview_fade_alpha(Instant::now());
        let mut frame_production_trace = FrameProductionTrace {
            class: scheduled_frame_class,
            ..FrameProductionTrace::default()
        };
        let mut frame_production_completed_at = None;
        let mut screensaver_render_trace = ScreensaverRenderTrace::default();
        let mut accepted_screensaver_frame = false;
        let mut screensaver_buffer_to_recycle_after_present = None;
        let mut completed_hidden_frame_for_present = None;
        let mut accepted_startup_intro_frame = false;
        let mut startup_intro_failure = None;
        let mut navigation_capture_source_carrier_rendered = false;
        let mut orientation_capture_source_carrier_rendered = false;
        if navigation_capture_source_carrier_required(
            full_screen_transition_policy_before_render,
            full_screen_transition.owner(),
            navigation_transition.frame().phase,
            navigation_transition.settings_physical_space(),
        ) {
            let mut direct_render_timing = None;
            match launcher_presenter.try_render_direct_hidden_frame(
                f,
                display_session,
                |_, pixels| {
                    let started = Instant::now();
                    let start_phase_us = pacer.age_since_last_hit_us(started);
                    let rendered = navigation_transition.render_into(pixels).is_ok();
                    direct_render_timing = Some((started, Instant::now(), start_phase_us));
                    rendered
                },
            ) {
                Ok(Some(completed)) => {
                    let (direct_render_started, direct_render_completed, start_phase_us) =
                        direct_render_timing.expect("successful source carrier was timed");
                    frame_production_trace.class = FrameProductionClass::SynchronousAnimation;
                    frame_production_trace.sequence = completed.grant.generation;
                    frame_production_trace.render_start_phase_us = start_phase_us;
                    frame_production_trace.render_wall_us = direct_render_completed
                        .saturating_duration_since(direct_render_started)
                        .as_micros()
                        .try_into()
                        .unwrap_or(u64::MAX);
                    frame_production_completed_at = Some(direct_render_completed);
                    completed_hidden_frame_for_present = Some(completed);
                    navigation_capture_source_carrier_rendered = true;
                }
                Ok(None) => {}
                Err(failure) => launcher_presenter.fail_latch_completion(failure),
            }
        }
        if orientation_capture_source_carrier_required(
            full_screen_transition_policy_before_render,
            full_screen_transition.owner(),
            orientation_transition.is_active(),
            orientation_transition.destination_ready(),
        ) {
            let mut direct_render_timing = None;
            match launcher_presenter.try_render_direct_hidden_frame(
                f,
                display_session,
                |_, pixels| {
                    let started = Instant::now();
                    let start_phase_us = pacer.age_since_last_hit_us(started);
                    let rendered = orientation_transition.copy_source_into(pixels);
                    direct_render_timing = Some((started, Instant::now(), start_phase_us));
                    rendered
                },
            ) {
                Ok(Some(completed)) => {
                    let (direct_render_started, direct_render_completed, start_phase_us) =
                        direct_render_timing.expect("successful orientation carrier was timed");
                    frame_production_trace.class = FrameProductionClass::SynchronousAnimation;
                    frame_production_trace.sequence = completed.grant.generation;
                    frame_production_trace.render_start_phase_us = start_phase_us;
                    frame_production_trace.render_wall_us = direct_render_completed
                        .saturating_duration_since(direct_render_started)
                        .as_micros()
                        .try_into()
                        .unwrap_or(u64::MAX);
                    frame_production_completed_at = Some(direct_render_completed);
                    completed_hidden_frame_for_present = Some(completed);
                    orientation_capture_source_carrier_rendered = true;
                }
                Ok(None) => {}
                Err(failure) => launcher_presenter.fail_latch_completion(failure),
            }
        }
        if let Some(intro) = startup_intro.as_mut() {
            if intro.snapshot_capture_needed() && startup_intro_launcher_frame_ready {
                let launcher_pixels = layer_target.presentation_frame_view().pixels();
                if let Err(error) = intro.begin_launcher_snapshot_preparation(launcher_pixels) {
                    startup_intro_failure = Some(error);
                } else {
                    print_startup_event(
                        start,
                        "startup_intro_launcher_snapshot_captured",
                        format!(
                            "pixels={} cabinet_wait_frames={}",
                            launcher_pixels.len(),
                            intro.waiting_frames(),
                        ),
                    );
                }
            }
            if startup_intro_failure.is_none() {
                match intro.poll_launcher_snapshot_preparation() {
                    Ok(true) => print_startup_event(
                        start,
                        "startup_intro_launcher_snapshot_prepared",
                        format!("cabinet_wait_frames={}", intro.waiting_frames()),
                    ),
                    Ok(false) => {}
                    Err(error) => startup_intro_failure = Some(error),
                }
            }
            if startup_intro_failure.is_none() {
                let mut source_evidence = None;
                let mut render_error = None;
                match launcher_presenter.try_render_startup_intro_hidden_frame(
                    f,
                    display_session,
                    |grant, pixels| match intro.render_into(
                        grant,
                        pixels,
                        launcher_readiness.needs_source_evidence(),
                    ) {
                        Ok(evidence) => {
                            source_evidence = evidence;
                            true
                        }
                        Err(error) => {
                            render_error = Some(error);
                            false
                        }
                    },
                ) {
                    Ok(Some(mut completed)) => {
                        completed.source_evidence = source_evidence;
                        completed_hidden_frame_for_present = Some(completed);
                        accepted_startup_intro_frame = true;
                    }
                    Ok(None) => {}
                    Err(failure) => {
                        launcher_presenter.fail_latch_completion(failure);
                        startup_intro_failure = Some("hidden-slot grant failed".into());
                    }
                }
                if startup_intro_failure.is_none() {
                    startup_intro_failure = render_error;
                }
            }
        }
        if let Some(error) = startup_intro_failure.take() {
            crate::ui_errln!("startup intro stopped: {error}");
            startup_intro = None;
            launcher_presenter.invalidate_external_hidden_mode();
            full_frame_present = true;
            window.request_redraw();
        }
        if startup_intro.is_none() && screensaver.active {
            let render_ahead_poll = screensaver_pipeline
                .as_mut()
                .map(ScreensaverRenderAhead::try_next)
                .unwrap_or(RenderAheadPoll::Empty);
            match render_ahead_poll {
                RenderAheadPoll::Frame(frame) => {
                    let mut pixels = frame.pixels;
                    if layer_target.swap_presentation_cached(&mut pixels) {
                        retain_or_defer_screensaver_buffer(
                            &mut screensaver_launcher_frame,
                            &mut screensaver_buffer_to_recycle_after_present,
                            pixels,
                        );
                        screensaver_render_trace = frame.trace;
                        screensaver_render_sequence = frame.sequence;
                        frame_production_trace.class = FrameProductionClass::Prepared;
                        frame_production_trace.sequence = frame.sequence;
                        frame_production_completed_at = Some(frame.completed_at);
                        frame_production_trace.ready_depth = screensaver_pipeline
                            .as_ref()
                            .map(ScreensaverRenderAhead::ready_depth)
                            .unwrap_or(0);
                        frame_production_trace.render_wall_us = frame.render_wall_us;
                        screensaver_active_cards = frame.active_cards;
                        screensaver_frame_visible = true;
                        accepted_screensaver_frame = true;
                    } else {
                        crate::ui_errln!(
                            "screensaver: render-ahead frame geometry mismatch sequence={} pixels={} cached={}",
                            frame.sequence,
                            pixels.len(),
                            layer_target.cached_frame_view().pixels().len()
                        );
                        if let Some(pipeline) = screensaver_pipeline.as_ref() {
                            let _ = pipeline.recycle(pixels);
                        }
                    }
                }
                RenderAheadPoll::Empty => {}
                RenderAheadPoll::SequenceFailure {
                    expected_tick,
                    actual_tick,
                    frame: _,
                } => {
                    crate::ui_errln!(
                        "screensaver: strict render-ahead sequence failure expected_tick={} actual_tick={}",
                        expected_tick,
                        actual_tick,
                    );
                    screensaver.fail_current_activation(Instant::now());
                    if let Some(pipeline) = screensaver_pipeline.take() {
                        pipeline.cancel();
                        retiring_screensaver_pipelines.push(pipeline);
                    }
                    screensaver_frame_visible = false;
                    screensaver_active_cards = 0;
                    window.request_redraw();
                    full_frame_present = true;
                }
                RenderAheadPoll::Disconnected => {
                    crate::ui_errln!(
                        "screensaver: render-ahead pipeline disconnected; suppressing reactivation until fresh user activity"
                    );
                    screensaver.fail_current_activation(Instant::now());
                    if let Some(mut snapshot) = screensaver_launcher_frame.take()
                        && !layer_target.swap_presentation_cached(&mut snapshot)
                    {
                        crate::ui_errln!(
                            "screensaver: launcher frame restore size mismatch after pipeline disconnect snapshot={} cached={}",
                            snapshot.len(),
                            layer_target.cached_frame_view().pixels().len()
                        );
                    }
                    if let Some(pipeline) = screensaver_pipeline.take() {
                        pipeline.cancel();
                        retiring_screensaver_pipelines.push(pipeline);
                    }
                    screensaver_frame_visible = false;
                    screensaver_active_cards = 0;
                    window.request_redraw();
                    full_frame_present = true;
                }
            }
        }
        if screensaver.active
            && screensaver_frame_visible
            && !accepted_screensaver_frame
            && screensaver_pipeline.is_some()
        {
            screensaver_starvation_count = screensaver_starvation_count.saturating_add(1);
            crate::ui_errln!("screensaver: shared screenshot runtime starved; restoring launcher");
            screensaver.fail_current_activation(Instant::now());
            if let Some(pipeline) = screensaver_pipeline.take() {
                pipeline.cancel();
                retiring_screensaver_pipelines.push(pipeline);
            }
            screensaver_frame_visible = false;
            window.request_redraw();
            full_frame_present = true;
        }
        if screensaver.active {
            frame_production_trace.class = FrameProductionClass::Prepared;
            frame_production_trace.sequence = screensaver_render_sequence;
            frame_production_trace.ready_depth = screensaver_pipeline
                .as_ref()
                .map(ScreensaverRenderAhead::ready_depth)
                .unwrap_or(0);
            frame_production_trace.starvation_count = screensaver_starvation_count;
            frame_production_trace.cancelled =
                screensaver_pipeline.is_none() && !retiring_screensaver_pipelines.is_empty();
        }
        let mut slint_damage = DirtyRectList::new();
        let mut full_screen_transition_release_raster_rendered = false;
        let mut full_screen_controlled_capture_rendered = false;
        let mut orientation_controlled_slint_raster_us = 0;
        let mut gui_raster_phase = GuiRasterProfilePhase::None;
        let response_raster_started_at_us = crate::input_hub::monotonic_us();
        let response_raster_started_execution = launcher_response_trace.execution_stamp();
        let raster_pmu = launcher_response_trace.input_pmu_span(
            latency_critical_input_pending,
            "launcher-response.slint-raster",
        );
        let this_rect = if screensaver.active && screensaver_frame_visible {
            if accepted_screensaver_frame {
                if screensaver_fade_alpha.is_some_and(|alpha| alpha < 255) {
                    Some(
                        layer_target.blend_screensaver_crossfade(
                            screensaver_launcher_frame
                                .as_deref()
                                .expect("launcher frame retained by first buffer swap"),
                            screensaver_fade_alpha.expect("checked above"),
                        ),
                    )
                } else {
                    Some(DirtyRect {
                        x0: 0,
                        y0: 0,
                        x1: layout.composition_w(),
                        y1: layout.composition_h(),
                    })
                }
            } else {
                None
            }
        } else if screensaver.active {
            None
        } else if startup_intro_suppress_launcher_ui {
            None
        } else if full_screen_transition_policy_before_render.snapshot_locked {
            if let Some(generation) = full_screen_transition.generation() {
                let _ = full_screen_transition.retain_redraw(generation);
            }
            None
        } else if full_screen_transition_policy_before_render.force_live_raster {
            gui_raster_phase = gui_raster_profile_phase(true, true);
            let gui_raster_pmu = gui_profiling.phase_span(gui_raster_phase.span_name());
            let (dirty, damage, rendered) = layer_target.render_slint_full(&window);
            drop(gui_raster_pmu);
            slint_damage = damage;
            full_screen_transition_release_raster_rendered = rendered;
            dirty
        } else if full_screen_transition_policy_before_render.controlled_capture
            && (composition_decision.force_full_slint_raster
                || full_screen_transition.owner() == Some(FullScreenTransitionOwner::Orientation))
        {
            let authorized = full_screen_transition
                .generation()
                .is_some_and(|generation| {
                    match full_screen_transition.take_controlled_capture(generation) {
                        Ok(authorized) => authorized,
                        Err(error) => {
                            crate::ui_errln!("navigation controlled capture rejected: {error:?}");
                            false
                        }
                    }
                });
            if authorized {
                gui_raster_phase = gui_raster_profile_phase(true, true);
                let gui_raster_pmu = gui_profiling.phase_span(gui_raster_phase.span_name());
                let controlled_raster_started = Instant::now();
                let (dirty, damage, rendered) = layer_target.render_slint_full(&window);
                drop(gui_raster_pmu);
                if full_screen_transition.owner() == Some(FullScreenTransitionOwner::Orientation) {
                    orientation_controlled_slint_raster_us =
                        controlled_raster_started.elapsed().as_micros();
                }
                slint_damage = damage;
                full_screen_controlled_capture_rendered = rendered;
                if !rendered
                    && let Some(generation) = full_screen_transition.generation()
                    && let Err(error) = full_screen_transition.capture_deferred(generation)
                {
                    crate::ui_errln!("full-screen controlled capture defer rejected: {error:?}");
                } else if !rendered {
                    window.request_redraw();
                }
                dirty
            } else {
                None
            }
        } else if !full_screen_transition_policy_before_render.automatic_slint_raster {
            if let Some(generation) = full_screen_transition.generation() {
                let _ = full_screen_transition.retain_redraw(generation);
            }
            None
        } else if crt_backdrop_eligible
            && crt_backdrop_was_eligible
            && !crt_backdrop_leaving
            && !full_frame_present
            && (arcade_scroll_active || preview.raw_transition_frame().is_some())
        {
            // During CRT Arcade motion, the custom compositor owns the
            // changing backdrop, list, and chrome restoration. Keep the
            // launcher cadence alive without rerasterizing the unchanged
            // Slint base on every velocity tick.
            request_launcher_redraw!();
            None
        } else if composition_decision.force_full_slint_raster || crt_backdrop_leaving {
            gui_raster_phase = gui_raster_profile_phase(true, true);
            let gui_raster_pmu = gui_profiling.phase_span(gui_raster_phase.span_name());
            let (dirty, damage, _) = layer_target.render_slint_full(&window);
            drop(gui_raster_pmu);
            slint_damage = damage;
            dirty
        } else if startup_intro_prepare_live_launcher {
            gui_raster_phase = gui_raster_profile_phase(true, false);
            let gui_raster_pmu = gui_profiling.phase_span(gui_raster_phase.span_name());
            let (dirty, damage) = layer_target.render_slint_base(&window);
            drop(gui_raster_pmu);
            slint_damage = damage;
            dirty
        } else {
            gui_raster_phase = gui_raster_profile_phase(true, false);
            let gui_raster_pmu = gui_profiling.phase_span(gui_raster_phase.span_name());
            let (dirty, damage) = layer_target.render_slint_base(&window);
            drop(gui_raster_pmu);
            let expanded = if layout.is_portrait() {
                dirty
            } else {
                expand_home_pan_dirty_rect(dirty, ui, home_pan_present_active)
            };
            slint_damage = if expanded == dirty {
                damage
            } else {
                expanded.map_or_else(DirtyRectList::new, DirtyRectList::from_one)
            };
            expanded
        };
        let response_raster_completed_at_us = crate::input_hub::monotonic_us();
        let response_raster_completed_execution = launcher_response_trace.execution_stamp();
        drop(raster_pmu);
        gui_profiling.record_frame(
            frames,
            response_raster_completed_at_us,
            frame_production_trace.class.label(),
            gui_bridge_phase,
            gui_raster_phase,
            slint_damage
                .iter()
                .map(|rect| [rect.x0, rect.y0, rect.x1, rect.y1])
                .collect(),
        );
        if can_preempt_disposable_home_raster(
            nav.screen,
            input_batch_empty,
            latency_critical_input_pending,
            input_observation_probe
                .as_ref()
                .is_some_and(|probe| probe.changed_since(input_observation)),
            navigation_transition.is_active()
                || orientation_transition.is_active()
                || full_screen_transition.state() != FullScreenTransitionState::Live,
            screensaver.active,
            composition_decision.state != UiCompositionState::FullSlint
                || composition_decision.retirement_generation.is_some(),
            startup_intro.is_some(),
        ) {
            // Slint has already updated the cached RGB565 image, but this
            // disposable frame has not reached a hidden scanout slot. Carry a
            // full cached-frame copy into the replacement so damage consumed
            // by this abandoned raster cannot diverge from either hidden slot.
            unpublished_cached_frame_present = true;
            launcher_response_trace.record_lab(Some(serde_json::json!({
                "phase": "input-priority-restart",
                "checkpoint": "after-slint-raster",
                "at_us": response_raster_completed_at_us,
                "slint_damage_rects": slint_damage.len(),
            })));
            let _ = launcher_response_trace
                .record_scheduler_interval("input-priority-restart", scheduler_phase);
            request_launcher_redraw!();
            continue 'launcher;
        }
        let frame_plan_pmu = launcher_response_trace.input_pmu_span(
            latency_critical_input_pending,
            "launcher-response.damage-frame-plan",
        );
        let mut launcher_response_frame_stamp = launcher_response_trace.frame_stamp(
            &nav,
            response_projected_at_us,
            response_projected_execution,
            response_raster_started_at_us,
            response_raster_started_execution,
            response_raster_completed_at_us,
            response_raster_completed_execution,
        );
        if let Some(stamp) = launcher_response_frame_stamp.as_mut() {
            stamp.slint_damage_rects.extend(
                slint_damage
                    .iter()
                    .map(|rect| (rect.x0, rect.y0, rect.x1, rect.y1)),
            );
        }
        if full_screen_transition.owner() == Some(FullScreenTransitionOwner::Orientation)
            && full_screen_transition.state() == FullScreenTransitionState::CapturePending
            && !full_screen_transition.policy().controlled_capture
            && !full_screen_controlled_capture_rendered
        {
            orientation_transition.cancel();
            release_full_screen_transition(
                &mut full_screen_transition,
                orientation_transition_generation,
            );
        }
        if startup_intro_prepare_live_launcher {
            startup_intro_launcher_frame_ready = true;
            print_startup_event(
                start,
                "startup_intro_launcher_frame_ready",
                format!("games={} systems={}", catalog.len(), catalog.systems.len()),
            );
        }
        if accepted_screensaver_frame && !screensaver_first_render_logged {
            screensaver_first_render_logged = true;
            if let Some(started) = screensaver_show_started {
                crate::ui_logln!(
                    "screensaver_startup_timing milestone=first_saver_render elapsed_us={}",
                    started.elapsed().as_micros()
                );
            }
        }
        let cpu_t2 = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let frame_t2 = Instant::now();
        let cpu_custom_draw_start = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let custom_draw_start = Instant::now();
        let logical_slint_rect = this_rect.map(|rect| {
            if layout.is_portrait() && !slint_damage.is_empty() {
                layout.composition_rect_to_logical_rect(rect)
            } else {
                rect
            }
        });
        let gui_custom_selection = gui_custom_profile_selection(
            wants_arcade_list && composition_decision.allow_arcade_list_blit,
            (wants_preview || preview.empty_base_commit_pending())
                && composition_decision.allow_preview_blit,
            navigation_transition.is_active(),
            orientation_transition.is_active(),
        );
        let gui_custom_generation_pmu = gui_profiling.phase_span(
            gui_custom_selection
                .any()
                .then_some("gui.custom-layer-generation"),
        );
        let arcade_list_update_start = Instant::now();
        let arcade_list_rect = if wants_arcade_list && composition_decision.allow_arcade_list_blit {
            let gui_arcade_pmu = gui_profiling.phase_span(gui_custom_selection.arcade_row_update);
            let arcade_list_profile_pmu =
                mister_magik_perf_events::sampled_span("gui.custom.crt-arcade-list-update");
            arcade_list_renderer.set_crt_portrait_rows(layout.is_portrait());
            configure_arcade_list_renderer_geometry(&mut arcade_list_renderer, &nav, ui);
            let force_arcade_redraw = if layout.is_portrait() && !crt_layout {
                // The portrait list is a separately versioned physical layer.
                // Slint/base damage is restored by the latch presenter and
                // must not force regeneration of unchanged list content.
                false
            } else {
                arcade_list_needs_forced_redraw(
                    &arcade_list_renderer,
                    logical_slint_rect,
                    full_frame_present,
                )
            };
            let update = if nav.arcade_filter.drawer_open {
                let items = arcade_drawer_view_cache.items(&catalog, &nav, catalog_version);
                arcade_list_renderer.draw_filter_items(
                    items,
                    nav.arcade_filter.selected,
                    nav.arcade_filter.visual_index,
                    force_arcade_redraw,
                )
            } else {
                arcade_list_renderer.draw(
                    active_arcade_games,
                    nav.arcade.selected,
                    nav.arcade.visual_index,
                    force_arcade_redraw,
                )
            };
            drop(arcade_list_profile_pmu);
            drop(gui_arcade_pmu);
            update
        } else {
            None
        };
        let arcade_list_update_us = arcade_list_update_start.elapsed().as_micros();
        let mut portrait_arcade_list_pixels = 0_u64;
        let mut portrait_arcade_list_bytes = 0_u64;
        let preview_blit_start = Instant::now();
        let gui_preview_pmu = gui_profiling.phase_span(gui_custom_selection.preview_composition);
        let empty_base_cached_rect = if (layout.is_portrait() || preview_direct_present_enabled())
            && preview_route.allows_hdmi_preview()
            && composition_decision.allow_preview_blit
            && !memory_guard.active()
            && preview.empty_base_commit_pending()
        {
            Some(if layout.is_portrait() {
                layer_target.clear_presentation_preview()
            } else {
                layer_target.clear_cached_preview()
            })
        } else {
            None
        };
        if should_start_preview_compositor(
            wants_preview,
            preview_route.allows_hdmi_preview(),
            composition_decision.allow_preview_blit,
            memory_guard.active(),
            preview_compositor_start_attempted,
        ) {
            preview_compositor_start_attempted = true;
            match PreviewCompositor::start() {
                Ok(worker) => preview_compositor = Some(worker),
                Err(error) => crate::ui_errln!("preview_compositor_start_failed: {error}"),
            }
        }
        let (
            raw_preview,
            preview_transition_trace,
            preview_compositor_pending,
            preview_compositor_telemetry,
        ) = if wants_preview && composition_decision.allow_preview_blit && !memory_guard.active() {
            layer_target.blit_raw_preview_if_needed(
                &mut preview,
                &mut preview_transition,
                loop_start.duration_since(run_start),
                logical_slint_rect,
                full_frame_present,
                preview_compositor.as_mut(),
            )
        } else {
            (None, PreviewTransitionTrace::default(), false, None)
        };
        if preview_compositor_pending {
            request_launcher_redraw!();
        }
        drop(gui_preview_pmu);
        let preview_blit_us = preview_blit_start.elapsed().as_micros();
        let portrait_preview_rotation_pixels = if layout.is_portrait() {
            raw_preview
                .map(|present| match present {
                    RawPreviewPresent::Cached(rect) | RawPreviewPresent::Direct(rect) => rect,
                })
                .map(|rect| (rect.width() as u64).saturating_mul(u64::from(rect.rows())))
                .unwrap_or(0)
        } else {
            0
        };
        let portrait_preview_blend_pixels = if layout.is_portrait() {
            u64::from(preview_transition_trace.fade.pixels)
        } else {
            0
        };
        if preview_transition_trace.active {
            request_launcher_redraw!();
        }
        let mut crt_backdrop_full_damage = None;
        let mut crt_backdrop_work_trace = crate::crt_backdrop::CrtBackdropWorkTrace::default();
        let mut crt_backdrop_copy_us = 0_u64;
        let mut crt_backdrop_list_overlay_us = 0_u64;
        let mut crt_backdrop_copy_pixels = 0_u32;
        let mut crt_backdrop_list_overlay_pixels = 0_u32;
        let mut crt_backdrop_list_restore_pixels = 0_u32;
        let mut crt_backdrop_list_foreground_pixels = 0_u32;
        let transition_id = preview
            .raw_transition_frame()
            .as_ref()
            .map(|frame| frame.transition_id);
        // Releasing a full-screen transition performs one live full Slint
        // raster. That raster owns the CRT placeholder background, so restore
        // the settled custom backdrop in the same frame before the list layer.
        let force_crt_backdrop_repaint = full_screen_transition_release_raster_rendered;
        if let Some(backdrop) = crt_backdrop.as_mut() {
            let compose_start = Instant::now();
            let crt_arcade_layout = CrtArcadeLayout::for_layout(
                layout,
                crt_metrics,
                nav.arcade_search.is_active(&nav.arcade_filter.active),
            );
            let result = backdrop.compose(
                crt_backdrop_eligible,
                force_crt_backdrop_repaint,
                arcade_turbo_active,
                nav.arcade.selected,
                transition_id,
                (preview_cache_state_before_composition == "exact")
                    .then(|| preview.selected_backdrop_source())
                    .flatten(),
                loop_start.saturating_duration_since(run_start),
                layer_target.presentation_pixels_mut(),
                layout,
                crt_arcade_layout,
                crt_metrics,
            );
            crt_backdrop_work_trace = result.trace;
            crt_backdrop_copy_us = compose_start
                .elapsed()
                .as_micros()
                .saturating_sub(u128::from(crt_backdrop_work_trace.blend_us))
                .min(u128::from(u64::MAX)) as u64;
            crt_backdrop_copy_pixels = backdrop
                .width()
                .saturating_mul(backdrop.height())
                .min(u32::MAX as usize) as u32;
            if result.full_damage {
                crt_backdrop_full_damage = Some(DirtyRect {
                    x0: 0,
                    y0: 0,
                    x1: layout.composition_w(),
                    y1: layout.composition_h(),
                });
            }
            if crt_backdrop_work_trace.active {
                request_launcher_redraw!();
            }
        }
        let navigation_transition_composition_active = navigation_transition.is_active();
        let navigation_settings_physical_space = navigation_transition.settings_physical_space();
        let navigation_transition_frame_active = navigation_transition_composition_active
            && navigation_transition.frame().phase != NavigationTransitionPhase::Capture;
        let (
            navigation_transition_route,
            navigation_transition_direction,
            navigation_transition_renderer,
        ) = if navigation_transition_frame_active {
            navigation_transition
                .route()
                .zip(navigation_transition.request())
                .map_or(("", "", ""), |(route, request)| {
                    (route.label(), request.direction.label(), route.renderer())
                })
        } else {
            ("", "", "")
        };
        let navigation_transition_frame_started =
            navigation_transition_frame_active.then_some(loop_start);
        let mut navigation_transition_render_us = 0u128;
        let mut navigation_logical_frame_rendered = false;
        if navigation_transition_composition_active {
            let navigation_transition_compositor_started = Instant::now();
            let now_us = loop_start
                .saturating_duration_since(start)
                .as_micros()
                .min(u64::MAX as u128) as u64;
            let destination_committed = pending_navigation_transition
                .as_ref()
                .is_some_and(|pending| pending.committed);
            let mut render_transition_frame = !navigation_capture_source_carrier_rendered;
            if destination_committed && !navigation_transition.destination_ready() {
                let controlled_destination_raster_ready = full_screen_controlled_capture_rendered
                    || (full_screen_transition.owner()
                        == Some(FullScreenTransitionOwner::Navigation)
                        && full_screen_transition.capture_issued());
                let destination_raster_ready = composition_decision.prepare_navigation_destination
                    && controlled_destination_raster_ready;
                let mut destination_layers_ready =
                    destination_raster_ready && nav.screen != Screen::Arcade;
                if destination_raster_ready && nav.screen == Screen::Arcade {
                    let preview_expected = selected_arcade_game_has_preview(&nav, &catalog);
                    let preview_exact = preview_expected
                        && !preview.terminal_empty()
                        && preview.trace_cache_state() == "exact"
                        && preview.raw_frame_status() == PreviewRawFrameStatus::Ready;
                    let preview_surface_ready = if preview_exact {
                        if navigation_transition.settings_physical_space() {
                            let (ready, publication) = layer_target.compose_exact_preview_physical(
                                &preview,
                                launcher_preview_publication.as_ref(),
                                &mut launcher_preview_version,
                            );
                            if let Some(publication) = publication {
                                launcher_preview_publication = Some(publication);
                            }
                            ready
                        } else {
                            match layer_target.compose_exact_preview(&preview) {
                                Some(RawPreviewPresent::Cached(_)) => true,
                                Some(RawPreviewPresent::Direct(rect)) => {
                                    layer_target.compose_direct_preview_rect(rect) > 0
                                }
                                None => false,
                            }
                        }
                    } else {
                        // Capture a clean list destination now. If an exact preview
                        // arrives later, the normal Arcade presentation path adopts it.
                        if navigation_transition.settings_physical_space() {
                            let _ = layer_target.clear_presentation_preview();
                        } else {
                            let _ = layer_target.clear_cached_preview();
                        }
                        true
                    };
                    if preview_surface_ready {
                        configure_arcade_list_renderer_geometry(
                            &mut arcade_list_renderer,
                            &nav,
                            ui,
                        );
                        if let Some(update) = arcade_list_renderer.draw(
                            active_system_game_view(&catalog, &nav),
                            nav.arcade.selected,
                            nav.arcade.visual_index,
                            true,
                        ) {
                            if navigation_transition.settings_physical_space() {
                                let _ = layer_target.reclaim_arcade_publication(
                                    &mut arcade_list_renderer,
                                    &mut launcher_arcade_publication,
                                );
                                launcher_arcade_content_generation =
                                    launcher_arcade_content_generation.wrapping_add(1).max(1);
                                let (_, publication) = layer_target
                                    .compose_arcade_list_direct_layer_snapshot(
                                        &mut arcade_list_renderer,
                                        update,
                                        catalog_version as u64,
                                        launcher_arcade_version,
                                        launcher_arcade_scroll_offset,
                                        launcher_arcade_content_generation,
                                    );
                                launcher_arcade_publication = publication;
                            } else {
                                let _ = layer_target.compose_arcade_list_snapshot_update(
                                    &mut arcade_list_renderer,
                                    update,
                                );
                            }
                        }
                        destination_layers_ready = true;
                    }
                }
                let mut status_quiesce = None;
                if destination_layers_ready {
                    let worker_active = frame_accounting.runtime_status_worker_active();
                    if let Some(pending) = pending_navigation_transition.as_mut() {
                        let started = pending
                            .status_quiesce_started_at
                            .get_or_insert_with(Instant::now);
                        let waited = started.elapsed();
                        let timed_out = worker_active && waited >= NAVIGATION_STATUS_QUIESCE_LIMIT;
                        status_quiesce = Some((waited, timed_out));
                        if worker_active && !timed_out {
                            destination_layers_ready = false;
                        }
                    }
                }
                if destination_layers_ready {
                    if let Some((waited, timed_out)) = status_quiesce {
                        navigation_transition.note_pending_status_quiesce(
                            waited.as_micros().min(u64::MAX as u128) as u64,
                            timed_out,
                        );
                    }
                    if navigation_transition
                        .capture_destination(
                            if navigation_transition.settings_physical_space() {
                                layer_target.presentation_frame_view().pixels()
                            } else {
                                layer_target.cached_frame_view().pixels()
                            },
                            now_us,
                        )
                        .is_err()
                    {
                        navigation_transition.settle_at_destination();
                        render_transition_frame = false;
                    } else if navigation_transition_generation.is_some_and(|generation| {
                        full_screen_transition
                            .capture_completed(generation)
                            .is_err()
                    }) {
                        navigation_transition.settle_at_destination();
                        render_transition_frame = false;
                    }
                    navigation_transition.tick(now_us);
                }
            }
            if render_transition_frame {
                let gui_navigation_pmu =
                    gui_profiling.phase_span(gui_custom_selection.navigation_transition_raster);
                let mut rendered_direct = false;
                if navigation_transition.settings_physical_space() {
                    let mut direct_render_timing = None;
                    match launcher_presenter.try_render_direct_hidden_frame(
                        f,
                        display_session,
                        |_, pixels| {
                            let started = Instant::now();
                            let start_phase_us = pacer.age_since_last_hit_us(started);
                            let rendered = navigation_transition.render_into(pixels).is_ok();
                            direct_render_timing = Some((started, Instant::now(), start_phase_us));
                            rendered
                        },
                    ) {
                        Ok(Some(completed)) => {
                            let (direct_render_started, direct_render_completed, start_phase_us) =
                                direct_render_timing.expect("successful direct render was timed");
                            frame_production_trace.class =
                                FrameProductionClass::SynchronousAnimation;
                            frame_production_trace.sequence = completed.grant.generation;
                            frame_production_trace.render_start_phase_us = start_phase_us;
                            frame_production_trace.render_wall_us = direct_render_completed
                                .saturating_duration_since(direct_render_started)
                                .as_micros()
                                .try_into()
                                .unwrap_or(u64::MAX);
                            frame_production_completed_at = Some(direct_render_completed);
                            completed_hidden_frame_for_present = Some(completed);
                            rendered_direct = true;
                        }
                        Ok(None) => {}
                        Err(failure) => launcher_presenter.fail_latch_completion(failure),
                    }
                    if !rendered_direct {
                        let _ = navigation_transition
                            .render_into(layer_target.presentation_pixels_mut());
                    }
                } else if let Ok(frame) = navigation_transition.render() {
                    let _ = layer_target.restore_cached(frame);
                    navigation_logical_frame_rendered = true;
                }
                drop(gui_navigation_pmu);
            }
            full_frame_present = true;
            request_launcher_redraw!();
            if navigation_transition.frame().phase == NavigationTransitionPhase::Settled {
                settings_navigation_benchmark.note_rendered_endpoint(frames);
                let completion = navigation_transition.complete();
                if completion.is_some() {
                    release_full_screen_transition(
                        &mut full_screen_transition,
                        navigation_transition_generation,
                    );
                }
                let pending = pending_navigation_transition.take();
                if completion.is_some_and(|completion| {
                    completion.endpoint == NavigationTransitionEndpoint::Destination
                }) && pending
                    .as_ref()
                    .is_some_and(|pending| pending.event.action == LauncherAction::NavigateHome)
                {
                    navigation_transition.clear_geometry_history();
                }
                if completion.is_some_and(|completion| {
                    completion.endpoint == NavigationTransitionEndpoint::Source
                }) {
                    if let Some(entry) = pending_collection_entry.take() {
                        preview.cancel_system_entry_preview();
                        nav.catalog_system_hydration_finished(&entry.collection_id);
                        arcade_entry_latency.cancel_enter();
                    }
                    if let Some(pending) = pending {
                        let before = LauncherBridgeKey::from_nav(&nav);
                        nav.restore_navigation_transition_state(pending.source_state);
                        let after = LauncherBridgeKey::from_nav(&nav);
                        if before != after {
                            media_session.note_nav_change(&before, &after, Instant::now());
                        }
                        navigation_source_bridge_sync_pending = true;
                        request_launcher_redraw!();
                    }
                }
            }
            navigation_transition_render_us = navigation_transition_compositor_started
                .elapsed()
                .as_micros();
            navigation_transition
                .note_frame_work_us(navigation_transition_render_us.min(u64::MAX as u128) as u64);
            sync_navigation_transition_active(&app, &navigation_transition);
        }
        let effect_label_us = navigation_transition_render_us;
        let navigation_telemetry = navigation_transition.telemetry();
        let mut custom_draw_trace = LauncherCustomDrawTrace {
            arcade_list_update_us,
            portrait_arcade_list_pixels,
            portrait_arcade_list_bytes,
            preview_blit_us,
            portrait_preview_rotation_pixels,
            portrait_preview_blend_pixels,
            portrait_preview_worker_queue_replacements: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.queue_replacements)
                .unwrap_or(0),
            portrait_preview_worker_result_replacements: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.result_replacements)
                .unwrap_or(0),
            portrait_preview_worker_stale_results: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.stale_results)
                .unwrap_or(0),
            portrait_preview_worker_age_us: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.worker_age_us)
                .unwrap_or(0),
            portrait_preview_worker_generation_lag: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.generation_lag)
                .unwrap_or(0),
            portrait_preview_worker_affinity_status: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.affinity_status)
                .unwrap_or("inactive"),
            portrait_preview_worker_errors: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.worker_errors)
                .unwrap_or(0),
            portrait_preview_worker_adoption_failures: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.adoption_failures)
                .unwrap_or(0),
            portrait_preview_worker_alive: preview_compositor_telemetry
                .as_ref()
                .is_some_and(|telemetry| telemetry.worker_alive),
            crt_backdrop_prepare_us: crt_backdrop_work_trace.prepare_us,
            crt_backdrop_prepare_pixels: crt_backdrop_work_trace.prepare_pixels,
            crt_backdrop_blend_us: crt_backdrop_work_trace.blend_us,
            crt_backdrop_blend_pixels: crt_backdrop_work_trace.blend_pixels,
            crt_backdrop_copy_us,
            crt_backdrop_copy_pixels,
            crt_backdrop_list_overlay_us,
            crt_backdrop_list_overlay_pixels,
            crt_backdrop_alpha_bucket: crt_backdrop_work_trace.alpha_bucket,
            crt_backdrop_active: crt_backdrop_work_trace.active,
            crt_backdrop_selected: nav.arcade.selected,
            crt_backdrop_transition_id: crt_backdrop
                .as_ref()
                .and_then(CrtBackdropController::transition_id)
                .unwrap_or(0),
            crt_backdrop_cache_state: preview_cache_state_before_composition,
            effect_label_us,
            navigation_transition_base_copy_us: navigation_transition
                .last_render_stats()
                .base_copy_us as u128,
            navigation_transition_settings_blit_us: navigation_transition
                .last_render_stats()
                .settings_blit_us as u128,
            navigation_transition_card_scale_us: navigation_transition
                .last_render_stats()
                .card_scale_us as u128,
            navigation_transition_destination_reveal_us: navigation_transition
                .last_render_stats()
                .destination_reveal_us
                as u128,
            navigation_transition_overlay_us: navigation_transition.last_render_stats().overlay_us
                as u128,
            navigation_transition_edge: navigation_transition_route,
            navigation_transition_route,
            navigation_transition_direction,
            navigation_transition_renderer,
            navigation_transition_orientation: navigation_transition_frame_active
                .then(|| nav.settings.screen_orientation.id())
                .unwrap_or(""),
            settings_navigation_benchmark_leg: settings_navigation_benchmark.active_leg(),
            navigation_snapshot_locked: navigation_snapshot_locked_before_render,
            navigation_slint_render_called: !screensaver.active
                && !navigation_snapshot_locked_before_render,
            navigation_status_quiesce_wait_us: navigation_telemetry.status_quiesce_wait_us,
            navigation_status_quiesce_timeout: navigation_telemetry.status_quiesce_timeout,
            ..LauncherCustomDrawTrace::default()
        };
        let cpu_custom_draw_done = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let custom_draw_done = Instant::now();
        if !first_render_logged {
            first_render_logged = true;
            boot_analytics::event(
                "first_render",
                format!("frame={frames} dirty_rect={}", format_dirty_rect(this_rect)),
            );
        }
        let full_rect = DirtyRect {
            x0: 0,
            y0: 0,
            x1: layout.composition_w(),
            y1: layout.composition_h(),
        };
        let raw_preview_cached_rect = raw_preview.and_then(RawPreviewPresent::cached_rect);
        let logical_raw_preview_rect = (!layout.is_portrait())
            .then_some(raw_preview_cached_rect)
            .flatten();
        let physical_raw_preview_rect = layout
            .is_portrait()
            .then_some(raw_preview_cached_rect)
            .flatten();
        let logical_empty_preview_rect = (!layout.is_portrait())
            .then_some(empty_base_cached_rect)
            .flatten();
        let physical_empty_preview_rect = layout
            .is_portrait()
            .then_some(empty_base_cached_rect)
            .flatten();
        let raw_preview_direct_rect = raw_preview.and_then(RawPreviewPresent::direct_rect);
        if let Some(rect) = raw_preview_direct_rect {
            launcher_preview_version = launcher_preview_version.wrapping_add(1).max(1);
            if !crt_layout {
                let state = PhysicalLayerState::new(rect, launcher_preview_version);
                launcher_preview_publication = layer_target.capture_preview_publication(
                    state,
                    Some(PhysicalLayerUpdate::Full(rect)),
                    launcher_preview_version,
                );
            }
        } else if let Some((state, content_generation)) = reclaimed_preview_publication
            && !crt_layout
            && launcher_preview_publication.is_none()
        {
            launcher_preview_publication =
                layer_target.capture_preview_publication(state, None, content_generation);
        }
        let mut physical_arcade_rect = None;
        let mut direct_arcade_update = None;
        if !crt_backdrop_eligible {
            crt_arcade_overlay.clear();
        } else if crt_backdrop_work_trace.active || crt_backdrop_full_damage.is_some() {
            crt_arcade_overlay.invalidate();
        }
        let cached_arcade_rect = if crt_backdrop_eligible {
            arcade_list_rect
                .or_else(|| {
                    crt_backdrop_full_damage
                        .map(|_| ArcadeListUpdate::Full(arcade_list_renderer.dirty_rect()))
                })
                .and_then(|update| {
                    let rect = arcade_update_dirty_rect(&update);
                    let crt_overlay_profile_pmu =
                        mister_magik_perf_events::sampled_span("gui.custom.crt-list-overlay");
                    let composition = crt_backdrop
                        .as_ref()
                        .map(|backdrop| {
                            layer_target.compose_arcade_list_over_backdrop(
                                &mut arcade_list_renderer,
                                backdrop.pixels(),
                                update,
                                backdrop.backdrop_revision(),
                                catalog_version as u64,
                                crt_backdrop_full_damage.is_some(),
                                !backdrop.is_transitioning() && !crt_backdrop_work_trace.active,
                                full_frame_present || crt_backdrop_full_damage.is_some(),
                                &mut crt_arcade_overlay,
                            )
                        })
                        .unwrap_or_default();
                    crt_backdrop_list_overlay_us = composition.elapsed_us;
                    crt_backdrop_list_restore_pixels = composition.restored_pixels;
                    crt_backdrop_list_foreground_pixels = composition.foreground_pixels;
                    crt_backdrop_list_overlay_pixels = composition
                        .restored_pixels
                        .saturating_add(composition.foreground_pixels);
                    portrait_arcade_list_pixels = u64::from(crt_backdrop_list_overlay_pixels);
                    portrait_arcade_list_bytes = portrait_arcade_list_pixels.saturating_mul(2);
                    drop(crt_overlay_profile_pmu);
                    if layout.is_portrait() {
                        physical_arcade_rect = Some(layout.logical_rect_to_composition(rect));
                        None
                    } else {
                        Some(rect)
                    }
                })
        } else if layout.is_portrait() {
            arcade_list_rect.and_then(|update| {
                let _ = layer_target.reclaim_arcade_publication(
                    &mut arcade_list_renderer,
                    &mut launcher_arcade_publication,
                );
                let (composition, physical_update) = layer_target.compose_arcade_list_direct_layer(
                    &mut arcade_list_renderer,
                    update,
                    catalog_version as u64,
                );
                custom_draw_trace.persistent_arcade_composition =
                    arcade_list_renderer.persistent_composition_trace();
                portrait_arcade_list_bytes = composition.bytes as u64;
                portrait_arcade_list_pixels = composition.bytes.saturating_div(2) as u64;
                direct_arcade_update = Some(physical_update);
                None
            })
        } else if crt_layout {
            arcade_list_rect.and_then(|update| {
                let rect = arcade_update_dirty_rect(&update);
                let composition =
                    layer_target.compose_arcade_list_update(&mut arcade_list_renderer, update);
                portrait_arcade_list_bytes = composition.bytes as u64;
                portrait_arcade_list_pixels = composition.bytes.saturating_div(2) as u64;
                Some(rect)
            })
        } else {
            None
        };
        let layer_arcade_update = direct_arcade_update.or(arcade_list_rect);
        if !crt_layout {
            update_arcade_physical_layer_tracking(
                &mut launcher_arcade_version,
                &mut launcher_arcade_scroll_offset,
                layer_arcade_update,
                layout.is_portrait(),
            );
        }
        if layout.is_portrait()
            && let Some(update) = direct_arcade_update
            && let Some(rect) = arcade_list_renderer
                .persistent_oriented_layer_view()
                .map(PhysicalLayerView::rect)
        {
            launcher_arcade_content_generation =
                launcher_arcade_content_generation.wrapping_add(1).max(1);
            let state = PhysicalLayerState::new(rect, launcher_arcade_version)
                .with_content_offset(launcher_arcade_scroll_offset);
            launcher_arcade_publication = layer_target.capture_arcade_publication(
                &mut arcade_list_renderer,
                state,
                Some(update),
                launcher_arcade_content_generation,
            );
        }
        custom_draw_trace.crt_backdrop_copy_us = crt_backdrop_copy_us;
        custom_draw_trace.crt_backdrop_copy_pixels = crt_backdrop_copy_pixels;
        custom_draw_trace.crt_backdrop_list_overlay_us = crt_backdrop_list_overlay_us;
        custom_draw_trace.crt_backdrop_list_overlay_pixels = crt_backdrop_list_overlay_pixels;
        custom_draw_trace.crt_backdrop_list_restore_pixels = crt_backdrop_list_restore_pixels;
        custom_draw_trace.crt_backdrop_list_foreground_pixels = crt_backdrop_list_foreground_pixels;
        custom_draw_trace.portrait_arcade_list_pixels = portrait_arcade_list_pixels;
        custom_draw_trace.portrait_arcade_list_bytes = portrait_arcade_list_bytes;
        let physical_custom_damage = accepted_screensaver_frame.then_some(this_rect).flatten();
        let preview_layer_desired = should_desire_preview_direct_layer(
            wants_preview_layer,
            composition_decision.allow_preview_blit,
            wants_preview,
            preview_compositor_pending,
            launcher_preview_publication.is_some() || layer_target.direct_preview_rect().is_some(),
            raw_preview_direct_rect.is_some(),
        );
        let mut preview_publication =
            if !crt_layout && preview_layer_desired && preview_direct_present_enabled() {
                launcher_preview_publication
                    .as_ref()
                    .filter(|publication| {
                        publication.layout_generation() == layer_target.output_layout_generation()
                            && publication.layout_epoch() == layer_target.output_layout_epoch()
                    })
                    .and_then(|publication| {
                        publication.for_frame(
                            publication.state(),
                            raw_preview_direct_rect.map(PhysicalLayerUpdate::Full),
                        )
                    })
            } else {
                None
            };
        let preview_desired = preview_publication
            .as_ref()
            .map(PhysicalLayerPublication::state);
        let mut arcade_publication = if layout.is_portrait()
            && !crt_layout
            && should_desire_direct_layer(
                wants_arcade_list,
                composition_decision.allow_arcade_list_blit,
            ) {
            launcher_arcade_publication
                .as_ref()
                .filter(|publication| {
                    publication.layout_generation() == layer_target.output_layout_generation()
                        && publication.layout_epoch() == layer_target.output_layout_epoch()
                })
                .and_then(|publication| {
                    publication.for_frame(publication.state(), direct_arcade_update)
                })
        } else {
            None
        };
        let arcade_desired = if layout.is_portrait() {
            arcade_publication
                .as_ref()
                .map(PhysicalLayerPublication::state)
        } else if !crt_layout
            && should_desire_direct_layer(
                wants_arcade_list,
                composition_decision.allow_arcade_list_blit,
            )
        {
            let rect = arcade_list_renderer.dirty_rect();
            Some(
                PhysicalLayerState::new(rect, launcher_arcade_version)
                    .with_content_offset(launcher_arcade_scroll_offset),
            )
        } else {
            None
        };
        let mut logical_custom_damage = DirtyRectList::new();
        if navigation_logical_frame_rendered {
            logical_custom_damage.push(DirtyRect {
                x0: 0,
                y0: 0,
                x1: layout.logical_w(),
                y1: layout.logical_h(),
            });
        } else if slint_damage.is_empty() && physical_custom_damage.is_none() {
            logical_custom_damage.push_if_some(this_rect);
        }
        logical_custom_damage.push_if_some(logical_empty_preview_rect);
        logical_custom_damage.push_if_some(logical_raw_preview_rect);
        logical_custom_damage.push_if_some(cached_arcade_rect);
        let orientation_damage_rects_before = logical_custom_damage.len() as u32;
        debug_assert!(!layout.is_portrait() || logical_custom_damage.is_empty());
        let mapped_custom_damage = logical_custom_damage;
        let mut cached_damage = if full_frame_present || navigation_settings_physical_space {
            DirtyRectList::from_one(full_rect)
        } else {
            let mut damage = slint_damage;
            damage.extend_from(&mapped_custom_damage);
            damage.push_if_some(physical_custom_damage);
            damage.push_if_some(physical_arcade_rect);
            damage.push_if_some(physical_empty_preview_rect);
            damage.push_if_some(physical_raw_preview_rect);
            damage.push_if_some(crt_backdrop_full_damage);
            damage
        };
        // Retain the v1 telemetry field for schema compatibility. Native Slint
        // and custom layer composition no longer run a post-raster rotation.
        let orientation_damage_rotation_us = 0;
        let orientation_damage_rects_after_rotation = cached_damage.len() as u32;
        if orientation_transition.is_active() {
            let orientation_started = Instant::now();
            let transition_from = orientation_transition.from();
            let transition_to = orientation_transition.to();
            custom_draw_trace.orientation_transition_active = true;
            custom_draw_trace.orientation_transition_from = transition_from.id();
            custom_draw_trace.orientation_transition_to = transition_to.id();
            custom_draw_trace.orientation_transition_leg = orientation_benchmark
                .active_leg()
                .map_or(0, |leg| (leg.index + 1).min(u8::MAX as usize) as u8);
            custom_draw_trace.orientation_transition_effect = orientation_transition.effect().id();
            let preparation_trace = std::mem::take(&mut orientation_preparation_trace);
            custom_draw_trace.orientation_begin_us = preparation_trace.begin_us;
            custom_draw_trace.orientation_source_snapshot_us = preparation_trace.source_snapshot_us;
            custom_draw_trace.orientation_layout_us = preparation_trace.layout_us;
            custom_draw_trace.orientation_source_snapshot_bytes =
                preparation_trace.source_snapshot_bytes;
            custom_draw_trace.orientation_controlled_slint_raster_us =
                orientation_controlled_slint_raster_us;
            custom_draw_trace.orientation_damage_rotation_us = orientation_damage_rotation_us;
            custom_draw_trace.orientation_damage_rects_before = orientation_damage_rects_before;
            custom_draw_trace.orientation_damage_rects_after =
                orientation_damage_rects_after_rotation;
            if !orientation_transition.destination_ready()
                && full_screen_controlled_capture_rendered
            {
                let capture_started = Instant::now();
                let destination_pmu =
                    mister_magik_perf_events::sampled_span(orientation_pmu_label(
                        orientation_transition.effect(),
                        transition_from,
                        transition_to,
                        OrientationPmuPhase::Destination,
                    ));
                let captured = orientation_transition
                    .capture_destination(layer_target.presentation_frame_view().pixels());
                drop(destination_pmu);
                custom_draw_trace.orientation_transition_destination_capture_us =
                    capture_started.elapsed().as_micros();
                custom_draw_trace.orientation_destination_snapshot_bytes = layer_target
                    .presentation_frame_view()
                    .pixels()
                    .len()
                    .saturating_mul(2)
                    as u64;
                if captured {
                    if let Some(generation) = orientation_transition_generation
                        && let Err(error) = full_screen_transition.capture_completed(generation)
                    {
                        crate::ui_errln!("orientation snapshot lock rejected: {error:?}");
                        orientation_transition.cancel();
                        release_full_screen_transition(
                            &mut full_screen_transition,
                            orientation_transition_generation,
                        );
                    }
                } else {
                    orientation_transition.cancel();
                    release_full_screen_transition(
                        &mut full_screen_transition,
                        orientation_transition_generation,
                    );
                }
            }
            let gui_orientation_pmu =
                gui_profiling.phase_span(gui_custom_selection.orientation_transition_raster);
            let orientation_rendered = (!orientation_capture_source_carrier_rendered).then(|| {
                orientation_transition
                    .render_into(layer_target.presentation_pixels_mut(), Instant::now())
            });
            drop(gui_orientation_pmu);
            if let Some(Some((done, render_stats, transition_damage))) = orientation_rendered {
                custom_draw_trace.orientation_transition_stats = render_stats;
                custom_draw_trace.orientation_effect_read_bytes =
                    render_stats.blended_pixels.saturating_mul(2);
                custom_draw_trace.orientation_effect_write_bytes =
                    render_stats.blended_pixels.saturating_mul(2);
                let damage_build_started = Instant::now();
                cached_damage.clear();
                for row in 0..9 {
                    if let Some((x0, y0, x1, y1)) =
                        transition_damage.rect_for_row(row, ui.render_w(), ui.render_h())
                    {
                        cached_damage.push(DirtyRect { x0, y0, x1, y1 });
                    }
                }
                custom_draw_trace.orientation_damage_build_us =
                    damage_build_started.elapsed().as_micros();
                custom_draw_trace.orientation_damage_rects_after = cached_damage.len() as u32;
                if done {
                    let _ = orientation_transition.take_completion();
                    release_full_screen_transition(
                        &mut full_screen_transition,
                        orientation_transition_generation,
                    );
                    match orientation_transition_intent.take() {
                        Some(OrientationTransitionIntent::Confirm) => {
                            orientation_confirm_deadline = Some(
                                Instant::now()
                                    + Duration::from_secs(u64::from(
                                        launcher::DISPLAY_CONFIRM_SECONDS,
                                    )),
                            );
                        }
                        Some(OrientationTransitionIntent::Benchmark) => {
                            orientation_benchmark.note_rendered_endpoint(frames);
                        }
                        Some(OrientationTransitionIntent::Rollback) | None => {}
                    }
                } else {
                    window.request_redraw();
                }
            }
            custom_draw_trace.orientation_transition_total_us =
                orientation_started.elapsed().as_micros();
        }
        cached_damage =
            shield_base_damage_under_publication(cached_damage, &mut preview_publication);
        cached_damage =
            shield_base_damage_under_publication(cached_damage, &mut arcade_publication);
        // CRT routes do not own an HDMI preview layer, so the normal preview
        // presentation acknowledgement can never fire for them.  Without a
        // route-specific acknowledgement the preview remains `animating`
        // forever, keeping the launcher awake and allowing the Slint base
        // raster to overwrite the settled CRT backdrop between list ticks.
        let crt_backdrop_target_presented = crt_backdrop_frame_is_presented(
            navigation_transition_composition_active,
            crt_backdrop_full_damage.is_some(),
            crt_backdrop_work_trace.active,
            preview_cache_state_before_composition == "exact",
            preview.raw_frame_status() == PreviewRawFrameStatus::Ready,
            crt_backdrop
                .as_ref()
                .is_some_and(CrtBackdropController::is_transitioning),
        );
        let final_preview_target_presented = (raw_preview.is_some()
            || crt_backdrop_target_presented)
            && preview.presentation_requires_present()
            && preview_transition_trace.progress >= 1.0;
        let cached_empty_target_presented = (layout.is_portrait()
            || !preview_direct_present_enabled())
            && final_preview_target_presented
            && raw_preview_cached_rect.is_some()
            && matches!(
                preview.presentation_state(),
                PreviewPresentationState::Animating {
                    target: PreviewPresentationTarget::Empty,
                    ..
                }
            );
        let preview_presentation_commit = preview.presentation_commit(
            final_preview_target_presented,
            empty_base_cached_rect.is_some() || cached_empty_target_presented,
        );
        drop(gui_custom_generation_pmu);
        if full_screen_transition.state() != FullScreenTransitionState::Live {
            record_launcher_frame_phase!(LauncherFramePhase::FullScreenTransition);
        }
        let frame_plan = if layout.is_portrait() {
            LauncherFramePlan::from_publications(
                cached_damage,
                preview_publication,
                arcade_publication,
            )
        } else if !crt_layout {
            LauncherFramePlan::from_preview_publication_and_cached_arcade(
                cached_damage,
                preview_publication,
                arcade_desired,
                arcade_list_rect,
            )
        } else {
            LauncherFramePlan::from_cached_layers(
                cached_damage,
                preview_desired,
                raw_preview_direct_rect,
                arcade_desired,
                if crt_layout { None } else { arcade_list_rect },
            )
        };
        record_launcher_frame_phase!(LauncherFramePhase::FramePlanned);
        let startup_can_present = lifecycle.startup_can_present_frame();
        let stream_motion_active = stream_motion_before_render
            || preview_transition_trace.active
            || navigation_transition_composition_active;
        let direct_hidden_present_mode =
            startup_intro.is_some() || completed_hidden_frame_for_present.is_some();
        drop(frame_plan_pmu);
        let hidden_present_pmu = launcher_response_trace.input_pmu_span(
            latency_critical_input_pending,
            "launcher-response.hidden-present",
        );
        let present_cycle = launcher_presenter.present(
            LauncherPresentFrame {
                plan: frame_plan,
                startup_can_present,
                first_visible_copy_done: frame_accounting.first_visible_copy_done(),
                frame_start_phase_us,
                pre_render_pace,
                frame_analytics_mode,
                stream_motion_active,
                direct_hidden_mode: direct_hidden_present_mode,
                completed_hidden_frame: completed_hidden_frame_for_present,
                capture_readiness_source: launcher_readiness.needs_source_evidence(),
                profile_latch_phases: gui_profiling.active(),
            },
            LauncherPresentTargets {
                layer_target: &layer_target,
                fb0: disp,
                hardware: f,
                arcade_list_renderer: &mut arcade_list_renderer,
                pacer: &mut pacer,
                present_timing,
            },
            display_session,
        );
        drop(hidden_present_pmu);
        let LauncherPresentCycle {
            presentation,
            frame_t3,
            frame_t4,
            cpu_t3,
            cpu_t4,
            pacing_trace,
        } = present_cycle;
        record_launcher_frame_phase!(LauncherFramePhase::FrameSubmitted);
        if let Some(worker) = preview_compositor.as_ref() {
            worker.release_queued();
        }
        let readiness_source_evidence = presentation.readiness_source_evidence.clone();
        gui_profiling.record_latch(
            frames,
            presentation.main_present_hidden_copied_bytes,
            presentation.main_present_hidden_invalid_bytes,
            presentation.main_present_hidden_catchup_bytes,
            presentation.main_present_hidden_rect_count,
            presentation.main_present_hidden_full_copy,
            presentation.main_present_buffer,
            presentation.main_present_copy_path,
            presentation.arcade_copy_trace,
        );
        scheduler_phase =
            launcher_response_trace.record_scheduler_interval("raster-and-post", scheduler_phase);
        if let Some(completed_at) = frame_production_completed_at {
            frame_production_trace.ready_age_us = frame_t3
                .saturating_duration_since(completed_at)
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX);
        }
        if let Some(frame_started) = navigation_transition_frame_started {
            navigation_transition.note_frame_work_us(
                frame_started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            );
        }
        if accepted_screensaver_frame
            && screensaver_pipeline.is_some()
            && presentation.main_present_backend.is_latch()
            && presentation.main_present_status == LauncherPresentStatus::Ok
            && let Some(pipeline) = screensaver_pipeline.as_mut()
            && let Err(error) = pipeline.confirm_presented(screensaver_render_sequence)
        {
            crate::ui_errln!(
                "screensaver: shared screenshot confirmation failed: {error}; restoring launcher"
            );
            screensaver.fail_current_activation(Instant::now());
            if let Some(pipeline) = screensaver_pipeline.take() {
                pipeline.cancel();
                retiring_screensaver_pipelines.push(pipeline);
            }
            screensaver_frame_visible = false;
            window.request_redraw();
        }
        if let Some(pixels) = screensaver_buffer_to_recycle_after_present.take()
            && let Some(pipeline) = screensaver_pipeline.as_ref()
        {
            let _ = pipeline.recycle(pixels);
        }
        if presentation.main_present_backend.is_latch() {
            phase_alignment.observe(
                frame_t4
                    .saturating_duration_since(frame_t0)
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
        }
        if let Some(failure) = launcher_presenter.latch_failure() {
            frame_accounting.record_latch_failure(failure);
        }
        app.global::<slint_ui::launcher::MisterBridge>()
            .set_present_mode_label(
                present_mode_label_for_backend_status(
                    presentation.main_present_backend,
                    presentation.main_present_status,
                )
                .into(),
            );
        let post_present_wait_us = if presentation.main_present_backend.is_latch() {
            presentation.vsync_us_override.unwrap_or(0)
        } else {
            0
        };
        let latch_trace_flush_deferred = presentation.main_present_backend.is_latch();
        if !latch_trace_flush_deferred {
            record_launcher_frame_phase!(LauncherFramePhase::CompatibilityResolved);
        }
        if !first_vsync_logged && pacing_trace.vsync_source == Some(VsyncPaceSource::Vsync) {
            first_vsync_logged = true;
            boot_analytics::event("first_vsync", format!("frame={frames}"));
        }
        let visible_frame_presented = visible_frame_was_presented(
            presentation.copied_rows,
            presentation.main_present_status,
            presentation.main_present_copy_path,
        );
        // Posting a buffer and observing it pending proves latch acceptance,
        // not physical presentation. The intro advances only after the final
        // active-sequence confirmation below.
        let startup_intro_frame_posted = visible_frame_presented && accepted_startup_intro_frame;
        if navigation_transition_frame_active && visible_frame_presented {
            if settings_navigation_benchmark.enabled() {
                screensaver_cpu_profile
                    .begin_settings_navigation_transition(frames.saturating_add(1));
            } else {
                screensaver_cpu_profile.begin_navigation_transition(frames.saturating_add(1));
            }
        }
        if screensaver.active && visible_frame_presented {
            // Profile only completed screensaver output. Starting when Preview is pressed
            // includes loader/render-worker startup frames that have no presentation evidence.
            screensaver_cpu_profile.begin_screensaver(frames.saturating_add(1));
            if screensaver_first_render_logged && !screensaver_first_present_logged {
                screensaver_first_present_logged = true;
                if let Some(started) = screensaver_show_started {
                    crate::ui_logln!(
                        "screensaver_startup_timing milestone=first_saver_present elapsed_us={}",
                        started.elapsed().as_micros()
                    );
                }
            }
            if !screensaver_first_card_present_logged && accepted_screensaver_frame {
                screensaver_first_card_present_logged = true;
                if let Some(started) = screensaver_show_started {
                    crate::ui_logln!(
                        "screensaver_startup_timing milestone=first_card_visible elapsed_us={}",
                        started.elapsed().as_micros()
                    );
                }
            }
        }
        if visible_frame_presented && startup_intro.is_none() {
            if !first_launcher_frame_logged
                && lifecycle.startup_status().state == StartupRevealState::RevealLauncher
            {
                first_launcher_frame_logged = true;
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                let nav_menu_items = nav.current_menu_count();
                let bridge_menu_items = bridge.get_menu_items().row_count();
                print_startup_event(
                    start,
                    "launcher_first_frame_presented",
                    format!(
                        "screen={} systems={} nav_menu_items={} bridge_menu_items={} catalog_ready={}",
                        screen_label(nav.screen),
                        catalog.systems.len(),
                        nav_menu_items,
                        bridge_menu_items,
                        u8::from(catalog_ready)
                    ),
                );
                catalog_publication_test.hold_first_launcher_frame(start);
            }
            lifecycle.note_startup_frame_presented(frames, frame_t4, &mut lifecycle_effects);
            if first_launcher_frame_logged
                && lifecycle.startup_status().input_enabled
                && profile_config.cpu().cold_boot_requested()
                && cpu.is_some()
                && let Err(error) =
                    cpu_profile::finish_cold_boot_async(cpu.take(), profile_config.cpu())
            {
                crate::ui_errln!("cold-boot cpu profile finalization failed: {error}");
            }
            if lifecycle.startup_status().mode == StartupMode::ReturnFromGame
                && lifecycle.startup_status().revealed
            {
                launch_return_session.mark_correct_present(&nav, &catalog);
                if launch_return_session.first_correct_present_monotonic_us != 0
                    && profile_config.cpu().launch_return_requested()
                    && cpu.is_some()
                    && let Err(error) =
                        cpu_profile::finish_launch_return_async(cpu.take(), profile_config.cpu())
                {
                    crate::ui_errln!("launch-return cpu profile finalization failed: {error}");
                }
                if catalog_session.refresh_done() {
                    launch_return_session.release_if_complete();
                }
            }
            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
        }
        let presented_copied_rows = presentation.copied_rows;
        arcade_entry_latency.record_destination_prepared_frame(
            start,
            frame_t4,
            &lifecycle,
            &catalog,
            &nav,
            &preview,
            frames,
            prepare_us,
            presented_copied_rows,
            catalog_version,
        );
        arcade_entry_latency.record_presented_frame(
            start,
            frame_t4,
            &lifecycle,
            &catalog,
            &nav,
            &preview,
            frames,
            prepare_us,
            presented_copied_rows,
        );
        gui_profiling.record_frame_work(GuiFrameWorkRecord::from_traces(
            frames,
            frame_t4.saturating_duration_since(loop_start).as_micros(),
            presentation.vsync_us_override.unwrap_or_else(|| {
                frame_t3
                    .saturating_duration_since(custom_draw_done)
                    .as_micros()
            }),
            &custom_draw_trace,
            &presentation,
        ));
        let mut presented_frame = LauncherFrameSnapshotBuilder {
            identity: LauncherFrameIdentity {
                frames,
                automation: automation_frame_stamp,
                selection_feedback: bridge_models.selection_feedback_stamp(),
                selected: nav.arcade.selected,
                visual_index: nav.arcade.visual_index,
                #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
                home_trace: LauncherHomeFrameTrace::from_nav(&nav),
                search_index_state: match nav.arcade_search.status {
                    launcher::ArcadeSearchStatus::Idle => "idle",
                    launcher::ArcadeSearchStatus::Searching => "searching",
                    launcher::ArcadeSearchStatus::Ready => "ready",
                    launcher::ArcadeSearchStatus::Failed => "failed",
                },
            },
            timing: LauncherFrameTiming {
                startup_start: start,
                startup_monotonic_us,
                run_start,
                loop_start,
                frame_t0,
                frame_t1,
                frame_t2,
                frame_t3,
                frame_t4,
                pre_render_wait_us,
                post_present_wait_us,
                custom_draw_start,
                custom_draw_done,
                prepare_us,
                home_pan_present_active,
                home_horizontal_input_held,
                redraw_pending: redraw_pending_for_trace,
                wake_reasons_bits,
            },
            render: LauncherFrameRenderData {
                custom_draw_trace,
                prepare_trace,
                dirty_rect: this_rect,
                preview_cache_state: preview.trace_cache_state(),
                preview_transition: preview_transition_trace,
                composition_status: composition_status.clone(),
                screensaver_active: screensaver.active && screensaver_pipeline.is_some(),
                screensaver_active_cards,
                frame_production_trace,
                screensaver_render_trace,
            },
            pacing: pacing_trace,
            presentation,
            status: LauncherFrameStatusData {
                status_write_due,
                status_string_copy_us,
                status_string_copy_bytes,
                clock_update_due,
                clock_update_us,
            },
            cpu: LauncherFrameCpuTrace {
                loop_start: cpu_loop_start,
                t0: cpu_t0,
                t1: cpu_t1,
                t2: cpu_t2,
                custom_draw_start: cpu_custom_draw_start,
                custom_draw_done: cpu_custom_draw_done,
                t3: cpu_t3,
                t4: cpu_t4,
            },
        }
        .build();
        let launcher_response_present_receipt = LauncherResponsePresentReceipt {
            post_accepted_at_us: crate::input_hub::monotonic_us(),
            post_accepted_execution: launcher_response_trace.execution_stamp(),
            dirty_rect: presented_frame
                .dirty_rect
                .map(|rect| (rect.x0, rect.y0, rect.x1, rect.y1)),
            present_bytes: presented_frame.present_bytes,
            wasted_present_bytes: presented_frame.wasted_present_bytes,
            cached_present_us: launcher_response_u64(presented_frame.cached_present_us),
            hidden_compose_us: launcher_response_u64(presented_frame.hidden_compose_us),
            hidden_copy_us: launcher_response_u64(presented_frame.main_present_hidden_copy_us),
            hidden_publish_us: launcher_response_u64(
                presented_frame.main_present_hidden_publish_us,
            ),
            hidden_invalid_bytes: presented_frame.main_present_hidden_invalid_bytes,
            hidden_rect_count: presented_frame.main_present_hidden_rect_count,
            hidden_catchup_bytes: presented_frame.main_present_hidden_catchup_bytes,
            hidden_full_copy: presented_frame.main_present_hidden_full_copy,
            hidden_copy_path: presented_frame.main_present_copy_path,
            present_request_us: launcher_response_u64(presented_frame.main_present_request_us),
            set_vga_fb_us: launcher_response_u64(presented_frame.main_present_set_vga_fb_us),
            present_wait_us: presented_frame.main_present_wait_us,
            posted_sequence: presented_frame.main_present_sequence,
            post_active_sequence: presented_frame.main_present_post_active_sequence,
            post_pending_sequence: presented_frame.main_present_post_pending_sequence,
            post_pending: presented_frame.main_present_post_pending,
            refresh_period_us: pacer.period_us(),
        };
        let selection_feedback_stamp = presented_frame.selection_feedback.clone();
        let mut accepted_and_active_confirmed = false;
        let mut confirmed_present_sequence = 0u16;
        let mut confirmed_direct_layer_receipt = None;
        let mut selection_feedback_confirmed_at =
            (!latch_trace_flush_deferred && visible_frame_presented).then_some(frame_t4);
        let runtime_status_sequence_before_frame = settings_navigation_benchmark
            .enabled()
            .then(|| frame_accounting.runtime_status_submitted_sequence())
            .unwrap_or_default();
        if latch_trace_flush_deferred {
            let finish_timing = frame_accounting.finish_frame_before_trace(
                &presented_frame,
                &nav,
                &pad,
                &catalog,
                catalog_ready,
                catalog_session.refresh_done(),
                launching,
                scheduler.visible_loading_title(&loading_title),
                catalog_scan_visible,
                status_text
                    .as_ref()
                    .map(|text| text.catalog_scan_title.as_str())
                    .unwrap_or(""),
                status_text
                    .as_ref()
                    .map(|text| text.catalog_scan_detail.as_str())
                    .unwrap_or(""),
                catalog_scan_percent,
                catalog_background_scan_visible,
                status_text
                    .as_ref()
                    .map(|text| text.catalog_scan_message.as_str())
                    .unwrap_or(""),
                confirm_visible,
                status_text
                    .as_ref()
                    .map(|text| text.confirm_title.as_str())
                    .unwrap_or(""),
                status_text
                    .as_ref()
                    .map(|text| text.confirm_message.as_str())
                    .unwrap_or(""),
                confirm_selected,
                status_text
                    .as_ref()
                    .map(|text| text.confirm_left_label.as_str())
                    .unwrap_or(""),
                status_text
                    .as_ref()
                    .map(|text| text.confirm_right_label.as_str())
                    .unwrap_or(""),
                launcher_bench_scenario,
                start_screen,
                lock_screen,
                display_session.reassert_count(),
                display_session.last_reassert_frame(),
                display_session.last_reassert_ok(),
                display_session.last_reassert_error(),
                lifecycle.startup_status(),
                &launch_return_session,
            );
            record_launcher_frame_phase!(LauncherFramePhase::PostSubmitAccounted);
            // Latch mode posts the hidden buffer first, then spends the slack before
            // vblank on normal per-frame accounting. The final wait is only the
            // pacing boundary for the next frame.
            let wait_start = Instant::now();
            scheduler_phase = launcher_response_trace
                .record_scheduler_interval("post-submit-accounting", scheduler_phase);
            let interruptible_home_wait = can_preempt_home_latch_wait(
                nav.screen,
                launcher_response_frame_stamp.is_some(),
                !selection_feedback_stamp.entries.is_empty(),
                navigation_transition.is_active()
                    || orientation_transition.is_active()
                    || full_screen_transition.state() != FullScreenTransitionState::Live,
                screensaver.active,
                composition_decision.state != UiCompositionState::FullSlint
                    || composition_decision.retirement_generation.is_some(),
                preview_presentation_commit.is_some(),
                startup_intro_frame_posted,
            );
            let pace = match pacer.wait_interruptible(|| {
                interruptible_home_wait
                    && matches!(
                        pad.wait_for_input(input_observation, Duration::ZERO),
                        crate::input_hub::InputWaitOutcome::Changed
                    )
            }) {
                VsyncWaitOutcome::Pace(pace) => pace,
                VsyncWaitOutcome::Interrupted => {
                    launcher_response_trace.record_lab(Some(serde_json::json!({
                        "phase": "latch-wait-interrupted-input",
                        "interrupted_at_us": crate::input_hub::monotonic_us(),
                        "posted_sequence": presented_frame.main_present_sequence,
                    })));
                    let _ = launcher_response_trace.record_scheduler_interval(
                        "latch-confirmation-wait-interrupted",
                        scheduler_phase,
                    );
                    record_launcher_frame_phase!(LauncherFramePhase::ConfirmationInterrupted);
                    request_launcher_redraw!();
                    record_launcher_frame_phase!(LauncherFramePhase::Yielded);
                    continue 'launcher;
                }
            };
            let completion_timeout = Duration::from_micros(pacer.period_us().saturating_mul(3) / 2);
            let completion_remaining = completion_timeout.saturating_sub(wait_start.elapsed());
            let completion_poll_pmu = gui_profiling.span("gui.latch.completion-polling");
            let completion = wait_for_latch_completion(
                f,
                presented_frame.main_present_sequence,
                completion_remaining,
            );
            drop(completion_poll_pmu);
            let wait_done = Instant::now();
            scheduler_phase = launcher_response_trace
                .record_scheduler_interval("latch-confirmation-wait", scheduler_phase);
            let post_wait_us = wait_done.saturating_duration_since(wait_start).as_micros();
            let wait_trace = LauncherPacingTrace::from_pace_with_present_phase(
                Some(&pace),
                presented_frame.frame_start_phase_us,
                pacer.period_us(),
                presented_frame.present_phase_us,
            );
            presented_frame.frame_t4 = wait_done;
            presented_frame.post_present_wait_us = post_wait_us;
            presented_frame.vsync_us_override = Some(post_wait_us);
            presented_frame.cpu_t4 = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
            presented_frame.vsync_source = wait_trace.vsync_source;
            presented_frame.vsync_period_us = wait_trace.vsync_period_us;
            presented_frame.vsync_miss_streak = wait_trace.vsync_miss_streak;
            presented_frame.vsync_stale_hits = wait_trace.vsync_stale_hits;
            presented_frame.vsync_wait_start_age_us = wait_trace.vsync_wait_start_age_us;
            presented_frame.vsync_accepted_hit_age_us = wait_trace.vsync_accepted_hit_age_us;
            let mut readiness_post = None;
            match completion {
                Ok(completion) => {
                    let status = completion.status;
                    readiness_post = Some(super::launcher_readiness::ConfirmedLatchPost {
                        sequence: status.active_sequence,
                        route_epoch: status.active_route_epoch,
                        slot: presented_frame.main_present_buffer,
                        receipt_crc: presented_frame.main_present_receipt_crc,
                        active_base: status.active_base,
                        width: status.active_width,
                        height: status.active_height,
                        stride: status.active_stride,
                    });
                    presented_frame.main_present_active_sequence = status.active_sequence;
                    presented_frame.main_present_pending = status.pending();
                    presented_frame.main_present_flip_count = status.flip_count;
                    presented_frame.main_present_drop_count = status.drop_count;
                    presented_frame.main_present_completion_poll_count = completion.poll_count;
                    presented_frame.main_present_completion_poll_wall_us = completion.wall_us;
                    presented_frame.main_present_completion_poll_cpu_us = completion.cpu_us;
                    confirmed_direct_layer_receipt = Some(DirectLayerPresentationReceipt {
                        sequence: status.active_sequence,
                        slot: presented_frame.main_present_buffer,
                        route_epoch: status.active_route_epoch,
                        carrier: composition_decision.retirement_carrier,
                    });
                }
                Err(failure) => {
                    if let Some(generation) = composition_decision.retirement_generation {
                        let _ = composition.mark_retirement_uncertain(generation);
                    }
                    launcher_presenter.fail_latch_completion(failure);
                    if let Some(failure) = launcher_presenter.latch_failure() {
                        frame_accounting.record_latch_failure(failure);
                    }
                    presented_frame.main_present_active_sequence = 0;
                    presented_frame.main_present_pending = true;
                }
            }
            accepted_and_active_confirmed = presented_frame.main_present_sequence != 0
                && presented_frame.main_present_active_sequence
                    == presented_frame.main_present_sequence
                && !presented_frame.main_present_pending
                && launcher_presenter.latch_failure().is_none();
            if accepted_and_active_confirmed {
                record_launcher_frame_phase!(LauncherFramePhase::ActiveConfirmed);
                confirmed_present_sequence = presented_frame.main_present_sequence;
                let confirmed_at = pace.hit_at.unwrap_or(wait_done);
                selection_feedback_confirmed_at = Some(confirmed_at);
                if orientation_capture_source_carrier_rendered {
                    if !orientation_transition.restart_animation(Instant::now()) {
                        orientation_benchmark.fail("orientation-carrier-restart-failed");
                    } else if orientation_benchmark.enabled() {
                        orientation_benchmark.capture_presentation_start(
                            Instant::now(),
                            f.read_magik_presentation_telemetry(),
                        );
                    }
                    request_launcher_redraw!();
                }
                if navigation_capture_source_carrier_rendered
                    && settings_navigation_benchmark.enabled()
                {
                    let telemetry = f.read_magik_presentation_telemetry();
                    settings_navigation_benchmark
                        .capture_presentation_start(Instant::now(), telemetry);
                }
                if arcade_entry_latency.record_ready_presented_frame(
                    start,
                    confirmed_at,
                    &lifecycle,
                    &catalog,
                    &nav,
                    &preview,
                    frames,
                    prepare_us,
                    presented_copied_rows,
                    true,
                    catalog_version,
                    confirmed_present_sequence,
                    f.read_magik_presentation_telemetry().ok(),
                    presented_frame.main_present_drop_count,
                    SystemEntryPublicationPhases::from_presented_frame(&presented_frame),
                ) && let Err(error) = cpu_profile::finish_system_entry_async(
                    system_entry_cpu_profile.take(),
                    profile_config.cpu(),
                ) {
                    crate::ui_errln!("system-entry cpu profile finish failed: {error}");
                }
                settings_navigation_benchmark
                    .note_orientation_presented(nav.settings.screen_orientation);
            }
            if accepted_and_active_confirmed
                && full_screen_transition_release_raster_rendered
                && let Some(generation) = full_screen_transition.generation()
            {
                let owner = full_screen_transition.owner();
                match full_screen_transition.live_frame_presented(generation) {
                    Ok(retained_redraw) => {
                        match owner {
                            Some(FullScreenTransitionOwner::Navigation) => {
                                navigation_transition_generation = None;
                                let benchmark_record = if settings_navigation_benchmark.enabled() {
                                    let telemetry = f.read_magik_presentation_telemetry();
                                    settings_navigation_benchmark.note_confirmed_presentation(
                                        nav.screen,
                                        frames,
                                        confirmed_present_sequence,
                                        Instant::now(),
                                        telemetry,
                                    )
                                } else {
                                    None
                                };
                                if let Some(record) = benchmark_record {
                                    print_startup_event(
                                        start,
                                        "settings_navigation_benchmark_leg_completed",
                                        format!(
                                            concat!(
                                                "leg={} route={} direction={} source={} destination={} ",
                                                "start_frame={} rendered_endpoint_frame={} ",
                                                "presented_endpoint_frame={} sequence={}"
                                            ),
                                            settings_navigation_benchmark.records().len(),
                                            record.leg.route.label(),
                                            record.leg.direction.label(),
                                            screen_label(record.leg.source),
                                            screen_label(record.leg.destination),
                                            record.start_frame,
                                            record.rendered_endpoint_frame,
                                            record.presented_endpoint_frame,
                                            record.presented_sequence,
                                        ),
                                    );
                                }
                            }
                            Some(FullScreenTransitionOwner::Orientation) => {
                                orientation_transition_generation = None;
                            }
                            _ => {}
                        }
                        if retained_redraw {
                            request_launcher_redraw!();
                        }
                    }
                    Err(error) => {
                        crate::ui_errln!("full-screen live-frame confirmation rejected: {error:?}")
                    }
                }
            }
            if accepted_and_active_confirmed {
                launcher_automation.acknowledge_presented(
                    presented_frame.automation,
                    presented_frame.main_present_sequence,
                );
                launcher_response_trace.confirm(
                    launcher_response_frame_stamp.as_ref(),
                    launcher_response_present_receipt,
                    frames,
                    presented_frame.main_present_sequence,
                );
                if launcher_response_trace.launcher_profile_start_ready() {
                    launcher_response_trace.start_pmu_if_ready();
                    screensaver_cpu_profile.begin_launcher_response(frames.saturating_add(1));
                }
                if let Ok(telemetry) = f.read_magik_presentation_telemetry() {
                    launcher_response_trace.observe_presentation(
                        telemetry,
                        pace.period_us,
                        presented_frame.main_present_drop_count.into(),
                    );
                    gui_profiling.record_presentation(
                        frames,
                        telemetry,
                        presented_frame.main_present_drop_count.into(),
                        presented_frame.main_present_sequence,
                    );
                }
                let terminal_preview = preview_terminal_for_route(
                    preview_route,
                    preview.trace_cache_state(),
                    preview.presentation_label(),
                    preview.raw_frame_status() == PreviewRawFrameStatus::Ready,
                    preview.terminal_empty(),
                    crt_backdrop
                        .as_ref()
                        .is_some_and(|backdrop| backdrop.selection_matches(nav.arcade.selected)),
                    crt_backdrop
                        .as_ref()
                        .is_some_and(CrtBackdropController::is_transitioning),
                );
                if gui_profiling.pmu_requested()
                    && gui_profiling.settled_arcade_phase_pending()
                    && let Some(worker) = preview_compositor.as_ref()
                    && !worker.flush_pmu_profile(Duration::from_millis(100))
                {
                    crate::ui_errln!("preview_compositor_pmu_flush_timeout");
                }
                gui_profiling.observe_route_presentation(
                    screen_label(nav.screen),
                    nav.arcade.is_scroll_active(),
                    terminal_preview,
                    Instant::now(),
                    crate::input_hub::monotonic_us(),
                );
                if gui_profiling.settled_arcade_phase_pending() {
                    screensaver_cpu_profile
                        .complete_arcade_velocity_scroll(frames.saturating_add(1));
                }
                if gui_profiling.needs_presentation() {
                    request_launcher_redraw!();
                }
                if let Some(post) = readiness_post {
                    if launcher_readiness.needs_source_evidence()
                        && lifecycle.startup_can_present_frame()
                        && let Some(source) = readiness_source_evidence
                    {
                        launcher_readiness.observe_posted(post, source, true);
                        record_launcher_frame_phase!(
                            LauncherFramePhase::ReadinessSourceAcknowledged
                        );
                    }
                    if launcher_readiness.needs_full_present() {
                        request_launcher_redraw!();
                    }
                }
            }
            if accepted_and_active_confirmed
                && startup_intro_frame_posted
                && let Some(intro) = startup_intro.as_mut()
            {
                let confirmed_at = pace.hit_at.unwrap_or(wait_done);
                if intro.presentation_start_capture_needed() {
                    let telemetry = f.read_magik_presentation_telemetry();
                    intro.capture_presentation_start(confirmed_at, telemetry);
                }
                let software_cadence = intro.note_confirmed_present(
                    confirmed_at,
                    pace.period_us,
                    pace.source == VsyncPaceSource::Vsync,
                );
                if let Some(software_cadence) = software_cadence {
                    let authoritative_cadence = intro.authoritative_cadence_status(
                        confirmed_at,
                        f.read_magik_presentation_telemetry(),
                        software_cadence,
                    );
                    let dropped_frames = authoritative_cadence
                        .dropped_frames
                        .map_or_else(|| "unavailable".to_string(), |count| count.to_string());
                    let cadence_qualified = authoritative_cadence.qualified;
                    let cadence_error = authoritative_cadence.error.as_deref().unwrap_or("none");
                    frame_accounting.record_startup_intro_cadence(authoritative_cadence.clone());
                    let restored = intro.restore_handoff_snapshot(&mut layer_target);
                    if !restored {
                        crate::ui_errln!("startup intro handoff cache geometry mismatch");
                    }
                    launcher_presenter.invalidate_external_hidden_mode();
                    startup_intro = None;
                    window.request_redraw();
                    print_startup_event(
                        start,
                        "startup_intro_completed",
                        format!(
                            concat!(
                                "frames={} logical_elapsed_ms=20000 cabinet_wait_frames={} ",
                                "expected_refresh_intervals={} ",
                                "dropped_frames={} ",
                                "software_estimated_dropped_frames={} pacing_failures={} ",
                                "max_confirmation_gap_us={} cadence_qualified={} cadence_error={}"
                            ),
                            software_cadence.confirmed_frames,
                            software_cadence.cabinet_wait_frames,
                            software_cadence.expected_refresh_intervals,
                            dropped_frames,
                            software_cadence.software_estimated_dropped_frames,
                            software_cadence.pacing_failures,
                            software_cadence.max_confirmation_gap_us,
                            cadence_qualified,
                            cadence_error,
                        ),
                    );
                }
            }
            frame_accounting.record_finished_frame(
                &presented_frame,
                start,
                disp,
                catalog_ready,
                finish_timing.runtime_status_write_us,
            );
            gui_profiling.finalize_frame_timing(
                frames,
                GuiFrameTimingTrace::from_presented_frame(
                    &presented_frame,
                    finish_timing.frame_finish_us,
                ),
            );
            frame_accounting.write_finished_frame_trace(
                &presented_frame,
                finish_timing,
                latch_trace_flush_deferred,
            );
        } else {
            gui_profiling.finalize_frame_timing(
                frames,
                GuiFrameTimingTrace::from_presented_frame(&presented_frame, 0),
            );
            frame_accounting.finish_frame(
                presented_frame,
                start,
                disp,
                &nav,
                &pad,
                &catalog,
                catalog_ready,
                catalog_session.refresh_done(),
                launching,
                scheduler.visible_loading_title(&loading_title),
                catalog_scan_visible,
                status_text
                    .as_ref()
                    .map(|text| text.catalog_scan_title.as_str())
                    .unwrap_or(""),
                status_text
                    .as_ref()
                    .map(|text| text.catalog_scan_detail.as_str())
                    .unwrap_or(""),
                catalog_scan_percent,
                catalog_background_scan_visible,
                status_text
                    .as_ref()
                    .map(|text| text.catalog_scan_message.as_str())
                    .unwrap_or(""),
                confirm_visible,
                status_text
                    .as_ref()
                    .map(|text| text.confirm_title.as_str())
                    .unwrap_or(""),
                status_text
                    .as_ref()
                    .map(|text| text.confirm_message.as_str())
                    .unwrap_or(""),
                confirm_selected,
                status_text
                    .as_ref()
                    .map(|text| text.confirm_left_label.as_str())
                    .unwrap_or(""),
                status_text
                    .as_ref()
                    .map(|text| text.confirm_right_label.as_str())
                    .unwrap_or(""),
                launcher_bench_scenario,
                start_screen,
                lock_screen,
                display_session.reassert_count(),
                display_session.last_reassert_frame(),
                display_session.last_reassert_ok(),
                display_session.last_reassert_error(),
                lifecycle.startup_status(),
                &launch_return_session,
                latch_trace_flush_deferred,
            );
        }
        let post_confirmation_pmu = launcher_response_trace.input_pmu_span(
            launcher_response_frame_stamp.is_some(),
            "launcher-response.post-confirmation",
        );
        if let Some(confirmed_at) = selection_feedback_confirmed_at {
            for confirmation in
                bridge_models.confirm_selection_feedback(&selection_feedback_stamp, confirmed_at)
            {
                launcher_response_trace.record_feedback_confirmation(
                    &confirmation,
                    frames,
                    confirmed_present_sequence,
                );
                match confirmation {
                    crate::launcher_presentation::SelectionFeedbackConfirmation::Visible {
                        event_id,
                        target,
                        ..
                    } => crate::ui_logln!(
                        "selection_feedback phase=visible event={} surface={} item={} frame={} sequence={}",
                        event_id,
                        target.surface,
                        target.item,
                        frames,
                        confirmed_present_sequence,
                    ),
                    crate::launcher_presentation::SelectionFeedbackConfirmation::Hidden {
                        event_id,
                        target,
                        visible_for,
                        ..
                    } => crate::ui_logln!(
                        "selection_feedback phase=hidden event={} surface={} item={} dwell_us={} frame={} sequence={}",
                        event_id,
                        target.surface,
                        target.item,
                        visible_for.as_micros(),
                        frames,
                        confirmed_present_sequence,
                    ),
                    crate::launcher_presentation::SelectionFeedbackConfirmation::Cancelled {
                        event_id,
                        target,
                        ..
                    } => crate::ui_logln!(
                        "selection_feedback phase=cancelled event={} surface={} item={} frame={} sequence={}",
                        event_id,
                        target.surface,
                        target.item,
                        frames,
                        confirmed_present_sequence,
                    ),
                }
            }
        }
        drop(post_confirmation_pmu);
        scheduler_phase =
            launcher_response_trace.record_scheduler_interval("post-confirmation", scheduler_phase);
        launcher_response_trace.flush();
        if launcher_response_trace.take_frame_trace_finalize_pending() {
            frame_accounting.finish_preview_scroll_trace();
            if let Err(error) = launcher_response_trace.finish_pmu() {
                crate::ui_errln!("launcher response PMU finalization failed: {error}");
            }
            screensaver_cpu_profile.complete_launcher_response(frames.saturating_add(1));
        }
        let frame_tail_pmu = launcher_response_trace.input_pmu_span(
            launcher_response_frame_stamp.is_some(),
            "launcher-response.frame-tail",
        );
        latch_v5_qualification.record_present(
            accepted_and_active_confirmed,
            scheduler.catalog_worker_running(),
        );
        if accepted_and_active_confirmed
            && orientation_benchmark.enabled()
            && full_screen_transition.state() == FullScreenTransitionState::Live
            && let Some(record) = orientation_benchmark.note_confirmed_presentation(
                nav.settings.screen_orientation,
                frames,
                confirmed_present_sequence,
                Instant::now(),
                f.read_magik_presentation_telemetry(),
            )
        {
            print_startup_event(
                start,
                "orientation_transition_benchmark_leg_completed",
                format!(
                    concat!(
                        "leg={} effect={} label={} from={} to={} start_frame={} ",
                        "rendered_endpoint_frame={} presented_endpoint_frame={} sequence={}"
                    ),
                    record.leg.index + 1,
                    record.leg.effect.id(),
                    record.leg.label(),
                    record.leg.from.id(),
                    record.leg.to.id(),
                    record.start_frame,
                    record.rendered_endpoint_frame,
                    record.presented_endpoint_frame,
                    record.presented_sequence,
                ),
            );
        }
        record_launcher_frame_phase!(LauncherFramePhase::FrameAccounted);
        let preview_present_confirmed = if latch_trace_flush_deferred {
            accepted_and_active_confirmed
        } else {
            visible_frame_presented
        };
        if preview_present_confirmed && let Some(commit) = preview_presentation_commit {
            preview.confirm_presentation(commit);
        }
        let direct_layer_receipt = if accepted_and_active_confirmed {
            confirmed_direct_layer_receipt
        } else if !latch_trace_flush_deferred && visible_frame_presented {
            Some(DirectLayerPresentationReceipt {
                sequence: (frames as u16).wrapping_add(1).max(1),
                slot: 0,
                route_epoch: 0,
                carrier: composition_decision.retirement_carrier,
            })
        } else {
            None
        };
        if let Some(receipt) = direct_layer_receipt {
            let retired = composition.confirm_presented_layers(
                composition_decision.retirement_generation,
                composition_decision.direct_layers_desired,
                receipt,
            );
            if retired && let Some(generation) = preview.retirement_generation() {
                preview.confirm_retirement(generation);
            }
        }
        record_launcher_frame_phase!(LauncherFramePhase::PresentationAcknowledged);
        if preview.frame_intent().is_actionable() {
            request_launcher_redraw!();
        }
        latch_v5_qualification.write_state_if_due(Instant::now());
        if if latch_backend_active {
            accepted_and_active_confirmed
        } else {
            visible_frame_presented
        } {
            latency_critical_input_pending = false;
        }
        frames += 1;
        if settings_navigation_benchmark.complete()
            && settings_navigation_benchmark_completed_at.is_none()
        {
            if let Some(directory) = settings_navigation_benchmark_evidence_dir()
                && let Err(error) = write_settings_navigation_benchmark_completion(
                    &directory,
                    &settings_navigation_benchmark,
                    frames,
                )
            {
                crate::ui_errln!(
                    "settings_navigation_benchmark_completion_write_failed error={error}"
                );
                settings_navigation_benchmark.fail("completion-write-failed");
            }
            if settings_navigation_benchmark.complete() {
                let (status_baseline, request_status_write) = settings_navigation_status_drain_plan(
                    runtime_status_sequence_before_frame,
                    frame_accounting.runtime_status_submitted_sequence(),
                );
                settings_navigation_status_baseline = Some(status_baseline);
                print_startup_event(
                    start,
                    "settings_navigation_benchmark_complete",
                    format!(
                        "orientations=normal,monitor-counterclockwise legs={} frames={frames}",
                        settings_navigation_benchmark.records().len(),
                    ),
                );
                settings_navigation_benchmark_completed_at = Some(Instant::now());
                screensaver_cpu_profile.complete_settings_navigation_transitions(frames);
                if request_status_write {
                    frame_accounting.request_status_write();
                    request_launcher_redraw!();
                }
            }
        }
        if settings_navigation_benchmark_completed_at.is_some_and(|completed| {
            settings_navigation_status_drain_complete(
                completed.elapsed(),
                settings_navigation_status_baseline.is_some_and(|sequence| {
                    frame_accounting.runtime_status_written_after(sequence)
                }),
            )
        }) {
            break;
        }
        if settings_navigation_benchmark.failed() {
            if let Some(directory) = settings_navigation_benchmark_evidence_dir()
                && let Err(error) = write_settings_navigation_benchmark_completion(
                    &directory,
                    &settings_navigation_benchmark,
                    frames,
                )
            {
                crate::ui_errln!(
                    "settings_navigation_benchmark_failure_write_failed error={error}"
                );
            }
            print_startup_event(
                start,
                "settings_navigation_benchmark_failed",
                format!(
                    "failure={} orientation={} legs={} frames={frames}",
                    settings_navigation_benchmark.failure().unwrap_or("unknown"),
                    settings_navigation_benchmark.orientation().id(),
                    settings_navigation_benchmark.records().len(),
                ),
            );
            break;
        }
        if orientation_benchmark.complete() && orientation_benchmark_completed_at.is_none() {
            if let Some(directory) = orientation_transition_benchmark_evidence_dir()
                && let Err(error) = write_orientation_transition_benchmark_completion(
                    &directory,
                    &orientation_benchmark,
                    frames,
                )
            {
                crate::ui_errln!(
                    "orientation_transition_benchmark_completion_write_failed error={error}"
                );
                orientation_benchmark.fail("completion-write-failed");
            }
            if orientation_benchmark.complete() {
                print_startup_event(
                    start,
                    "orientation_transition_benchmark_complete",
                    format!(
                        "legs={} frames={frames}",
                        orientation_benchmark.records().len()
                    ),
                );
                orientation_benchmark_completed_at = Some(Instant::now());
                screensaver_cpu_profile.complete_orientation_transitions(frames);
                if let Err(error) = write_orientation_transition_pmu_completion(
                    benchmark_config.orientation_pmu_completion(),
                    orientation_benchmark.effect(),
                ) {
                    crate::ui_errln!(
                        "orientation_transition_benchmark_pmu_write_failed error={error}"
                    );
                }
            }
        }
        if let Some(completed) = orientation_benchmark_completed_at {
            let elapsed = completed.elapsed();
            if elapsed >= Duration::from_millis(300)
                && !orientation_benchmark_terminal_status_requested
            {
                orientation_benchmark_terminal_status_requested = true;
                frame_accounting.request_status_write();
                request_launcher_redraw!();
            }
            if elapsed >= Duration::from_millis(800) {
                break;
            }
        }
        if orientation_benchmark.failed() {
            if let Some(directory) = orientation_transition_benchmark_evidence_dir()
                && let Err(error) = write_orientation_transition_benchmark_completion(
                    &directory,
                    &orientation_benchmark,
                    frames,
                )
            {
                crate::ui_errln!(
                    "orientation_transition_benchmark_failure_write_failed error={error}"
                );
            }
            print_startup_event(
                start,
                "orientation_transition_benchmark_failed",
                format!(
                    "failure={} legs={} frames={frames}",
                    orientation_benchmark.failure().unwrap_or("unknown"),
                    orientation_benchmark.records().len(),
                ),
            );
            break;
        }
        launcher_response_trace
            .record_lab(input_latency_lab.cooperative_quantum(input_observation));
        drop(frame_tail_pmu);
        let _ = launcher_response_trace.record_scheduler_interval("frame-tail", scheduler_phase);
        record_launcher_frame_phase!(LauncherFramePhase::FrameFinished);
    }
    if startup_intro.take().is_some() {
        launcher_presenter.invalidate_external_hidden_mode();
    }
    // Preserve the continuous background permission for a later launcher run
    // in the same process (notably host tests and diagnostic runners).
    catalog_work_telemetry.account(Instant::now());
    let gate = mister_magik_catalog::builder_service::catalog_work_gate_snapshot();
    crate::ui_logln!(
        "catalog_work_mode_summary_tsv\ttransitions={}\tcpu0_us={}\tpaused_us={}\tburst_us={}\tepoch={}\tpark_count={}\tparked_threads={}\tcheckpoints={}",
        catalog_work_telemetry.transitions,
        catalog_work_telemetry.cpu0_us,
        catalog_work_telemetry.paused_us,
        catalog_work_telemetry.burst_us,
        gate.epoch,
        gate.park_count,
        gate.parked_threads,
        gate.checkpoints,
    );
    mister_magik_catalog::builder_service::set_catalog_work_mode(CatalogWorkMode::Cpu0);
    frame_accounting.finish_preview_scroll_trace();
    let elapsed = run_start.elapsed().as_secs_f64();
    crate::ui_logln!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu.take()) {
        crate::ui_errln!("{e}");
    }
}

fn display_confirmation_ui_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    value != Some(std::ffi::OsStr::new("0"))
}

fn apply_startup_pending_display(
    nav: &mut LauncherNav,
    state: &launcher::DisplayCommandState,
    confirmation_ui_enabled: bool,
    now: Instant,
) -> Option<Instant> {
    if state.pending.is_none() || !confirmation_ui_enabled {
        return None;
    }
    nav.screen = Screen::Settings;
    nav.settings_selected = 0;
    nav.confirm_action = Some(launcher::ConfirmAction::DisplayResolution);
    nav.confirm_selected = 0;
    nav.display_confirm_remaining = state.remaining.max(1);
    Some(now + Duration::from_secs(u64::from(state.remaining.max(1))))
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

fn preview_frame_from_raw<'a>(frame: &'a PreviewRawFrame<'a>) -> PreviewFrame<'a> {
    PreviewFrame {
        pixels: match frame.pixels {
            PreviewRawPixels::Empty => PreviewPixels::Empty,
            PreviewRawPixels::Rgb8(pixels) => PreviewPixels::Rgb8(pixels),
            PreviewRawPixels::Rgb565 {
                pixels,
                stride_pixels,
            } => PreviewPixels::Rgb565 {
                pixels,
                stride_pixels,
            },
        },
        source_width: frame.source_w as usize,
        source_height: frame.source_h as usize,
        display_width: frame.display_w as usize,
        display_height: frame.display_h as usize,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewRoutePolicy {
    kind: PreviewRouteKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewRouteKind {
    Hdmi,
    LowResolutionCrtBackdrop,
    UnsupportedCrt,
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
                ResolvedOutputRoute::Crt240p60 | ResolvedOutputRoute::Crt288p50 => {
                    PreviewRouteKind::LowResolutionCrtBackdrop
                }
                ResolvedOutputRoute::Crt480p60 | ResolvedOutputRoute::Crt576p50 => {
                    PreviewRouteKind::UnsupportedCrt
                }
            },
        }
    }

    const fn allows_preview_work(self) -> bool {
        !matches!(self.kind, PreviewRouteKind::UnsupportedCrt)
    }

    const fn allows_hdmi_preview(self) -> bool {
        matches!(self.kind, PreviewRouteKind::Hdmi)
    }

    const fn allows_crt_backdrop(self) -> bool {
        matches!(self.kind, PreviewRouteKind::LowResolutionCrtBackdrop)
    }
}

fn preview_terminal_for_route(
    policy: PreviewRoutePolicy,
    cache_state: &str,
    presentation_label: &str,
    raw_frame_ready: bool,
    terminal_empty: bool,
    crt_selection_matches: bool,
    crt_transitioning: bool,
) -> bool {
    if policy.allows_crt_backdrop() {
        return ((cache_state == "exact" && raw_frame_ready) || terminal_empty)
            && crt_selection_matches
            && !crt_transitioning;
    }
    matches!(cache_state, "exact" | "cached" | "empty")
        && matches!(presentation_label, "visible" | "detached")
}

/// Runs the catalog-to-media boundary only for routes that own screenshot work.
fn dispatch_catalog_media_effect(
    policy: PreviewRoutePolicy,
    effect: &CatalogSessionEffect,
    media_session: &mut ScreenshotMediaUpdateSession,
) -> Option<ScreenshotMediaUpdateEffects> {
    let is_media_effect = matches!(
        effect,
        CatalogSessionEffect::FinishMediaWorker
            | CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending
            | CatalogSessionEffect::RequestMediaCatalogSeed
            | CatalogSessionEffect::MediaSystemDiscovered { .. }
    );
    if !is_media_effect {
        return None;
    }
    if !policy.allows_preview_work() {
        return Some(ScreenshotMediaUpdateEffects::default());
    }
    Some(match effect {
        CatalogSessionEffect::FinishMediaWorker => media_session.finish_worker(),
        CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending => {
            media_session.finish_worker_if_no_catalog_seed_pending()
        }
        CatalogSessionEffect::RequestMediaCatalogSeed => {
            media_session.request_catalog_seed();
            ScreenshotMediaUpdateEffects::default()
        }
        CatalogSessionEffect::MediaSystemDiscovered {
            system_id,
            media_gate,
        } => media_session.handle_catalog_system_discovered(system_id.clone(), *media_gate),
        _ => unreachable!("non-media catalog effect returned above"),
    })
}

#[cfg(not(any(feature = "bench-tools", feature = "diagnostics")))]
fn preview_scroll_exit_after_trace_deadline(_run_start: Instant) -> Option<Instant> {
    None
}

#[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
fn preview_scroll_exit_after_trace_deadline(run_start: Instant) -> Option<Instant> {
    if !matches!(
        std::env::var("MISTER_PREVIEW_SCROLL_EXIT_AFTER_TRACE").as_deref(),
        Ok("1") | Ok("on") | Ok("true") | Ok("yes")
    ) {
        return None;
    }
    let secs = std::env::var("MISTER_PREVIEW_SCROLL_TRACE_SECS")
        .ok()?
        .parse::<u64>()
        .ok()?;
    (secs > 0).then(|| run_start + Duration::from_secs(secs))
}

#[cfg(feature = "bench-tools")]
fn media_benchmark_contention_enabled() -> bool {
    matches!(
        std::env::var("MISTER_MEDIA_BENCH_CONTENTION").as_deref(),
        Ok("1") | Ok("on") | Ok("true") | Ok("yes")
    )
}

#[cfg(not(feature = "bench-tools"))]
fn media_benchmark_contention_enabled() -> bool {
    false
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

fn benchmark_media_interaction_gate_active(
    benchmark_active: bool,
    media_benchmark_contention: bool,
) -> bool {
    benchmark_active && !media_benchmark_contention
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

#[allow(clippy::too_many_arguments)]
fn process_catalog_worker_message(
    message: CatalogWorkerMessage,
    preview_route: PreviewRoutePolicy,
    prepare_trace: &mut LauncherPrepareTrace,
    launcher_response_trace: &mut LauncherResponseTrace,
    first_visible_copy_done: bool,
    launching: bool,
    benchmark_media_interaction_active: bool,
    media_benchmark_contention: bool,
    loop_start: Instant,
    app: &slint_ui::launcher::Launcher,
    nav: &mut LauncherNav,
    catalog: &mut ArcadeCatalog,
    catalog_ready: &mut bool,
    catalog_version: &mut usize,
    return_capsule_active: &mut bool,
    catalog_generation: &mut CatalogGenerationState,
    launch_return_session: &mut LaunchReturnSession,
    preview: &mut PreviewState,
    media_session: &mut ScreenshotMediaUpdateSession,
    scheduler: &mut LauncherScheduler,
    catalog_session: &mut LauncherCatalogSession,
    lifecycle: &mut LauncherLifecycle,
    lifecycle_effects: &mut LifecycleEffects,
    full_bridge_dirty: &mut bool,
    startup_intro_catalog_ui_replay: &mut Option<LauncherWorkerUiIntent>,
    startup_intro_catalog_shells_pending: &mut bool,
    defer_bridge_ui: bool,
    start: Instant,
) {
    prepare_trace.catalog_message_count = prepare_trace.catalog_message_count.saturating_add(1);
    let media_gate = if matches!(&message, CatalogWorkerMessage::SystemDiscovered { .. }) {
        let media_gate = media_session.current_gate(
            first_visible_copy_done,
            scheduler.has_pending_launch() || launching,
            benchmark_media_interaction_active,
            media_benchmark_contention,
            loop_start,
        );
        let media_gate = if !preview_route.allows_preview_work() {
            MediaInteractionGate {
                active: true,
                reason: "crt-no-screenshots",
            }
        } else {
            catalog_build_media_gate(catalog_session.refresh_done(), media_gate)
        };
        apply_screenshot_media_update_effects(
            media_session.sync_gate(media_gate),
            app,
            catalog,
            scheduler,
            Some(&mut *preview),
            full_bridge_dirty,
            start,
        );
        Some(media_gate)
    } else {
        None
    };
    let effects = catalog_session.handle_worker_message(
        CatalogWorkerMessageContext {
            catalog_ready: *catalog_ready,
            catalog_partial: *return_capsule_active,
            screen: nav.screen,
            media_gate,
        },
        message,
        loop_start,
    );
    apply_catalog_session_effects(
        effects,
        preview_route,
        launcher_response_trace,
        app,
        nav,
        catalog,
        catalog_ready,
        catalog_version,
        return_capsule_active,
        catalog_generation,
        launch_return_session,
        preview,
        media_session,
        scheduler,
        lifecycle,
        lifecycle_effects,
        full_bridge_dirty,
        startup_intro_catalog_ui_replay,
        startup_intro_catalog_shells_pending,
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

fn launcher_auto_launch_gate_ready(path: Option<&Path>) -> bool {
    launcher_auto_launch_gate_ready_from_value(path.and_then(Path::to_str))
}

fn launcher_auto_launch_gate_ready_from_value(path: Option<&str>) -> bool {
    path.is_none_or(|path| path.trim().is_empty() || std::path::Path::new(path.trim()).is_file())
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

fn sync_startup_visibility(app: &slint_ui::launcher::Launcher, lifecycle: &LauncherLifecycle) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let visible = lifecycle.startup_should_show_splash();
    if bridge.get_startup_visible() != visible {
        bridge.set_startup_visible(visible);
    }
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
            LauncherEffect::BeginLaunchHandoff {
                launch_ref,
                presented_at,
            } => {
                scheduler.complete_loading_frame(presented_at);
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

fn maybe_present_modal_input_test_dialog(
    pending: &mut bool,
    catalog_ready: bool,
    lifecycle: &mut LauncherLifecycle,
    lifecycle_effects: &mut LifecycleEffects,
    scheduler: &mut LauncherScheduler,
    start: Instant,
) -> bool {
    if !*pending || !catalog_ready {
        return false;
    }
    *pending = false;
    lifecycle.handle(
        LauncherLifecycleInput::CatalogRecoveryRequired {
            error: "isolated modal input verification".to_string(),
            has_stale_catalog: true,
            mode: CatalogRecoveryMode::UpgradeRequired,
        },
        lifecycle_effects,
    );
    apply_lifecycle_effects(lifecycle_effects, scheduler, start);
    print_startup_event(
        start,
        "modal_input_test_dialog",
        "mode=upgrade-required isolated=1",
    );
    true
}

#[allow(clippy::too_many_arguments)]
fn apply_catalog_session_effects(
    effects: CatalogSessionEffects,
    preview_route: PreviewRoutePolicy,
    launcher_response_trace: &mut LauncherResponseTrace,
    app: &slint_ui::launcher::Launcher,
    nav: &mut LauncherNav,
    catalog: &mut ArcadeCatalog,
    catalog_ready: &mut bool,
    catalog_version: &mut usize,
    return_capsule_active: &mut bool,
    catalog_generation: &mut CatalogGenerationState,
    launch_return_session: &mut LaunchReturnSession,
    preview: &mut PreviewState,
    media_session: &mut ScreenshotMediaUpdateSession,
    scheduler: &mut LauncherScheduler,
    lifecycle: &mut LauncherLifecycle,
    lifecycle_effects: &mut LifecycleEffects,
    full_bridge_dirty: &mut bool,
    startup_intro_catalog_ui_replay: &mut Option<LauncherWorkerUiIntent>,
    startup_intro_catalog_shells_pending: &mut bool,
    defer_bridge_ui: bool,
    now: Instant,
    start: Instant,
) {
    for effect in effects.into_effects() {
        if let Some(media_effects) =
            dispatch_catalog_media_effect(preview_route, &effect, media_session)
        {
            apply_screenshot_media_update_effects(
                media_effects,
                app,
                catalog,
                scheduler,
                Some(&mut *preview),
                full_bridge_dirty,
                start,
            );
            continue;
        }
        match effect {
            CatalogSessionEffect::StartupEvent(event) => {
                print_startup_event(start, &event.name, event.detail);
            }
            CatalogSessionEffect::UseCatalog {
                catalog: ready_catalog,
                load_us: _,
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
            CatalogSessionEffect::MarkCatalogDurable {
                generation_fingerprint,
            } => {
                catalog_generation.mark_durable(generation_fingerprint);
            }
            CatalogSessionEffect::ConfirmCatalogSeed => {
                *return_capsule_active = false;
                nav.set_arcade_exit_locked(false);
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
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
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
            CatalogSessionEffect::CatalogBuildStarted => {
                nav.catalog_build_started();
                if defer_bridge_ui {
                    *startup_intro_catalog_shells_pending = true;
                    continue;
                }
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                let _ = reapply_pending_launch_return_state(nav, catalog, launch_return_session);
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
            CatalogSessionEffect::CatalogSystemDiscovered { .. } => {}
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
            CatalogSessionEffect::FinishMediaWorker
            | CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending
            | CatalogSessionEffect::RequestMediaCatalogSeed
            | CatalogSessionEffect::MediaSystemDiscovered { .. } => {
                unreachable!("media effects dispatched before general catalog effects")
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
                profile,
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
                let phase = launcher_response_trace.begin_catalog_phase("hydration-state");
                nav.catalog_system_hydration_finished(&system_id);
                launcher_response_trace.end_catalog_phase(phase);
                let phase = launcher_response_trace.begin_catalog_phase("catalog-replacement");
                let retired_catalog = std::mem::replace(catalog, prepared_catalog);
                *catalog_version = (*catalog_version).wrapping_add(1);
                launcher_response_trace.end_catalog_phase(phase);
                if let Some(prelude) = preview_prelude
                    && let Some(game) = catalog.system_game_at(&system_id, 0)
                {
                    preview.adopt_system_entry_preview(game, prelude);
                }
                let phase = launcher_response_trace.begin_catalog_phase("catalog-retirement");
                scheduler.retire_catalog(retired_catalog);
                launcher_response_trace.end_catalog_phase(phase);
                let taxonomy_start = launcher_response_trace.catalog_boundary();
                let mut taxonomy_end = None;
                let taxonomy_timing = nav.sync_launcher_taxonomy_with_timing(catalog, &mut || {
                    taxonomy_end = Some(launcher_response_trace.catalog_boundary());
                });
                let taxonomy_end = taxonomy_end.unwrap_or(taxonomy_start);
                let navigation_end = launcher_response_trace.catalog_boundary();
                launcher_response_trace.record_catalog_interval(
                    "taxonomy-construction",
                    taxonomy_start,
                    taxonomy_end,
                    taxonomy_timing.taxonomy_build_us,
                );
                launcher_response_trace.record_catalog_interval(
                    "navigation-reconciliation",
                    taxonomy_end,
                    navigation_end,
                    taxonomy_timing.navigation_reconcile_us,
                );
                let phase = launcher_response_trace.begin_catalog_phase("return-state-restore");
                let return_restored =
                    reapply_pending_launch_return_state(nav, catalog, launch_return_session);
                launcher_response_trace.end_catalog_phase(phase);
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
                let phase = launcher_response_trace.begin_catalog_phase("bridge-invalidation");
                *full_bridge_dirty = true;
                launcher_response_trace.end_catalog_phase(phase);
                let adoption_us = adoption_started.elapsed().as_micros();
                if let Some(path) = launcher_response_trace.system_entry_profile_path.as_deref() {
                    let path = std::path::Path::new(path);
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let evidence = serde_json::json!({
                        "schema": "mister-magik-system-entry-profile-v1",
                        "system": system_id,
                        "catalog": profile,
                        "adoption_us": adoption_us,
                        "pmu": mister_magik_perf_events::take_process_profiles(),
                    });
                    if let Err(error) = std::fs::write(
                        path,
                        format!(
                            "{}\n",
                            serde_json::to_string_pretty(&evidence).unwrap_or_default()
                        ),
                    ) {
                        crate::ui_errln!("system-entry profile write failed: {error}");
                    }
                }
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
            ScreenshotMediaUpdateEffect::EnsureCatalogSystems => {
                ensure_media_for_catalog_systems(catalog, scheduler, start);
            }
            ScreenshotMediaUpdateEffect::FinishWorker => {
                scheduler.finish_media_worker();
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
                let (replacement, launch_plans) =
                    arcade_rows_from_persisted_shard(&system_id, &games);
                *catalog = catalog.replacing_system_games(&system_id, replacement, launch_plans);
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

fn ensure_media_for_catalog_systems(
    catalog: &ArcadeCatalog,
    scheduler: &mut LauncherScheduler,
    start: Instant,
) {
    let systems = catalog_media_system_ids(catalog);
    if systems.is_empty() {
        return;
    }
    scheduler.ensure_media_worker_started(start, "catalog-systems");
    for system_id in systems {
        print_startup_event(
            start,
            "screenshot_media_catalog_system_present",
            format!("system={system_id} source=catalog-seed"),
        );
        print_startup_event(
            start,
            "screenshot_media_catalog_ensure",
            format!("system={system_id}"),
        );
        scheduler.ensure_media_system(&system_id);
    }
}

fn catalog_media_system_ids(catalog: &ArcadeCatalog) -> Vec<String> {
    let mut seen = BTreeSet::new();
    catalog
        .systems
        .iter()
        .filter_map(|system| {
            let id = system.id.as_str();
            (mister_magik_fb::media_update::is_supported_pack_id(id)
                && (system.count > 0 || catalog.system_game_count(id) > 0)
                && seen.insert(system.id.clone()))
            .then(|| system.id.clone())
        })
        .collect()
}

fn catalog_background_validation_delay() -> Duration {
    std::env::var("MISTER_CATALOG_BACKGROUND_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogStartupSqliteState {
    Missing,
    HeaderValid,
    ExistingUnusable,
}

impl CatalogStartupSqliteState {
    fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::HeaderValid => "sqlite-or-navigation",
            Self::ExistingUnusable => "existing-unusable",
        }
    }
}

fn catalog_startup_sqlite_state(path: &Path) -> CatalogStartupSqliteState {
    if !path.exists() {
        CatalogStartupSqliteState::Missing
    } else if sqlite_file_has_valid_header(path) {
        CatalogStartupSqliteState::HeaderValid
    } else {
        CatalogStartupSqliteState::ExistingUnusable
    }
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

fn catalog_startup_without_summary_plan(
    sqlite_state: CatalogStartupSqliteState,
    catalog_worker_enabled: bool,
    _refresh_policy: CatalogRefreshPolicy,
    _deferred_library_rebuild: bool,
) -> CatalogStartupWithoutSummaryPlan {
    match sqlite_state {
        CatalogStartupSqliteState::HeaderValid => {
            return CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            };
        }
        CatalogStartupSqliteState::ExistingUnusable => {
            return CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            };
        }
        CatalogStartupSqliteState::Missing => {}
    }
    if catalog_worker_enabled {
        return CatalogStartupWithoutSummaryPlan::DeferredWorker {
            request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
            initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
            execution_mode: CatalogExecutionMode::ForegroundExclusive,
        };
    }
    CatalogStartupWithoutSummaryPlan::NoCatalog
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
            allowed: first_visible_copy_done || startup_return_waiting_for_catalog,
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

fn library_changed_test_dialog_choice_from_value(
    value: Option<&str>,
    start: Instant,
) -> Option<launcher::LibraryChangedTestDialogChoice> {
    let value = value?;
    match launcher::parse_library_changed_test_dialog_choice(value) {
        Ok(choice) => choice,
        Err(e) => {
            crate::ui_errln!("{e}");
            print_startup_event(start, "library_changed_test_dialog_choice_invalid", e);
            None
        }
    }
}

fn initial_catalog_scan_visible(
    catalog_ready: bool,
    _arcade_catalog_required_at_start: bool,
    catalog_worker_enabled: bool,
    foreground_update: bool,
) -> bool {
    catalog_worker_enabled && (foreground_update || !catalog_ready)
}

fn arcade_catalog_rows_ready(catalog: &ArcadeCatalog) -> bool {
    !catalog.games.is_empty() || catalog.systems.iter().all(|system| system.count == 0)
}

fn arcade_navigation_ready(catalog_ready: bool, catalog: &ArcadeCatalog) -> bool {
    catalog_ready && arcade_catalog_rows_ready(catalog)
}

fn should_draw_arcade_overlay(
    nav: &LauncherNav,
    launching: bool,
    active_arcade_games_available: bool,
) -> bool {
    !launching && nav.screen == Screen::Arcade && active_arcade_games_available
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

fn effective_lock_screen(
    lock_screen: Option<Screen>,
    catalog_ready: bool,
    catalog: &ArcadeCatalog,
) -> Option<Screen> {
    match lock_screen {
        Some(Screen::Arcade | Screen::SystemHub)
            if !arcade_navigation_ready(catalog_ready, catalog) =>
        {
            None
        }
        other => other,
    }
}

fn ready_catalog_worker_request(refresh_policy: CatalogRefreshPolicy) -> CatalogWorkerRequest {
    if refresh_policy.force_requested() {
        CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS
    } else {
        CatalogWorkerRequest::LoadOnly
    }
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

fn summary_seed_catalog_worker_initial_cache(
    _request: CatalogWorkerRequest,
    _return_catalog_hydration_needed: bool,
) -> CatalogWorkerInitialCache {
    CatalogWorkerInitialCache::AlreadyLoadedReady
}

fn launcher_bench_initial_preview_ready(
    scenario: LauncherBenchScenario,
    preview_cache_state: &str,
    selected_has_preview: bool,
) -> bool {
    if !scenario.starts_on_arcade() {
        return true;
    }
    if selected_has_preview {
        preview_cache_state == "exact"
    } else {
        matches!(preview_cache_state, "exact" | "empty")
    }
}

fn apply_start_system_from_env(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    system_id: &str,
    forced_arcade_selected: Option<usize>,
) -> bool {
    if !nav.open_system(catalog, system_id) {
        return false;
    }
    nav.arcade_filter.drawer_open = false;
    nav.arcade_filter.level = launcher::ArcadeFilterLevel::Top;
    ui_frame_target::apply_forced_arcade_selected_index(nav, catalog, forced_arcade_selected);
    true
}

fn apply_home_selected(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    selected: Option<&Result<usize, String>>,
    start: Instant,
) {
    let Some(selected) = selected else {
        return;
    };
    let selected = match selected {
        Ok(selected) => *selected,
        Err(value) => {
            print_startup_event(
                start,
                "launcher_home_selected_index_invalid",
                format!("value={value}"),
            );
            return;
        }
    };
    nav.sync_launcher_taxonomy(catalog);
    let item_count = nav.current_menu_count();
    if nav.screen != Screen::Home || selected >= item_count {
        print_startup_event(
            start,
            "launcher_home_selected_index_ignored",
            format!(
                "value={} screen={} menu_items={}",
                selected,
                screen_label(nav.screen),
                item_count
            ),
        );
        return;
    }
    nav.selected = selected;
    keep_bench_home_visible(&mut nav.scroll_x, nav.selected, item_count);
    print_startup_event(
        start,
        "launcher_home_selected_index_applied",
        format!("selected={selected}"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn portrait_navigation_geometry_uses_physical_rectangles() {
        let display = UiDisplay::for_framebuffer(4, 3);
        let layout = UiLayoutGeometry::for_display(&display, ScreenOrientation::MonitorClockwise);
        let logical_rect = NavigationTransitionRect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let geometry = NavigationTransitionGeometry {
            source_card: logical_rect,
            destination_preview: logical_rect,
            ..NavigationTransitionGeometry::default()
        };

        let mapped = navigation_geometry_to_composition(layout, geometry);

        assert_eq!(mapped.source_card, mapped.destination_preview);
        assert_eq!(mapped.source_card.x, 0);
        assert_eq!(mapped.source_card.y, 1);
        assert_eq!(mapped.source_card.width, 1);
        assert_eq!(mapped.source_card.height, 2);
    }

    #[test]
    fn system_entry_ready_marker_requires_main_active_confirmation() {
        assert!(!system_entry_ready_frame_eligible(
            true,
            true,
            true,
            false,
            Screen::Arcade,
            240,
            false,
        ));
        assert!(system_entry_ready_frame_eligible(
            true,
            true,
            true,
            false,
            Screen::Arcade,
            240,
            true,
        ));
    }

    #[test]
    fn system_entry_publication_evidence_keeps_cpu_and_confirmation_wait_distinct() {
        let phases = SystemEntryPublicationPhases {
            bridge_model_assembly_us: 101,
            bridge_updates_us: 102,
            list_projection_us: 103,
            slint_raster_us: 104,
            overlay_composition_us: 105,
            latch_copy_us: 106,
            post_us: 107,
            confirmation_wait_wall_us: 16_667,
            confirmation_poll_cpu_us: 108,
        };

        let evidence = phases.json();

        assert_eq!(evidence["bridge_model_assembly"], 101);
        assert_eq!(evidence["list_projection"], 103);
        assert_eq!(evidence["slint_raster"], 104);
        assert_eq!(evidence["overlay_composition"], 105);
        assert_eq!(evidence["latch_copy"], 106);
        assert_eq!(evidence["post"], 107);
        assert_eq!(evidence["confirmation_wait_wall"], 16_667);
        assert_eq!(evidence["confirmation_poll_cpu"], 108);
    }

    #[test]
    fn system_entry_ready_marker_requires_rows_and_terminal_preview() {
        assert!(!system_entry_ready_frame_eligible(
            true,
            false,
            true,
            false,
            Screen::Arcade,
            240,
            true,
        ));
        assert!(!system_entry_ready_frame_eligible(
            true,
            true,
            false,
            false,
            Screen::Arcade,
            240,
            true,
        ));
    }

    #[test]
    fn system_entry_destination_requires_list_and_terminal_preview_in_one_frame() {
        assert!(system_entry_destination_frame_eligible(
            true,
            true,
            true,
            false,
            Screen::Arcade,
            240,
        ));
        assert!(!system_entry_destination_frame_eligible(
            true,
            true,
            false,
            false,
            Screen::Arcade,
            240,
        ));
        assert!(!system_entry_destination_frame_eligible(
            true,
            true,
            true,
            false,
            Screen::Arcade,
            0,
        ));
    }

    #[test]
    fn direct_system_entry_measurement_starts_after_home_settles() {
        assert!(!system_entry_benchmark_settled(2_245, 246));
        assert!(system_entry_benchmark_settled(2_246, 246));
    }

    #[test]
    fn direct_arcade_entry_uses_the_production_root_collection() {
        assert_eq!(
            system_entry_collection_id("arcade"),
            arcade_catalog::MENU_ARCADE_SYSTEM_ID
        );
        assert_eq!(system_entry_collection_id("c64"), "c64");
    }

    #[test]
    fn system_entry_no_preview_requires_confirmed_terminal_empty_state() {
        assert!(!system_entry_preview_terminal(false, "empty", false));
        assert!(system_entry_preview_terminal(false, "empty", true));
        assert!(!system_entry_preview_terminal(true, "empty", true));
        assert!(system_entry_preview_terminal(true, "exact", false));
    }

    #[test]
    fn discrete_feedback_targets_cover_included_and_excluded_surfaces() {
        let mut nav = LauncherNav::new();

        nav.screen = Screen::SystemHub;
        nav.system_hub_selected = 2;
        assert_eq!(
            nav_selection_feedback_target(&nav),
            Some(SelectionFeedbackTarget::new("system-hub", "favorites"))
        );

        nav.screen = Screen::Settings;
        nav.settings_selected = 3;
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

        nav.screen = Screen::Screensaver;
        nav.screensaver_selected = 2;
        assert_eq!(
            nav_selection_feedback_target(&nav),
            Some(SelectionFeedbackTarget::new(
                "screensaver-settings",
                "preview"
            ))
        );

        nav.screen = Screen::About;
        nav.about_selected = 1;
        assert_eq!(
            nav_selection_feedback_target(&nav),
            Some(SelectionFeedbackTarget::new("about", "licenses"))
        );

        nav.screen = Screen::Licenses;
        nav.licenses_selected = 9;
        assert_eq!(
            nav_selection_feedback_target(&nav),
            Some(SelectionFeedbackTarget::new("licenses", "slint"))
        );
        nav.licenses_expanded = true;
        assert_eq!(nav_selection_feedback_target(&nav), None);

        nav.screen = Screen::Arcade;
        nav.licenses_expanded = false;
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
        nav.screen = Screen::Info;
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
    fn every_owned_full_screen_transition_state_quiesces_cpu1() {
        assert!(!full_screen_transition_owns_cpu1(
            FullScreenTransitionState::Live
        ));
        assert!(full_screen_transition_owns_cpu1(
            FullScreenTransitionState::CapturePending
        ));
        assert!(full_screen_transition_owns_cpu1(
            FullScreenTransitionState::SnapshotLocked
        ));
        assert!(full_screen_transition_owns_cpu1(
            FullScreenTransitionState::Releasing
        ));
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
            false,
        ));
        for blocked in 0..7 {
            let mut conditions = [false; 7];
            conditions[blocked] = true;
            assert!(!can_preempt_home_latch_wait(
                Screen::Home,
                conditions[0],
                conditions[1],
                conditions[2],
                conditions[3],
                conditions[4],
                conditions[5],
                conditions[6],
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
    fn launcher_response_trace_confirms_the_visible_state_change() {
        let mut nav = LauncherNav::new();
        nav.screen = Screen::SystemHub;
        let mut trace = LauncherResponseTrace::enabled_for_test(&nav);
        trace.enable_execution_for_test();
        let mut event = normalized_test_press(LogicalAction::Right);
        event.source.kind = InputSourceKind::MainProxy;
        trace
            .published_at_us
            .insert(event.sequence, event.captured_at_us + 10);
        trace
            .drained_at_us
            .insert(event.sequence, event.captured_at_us + 20);
        trace
            .drained_execution
            .insert(event.sequence, ThreadExecutionStamp::capture());
        let context = ContextId {
            target: FocusTarget {
                kind: InputContextKind::Screen,
                owner: 1,
            },
            generation: 1,
        };
        trace.record_route(
            event,
            InputOutcome::Dispatch {
                event,
                context,
                kind: DispatchKind::Initial,
            },
        );
        nav.system_hub_selected = 1;
        trace.observe_state(&nav, false);
        let applied_at_us = trace.records[0].state_applied_at_us.unwrap();
        assert!(
            trace
                .frame_stamp(
                    &nav,
                    applied_at_us.saturating_sub(1),
                    None,
                    applied_at_us,
                    None,
                    applied_at_us,
                    None,
                )
                .is_none()
        );
        let stamp = trace.frame_stamp(
            &nav,
            applied_at_us,
            Some(ThreadExecutionStamp::capture()),
            applied_at_us + 1,
            Some(ThreadExecutionStamp::capture()),
            applied_at_us + 2,
            Some(ThreadExecutionStamp::capture()),
        );
        trace.confirm(
            stamp.as_ref(),
            LauncherResponsePresentReceipt {
                post_accepted_at_us: applied_at_us + 3,
                post_accepted_execution: Some(ThreadExecutionStamp::capture()),
                ..LauncherResponsePresentReceipt::default()
            },
            42,
            7,
        );

        assert_eq!(trace.records.len(), 1);
        assert_eq!(trace.records[0].disposition, "confirmed");
        assert_eq!(trace.records[0].confirmed_frame, Some(42));
        assert_eq!(trace.records[0].confirmed_sequence, Some(7));
        assert_eq!(
            trace.records[0].published_at_us,
            Some(event.captured_at_us + 10)
        );
        assert_eq!(
            trace.records[0].drained_at_us,
            Some(event.captured_at_us + 20)
        );
        assert_eq!(
            trace.records[0]
                .frame
                .as_ref()
                .map(|frame| frame.selected.selected_index),
            Some(1)
        );
        let payload: serde_json::Value =
            serde_json::from_str(&trace.snapshot().payload()).expect("response trace payload");
        assert_eq!(payload["schema"], "mister-magik-launcher-response-trace-v6");
        assert_eq!(payload["execution_attribution"]["enabled"], true);
        assert!(payload["records"][0]["execution"]["stamps"]["drained"].is_object());
        assert!(payload["records"][0]["frame"]["execution"]["intervals"]["raster"].is_object());
    }

    #[test]
    fn launcher_response_confirmation_uses_the_stamped_frame_state() {
        let mut nav = LauncherNav::new();
        nav.screen = Screen::SystemHub;
        let mut trace = LauncherResponseTrace::enabled_for_test(&nav);
        let mut event = normalized_test_press(LogicalAction::Right);
        event.source.kind = InputSourceKind::MainProxy;
        let context = ContextId {
            target: FocusTarget {
                kind: InputContextKind::Screen,
                owner: 1,
            },
            generation: 1,
        };
        trace.record_route(
            event,
            InputOutcome::Dispatch {
                event,
                context,
                kind: DispatchKind::Initial,
            },
        );
        nav.system_hub_selected = 1;
        trace.observe_state(&nav, false);
        let applied_at_us = trace.records[0].state_applied_at_us.unwrap();
        let mut stamp = trace.frame_stamp(
            &nav,
            applied_at_us,
            None,
            applied_at_us + 1,
            None,
            applied_at_us + 2,
            None,
        );
        stamp
            .as_mut()
            .expect("response frame stamp")
            .slint_damage_rects
            .push((10, 20, 30, 40));
        nav.system_hub_selected = 2;
        trace.confirm(
            stamp.as_ref(),
            LauncherResponsePresentReceipt {
                present_bytes: 1_234,
                hidden_copy_us: 321,
                hidden_rect_count: 2,
                ..LauncherResponsePresentReceipt::default()
            },
            5,
            9,
        );

        assert_eq!(trace.records[0].disposition, "confirmed");
        assert_eq!(
            trace.records[0]
                .frame
                .as_ref()
                .map(|frame| frame.selected.selected_index),
            Some(1)
        );
        let frame = trace.records[0].frame.as_ref().expect("frame evidence");
        assert_eq!(frame.slint_damage_rects, vec![(10, 20, 30, 40)]);
        assert_eq!(frame.present_bytes, 1_234);
        assert_eq!(frame.hidden_copy_us, 321);
        assert_eq!(frame.hidden_rect_count, 2);
    }

    #[test]
    fn launcher_response_trace_records_exact_feedback_on_and_off_frames() {
        let nav = LauncherNav::new();
        let mut trace = LauncherResponseTrace::enabled_for_test(&nav);
        let target = SelectionFeedbackTarget::new("menu:computers", "menu:computers:other");
        let visible_at = Instant::now();
        trace.record_feedback_confirmation(
            &crate::launcher_presentation::SelectionFeedbackConfirmation::Visible {
                event_id: 9,
                target: target.clone(),
                confirmed_at: visible_at,
            },
            40,
            6,
        );
        trace.record_feedback_confirmation(
            &crate::launcher_presentation::SelectionFeedbackConfirmation::Hidden {
                event_id: 9,
                target,
                visible_for: Duration::from_millis(84),
                confirmed_at: visible_at + Duration::from_millis(84),
            },
            45,
            11,
        );
        trace.record_feedback_confirmation(
            &crate::launcher_presentation::SelectionFeedbackConfirmation::Cancelled {
                event_id: 10,
                target: SelectionFeedbackTarget::new("menu:computers", "apple-ii"),
                confirmed_at: visible_at + Duration::from_millis(85),
            },
            46,
            12,
        );

        assert_eq!(trace.feedback_records.len(), 3);
        assert_eq!(trace.feedback_records[0].phase, "visible");
        assert_eq!(trace.feedback_records[0].confirmed_frame, 40);
        assert_eq!(trace.feedback_records[1].phase, "hidden");
        assert_eq!(trace.feedback_records[1].dwell_us, Some(84_000));
        assert_eq!(trace.feedback_records[1].confirmed_sequence, 11);
        assert_eq!(trace.feedback_records[2].phase, "cancelled");
        assert_eq!(trace.cancelled_feedback_count, 1);
    }

    #[test]
    fn launcher_response_partial_snapshot_streams_bounded_lab_records_only() {
        let nav = LauncherNav::new();
        let mut trace = LauncherResponseTrace::enabled_for_test(&nav);
        trace
            .catalog_phases
            .push(serde_json::json!({"phase": "catalog"}));
        trace
            .scheduler_phases
            .push(serde_json::json!({"phase": "scheduler"}));
        trace.lab_records.push(serde_json::json!({"phase": "lab"}));

        let (partial, _, _, lab_count) = trace.partial_snapshot();
        assert!(partial.catalog_phases.is_empty());
        assert!(partial.scheduler_phases.is_empty());
        assert_eq!(lab_count, 1);
        assert_eq!(partial.lab_records.len(), 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&partial.payload())
                .expect("partial response trace payload")["completion"]["state"],
            "running"
        );

        trace.complete = true;
        let complete = trace.snapshot();
        assert_eq!(complete.catalog_phases.len(), 1);
        assert_eq!(complete.scheduler_phases.len(), 1);
        assert_eq!(complete.lab_records.len(), 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&complete.payload())
                .expect("complete response trace payload")["completion"]["state"],
            "complete"
        );
    }

    #[test]
    fn launcher_response_partial_snapshots_send_only_new_feedback() {
        let nav = LauncherNav::new();
        let mut trace = LauncherResponseTrace::enabled_for_test(&nav);
        let target = SelectionFeedbackTarget::new("menu:computers", "menu:computers:other");
        let visible_at = Instant::now();
        trace.record_feedback_confirmation(
            &crate::launcher_presentation::SelectionFeedbackConfirmation::Visible {
                event_id: 9,
                target: target.clone(),
                confirmed_at: visible_at,
            },
            40,
            6,
        );
        trace
            .lab_records
            .push(serde_json::json!({"phase": "first"}));
        let (mut accumulated, confirmed_count, feedback_count, lab_count) =
            trace.partial_snapshot();
        assert_eq!(confirmed_count, 0);
        assert_eq!(feedback_count, 1);
        assert_eq!(lab_count, 1);
        assert_eq!(accumulated.feedback_records.len(), 1);
        assert_eq!(accumulated.lab_records.len(), 1);
        trace.partial_feedback_sent = feedback_count;
        trace.partial_lab_sent = lab_count;

        trace.record_feedback_confirmation(
            &crate::launcher_presentation::SelectionFeedbackConfirmation::Hidden {
                event_id: 9,
                target,
                visible_for: Duration::from_millis(84),
                confirmed_at: visible_at + Duration::from_millis(84),
            },
            45,
            11,
        );
        trace
            .lab_records
            .push(serde_json::json!({"phase": "second"}));
        let (next, _, feedback_count, lab_count) = trace.partial_snapshot();
        assert_eq!(feedback_count, 2);
        assert_eq!(lab_count, 2);
        assert_eq!(next.feedback_records.len(), 1);
        assert_eq!(next.lab_records.len(), 1);

        accumulated.merge_partial(next);
        assert_eq!(accumulated.feedback_records.len(), 2);
        assert_eq!(accumulated.lab_records.len(), 2);
        assert_eq!(accumulated.feedback_records[0].phase, "visible");
        assert_eq!(accumulated.feedback_records[1].phase, "hidden");
    }

    #[test]
    fn launcher_response_trace_completes_after_focus_and_feedback_removal() {
        let mut nav = LauncherNav::new();
        nav.screen = Screen::SystemHub;
        let mut trace = LauncherResponseTrace::configured_for_test(&nav, 1, 1);
        let mut event = normalized_test_press(LogicalAction::Right);
        event.source.kind = InputSourceKind::MainProxy;
        let context = ContextId {
            target: FocusTarget {
                kind: InputContextKind::Screen,
                owner: 1,
            },
            generation: 1,
        };
        trace.record_route(
            event,
            InputOutcome::Dispatch {
                event,
                context,
                kind: DispatchKind::Initial,
            },
        );
        nav.system_hub_selected = 1;
        trace.observe_state(&nav, false);
        let applied_at_us = trace.records[0].state_applied_at_us.unwrap();
        let stamp = trace.frame_stamp(
            &nav,
            applied_at_us,
            None,
            applied_at_us + 1,
            None,
            applied_at_us + 2,
            None,
        );
        trace.confirm(
            stamp.as_ref(),
            LauncherResponsePresentReceipt::default(),
            42,
            7,
        );
        assert!(!trace.complete);

        let target = SelectionFeedbackTarget::new("system-hub", "recent");
        let visible_at = Instant::now();
        trace.record_feedback_confirmation(
            &crate::launcher_presentation::SelectionFeedbackConfirmation::Visible {
                event_id: 9,
                target: target.clone(),
                confirmed_at: visible_at,
            },
            42,
            7,
        );
        assert!(!trace.complete);
        trace.record_feedback_confirmation(
            &crate::launcher_presentation::SelectionFeedbackConfirmation::Hidden {
                event_id: 9,
                target,
                visible_for: Duration::from_millis(84),
                confirmed_at: visible_at + Duration::from_millis(84),
            },
            47,
            12,
        );

        assert!(trace.complete);
        assert!(trace.take_frame_trace_finalize_pending());
        assert!(!trace.take_frame_trace_finalize_pending());
    }

    #[test]
    fn arcade_response_confirmation_waits_for_first_visible_motion() {
        let mut before = LauncherNav::new();
        before.screen = Screen::Arcade;
        let before = LauncherResponseState::capture(&before);
        let mut selected = before.clone();
        selected.selected_index = 1;
        let mut stationary = selected.clone();
        stationary.arcade_visual_index_milli = before.arcade_visual_index_milli;
        let mut moved = stationary.clone();
        moved.arcade_visual_index_milli =
            Some(before.arcade_visual_index_milli.unwrap_or_default() + 25);

        assert!(!selected.matches_presented(&before, &stationary));
        assert!(selected.matches_presented(&before, &moved));
    }

    #[test]
    fn arming_orientation_confirmation_sets_destination_dialog_state() {
        let mut nav = LauncherNav::new();

        arm_orientation_confirmation(&mut nav);

        assert_eq!(
            nav.confirm_action,
            Some(launcher::ConfirmAction::ScreenOrientation)
        );
        assert_eq!(nav.confirm_selected, 0);
        assert_eq!(
            nav.orientation_confirm_remaining,
            launcher::DISPLAY_CONFIRM_SECONDS
        );
    }

    #[test]
    fn modal_input_test_requires_every_path_below_fixed_tmp_root() {
        assert!(modal_input_test_paths_are_isolated([
            Path::new("/tmp/mister-magik/modal-input-benchmark/catalog-v3"),
            Path::new("/tmp/mister-magik/modal-input-benchmark/library.sqlite3"),
            Path::new("/tmp/mister-magik/modal-input-benchmark/catalog-ready.snapshot"),
        ]));
        assert!(!modal_input_test_paths_are_isolated([
            Path::new("/tmp/mister-magik/modal-input-benchmark/catalog-v3"),
            Path::new("/media/fat/mister-magik-dev/library.sqlite3"),
        ]));
        assert!(!modal_input_test_paths_are_isolated([Path::new(
            "/tmp/mister-magik/modal-input-benchmark"
        ),]));
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
                x0: 48,
                y0: 128,
                x1: 592,
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
                x0: 8,
                y0: 56,
                x1: 518,
                y1: 508,
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
                x0: 294,
                y0: 128,
                x1: 592,
                y1: 392,
            }
        );
    }

    #[test]
    fn crt_routes_use_roomier_rows_in_normal_and_search_layouts() {
        for (pal, scandoubler, expected_row_height, expected_full_rows) in
            [(0, 0, 32, 8), (1, 0, 19, 7), (0, 1, 32, 12), (1, 1, 39, 11)]
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
                    expected_full_rows
                );
                let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &display);
                renderer.set_geometry_for_visible_height(geometry, visible_height);
                assert_eq!(
                    (renderer.selection_rect().y0 - renderer.dirty_rect().y0)
                        / metrics.game_row_height as usize,
                    (expected_full_rows / 2).saturating_sub(1)
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
            settings_page_transition(Screen::Screensaver, Screen::Home),
            Some((
                NavigationTransitionRoute::NestedToHome,
                NavigationTransitionDirection::Reverse
            ))
        );
        assert_eq!(settings_page_transition(Screen::Home, Screen::Arcade), None);
        assert_eq!(
            settings_page_transition(Screen::Screensaver, Screen::About),
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
    fn sequential_dispatch_recomputes_focus_after_modal_opens() {
        let catalog = empty_arcade_catalog("/tmp");
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Settings;
        nav.settings_selected = 4;
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
        assert!(navigation_transition_for_intent(&nav, &navigation).is_some());
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
    #[cfg(mister_experiments)]
    use crate::ui_effect_bench::{EffectFill, EffectTarget};
    #[cfg(mister_experiments)]
    use mister_magik_fb::experiments::effects::framebuffer_effects::EffectSize;

    #[test]
    fn screenshot_media_actions_follow_route_capability() {
        fn dispatched_media_actions(policy: PreviewRoutePolicy) -> Vec<&'static str> {
            let now = Instant::now();
            let mut catalog_session = LauncherCatalogSession::new(false);
            let catalog_effects = catalog_session.handle_worker_message(
                CatalogWorkerMessageContext {
                    catalog_ready: false,
                    catalog_partial: true,
                    screen: Screen::Home,
                    media_gate: None,
                },
                CatalogWorkerMessage::SystemDiscovered {
                    system_id: "arcade".to_string(),
                },
                now,
            );
            let mut media_session = ScreenshotMediaUpdateSession::default();
            let mut actions = Vec::new();
            for effect in catalog_effects.into_effects() {
                let Some(media_effects) =
                    dispatch_catalog_media_effect(policy, &effect, &mut media_session)
                else {
                    continue;
                };
                actions.extend(
                    media_effects
                        .into_effects()
                        .into_iter()
                        .filter_map(|effect| match effect {
                            ScreenshotMediaUpdateEffect::EnsureWorker { .. } => {
                                Some("ensure-worker")
                            }
                            ScreenshotMediaUpdateEffect::EnsureSystem { .. } => {
                                Some("ensure-system")
                            }
                            ScreenshotMediaUpdateEffect::SetInteractionActive { .. } => {
                                Some("set-interaction")
                            }
                            _ => None,
                        }),
                );
            }
            actions
        }

        assert!(
            dispatched_media_actions(PreviewRoutePolicy::for_output_route(
                ResolvedOutputRoute::Crt480p60,
            ))
            .is_empty()
        );
        assert_eq!(
            dispatched_media_actions(PreviewRoutePolicy::for_output_route(
                ResolvedOutputRoute::Crt240p60,
            )),
            vec!["ensure-worker", "set-interaction", "ensure-system"]
        );
        assert_eq!(
            dispatched_media_actions(PreviewRoutePolicy::for_output_route(
                ResolvedOutputRoute::Hdmi,
            )),
            vec!["ensure-worker", "set-interaction", "ensure-system"]
        );
    }

    #[test]
    fn crt_profile_terminal_tracks_the_composed_backdrop_not_hdmi_layer_state() {
        let crt = PreviewRoutePolicy::for_output_route(ResolvedOutputRoute::Crt240p60);
        assert!(preview_terminal_for_route(
            crt, "exact", "loading", true, false, true, false,
        ));
        assert!(preview_terminal_for_route(
            crt, "empty", "loading", false, true, true, false,
        ));
        assert!(!preview_terminal_for_route(
            crt, "exact", "loading", true, false, false, false,
        ));
        assert!(!preview_terminal_for_route(
            crt, "exact", "loading", true, false, true, true,
        ));

        let hdmi = PreviewRoutePolicy::for_output_route(ResolvedOutputRoute::Hdmi);
        assert!(preview_terminal_for_route(
            hdmi, "exact", "visible", true, false, false, true,
        ));
        assert!(!preview_terminal_for_route(
            hdmi, "exact", "loading", true, false, true, false,
        ));
    }

    #[test]
    fn crt_route_policy_is_fixed_to_the_supported_backdrop_matrix() {
        let hdmi = PreviewRoutePolicy::for_output_route(ResolvedOutputRoute::Hdmi);
        assert!(hdmi.allows_preview_work());
        assert!(hdmi.allows_hdmi_preview());
        assert!(!hdmi.allows_crt_backdrop());

        for route in [
            ResolvedOutputRoute::Crt240p60,
            ResolvedOutputRoute::Crt288p50,
        ] {
            let crt = PreviewRoutePolicy::for_output_route(route);
            assert!(crt.allows_preview_work());
            assert!(!crt.allows_hdmi_preview());
            assert!(crt.allows_crt_backdrop());
        }

        for route in [
            ResolvedOutputRoute::Crt480p60,
            ResolvedOutputRoute::Crt576p50,
        ] {
            let unsupported = PreviewRoutePolicy::for_output_route(route);
            assert!(!unsupported.allows_preview_work());
            assert!(!unsupported.allows_hdmi_preview());
            assert!(!unsupported.allows_crt_backdrop());
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
    fn media_benchmark_contention_disables_only_the_benchmark_media_gate() {
        assert!(benchmark_media_interaction_gate_active(true, false));
        assert!(!benchmark_media_interaction_gate_active(true, true));
        assert!(!benchmark_media_interaction_gate_active(false, false));
    }

    #[test]
    fn media_stays_gated_through_ready_and_opens_after_persistence() {
        let now = Instant::now();
        let mut session = LauncherCatalogSession::new(false);
        let idle = MediaInteractionGate {
            active: false,
            reason: "idle",
        };
        let ready = CatalogWorkerMessage::Ready {
            catalog: catalog_for_media_systems(&["arcade"]),
            summary: None,
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
                screen: Screen::Home,
                media_gate: None,
            },
            ready,
            now,
        );
        let gated = catalog_build_media_gate(session.refresh_done(), idle);
        assert!(gated.active);
        assert_eq!(gated.reason, "catalog-build");

        session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: true,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Persisted {
                summary: library_db::LibraryRefreshSummary {
                    skipped: false,
                    scan_us: 1,
                    discover_us: 1,
                    classify_us: 1,
                    import_us: 1,
                    bytes: 1,
                    normal_files: 1,
                    containers: 0,
                    entries: 0,
                    audit_rows: 0,
                    discoveries: 1,
                },
                completed_build_seconds: Some(120),
                generation_fingerprint: None,
            },
            now,
        );
        assert_eq!(catalog_build_media_gate(session.refresh_done(), idle), idle);
    }

    #[cfg(not(feature = "bench-tools"))]
    #[test]
    fn production_build_cannot_enable_media_benchmark_contention() {
        assert!(!media_benchmark_contention_enabled());
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
    fn catalog_publication_syncs_before_startup_input_is_enabled() {
        let mut session = LauncherCatalogSession::new(false);
        let effects = session.handle_worker_message(
            CatalogWorkerMessageContext {
                catalog_ready: false,
                catalog_partial: false,
                screen: Screen::Home,
                media_gate: None,
            },
            CatalogWorkerMessage::Ready {
                catalog: catalog_for_media_systems(&["arcade", "amiga"]),
                summary: None,
                load_us: 0,
                source: CatalogSource::FreshBuild,
                durable_save_pending: false,
                generation_fingerprint: None,
                publication_ack: None,
            },
            Instant::now(),
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
            launcher_bridge_sync_plan(false, false, full_bridge_dirty, false),
            LauncherBridgeSyncPlan::Full
        );
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[cfg(mister_experiments)]
    #[test]
    pub(super) fn effect_half_target_allows_640x448_at_native_scale() {
        let ui = UiDisplay::for_framebuffer(1920, 1080);
        let target = EffectTarget::new(EffectFill::Half, EffectSize { w: 640, h: 448 }, &ui)
            .expect("640x448 should fit in half-fill benchmark mode");

        assert_eq!(target.physical_w, 640);
        assert_eq!(target.physical_h, 448);
        assert_eq!(target.render_w, 640);
        assert_eq!(target.render_h, 448);
        assert_eq!(target.scale, 1);
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
        let mut scheduler = LauncherScheduler::new(false);
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
        let mut scheduler = LauncherScheduler::new(false);
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
        assert!(launched_nav.open_system(&full_catalog, "c64"));
        let state = launcher::capture_launch_return_state(
            &launched_nav,
            &full_catalog,
            "/media/fat/_Arcade/c64.mra",
        )
        .expect("return state");
        let registry = arcade_catalog(Vec::new(), vec![arcade_system("c64", 1)]);
        let mut restored_nav = LauncherNav::new();
        restored_nav.sync_launcher_taxonomy(&registry);
        let mut scheduler = LauncherScheduler::new(false);
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
        let mut scheduler = LauncherScheduler::new(false);
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
        assert!(launched_nav.open_system(&catalog, "c64"));
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
        assert_eq!(launched_nav.screen, Screen::SystemHub);
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
        let mut scheduler = LauncherScheduler::new(false);
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
        assert!(launched_nav.open_system(&catalog, "c64"));
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
        assert!(nav.open_system(&catalog, "c64"));
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
        assert!(launched_nav.open_system(&full_catalog, "c64"));
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
        assert!(launched_nav.open_system(&catalog, "c64"));
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

        assert!(empty_collection_invariant_violated(
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
            LauncherWorkerUiIntent::HideCatalogBackgroundScan,
        );

        assert!(matches!(
            replay,
            Some(LauncherWorkerUiIntent::HideCatalogBackgroundScan)
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
    fn start_system_env_selects_matching_system_and_enters_arcade() {
        let catalog = catalog_for_media_systems(&["arcade", "neogeo", "saturn"]);
        let mut nav = LauncherNav::new();

        assert!(apply_start_system_from_env(
            &mut nav, &catalog, "neogeo", None,
        ));

        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.arcade.selected, 0);
        assert_eq!(nav.arcade_filter.active, arcade_catalog::ArcadeFilter::All);
    }

    #[test]
    fn start_system_env_preserves_forced_arcade_selected_index() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("Arcade Game")
                    .path("/media/fat/_Arcade/arcade.mra")
                    .system_id("arcade")
                    .build(),
                arcade_game("Neo Geo First")
                    .path("/media/fat/_Arcade/neogeo-first.mra")
                    .system_id("neogeo")
                    .build(),
                arcade_game("Neo Geo Second")
                    .path("/media/fat/_Arcade/neogeo-second.mra")
                    .system_id("neogeo")
                    .build(),
                arcade_game("Saturn Game")
                    .path("/media/fat/_Arcade/saturn.mra")
                    .system_id("saturn")
                    .build(),
            ],
            vec![
                arcade_system("arcade", 1),
                arcade_system("neogeo", 2),
                arcade_system("saturn", 1),
            ],
        );
        let mut nav = LauncherNav::new();
        let applied = apply_start_system_from_env(&mut nav, &catalog, "neogeo", Some(1));
        assert!(applied);
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.arcade.selected, 1);
    }

    #[test]
    fn start_system_env_matches_case_insensitively() {
        let catalog = catalog_for_media_systems(&["arcade", "neogeo", "saturn"]);
        let mut nav = LauncherNav::new();

        assert!(apply_start_system_from_env(
            &mut nav, &catalog, "SATURN", None,
        ));

        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.selected, 2);
    }

    #[test]
    fn auto_launch_gate_waits_for_requested_file() {
        let gate = std::env::temp_dir().join(format!(
            "mister-magik-auto-launch-gate-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&gate);
        assert!(!launcher_auto_launch_gate_ready_from_value(Some(
            gate.to_str().expect("gate path")
        )));
        std::fs::write(&gate, b"ready\n").expect("write launch gate");
        assert!(launcher_auto_launch_gate_ready_from_value(Some(
            gate.to_str().expect("gate path")
        )));

        let _ = std::fs::remove_file(gate);
        assert!(launcher_auto_launch_gate_ready_from_value(None));
        assert!(launcher_auto_launch_gate_ready_from_value(Some("  ")));
    }

    #[test]
    fn start_system_env_fails_without_changing_nav_for_missing_system() {
        let catalog = catalog_for_media_systems(&["arcade", "neogeo", "saturn"]);
        let mut nav = LauncherNav::new();

        assert!(!apply_start_system_from_env(
            &mut nav, &catalog, "psx", None,
        ));

        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.selected, 0);
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
        let source_bridge = LauncherBridgeKey::from_nav(&nav);

        assert!(!commit_pending_collection_entry(
            &mut pending,
            &mut nav,
            &empty,
            Instant::now()
        ));
        assert_eq!(nav.screen, Screen::Home);
        assert!(pending.is_some());
        assert_eq!(LauncherBridgeKey::from_nav(&nav).screen, Screen::Home);
        assert_eq!(
            LauncherBridgeKey::from_nav(&nav).menu_id,
            source_bridge.menu_id
        );

        assert!(commit_pending_collection_entry(
            &mut pending,
            &mut nav,
            &hydrated,
            Instant::now()
        ));
        assert_eq!(nav.screen, Screen::Arcade);
        assert!(pending.is_none());
        assert_eq!(active_system_game_view(&hydrated, &nav).len(), 1);
        assert!(!empty_collection_invariant_violated(&hydrated, &nav));
        assert_eq!(LauncherBridgeKey::from_nav(&nav).screen, Screen::Arcade);
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

        for (open_game_list_directly, expected_screen) in
            [(false, Screen::SystemHub), (true, Screen::Arcade)]
        {
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
            assert_eq!(nav.screen, expected_screen);
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
        assert!(nav.open_system(&catalog, "c64"));

        assert!(empty_collection_invariant_violated(&catalog, &nav));
        nav.recover_empty_collection_to_home();
        assert!(!empty_collection_invariant_violated(&catalog, &nav));
    }

    fn ready_catalog_message() -> CatalogWorkerMessage {
        CatalogWorkerMessage::Ready {
            catalog: catalog_for_media_systems(&["arcade"]),
            summary: None,
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
            summary: None,
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

        let message = CatalogWorkerMessage::Unchanged {
            summary: library_db::LibraryRefreshSummary {
                skipped: true,
                scan_us: 1,
                discover_us: 1,
                classify_us: 1,
                import_us: 1,
                bytes: 0,
                normal_files: 0,
                containers: 0,
                entries: 0,
                audit_rows: 0,
                discoveries: 0,
            },
        };

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
    pub(super) fn summary_projection_without_hot_rows_is_not_ready_for_arcade_navigation() {
        let catalog = summary_catalog_for_media_systems(&["arcade", "amiga"]);

        assert!(!arcade_catalog_rows_ready(&catalog));
        assert!(!arcade_navigation_ready(true, &catalog));
        assert_eq!(
            effective_lock_screen(Some(Screen::Arcade), true, &catalog),
            None
        );
    }

    #[test]
    pub(super) fn summary_hot_arcade_rows_are_ready_for_arcade_navigation() {
        let full_catalog = catalog_for_media_systems(&["arcade", "cps1", "amiga"]);
        let stamp = mister_magik_catalog::catalog_stamp::CatalogStamp::from_lines(vec![
            "root\t/media/fat".to_string(),
        ]);
        let summary =
            catalog_summary::CatalogSummaryProjection::from_catalog(&full_catalog, &stamp);
        let catalog = catalog_from_summary("/media/fat/_Arcade", &summary);
        let mut nav = LauncherNav::new();
        assert!(nav.open_default_arcade(&catalog));

        assert!(!active_system_games_loading(&catalog, &nav));
        assert!(arcade_catalog_rows_ready(&catalog));
        assert!(arcade_navigation_ready(true, &catalog));
        assert_eq!(catalog.system_game_count("arcade"), 1);
        assert_eq!(
            catalog.system_game_count(arcade_catalog::MENU_ARCADE_SYSTEM_ID),
            2
        );
        assert_eq!(
            catalog
                .system_game_view(arcade_catalog::MENU_ARCADE_SYSTEM_ID)
                .iter()
                .map(|game| game.title.as_ref())
                .collect::<Vec<_>>(),
            vec!["arcade game", "cps1 game"]
        );
    }

    #[test]
    pub(super) fn sharded_registry_keeps_system_authority_and_summary_hot_rows() {
        let full_catalog = catalog_for_media_systems(&["arcade", "cps1", "amiga"]);
        let stamp = mister_magik_catalog::catalog_stamp::CatalogStamp::from_lines(vec![
            "root\t/media/fat".to_string(),
        ]);
        let summary =
            catalog_summary::CatalogSummaryProjection::from_catalog(&full_catalog, &stamp);
        let sharded = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            Vec::new(),
            vec![arcade_catalog::GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade from V3".into(),
                count: 1234,
            }],
        );

        let catalog =
            catalog_from_sharded_registry_and_summary("/media/fat/_Arcade", sharded, &summary);

        assert_eq!(catalog.systems.len(), 1);
        assert_eq!(catalog.systems[0].title, "Arcade from V3");
        assert_eq!(catalog.systems[0].count, 1234);
        assert_eq!(
            catalog.system_game_count(arcade_catalog::MENU_ARCADE_SYSTEM_ID),
            2
        );
    }

    #[test]
    pub(super) fn valid_sharded_seed_never_reads_the_legacy_summary() {
        assert!(!legacy_summary_seed_needed(false, true));
        assert!(!legacy_summary_seed_needed(true, false));
        assert!(!legacy_summary_seed_needed(true, true));
        assert!(legacy_summary_seed_needed(false, false));
    }

    #[test]
    pub(super) fn full_catalog_is_ready_for_arcade_navigation() {
        let catalog = catalog_for_media_systems(&["arcade", "amiga"]);

        assert!(arcade_catalog_rows_ready(&catalog));
        assert!(arcade_navigation_ready(true, &catalog));
        assert_eq!(
            effective_lock_screen(Some(Screen::Arcade), true, &catalog),
            Some(Screen::Arcade)
        );
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
    pub(super) fn orientation_benchmark_selects_landscape_layout_before_window_creation() {
        for persisted in [
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
        ] {
            assert_eq!(
                launcher_startup_orientation(persisted, None, true, false),
                ScreenOrientation::Normal
            );
            assert_eq!(
                launcher_startup_orientation(persisted, None, false, false),
                persisted
            );
            assert_eq!(
                launcher_startup_orientation(
                    persisted,
                    Some(ScreenOrientation::Normal),
                    false,
                    false,
                ),
                ScreenOrientation::Normal
            );
        }
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
    pub(super) fn settings_navigation_benchmark_starts_in_landscape() {
        let persisted = ScreenOrientation::MonitorCounterclockwise;
        assert_eq!(
            launcher_startup_orientation(persisted, None, false, true),
            ScreenOrientation::Normal
        );
    }

    #[test]
    pub(super) fn settings_capture_uses_one_source_carrier_only_while_capture_is_pending() {
        let mut transition = FullScreenTransitionStateChart::default();
        let generation = transition
            .begin(FullScreenTransitionOwner::Navigation)
            .unwrap();
        let capture_policy = transition.policy();

        assert!(navigation_capture_source_carrier_required(
            capture_policy,
            transition.owner(),
            NavigationTransitionPhase::Capture,
            true,
        ));
        assert!(!navigation_capture_source_carrier_required(
            capture_policy,
            transition.owner(),
            NavigationTransitionPhase::Expand,
            true,
        ));
        assert!(!navigation_capture_source_carrier_required(
            capture_policy,
            transition.owner(),
            NavigationTransitionPhase::Capture,
            false,
        ));

        assert!(transition.take_controlled_capture(generation).unwrap());
        assert!(!navigation_capture_source_carrier_required(
            transition.policy(),
            transition.owner(),
            NavigationTransitionPhase::Capture,
            true,
        ));
    }

    #[test]
    pub(super) fn orientation_capture_uses_source_carrier_until_destination_is_ready() {
        let mut transition = FullScreenTransitionStateChart::default();
        let generation = transition
            .begin(FullScreenTransitionOwner::Orientation)
            .unwrap();
        let capture_policy = transition.policy();

        assert!(orientation_capture_source_carrier_required(
            capture_policy,
            transition.owner(),
            true,
            false,
        ));
        assert!(!orientation_capture_source_carrier_required(
            capture_policy,
            transition.owner(),
            false,
            false,
        ));
        assert!(!orientation_capture_source_carrier_required(
            capture_policy,
            transition.owner(),
            true,
            true,
        ));
        assert!(transition.take_controlled_capture(generation).unwrap());
        assert!(!orientation_capture_source_carrier_required(
            transition.policy(),
            transition.owner(),
            true,
            false,
        ));
    }

    #[test]
    pub(super) fn settings_evidence_waits_for_fresh_status_with_a_bounded_fallback() {
        assert!(!settings_navigation_status_drain_complete(
            SETTINGS_NAVIGATION_STATUS_DRAIN_MIN - Duration::from_millis(1),
            true,
        ));
        assert!(settings_navigation_status_drain_complete(
            SETTINGS_NAVIGATION_STATUS_DRAIN_MIN,
            true,
        ));
        assert!(!settings_navigation_status_drain_complete(
            SETTINGS_NAVIGATION_STATUS_DRAIN_LIMIT - Duration::from_millis(1),
            false,
        ));
        assert!(settings_navigation_status_drain_complete(
            SETTINGS_NAVIGATION_STATUS_DRAIN_LIMIT,
            false,
        ));
    }

    #[test]
    pub(super) fn settings_evidence_reuses_completion_frame_status_submission() {
        assert_eq!(settings_navigation_status_drain_plan(7, 8), (7, false));
        assert_eq!(settings_navigation_status_drain_plan(8, 8), (8, true));
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
    pub(super) fn genuinely_empty_catalog_rows_are_not_pending_summary_rows() {
        let catalog = empty_arcade_catalog("/media/fat/_Arcade");

        assert!(!active_system_games_loading(&catalog, &LauncherNav::new()));
        assert!(arcade_catalog_rows_ready(&catalog));
        assert!(!arcade_navigation_ready(false, &catalog));
    }

    #[test]
    pub(super) fn catalog_media_system_ids_are_selective_and_supported() {
        let catalog = catalog_for_media_systems(&["arcade", "pcengine", "neogeo", "arcade"]);

        assert_eq!(
            catalog_media_system_ids(&catalog),
            vec!["arcade".to_string(), "neogeo".to_string()]
        );
    }

    #[test]
    pub(super) fn catalog_media_system_ids_use_summary_counts_before_full_hydration() {
        let catalog = summary_catalog_for_media_systems(&["arcade", "pcengine", "neogeo"]);

        assert_eq!(
            catalog_media_system_ids(&catalog),
            vec!["arcade".to_string(), "neogeo".to_string()]
        );
    }

    #[test]
    pub(super) fn catalog_summary_seed_requires_usable_sqlite_database() {
        let root = unique_temp_dir("catalog-summary-seed");
        let db = root.join("library.sqlite3");
        let summary_path = catalog_summary::summary_path_for_sqlite(&db);
        let summary = catalog_summary::CatalogSummaryProjection {
            schema: catalog_summary::CATALOG_SUMMARY_SCHEMA_VERSION,
            catalog_schema_version: mister_magik_catalog::catalog_config::SCHEMA_VERSION,
            catalog_build_version: mister_magik_catalog::catalog_config::CATALOG_BUILD_VERSION,
            catalog_generation: "test-generation".to_string(),
            catalog_stamp_fingerprint: "test-generation".to_string(),
            catalog_stamp_lines: Vec::new(),
            total_game_count: 7,
            systems: vec![catalog_summary::CatalogSummarySystem {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 7,
                platform_kind: arcade_catalog::PlatformKind::Arcade,
                supported_media: vec!["screenshots".to_string()],
            }],
            hot_games: Vec::new(),
        };
        std::fs::write(
            &summary_path,
            serde_json::to_vec(&summary).expect("summary json"),
        )
        .expect("write summary");

        assert!(read_catalog_summary_seed(&db, &summary_path, Instant::now()).is_none());

        std::fs::write(&db, b"").expect("write zero-byte sqlite placeholder");
        assert!(read_catalog_summary_seed(&db, &summary_path, Instant::now()).is_none());

        std::fs::write(&db, b"not-a-sqlite-db").expect("write corrupt sqlite placeholder");
        assert!(read_catalog_summary_seed(&db, &summary_path, Instant::now()).is_none());

        std::fs::write(&db, SQLITE_HEADER).expect("write sqlite header");
        assert!(
            read_catalog_summary_seed(&db, &summary_path, Instant::now()).is_none(),
            "warm summary seed must require a current SQLite catalog stamp, not just a SQLite header"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    pub(super) fn home_boot_with_ready_catalog_hides_catalog_popup() {
        assert!(!initial_catalog_scan_visible(true, false, true, false));
        assert!(initial_catalog_scan_visible(true, false, true, true));
    }

    #[test]
    pub(super) fn library_changed_test_driver_presses_continue_dialog_button() {
        let start = Instant::now();
        let mut nav = LauncherNav::new();
        let mut driver = LibraryChangedDialogTestDriver {
            choice: Some(launcher::LibraryChangedTestDialogChoice::Continue),
            dialog_seen_at: None,
            phase: LibraryChangedDialogTestPhase::Waiting,
            next_sequence: 0,
            next_press_id: 0,
            active_press: None,
        };

        assert!(driver.event_for(&nav, start, start).is_none());
        nav.confirm_action = Some(launcher::ConfirmAction::LibraryChanged);

        assert!(driver.event_for(&nav, start, start).is_none());
        let input = driver
            .event_for(&nav, start + LIBRARY_CHANGED_TEST_ACTION_SETTLE, start)
            .expect("continue driver should press A");
        assert_eq!(input.action, LogicalAction::Activate);
        assert_eq!(input.phase, InputPhase::Pressed);
        let event = nav
            .handle_action_with_navigation_intents(
                &input,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE,
                &empty_arcade_catalog("/tmp"),
            )
            .expect("continue button should choose stale library");
        assert_eq!(event.action, LauncherAction::ContinueWithStaleLibrary);
        assert_eq!(nav.confirm_action, None);
    }

    #[test]
    fn display_transactions_rearm_vsync_after_every_stable_boundary() {
        let source = include_str!("launcher_loop.rs");
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
    pub(super) fn library_changed_test_driver_selects_rebuild_dialog_button() {
        let start = Instant::now();
        let mut nav = LauncherNav::new();
        nav.confirm_action = Some(launcher::ConfirmAction::LibraryChanged);
        let mut driver = LibraryChangedDialogTestDriver {
            choice: Some(launcher::LibraryChangedTestDialogChoice::Rebuild),
            dialog_seen_at: None,
            phase: LibraryChangedDialogTestPhase::Waiting,
            next_sequence: 0,
            next_press_id: 0,
            active_press: None,
        };
        let catalog = empty_arcade_catalog("/tmp");

        assert!(driver.event_for(&nav, start, start).is_none());
        let right = driver
            .event_for(&nav, start + LIBRARY_CHANGED_TEST_ACTION_SETTLE, start)
            .expect("rebuild driver should press right first");
        assert_eq!(right.action, LogicalAction::Right);
        assert_eq!(right.phase, InputPhase::Pressed);
        assert!(
            nav.handle_action_with_navigation_intents(
                &right,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE,
                &catalog,
            )
            .is_none()
        );
        assert_eq!(nav.confirm_selected, 1);

        let release = driver
            .event_for(
                &nav,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(16),
                start,
            )
            .expect("rebuild driver should release right before A");
        assert_eq!(release.action, LogicalAction::Right);
        assert_eq!(release.phase, InputPhase::Released);
        assert!(
            nav.handle_action_with_navigation_intents(
                &release,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(16),
                &catalog,
            )
            .is_none()
        );

        let press_a = driver
            .event_for(
                &nav,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(32),
                start,
            )
            .expect("rebuild driver should press A");
        assert_eq!(press_a.action, LogicalAction::Activate);
        assert_eq!(press_a.phase, InputPhase::Pressed);
        let event = nav
            .handle_action_with_navigation_intents(
                &press_a,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(32),
                &catalog,
            )
            .expect("A should confirm rebuild");
        assert_eq!(event.action, LauncherAction::RebuildLibrary);
        assert_eq!(nav.confirm_action, None);
    }

    #[test]
    pub(super) fn launcher_input_script_presses_and_releases_each_button() {
        let start = Instant::now();
        let mut driver = LauncherInputScriptDriver::from_script("left,down,right", start);
        driver.wait_frames = 0;
        let mut frame = 0_u64;

        let left = driver.event_for(frame).expect("left press");
        assert_eq!(left.action, LogicalAction::Left);
        assert_eq!(left.phase, InputPhase::Pressed);

        for _ in 1..LAUNCHER_INPUT_SCRIPT_PRESS_FRAMES {
            frame += 1;
            assert!(driver.event_for(frame).is_none());
        }
        frame += 1;
        let release = driver.event_for(frame).expect("left release");
        assert_eq!(release.action, LogicalAction::Left);
        assert_eq!(release.phase, InputPhase::Released);
        for _ in 1..LAUNCHER_INPUT_SCRIPT_RELEASE_FRAMES {
            frame += 1;
            assert!(driver.event_for(frame).is_none());
        }
        frame += 1;
        assert!(driver.event_for(frame).is_none());

        frame += 1;
        let down = driver.event_for(frame).expect("down press");
        assert_eq!(down.action, LogicalAction::Down);
        assert_eq!(down.phase, InputPhase::Pressed);
    }

    #[test]
    fn screensaver_show_navigation_script_uses_production_settings() {
        let start = Instant::now();
        let catalog = empty_arcade_catalog("/tmp");
        let mut nav = LauncherNav::new();
        let mut driver =
            LauncherInputScriptDriver::from_script("up,a,down,down,a,down,down,a", start);
        driver.wait_frames = 0;
        let mut action = None;
        let mut frame = 0_u64;

        while driver.active() {
            let frame_now = start + Duration::from_millis(frame * 17);
            if let Some(input) = driver.event_for(frame * 17_000)
                && let Some(event) =
                    nav.handle_action_with_navigation_intents(&input, frame_now, &catalog)
            {
                action = Some(event.action);
            }
            frame += 1;
        }

        assert_eq!(nav.screen, Screen::Screensaver);
        assert_eq!(nav.screensaver_selected, 2);
        assert_eq!(action, Some(LauncherAction::PreviewScreensaver));
    }

    #[test]
    pub(super) fn arcade_bench_waits_for_initial_visible_preview() {
        let scenario = LauncherBenchScenario::HeldScroll;

        assert!(!launcher_bench_initial_preview_ready(
            scenario,
            "placeholder",
            true
        ));
        assert!(!launcher_bench_initial_preview_ready(
            scenario, "cached", true
        ));
        assert!(!launcher_bench_initial_preview_ready(
            scenario, "stale", true
        ));
        assert!(!launcher_bench_initial_preview_ready(
            scenario, "empty", true
        ));
        assert!(launcher_bench_initial_preview_ready(
            scenario, "exact", true
        ));
        assert!(launcher_bench_initial_preview_ready(
            scenario, "empty", false
        ));
    }

    #[test]
    pub(super) fn non_arcade_bench_does_not_wait_for_preview() {
        assert!(launcher_bench_initial_preview_ready(
            LauncherBenchScenario::HomeNav,
            "placeholder",
            true
        ));
    }

    #[test]
    pub(super) fn missing_catalog_shows_catalog_popup_on_home_or_arcade_boot() {
        assert!(initial_catalog_scan_visible(false, false, true, false));
        assert!(initial_catalog_scan_visible(false, true, true, false));
        assert!(!initial_catalog_scan_visible(true, true, true, false));
        assert!(!initial_catalog_scan_visible(false, true, false, false));
    }

    #[test]
    pub(super) fn ready_catalog_foreground_rebuild_uses_full_screen_progress() {
        for title in ["Indexing library", "Loading library"] {
            let full_visible = catalog_scan_progress_visible(true, Screen::Home, title, true);
            assert!(full_visible, "{title} should cover a foreground rebuild");
            assert!(!catalog_background_scan_progress_visible(
                true,
                full_visible,
                title
            ));
        }
    }

    #[test]
    pub(super) fn cached_home_validation_progress_stays_hidden() {
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Validating library",
            false
        ));
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Preview images changed",
            false
        ));
        assert!(catalog_background_scan_progress_visible(
            true,
            false,
            "Validating library"
        ));
        assert!(catalog_background_scan_progress_visible(
            true,
            false,
            "Checking library"
        ));
    }

    #[test]
    pub(super) fn missing_catalog_and_rebuild_progress_are_visible() {
        assert!(catalog_scan_progress_visible(
            false,
            Screen::Home,
            "Indexing library",
            false
        ));
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Indexing library",
            true
        ));
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Library changed",
            false
        ));
        assert!(!catalog_background_scan_progress_visible(
            true,
            true,
            "Indexing library"
        ));
    }

    #[test]
    pub(super) fn catalog_scan_failures_are_visible_even_with_cache() {
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Library scan failed",
            false
        ));
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Arcade,
            "Library load failed",
            false
        ));
        assert!(!catalog_background_scan_progress_visible(
            true,
            true,
            "Library scan failed"
        ));
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
    pub(super) fn catalog_work_pauses_for_interaction_and_bursts_after_static_settle() {
        let started = Instant::now();
        let mut idle_since = None;
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
    pub(super) fn launcher_idle_wait_rejects_active_work() {
        for reason in [
            LauncherWakeReasons::REDRAW_PENDING,
            LauncherWakeReasons::LAUNCHING,
            LauncherWakeReasons::SETUP_ACTIVE,
            LauncherWakeReasons::BENCHMARK_ACTIVE,
            LauncherWakeReasons::SCRIPTED_INPUT_ACTIVE,
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
            LauncherWakeReasons::FB0_ROUTE_RECOVERY_PENDING,
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
            Screen::Screensaver,
            Screen::About,
            Screen::Licenses,
            Screen::Info,
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
    pub(super) fn presenter_recovery_keeps_launcher_awake() {
        let sleeping_intent = |wake_reasons| LauncherRenderIntent {
            first_visible_copy_done: true,
            startup_input_enabled: true,
            wake_reasons,
        };

        assert!(sleeping_intent(launcher_presentation_recovery_wake_reasons(false)).can_sleep());
        assert!(!sleeping_intent(launcher_presentation_recovery_wake_reasons(true)).can_sleep());
        assert!(sleeping_intent(launcher_presentation_recovery_wake_reasons(false)).can_sleep());
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
    pub(super) fn home_repeat_benchmark_counts_as_active_home_motion() {
        assert!(home_repeat_benchmark_active(Some(
            LauncherBenchScenario::HomeRepeatHold
        )));
        assert!(!home_repeat_benchmark_active(Some(
            LauncherBenchScenario::HomeNav
        )));
        assert!(!home_repeat_benchmark_active(None));
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
    pub(super) fn summary_seed_worker_reuses_the_loaded_navigation_projection() {
        assert_eq!(
            summary_seed_catalog_worker_initial_cache(CatalogWorkerRequest::CheckStamp, false),
            CatalogWorkerInitialCache::AlreadyLoadedReady
        );
        assert_eq!(
            summary_seed_catalog_worker_initial_cache(CatalogWorkerRequest::LoadOnly, true),
            CatalogWorkerInitialCache::AlreadyLoadedReady
        );
        assert_eq!(
            summary_seed_catalog_worker_initial_cache(
                CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                false,
            ),
            CatalogWorkerInitialCache::AlreadyLoadedReady
        );
    }

    #[test]
    pub(super) fn startup_without_navigation_projection_forces_a_fresh_build() {
        assert_eq!(
            catalog_startup_without_summary_plan(
                CatalogStartupSqliteState::HeaderValid,
                true,
                CatalogRefreshPolicy::Default,
                false,
            ),
            CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            }
        );
        assert_eq!(
            catalog_startup_without_summary_plan(
                CatalogStartupSqliteState::HeaderValid,
                false,
                CatalogRefreshPolicy::Off,
                false,
            ),
            CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            },
            "without a navigation projection the retired SQLite cache is not a startup source"
        );
        assert_eq!(
            catalog_startup_without_summary_plan(
                CatalogStartupSqliteState::Missing,
                true,
                CatalogRefreshPolicy::Default,
                false,
            ),
            CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            }
        );
        assert_eq!(
            catalog_startup_without_summary_plan(
                CatalogStartupSqliteState::Missing,
                false,
                CatalogRefreshPolicy::Off,
                false,
            ),
            CatalogStartupWithoutSummaryPlan::NoCatalog
        );
    }

    #[test]
    pub(super) fn existing_invalid_sqlite_forces_v3_rebuild_after_first_frame() {
        let root = unique_temp_dir("catalog-invalid-header-startup");
        let sqlite_path = root.join("library.sqlite3");
        assert_eq!(
            catalog_startup_sqlite_state(&sqlite_path),
            CatalogStartupSqliteState::Missing
        );

        std::fs::write(&sqlite_path, b"not-a-sqlite-database").expect("write invalid database");
        let sqlite_state = catalog_startup_sqlite_state(&sqlite_path);
        assert_eq!(sqlite_state, CatalogStartupSqliteState::ExistingUnusable);
        assert_eq!(
            catalog_startup_without_summary_plan(
                sqlite_state,
                true,
                CatalogRefreshPolicy::Default,
                false,
            ),
            CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            }
        );
        assert_eq!(
            catalog_startup_without_summary_plan(
                sqlite_state,
                false,
                CatalogRefreshPolicy::Off,
                false,
            ),
            CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            },
            "the retired SQLite cache is never used as a V3 startup source"
        );
        assert_eq!(
            catalog_startup_without_summary_plan(
                sqlite_state,
                true,
                CatalogRefreshPolicy::Force,
                false,
            ),
            CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            },
            "an explicit force request may rebuild the unusable catalog"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    pub(super) fn cold_catalog_worker_starts_after_first_copy_without_delay() {
        let before_copy =
            deferred_catalog_worker_start_policy(false, false, false, Duration::from_secs(2));
        assert!(!before_copy.allowed);
        assert_eq!(before_copy.delay, Duration::ZERO);
        assert!(before_copy.foreground);

        let after_copy =
            deferred_catalog_worker_start_policy(false, true, false, Duration::from_secs(2));
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
            deferred_catalog_worker_start_policy(false, false, true, Duration::from_secs(2));
        assert!(policy.allowed);
        assert_eq!(policy.delay, Duration::ZERO);
        assert!(policy.foreground);
    }

    #[test]
    pub(super) fn warm_catalog_worker_starts_without_an_interaction_gate() {
        let delay = Duration::from_secs(2);
        let allowed = deferred_catalog_worker_start_policy(true, true, false, delay);
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
        let mut resting = PadState::default();
        resting.left_x = 0.5;
        resting.right_y = -1.0;
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
    fn forced_hydration_with_a_usable_catalog_stays_background() {
        assert_eq!(
            catalog_hydration_execution_mode(CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS),
            CatalogExecutionMode::BackgroundInteractive
        );
        assert_eq!(
            catalog_hydration_execution_mode(CatalogWorkerRequest::LoadOnly),
            CatalogExecutionMode::BackgroundInteractive
        );
    }

    #[test]
    fn catalog_generation_becomes_capsule_eligible_only_after_matching_persistence() {
        let mut generation = CatalogGenerationState::default();
        generation.publish(Some("new".to_string()), false);
        assert!(generation.durable.is_none());

        generation.mark_durable(Some("old".to_string()));
        assert!(generation.durable.is_none());

        generation.mark_durable(Some("new".to_string()));
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
    fn startup_pending_display_only_enters_confirmation_for_the_ui_route() {
        let state = launcher::DisplayCommandState {
            active: "hdmi-1920x1080p60".to_string(),
            pending: Some("hdmi-1280x720p60".to_string()),
            remaining: launcher::DISPLAY_CONFIRM_SECONDS,
            phase: launcher::DisplayTransactionPhase::Provisional,
            error: None,
            return_to_settings: false,
        };
        let now = Instant::now();
        let mut ui_nav = LauncherNav::new();
        let deadline = apply_startup_pending_display(&mut ui_nav, &state, true, now);
        assert_eq!(ui_nav.screen, Screen::Settings);
        assert_eq!(
            ui_nav.confirm_action,
            Some(launcher::ConfirmAction::DisplayResolution)
        );
        assert_eq!(
            ui_nav.display_confirm_remaining,
            launcher::DISPLAY_CONFIRM_SECONDS
        );
        assert_eq!(
            deadline,
            Some(now + Duration::from_secs(u64::from(launcher::DISPLAY_CONFIRM_SECONDS)))
        );

        let mut headless_nav = LauncherNav::new();
        assert_eq!(
            apply_startup_pending_display(&mut headless_nav, &state, false, now),
            None
        );
        assert_eq!(headless_nav.screen, Screen::Home);
        assert_eq!(headless_nav.confirm_action, None);
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
    fn disabled_qualification_preserves_preview_for_pipeline_start() {
        let start = Instant::now();
        let next_frame = start + Duration::from_millis(16);
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);

        saver.preview(start);
        saver.set_qualification_particles(next_frame, false, true);
        saver.update(next_frame, false, Duration::from_secs(300), true, true);

        assert!(saver.active);
        assert!(saver.preview_active);
        assert!(!saver.restore_full_frame);
        assert!(screensaver_pipeline_start_allowed(saver.active, false));
    }

    #[test]
    fn enabled_qualification_particles_start_and_stop_screensaver() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);

        saver.set_qualification_particles(start, true, true);
        assert_eq!(saver.start_mode, ScreensaverStartMode::IdleWhenReady);
        assert!(!saver.active);

        saver.update(
            start + Duration::from_millis(16),
            false,
            Duration::from_secs(300),
            false,
            true,
        );
        assert!(saver.active);
        assert_eq!(saver.start_mode, ScreensaverStartMode::Inactive);

        saver.set_qualification_particles(start + Duration::from_millis(32), true, false);
        assert!(!saver.active);
        assert_eq!(saver.start_mode, ScreensaverStartMode::Inactive);
        assert!(saver.restore_full_frame);
    }

    #[test]
    fn settings_screensaver_preview_waits_for_activation_release_then_consumes_next_input() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);
        let mut physical_input = PadState::default();
        physical_input.btn_a = true;
        assert!(!saver.input_held_for_control(false, true));

        saver.preview(start);
        saver.update(start, true, Duration::from_secs(300), true, true);
        assert!(saver.active);
        assert_eq!(saver.preview_fade_alpha(start), Some(0));
        assert_eq!(
            saver.preview_fade_alpha(start + Duration::from_millis(100)),
            Some(127)
        );
        assert_eq!(
            saver.preview_fade_alpha(start + Duration::from_millis(200)),
            Some(255)
        );
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
}
