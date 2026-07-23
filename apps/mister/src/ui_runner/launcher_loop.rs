// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::arcade_drawer::{ArcadeDrawerViewCache, arcade_filter_cache_token};
use super::launcher_frame_accounting::{
    FrameAnalyticsCpuStamp, LauncherCustomDrawTrace, LauncherFrameAccounting,
    LauncherFrameCpuTrace, LauncherFrameIdentity, LauncherFrameRenderData,
    LauncherFrameSnapshotBuilder, LauncherFrameStatusData, LauncherFrameTiming,
};
use super::launcher_pacing::{
    FB0_LATE_FRAME_START_HEADROOM_US, FPGA_LATCH_LATE_FRAME_START_HEADROOM_US,
    LauncherFramePacingInput, LauncherFramePacingPolicy, LauncherPacingTrace,
};
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
            Self::None | Self::Fb0Dirty | Self::CompatibilityFb0 => {}
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
        (LauncherPresentBackend::CompatibilityFb0, _)
        | (_, LauncherPresentStatus::Compatibility) => "Mode=compatibility",
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
            confirm_left_label: bridge.get_confirm_left_label(),
            confirm_right_label: bridge.get_confirm_right_label(),
        }
    }

    fn bytes_len(&self) -> usize {
        self.catalog_scan_message.len()
            + self.catalog_scan_title.len()
            + self.catalog_scan_detail.len()
            + self.confirm_title.len()
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LauncherBridgeSyncPlan {
    None,
    Full,
    Light,
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
const CATALOG_BACKGROUND_IDLE_SETTLE: Duration = Duration::from_millis(2000);
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
    let scale = SLINT_UI_SCALE.max(1) as usize;
    let x0 = HOME_LAYOUT_PADDING * scale;
    let y0 = (HOME_LAYOUT_PADDING + HOME_HEADER_H + HOME_LAYOUT_SPACING) * scale;
    let x1 = ui.render_w().saturating_sub(HOME_LAYOUT_PADDING * scale);
    let y1 = ui.render_h().saturating_sub(HOME_LAYOUT_PADDING * scale);
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

#[derive(Clone, Copy, Debug, Default)]
struct CatalogBackgroundIdleInput {
    first_visible_copy_done: bool,
    startup_return_waiting_for_catalog: bool,
    startup_input_enabled: bool,
    launching: bool,
    setup_active: bool,
    benchmark_active: bool,
    scripted_input_active: bool,
    pad_changed: bool,
    pad_active: bool,
    catalog_publication_pending: bool,
    media_message_seen: bool,
    nav_motion_active: bool,
    preview_critical: bool,
    visual_animation_active: bool,
}

impl CatalogBackgroundIdleInput {
    /// Whether the user-facing launcher is quiet enough for heavy catalog work.
    ///
    /// This deliberately describes interaction, not rendering. A visual-only
    /// Slint animation (for example the flashing catalog-build badge) may keep
    /// frames rendering without resetting the catalog idle-settle window.
    fn is_interaction_idle(self) -> bool {
        let _visual_animation_does_not_block_catalog_work = self.visual_animation_active;
        (self.first_visible_copy_done || self.startup_return_waiting_for_catalog)
            && self.startup_input_enabled
            && !self.launching
            && !self.setup_active
            && !self.benchmark_active
            && !self.scripted_input_active
            && !self.pad_changed
            && !self.pad_active
            && !self.catalog_publication_pending
            && !self.media_message_seen
            && !self.nav_motion_active
            && !self.preview_critical
    }
}

#[derive(Clone, Copy, Debug)]
struct CatalogBackgroundIdleGate {
    idle_since: Option<Instant>,
    settle: Duration,
}

impl CatalogBackgroundIdleGate {
    fn new(settle: Duration) -> Self {
        Self {
            idle_since: None,
            settle,
        }
    }

    fn allow(&mut self, input: CatalogBackgroundIdleInput, now: Instant) -> bool {
        if !input.is_interaction_idle() {
            self.idle_since = None;
            return false;
        }
        let since = *self.idle_since.get_or_insert(now);
        now.saturating_duration_since(since) >= self.settle
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

fn catalog_message_requires_publication_pause(message: &CatalogWorkerMessage) -> bool {
    matches!(message, CatalogWorkerMessage::Ready { .. })
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

fn home_repeat_benchmark_active(scenario: Option<LauncherBenchScenario>) -> bool {
    scenario == Some(LauncherBenchScenario::HomeRepeatHold)
}

fn catalog_background_nav_motion_active(nav: &LauncherNav) -> bool {
    nav.arcade.has_scroll_motion_or_queue()
        || nav.arcade.is_scroll_active()
        || (nav.arcade_filter.drawer_open && nav.arcade_filter.is_scroll_active())
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

pub(super) struct ShardedCatalogSeed {
    pub(super) catalog: ArcadeCatalog,
    pub(super) catalog_fingerprint: String,
    pub(super) generation: u64,
}

pub(super) struct ShardedCatalogSeedLoadError {
    pub(super) status: &'static str,
    error: String,
}

impl std::fmt::Display for ShardedCatalogSeedLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.error)
    }
}

pub(super) fn load_sharded_registry_seed(
    root: &str,
) -> Result<ShardedCatalogSeed, ShardedCatalogSeedLoadError> {
    load_sharded_registry_seed_at(
        root,
        &mister_magik_catalog::catalog_config::default_sharded_catalog_path(),
    )
}

pub(super) fn load_sharded_registry_seed_at(
    root: &str,
    storage: &Path,
) -> Result<ShardedCatalogSeed, ShardedCatalogSeedLoadError> {
    use mister_magik_catalog::sharded_catalog::CatalogReader;

    let reader = mister_magik_catalog::lazy_sharded_reader::LazyShardedCatalogReader::open(
        storage,
        mister_magik_catalog::production_sharded_projection::production_registry_limits(),
    )
    .map_err(|error| ShardedCatalogSeedLoadError {
        status: "unavailable",
        error: error.to_string(),
    })?;
    let registry = reader
        .open_registry()
        .map_err(|error| ShardedCatalogSeedLoadError {
            status: "failed",
            error: error.to_string(),
        })?;
    if registry.systems().is_empty() {
        return Err(ShardedCatalogSeedLoadError {
            status: "empty",
            error: "catalog registry has no systems".to_string(),
        });
    }
    let catalog_fingerprint =
        mister_magik_catalog::production_sharded_projection::validate_production_binding(
            storage,
            registry.generation(),
        )
        .map_err(|error| ShardedCatalogSeedLoadError {
            status: "stale",
            error: error.to_string(),
        })?;
    let systems = registry
        .systems()
        .iter()
        .map(|system| arcade_catalog::GameSystemEntry {
            id: system.system_id.as_str().to_string(),
            title: system.display_title.clone(),
            count: usize::try_from(system.games).unwrap_or(usize::MAX),
        })
        .collect::<Vec<_>>();
    let generation = registry.generation();
    let arcade_id = mister_magik_catalog::catalog_classify::SystemId::parse(
        arcade_catalog::MENU_ARCADE_SYSTEM_ID.trim_start_matches("menu:"),
    )
    .ok();
    let (games, launch_plans) = arcade_id
        .as_ref()
        .and_then(|system_id| reader.open_system(system_id).ok())
        .map(|system| arcade_rows_from_shard("arcade", system.games()))
        .unwrap_or_default();
    let platform_kinds = systems
        .iter()
        .map(|system| {
            (
                system.id.clone(),
                mister_magik_catalog::catalog_classify::platform_kind_for_system(&system.id),
            )
        })
        .collect();
    Ok(ShardedCatalogSeed {
        catalog: ArcadeCatalog::new_with_deferred_text_indexes_and_platform_kinds(
            PathBuf::from(root),
            games,
            systems,
            launch_plans,
            platform_kinds,
        ),
        catalog_fingerprint,
        generation,
    })
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

fn arcade_rows_from_shard(
    system_id: &str,
    games: &[mister_magik_catalog::sharded_catalog::CatalogGame],
) -> (
    Vec<arcade_catalog::ArcadeGameEntry>,
    Vec<arcade_catalog::StructuredLaunchPlan>,
) {
    let mut launch_plans = Vec::new();
    let games = games
        .iter()
        .map(|game| {
            if let Some(plan) = &game.launch_plan {
                launch_plans.push(arcade_catalog::StructuredLaunchPlan {
                    launch_ref: plan.launch_ref.as_str().into(),
                    title: plan.title.as_str().into(),
                    system_id: plan.system_id.as_str().into(),
                    core_path: plan.core_path.as_str().into(),
                    payload_path: plan.payload_path.as_str().into(),
                    mount_kind: plan.mount_kind.as_str().into(),
                    mount_index: plan.mount_index,
                    delay_secs: plan.delay_secs,
                });
            }
            arcade_catalog::ArcadeGameEntry {
                title: game.title.as_str().into(),
                mra_path: game.launch_ref.as_str().into(),
                preview_archive_path: game.preview_archive_path.as_str().into(),
                preview_asset_key: game.preview_asset_key.as_str().into(),
                has_preview: game.has_preview,
                system_id: system_id.into(),
                year: game.year,
                manufacturer: game.manufacturer.as_str().into(),
                players: game.players,
                control: game.control.as_str().into(),
                is_new: game.is_new,
            }
        })
        .collect();
    (games, launch_plans)
}

fn arcade_rows_from_persisted_shard(
    system_id: &str,
    games: &[mister_magik_catalog::system_shard::SystemGame],
) -> (
    Vec<arcade_catalog::ArcadeGameEntry>,
    Vec<arcade_catalog::StructuredLaunchPlan>,
) {
    let mut launch_plans = Vec::new();
    let games = games
        .iter()
        .map(|game| {
            if let Some(plan) = &game.launch_plan {
                launch_plans.push(arcade_catalog::StructuredLaunchPlan {
                    launch_ref: plan.launch_ref.as_str().into(),
                    title: plan.title.as_str().into(),
                    system_id: plan.system_id.as_str().into(),
                    core_path: plan.core_path.as_str().into(),
                    payload_path: plan.payload_path.as_str().into(),
                    mount_kind: plan.mount_kind.as_str().into(),
                    mount_index: plan.mount_index,
                    delay_secs: plan.delay_secs,
                });
            }
            arcade_catalog::ArcadeGameEntry {
                title: game.title.as_str().into(),
                mra_path: game.launch_ref.as_str().into(),
                preview_archive_path: game.preview_archive_path.as_str().into(),
                preview_asset_key: game.preview_asset_key.as_str().into(),
                has_preview: game.has_preview,
                system_id: system_id.into(),
                year: game.year,
                manufacturer: game.manufacturer.as_str().into(),
                players: game.players,
                control: game.control.as_str().into(),
                is_new: game.is_new,
            }
        })
        .collect();
    (games, launch_plans)
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

fn catalog_hydration_execution_mode(_request: CatalogWorkerRequest) -> CatalogExecutionMode {
    CatalogExecutionMode::BackgroundInteractive
}

fn catalog_taxonomy_sync_required(catalog_ready: bool, source: CatalogSource) -> bool {
    !(catalog_ready && source == CatalogSource::NavigationProjection)
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

#[derive(Debug)]
struct ScreensaverControl {
    last_activity: Instant,
    active: bool,
    start_when_ready: bool,
    preview_active: bool,
    waiting_for_input_release: bool,
    restore_full_frame: bool,
    preview_fade_started: Option<Instant>,
}

impl ScreensaverControl {
    fn new(now: Instant, active: bool) -> Self {
        Self {
            last_activity: now,
            active: false,
            start_when_ready: active,
            preview_active: false,
            waiting_for_input_release: false,
            restore_full_frame: false,
            preview_fade_started: None,
        }
    }

    fn update(&mut self, now: Instant, enabled: bool, delay: Duration, catalog_busy: bool) {
        if self.start_when_ready {
            if catalog_busy {
                self.last_activity = now;
                self.active = false;
            } else {
                self.active = true;
                self.start_when_ready = false;
                self.waiting_for_input_release = false;
            }
        } else if catalog_busy && !self.preview_active {
            self.restore_full_frame |= self.active;
            self.last_activity = now;
            self.active = false;
            self.preview_fade_started = None;
        } else if enabled && now.saturating_duration_since(self.last_activity) >= delay {
            self.active = true;
            self.waiting_for_input_release = false;
        }
    }

    fn preview(&mut self, now: Instant) {
        self.active = true;
        self.preview_active = true;
        self.waiting_for_input_release = true;
        self.last_activity = now;
        self.preview_fade_started = Some(now);
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
        }
        false
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
) {
    let start = Instant::now();
    let startup_monotonic_us = monotonic_clock_us().unwrap_or(0);
    let mut frames = 0u64;
    let screensaver_start_active = matches!(
        std::env::var("MISTER_SCREENSAVER_START_ACTIVE")
            .ok()
            .as_deref(),
        Some("1" | "on" | "true" | "yes")
    );
    let mut screensaver = ScreensaverControl::new(Instant::now(), screensaver_start_active);
    let mut screensaver_renderer: Option<LauncherScreensaver> = None;
    let mut screensaver_loader: Option<LauncherScreensaverLoader> = None;
    let mut screensaver_launcher_frame: Option<Vec<Rgb565Pixel>> = None;
    let mut screensaver_show_started: Option<Instant> = None;
    let mut screensaver_first_render_logged = false;
    let mut screensaver_first_present_logged = false;
    let mut screensaver_first_card_present_logged = false;
    let mut launcher_presenter = LauncherPresenter::new(ui);
    let launcher_bench_scenario = LauncherBenchScenario::from_env();
    let launcher_bench_after_input_script =
        launcher_bench_scenario.is_some() && launcher_bench_after_input_script_enabled();
    let launcher_bench_launch_handoff =
        launcher_bench_scenario == Some(LauncherBenchScenario::LaunchHandoff);
    let mut scheduler = LauncherScheduler::new(launcher_bench_launch_handoff);
    let mut catalog_events = CatalogJobEventBuf::new();
    let mut deferred_catalog_events: VecDeque<CatalogWorkerMessage> = VecDeque::new();
    let mut pending_catalog_ready: Option<CatalogWorkerMessage> = None;
    let mut pending_collection_entry: Option<PendingCollectionEntry> = None;
    let mut catalog_ready_deferred_since: Option<Instant> = None;
    let mut catalog_ready_stationary_edge_since: Option<Instant> = None;
    let mut catalog_background_idle_gate =
        CatalogBackgroundIdleGate::new(CATALOG_BACKGROUND_IDLE_SETTLE);
    let mut media_message_seen_last_loop = false;
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
    let start_screen = env_start_screen
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
    let mut pending_launch_return_state =
        launcher::take_launch_return_state().filter(|_| launch_return_restore_allowed);
    if !launch_return_restore_allowed || pending_launch_return_state.is_none() {
        return_catalog_capsule::remove_return_catalog_capsule();
    }
    let startup_return_requested = pending_launch_return_state.is_some();
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
    nav.settings = crate::settings::MagikSettings::load();
    nav.screen = start_screen;
    let mut display_confirm_deadline = None;
    let (display_confirm_tx, display_confirm_rx) =
        mpsc::channel::<Result<launcher::DisplayCommandState, String>>();
    // Main owns the active display mode; the launcher only mirrors its reported state.
    if std::env::var_os("MISTER_MAGIK_PARENT").is_some() {
        if let Ok(state) = launcher::display_state() {
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
    let mut pacer = VsyncPacer::from_env();
    let pacing_policy = LauncherFramePacingPolicy::default();
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
        ArcadeListRenderer::new_for_crt(crt_metrics.game_row_height)
    } else {
        ArcadeListRenderer::new()
    };
    let mut launcher_preview_version = 1u64;
    let mut launcher_arcade_version = 1u64;
    let mut launcher_arcade_scroll_offset = 0i64;
    let mut arcade_drawer_view_cache = ArcadeDrawerViewCache::default();
    let mut composition = UiCompositionController::new();
    let cpu = cpu_profile::start();
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
    let return_capsule_catalog = pending_launch_return_state.as_ref().and_then(|state| {
        let collection_id = state.collection_id()?;
        return_catalog_capsule::take_return_catalog_capsule(
            Path::new(&arcade_root),
            collection_id,
            state.game_path(),
        )
    });
    let mut catalog = return_capsule_catalog.unwrap_or_else(|| empty_arcade_catalog(&arcade_root));
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
    let initial_catalog_fingerprint = sharded_catalog_fingerprint;
    let mut catalog_generation = CatalogGenerationState {
        current: initial_catalog_fingerprint.clone(),
        durable: initial_catalog_fingerprint,
    };
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
    if capsule_seed_ready {
        launch_return_restored = pending_launch_return_state
            .as_ref()
            .cloned()
            .is_some_and(|state| launcher::apply_launch_return_state(&mut nav, &catalog, state));
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
    if catalog_ready && root_arcade_focused && preview_route.allows_preview_work() {
        let games = catalog.system_game_view(arcade_catalog::MENU_ARCADE_SYSTEM_ID);
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
        );
    }
    let _ = lifecycle.after_boot_splash_presented(startup_catalog_state, &mut lifecycle_effects);
    apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
    window.request_redraw();
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
    let mut preview_scroll_exit_at = preview_scroll_exit_after_trace_deadline(run_start);
    let mut first_render_logged = false;
    let mut first_vsync_logged = false;
    let mut first_launcher_frame_logged = false;
    let mut frame_accounting = LauncherFrameAccounting::new(run_start, ui.output_route().label());
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
    while (secs == 0 || run_start.elapsed().as_secs() < secs)
        && preview_scroll_exit_at.is_none_or(|deadline| Instant::now() < deadline)
    {
        if catalog_publication_test.wait_for_first_frame_release(Instant::now(), start) {
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }
        let loop_start = Instant::now();
        slint::platform::update_timers_and_animations();
        let mut full_bridge_dirty = false;
        if let Some(deadline) = display_confirm_deadline {
            nav.display_confirm_remaining = if loop_start >= deadline {
                0
            } else {
                ((deadline - loop_start).as_millis().div_ceil(1000) as u8).min(10)
            };
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
        let prepare_trace_enabled = frame_accounting.preview_scroll_trace_enabled();
        let mut prepare_trace = LauncherPrepareTrace::default();
        lifecycle.tick_startup_reveal(loop_start, catalog_ready, &mut lifecycle_effects);
        apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
        sync_startup_visibility(&app, &lifecycle);
        scheduler.record_loading_frame(loop_start);
        let compatibility_active = launcher_presenter.compatibility_failure().is_some();
        let launching =
            scheduler.launch_is_active() || !loading_title.is_empty() || compatibility_active;
        let setup_active = setup.is_active();
        let mut light_bridge_dirty = false;
        let mut pad_changed_for_input = if !launching && lifecycle.startup_input_enabled() {
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
        let route_action = display_session.begin_frame(frames, launching, f);
        // The catalog contention harness first proves one exact preview, then
        // freezes further selected-preview work so frame failures can be
        // attributed to the catalog rather than an independent image decode.
        let defer_selected_preview =
            catalog_contention_quiet_previews && preview.trace_cache_state() == "exact";
        let mut preview_scheduled_this_loop = false;
        if last_clock_update.elapsed() >= Duration::from_secs(1) {
            let clock_text = launcher_clock_text();
            if dirty_opt {
                if clock_text != last_clock_text {
                    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                    bridge.set_clock_text(clock_text.clone().into());
                    last_clock_text = clock_text;
                    light_bridge_dirty = true;
                }
            } else {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                bridge.set_clock_text(clock_text.clone().into());
                last_clock_text = clock_text;
                full_bridge_dirty = true;
            }
            last_clock_update = Instant::now();
        }
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
        let pad_changed_for_background = pad_changed_for_input.unwrap_or(false);
        let catalog_background_input = CatalogBackgroundIdleInput {
            first_visible_copy_done: frame_accounting.first_visible_copy_done(),
            startup_return_waiting_for_catalog,
            startup_input_enabled: lifecycle.startup_input_enabled(),
            launching,
            setup_active,
            benchmark_active: launcher_bench_active,
            scripted_input_active: launcher_input_script.active(),
            pad_changed: pad_changed_for_background,
            pad_active: pad_state_has_active_input(pad.state()),
            catalog_publication_pending: pending_catalog_ready.is_some()
                || deferred_catalog_events
                    .iter()
                    .any(catalog_message_requires_publication_pause),
            media_message_seen: media_message_seen_last_loop,
            nav_motion_active: catalog_background_nav_motion_active(&nav),
            preview_critical: nav.screen == Screen::Arcade
                && selected_arcade_game_has_preview(&nav, &catalog)
                && !matches!(preview.trace_cache_state(), "exact" | "empty"),
            visual_animation_active: slint_animation_active,
        };
        let catalog_background_allowed =
            catalog_background_idle_gate.allow(catalog_background_input, loop_start);
        mister_magik_catalog::builder_service::set_background_heavy_work_allowed(
            catalog_background_allowed,
        );
        scheduler.set_search_index_allowed(catalog_background_allowed);
        if catalog_ready && nav.screen == Screen::Home {
            for (index, system_id) in nav.collection_prefetch_order().into_iter().enumerate() {
                if collection_has_resident_rows(&catalog, &system_id) {
                    continue;
                }
                let priority = if index == 0 {
                    SystemShardPriority::Selected
                } else {
                    SystemShardPriority::Prefetch
                };
                if scheduler.request_system_shard(
                    system_id.clone(),
                    priority,
                    if index == 0 {
                        "home-highlight"
                    } else {
                        "home-neighbor"
                    },
                    loop_start,
                ) {
                    nav.catalog_system_hydration_started(&system_id);
                    full_bridge_dirty = true;
                }
            }
        }
        let search_index_effects = catalog_session
            .maybe_start_search_index(catalog_background_allowed, scheduler.search_index_running());
        apply_catalog_session_effects(
            search_index_effects,
            &app,
            &mut nav,
            &mut catalog,
            &mut catalog_ready,
            &mut catalog_version,
            &mut return_capsule_active,
            &mut catalog_generation,
            &mut pending_launch_return_state,
            &mut preview,
            &mut media_session,
            &mut scheduler,
            &mut lifecycle,
            &mut lifecycle_effects,
            &mut full_bridge_dirty,
            loop_start,
            start,
        );
        let deferred_worker_policy = deferred_catalog_worker_start_policy(
            catalog_ready,
            frame_accounting.first_visible_copy_done(),
            startup_return_waiting_for_catalog,
            catalog_background_allowed,
            lifecycle.catalog_worker_start_delay(catalog_background_validation_delay()),
        );
        if let Some(worker) = catalog_session.maybe_start_deferred_worker(
            scheduler.catalog_worker_running(),
            frame_accounting.first_visible_copy_done() || startup_return_waiting_for_catalog,
            deferred_worker_policy.allowed && catalog_publication_test.catalog_worker_allowed(),
            loop_start,
            deferred_worker_policy.delay,
            catalog_builder_lock_available,
        ) {
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

        if let Some(message) = catalog_publication_test.tick(loop_start, start) {
            deferred_catalog_events.push_back(message);
        }
        if catalog_messages_need_polling(
            pending_catalog_ready.is_some(),
            catalog_session.refresh_done(),
            scheduler.catalog_messages_running() || !deferred_catalog_events.is_empty(),
        ) {
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
                        &mut pending_launch_return_state,
                        &mut preview,
                        &mut media_session,
                        &mut scheduler,
                        &mut catalog_session,
                        &mut lifecycle,
                        &mut lifecycle_effects,
                        &mut full_bridge_dirty,
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
                    &mut pending_launch_return_state,
                    &mut preview,
                    &mut media_session,
                    &mut scheduler,
                    &mut catalog_session,
                    &mut lifecycle,
                    &mut lifecycle_effects,
                    &mut full_bridge_dirty,
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
                    &mut pending_launch_return_state,
                    &mut preview,
                    &mut media_session,
                    &mut scheduler,
                    &mut catalog_session,
                    &mut lifecycle,
                    &mut lifecycle_effects,
                    &mut full_bridge_dirty,
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
        let media_worker_trace_start = prepare_trace_enabled.then(Instant::now);
        let mut media_message_seen = false;
        if preview_route.allows_preview_work() {
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
        media_message_seen_last_loop = media_message_seen;

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

        if commit_pending_collection_entry(&mut pending_collection_entry, &mut nav, &catalog, start)
        {
            arcade_entry_latency.record_rows_ready(start, loop_start, &lifecycle, &catalog, &nav);
            full_bridge_dirty = true;
            request_launcher_redraw!();
        } else if pending_collection_entry
            .as_ref()
            .is_some_and(|entry| nav.catalog_system_has_failed(&entry.collection_id))
        {
            if let Some(entry) = pending_collection_entry.take() {
                nav.catalog_system_hydration_finished(&entry.collection_id);
                nav.restore_pending_home_view(entry.source);
                arcade_entry_latency.cancel_enter();
                print_startup_event(
                    start,
                    "catalog_system_entry_failed",
                    format!("system={}", entry.collection_id),
                );
                full_bridge_dirty = true;
            }
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

        if let Some(scenario) = launcher_bench_scenario {
            if launcher_bench_after_input_script
                && !launcher_bench_active
                && !launcher_input_script.active()
                && nav.screen == Screen::Arcade
                && arcade_navigation_ready(catalog_ready, &catalog)
            {
                run_start = Instant::now();
                frame_accounting =
                    LauncherFrameAccounting::new(run_start, ui.output_route().label());
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
                && !compatibility_active
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
                && !compatibility_active
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

        let catalog_build_busy =
            scheduler.catalog_worker_running() || !catalog_session.refresh_done();
        let restore_before = screensaver.restore_full_frame;
        screensaver.update(
            Instant::now(),
            nav.settings.screensaver_enabled,
            Duration::from_secs(u64::from(nav.settings.screensaver_delay_minutes) * 60),
            catalog_build_busy,
        );
        if !restore_before && screensaver.restore_full_frame {
            request_launcher_redraw!();
        }

        if !launching && lifecycle.startup_input_enabled() {
            let pad_changed = pad_changed_for_input
                .take()
                .unwrap_or_else(|| pad.poll_with_debug_labels(setup_active));
            let screensaver_input_activity = pad.user_activity();
            let frame_now = Instant::now();

            if setup_active && setup.target_pad_idx >= pad.len() {
                crate::ui_errln!(
                    "controller setup: pad {} disappeared; closing setup flow",
                    setup.target_pad_idx
                );
                setup.advance_to_next_pad(&pad);
                full_bridge_dirty = true;
            }

            let input_session = ControllerSetupInputSession::new(&pad, &setup);
            let launcher_state = input_session.launcher_state().clone();
            if screensaver.handle_input(
                frame_now,
                pad_state_has_active_input(&launcher_state),
                screensaver_input_activity,
            ) {
                nav.absorb_input(&launcher_state);
                request_launcher_redraw!();
                continue;
            }
            let setup_state = input_session.setup_state();
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
                    let mut nav_state = launcher_state.clone();
                    if let Some(test_state) =
                        library_changed_dialog_test.input_for(&nav, loop_start, start)
                    {
                        nav_state = test_state;
                    }
                    if let Some(script_state) = launcher_input_script.input_for() {
                        nav_state = script_state;
                    }
                    let lifecycle_view = lifecycle.view();
                    let launch_failure_visible = lifecycle_view.launch_failure_dialog().is_some();
                    let recovery_dialog_visible =
                        lifecycle_view.catalog_recovery_dialog().is_some();
                    let recovery_prev =
                        std::mem::replace(&mut catalog_recovery_prev, nav_state.clone());
                    if cancel_pending_collection_entry_for_input(
                        &mut pending_collection_entry,
                        &mut nav,
                        &nav_state,
                        &recovery_prev,
                        start,
                    ) {
                        arcade_entry_latency.cancel_enter();
                    }
                    let event = if launch_failure_visible {
                        if (nav_state.btn_a && !recovery_prev.btn_a)
                            || (nav_state.btn_b && !recovery_prev.btn_b)
                            || (nav_state.btn_home && !recovery_prev.btn_home)
                        {
                            lifecycle.handle(
                                LauncherLifecycleInput::LaunchFailureAcknowledge,
                                &mut lifecycle_effects,
                            );
                            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                            full_bridge_dirty = true;
                        }
                        None
                    } else if recovery_dialog_visible {
                        let recovery_input = if nav_state.dpad_left && !recovery_prev.dpad_left {
                            Some(LauncherLifecycleInput::CatalogRecoveryLeft)
                        } else if nav_state.dpad_right && !recovery_prev.dpad_right {
                            Some(LauncherLifecycleInput::CatalogRecoveryRight)
                        } else if nav_state.btn_a && !recovery_prev.btn_a {
                            Some(LauncherLifecycleInput::CatalogRecoveryConfirm)
                        } else {
                            None
                        };
                        if let Some(input) = recovery_input {
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
                            });
                        auto_launch_selected_done = event.is_some();
                        event
                    } else if scheduler.launch_benchmark_enabled() {
                        None
                    } else {
                        nav.handle_input_with_collection_intents(&nav_state, frame_now, &catalog)
                    };
                    if let Some(event) = event {
                        match event.action {
                            LauncherAction::OpenCollection => {
                                let Some(collection_id) = event.path.as_deref() else {
                                    continue;
                                };
                                if collection_has_resident_rows(&catalog, &collection_id) {
                                    if nav.activate_collection(&catalog, &collection_id) {
                                        print_startup_event(
                                            start,
                                            "catalog_system_entry_immediate",
                                            format!(
                                                "system={collection_id} resident_rows={}",
                                                catalog.system_game_count(&collection_id)
                                            ),
                                        );
                                        full_bridge_dirty = true;
                                        request_launcher_redraw!();
                                    }
                                } else {
                                    let requested_at = Instant::now();
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
                                    if nav.catalog_system_has_failed(&collection_id) {
                                        nav.catalog_system_retry_started(&collection_id);
                                        let _ = scheduler.retry_system_shard(
                                            collection_id.to_string(),
                                            "explicit-retry",
                                            requested_at,
                                        );
                                    } else {
                                        nav.catalog_system_hydration_started(&collection_id);
                                        let _ = scheduler.request_system_shard(
                                            collection_id.to_string(),
                                            SystemShardPriority::Urgent,
                                            "open-collection",
                                            requested_at,
                                        );
                                    }
                                    print_startup_event(
                                        start,
                                        "catalog_system_entry_pending",
                                        format!("system={collection_id}"),
                                    );
                                    full_bridge_dirty = true;
                                    request_launcher_redraw!();
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
                            LauncherAction::ResetDatabase => {
                                loading_title = "Shutting down…".to_string();
                                apply_screenshot_media_update_effects(
                                    media_session.shutdown_for_reset(),
                                    &app,
                                    &mut catalog,
                                    &mut scheduler,
                                    Some(&mut preview),
                                    &mut full_bridge_dirty,
                                    start,
                                );
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
                                match launcher::reset_catalog_and_reboot() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        crate::ui_errln!("reset database failed: {e}");
                                        loading_title.clear();
                                    }
                                }
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
                                    &mut pending_launch_return_state,
                                    &mut preview,
                                    &mut media_session,
                                    &mut scheduler,
                                    &mut lifecycle,
                                    &mut lifecycle_effects,
                                    &mut full_bridge_dirty,
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
                                    &mut pending_launch_return_state,
                                    &mut preview,
                                    &mut media_session,
                                    &mut scheduler,
                                    &mut lifecycle,
                                    &mut lifecycle_effects,
                                    &mut full_bridge_dirty,
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
                            LauncherAction::LaunchGame => {}
                        }
                        if event.action == LauncherAction::LaunchGame {
                            let Some(mra) = event.path else {
                                continue;
                            };
                            if scheduler.launch_is_active() {
                                continue;
                            }
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
            let _ = pad.poll();
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

        sync_settings_bridge(&app, &nav, &lifecycle);
        match launcher_bridge_sync_plan(
            launching,
            lifecycle.startup_input_enabled(),
            full_bridge_dirty,
            light_bridge_dirty,
        ) {
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
                    defer_selected_preview,
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
                    defer_selected_preview,
                    ui,
                );
                preview_scheduled_this_loop = nav.screen == Screen::Arcade;
                request_launcher_redraw!();
            }
            LauncherBridgeSyncPlan::None => {}
        }
        sync_startup_visibility(&app, &lifecycle);

        let media_gate_trace_start = prepare_trace_enabled.then(Instant::now);
        {
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
        let status_string_copy_start = (status_write_due
            && frame_accounting.preview_scroll_trace_enabled())
        .then(Instant::now);
        let status_text =
            status_write_due.then(|| LauncherStatusTextSnapshot::from_bridge(&bridge));
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
                defer_selected_preview,
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
            && !arcade_search_active
            && !memory_guard.active()
            && preview_route.allows_preview_work()
        {
            let dirty = apply_ready_preview(
                &app,
                &mut preview,
                defer_selected_preview,
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
        );
        apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
        sync_startup_visibility(&app, &lifecycle);
        let startup_reveal_ready =
            lifecycle.startup_status().state == StartupRevealState::RevealLauncher;
        let mut full_frame_present = display_session
            .should_present_full_frame(launching, route_action)
            || startup_reveal_ready;
        let wants_arcade_list = !screensaver.active
            && should_draw_arcade_overlay(&nav, launching, active_arcade_games_available);
        let wants_preview = preview_route.allows_preview_work()
            && !screensaver.active
            && direct_preview_requested(
                nav.screen,
                memory_guard.active(),
                preview.raw_transition_frame().is_some(),
            );
        let preview_frame_status = preview.raw_frame_status();
        let preview_cache_state_before_composition = preview.trace_cache_state();
        let composition_decision = composition.tick(UiCompositionInput {
            screen: nav.screen,
            screensaver_active: screensaver.active,
            confirm_visible,
            fullscreen_overlay_visible: catalog_scan_visible,
            arcade_ready: active_arcade_games_available,
            route_ok: display_session.route_ok(),
            wants_arcade_list,
            wants_preview,
            preview_cache_state: preview_cache_state_before_composition,
            preview_frame_status,
        });
        if screensaver.active {
            full_frame_present = true;
            request_launcher_redraw!();
        } else if screensaver.start_when_ready {
            request_launcher_redraw!();
        }
        for event in composition_decision.events.iter() {
            runtime_status::event(event.name, event.detail.as_str());
        }
        if composition_decision.force_full_slint_present {
            full_frame_present = true;
        }
        if composition_decision.clear_direct_layers {
            arcade_list_renderer.invalidate_presented_layer();
            if !composition_decision.allow_preview_blit {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                preview.clear(&bridge);
            }
            request_launcher_redraw!();
        }
        let startup_status = lifecycle.startup_status();
        let composition_status = composition_decision.status();
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
        let stream_motion_before_render = slint_animation_active
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
            launcher_input_script.active(),
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
        wake_reasons.insert_if(
            LauncherWakeReasons::FB0_ROUTE_RECOVERY_PENDING,
            launcher_presenter.needs_frame(),
        );
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
            FPGA_LATCH_LATE_FRAME_START_HEADROOM_US
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
        update_slint_animations(animation_clock);
        let mut layer_target = LayerTarget::new(target, ui);
        let cpu_t1 = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let frame_t1 = Instant::now();
        if screensaver.take_restore_full_frame() {
            if let Some(snapshot) = screensaver_launcher_frame.take() {
                if !layer_target.restore_cached(&snapshot) {
                    crate::ui_errln!(
                        "screensaver: launcher frame restore size mismatch snapshot={} cached={}",
                        snapshot.len(),
                        layer_target.cached_frame_view().pixels().len()
                    );
                }
            }
            window.request_redraw();
            full_frame_present = true;
        }
        if screensaver.active && screensaver_renderer.is_none() {
            if screensaver_loader.is_none() {
                if let Some(started) = screensaver_show_started {
                    crate::ui_logln!(
                        "screensaver_startup_timing milestone=loader_started elapsed_us={}",
                        started.elapsed().as_micros()
                    );
                }
                screensaver_loader = Some(LauncherScreensaverLoader::start(
                    ui.render_w(),
                    ui.render_h(),
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
                screensaver_renderer = Some(ready);
                screensaver_loader = None;
            }
        }
        if !screensaver.active {
            screensaver_loader = None;
            if screensaver_renderer
                .as_ref()
                .is_some_and(LauncherScreensaver::is_loading_archive)
            {
                screensaver_renderer = None;
            }
            screensaver_launcher_frame = None;
        }
        let screensaver_fade_alpha = screensaver.preview_fade_alpha(Instant::now());
        if screensaver.active
            && (screensaver_renderer.is_some() || screensaver_fade_alpha.is_some())
            && screensaver_launcher_frame.is_none()
        {
            screensaver_launcher_frame = Some(layer_target.snapshot_cached());
        }
        let this_rect =
            if screensaver.active && screensaver_fade_alpha.is_some_and(|alpha| alpha < 255) {
                let alpha = screensaver_fade_alpha.expect("checked above");
                if let Some(renderer) = screensaver_renderer.as_mut() {
                    Some(
                        layer_target.render_screensaver_crossfade(
                            renderer,
                            screensaver_launcher_frame
                                .as_deref()
                                .expect("captured above"),
                            alpha,
                        ),
                    )
                } else {
                    Some(
                        layer_target.render_screensaver_fade(
                            screensaver_launcher_frame
                                .as_deref()
                                .expect("captured above"),
                            alpha,
                        ),
                    )
                }
            } else if screensaver.active && screensaver_renderer.is_some() {
                if screensaver_launcher_frame.is_none() {
                    screensaver_launcher_frame = Some(layer_target.snapshot_cached());
                }
                Some(
                    layer_target
                        .render_screensaver(screensaver_renderer.as_mut().expect("checked above")),
                )
            } else if screensaver.active && screensaver_fade_alpha.is_some() {
                Some(
                    layer_target.render_screensaver_fade(
                        screensaver_launcher_frame
                            .as_deref()
                            .expect("captured above"),
                        255,
                    ),
                )
            } else {
                expand_home_pan_dirty_rect(
                    layer_target.render_slint_base(&window),
                    ui,
                    home_pan_present_active,
                )
            };
        if screensaver.active && screensaver_renderer.is_some() && !screensaver_first_render_logged
        {
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
            arcade_list_renderer.set_geometry_for_render_h(
                if nav.uses_crt_layout() {
                    ArcadeListGeometry::crt_for_content(
                        ui.content_rect(),
                        CrtUiMetrics::for_display(ui),
                        arcade_search_active,
                    )
                } else if arcade_search_active {
                    ArcadeListGeometry::search_for_render_w(ui.render_w())
                } else {
                    ArcadeListGeometry::NORMAL
                },
                if nav.uses_crt_layout() {
                    ui.content_rect().bottom()
                } else {
                    ui.render_h()
                },
            );
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
        let effect_label_us = 0;
        let custom_draw_trace = LauncherCustomDrawTrace {
            arcade_list_update_us,
            preview_blit_us,
            effect_label_us,
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
            x1: ui.render_w(),
            y1: ui.render_h(),
        };
        let base_damage = if full_frame_present {
            Some(full_rect)
        } else {
            this_rect
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
        let crt_arcade_cached_rect = if crt_layout {
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
        let preview_desired = if preview_layer_desired
            && preview_direct_present_enabled()
            && preview_frame_status == PreviewRawFrameStatus::Ready
        {
            Some(DirectLayerState::new(
                preview_screen_rect(ui),
                launcher_preview_version,
            ))
        } else {
            None
        };
        let arcade_desired = if !crt_layout
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
        let mut cached_damage = DirtyRectList::new();
        cached_damage.push_if_some(base_damage);
        cached_damage.push_if_some(raw_preview_cached_rect);
        cached_damage.push_if_some(crt_arcade_cached_rect);
        let frame_plan = LauncherFramePlan::new(
            cached_damage,
            preview_desired,
            raw_preview_direct_rect,
            arcade_desired,
            if crt_layout { None } else { arcade_list_rect },
        );
        let startup_can_present = lifecycle.startup_can_present_frame();
        let stream_motion_active = stream_motion_before_render || preview_transition_trace.active;
        let present_cycle = launcher_presenter.present(
            LauncherPresentFrame {
                plan: frame_plan,
                startup_can_present,
                first_visible_copy_done: frame_accounting.first_visible_copy_done(),
                frame_start_phase_us,
                pre_render_pace,
                frame_analytics_mode,
                stream_motion_active,
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
        if let Some(failure) = launcher_presenter.compatibility_failure() {
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            bridge.set_compatibility_visible(true);
            bridge.set_compatibility_reason(failure.reason_code().into());
            bridge.set_compatibility_detail(failure.detail.as_str().into());
            request_launcher_redraw!();
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
        if presentation.copied_rows > 0 {
            if screensaver.active
                && screensaver_first_render_logged
                && !screensaver_first_present_logged
            {
                screensaver_first_present_logged = true;
                if let Some(started) = screensaver_show_started {
                    crate::ui_logln!(
                        "screensaver_startup_timing milestone=first_saver_present elapsed_us={}",
                        started.elapsed().as_micros()
                    );
                }
            }
            if screensaver.active
                && !screensaver_first_card_present_logged
                && screensaver_renderer
                    .as_ref()
                    .is_some_and(LauncherScreensaver::has_rendered_card)
            {
                screensaver_first_card_present_logged = true;
                if let Some(started) = screensaver_show_started {
                    crate::ui_logln!(
                        "screensaver_startup_timing milestone=first_card_visible elapsed_us={}",
                        started.elapsed().as_micros()
                    );
                }
            }
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
                selected: nav.arcade.selected,
                visual_index: nav.arcade.visual_index,
                #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
                home_trace: LauncherHomeFrameTrace::from_nav(&nav),
                search_index_state: if catalog.text_indexes_ready() {
                    "ready"
                } else {
                    "building"
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
            },
            pacing: pacing_trace,
            presentation,
            status: LauncherFrameStatusData {
                status_write_due,
                status_string_copy_us,
                status_string_copy_bytes,
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
        if latch_trace_flush_deferred {
            let finish_timing = frame_accounting.finish_frame_before_trace(
                &presented_frame,
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
            );
            // Latch mode posts the hidden buffer first, then spends the slack before
            // vblank on normal per-frame accounting. The final wait is only the
            // pacing boundary for the next frame.
            let wait_start = Instant::now();
            let pace = pacer.wait();
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
                latch_trace_flush_deferred,
            );
        }
        frames += 1;
    }
    // Do not leak the launcher's last interactive gate state into a later
    // launcher run in the same process (notably host tests and diagnostic
    // runners). Normal device execution exits here, but the global policy is
    // deliberately restored to its permissive default for lifecycle safety.
    mister_magik_catalog::builder_service::set_background_heavy_work_allowed(true);
    let elapsed = run_start.elapsed().as_secs_f64();
    crate::ui_logln!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu) {
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
    pending_launch_return_state: &mut Option<launcher::LaunchReturnState>,
    preview: &mut PreviewState,
    media_session: &mut ScreenshotMediaUpdateSession,
    scheduler: &mut LauncherScheduler,
    catalog_session: &mut LauncherCatalogSession,
    lifecycle: &mut LauncherLifecycle,
    lifecycle_effects: &mut LifecycleEffects,
    full_bridge_dirty: &mut bool,
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
        pending_launch_return_state,
        preview,
        media_session,
        scheduler,
        lifecycle,
        lifecycle_effects,
        full_bridge_dirty,
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
        CatalogWorkerMessage::Ready { catalog, .. } if !catalog.text_indexes_ready()
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

fn return_to_launcher_env_is_set(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true") | Some("yes"))
}

fn apply_pending_launch_return_state(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    pending: &mut Option<launcher::LaunchReturnState>,
) -> bool {
    let Some(state) = pending.as_ref().cloned() else {
        return false;
    };
    if launcher::apply_launch_return_state(nav, catalog, state) {
        pending.take();
        true
    } else {
        false
    }
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
) {
    if lifecycle.startup_status().mode != StartupMode::ReturnFromGame {
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
        },
        effects,
    );
    if return_preview_ready(nav, catalog, preview) {
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
) {
    let status = lifecycle.startup_status();
    if status.mode != StartupMode::ReturnFromGame
        || status.state != StartupRevealState::WaitRelevantPreview
        || !return_preview_ready(nav, catalog, preview)
    {
        return;
    }
    lifecycle.handle(
        LauncherLifecycleInput::StartupReturnPreviewReady {
            preview_state: preview.trace_cache_state(),
        },
        effects,
    );
}

fn return_preview_ready(
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
    preview: &PreviewState,
) -> bool {
    if nav.screen != Screen::Arcade {
        return true;
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
                    CatalogWorkerRequest::ForceBuild,
                    CatalogWorkerInitialCache::AlreadyProbedMissing,
                    CatalogExecutionMode::ForegroundExclusive,
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
        }
    }
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
    pending_launch_return_state: &mut Option<launcher::LaunchReturnState>,
    preview: &mut PreviewState,
    media_session: &mut ScreenshotMediaUpdateSession,
    scheduler: &mut LauncherScheduler,
    lifecycle: &mut LauncherLifecycle,
    lifecycle_effects: &mut LifecycleEffects,
    full_bridge_dirty: &mut bool,
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
                *catalog = nav.catalog_with_build_shells(ready_catalog);
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
                    apply_pending_launch_return_state(nav, catalog, pending_launch_return_state);
                if return_restored {
                    emit_return_context_restored(
                        lifecycle,
                        lifecycle_effects,
                        nav,
                        catalog,
                        preview,
                    );
                    // The preview hold is measured from return startup. If
                    // navigation hydration already consumed that budget,
                    // transition to reveal in this same worker-message turn.
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
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                preview.clear(&bridge);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::StartSearchIndex { job, games, source } => {
                print_startup_event(
                    start,
                    "arcade_search_index_scheduled",
                    format!(
                        "games={games} source={} after=launcher_idle",
                        source.label()
                    ),
                );
                scheduler.start_search_index(job, games, source);
            }
            CatalogSessionEffect::SearchIndexesReady {
                text_index_token,
                games,
                source,
                timing,
            } => {
                if catalog.text_index_token() != text_index_token || !catalog.text_indexes_ready() {
                    print_startup_event(
                        start,
                        "arcade_search_index_stale_ignored",
                        format!("token={text_index_token}"),
                    );
                    continue;
                }
                print_startup_event(
                    start,
                    "arcade_search_index_ready",
                    format!(
                        "token={text_index_token} built={} games={games} elapsed_us={} source={} search_keys_us={} autocomplete_us={}",
                        u8::from(timing.built),
                        timing.total_us,
                        source.label(),
                        timing.search_keys_us,
                        timing.autocomplete_us
                    ),
                );
                let return_restored =
                    apply_pending_launch_return_state(nav, catalog, pending_launch_return_state);
                if return_restored {
                    emit_return_context_restored(
                        lifecycle,
                        lifecycle_effects,
                        nav,
                        catalog,
                        preview,
                    );
                    lifecycle.tick_startup_reveal(now, true, lifecycle_effects);
                }
                if let Some(system_id) = active_system(catalog, nav).map(|system| system.id.clone())
                {
                    nav.refresh_arcade_search_if_active(catalog, &system_id);
                    *full_bridge_dirty = true;
                }
            }
            CatalogSessionEffect::SyncCatalogBridge => {
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogBuildStarted => {
                nav.catalog_build_started();
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogSystemDiscovered { system_id } => {
                nav.catalog_system_discovered(&system_id);
                *catalog = catalog.with_system_placeholder(&system_id);
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogSystemReady { system_id } => {
                nav.catalog_system_hydration_finished(&system_id);
                nav.catalog_system_ready(&system_id);
                *catalog_version = (*catalog_version).wrapping_add(1);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogSystemFailed { system_id } => {
                nav.catalog_system_hydration_finished(&system_id);
                nav.catalog_system_failed(&system_id);
                *catalog = catalog.with_system_placeholder(&system_id);
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::CatalogBuildFinished => {
                nav.catalog_build_finished(catalog);
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
                *full_bridge_dirty = true;
            }
            CatalogSessionEffect::Ui(intent) => {
                apply_launcher_worker_ui_intent(app, intent, full_bridge_dirty);
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
                let (replacement, launch_plans) = arcade_rows_from_shard(&system_id, &games);
                *catalog = catalog.replacing_system_games(&system_id, replacement, launch_plans);
                *catalog_version = (*catalog_version).wrapping_add(1);
                nav.sync_launcher_taxonomy(catalog);
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
                request: CatalogWorkerRequest::ForceBuild,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            };
        }
        CatalogStartupSqliteState::ExistingUnusable => {
            return CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request: CatalogWorkerRequest::ForceBuild,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            };
        }
        CatalogStartupSqliteState::Missing => {}
    }
    if catalog_worker_enabled {
        return CatalogStartupWithoutSummaryPlan::DeferredWorker {
            request: CatalogWorkerRequest::ForceBuild,
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
    background_allowed: bool,
    background_delay: Duration,
) -> DeferredCatalogWorkerStartPolicy {
    if catalog_ready {
        DeferredCatalogWorkerStartPolicy {
            allowed: background_allowed,
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
                CatalogWorkerRequest::ForceBuild | CatalogWorkerRequest::FreshBuild
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
        CatalogWorkerRequest::ForceBuild
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
        return Some(CatalogWorkerRequest::ForceBuild);
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
    request == CatalogWorkerRequest::ForceBuild || return_catalog_hydration_needed
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
            screen: Screen::Arcade,
            screensaver_active: false,
            confirm_visible: false,
            fullscreen_overlay_visible: false,
            arcade_ready: true,
            route_ok: true,
            wants_arcade_list: true,
            wants_preview: false,
            preview_cache_state: "empty",
            preview_frame_status: PreviewRawFrameStatus::Empty,
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
                LauncherPresentStatus::Compatibility,
            ),
            "Mode=compatibility"
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
            CatalogWorkerRequest::ForceBuild
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
            Some(CatalogWorkerRequest::ForceBuild)
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
            CatalogWorkerRequest::ForceBuild,
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
            summary_seed_catalog_worker_initial_cache(CatalogWorkerRequest::ForceBuild, false),
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
                request: CatalogWorkerRequest::ForceBuild,
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
                request: CatalogWorkerRequest::ForceBuild,
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
                request: CatalogWorkerRequest::ForceBuild,
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
                request: CatalogWorkerRequest::ForceBuild,
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
                request: CatalogWorkerRequest::ForceBuild,
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
                request: CatalogWorkerRequest::ForceBuild,
                initial_cache: CatalogWorkerInitialCache::AlreadyProbedMissing,
                execution_mode: CatalogExecutionMode::ForegroundExclusive,
            },
            "an explicit force request may rebuild the unusable catalog"
        );

        let _ = std::fs::remove_dir_all(root);
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
                CatalogWorkerRequest::ForceBuild,
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
    pub(super) fn warm_catalog_worker_keeps_background_idle_policy() {
        let delay = Duration::from_secs(2);
        let blocked = deferred_catalog_worker_start_policy(true, true, false, false, delay);
        assert!(!blocked.allowed);
        assert_eq!(blocked.delay, delay);
        assert!(!blocked.foreground);

        let allowed = deferred_catalog_worker_start_policy(true, true, false, true, delay);
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

    fn idle_catalog_background_input() -> CatalogBackgroundIdleInput {
        CatalogBackgroundIdleInput {
            first_visible_copy_done: true,
            startup_input_enabled: true,
            ..CatalogBackgroundIdleInput::default()
        }
    }

    #[test]
    pub(super) fn catalog_background_worker_requires_continuous_idle_settle() {
        let now = Instant::now();
        let mut gate = CatalogBackgroundIdleGate::new(Duration::from_secs(2));
        let input = idle_catalog_background_input();

        assert!(!gate.allow(input, now));
        assert!(!gate.allow(input, now + Duration::from_millis(500)));
        assert!(gate.allow(input, now + Duration::from_millis(2000)));
    }

    #[test]
    pub(super) fn catalog_interaction_idle_ignores_visual_only_slint_animation() {
        let now = Instant::now();
        let mut gate = CatalogBackgroundIdleGate::new(Duration::from_secs(2));
        let input = CatalogBackgroundIdleInput {
            visual_animation_active: true,
            ..idle_catalog_background_input()
        };

        let render_intent = LauncherRenderIntent {
            first_visible_copy_done: true,
            startup_input_enabled: true,
            wake_reasons: LauncherWakeReasons::SLINT_ANIMATION_ACTIVE,
        };
        assert!(!render_intent.can_sleep());
        assert!(!gate.allow(input, now));
        assert!(gate.allow(input, now + Duration::from_secs(2)));
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
    pub(super) fn catalog_background_worker_resets_on_human_sized_pause_activity() {
        let now = Instant::now();
        let mut gate = CatalogBackgroundIdleGate::new(Duration::from_secs(2));
        let input = idle_catalog_background_input();

        assert!(!gate.allow(input, now));
        assert!(!gate.allow(input, now + Duration::from_millis(1900)));
        assert!(!gate.allow(
            CatalogBackgroundIdleInput {
                pad_changed: true,
                ..input
            },
            now + Duration::from_millis(1901)
        ));
        assert!(!gate.allow(input, now + Duration::from_millis(2401)));
        assert!(gate.allow(input, now + Duration::from_millis(4401)));
    }

    #[test]
    pub(super) fn catalog_progress_does_not_pause_its_own_background_worker() {
        let progress = CatalogWorkerMessage::Progress {
            title: "Indexing library".to_string(),
            detail: "Still working".to_string(),
            percent: -1,
        };
        assert!(!catalog_message_requires_publication_pause(&progress));

        let ready = CatalogWorkerMessage::Ready {
            catalog: ArcadeCatalog::new(PathBuf::from("/fixture"), Vec::new(), Vec::new()),
            summary: None,
            load_us: 0,
            source: CatalogSource::FreshBuild,
            durable_save_pending: true,
            generation_fingerprint: None,
            publication_ack: None,
        };
        assert!(catalog_message_requires_publication_pause(&ready));
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
    pub(super) fn catalog_background_worker_blocks_global_activity() {
        let now = Instant::now();
        for active in [
            CatalogBackgroundIdleInput {
                benchmark_active: true,
                ..idle_catalog_background_input()
            },
            CatalogBackgroundIdleInput {
                scripted_input_active: true,
                ..idle_catalog_background_input()
            },
            CatalogBackgroundIdleInput {
                nav_motion_active: true,
                ..idle_catalog_background_input()
            },
            CatalogBackgroundIdleInput {
                preview_critical: true,
                ..idle_catalog_background_input()
            },
        ] {
            let mut gate = CatalogBackgroundIdleGate::new(Duration::from_secs(2));
            assert!(!gate.allow(active, now + Duration::from_secs(3)));
        }
    }

    #[test]
    fn forced_hydration_with_a_usable_catalog_stays_background() {
        assert_eq!(
            catalog_hydration_execution_mode(CatalogWorkerRequest::ForceBuild),
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
        let mut saver = ScreensaverControl::new(start, false);
        let delay = Duration::from_secs(300);

        assert!(!saver.handle_input(start + Duration::from_secs(250), false, true));
        saver.update(start + Duration::from_secs(500), true, delay, false);
        assert!(!saver.active);
        saver.update(start + Duration::from_secs(551), true, delay, false);
        assert!(saver.active);

        saver.update(start + Duration::from_secs(552), true, delay, true);
        assert!(!saver.active);
        assert!(saver.take_restore_full_frame());
        saver.update(start + Duration::from_secs(851), true, delay, false);
        assert!(!saver.active);
        saver.update(start + Duration::from_secs(852), true, delay, false);
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
            remaining: 10,
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
        assert_eq!(ui_nav.display_confirm_remaining, 10);
        assert_eq!(deadline, Some(now + Duration::from_secs(10)));

        let mut headless_nav = LauncherNav::new();
        assert_eq!(
            apply_startup_pending_display(&mut headless_nav, &state, false, now),
            None
        );
        assert_eq!(headless_nav.screen, Screen::Home);
        assert_eq!(headless_nav.confirm_action, None);
    }

    #[test]
    fn screensaver_start_active_keeps_waiting_for_startup_catalog_work() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, true);
        let delay = Duration::from_secs(300);

        saver.update(start, true, delay, true);
        assert!(!saver.active);
        assert!(saver.start_when_ready);
        saver.update(start + Duration::from_secs(1), true, delay, true);
        assert!(!saver.active);
        saver.update(start + Duration::from_secs(2), true, delay, false);
        assert!(saver.active);
        assert!(!saver.start_when_ready);
    }

    #[test]
    fn screensaver_preview_ignores_launch_release_then_consumes_next_input() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, false);

        saver.preview(start);
        saver.update(start, true, Duration::from_secs(300), true);
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
    fn disabled_screensaver_never_activates_but_preview_still_can() {
        let start = Instant::now();
        let mut saver = ScreensaverControl::new(start, false);

        saver.update(
            start + Duration::from_secs(600),
            false,
            Duration::from_secs(60),
            false,
        );
        assert!(!saver.active);
        saver.preview(start + Duration::from_secs(601));
        assert!(saver.active);
    }
}
