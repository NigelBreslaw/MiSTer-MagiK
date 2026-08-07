// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::arcade_drawer::{ArcadeDrawerViewCache, arcade_filter_cache_token};
use super::launcher_frame_accounting::{
    FrameAnalyticsCpuStamp, FrameAnalyticsMode, LauncherCustomDrawTrace, LauncherFrameAccounting,
    LauncherFrameCpuTrace, LauncherFrameIdentity, LauncherFrameRenderData,
    LauncherFrameSnapshotBuilder, LauncherFrameStatusData, LauncherFrameTiming,
};
use super::launcher_pacing::{
    FB0_LATE_FRAME_START_HEADROOM_US, LauncherFramePacingInput, LauncherFramePacingPolicy,
    LauncherPacingTrace, LauncherPhaseAlignment,
};
use super::launcher_screensaver::ScreensaverFrameTrace;
use super::launcher_worker_intents::{apply_launcher_worker_ui_intent, catalog_scan_message};
#[cfg(test)]
use super::launcher_worker_intents::{
    catalog_background_scan_progress_visible, catalog_scan_progress_visible,
};
use super::*;
use crate::input_state::PadState;
use crate::preview_state::PreviewApplyTrace;
use crate::preview_worker;
#[cfg(test)]
use mister_magik_catalog::catalog_summary;
use std::collections::{BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::path::Path;

const DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY: Duration = Duration::from_secs(2);
const CATALOG_READY_STATIONARY_EDGE_SETTLE: Duration = Duration::from_millis(250);
const LIBRARY_CHANGED_TEST_ACTION_SETTLE: Duration = Duration::from_millis(1200);
const LAUNCHER_INPUT_SCRIPT_DEFAULT_WAIT_FRAMES: usize = 60;
const LAUNCHER_INPUT_SCRIPT_PRESS_FRAMES: usize = 2;
const LAUNCHER_INPUT_SCRIPT_RELEASE_FRAMES: usize = 6;
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
const MODAL_INPUT_TEST_ROOT: &str = "/tmp/mister-magik/modal-input-benchmark";
const MODAL_INPUT_TEST_ENV: &str = "MISTER_MAGIK_TEST_CATALOG_RECOVERY_DIALOG";
const MODAL_INPUT_TEST_PATH_ENVS: &[&str] = &[
    "MISTER_SHARDED_CATALOG_DIR",
    "MISTER_LIBRARY_SQLITE",
    "MISTER_ARCADE_BOOTSTRAP_INDEX",
    "MISTER_LIBRARY_REFRESH_LOCK",
    "MISTER_CATALOG_BUILDER_LOCK",
    "MISTER_CATALOG_READY_SNAPSHOT",
    "MISTER_CATALOG_DIAGNOSTICS_DIR",
];

impl LauncherPresentBackend {
    fn from_env_values(backend: Option<&str>) -> Self {
        match backend {
            None | Some("") => Self::FpgaVblankLatchHidden,
            Some("fb0-dirty") => Self::Fb0Dirty,
            Some("fpga-vblank-latch-hidden") => Self::FpgaVblankLatchHidden,
            Some(retired) if is_retired_present_backend(retired) => {
                crate::ui_errln!(
                    "launcher_present_backend_retired value={retired}; using required latch backend"
                );
                boot_analytics::event(
                    "launcher_present_backend_retired",
                    format!("{retired} backend=fpga-vblank-latch-hidden"),
                );
                Self::FpgaVblankLatchHidden
            }
            Some(invalid) => {
                crate::ui_errln!(
                    "launcher_present_backend_invalid value={invalid}; using required latch backend"
                );
                Self::FpgaVblankLatchHidden
            }
        }
    }

    fn from_env() -> Self {
        Self::from_env_values(std::env::var("MISTER_PRESENT_BACKEND").ok().as_deref())
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

fn is_retired_present_backend(value: &str) -> bool {
    value == ["main", "flip-v1"].join("-")
        || value == ["main", "vsync-hidden"].join("-")
        || value == ["plugin", "main", "vsync-hidden"].join("-")
}

pub(super) fn launcher_present_backend() -> LauncherPresentBackend {
    static VALUE: OnceLock<LauncherPresentBackend> = OnceLock::new();
    *VALUE.get_or_init(|| {
        let backend = LauncherPresentBackend::from_env();
        backend.log_if_experimental();
        backend
    })
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

fn launcher_input_script_wait_frames() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(LAUNCHER_INPUT_SCRIPT_DEFAULT_WAIT_FRAMES)
            .min(600)
    })
}

struct ArcadeEntryLatencyTrace {
    writer: Option<std::io::BufWriter<std::fs::File>>,
    run_id: String,
}

impl ArcadeEntryLatencyTrace {
    fn from_env() -> Self {
        let run_id = std::env::var("MISTER_ARCADE_ENTRY_RUN_ID").unwrap_or_default();
        let writer = std::env::var("MISTER_ARCADE_ENTRY_TRACE")
            .ok()
            .and_then(|path| {
                let file = std::fs::File::create(&path)
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
        Self { writer, run_id }
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
    enter_presented: bool,
    rows_ready: bool,
    preview_exact: bool,
    first_nav_input_at: Option<Instant>,
    first_nav_presented: bool,
}

struct PendingCollectionEntry {
    collection_id: String,
    requested_at: Instant,
    source: launcher::HomeViewState,
}

struct PendingNavigationTransition {
    event: launcher::LauncherEvent,
    source_state: launcher::NavigationTransitionState,
    source_was_arcade: bool,
    committed: bool,
    status_quiesce_started_at: Option<Instant>,
}

const NAVIGATION_STATUS_QUIESCE_LIMIT: Duration = Duration::from_millis(50);

fn navigation_preview_snapshot_ready(
    preview_expected: bool,
    terminal_empty: bool,
    cache_state: &str,
    frame_status: PreviewRawFrameStatus,
) -> bool {
    !preview_expected
        || terminal_empty
        || (cache_state == "exact" && frame_status == PreviewRawFrameStatus::Ready)
}

fn should_clear_suppressed_preview(
    allow_preview_blit: bool,
    preserve_navigation_source_preview: bool,
) -> bool {
    !allow_preview_blit && !preserve_navigation_source_preview
}

fn should_defer_or_preserve_selected_preview(
    defer_selected_preview: bool,
    navigation_transition_active: bool,
    source_was_arcade: bool,
) -> bool {
    defer_selected_preview || (navigation_transition_active && source_was_arcade)
}

fn configure_arcade_list_renderer_geometry(
    renderer: &mut ArcadeListRenderer,
    nav: &LauncherNav,
    ui: &UiDisplay,
) {
    let (geometry, render_h) = arcade_list_layout(nav, ui);
    renderer.set_geometry_for_render_h(geometry, render_h);
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

fn settings_page_transition_direction(
    source: Screen,
    destination: Screen,
) -> Option<NavigationTransitionDirection> {
    let source_depth = settings_page_depth(source)?;
    let destination_depth = settings_page_depth(destination)?;
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
    (adjacent || direct_home).then_some(if destination_depth > source_depth {
        NavigationTransitionDirection::Forward
    } else {
        NavigationTransitionDirection::Reverse
    })
}

const fn settings_page_depth(screen: Screen) -> Option<u8> {
    match screen {
        Screen::Home => Some(0),
        Screen::Settings => Some(1),
        Screen::Screensaver | Screen::About => Some(2),
        Screen::Info | Screen::Licenses => Some(3),
        Screen::Controller | Screen::Arcade => None,
    }
}

fn settings_navigation_input_candidate(
    screen: Screen,
    now: &PadState,
    previous: &PadState,
) -> bool {
    let activated = now.btn_a && !previous.btn_a;
    let backed = now.btn_b && !previous.btn_b;
    let went_home = now.btn_home && !previous.btn_home;
    match screen {
        Screen::Home => activated || went_home,
        Screen::Settings
        | Screen::Screensaver
        | Screen::About
        | Screen::Info
        | Screen::Licenses => activated || backed || went_home,
        Screen::Controller | Screen::Arcade => false,
    }
}

fn absorb_exclusive_input(nav: &mut LauncherNav, now: &PadState) {
    nav.absorb_input(now);
}

fn route_lifecycle_dialog_input(
    nav: &mut LauncherNav,
    now: &PadState,
    previous: &PadState,
    launch_failure_visible: bool,
    recovery_dialog_visible: bool,
) -> Option<LauncherLifecycleInput> {
    let input = if launch_failure_visible {
        ((now.btn_a && !previous.btn_a)
            || (now.btn_b && !previous.btn_b)
            || (now.btn_home && !previous.btn_home))
            .then_some(LauncherLifecycleInput::LaunchFailureAcknowledge)
    } else if recovery_dialog_visible {
        if now.dpad_left && !previous.dpad_left {
            Some(LauncherLifecycleInput::CatalogRecoveryLeft)
        } else if now.dpad_right && !previous.dpad_right {
            Some(LauncherLifecycleInput::CatalogRecoveryRight)
        } else if now.btn_a && !previous.btn_a {
            Some(LauncherLifecycleInput::CatalogRecoveryConfirm)
        } else if (now.btn_b && !previous.btn_b) || (now.btn_home && !previous.btn_home) {
            Some(LauncherLifecycleInput::CatalogRecoveryCancel)
        } else {
            None
        }
    } else {
        None
    };
    if launch_failure_visible || recovery_dialog_visible {
        absorb_exclusive_input(nav, now);
    }
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

fn collection_has_resident_rows(catalog: &ArcadeCatalog, collection_id: &str) -> bool {
    catalog.system_game_count(collection_id) > 0
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
    now: &PadState,
    previous: &PadState,
    start: Instant,
) -> bool {
    if !((now.btn_b && !previous.btn_b) || (now.btn_home && !previous.btn_home)) {
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
    fn from_env() -> Self {
        Self {
            trace: ArcadeEntryLatencyTrace::from_env(),
            enter_input_at: None,
            enter_presented: false,
            rows_ready: false,
            preview_exact: false,
            first_nav_input_at: None,
            first_nav_presented: false,
        }
    }

    fn input_enabled_ms(lifecycle: &LauncherLifecycle) -> u64 {
        lifecycle.startup_status().input_enabled_ms
    }

    fn cancel_enter(&mut self) {
        self.enter_input_at = None;
        self.enter_presented = false;
        self.rows_ready = false;
        self.preview_exact = false;
        self.first_nav_input_at = None;
        self.first_nav_presented = false;
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
    ) {
        if self.enter_input_at.is_some() {
            return;
        }
        self.enter_input_at = Some(at);
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
            "source=open_collection_intent",
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
            format!("games={}", catalog.system_game_count(&system)),
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
        if (selected_has_preview && preview_state != "exact")
            || (!selected_has_preview && !matches!(preview_state, "exact" | "empty"))
        {
            return;
        }
        self.preview_exact = true;
        let system = Self::active_system_id(catalog, nav);
        let asset_key = Self::selected_asset_key(catalog, nav);
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
            "source=preview_state",
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
    RebuildReleaseRight,
    RebuildPressA,
    Done,
}

struct LibraryChangedDialogTestDriver {
    choice: Option<launcher::LibraryChangedTestDialogChoice>,
    dialog_seen_at: Option<Instant>,
    phase: LibraryChangedDialogTestPhase,
}

impl LibraryChangedDialogTestDriver {
    fn from_env(start: Instant) -> Self {
        let choice = library_changed_test_dialog_choice_from_env(start);
        Self {
            choice,
            dialog_seen_at: None,
            phase: LibraryChangedDialogTestPhase::Waiting,
        }
    }

    fn input_for(&mut self, nav: &LauncherNav, now: Instant, start: Instant) -> Option<PadState> {
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
            launcher::LibraryChangedTestDialogChoice::Continue => {
                if self.phase != LibraryChangedDialogTestPhase::Waiting {
                    return None;
                }
                self.phase = LibraryChangedDialogTestPhase::Done;
                print_startup_event(
                    start,
                    "library_changed_test_dialog_input",
                    "choice=continue button=a",
                );
                Some(pad_state_with(|state| state.btn_a = true))
            }
            launcher::LibraryChangedTestDialogChoice::Rebuild => match self.phase {
                LibraryChangedDialogTestPhase::Waiting => {
                    self.phase = LibraryChangedDialogTestPhase::RebuildReleaseRight;
                    print_startup_event(
                        start,
                        "library_changed_test_dialog_input",
                        "choice=rebuild button=right",
                    );
                    Some(pad_state_with(|state| state.dpad_right = true))
                }
                LibraryChangedDialogTestPhase::RebuildReleaseRight => {
                    self.phase = LibraryChangedDialogTestPhase::RebuildPressA;
                    Some(PadState::default())
                }
                LibraryChangedDialogTestPhase::RebuildPressA => {
                    self.phase = LibraryChangedDialogTestPhase::Done;
                    print_startup_event(
                        start,
                        "library_changed_test_dialog_input",
                        "choice=rebuild button=a",
                    );
                    Some(pad_state_with(|state| state.btn_a = true))
                }
                LibraryChangedDialogTestPhase::Done => None,
            },
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

    fn apply(self, state: &mut PadState) {
        match self {
            Self::Up => state.dpad_up = true,
            Self::Down => state.dpad_down = true,
            Self::Left => state.dpad_left = true,
            Self::Right => state.dpad_right = true,
            Self::A => state.btn_a = true,
            Self::B => state.btn_b = true,
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
}

impl LauncherInputScriptDriver {
    fn from_env(start: Instant) -> Self {
        match std::env::var("MISTER_LAUNCHER_INPUT_SCRIPT") {
            Ok(value) => Self::from_script(&value, start),
            Err(_) => Self::empty(),
        }
    }

    fn from_script(value: &str, start: Instant) -> Self {
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
            wait_frames: launcher_input_script_wait_frames(),
        }
    }

    fn empty() -> Self {
        Self {
            steps: Vec::new(),
            step_idx: 0,
            frame_in_step: 0,
            wait_frames: 0,
        }
    }

    fn input_for(&mut self) -> Option<PadState> {
        let step = *self.steps.get(self.step_idx)?;
        if self.frame_in_step < self.wait_frames {
            self.frame_in_step += 1;
            return None;
        }

        let local_frame = self.frame_in_step - self.wait_frames;
        self.frame_in_step += 1;
        if let LauncherInputScriptStep::Wait(frames) = step {
            if local_frame < frames {
                return Some(PadState::default());
            }
            self.step_idx += 1;
            self.frame_in_step = 0;
            return Some(PadState::default());
        }
        let LauncherInputScriptStep::Button(button) = step else {
            unreachable!();
        };
        if local_frame < LAUNCHER_INPUT_SCRIPT_PRESS_FRAMES {
            let mut state = PadState::default();
            button.apply(&mut state);
            return Some(state);
        }
        if local_frame < LAUNCHER_INPUT_SCRIPT_PRESS_FRAMES + LAUNCHER_INPUT_SCRIPT_RELEASE_FRAMES {
            return Some(PadState::default());
        }

        self.step_idx += 1;
        self.frame_in_step = 0;
        Some(PadState::default())
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
const HOME_LAYOUT_PADDING: usize = 18;
const HOME_HEADER_H: usize = 42;
const HOME_LAYOUT_SPACING: usize = 14;

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
    let y1 = ui.render_h().saturating_sub(HOME_LAYOUT_PADDING);
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

fn launcher_idle_sleep_duration(pacer: &VsyncPacer) -> Duration {
    let frame_period = Duration::from_micros(pacer.period_us().max(1));
    slint::platform::duration_until_next_timer_update()
        .map_or(frame_period, |timer| frame_period.min(timer))
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

fn latch_late_start_wait_enabled(latch_backend_active: bool, home_motion_active: bool) -> bool {
    !(latch_backend_active && home_motion_active)
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
    accepted_screensaver_frame: bool,
    status: LauncherPresentStatus,
    copy_path: &str,
) -> bool {
    copied_rows > 0
        || (accepted_screensaver_frame
            && status == LauncherPresentStatus::Ok
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

fn read_sharded_registry_seed(root: &str, start: Instant) -> Option<ShardedCatalogSeed> {
    let load_started = Instant::now();
    let storage = mister_magik_catalog::catalog_config::default_sharded_catalog_path();
    match load_sharded_registry_seed(root) {
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
    system_id: &str,
    priority: SystemShardPriority,
    reason: &'static str,
    now: Instant,
) -> bool {
    if !scheduler.request_system_shard(system_id.to_string(), priority, reason, now) {
        return false;
    }
    nav.catalog_system_hydration_started(system_id);
    true
}

fn retry_system_shard_hydration(
    scheduler: &mut LauncherScheduler,
    nav: &mut LauncherNav,
    system_id: &str,
    reason: &'static str,
    now: Instant,
) -> bool {
    if !scheduler.retry_system_shard(system_id.to_string(), reason, now) {
        return false;
    }
    nav.catalog_system_hydration_started(system_id);
    true
}

fn request_pending_launch_return_shard(
    pending: Option<&launcher::LaunchReturnState>,
    catalog: &ArcadeCatalog,
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
        system_id,
        SystemShardPriority::Urgent,
        "launch-return",
        now,
    ) {
        return false;
    }
    print_startup_event(
        start,
        "launch_return_system_shard_requested",
        format!("system={system_id} priority=urgent"),
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

fn modal_input_test_paths_are_isolated<'a>(paths: impl IntoIterator<Item = &'a str>) -> bool {
    let root = Path::new(MODAL_INPUT_TEST_ROOT);
    paths.into_iter().all(|path| {
        let path = Path::new(path);
        path != root && path.starts_with(root)
    })
}

fn modal_input_catalog_recovery_test_requested(start: Instant) -> bool {
    if std::env::var(MODAL_INPUT_TEST_ENV).as_deref() != Ok("upgrade") {
        return false;
    }
    let paths = MODAL_INPUT_TEST_PATH_ENVS
        .iter()
        .map(|name| std::env::var(name).ok())
        .collect::<Vec<_>>();
    let isolated = paths.iter().all(Option::is_some)
        && modal_input_test_paths_are_isolated(paths.iter().filter_map(Option::as_deref));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreensaverStartMode {
    Inactive,
    IdleWhenReady,
    PreviewWhenReady,
}

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

fn launcher_env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1" | "on" | "true" | "yes")
    )
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

fn apply_orientation_layout(
    app: &slint_ui::launcher::Launcher,
    window: &Rc<MisterSoftwareWindow>,
    ui: &UiDisplay,
    orientation: ScreenOrientation,
    nav: &mut LauncherNav,
    layout: &mut UiLayoutGeometry,
    portrait_target: &mut Option<UiFrameTarget>,
    navigation_transition: &mut NavigationTransitionRuntime,
) {
    nav.settings.screen_orientation = orientation;
    nav.sync_orientation_selection();
    *layout = UiLayoutGeometry::for_display(ui, orientation);
    nav.set_portrait_layout(layout.is_portrait());
    if layout.is_portrait() {
        let expected_len = layout.logical_w().saturating_mul(layout.logical_h());
        if portrait_target
            .as_ref()
            .is_none_or(|target| target.cached_565().len() != expected_len)
        {
            *portrait_target = Some(UiFrameTarget::cached(FramebufferTargetGeometry::new(
                layout.logical_w(),
                layout.logical_h(),
            )));
        }
    } else {
        *portrait_target = None;
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

fn preview_archive_warm_skip_enabled() -> bool {
    matches!(
        std::env::var("MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM")
            .ok()
            .as_deref(),
        Some("1") | Some("on") | Some("true") | Some("yes")
    )
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
    launch_return_cpu_profile: Option<cpu_profile::CpuProfiler>,
    mut layout: UiLayoutGeometry,
) {
    let start = Instant::now();
    let startup_monotonic_us = monotonic_clock_us().unwrap_or(0);
    let mut frames = 0u64;
    let screensaver_start_mode = screensaver_start_mode(
        launcher_env_flag("MISTER_SCREENSAVER_START_IDLE_WHEN_READY"),
        launcher_env_flag("MISTER_SCREENSAVER_START_PREVIEW_WHEN_READY"),
        launcher_env_flag("MISTER_SCREENSAVER_START_ACTIVE"),
    );
    let screensaver_preview_waits_for_analytics =
        launcher_env_flag("MISTER_SCREENSAVER_START_PREVIEW_AFTER_ANALYTICS");
    let mut screensaver = ScreensaverControl::new(Instant::now(), screensaver_start_mode);
    let mut screensaver_pipeline: Option<ScreensaverRenderAhead> = None;
    let mut retiring_screensaver_pipelines: Vec<ScreensaverRenderAhead> = Vec::new();
    let mut screensaver_loader: Option<LauncherScreensaverLoader> = None;
    let mut screensaver_launcher_frame: Option<Vec<Rgb565Pixel>> = None;
    let mut screensaver_frame_visible = false;
    let mut screensaver_active_cards = 0usize;
    let mut screensaver_archive_loading = false;
    let mut screensaver_has_rendered_card = false;
    let mut screensaver_render_sequence = 0u64;
    let mut screensaver_starvation_count = 0u64;
    let mut screensaver_superseded_frames = 0u64;
    let mut screensaver_reused_frames = 0u64;
    let mut screensaver_show_started: Option<Instant> = None;
    let mut screensaver_first_render_logged = false;
    let mut screensaver_first_present_logged = false;
    let mut screensaver_first_card_present_logged = false;
    let mut launcher_presenter = LauncherPresenter::new(ui);
    let mut launcher_readiness = super::launcher_readiness::LauncherReadiness::from_env();
    let launcher_bench_scenario = LauncherBenchScenario::from_env();
    let mut latch_v5_qualification = LatchV5Qualification::from_env(start);
    let mut latch_v5_bench_state = LauncherBenchState::default();
    let launcher_bench_after_input_script =
        launcher_bench_scenario.is_some() && launcher_bench_after_input_script_enabled();
    let launcher_bench_launch_handoff =
        launcher_bench_scenario == Some(LauncherBenchScenario::LaunchHandoff);
    let mut scheduler = LauncherScheduler::new(launcher_bench_launch_handoff);
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
        launcher_bench_scenario.is_some(),
        media_benchmark_contention,
    );
    let env_start_screen = launcher_start_screen_from_env();
    let env_start_system = launcher_start_system_from_env();
    let env_start_menu = launcher_bench_scenario
        .is_some()
        .then(launcher_start_menu_from_env)
        .flatten();
    let start_screen = latch_v5_qualification
        .enabled()
        .then_some(Screen::Arcade)
        .or(env_start_screen)
        .or_else(|| env_start_system.as_ref().map(|_| Screen::Arcade))
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade))
        .unwrap_or(Screen::Home);
    let lock_screen = launcher_lock_screen_from_env()
        .or_else(|| env_start_system.as_ref().map(|_| Screen::Arcade))
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade));
    let launch_return_restore_allowed = launcher_return_to_launcher_requested()
        && env_start_screen.is_none()
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
    let arcade_catalog_required_at_start = start_screen == Screen::Arcade
        || lock_screen == Some(Screen::Arcade)
        || launcher_bench_after_input_script;
    let mut pending_start_system = env_start_system.clone();
    let mut pending_start_menu = env_start_system
        .is_none()
        .then(|| env_start_menu.clone())
        .flatten();
    let crt_layout = ui.output_route().is_crt();
    let crt_metrics = crate::ui_display::CrtUiMetrics::for_display(ui);
    let preview_route = PreviewRoutePolicy::new(crt_layout);
    let mut nav =
        LauncherNav::for_crt_layout_with_row_height(crt_layout, crt_metrics.game_row_height);
    let settings_store = FileSettingsStore::new(
        mister_magik_catalog::device_layout::current_app_path("settings.json"),
    );
    nav.settings = settings_store.load();
    layout = UiLayoutGeometry::for_display(ui, nav.settings.screen_orientation);
    nav.set_portrait_layout(layout.is_portrait());
    nav.sync_orientation_selection();
    let mut portrait_target = layout.is_portrait().then(|| {
        UiFrameTarget::cached(FramebufferTargetGeometry::new(
            layout.logical_w(),
            layout.logical_h(),
        ))
    });
    let navigation_motion_enabled =
        !nav.settings.reduce_motion || cpu_profile::navigation_transition_profile_requested();
    let mut navigation_transition = NavigationTransitionRuntime::new(
        layout.logical_w(),
        layout.logical_h(),
        navigation_motion_enabled,
    );
    nav.screen = start_screen;
    let mut display_confirm_deadline = None;
    let mut orientation_confirm_deadline = None;
    let mut orientation_previous = None;
    let mut orientation_full_redraw_pending = layout.is_portrait();
    let (display_confirm_tx, display_confirm_rx) =
        mpsc::channel::<Result<launcher::DisplayCommandState, String>>();
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
                nav.display_highlighted = index;
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
    let mut loading_title = String::new();
    let mut last_clock_update = Instant::now() - Duration::from_secs(2);
    let mut last_clock_text = launcher_clock_text();
    let mut launcher_bench_next_step: Instant;
    let mut launcher_bench_state = LauncherBenchState::default();
    let mut launcher_bench_active =
        launcher_bench_scenario.is_some() && !launcher_bench_after_input_script;
    let auto_launch_selected = launcher_auto_launch_selected_enabled();
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
        if let Some(idx) = pad.index_needing_setup() {
            let status = pad.db().registry_status(pad.info_at(idx));
            crate::ui_errln!(
                "controller setup: pad {idx} needs setup ({status:?}) - showing prompt"
            );
            setup.open_for(status, idx);
        }
    }
    let mut pacer = ui
        .output_route()
        .nominal_period_us()
        .map(VsyncPacer::from_env_with_default_period)
        .unwrap_or_else(VsyncPacer::from_env);
    let pacing_policy = LauncherFramePacingPolicy::default();
    let mut phase_alignment = LauncherPhaseAlignment::default();
    let present_timing = PresentTiming::from_env();
    if preview_route.allows_preview_work()
        && launcher_bench_scenario.is_some()
        && !preview_archive_warm_skip_enabled()
    {
        let warm_t = Instant::now();
        match preview_worker::warm_preview_archives_from_env() {
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
    let mut preview = PreviewState::new_with_trace_start(start);
    let mut launcher_bench_waiting_for_initial_preview = launcher_bench_scenario
        .is_some_and(|scenario| scenario.starts_on_arcade() && !launcher_bench_after_input_script);
    let mut preview_transition = if preview_route.allows_preview_work() {
        PreviewTransitionDemo::from_env()
    } else {
        PreviewTransitionDemo::disabled()
    };
    let transition_picker_enabled = preview_transition.picker_enabled();
    let mut transition_picker_prev_left = false;
    let mut transition_picker_prev_right = false;
    let mut arcade_list_renderer = if crt_layout {
        ArcadeListRenderer::new_for_crt_display(crt_metrics, ui)
    } else {
        ArcadeListRenderer::new()
    };
    let mut launcher_preview_version = 1u64;
    let mut launcher_arcade_version = 1u64;
    let mut launcher_arcade_scroll_offset = 0i64;
    let mut arcade_drawer_view_cache = ArcadeDrawerViewCache::default();
    let mut composition = UiCompositionController::new();
    let mut cpu = launch_return_cpu_profile.or_else(cpu_profile::start);
    let mut screensaver_cpu_profile = cpu_profile::ScreensaverProfiler::from_env();
    let mut bridge_models = LauncherBridgeModels::default();
    let mut catalog_version = 0usize;
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    crate::ui_logln!(
        "preview_visual_pct={} preview_blitter=raw",
        preview_visual_pct()
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
    let mut catalog_publication_test = CatalogPublicationTestDriver::from_env(start);
    let mut media_session = ScreenshotMediaUpdateSession::default();
    let mut library_changed_dialog_test = LibraryChangedDialogTestDriver::from_env(start);
    let mut launcher_input_script = LauncherInputScriptDriver::from_env(start);
    let mut launcher_automation = LauncherAutomation::new();
    let mut catalog_recovery_prev = PadState::default();
    let sqlite_path = mister_magik_catalog::catalog_state::default_path();
    let capsule_seed_ready = catalog_ready;
    let sharded_seed = (!capsule_seed_ready)
        .then(|| read_sharded_registry_seed(&arcade_root, start))
        .flatten();
    let sharded_seed_ready = sharded_seed.is_some();
    let sharded_catalog_fingerprint = sharded_seed
        .as_ref()
        .map(|seed| seed.catalog_fingerprint.clone());
    if let Some(seed) = sharded_seed {
        catalog = seed.catalog;
        catalog_ready = true;
    }
    let retained_arcade_seed =
        (!capsule_seed_ready && !sharded_seed_ready).then(|| {
            let probe = mister_magik_catalog::builder_service::probe_retained_arcade_startup_seed(
                Path::new(&arcade_root),
            );
            match &probe {
                mister_magik_catalog::builder_service::RetainedArcadeStartupProbe::Ready(seed) => {
                    print_startup_event(
                        start,
                        "retained_arcade_startup_seed",
                        format!(
                            "status=ready games={} probe_us={} decode_us={} bytes={}",
                            seed.catalog.len(), seed.probe_us, seed.decode_us, seed.bytes
                        ),
                    );
                }
                mister_magik_catalog::builder_service::RetainedArcadeStartupProbe::Unavailable {
                    reason,
                    probe_us,
                } => {
                    print_startup_event(
                        start,
                        "retained_arcade_startup_seed",
                        format!("status=unavailable reason={reason} probe_us={probe_us}"),
                    );
                }
            }
            probe
        });
    let retained_arcade_seed = match retained_arcade_seed {
        Some(mister_magik_catalog::builder_service::RetainedArcadeStartupProbe::Ready(seed)) => {
            Some(seed)
        }
        _ => None,
    };
    let retained_arcade_seed_ready = retained_arcade_seed.is_some();
    let retained_arcade_fingerprint = retained_arcade_seed
        .as_ref()
        .map(|seed| seed.stamp_fingerprint.clone());
    if let Some(seed) = retained_arcade_seed {
        catalog = seed.catalog;
        catalog_ready = true;
    }
    let initial_catalog_fingerprint = return_capsule_fingerprint
        .or(sharded_catalog_fingerprint)
        .or(retained_arcade_fingerprint);
    let mut catalog_generation =
        initialize_catalog_generation(&mut scheduler, initial_catalog_fingerprint);
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
    } else if retained_arcade_seed_ready {
        startup_ready_catalog_source = CatalogSource::RetainedArcadeBootstrap;
        catalog_session.note_summary_seed_ready();
        if preview_route.allows_preview_work() {
            media_session.request_catalog_seed();
        }
        catalog_version = catalog_version.wrapping_add(1);
        print_startup_event(
            start,
            "catalog_worker_deferred",
            format!(
                "root={} request={} reason=retained_arcade_bootstrap",
                arcade_root,
                CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS.label()
            ),
        );
        // The retained projection is sufficient for immediate Arcade
        // navigation but is never the authoritative complete library.
        catalog_session.defer_catalog_worker(
            arcade_root.clone(),
            CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS,
            CatalogWorkerInitialCache::AlreadyProbedMissing,
            CatalogExecutionMode::BackgroundInteractive,
        );
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
    apply_home_selected_from_env(&mut nav, &catalog, start);
    let root_arcade_focused = nav.screen == Screen::Home
        && nav
            .current_menu_items()
            .get(nav.selected)
            .is_some_and(|item| item.id == arcade_catalog::MENU_ARCADE_SYSTEM_ID);
    let restored_arcade_collection = (nav.screen == Screen::Arcade)
        .then(|| nav.active_collection_scope_id(&catalog))
        .filter(|collection_id| !collection_id.is_empty());
    if catalog_ready
        && (root_arcade_focused || restored_arcade_collection.is_some())
        && preview_route.allows_preview_work()
    {
        let collection_id =
            restored_arcade_collection.unwrap_or(arcade_catalog::MENU_ARCADE_SYSTEM_ID);
        let games = catalog.system_game_view(collection_id);
        if !games.is_empty() {
            let selected = nav.arcade.selected.min(games.len() - 1);
            let _ = prewarm_arcade_selected_preview(games, selected, &mut preview);
        }
    }
    let bridge_systems_t = Instant::now();
    let mut arcade_screen_pending = (start_screen == Screen::Arcade
        || lock_screen == Some(Screen::Arcade))
        && !arcade_navigation_ready(catalog_ready, &catalog);
    bridge.set_menu_title(nav.current_menu_title().into());
    bridge.set_menu_breadcrumb(nav.current_menu_breadcrumb().into());
    bridge.set_update_available(false);
    bridge.set_menu_items(bridge_models.menu_items(&nav, catalog_version));
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
        Some(&catalog),
        &mut preview,
        &mut bridge_models,
        catalog_version,
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
    let mut modal_input_test_dialog_pending = modal_input_catalog_recovery_test_requested(start);
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
        && screensaver_start_mode == ScreensaverStartMode::Inactive;
    let mut startup_intro = if startup_intro_eligible
        && launcher_presenter.startup_intro_native_hidden_slots_available(ui)
    {
        match PreparedStartupIntro::new(ui) {
            Ok(prepared) => match launcher_presenter.take_direct_hidden_frame_buffers() {
                Ok(buffers) => {
                    print_startup_event(
                        start,
                        "startup_intro_started",
                        format!("width={} height={} fps=60", ui.fb_w(), ui.fb_h()),
                    );
                    Some(prepared.attach(buffers))
                }
                Err(failure) => {
                    launcher_presenter.fail_latch_completion(failure);
                    None
                }
            },
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
    let mut frame_accounting =
        LauncherFrameAccounting::new(run_start, ui.output_route().label(), ui.fb_w(), ui.fb_h());
    if let Some(failure) = launcher_presenter.latch_failure() {
        frame_accounting.record_latch_failure(failure);
    }
    if launcher_bench_after_input_script {
        // Activation below replaces accounting and opens the measured trace.
        frame_accounting.close_preview_scroll_trace_for_restart();
    }
    let mut arcade_entry_latency = ArcadeEntryLatencyTracker::from_env();
    let mut memory_guard = crate::memory_pressure::MemoryPressureGuard::from_env();
    let catalog_contention_quiet_previews = matches!(
        std::env::var("MISTER_CATALOG_CONTENTION_QUIET_PREVIEWS")
            .ok()
            .as_deref(),
        Some("1") | Some("on") | Some("true") | Some("yes")
    );
    let mut last_home_pan_scroll_x = nav.scroll_x;
    let mut home_pan_present_until = None;
    let mut navigation_source_bridge_sync_pending = false;
    while (secs == 0 || run_start.elapsed().as_secs() < secs)
        && preview_scroll_exit_at.is_none_or(|deadline| Instant::now() < deadline)
    {
        screensaver_cpu_profile.poll(frames);
        if catalog_publication_test.wait_for_first_frame_release(Instant::now(), start) {
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }
        let loop_start = Instant::now();
        let slint_timer_dispatch_started = Instant::now();
        let navigation_snapshot_locked_at_loop_start = navigation_transition.snapshot_locked();
        let startup_intro_needs_live_launcher = startup_intro_launcher_ui_plan(
            startup_intro.is_some(),
            lifecycle.startup_status().state,
            startup_intro_launcher_frame_ready,
        ) == StartupIntroLauncherUiPlan::PrepareLiveFrame;
        if !navigation_snapshot_locked_at_loop_start
            && (startup_intro.is_none() || startup_intro_needs_live_launcher)
        {
            slint::platform::update_timers_and_animations();
        }
        let slint_timer_dispatch_us = slint_timer_dispatch_started.elapsed().as_micros();
        let mut full_bridge_dirty = std::mem::take(&mut navigation_source_bridge_sync_pending)
            || std::mem::take(&mut modal_input_test_bridge_sync_pending);
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
                    apply_orientation_layout(
                        &app,
                        window,
                        ui,
                        previous,
                        &mut nav,
                        &mut layout,
                        &mut portrait_target,
                        &mut navigation_transition,
                    );
                }
                orientation_confirm_deadline = None;
                nav.confirm_action = None;
                nav.confirm_selected = 0;
                nav.orientation_confirm_remaining = 0;
                orientation_full_redraw_pending = true;
                full_bridge_dirty = true;
            }
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
                            nav.display_highlighted = index;
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
        let frame_analytics_mode = frame_accounting.frame_analytics_mode();
        let cpu_loop_start = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let arcade_visual_index_at_loop_start = nav.arcade.visual_index;
        let arcade_filter_visual_index_at_loop_start = nav.arcade_filter.visual_index;
        let prepare_trace_enabled =
            frame_accounting.preview_scroll_trace_enabled() || frame_analytics_mode.records_wall();
        let mut prepare_trace = LauncherPrepareTrace::default();
        prepare_trace.slint_timer_dispatch_us = slint_timer_dispatch_us;
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
        let mut effective_view =
            EffectiveLauncherView::resolve(&lifecycle, screensaver.active, nav.screen);
        let mut launching = effective_view.launch_active();
        let setup_active = setup.is_active();
        let mut light_bridge_dirty = false;
        let mut pad_changed_for_input =
            if effective_view.accepts_application_input() && lifecycle.startup_input_enabled() {
                Some(pad.poll_with_debug_labels(setup_active))
            } else {
                None
            };
        if let Some(sample) = memory_guard.tick(loop_start) {
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
        apply_screenshot_media_update_effects(
            media_session.clear_progress_if_due(loop_start),
            &app,
            &mut catalog,
            &mut scheduler,
            Some(&mut preview),
            &mut full_bridge_dirty,
            start,
        );
        launcher_readiness.poll();
        let mut route_action = display_session.begin_frame(frames, launching, f);
        route_action.force_full_present |= launcher_readiness.needs_full_present();
        // The catalog contention harness first proves one exact preview, then
        // freezes further selected-preview work so frame failures can be
        // attributed to the catalog rather than an independent image decode.
        let defer_selected_preview =
            catalog_contention_quiet_previews && preview.trace_cache_state() == "exact";
        let mut preview_scheduled_this_loop = false;
        let clock_update_due = last_clock_update.elapsed() >= Duration::from_secs(1);
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
        if let Some(available) = update_check.try_recv() {
            if available {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                bridge.set_update_available(true);
                light_bridge_dirty = true;
                runtime_status::event("update_available", "source=downloader_mister_magik");
            }
        }

        let catalog_worker_trace_start = prepare_trace_enabled.then(Instant::now);
        let slint_animation_active = app.window().has_active_animations();
        let startup_return_waiting_for_catalog = lifecycle.startup_waiting_for_return_catalog();
        // Post-reveal catalog work is already constrained to the CPU0
        // background role. Input, navigation, media, and preview activity must
        // never suspend it or catalog construction can starve indefinitely.
        mister_magik_catalog::builder_service::set_background_heavy_work_allowed(true);
        if !navigation_snapshot_locked_at_loop_start {
            scheduler.tick_catalog_progress(true, loop_start);
        }
        if !navigation_snapshot_locked_at_loop_start
            && let Some(request) = nav.take_arcade_search_request(&catalog, catalog_version)
        {
            scheduler.request_arcade_search(request);
        }
        if !navigation_snapshot_locked_at_loop_start && catalog_ready && nav.screen == Screen::Home
        {
            for (index, system_id) in nav.collection_prefetch_order().into_iter().enumerate() {
                if collection_has_resident_rows(&catalog, &system_id) {
                    continue;
                }
                let priority = if index == 0 {
                    SystemShardPriority::Selected
                } else {
                    SystemShardPriority::Prefetch
                };
                let _ = request_system_shard_hydration(
                    &mut scheduler,
                    &mut nav,
                    &system_id,
                    priority,
                    if index == 0 {
                        "home-highlight"
                    } else {
                        "home-neighbor"
                    },
                    loop_start,
                );
            }
        }
        let deferred_worker_policy = deferred_catalog_worker_start_policy(
            catalog_ready,
            frame_accounting.first_visible_copy_done(),
            startup_return_waiting_for_catalog,
            lifecycle.catalog_worker_start_delay(catalog_background_validation_delay()),
        );
        if !navigation_snapshot_locked_at_loop_start
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

        if !navigation_snapshot_locked_at_loop_start
            && let Some(message) = catalog_publication_test.tick(loop_start, start)
        {
            deferred_catalog_events.push_back(message);
        }
        if !navigation_snapshot_locked_at_loop_start
            && catalog_messages_need_polling(
                pending_catalog_ready.is_some(),
                catalog_session.refresh_done(),
                scheduler.catalog_messages_running() || !deferred_catalog_events.is_empty(),
            )
        {
            let catalog_disconnected = scheduler.poll_catalog(&mut catalog_events);
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
                        &mut prepare_trace,
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
                    &mut prepare_trace,
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
                    &mut prepare_trace,
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
        if preview_route.allows_preview_work() && !navigation_snapshot_locked_at_loop_start {
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
                        Some(&catalog),
                        &mut preview,
                        &mut bridge_models,
                        catalog_version,
                        false,
                        ui,
                    );
                    update_slint_animations(animation_clock);
                    let mut recovery_rect = None;
                    window.draw_if_needed(|renderer| {
                        let region = target.render(renderer, frame_target_geometry(ui));
                        recovery_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                    });
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
                    crate::ui_errln!("game launch failed: {error}");
                }
            }
        }

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
            let transition_frame = navigation_transition.tick(now_us);
            let should_commit = transition_frame.phase == NavigationTransitionPhase::Capture
                && pending_navigation_transition
                    .as_ref()
                    .is_some_and(|pending| !pending.committed);
            if should_commit {
                let navigation_commit_started = Instant::now();
                let event = pending_navigation_transition
                    .as_ref()
                    .map(|pending| pending.event.clone())
                    .expect("checked pending transition");
                let before = LauncherBridgeKey::from_nav(&nav);
                let committed = if event.action == LauncherAction::OpenCollection
                    && pending_collection_entry.is_some()
                {
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

        latch_v5_qualification.poll_control(loop_start);
        latch_v5_qualification.observe_catalog_worker(
            scheduler.catalog_worker_running(),
            catalog_session.refresh_done(),
        );
        if latch_v5_qualification.take_catalog_request(scheduler.catalog_worker_running()) {
            let effects = catalog_session.qualification_fresh_rebuild(arcade_root.clone());
            apply_catalog_session_effects(
                effects,
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
                    ui.fb_w(),
                    ui.fb_h(),
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

        let pad_changed = pad_changed_for_input
            .take()
            .unwrap_or_else(|| pad.poll_with_debug_labels(setup_active));
        let frame_now = Instant::now();
        let launcher_state = launcher_automation.poll_input(
            ControllerSetupInputSession::new(&pad, &setup).launcher_state(),
            effective_view.accepts_application_input() && lifecycle.startup_input_enabled(),
            setup.is_active(),
            frame_now,
        );
        frame_accounting.set_automation_action_sequence(launcher_automation.action_sequence());

        if effective_view.accepts_application_input() && lifecycle.startup_input_enabled() {
            if setup_active && setup.target_pad_idx >= pad.len() {
                crate::ui_errln!(
                    "controller setup: pad {} disappeared; closing setup flow",
                    setup.target_pad_idx
                );
                setup.advance_to_next_pad(&pad);
                full_bridge_dirty = true;
            }

            let raw_screensaver_input_activity =
                pad.user_activity() || launcher_automation.active();
            if screensaver.handle_input(
                frame_now,
                pad_state_has_active_input(&launcher_state),
                raw_screensaver_input_activity,
            ) {
                nav.absorb_input(&launcher_state);
                request_launcher_redraw!();
                continue;
            }
            let setup_state = if launcher_automation.active() {
                launcher_state.clone()
            } else {
                ControllerSetupInputSession::new(&pad, &setup).setup_state()
            };
            let active_idx = pad.active_idx();
            let info = pad.info();

            if launcher_bench_scenario.is_none() && setup.is_active() {
                let setup_before = SetupBridgeKey::from_setup(&setup);
                let setup_info = pad.info_at(setup.target_pad_idx);
                match setup.handle_input(&setup_state, frame_now, setup_info, pad.db()) {
                    SetupAction::None => {}
                    SetupAction::RegisterNew => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.register_new_at(idx) {
                            crate::ui_errln!("controller setup: register new: {e}");
                        }
                    }
                    SetupAction::ClaimExisting { list_index } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.claim_existing_at(idx, list_index) {
                            crate::ui_errln!("controller setup: claim existing: {e}");
                        }
                    }
                    SetupAction::SaveFinish { label, kind } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.finish_setup_at(idx, label, kind) {
                            crate::ui_errln!("controller setup: save: {e}");
                        } else {
                            crate::ui_errln!(
                                "controller setup: saved \"{}\" ({})",
                                pad.db().display_label(pad.info_at(idx)),
                                kind.as_str()
                            );
                        }
                        setup.advance_to_next_pad(&pad);
                    }
                    SetupAction::Done => {
                        setup.advance_to_next_pad(&pad);
                    }
                }
                absorb_exclusive_input(&mut nav, &launcher_state);
                let setup_after = SetupBridgeKey::from_setup(&setup);
                full_bridge_dirty |= pad_changed || setup_before != setup_after;
            } else if launcher_bench_scenario.is_none()
                || launcher_bench_launch_handoff
                || (launcher_bench_after_input_script && !launcher_bench_active)
            {
                if AUTO_CONTROLLER_SETUP_ENABLED && pad_changed {
                    let setup_before = SetupBridgeKey::from_setup(&setup);
                    setup.maybe_open(info, active_idx, pad.db(), true);
                    full_bridge_dirty |= setup_before != SetupBridgeKey::from_setup(&setup);
                }
                if !setup.is_active() {
                    let nav_before = LauncherBridgeKey::from_nav(&nav);
                    let arcade_selected_before_input = nav.arcade.selected;
                    if transition_picker_enabled && nav.screen == Screen::Arcade {
                        let left = launcher_state.dpad_left && !transition_picker_prev_left;
                        let right = launcher_state.dpad_right && !transition_picker_prev_right;
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
                    transition_picker_prev_left = launcher_state.dpad_left;
                    transition_picker_prev_right = launcher_state.dpad_right;
                    let mut physical_nav_state = launcher_state.clone();
                    if let Some(test_state) =
                        library_changed_dialog_test.input_for(&nav, loop_start, start)
                    {
                        physical_nav_state = test_state;
                    }
                    if let Some(script_state) = launcher_input_script.input_for() {
                        physical_nav_state = script_state;
                    }
                    let lifecycle_view = lifecycle.view();
                    let launch_failure_visible = lifecycle_view.launch_failure_dialog().is_some();
                    let recovery_dialog_visible =
                        lifecycle_view.catalog_recovery_dialog().is_some();
                    let physical_previous =
                        std::mem::replace(&mut catalog_recovery_prev, physical_nav_state.clone());
                    let pending_collection_cancelled = cancel_pending_collection_entry_for_input(
                        &mut pending_collection_entry,
                        &mut nav,
                        &physical_nav_state,
                        &physical_previous,
                        start,
                    );
                    if pending_collection_cancelled {
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
                    let routed_input = navigation_transition.route_input(
                        &physical_nav_state,
                        &physical_previous,
                        pending_collection_cancelled,
                    );
                    if routed_input.is_none() {
                        nav.absorb_input(&physical_nav_state);
                    }
                    let input_previous = routed_input
                        .as_ref()
                        .map_or(&physical_previous, |input| &input.previous);
                    let nav_state = routed_input
                        .as_ref()
                        .map_or(&physical_nav_state, |input| &input.now);
                    if routed_input.as_ref().is_some_and(|input| input.replayed) {
                        nav.absorb_input(input_previous);
                    }
                    let settings_transition_source = (!launch_failure_visible
                        && !recovery_dialog_visible
                        && !navigation_transition.is_active()
                        && navigation_transition.enabled()
                        && settings_navigation_input_candidate(
                            nav.screen,
                            nav_state,
                            input_previous,
                        ))
                    .then(|| (nav.screen, nav.navigation_transition_state()));
                    let event = if navigation_transition.is_active() {
                        None
                    } else if launch_failure_visible || recovery_dialog_visible {
                        if let Some(input) = route_lifecycle_dialog_input(
                            &mut nav,
                            nav_state,
                            input_previous,
                            launch_failure_visible,
                            recovery_dialog_visible,
                        ) {
                            lifecycle.handle(input, &mut lifecycle_effects);
                            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                            full_bridge_dirty = true;
                        }
                        None
                    } else if scheduler.should_request_benchmark_launch()
                        && catalog_ready
                        && !launcher_bench_waiting_for_initial_preview
                        && nav.screen == Screen::Arcade
                    {
                        active_system(&catalog, &nav)
                            .and_then(|system| {
                                nav.active_arcade_game_at(&catalog, &system.id, nav.arcade.selected)
                            })
                            .map(|game| launcher::LauncherEvent {
                                action: LauncherAction::LaunchGame,
                                path: Some(game.mra_path.to_string()),
                                settings: None,
                            })
                    } else if auto_launch_selected
                        && !auto_launch_selected_done
                        && launcher_auto_launch_gate_ready()
                        && catalog_ready
                        && nav.screen == Screen::Arcade
                    {
                        let event = active_system(&catalog, &nav)
                            .and_then(|system| {
                                nav.active_arcade_game_at(&catalog, &system.id, nav.arcade.selected)
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
                    } else if navigation_transition.enabled() {
                        nav.handle_input_with_navigation_intents(nav_state, frame_now, &catalog)
                    } else {
                        nav.handle_input_with_collection_intents(nav_state, frame_now, &catalog)
                    };
                    if let Some((source_screen, source_state)) = settings_transition_source
                        && let Some(direction) =
                            settings_page_transition_direction(source_screen, nav.screen)
                    {
                        let now_us = frame_now
                            .saturating_duration_since(start)
                            .as_micros()
                            .min(u64::MAX as u128) as u64;
                        if navigation_transition
                            .begin_settings_page(direction, target.cached_565(), now_us)
                            .unwrap_or(false)
                        {
                            pending_navigation_transition = Some(PendingNavigationTransition {
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
                            full_bridge_dirty = true;
                            request_launcher_redraw!();
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
                                    && !collection_has_resident_rows(&catalog, collection_id)
                                {
                                    let requested_at = Instant::now();
                                    let hydration_failed =
                                        nav.catalog_system_hydration_has_failed(collection_id);
                                    let hydration_requested = if hydration_failed {
                                        let accepted = retry_system_shard_hydration(
                                            &mut scheduler,
                                            &mut nav,
                                            collection_id,
                                            "explicit-retry",
                                            requested_at,
                                        );
                                        if accepted {
                                            catalog_version = catalog_version.wrapping_add(1);
                                            full_bridge_dirty = true;
                                        }
                                        accepted
                                    } else {
                                        request_system_shard_hydration(
                                            &mut scheduler,
                                            &mut nav,
                                            collection_id,
                                            SystemShardPriority::Urgent,
                                            "open-collection",
                                            requested_at,
                                        )
                                    };
                                    if hydration_requested
                                        || nav.catalog_system_hydration_is_loading(collection_id)
                                    {
                                        arcade_entry_latency.record_collection_enter_input(
                                            start,
                                            requested_at,
                                            &lifecycle,
                                            collection_id,
                                        );
                                        pending_collection_entry = Some(PendingCollectionEntry {
                                            collection_id: collection_id.to_string(),
                                            requested_at,
                                            source: nav.home_view_state(),
                                        });
                                        print_startup_event(
                                            start,
                                            "catalog_system_entry_pending",
                                            format!("system={collection_id}"),
                                        );
                                    }
                                }

                                let collection_navigation_ready =
                                    collection_id.as_deref().map_or(true, |collection_id| {
                                        collection_has_resident_rows(&catalog, collection_id)
                                            || pending_collection_entry.as_ref().is_some_and(
                                                |entry| entry.collection_id == collection_id,
                                            )
                                    });
                                let transition_spec = collection_navigation_ready
                                    .then(|| navigation_transition_for_intent(&nav, &event))
                                    .flatten();
                                if transition_spec.is_some()
                                    && nav.screen == Screen::Arcade
                                    && !crt_layout
                                {
                                    arcade_list_renderer.compose_layer_to_cached(target, true);
                                    let _ =
                                        target.compose_direct_preview_rect(preview_screen_rect(ui));
                                }
                                let transition_started =
                                    transition_spec.is_some_and(|(edge, direction)| {
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
                                                    let content = ui.content_rect();
                                                    crt_navigation_geometry(
                                                        ui.render_w(),
                                                        ui.render_h(),
                                                        CrtNavigationLayout {
                                                            content_x: content.x,
                                                            content_y: content.y,
                                                            content_width: content.width,
                                                            content_height: content.height,
                                                            grid_x: crt_metrics.grid_x.max(1)
                                                                as usize,
                                                            grid_y: crt_metrics.grid_y.max(1)
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
                                                        ui.render_w(),
                                                        ui.render_h(),
                                                        nav.selected,
                                                        nav.scroll_x,
                                                        root_menu,
                                                        edge,
                                                        selected_label,
                                                    )
                                                })
                                            }
                                            NavigationTransitionDirection::Reverse => {
                                                navigation_transition.geometry_for_reverse(edge)
                                            }
                                        };
                                        geometry.is_some_and(|geometry| {
                                            navigation_transition
                                                .begin(
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
                                                .unwrap_or(false)
                                        })
                                    });
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
                                    full_bridge_dirty = true;
                                    request_launcher_redraw!();
                                } else if collection_id.is_none()
                                    || collection_id.as_deref().is_some_and(|collection_id| {
                                        collection_has_resident_rows(&catalog, collection_id)
                                    })
                                {
                                    if nav.commit_navigation_intent(&event, &catalog) {
                                        if let Some(collection_id) = collection_id.as_deref() {
                                            print_startup_event(
                                                start,
                                                "catalog_system_entry_immediate",
                                                format!(
                                                    "system={collection_id} resident_rows={}",
                                                    catalog.system_game_count(collection_id)
                                                ),
                                            );
                                        }
                                        full_bridge_dirty = true;
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
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                    ui,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, frame_target_geometry(ui));
                                    let _ = region;
                                });
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
                                let effects = catalog_session.rebuild_database(arcade_root.clone());
                                apply_catalog_session_effects(
                                    effects,
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
                                    false,
                                    loop_start,
                                    start,
                                );
                                request_launcher_redraw!();
                                continue;
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
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                    ui,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, frame_target_geometry(ui));
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                copy_cached_rows_565(
                                    disp,
                                    target.cached_frame_view(),
                                    0,
                                    ui.render_h(),
                                );
                                std::thread::sleep(Duration::from_millis(250));
                                match launcher::reboot_mister() {
                                    Ok(()) => continue,
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
                                    false,
                                    loop_start,
                                    start,
                                );
                                request_launcher_redraw!();
                                continue;
                            }
                            LauncherAction::RebuildLibrary => {
                                let effects = catalog_session.rebuild_library(arcade_root.clone());
                                apply_catalog_session_effects(
                                    effects,
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
                                    false,
                                    loop_start,
                                    start,
                                );
                                request_launcher_redraw!();
                                continue;
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
                                        nav.confirm_action =
                                            Some(launcher::ConfirmAction::DisplayResolutionError);
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
                                    let result = launcher::confirm_display_resolution_and_wait(
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
                                    nav.confirm_action =
                                        Some(launcher::ConfirmAction::DisplayResolutionError);
                                    nav.confirm_selected = 0;
                                }
                            }
                            LauncherAction::ApplyScreenOrientation => {
                                if let Some(orientation) =
                                    event.path.as_deref().and_then(ScreenOrientation::parse)
                                    && orientation != nav.settings.screen_orientation
                                {
                                    orientation_previous = Some(nav.settings.screen_orientation);
                                    apply_orientation_layout(
                                        &app,
                                        window,
                                        ui,
                                        orientation,
                                        &mut nav,
                                        &mut layout,
                                        &mut portrait_target,
                                        &mut navigation_transition,
                                    );
                                    nav.confirm_action =
                                        Some(launcher::ConfirmAction::ScreenOrientation);
                                    nav.confirm_selected = 0;
                                    nav.orientation_confirm_remaining =
                                        launcher::DISPLAY_CONFIRM_SECONDS;
                                    orientation_confirm_deadline = Some(
                                        Instant::now()
                                            + Duration::from_secs(u64::from(
                                                launcher::DISPLAY_CONFIRM_SECONDS,
                                            )),
                                    );
                                    orientation_full_redraw_pending = true;
                                    full_bridge_dirty = true;
                                }
                            }
                            LauncherAction::ConfirmScreenOrientation => {
                                orientation_confirm_deadline = None;
                                orientation_previous = None;
                                nav.orientation_confirm_remaining = 0;
                                if let Err(error) = settings_store.save(&nav.settings) {
                                    crate::ui_errln!(
                                        "settings: failed to save screen orientation: {error}"
                                    );
                                }
                            }
                            LauncherAction::CancelScreenOrientation => {
                                if let Some(previous) = orientation_previous.take() {
                                    apply_orientation_layout(
                                        &app,
                                        window,
                                        ui,
                                        previous,
                                        &mut nav,
                                        &mut layout,
                                        &mut portrait_target,
                                        &mut navigation_transition,
                                    );
                                }
                                orientation_confirm_deadline = None;
                                nav.orientation_confirm_remaining = 0;
                                orientation_full_redraw_pending = true;
                                full_bridge_dirty = true;
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
                                continue;
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
                                        detail: "launch scheduler rejected request".to_string(),
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
                            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                            sync_bridge_launcher(
                                &app,
                                &pad,
                                &nav,
                                &lifecycle,
                                &setup,
                                scheduler.launch_loading_title(),
                                "",
                                Some(&catalog),
                                &mut preview,
                                &mut bridge_models,
                                catalog_version,
                                false,
                                ui,
                            );
                            window.request_redraw();
                            update_slint_animations(animation_clock);
                            window.draw_if_needed(|renderer| {
                                let region = target.render(renderer, frame_target_geometry(ui));
                                let _ = region;
                            });
                            let _pace = pacer.wait();
                            copy_cached_rows_565(
                                disp,
                                target.cached_frame_view(),
                                0,
                                ui.render_h(),
                            );
                            let loading_presented = Instant::now();
                            lifecycle
                                .loading_frame_presented(loading_presented, &mut lifecycle_effects);
                            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                            request_launcher_redraw!();
                        }
                    }
                    let nav_after = LauncherBridgeKey::from_nav(&nav);
                    if nav_before != nav_after {
                        if let Some(entry) = pending_collection_entry.take() {
                            nav.catalog_system_hydration_finished(&entry.collection_id);
                            print_startup_event(
                                start,
                                "catalog_system_entry_cancelled",
                                format!("system={} reason=navigation-changed", entry.collection_id),
                            );
                        }
                        media_session.note_nav_change(&nav_before, &nav_after, Instant::now());
                    }
                    if pad_changed && nav.screen == Screen::Controller {
                        full_bridge_dirty = true;
                    } else if pad_changed && !dirty_opt {
                        full_bridge_dirty = true;
                    }
                    if nav_before != nav_after {
                        if nav_before.screen == Screen::Home && nav_after.screen == Screen::Arcade {
                            arcade_entry_latency
                                .record_enter_input(start, frame_now, &lifecycle, &catalog, &nav);
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
                            full_bridge_dirty = true;
                        } else {
                            light_bridge_dirty = true;
                        }
                    }
                }
            }

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
                            if let Err(e) = display_session.recover_after_launch_failure(frames, f)
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

        if empty_collection_invariant_violated(&catalog, &nav) {
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
        match bridge_sync_plan {
            LauncherBridgeSyncPlan::Full => {
                sync_bridge_launcher(
                    &app,
                    &pad,
                    &nav,
                    &lifecycle,
                    &setup,
                    scheduler.visible_loading_title(&loading_title),
                    "",
                    Some(&catalog),
                    &mut preview,
                    &mut bridge_models,
                    catalog_version,
                    defer_or_preserve_selected_preview,
                    ui,
                );
                preview_scheduled_this_loop = nav.screen == Screen::Arcade;
                request_launcher_redraw!();
            }
            LauncherBridgeSyncPlan::Light => {
                let active_games = if nav.screen == Screen::Arcade {
                    Some(active_system_game_view(&catalog, &nav))
                } else {
                    None
                };
                sync_bridge_launcher_light(
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
                    ui,
                );
                preview_scheduled_this_loop = nav.screen == Screen::Arcade;
                request_launcher_redraw!();
            }
            LauncherBridgeSyncPlan::None => {}
        }
        prepare_trace.bridge_sync_us = bridge_sync_started
            .map(|started| started.elapsed().as_micros())
            .unwrap_or(0);
        if !startup_intro_suppress_launcher_ui {
            sync_startup_visibility(&app, &lifecycle);
        }

        let media_gate_trace_start = prepare_trace_enabled.then(Instant::now);
        if !navigation_snapshot_locked_at_loop_start {
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
        let confirm_visible = bridge.get_confirm_visible();
        let confirm_selected = bridge.get_confirm_selected();
        let status_write_due = frame_accounting.status_write_due();
        let status_snapshot_due = status_write_due && !navigation_transition.is_active();
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
        let preview_schedule_trace_start = prepare_trace_enabled.then(Instant::now);
        if dirty_opt
            && !navigation_snapshot_locked_at_loop_start
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
                nav.arcade.is_scroll_active(),
                nav.arcade.is_turbo_active(),
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
            && !navigation_snapshot_locked_at_loop_start
            && !arcade_search_active
            && !memory_guard.active()
            && preview_route.allows_preview_work()
        {
            let dirty = apply_ready_preview(
                &app,
                &mut preview,
                defer_or_preserve_selected_preview,
                nav.screen == Screen::Arcade && nav.arcade.is_turbo_active(),
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
            || display_session.should_present_full_frame(launching, route_action)
            || startup_reveal_ready;
        let wants_arcade_list = !screensaver.active
            && should_draw_arcade_overlay(&nav, launching, active_arcade_games_available);
        let preview_presentation_state = preview.presentation_state();
        let wants_preview = preview_route.allows_preview_work()
            && !screensaver.active
            && direct_preview_requested(
                nav.screen,
                memory_guard.active(),
                preview_presentation_state.owns_direct_layer(),
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
            let _ = navigation_transition.complete();
            if endpoint == Some(NavigationTransitionEndpoint::Source)
                && let Some(entry) = pending_collection_entry.take()
            {
                deferred_navigation_hydration_finish = Some(entry.collection_id);
                arcade_entry_latency.cancel_enter();
            }
            pending_navigation_transition = None;
        }
        let navigation_destination_committed = pending_navigation_transition
            .as_ref()
            .is_some_and(|pending| pending.committed);
        let navigation_destination_layers_ready = navigation_destination_committed
            && (nav.screen != Screen::Arcade
                || navigation_preview_snapshot_ready(
                    selected_arcade_game_has_preview(&nav, &catalog),
                    preview.terminal_empty(),
                    preview_cache_state_before_composition,
                    preview_frame_status,
                ));
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
            wants_preview,
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
        if composition_decision.force_full_slint_present {
            full_frame_present = true;
        }
        if composition_decision.force_full_slint_raster {
            request_launcher_redraw!();
        }
        if composition_decision.clear_direct_layers {
            arcade_list_renderer.invalidate_presented_layer();
            if should_clear_suppressed_preview(
                composition_decision.allow_preview_blit,
                preserve_navigation_source_preview,
            ) {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                preview.clear(&bridge);
            }
            request_launcher_redraw!();
        }
        let startup_status = lifecycle.startup_status();
        let composition_status = composition_decision.status();
        let automation_frame_stamp = if launcher_automation.active() {
            let selected_system_id = nav.active_collection_scope_id(&catalog);
            let selected_game = (nav.screen == Screen::Arcade)
                .then(|| {
                    nav.active_arcade_game_at(&catalog, selected_system_id, nav.arcade.selected)
                })
                .flatten();
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            launcher_automation.observe_state(AutomationSemanticState {
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
        wake_reasons.insert_if(LauncherWakeReasons::PREVIEW_DIRTY, preview.raw_dirty());
        wake_reasons.insert_if(
            LauncherWakeReasons::PREVIEW_SCHEDULED_THIS_LOOP,
            preview_scheduled_this_loop,
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
        if render_intent.can_sleep() {
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
            std::thread::sleep(launcher_idle_sleep_duration(&pacer));
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
        let late_frame_start_headroom_us = if latch_backend_active {
            phase_alignment.required_headroom_us()
        } else {
            FB0_LATE_FRAME_START_HEADROOM_US
        };
        let wait_before_render =
            latch_late_start_wait_enabled(latch_backend_active, home_motion_active)
                && pacing_policy
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
        let pre_render_pace = wait_before_render.then(|| {
            let wait_start = Instant::now();
            let pace = pacer.wait();
            let wait_done = Instant::now();
            (
                pace,
                wait_done,
                wait_done.saturating_duration_since(wait_start).as_micros(),
            )
        });
        let pre_render_wait_us = pre_render_pace
            .as_ref()
            .map(|(_, _, wait_us)| *wait_us)
            .unwrap_or(0);
        let navigation_snapshot_locked_before_render = navigation_transition.snapshot_locked();
        if !navigation_snapshot_locked_before_render {
            update_slint_animations(animation_clock);
        }
        let mut layer_target =
            LayerTarget::new_oriented(target, portrait_target.as_mut(), ui, layout);
        let cpu_t1 = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let frame_t1 = Instant::now();
        retiring_screensaver_pipelines.retain_mut(|pipeline| !pipeline.poll_stopped());
        if screensaver.take_restore_full_frame() {
            if let Some(mut snapshot) = screensaver_launcher_frame.take() {
                if !layer_target.swap_cached(&mut snapshot) {
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
            screensaver_archive_loading = false;
            screensaver_has_rendered_card = false;
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
                    layout.logical_w(),
                    layout.logical_h(),
                    screensaver_show_started,
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
                screensaver_superseded_frames = 0;
                screensaver_reused_frames = 0;
                screensaver_loader = None;
            }
        }
        if let Some(pipeline) = screensaver_pipeline.as_ref() {
            pipeline.update_period_us(pacer.period_us());
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
            screensaver_archive_loading = false;
            screensaver_has_rendered_card = false;
        }
        let screensaver_fade_alpha = screensaver.preview_fade_alpha(Instant::now());
        let mut screensaver_frame_trace = ScreensaverFrameTrace::default();
        let mut accepted_screensaver_frame = false;
        let mut screensaver_buffer_to_recycle_after_present = None;
        let mut completed_hidden_frame_for_present = None;
        let mut accepted_startup_intro_frame = false;
        let mut startup_intro_failure = None;
        if let Some(intro) = startup_intro.as_mut() {
            if intro.snapshot_capture_needed() && startup_intro_launcher_frame_ready {
                let launcher_pixels = layer_target.cached_frame_view().pixels();
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
                match launcher_presenter
                    .try_issue_startup_intro_hidden_slot_render_grant(f, display_session)
                {
                    Ok(Some(grant)) => match intro.render_grant(grant) {
                        Ok(completed) => {
                            completed_hidden_frame_for_present = Some(completed);
                            accepted_startup_intro_frame = true;
                        }
                        Err(error) => startup_intro_failure = Some(error),
                    },
                    Ok(None) => {}
                    Err(failure) => {
                        launcher_presenter.fail_latch_completion(failure);
                        startup_intro_failure = Some("hidden-slot grant failed".into());
                    }
                }
            }
        }
        if let Some(error) = startup_intro_failure.take() {
            crate::ui_errln!("startup intro stopped: {error}");
            if let Some(mut intro) = startup_intro.take() {
                let returned = intro.take_buffers();
                if let Err(failure) =
                    launcher_presenter.restore_direct_hidden_frame_buffers(returned)
                {
                    launcher_presenter.fail_latch_completion(failure);
                }
            }
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
                    if layer_target.swap_cached(&mut pixels) {
                        retain_or_defer_screensaver_buffer(
                            &mut screensaver_launcher_frame,
                            &mut screensaver_buffer_to_recycle_after_present,
                            pixels,
                        );
                        screensaver_frame_trace = frame.trace;
                        screensaver_render_sequence = frame.sequence;
                        screensaver_superseded_frames = frame.superseded_frames;
                        screensaver_frame_trace.render_ahead_sequence = frame.sequence;
                        screensaver_frame_trace.render_ahead_queue_depth = screensaver_pipeline
                            .as_ref()
                            .map(ScreensaverRenderAhead::ready_depth)
                            .unwrap_or(0);
                        screensaver_frame_trace.render_ahead_frame_age_us = frame
                            .completed_at
                            .elapsed()
                            .as_micros()
                            .try_into()
                            .unwrap_or(u64::MAX);
                        screensaver_frame_trace.render_ahead_render_wall_us = frame.render_wall_us;
                        screensaver_frame_trace.render_ahead_render_cpu_us = frame.render_cpu_us;
                        screensaver_active_cards = frame.active_cards;
                        screensaver_archive_loading = frame.archive_loading;
                        screensaver_has_rendered_card = frame.has_rendered_card;
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
                    screensaver_archive_loading = false;
                    screensaver_has_rendered_card = false;
                    window.request_redraw();
                    full_frame_present = true;
                }
                RenderAheadPoll::Disconnected => {
                    crate::ui_errln!(
                        "screensaver: render-ahead pipeline disconnected; suppressing reactivation until fresh user activity"
                    );
                    screensaver.fail_current_activation(Instant::now());
                    if let Some(mut snapshot) = screensaver_launcher_frame.take()
                        && !layer_target.swap_cached(&mut snapshot)
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
                    screensaver_archive_loading = false;
                    screensaver_has_rendered_card = false;
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
        screensaver_frame_trace.render_ahead_sequence = screensaver_render_sequence;
        screensaver_frame_trace.render_ahead_queue_depth = screensaver_pipeline
            .as_ref()
            .map(ScreensaverRenderAhead::ready_depth)
            .unwrap_or(0);
        screensaver_frame_trace.render_ahead_starvation_count = screensaver_starvation_count;
        screensaver_frame_trace.render_ahead_superseded_frames = screensaver_superseded_frames;
        screensaver_frame_trace.render_ahead_reused_frames = screensaver_reused_frames;
        screensaver_frame_trace.render_ahead_cancelled =
            screensaver_pipeline.is_none() && !retiring_screensaver_pipelines.is_empty();
        let mut slint_base_rendered = false;
        let mut slint_damage = DirtyRectList::new();
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
                        x1: layout.logical_w(),
                        y1: layout.logical_h(),
                    })
                }
            } else {
                None
            }
        } else if screensaver.active {
            None
        } else if startup_intro_suppress_launcher_ui {
            None
        } else if navigation_snapshot_locked_before_render {
            None
        } else if composition_decision.force_full_slint_raster {
            let (dirty, damage, rendered) = layer_target.render_slint_full(&window);
            slint_damage = damage;
            slint_base_rendered = rendered;
            dirty
        } else if startup_intro_prepare_live_launcher {
            let (dirty, damage) = layer_target.render_slint_base(&window);
            slint_damage = damage;
            dirty
        } else {
            let (dirty, damage) = layer_target.render_slint_base(&window);
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
        let arcade_list_update_start = Instant::now();
        let arcade_list_rect = if wants_arcade_list && composition_decision.allow_arcade_list_blit {
            configure_arcade_list_renderer_geometry(&mut arcade_list_renderer, &nav, ui);
            let force_arcade_redraw = arcade_list_needs_forced_redraw(
                &arcade_list_renderer,
                this_rect,
                full_frame_present,
            );
            if nav.arcade_filter.drawer_open {
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
            }
        } else {
            None
        };
        let arcade_list_update_us = arcade_list_update_start.elapsed().as_micros();
        let preview_blit_start = Instant::now();
        let empty_base_cached_rect = if (layout.is_portrait() || preview_direct_present_enabled())
            && preview_route.allows_preview_work()
            && composition_decision.allow_preview_blit
            && !memory_guard.active()
            && preview.empty_base_commit_pending()
        {
            Some(layer_target.clear_cached_preview())
        } else {
            None
        };
        let (raw_preview, preview_transition_trace) = if preview_route.allows_preview_work()
            && composition_decision.allow_preview_blit
            && !memory_guard.active()
        {
            layer_target.blit_raw_preview_if_needed(
                &mut preview,
                &mut preview_transition,
                loop_start.duration_since(run_start),
                this_rect,
                full_frame_present,
            )
        } else {
            (None, PreviewTransitionTrace::default())
        };
        let preview_blit_us = preview_blit_start.elapsed().as_micros();
        if preview_transition_trace.active {
            request_launcher_redraw!();
        }
        let navigation_transition_composition_active = navigation_transition.is_active();
        let navigation_transition_frame_active = navigation_transition_composition_active
            && navigation_transition.frame().phase != NavigationTransitionPhase::Capture;
        let (navigation_transition_edge, navigation_transition_direction) =
            if navigation_transition_frame_active {
                navigation_transition.request().map_or(("", ""), |request| {
                    (request.edge.label(), request.direction.label())
                })
            } else {
                ("", "")
            };
        let navigation_transition_frame_started =
            navigation_transition_frame_active.then_some(loop_start);
        let mut navigation_transition_render_us = 0u128;
        if navigation_transition_composition_active {
            let navigation_transition_compositor_started = Instant::now();
            let now_us = loop_start
                .saturating_duration_since(start)
                .as_micros()
                .min(u64::MAX as u128) as u64;
            let destination_committed = pending_navigation_transition
                .as_ref()
                .is_some_and(|pending| pending.committed);
            let mut render_transition_frame = true;
            if destination_committed && !navigation_transition.destination_ready() {
                let destination_raster_ready =
                    composition_decision.prepare_navigation_destination && slint_base_rendered;
                let mut destination_layers_ready =
                    destination_raster_ready && nav.screen != Screen::Arcade;
                if destination_raster_ready && nav.screen == Screen::Arcade {
                    let preview_expected = selected_arcade_game_has_preview(&nav, &catalog);
                    let preview_snapshot_ready = navigation_preview_snapshot_ready(
                        preview_expected,
                        preview.terminal_empty(),
                        preview.trace_cache_state(),
                        preview.raw_frame_status(),
                    );
                    let preview_surface_ready = if !preview_expected || preview.terminal_empty() {
                        if preview_snapshot_ready {
                            let _ = layer_target.clear_cached_preview();
                            true
                        } else {
                            false
                        }
                    } else if preview_snapshot_ready {
                        match layer_target.compose_exact_preview(&preview) {
                            Some(RawPreviewPresent::Cached(_)) => true,
                            Some(RawPreviewPresent::Direct(rect)) => {
                                layer_target.compose_direct_preview_rect(rect) > 0
                            }
                            None => false,
                        }
                    } else {
                        false
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
                            let _ = layer_target
                                .compose_arcade_list_update(&mut arcade_list_renderer, update);
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
                        .capture_destination(layer_target.cached_frame_view().pixels(), now_us)
                        .is_err()
                    {
                        navigation_transition.settle_at_destination();
                        render_transition_frame = false;
                    }
                    navigation_transition.tick(now_us);
                }
            }
            if render_transition_frame && let Ok(frame) = navigation_transition.render() {
                let _ = layer_target.restore_cached(frame);
            }
            full_frame_present = true;
            request_launcher_redraw!();
            if navigation_transition.frame().phase == NavigationTransitionPhase::Settled {
                let completion = navigation_transition.complete();
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
        let custom_draw_trace = LauncherCustomDrawTrace {
            arcade_list_update_us,
            preview_blit_us,
            effect_label_us,
            navigation_transition_overlay_us: navigation_transition.last_render_stats().overlay_us
                as u128,
            navigation_transition_edge,
            navigation_transition_direction,
            navigation_snapshot_locked: navigation_snapshot_locked_before_render,
            navigation_slint_render_called: !screensaver.active
                && !navigation_snapshot_locked_before_render,
            navigation_status_quiesce_wait_us: navigation_telemetry.status_quiesce_wait_us,
            navigation_status_quiesce_timeout: navigation_telemetry.status_quiesce_timeout,
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
            x1: layout.logical_w(),
            y1: layout.logical_h(),
        };
        let raw_preview_cached_rect = raw_preview.and_then(RawPreviewPresent::cached_rect);
        let raw_preview_direct_rect = raw_preview.and_then(RawPreviewPresent::direct_rect);
        if raw_preview_direct_rect.is_some() {
            launcher_preview_version = launcher_preview_version.wrapping_add(1).max(1);
        }
        if matches!(arcade_list_rect, Some(ArcadeListUpdate::Full(_))) {
            launcher_arcade_version = launcher_arcade_version.wrapping_add(1).max(1);
            launcher_arcade_scroll_offset = 0;
        } else if let Some(ArcadeListUpdate::Scroll { delta_y, .. }) = arcade_list_rect {
            launcher_arcade_scroll_offset =
                launcher_arcade_scroll_offset.saturating_add(delta_y as i64);
        }
        let cached_arcade_rect = if crt_layout || layout.is_portrait() {
            arcade_list_rect.map(|update| {
                let rect = arcade_update_dirty_rect(&update);
                let _ = layer_target.compose_arcade_list_update(&mut arcade_list_renderer, update);
                rect
            })
        } else {
            None
        };
        let preview_layer_desired =
            should_desire_direct_layer(wants_preview, composition_decision.allow_preview_blit);
        let preview_desired =
            if !layout.is_portrait() && preview_layer_desired && preview_direct_present_enabled() {
                Some(DirectLayerState::new(
                    preview_screen_rect(ui),
                    launcher_preview_version,
                ))
            } else {
                None
            };
        let arcade_desired = if !layout.is_portrait()
            && !crt_layout
            && should_desire_direct_layer(
                wants_arcade_list,
                composition_decision.allow_arcade_list_blit,
            ) {
            Some(
                DirectLayerState::new(arcade_list_renderer.dirty_rect(), launcher_arcade_version)
                    .with_content_offset_y(launcher_arcade_scroll_offset),
            )
        } else {
            None
        };
        let mut cached_damage = if full_frame_present {
            DirtyRectList::from_one(full_rect)
        } else if slint_damage.is_empty() {
            let mut damage = DirtyRectList::new();
            damage.push_if_some(this_rect);
            damage
        } else {
            slint_damage
        };
        cached_damage.push_if_some(empty_base_cached_rect);
        cached_damage.push_if_some(raw_preview_cached_rect);
        cached_damage.push_if_some(cached_arcade_rect);
        let cached_damage = layer_target.rotate_damage_to_composition(&cached_damage);
        let final_preview_target_presented = raw_preview.is_some()
            && preview.presentation_requires_present()
            && preview_transition_trace.progress >= 1.0;
        let cached_empty_target_presented = (layout.is_portrait()
            || !preview_direct_present_enabled())
            && final_preview_target_presented
            && raw_preview_cached_rect.is_some()
            && matches!(
                preview.presentation_state(),
                PreviewPresentationState::Transitioning {
                    target: PreviewPresentationTarget::Empty
                }
            );
        let preview_presentation_commit = preview.presentation_commit(
            final_preview_target_presented,
            empty_base_cached_rect.is_some() || cached_empty_target_presented,
        );
        let frame_plan = LauncherFramePlan::new(
            cached_damage,
            preview_desired,
            raw_preview_direct_rect,
            arcade_desired,
            if crt_layout || layout.is_portrait() {
                None
            } else {
                arcade_list_rect
            },
        );
        let startup_can_present = lifecycle.startup_can_present_frame();
        let stream_motion_active = stream_motion_before_render
            || preview_transition_trace.active
            || navigation_transition_composition_active;
        let direct_hidden_present_mode = startup_intro.is_some();
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
        let LauncherPresentCycle {
            presentation,
            frame_t3,
            frame_t4,
            cpu_t3,
            cpu_t4,
            pacing_trace,
        } = present_cycle;
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
        if !first_vsync_logged && pacing_trace.vsync_source == Some(VsyncPaceSource::Vsync) {
            first_vsync_logged = true;
            boot_analytics::event("first_vsync", format!("frame={frames}"));
        }
        let visible_frame_presented = visible_frame_was_presented(
            presentation.copied_rows,
            accepted_screensaver_frame || accepted_startup_intro_frame,
            presentation.main_present_status,
            presentation.main_present_copy_path,
        );
        // Posting a buffer and observing it pending proves latch acceptance,
        // not physical presentation. The intro advances only after the final
        // active-sequence confirmation below.
        let startup_intro_frame_posted = visible_frame_presented && accepted_startup_intro_frame;
        if navigation_transition_frame_active && visible_frame_presented {
            screensaver_cpu_profile.begin_navigation_transition(frames.saturating_add(1));
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
            if !screensaver_first_card_present_logged && screensaver_has_rendered_card {
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
            if lifecycle.startup_status().mode == StartupMode::ReturnFromGame
                && lifecycle.startup_status().revealed
            {
                launch_return_session.mark_correct_present(&nav, &catalog);
                if launch_return_session.first_correct_present_monotonic_us != 0
                    && cpu_profile::launch_return_profile_requested()
                    && cpu.is_some()
                    && let Err(error) = cpu_profile::finish_launch_return_async(cpu.take())
                {
                    crate::ui_errln!("launch-return cpu profile finalization failed: {error}");
                }
                if catalog_session.refresh_done() {
                    launch_return_session.release_if_complete();
                }
            }
            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
        }
        arcade_entry_latency.record_presented_frame(
            start,
            frame_t4,
            &lifecycle,
            &catalog,
            &nav,
            &preview,
            frames,
            prepare_us,
            presentation.copied_rows,
        );
        let mut presented_frame = LauncherFrameSnapshotBuilder {
            identity: LauncherFrameIdentity {
                frames,
                automation: automation_frame_stamp,
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
                screensaver_archive_loading,
                screensaver_frame_trace,
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
        let mut accepted_and_active_confirmed = false;
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
            // Latch mode posts the hidden buffer first, then spends the slack before
            // vblank on normal per-frame accounting. The final wait is only the
            // pacing boundary for the next frame.
            let wait_start = Instant::now();
            let pace = pacer.wait();
            let completion_timeout = Duration::from_micros(pacer.period_us().saturating_mul(3) / 2);
            let completion_remaining = completion_timeout.saturating_sub(wait_start.elapsed());
            let completion = wait_for_latch_completion(
                f,
                presented_frame.main_present_sequence,
                completion_remaining,
            );
            let wait_done = Instant::now();
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
                    });
                    presented_frame.main_present_active_sequence = status.active_sequence;
                    presented_frame.main_present_pending = status.pending();
                    presented_frame.main_present_flip_count = status.flip_count;
                    presented_frame.main_present_drop_count = status.drop_count;
                    presented_frame.main_present_completion_poll_count = completion.poll_count;
                    presented_frame.main_present_completion_poll_wall_us = completion.wall_us;
                    presented_frame.main_present_completion_poll_cpu_us = completion.cpu_us;
                }
                Err(failure) => {
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
                launcher_automation.acknowledge_presented(
                    presented_frame.automation,
                    presented_frame.main_present_sequence,
                );
                if let Some(post) = readiness_post {
                    launcher_readiness.observe(post, lifecycle.startup_can_present_frame());
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
                    let returned = intro.take_buffers();
                    if !restored {
                        crate::ui_errln!("startup intro handoff cache geometry mismatch");
                    }
                    if let Err(failure) =
                        launcher_presenter.restore_direct_hidden_frame_buffers(returned)
                    {
                        launcher_presenter.fail_latch_completion(failure);
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
            frame_accounting.write_finished_frame_trace(
                &presented_frame,
                finish_timing,
                latch_trace_flush_deferred,
            );
        } else {
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
        latch_v5_qualification.record_present(
            accepted_and_active_confirmed,
            scheduler.catalog_worker_running(),
        );
        let preview_present_confirmed = if latch_trace_flush_deferred {
            accepted_and_active_confirmed
        } else {
            visible_frame_presented
        };
        if preview_present_confirmed && let Some(commit) = preview_presentation_commit {
            preview.confirm_presentation(commit);
        }
        if preview.presentation_requires_present() {
            request_launcher_redraw!();
        }
        latch_v5_qualification.write_state_if_due(Instant::now());
        frames += 1;
    }
    if let Some(mut intro) = startup_intro.take() {
        let returned = intro.take_buffers();
        if let Err(failure) = launcher_presenter.restore_direct_hidden_frame_buffers(returned) {
            launcher_presenter.fail_latch_completion(failure);
        }
        launcher_presenter.invalidate_external_hidden_mode();
    }
    // Preserve the continuous background permission for a later launcher run
    // in the same process (notably host tests and diagnostic runners).
    mister_magik_catalog::builder_service::set_background_heavy_work_allowed(true);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewRoutePolicy {
    crt_layout: bool,
}

impl PreviewRoutePolicy {
    const fn new(crt_layout: bool) -> Self {
        Self { crt_layout }
    }

    const fn allows_preview_work(self) -> bool {
        !self.crt_layout
    }
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

#[allow(clippy::too_many_arguments)]
fn process_catalog_worker_message(
    message: CatalogWorkerMessage,
    prepare_trace: &mut LauncherPrepareTrace,
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
        let media_gate = if nav.uses_crt_layout() {
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

fn catalog_messages_need_polling(
    pending_catalog_ready: bool,
    refresh_done: bool,
    worker_running: bool,
) -> bool {
    pending_catalog_ready || !refresh_done || worker_running
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

fn launcher_auto_launch_selected_enabled() -> bool {
    matches!(
        std::env::var("MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn launcher_auto_launch_gate_ready() -> bool {
    let value = std::env::var("MISTER_MAGIK_TEST_AUTO_LAUNCH_GATE").ok();
    launcher_auto_launch_gate_ready_from_value(value.as_deref())
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
    defer_bridge_ui: bool,
    now: Instant,
    start: Instant,
) {
    let preview_route = PreviewRoutePolicy::new(nav.uses_crt_layout());
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
                }
                if let Some(publication_ack) = publication_ack {
                    let _ = publication_ack.send(());
                }
                if taxonomy_sync_required {
                    nav.sync_launcher_taxonomy(catalog);
                }
                apply_forced_arcade_selected(nav, catalog);
                let return_restored =
                    apply_pending_launch_return_state(nav, catalog, launch_return_session, source);
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
                if defer_bridge_ui {
                    continue;
                }
                nav.catalog_build_started();
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
                if defer_bridge_ui {
                    continue;
                }
                nav.catalog_reconciliation_plan(catalog, &system_ids, all_published_systems);
                *catalog = nav.catalog_with_build_shells(catalog.clone());
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                let _ = reapply_pending_launch_return_state(nav, catalog, launch_return_session);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogSystemDiscovered { .. } => {}
            CatalogSessionEffect::CatalogSystemScanning { system_id } => {
                if defer_bridge_ui {
                    continue;
                }
                nav.catalog_system_scanning(&system_id);
                *catalog = catalog.with_system_placeholder(&system_id);
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                let _ = reapply_pending_launch_return_state(nav, catalog, launch_return_session);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogSystemPrepared {
                system_id,
                generation,
            } => {
                if !defer_bridge_ui {
                    nav.catalog_system_prepared(&system_id);
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
            CatalogSessionEffect::ApplySystemShard { system_id, games } => {
                nav.catalog_system_hydration_finished(&system_id);
                let (replacement, launch_plans) = arcade_rows_from_shard(&system_id, &games);
                *catalog = catalog.replacing_system_games(&system_id, replacement, launch_plans);
                *catalog_version = (*catalog_version).wrapping_add(1);
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
                print_startup_event(
                    start,
                    "catalog_system_shard_ready",
                    format!("system={system_id} games={}", games.len()),
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

fn library_changed_test_dialog_choice_from_env(
    start: Instant,
) -> Option<launcher::LibraryChangedTestDialogChoice> {
    let value = std::env::var("MISTER_MAGIK_TEST_LIBRARY_CHANGED_DIALOG_CHOICE").ok()?;
    match launcher::parse_library_changed_test_dialog_choice(&value) {
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

fn effective_lock_screen(
    lock_screen: Option<Screen>,
    catalog_ready: bool,
    catalog: &ArcadeCatalog,
) -> Option<Screen> {
    match lock_screen {
        Some(Screen::Arcade) if !arcade_navigation_ready(catalog_ready, catalog) => None,
        other => other,
    }
}

fn ready_catalog_worker_request(refresh_policy: CatalogRefreshPolicy) -> CatalogWorkerRequest {
    if refresh_policy == CatalogRefreshPolicy::Off {
        CatalogWorkerRequest::LoadOnly
    } else if refresh_policy.force_requested() {
        CatalogWorkerRequest::RECONCILE_CHANGED_INPUTS
    } else {
        CatalogWorkerRequest::CheckStamp
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
        return Some(request);
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

fn apply_home_selected_from_env(nav: &mut LauncherNav, catalog: &ArcadeCatalog, start: Instant) {
    let Ok(value) = std::env::var("MISTER_HOME_SELECTED_INDEX") else {
        return;
    };
    let Ok(selected) = value.parse::<usize>() else {
        print_startup_event(
            start,
            "launcher_home_selected_index_invalid",
            format!("value={value}"),
        );
        return;
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
    fn modal_input_test_requires_every_path_below_fixed_tmp_root() {
        assert!(modal_input_test_paths_are_isolated([
            "/tmp/mister-magik/modal-input-benchmark/catalog-v3",
            "/tmp/mister-magik/modal-input-benchmark/library.sqlite3",
            "/tmp/mister-magik/modal-input-benchmark/catalog-ready.snapshot",
        ]));
        assert!(!modal_input_test_paths_are_isolated([
            "/tmp/mister-magik/modal-input-benchmark/catalog-v3",
            "/media/fat/mister-magik-dev/library.sqlite3",
        ]));
        assert!(!modal_input_test_paths_are_isolated([
            "/tmp/mister-magik/modal-input-benchmark",
        ]));
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
                x0: 16,
                y0: 104,
                x1: 624,
                y1: 416,
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
                x0: 288,
                y0: 104,
                x1: 624,
                y1: 416,
            }
        );
    }

    #[test]
    fn crt_routes_use_roomier_rows_in_normal_and_search_layouts() {
        for (pal, scandoubler, expected_row_height, expected_full_rows) in
            [(0, 0, 32, 9), (1, 0, 19, 7), (0, 1, 32, 12), (1, 1, 39, 11)]
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
                let (geometry, render_h) = arcade_list_layout(&nav, &display);
                let visible_height = geometry.visible_height_with_metrics(render_h, Some(metrics));
                assert_eq!(
                    visible_height / metrics.game_row_height as usize,
                    expected_full_rows
                );
                let mut renderer = ArcadeListRenderer::new_for_crt_display(metrics, &display);
                renderer.set_geometry_for_render_h(geometry, render_h);
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
            settings_page_transition_direction(Screen::Home, Screen::Settings),
            Some(NavigationTransitionDirection::Forward)
        );
        assert_eq!(
            settings_page_transition_direction(Screen::Settings, Screen::About),
            Some(NavigationTransitionDirection::Forward)
        );
        assert_eq!(
            settings_page_transition_direction(Screen::About, Screen::Licenses),
            Some(NavigationTransitionDirection::Forward)
        );
        assert_eq!(
            settings_page_transition_direction(Screen::Licenses, Screen::About),
            Some(NavigationTransitionDirection::Reverse)
        );
        assert_eq!(
            settings_page_transition_direction(Screen::Screensaver, Screen::Home),
            Some(NavigationTransitionDirection::Reverse)
        );
        assert_eq!(
            settings_page_transition_direction(Screen::Home, Screen::Arcade),
            None
        );
        assert_eq!(
            settings_page_transition_direction(Screen::Screensaver, Screen::About),
            None
        );
    }

    #[test]
    fn catalog_recovery_consumes_a_until_release() {
        let catalog = catalog_for_media_systems(&["arcade"]);
        let mut nav = LauncherNav::new();
        let released = PadState::default();
        let pressed = pad_state_with(|state| state.btn_a = true);
        let now = Instant::now();

        let input = route_lifecycle_dialog_input(&mut nav, &pressed, &released, false, true);
        assert!(matches!(
            input,
            Some(LauncherLifecycleInput::CatalogRecoveryConfirm)
        ));
        assert!(nav.handle_input(&pressed, now, &catalog).is_none());
        assert_eq!(nav.screen, Screen::Home);

        assert!(
            nav.handle_input(&released, now + Duration::from_millis(16), &catalog)
                .is_none()
        );
        let event = nav
            .handle_input_with_navigation_intents(
                &pressed,
                now + Duration::from_millis(32),
                &catalog,
            )
            .expect("fresh A should reach the selected Arcade tile");
        assert_eq!(event.action, LauncherAction::OpenCollection);
        assert_eq!(event.path.as_deref(), Some("menu:arcade"));
    }

    #[test]
    fn launch_failure_consumes_every_acknowledgement_button() {
        for pressed in [
            pad_state_with(|state| state.btn_a = true),
            pad_state_with(|state| state.btn_b = true),
            pad_state_with(|state| state.btn_home = true),
        ] {
            let catalog = catalog_for_media_systems(&["arcade"]);
            let mut nav = LauncherNav::new();
            let input =
                route_lifecycle_dialog_input(&mut nav, &pressed, &PadState::default(), true, false);
            assert!(matches!(
                input,
                Some(LauncherLifecycleInput::LaunchFailureAcknowledge)
            ));
            assert!(
                nav.handle_input(&pressed, Instant::now(), &catalog)
                    .is_none()
            );
            assert_eq!(nav.screen, Screen::Home);
        }
    }

    #[test]
    fn exclusive_input_absorption_resets_direction_repeat() {
        let mut nav = LauncherNav::new();
        let held = pad_state_with(|state| state.dpad_right = true);
        let now = Instant::now();
        absorb_exclusive_input(&mut nav, &held);

        let catalog = catalog_for_media_systems(&["arcade"]);
        assert!(nav.handle_input(&held, now, &catalog).is_none());
        assert_eq!(nav.selected, 0);
    }

    #[test]
    fn rapid_back_inputs_queue_through_the_settings_hierarchy_without_bouncing() {
        let catalog = catalog_for_media_systems(&[]);
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Info;
        let released = PadState::default();
        let back = PadState {
            btn_b: true,
            ..PadState::default()
        };
        let now = Instant::now();
        nav.absorb_input(&released);
        assert!(
            nav.handle_input_with_navigation_intents(&back, now, &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::About);

        let pixels = vec![Rgb565Pixel(0); 4 * 3];
        let mut transition = NavigationTransitionRuntime::new(4, 3, true);
        transition
            .begin_settings_page(NavigationTransitionDirection::Reverse, &pixels, 0)
            .unwrap();
        assert!(transition.route_input(&back, &released, false).is_none());
        assert!(transition.route_input(&released, &back, false).is_none());
        assert!(transition.route_input(&back, &released, false).is_none());
        transition.settle_at_destination();
        assert!(transition.complete().is_some());

        let to_settings = transition.route_input(&released, &back, false).unwrap();
        nav.absorb_input(&to_settings.previous);
        assert!(
            nav.handle_input_with_navigation_intents(&to_settings.now, now, &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::Settings);
        transition
            .begin_settings_page(NavigationTransitionDirection::Reverse, &pixels, 1)
            .unwrap();
        transition.settle_at_destination();
        assert!(transition.complete().is_some());

        let to_home = transition.route_input(&released, &released, false).unwrap();
        nav.absorb_input(&to_home.previous);
        assert!(
            nav.handle_input_with_navigation_intents(&to_home.now, now, &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::Home);
    }

    #[test]
    fn navigation_destination_waits_for_exact_ready_preview_pixels() {
        assert!(navigation_preview_snapshot_ready(
            false,
            false,
            "empty",
            PreviewRawFrameStatus::Empty,
        ));
        assert!(!navigation_preview_snapshot_ready(
            true,
            false,
            "placeholder",
            PreviewRawFrameStatus::Empty,
        ));
        assert!(!navigation_preview_snapshot_ready(
            true,
            false,
            "exact",
            PreviewRawFrameStatus::Empty,
        ));
        assert!(navigation_preview_snapshot_ready(
            true,
            false,
            "exact",
            PreviewRawFrameStatus::Ready,
        ));
        assert!(navigation_preview_snapshot_ready(
            true,
            true,
            "empty",
            PreviewRawFrameStatus::Empty,
        ));
    }

    #[test]
    fn arcade_source_preview_survives_suppressed_transition_composition() {
        assert!(should_clear_suppressed_preview(false, false));
        assert!(!should_clear_suppressed_preview(false, true));
        assert!(!should_clear_suppressed_preview(true, false));
        assert!(!should_clear_suppressed_preview(true, true));
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
            false,
            LauncherPresentStatus::Ok,
            LatchCopyPath::IdentityFull.label(),
        ));
        assert!(visible_frame_was_presented(
            0,
            true,
            LauncherPresentStatus::Ok,
            LatchCopyPath::ExternalDirect.label(),
        ));
        assert!(!visible_frame_was_presented(
            0,
            false,
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
    fn crt_catalog_discovery_sequence_never_reaches_media_worker_actions() {
        fn dispatched_media_actions(crt_layout: bool) -> Vec<&'static str> {
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
                let Some(media_effects) = dispatch_catalog_media_effect(
                    PreviewRoutePolicy::new(crt_layout),
                    &effect,
                    &mut media_session,
                ) else {
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

        assert!(dispatched_media_actions(true).is_empty());
        assert_eq!(
            dispatched_media_actions(false),
            vec!["ensure-worker", "set-interaction", "ensure-system"]
        );
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
            SystemShardPriority::Urgent,
            "startup-regression-test",
            Instant::now()
        ));
    }

    #[test]
    fn shard_request_state_changes_only_after_scheduler_acceptance() {
        let mut nav = LauncherNav::new();
        let mut scheduler = LauncherScheduler::new(false);

        assert!(!request_system_shard_hydration(
            &mut scheduler,
            &mut nav,
            "c64",
            SystemShardPriority::Urgent,
            "rejected-without-generation",
            Instant::now()
        ));
        assert!(!nav.catalog_system_hydration_is_loading("c64"));

        nav.catalog_system_hydration_failed("c64");
        assert!(!retry_system_shard_hydration(
            &mut scheduler,
            &mut nav,
            "c64",
            "rejected-retry-without-generation",
            Instant::now()
        ));
        assert!(nav.catalog_system_hydration_has_failed("c64"));

        let _ = initialize_catalog_generation(&mut scheduler, Some("generation-a".to_string()));
        assert!(retry_system_shard_hydration(
            &mut scheduler,
            &mut nav,
            "c64",
            "accepted-retry",
            Instant::now()
        ));
        assert!(nav.catalog_system_hydration_is_loading("c64"));
    }

    #[test]
    fn pending_launch_return_requests_its_registry_shard_before_home_prefetch() {
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
            &mut restored_nav,
            &mut scheduler,
            now,
            now,
        ));
        assert!(scheduler.system_shard_attempted("c64"));
        assert!(!scheduler.request_system_shard(
            "c64".to_string(),
            SystemShardPriority::Selected,
            "home-highlight",
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
    fn cold_collection_sequence_keeps_home_bridge_until_populated_commit() {
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
        });
        let mut now = PadState::default();
        now.btn_b = true;

        assert!(cancel_pending_collection_entry_for_input(
            &mut pending,
            &mut nav,
            &now,
            &PadState::default(),
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
        assert_eq!(
            LauncherPresentBackend::from_env_values(None),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
        assert_eq!(
            LauncherPresentBackend::from_env_values(Some("")),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
        assert_eq!(
            LauncherPresentBackend::from_env_values(Some("fb0-dirty")),
            LauncherPresentBackend::Fb0Dirty
        );
    }

    #[test]
    pub(super) fn launcher_present_backend_retired_values_use_required_latch_backend() {
        assert_eq!(
            LauncherPresentBackend::from_env_values(Some(&["main", "flip-v1"].join("-"))),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
        assert_eq!(
            LauncherPresentBackend::from_env_values(Some(&["main", "vsync-hidden"].join("-"))),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
        assert_eq!(
            LauncherPresentBackend::from_env_values(Some(
                &["plugin", "main", "vsync-hidden"].join("-")
            )),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
        assert_eq!(
            LauncherPresentBackend::from_env_values(Some("fpga-vblank-latch-hidden")),
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
        };

        assert!(driver.input_for(&nav, start, start).is_none());
        nav.confirm_action = Some(launcher::ConfirmAction::LibraryChanged);

        assert!(driver.input_for(&nav, start, start).is_none());
        let input = driver
            .input_for(&nav, start + LIBRARY_CHANGED_TEST_ACTION_SETTLE, start)
            .expect("continue driver should press A");
        assert!(input.btn_a);
        let event = nav
            .handle_input(
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
        };
        let catalog = empty_arcade_catalog("/tmp");

        assert!(driver.input_for(&nav, start, start).is_none());
        let right = driver
            .input_for(&nav, start + LIBRARY_CHANGED_TEST_ACTION_SETTLE, start)
            .expect("rebuild driver should press right first");
        assert!(right.dpad_right);
        assert!(
            nav.handle_input(&right, start + LIBRARY_CHANGED_TEST_ACTION_SETTLE, &catalog)
                .is_none()
        );
        assert_eq!(nav.confirm_selected, 1);

        let release = driver
            .input_for(
                &nav,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(16),
                start,
            )
            .expect("rebuild driver should release right before A");
        assert!(!release.dpad_right);
        assert!(!release.btn_a);
        assert!(
            nav.handle_input(
                &release,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(16),
                &catalog,
            )
            .is_none()
        );

        let press_a = driver
            .input_for(
                &nav,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(32),
                start,
            )
            .expect("rebuild driver should press A");
        assert!(press_a.btn_a);
        let event = nav
            .handle_input(
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

        let left = driver.input_for().expect("left press");
        assert!(left.dpad_left);
        assert!(!left.dpad_down);
        assert!(!left.dpad_right);

        for _ in 1..LAUNCHER_INPUT_SCRIPT_PRESS_FRAMES {
            assert!(driver.input_for().expect("left hold").dpad_left);
        }
        for _ in 0..LAUNCHER_INPUT_SCRIPT_RELEASE_FRAMES {
            let release = driver.input_for().expect("left release");
            assert!(!release.dpad_left);
            assert!(!release.dpad_down);
            assert!(!release.dpad_right);
        }
        let gap = driver.input_for().expect("between buttons");
        assert!(!gap.dpad_left);
        assert!(!gap.dpad_down);
        assert!(!gap.dpad_right);

        let down = driver.input_for().expect("down press");
        assert!(!down.dpad_left);
        assert!(down.dpad_down);
        assert!(!down.dpad_right);
    }

    #[test]
    fn screensaver_show_navigation_script_uses_production_settings() {
        let start = Instant::now();
        let catalog = empty_arcade_catalog("/tmp");
        let mut nav = LauncherNav::new();
        let mut driver = LauncherInputScriptDriver::from_script("up,a,down,a,down,down,a", start);
        driver.wait_frames = 0;
        let mut action = None;
        let mut frame = 0;

        while driver.active() {
            let input = driver.input_for().unwrap_or_default();
            if let Some(event) =
                nav.handle_input(&input, start + Duration::from_millis(frame * 17), &catalog)
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
            LauncherWakeReasons::COMPOSITION_FORCES_FULL_PRESENT,
            LauncherWakeReasons::COMPOSITION_CLEARS_DIRECT_LAYERS,
            LauncherWakeReasons::FB0_ROUTE_RECOVERY_PENDING,
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
    pub(super) fn latch_late_start_wait_is_disabled_only_for_active_home_motion() {
        assert!(latch_late_start_wait_enabled(false, false));
        assert!(latch_late_start_wait_enabled(false, true));
        assert!(latch_late_start_wait_enabled(true, false));
        assert!(!latch_late_start_wait_enabled(true, true));
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
    pub(super) fn home_pan_present_rect_matches_home_list_band() {
        let ui = UiDisplay::for_framebuffer(960, 540);
        assert_eq!(
            home_pan_present_rect(&ui),
            DirtyRect {
                x0: 18,
                y0: 74,
                x1: 942,
                y1: 522,
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
                y1: 522,
            })
        );
        assert_eq!(
            expand_home_pan_dirty_rect(None, &ui, true),
            Some(DirtyRect {
                x0: 18,
                y0: 74,
                x1: 942,
                y1: 522,
            })
        );
    }

    #[test]
    pub(super) fn ready_catalog_uses_background_worker_for_refresh_or_home_validation() {
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Default),
            CatalogWorkerRequest::CheckStamp
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
    pub(super) fn summary_return_hydration_runs_even_when_refresh_is_off() {
        assert_eq!(
            summary_seed_catalog_worker_request(CatalogRefreshPolicy::Off, false, false),
            None
        );
        assert_eq!(
            summary_seed_catalog_worker_request(CatalogRefreshPolicy::Off, false, true),
            Some(CatalogWorkerRequest::LoadOnly)
        );
        assert_eq!(
            summary_seed_catalog_worker_request(CatalogRefreshPolicy::Default, false, true),
            Some(CatalogWorkerRequest::CheckStamp)
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
    fn screensaver_preview_ignores_launch_release_then_consumes_next_input() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, ScreensaverStartMode::Inactive);

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
        assert!(saver.handle_input(start, true, true));
        assert!(saver.active);
        assert!(saver.handle_input(start + Duration::from_millis(16), false, true));
        assert!(saver.active);
        assert!(saver.handle_input(start + Duration::from_secs(1), true, true));
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
