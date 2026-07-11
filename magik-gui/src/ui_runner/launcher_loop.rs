use super::launcher_frame_accounting::{
    FrameAnalyticsCpuStamp, LauncherCustomDrawTrace, LauncherFrameAccounting,
    LauncherFrameCpuTrace, LauncherFrameIdentity, LauncherFrameRenderData,
    LauncherFrameSnapshotBuilder, LauncherFrameStatusData, LauncherFrameTiming,
};
use super::launcher_pacing::{
    LauncherFramePacingInput, LauncherFramePacingPolicy, LauncherPacingTrace,
    FB0_LATE_FRAME_START_HEADROOM_US, FPGA_LATCH_LATE_FRAME_START_HEADROOM_US,
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
                    "launcher_present_backend_retired value={retired}; falling back to fb0-dirty"
                );
                boot_analytics::event(
                    "launcher_present_backend_retired",
                    format!("{retired} fallback=fb0-dirty"),
                );
                Self::Fb0Dirty
            }
            _ => Self::Fb0Dirty,
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
    if backend.is_latch() && status == LauncherPresentStatus::Ok {
        "Mode=latch"
    } else {
        "Mode=/dev/fb0"
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
                frame.map(|frame| frame.to_string()).unwrap_or_else(|| "-".to_string()),
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

    fn active_system_id(catalog: &ArcadeCatalog, nav: &LauncherNav) -> String {
        active_system(catalog, nav)
            .map(|system| system.id.clone())
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
        && !active_system_games_loading(catalog, nav)
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
    const CATALOG_SCAN_VISIBLE: Self = Self(1 << 9);
    const CATALOG_BACKGROUND_SCAN_VISIBLE: Self = Self(1 << 10);
    const CATALOG_SCAN_REDRAW_DUE: Self = Self(1 << 11);
    const CATALOG_GAMES_FOUND_DETAIL_CHANGED: Self = Self(1 << 12);
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
    Duration::from_micros(pacer.period_us().max(1))
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
    catalog_messages_active: bool,
    media_message_seen: bool,
    nav_motion_active: bool,
    preview_critical: bool,
}

impl CatalogBackgroundIdleInput {
    fn is_idle(self) -> bool {
        (self.first_visible_copy_done || self.startup_return_waiting_for_catalog)
            && self.startup_input_enabled
            && !self.launching
            && !self.setup_active
            && !self.benchmark_active
            && !self.scripted_input_active
            && !self.pad_changed
            && !self.pad_active
            && !self.catalog_messages_active
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
        if !input.is_idle() {
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
        || state.left_x.abs() > 0.0
        || state.left_y.abs() > 0.0
        || state.right_x.abs() > 0.0
        || state.right_y.abs() > 0.0
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
    ArcadeCatalog::new_with_deferred_text_indexes(
        PathBuf::from(root),
        hot_games,
        systems,
        Vec::new(),
    )
}

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
    window: &Rc<MinimalSoftwareWindow>,
    target: &mut UiFrameTarget,
    mut pad: PadPool,
    app: slint_ui::launcher::Launcher,
    animation_clock: &AnimationClock,
) {
    let start = Instant::now();
    let mut frames = 0u64;
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
    let mut catalog_ready_deferred_since: Option<Instant> = None;
    let mut catalog_ready_stationary_edge_since: Option<Instant> = None;
    let mut catalog_background_idle_gate =
        CatalogBackgroundIdleGate::new(CATALOG_BACKGROUND_IDLE_SETTLE);
    let mut media_events = MediaJobEventBuf::new();
    let mut lifecycle_effects = LifecycleEffects::new();
    let mut preview_systems_entered = BTreeSet::new();
    let mut preview_initial_lists_ready = BTreeSet::new();
    let bench_starts_on_arcade = launcher_bench_scenario
        .is_some_and(|scenario| scenario.starts_on_arcade() && !launcher_bench_after_input_script);
    let benchmark_media_interaction_active = launcher_bench_scenario.is_some();
    let env_start_screen = launcher_start_screen_from_env();
    let env_start_system = launcher_start_system_from_env();
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
    let startup_return_requested = pending_launch_return_state.is_some();
    let mut launch_return_restored = false;
    let arcade_catalog_required_at_start = start_screen == Screen::Arcade
        || lock_screen == Some(Screen::Arcade)
        || launcher_bench_after_input_script;
    let mut pending_start_system = env_start_system.clone();
    let mut nav = LauncherNav::new();
    nav.settings = crate::settings::MagikSettings::load();
    nav.screen = start_screen;
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
    crate::ui_logln!(
        "launcher_dirty_opt={}",
        if dirty_opt { "on" } else { "off" }
    );
    boot_analytics::event(
        "launcher_loop_start",
        format!("label={label} pads={}", pad.len()),
    );
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
    if launcher_bench_scenario.is_some() && !preview_archive_warm_skip_enabled() {
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
    } else if launcher_bench_scenario.is_some() {
        print_startup_event(start, "preview_archive_warm_skipped", "env=1");
    }
    let mut preview = PreviewState::new_with_trace_start(start);
    let mut launcher_bench_waiting_for_initial_preview = launcher_bench_scenario
        .is_some_and(|scenario| scenario.starts_on_arcade() && !launcher_bench_after_input_script);
    let mut preview_transition = PreviewTransitionDemo::from_env();
    let transition_picker_enabled = preview_transition.picker_enabled();
    let mut transition_picker_prev_left = false;
    let mut transition_picker_prev_right = false;
    let mut arcade_list_renderer = ArcadeListRenderer::new();
    let mut launcher_preview_version = 1u64;
    let mut launcher_arcade_version = 1u64;
    let mut arcade_filter_items_cache = ArcadeFilterListItemCache::default();
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
    let mut catalog = empty_arcade_catalog(&arcade_root);
    let mut catalog_ready = false;
    let catalog_refresh_policy = catalog_refresh_policy();
    let catalog_refresh = catalog_refresh_policy.force_requested();
    let catalog_worker_enabled = catalog_refresh_policy.worker_enabled();
    let mut lifecycle = LauncherLifecycle::new(
        LauncherLifecycleConfig {
            catalog_worker_enabled,
        },
        start,
    );
    let deferred_library_rebuild = consume_library_rebuild_marker(catalog_worker_enabled, start);
    let mut catalog_session = LauncherCatalogSession::new(deferred_library_rebuild);
    let mut media_session = ScreenshotMediaUpdateSession::default();
    let mut library_changed_dialog_test = LibraryChangedDialogTestDriver::from_env(start);
    let mut launcher_input_script = LauncherInputScriptDriver::from_env(start);
    let sqlite_path = library_db::default_sqlite_path();
    let summary_path = catalog_summary::summary_path_for_sqlite(&sqlite_path);
    let summary_seed = read_catalog_summary_seed(&sqlite_path, &summary_path, start);
    let mut startup_ready_catalog_source = CatalogSource::FreshBuild;
    if let Some(summary) = summary_seed.as_ref() {
        catalog = catalog_from_summary(&arcade_root, &summary);
        catalog_ready = true;
        startup_ready_catalog_source = CatalogSource::SummaryProjection;
        catalog_session.note_summary_seed_ready();
        media_session.request_catalog_seed();
        catalog_version = catalog_version.wrapping_add(1);
        let return_catalog_hydration_needed = startup_return_requested;
        let request = summary_seed_catalog_worker_request(
            catalog_refresh_policy,
            deferred_library_rebuild,
            return_catalog_hydration_needed,
        );
        if let Some(request) = request {
            let initial_cache =
                summary_seed_catalog_worker_initial_cache(request, return_catalog_hydration_needed);
            if summary_seed_catalog_worker_starts_immediately(
                request,
                return_catalog_hydration_needed,
            ) {
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                scheduler.start_catalog_worker(arcade_root.clone(), request, initial_cache);
            } else {
                let delay = catalog_background_validation_delay();
                print_startup_event(
                    start,
                    "catalog_worker_deferred",
                    format!(
                        "root={} request={} delay_ms={} reason=summary_hydration",
                        arcade_root,
                        request.label(),
                        delay.as_millis()
                    ),
                );
                catalog_session.defer_catalog_worker(arcade_root.clone(), request, initial_cache);
            }
        } else {
            print_startup_event(
                start,
                "catalog_refresh_decision",
                format!(
                    "cache_state=summary refresh_policy={} background_validation=false plan=load_only",
                    catalog_refresh_policy.label()
                ),
            );
            catalog_session.mark_refresh_done();
        }
    } else {
        library_db::record_catalog_ui_load();
        match library_db::load_arcade_catalog_from_sqlite(&arcade_root) {
            Ok(loaded) if !loaded.catalog.games.is_empty() => {
                print_startup_event(
                    start,
                    "catalog_cache_load_sync",
                    catalog_load_timing_detail(&loaded),
                );
                catalog = loaded.catalog;
                catalog_ready = true;
                startup_ready_catalog_source = CatalogSource::FullSqlite;
                catalog_session.note_cached_catalog_ready();
                media_session.request_catalog_seed();
                catalog_version = catalog_version.wrapping_add(1);
                apply_forced_arcade_selected(&mut nav, &catalog);
                launch_return_restored = apply_pending_launch_return_state(
                    &mut nav,
                    &catalog,
                    &mut pending_launch_return_state,
                );
                let request = ready_catalog_worker_request(catalog_refresh_policy);
                if deferred_library_rebuild {
                    print_startup_event(start, "catalog_worker_start", &arcade_root);
                    scheduler.start_catalog_worker(
                        arcade_root.clone(),
                        CatalogWorkerRequest::ForceBuild,
                        CatalogWorkerInitialCache::AlreadyLoadedReady,
                    );
                } else if request != CatalogWorkerRequest::LoadOnly {
                    let delay = catalog_background_validation_delay();
                    print_startup_event(
                        start,
                        "catalog_worker_deferred",
                        format!(
                            "root={} request={} delay_ms={}",
                            arcade_root,
                            request.label(),
                            delay.as_millis()
                        ),
                    );
                    catalog_session.defer_catalog_worker(
                        arcade_root.clone(),
                        request,
                        CatalogWorkerInitialCache::AlreadyLoadedReady,
                    );
                } else {
                    print_startup_event(
                        start,
                        "catalog_refresh_decision",
                        format!(
                            "cache_state=ready refresh_policy={} background_validation=false plan=load_only",
                            catalog_refresh_policy.label()
                        ),
                    );
                    catalog_session.mark_refresh_done();
                }
            }
            Ok(loaded) => {
                print_startup_event(
                    start,
                    "catalog_cache_empty",
                    catalog_load_timing_detail(&loaded),
                );
                if catalog_worker_enabled {
                    print_startup_event(
                        start,
                        "catalog_worker_deferred",
                        format!("root={} reason=first_visible_copy", arcade_root),
                    );
                    catalog_session.defer_catalog_worker(
                        arcade_root.clone(),
                        CatalogWorkerRequest::ForceBuild,
                        catalog_worker_initial_cache_after_ui_probe(CatalogUiCacheProbe::Empty),
                    );
                } else {
                    print_startup_event(
                        start,
                        "catalog_refresh_decision",
                        format!(
                            "cache_state=empty refresh_policy={} background_validation=false plan=load_only",
                            catalog_refresh_policy.label()
                        ),
                    );
                    catalog_session.mark_refresh_done();
                }
            }
            Err(e) => {
                crate::ui_errln!("arcade catalog cache load failed: {e}");
                print_startup_event(start, "catalog_cache_load_failed", e);
                if catalog_worker_enabled {
                    let initial_cache = catalog_worker_initial_cache_after_ui_probe(
                        CatalogUiCacheProbe::LoadFailed {
                            sqlite_exists: sqlite_path.exists(),
                        },
                    );
                    print_startup_event(
                        start,
                        "catalog_worker_deferred",
                        format!("root={} reason=first_visible_copy", arcade_root),
                    );
                    catalog_session.defer_catalog_worker(
                        arcade_root.clone(),
                        CatalogWorkerRequest::ForceBuild,
                        initial_cache,
                    );
                } else {
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
    }
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    apply_home_selected_from_env(&mut nav, &catalog, start);
    if catalog_ready && nav.screen == Screen::Home {
        if let Some(system) = active_system(&catalog, &nav) {
            if system.id == "arcade" && catalog.system_game_count(&system.id) > 0 {
                let games = active_system_game_view(&catalog, &nav);
                let selected = nav.arcade.selected.min(games.len().saturating_sub(1));
                let _ = prewarm_arcade_selected_preview(games, selected, &mut preview);
            }
        }
    }
    let bridge_systems_t = Instant::now();
    let mut arcade_screen_pending = (start_screen == Screen::Arcade
        || lock_screen == Some(Screen::Arcade))
        && !arcade_navigation_ready(catalog_ready, &catalog);
    bridge.set_game_systems(bridge_models.game_systems(&catalog, catalog_version, nav.selected));
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
        &setup,
        "",
        "",
        Some(&catalog),
        &mut preview,
        &mut bridge_models,
        catalog_version,
        false,
        ui.render_w(),
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
    let mut launcher_redraw_pending = true;
    macro_rules! request_launcher_redraw {
        () => {{
            launcher_redraw_pending = true;
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
    let mut frame_accounting = LauncherFrameAccounting::new(run_start);
    let mut arcade_entry_latency = ArcadeEntryLatencyTracker::from_env();
    let mut memory_guard = crate::memory_pressure::MemoryPressureGuard::from_env();
    let mut catalog_scan_redraw = CatalogScanRedraw::new();
    let mut last_home_pan_scroll_x = nav.scroll_x;
    let mut home_pan_present_until = None;
    while (secs == 0 || run_start.elapsed().as_secs() < secs)
        && preview_scroll_exit_at.is_none_or(|deadline| Instant::now() < deadline)
    {
        let loop_start = Instant::now();
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
        let launching = scheduler.launch_is_active() || !loading_title.is_empty();
        let setup_active = setup.is_active();
        let mut light_bridge_dirty = false;
        let mut full_bridge_dirty = false;
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
                        media_session.pause_for_low_memory(),
                        &app,
                        &catalog,
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
            &catalog,
            &mut scheduler,
            Some(&mut preview),
            &mut full_bridge_dirty,
            start,
        );
        let route_action = display_session.begin_frame(frames, launching, f);
        let defer_selected_preview = false;
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

        let catalog_worker_trace_start = prepare_trace_enabled.then(Instant::now);
        let startup_return_waiting_for_catalog = lifecycle.startup_waiting_for_return_catalog();
        let pad_changed_for_background = pad_changed_for_input.unwrap_or(false);
        let catalog_background_allowed = catalog_background_idle_gate.allow(
            CatalogBackgroundIdleInput {
                first_visible_copy_done: frame_accounting.first_visible_copy_done(),
                startup_return_waiting_for_catalog,
                startup_input_enabled: lifecycle.startup_input_enabled(),
                launching,
                setup_active,
                benchmark_active: launcher_bench_active,
                scripted_input_active: launcher_input_script.active(),
                pad_changed: pad_changed_for_background,
                pad_active: pad_state_has_active_input(pad.state()),
                catalog_messages_active: pending_catalog_ready.is_some()
                    || !deferred_catalog_events.is_empty(),
                media_message_seen: false,
                nav_motion_active: catalog_background_nav_motion_active(&nav),
                preview_critical: nav.screen == Screen::Arcade
                    && selected_arcade_game_has_preview(&nav, &catalog)
                    && !matches!(preview.trace_cache_state(), "exact" | "empty"),
            },
            loop_start,
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
            deferred_worker_policy.allowed,
            loop_start,
            deferred_worker_policy.delay,
        ) {
            print_startup_event(start, "catalog_worker_start", &worker.root);
            let lifecycle_input = deferred_catalog_worker_lifecycle_input(
                deferred_worker_policy.foreground,
                worker.request,
            );
            lifecycle.handle(lifecycle_input, &mut lifecycle_effects);
            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
            scheduler.start_catalog_worker(worker.root, worker.request, worker.initial_cache);
        }

        if pending_catalog_ready.is_some() || !catalog_session.refresh_done() {
            scheduler.poll_catalog(&mut catalog_events);
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
                        loop_start,
                        &app,
                        &pad,
                        &mut nav,
                        &setup,
                        &loading_title,
                        &mut catalog,
                        &mut catalog_ready,
                        &mut catalog_version,
                        &mut pending_launch_return_state,
                        &mut preview,
                        &mut bridge_models,
                        &mut media_session,
                        &mut scheduler,
                        &mut catalog_session,
                        &mut lifecycle,
                        &mut lifecycle_effects,
                        &mut full_bridge_dirty,
                        start,
                        ui.render_w(),
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
                    loop_start,
                    &app,
                    &pad,
                    &mut nav,
                    &setup,
                    &loading_title,
                    &mut catalog,
                    &mut catalog_ready,
                    &mut catalog_version,
                    &mut pending_launch_return_state,
                    &mut preview,
                    &mut bridge_models,
                    &mut media_session,
                    &mut scheduler,
                    &mut catalog_session,
                    &mut lifecycle,
                    &mut lifecycle_effects,
                    &mut full_bridge_dirty,
                    start,
                    ui.render_w(),
                );
                catalog_messages_processed = catalog_messages_processed.saturating_add(1);
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
                &catalog,
                &mut scheduler,
                Some(&mut preview),
                &mut full_bridge_dirty,
                start,
            );
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
                LaunchHandoffCompletion::Failure { error } => {
                    lifecycle.handle(
                        LauncherLifecycleInput::LaunchFailed {
                            message: error.to_string(),
                        },
                        &mut lifecycle_effects,
                    );
                    apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                    LauncherStatusPresenter::new(&bridge)
                        .sync_loading("Launch failed", "Returning to launcher...");
                    if scheduler.stop_spawned_mister_for_recovery() {
                        if let Err(e) = display_session.recover_after_launch_failure(frames, f) {
                            crate::ui_errln!(
                                "failed to recover Slint framebuffer route after launch failure: {e}"
                            );
                        }
                    }
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
                    LauncherStatusPresenter::new(&bridge)
                        .sync_loading(scheduler.launch_loading_title(), "");
                    crate::ui_errln!("game launch failed: {error}");
                }
            }
        }

        if arcade_screen_pending && arcade_navigation_ready(catalog_ready, &catalog) {
            let before = LauncherBridgeKey::from_nav(&nav);
            nav.screen = Screen::Arcade;
            arcade_screen_pending = false;
            full_bridge_dirty = true;
            let after = LauncherBridgeKey::from_nav(&nav);
            if before != after {
                media_session.note_nav_change(&before, &after, Instant::now());
            }
        }

        if let Some(system_id) = pending_start_system.take() {
            if arcade_navigation_ready(catalog_ready, &catalog) {
                let before = LauncherBridgeKey::from_nav(&nav);
                if apply_start_system_from_env(&mut nav, &catalog, &system_id) {
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
                    nav.screen = Screen::Home;
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
                frame_accounting = LauncherFrameAccounting::new(run_start);
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
                        if !dirty_opt || before.screen != after.screen {
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

        if !launching && lifecycle.startup_input_enabled() {
            let pad_changed = pad_changed_for_input
                .take()
                .unwrap_or_else(|| pad.poll_with_debug_labels(setup_active));
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
            let setup_state = input_session.setup_state().clone();
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
                    let event = if scheduler.should_request_benchmark_launch()
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
                        nav.handle_input(&nav_state, frame_now, &catalog)
                    };
                    if let Some(event) = event {
                        match event.action {
                            LauncherAction::ExitToMister => {
                                loading_title = "Exit to MiSTer".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    scheduler.visible_loading_title(&loading_title),
                                    "Return to MiSTer MagiK after reboot",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                    ui.render_w(),
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
                                    &catalog,
                                    &mut scheduler,
                                    Some(&mut preview),
                                    &mut full_bridge_dirty,
                                    start,
                                );
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    scheduler.visible_loading_title(&loading_title),
                                    "Restarting MiSTer",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                    ui.render_w(),
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
                                match launcher::reset_database_and_reboot() {
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
                                    &setup,
                                    scheduler.visible_loading_title(&loading_title),
                                    "Restarting MiSTer",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                    ui.render_w(),
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
                                    &pad,
                                    &mut nav,
                                    &setup,
                                    &loading_title,
                                    &mut catalog,
                                    &mut catalog_ready,
                                    &mut catalog_version,
                                    &mut pending_launch_return_state,
                                    &mut preview,
                                    &mut bridge_models,
                                    &mut media_session,
                                    &mut scheduler,
                                    &mut lifecycle,
                                    &mut lifecycle_effects,
                                    &mut full_bridge_dirty,
                                    start,
                                    ui.render_w(),
                                );
                                request_launcher_redraw!();
                                continue;
                            }
                            LauncherAction::RebuildLibrary => {
                                let effects = catalog_session.rebuild_library(arcade_root.clone());
                                apply_catalog_session_effects(
                                    effects,
                                    &app,
                                    &pad,
                                    &mut nav,
                                    &setup,
                                    &loading_title,
                                    &mut catalog,
                                    &mut catalog_ready,
                                    &mut catalog_version,
                                    &mut pending_launch_return_state,
                                    &mut preview,
                                    &mut bridge_models,
                                    &mut media_session,
                                    &mut scheduler,
                                    &mut lifecycle,
                                    &mut lifecycle_effects,
                                    &mut full_bridge_dirty,
                                    start,
                                    ui.render_w(),
                                );
                                request_launcher_redraw!();
                                continue;
                            }
                            LauncherAction::LaunchGame => {}
                        }
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
                            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                            continue;
                        }
                        if !scheduler.begin_launch(&nav, &catalog, &mra, Instant::now()) {
                            lifecycle.handle(
                                LauncherLifecycleInput::LaunchFailed {
                                    message: "launch scheduler rejected request".to_string(),
                                },
                                &mut lifecycle_effects,
                            );
                            apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                            continue;
                        }
                        apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                        sync_bridge_launcher(
                            &app,
                            &pad,
                            &nav,
                            &setup,
                            scheduler.launch_loading_title(),
                            "",
                            Some(&catalog),
                            &mut preview,
                            &mut bridge_models,
                            catalog_version,
                            false,
                            ui.render_w(),
                        );
                        window.request_redraw();
                        update_slint_animations(animation_clock);
                        window.draw_if_needed(|renderer| {
                            let region = target.render(renderer, frame_target_geometry(ui));
                            let _ = region;
                        });
                        let _pace = pacer.wait();
                        copy_cached_rows_565(disp, target.cached_frame_view(), 0, ui.render_h());
                        let loading_presented = Instant::now();
                        lifecycle
                            .loading_frame_presented(loading_presented, &mut lifecycle_effects);
                        apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
                        request_launcher_redraw!();
                    }
                    let nav_after = LauncherBridgeKey::from_nav(&nav);
                    if nav_before != nav_after {
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
                        if !dirty_opt || nav_before.screen != nav_after.screen {
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

            if full_bridge_dirty {
                sync_bridge_launcher(
                    &app,
                    &pad,
                    &nav,
                    &setup,
                    scheduler.visible_loading_title(&loading_title),
                    "",
                    Some(&catalog),
                    &mut preview,
                    &mut bridge_models,
                    catalog_version,
                    defer_selected_preview,
                    ui.render_w(),
                );
                preview_scheduled_this_loop = nav.screen == Screen::Arcade;
                request_launcher_redraw!();
            } else if light_bridge_dirty {
                let active_games = if nav.screen == Screen::Arcade {
                    Some(active_system_game_view(&catalog, &nav))
                } else {
                    None
                };
                sync_bridge_launcher_light(
                    &app,
                    &nav,
                    &mut bridge_models,
                    &setup,
                    scheduler.visible_loading_title(&loading_title),
                    "",
                    &catalog,
                    active_games,
                    &mut preview,
                    should_defer_arcade_overlay_bridge(dirty_opt, launching, &nav, &catalog),
                    defer_selected_preview,
                    ui.render_w(),
                );
                preview_scheduled_this_loop = nav.screen == Screen::Arcade;
                request_launcher_redraw!();
            }
            sync_startup_visibility(&app, &lifecycle);
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

        let media_gate_trace_start = prepare_trace_enabled.then(Instant::now);
        {
            let media_gate = media_session.current_gate(
                frame_accounting.first_visible_copy_done(),
                scheduler.has_pending_launch() || launching,
                benchmark_media_interaction_active,
                loop_start,
            );
            let media_gate = if memory_guard.active() {
                MediaInteractionGate {
                    active: true,
                    reason: "low-memory",
                }
            } else {
                media_gate
            };
            apply_screenshot_media_update_effects(
                media_session.sync_gate(media_gate),
                &app,
                &catalog,
                &mut scheduler,
                Some(&mut preview),
                &mut full_bridge_dirty,
                start,
            );
            apply_screenshot_media_update_effects(
                media_session.apply_gate(media_gate),
                &app,
                &catalog,
                &mut scheduler,
                Some(&mut preview),
                &mut full_bridge_dirty,
                start,
            );
            apply_screenshot_media_update_effects(
                media_session.sync_gate(media_gate),
                &app,
                &catalog,
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
        let games_found_detail_changed = if catalog_scan_visible && catalog_scan_percent < 0 {
            catalog_session
                .tick_games_found_counter(loop_start)
                .is_some_and(|detail| {
                    LauncherStatusPresenter::new(&bridge).sync_catalog_scan_detail(detail);
                    true
                })
        } else {
            false
        };
        let catalog_scan_redraw_due = catalog_scan_redraw.should_request(
            catalog_scan_visible,
            catalog_background_scan_visible,
            catalog_scan_percent,
            loop_start,
        );
        if launching || games_found_detail_changed || catalog_scan_redraw_due {
            request_launcher_redraw!();
        }
        let active_arcade_games = if !launching && nav.screen == Screen::Arcade {
            active_system_game_view(&catalog, &nav)
        } else {
            ArcadeGameView::empty()
        };
        let active_arcade_games_loading = !launching
            && nav.screen == Screen::Arcade
            && active_system_games_loading(&catalog, &nav);
        let arcade_search_active = nav.arcade_search.is_active(&nav.arcade_filter.active);
        if !launching && nav.screen == Screen::Arcade {
            if let Some(system) = active_system(&catalog, &nav) {
                if preview_systems_entered.insert(system.id.clone()) {
                    crate::ui_logln!(
                        "startup_timing\tpreview_system_entered\t{}ms\tsystem={}\tselected_index={}",
                        start.elapsed().as_millis(),
                        system.id,
                        nav.arcade.selected
                    );
                }
                if !active_arcade_games_loading
                    && !active_arcade_games.is_empty()
                    && preview_initial_lists_ready.insert(system.id.clone())
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
                            system.id,
                            selected,
                            game.title,
                            if game.has_preview { 1 } else { 0 },
                            game.preview_asset_key
                        );
                    } else {
                        crate::ui_logln!(
                            "startup_timing\tpreview_initial_list_ready\t{}ms\tsystem={}\tselected_index={}\ttitle=\thas_preview=0\tasset_key=",
                            start.elapsed().as_millis(),
                            system.id,
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
            && nav.screen == Screen::Arcade
            && !active_arcade_games_loading
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
        let preview_apply_dirty = if !launching && !arcade_search_active && !memory_guard.active() {
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
        let wants_arcade_list =
            should_draw_arcade_overlay(&nav, launching, active_arcade_games_loading);
        let wants_preview = !memory_guard.active() && preview.raw_transition_frame().is_some();
        let preview_frame_status = preview.raw_frame_status();
        let preview_cache_state_before_composition = preview.trace_cache_state();
        let composition_decision = composition.tick(UiCompositionInput {
            screen: nav.screen,
            confirm_visible,
            fullscreen_overlay_visible: catalog_scan_visible,
            arcade_ready: !active_arcade_games_loading && active_arcade_games.len() > 0,
            route_ok: display_session.route_ok(),
            wants_arcade_list,
            wants_preview,
            preview_cache_state: preview_cache_state_before_composition,
            preview_frame_status,
        });
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
        let slint_animation_active = app.window().has_active_animations();
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
        let arcade_visual_changed_this_loop = nav.arcade.visual_index
            != arcade_visual_index_at_loop_start
            || nav.arcade_filter.visual_index != arcade_filter_visual_index_at_loop_start;
        let stream_motion_before_render = slint_animation_active
            || home_pan_present_active
            || home_horizontal_input_held
            || arcade_visual_changed_this_loop
            || (nav.screen == Screen::Arcade && nav.arcade.is_scroll_active())
            || (nav.screen == Screen::Arcade
                && nav.arcade_filter.drawer_open
                && nav.arcade_filter.is_scroll_active());
        if !stream_motion_before_render {
            let _ = launcher_presenter.publish_stream_refinement_if_due();
        }
        let mut wake_reasons = LauncherWakeReasons::default();
        wake_reasons.insert_if(LauncherWakeReasons::REDRAW_PENDING, launcher_redraw_pending);
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
            LauncherWakeReasons::CATALOG_SCAN_VISIBLE,
            catalog_scan_visible,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::CATALOG_BACKGROUND_SCAN_VISIBLE,
            catalog_background_scan_visible,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::CATALOG_SCAN_REDRAW_DUE,
            catalog_scan_redraw_due,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::CATALOG_GAMES_FOUND_DETAIL_CHANGED,
            games_found_detail_changed,
        );
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
        let redraw_pending_for_trace = launcher_redraw_pending;
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
        let this_rect = expand_home_pan_dirty_rect(
            layer_target.render_slint_base(&window),
            ui,
            home_pan_present_active,
        );
        launcher_redraw_pending = false;
        let cpu_t2 = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let frame_t2 = Instant::now();
        let cpu_custom_draw_start = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let custom_draw_start = Instant::now();
        let arcade_list_update_start = Instant::now();
        let arcade_list_rect = if wants_arcade_list && composition_decision.allow_arcade_list_blit {
            arcade_list_renderer.set_geometry(if arcade_search_active {
                ArcadeListGeometry::search_for_render_w(ui.render_w())
            } else {
                ArcadeListGeometry::NORMAL
            });
            let force_arcade_redraw = arcade_list_needs_forced_redraw(
                &arcade_list_renderer,
                this_rect,
                full_frame_present,
            );
            if nav.arcade_filter.drawer_open {
                let items = arcade_filter_items_cache.items(&catalog, &nav, catalog_version);
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
        let (raw_preview, preview_transition_trace) =
            if composition_decision.allow_preview_blit && !memory_guard.active() {
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
        if arcade_list_rect.is_some() {
            launcher_arcade_version = launcher_arcade_version.wrapping_add(1).max(1);
        }
        let preview_desired = if composition_decision.allow_preview_blit
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
        let arcade_desired = if composition_decision.allow_arcade_list_blit {
            Some(DirectLayerState::new(
                arcade_list_renderer.dirty_rect(),
                launcher_arcade_version,
            ))
        } else {
            None
        };
        let mut cached_damage = DirtyRectList::new();
        cached_damage.push_if_some(base_damage);
        cached_damage.push_if_some(raw_preview_cached_rect);
        let frame_plan = LauncherFramePlan::new(
            cached_damage,
            preview_desired,
            raw_preview_direct_rect,
            arcade_desired,
            arcade_list_rect,
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
        if presentation.main_present_backend.is_latch()
            && mister_magik_fb::framebuffer::plugin_probe::atomic_scanout_runtime_enabled()
        {
            // SwappedBuffers must visit the other render target after every post,
            // even when Slint itself has no new damage. This replays the previous
            // damage into that slot before direct preview/Arcade layers use it.
            request_launcher_redraw!();
        }
        if !first_vsync_logged && pacing_trace.vsync_source == Some(VsyncPaceSource::Vsync) {
            first_vsync_logged = true;
            boot_analytics::event("first_vsync", format!("frame={frames}"));
        }
        if presentation.copied_rows > 0 {
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
            },
            timing: LauncherFrameTiming {
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
    let elapsed = run_start.elapsed().as_secs_f64();
    crate::ui_logln!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu) {
        crate::ui_errln!("{e}");
    }
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

#[allow(clippy::too_many_arguments)]
fn process_catalog_worker_message(
    message: CatalogWorkerMessage,
    prepare_trace: &mut LauncherPrepareTrace,
    first_visible_copy_done: bool,
    launching: bool,
    benchmark_media_interaction_active: bool,
    loop_start: Instant,
    app: &slint_ui::launcher::Launcher,
    pad: &PadPool,
    nav: &mut LauncherNav,
    setup: &SetupNav,
    loading_title: &str,
    catalog: &mut ArcadeCatalog,
    catalog_ready: &mut bool,
    catalog_version: &mut usize,
    pending_launch_return_state: &mut Option<launcher::LaunchReturnState>,
    preview: &mut PreviewState,
    bridge_models: &mut LauncherBridgeModels,
    media_session: &mut ScreenshotMediaUpdateSession,
    scheduler: &mut LauncherScheduler,
    catalog_session: &mut LauncherCatalogSession,
    lifecycle: &mut LauncherLifecycle,
    lifecycle_effects: &mut LifecycleEffects,
    full_bridge_dirty: &mut bool,
    start: Instant,
    render_w: usize,
) {
    prepare_trace.catalog_message_count = prepare_trace.catalog_message_count.saturating_add(1);
    let media_gate = if matches!(&message, CatalogWorkerMessage::SystemDiscovered { .. }) {
        let media_gate = media_session.current_gate(
            first_visible_copy_done,
            scheduler.has_pending_launch() || launching,
            benchmark_media_interaction_active,
            loop_start,
        );
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
            screen: nav.screen,
            media_gate,
        },
        message,
        loop_start,
    );
    apply_catalog_session_effects(
        effects,
        app,
        pad,
        nav,
        setup,
        loading_title,
        catalog,
        catalog_ready,
        catalog_version,
        pending_launch_return_state,
        preview,
        bridge_models,
        media_session,
        scheduler,
        lifecycle,
        lifecycle_effects,
        full_bridge_dirty,
        start,
        render_w,
    );
}

fn should_defer_catalog_message(
    message: &CatalogWorkerMessage,
    catalog_ready: bool,
    nav: &LauncherNav,
    stationary_edge_since: Option<Instant>,
    now: Instant,
) -> bool {
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
    let Some(state) = pending.take() else {
        return false;
    };
    launcher::apply_launch_return_state(nav, catalog, state)
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
        .map(|system| system.id.clone())
        .unwrap_or_default();
    lifecycle.handle(
        LauncherLifecycleInput::StartupReturnContextRestored {
            screen: screen_label(nav.screen),
            system_id,
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
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_catalog_session_effects(
    effects: CatalogSessionEffects,
    app: &slint_ui::launcher::Launcher,
    pad: &PadPool,
    nav: &mut LauncherNav,
    setup: &SetupNav,
    loading_title: &str,
    catalog: &mut ArcadeCatalog,
    catalog_ready: &mut bool,
    catalog_version: &mut usize,
    pending_launch_return_state: &mut Option<launcher::LaunchReturnState>,
    preview: &mut PreviewState,
    bridge_models: &mut LauncherBridgeModels,
    media_session: &mut ScreenshotMediaUpdateSession,
    scheduler: &mut LauncherScheduler,
    lifecycle: &mut LauncherLifecycle,
    lifecycle_effects: &mut LifecycleEffects,
    full_bridge_dirty: &mut bool,
    start: Instant,
    render_w: usize,
) {
    for effect in effects.into_effects() {
        match effect {
            CatalogSessionEffect::StartupEvent(event) => {
                print_startup_event(start, &event.name, event.detail);
            }
            CatalogSessionEffect::UseCatalog {
                catalog: ready_catalog,
                load_us: _,
                source,
            } => {
                *catalog = ready_catalog;
                *catalog_version = (*catalog_version).wrapping_add(1);
                *catalog_ready = true;
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
            CatalogSessionEffect::SyncCatalogBridge => {
                let bridge_sync_t = Instant::now();
                let loading_title = scheduler.visible_loading_title(loading_title);
                sync_bridge_launcher(
                    app,
                    pad,
                    nav,
                    setup,
                    loading_title,
                    "",
                    Some(catalog),
                    preview,
                    bridge_models,
                    *catalog_version,
                    false,
                    render_w,
                );
                print_startup_event(
                    start,
                    "catalog_bridge_sync_update",
                    format!(
                        "games={} elapsed_us={}",
                        catalog.len(),
                        bridge_sync_t.elapsed().as_micros()
                    ),
                );
            }
            CatalogSessionEffect::Ui(intent) => {
                apply_launcher_worker_ui_intent(app, intent, full_bridge_dirty);
            }
            CatalogSessionEffect::FinishMediaWorker => {
                apply_screenshot_media_update_effects(
                    media_session.finish_worker(),
                    app,
                    catalog,
                    scheduler,
                    Some(&mut *preview),
                    full_bridge_dirty,
                    start,
                );
            }
            CatalogSessionEffect::FinishMediaWorkerIfNoCatalogSeedPending => {
                apply_screenshot_media_update_effects(
                    media_session.finish_worker_if_no_catalog_seed_pending(),
                    app,
                    catalog,
                    scheduler,
                    Some(&mut *preview),
                    full_bridge_dirty,
                    start,
                );
            }
            CatalogSessionEffect::CatalogValidationFinished => {
                lifecycle.handle(
                    LauncherLifecycleInput::CatalogValidationFinished,
                    lifecycle_effects,
                );
                apply_lifecycle_effects(lifecycle_effects, scheduler, start);
            }
            CatalogSessionEffect::RequestMediaCatalogSeed => {
                media_session.request_catalog_seed();
            }
            CatalogSessionEffect::MediaSystemDiscovered {
                system_id,
                media_gate,
            } => {
                apply_screenshot_media_update_effects(
                    media_session.handle_catalog_system_discovered(system_id, media_gate),
                    app,
                    catalog,
                    scheduler,
                    Some(&mut *preview),
                    full_bridge_dirty,
                    start,
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
            CatalogSessionEffect::StartCatalogWorker(worker) => {
                print_startup_event(start, "catalog_worker_start", &worker.root);
                lifecycle.handle(
                    LauncherLifecycleInput::CatalogBuilding {
                        foreground: worker.request == CatalogWorkerRequest::ForceBuild,
                        has_stale_catalog: *catalog_ready,
                    },
                    lifecycle_effects,
                );
                apply_lifecycle_effects(lifecycle_effects, scheduler, start);
                scheduler.start_catalog_worker(worker.root, worker.request, worker.initial_cache);
            }
        }
    }
}

fn apply_screenshot_media_update_effects(
    effects: ScreenshotMediaUpdateEffects,
    app: &slint_ui::launcher::Launcher,
    catalog: &ArcadeCatalog,
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

#[derive(Debug)]
struct CatalogScanRedraw {
    last_request: Instant,
    period: Duration,
}

impl CatalogScanRedraw {
    fn new() -> Self {
        Self {
            last_request: Instant::now() - catalog_scan_redraw_period(),
            period: catalog_scan_redraw_period(),
        }
    }

    fn should_request(
        &mut self,
        visible: bool,
        background_visible: bool,
        percent: i32,
        now: Instant,
    ) -> bool {
        if !visible && !background_visible {
            return false;
        }
        if visible && !background_visible && percent >= 0 {
            return false;
        }
        if now.duration_since(self.last_request) < self.period {
            return false;
        }
        self.last_request = now;
        true
    }
}

fn catalog_scan_redraw_period() -> Duration {
    let fps = std::env::var("MISTER_CATALOG_SCAN_FPS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(15)
        .clamp(1, 60);
    Duration::from_millis((1000 / fps).max(1))
}

fn catalog_background_validation_delay() -> Duration {
    std::env::var("MISTER_CATALOG_BACKGROUND_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogUiCacheProbe {
    Empty,
    LoadFailed { sqlite_exists: bool },
}

fn catalog_worker_initial_cache_after_ui_probe(
    probe: CatalogUiCacheProbe,
) -> CatalogWorkerInitialCache {
    match probe {
        CatalogUiCacheProbe::Empty => CatalogWorkerInitialCache::AlreadyProbedEmpty,
        CatalogUiCacheProbe::LoadFailed {
            sqlite_exists: true,
        } => CatalogWorkerInitialCache::ProbeSqlite,
        CatalogUiCacheProbe::LoadFailed {
            sqlite_exists: false,
        } => CatalogWorkerInitialCache::AlreadyProbedMissing,
    }
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
    foreground: bool,
    request: CatalogWorkerRequest,
) -> LauncherLifecycleInput {
    if foreground {
        LauncherLifecycleInput::CatalogBuilding {
            foreground: request == CatalogWorkerRequest::ForceBuild,
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
    active_arcade_games_loading: bool,
) -> bool {
    !launching && nav.screen == Screen::Arcade && !active_arcade_games_loading
}

#[derive(Default)]
struct ArcadeFilterListItemCache {
    key: Option<ArcadeFilterListItemCacheKey>,
    items: Vec<ArcadeListItem>,
    rebuilds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArcadeFilterListItemCacheKey {
    catalog_version: usize,
    system_id: String,
    level: launcher::ArcadeFilterLevel,
    active_filter: String,
}

impl ArcadeFilterListItemCache {
    fn items(
        &mut self,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
        catalog_version: usize,
    ) -> &[ArcadeListItem] {
        let system_id = active_system(catalog, nav)
            .map(|system| system.id.as_str())
            .unwrap_or("");
        let key = ArcadeFilterListItemCacheKey {
            catalog_version,
            system_id: system_id.to_string(),
            level: nav.arcade_filter.level,
            active_filter: arcade_filter_cache_token(&nav.arcade_filter.active),
        };
        if self.key.as_ref() != Some(&key) {
            self.items = arcade_filter_list_items_for_system(catalog, nav, system_id);
            self.key = Some(key);
            self.rebuilds = self.rebuilds.wrapping_add(1);
        }
        &self.items
    }
}

fn arcade_filter_cache_token(filter: &arcade_catalog::ArcadeFilter) -> String {
    match filter {
        arcade_catalog::ArcadeFilter::All => "all".to_string(),
        arcade_catalog::ArcadeFilter::Search => "search".to_string(),
        arcade_catalog::ArcadeFilter::Decade(decade) => format!("decade:{decade}"),
        arcade_catalog::ArcadeFilter::Manufacturer(manufacturer) => {
            format!("manufacturer:{manufacturer}")
        }
        arcade_catalog::ArcadeFilter::Category(category) => format!("category:{category}"),
    }
}

fn arcade_filter_list_items_for_system(
    catalog: &ArcadeCatalog,
    nav: &LauncherNav,
    system_id: &str,
) -> Vec<ArcadeListItem> {
    nav.arcade_filter_items(catalog, system_id)
        .into_iter()
        .map(|item| ArcadeListItem {
            title: item.label,
            count: Some(item.count),
            active: item.active,
        })
        .collect()
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
    request: CatalogWorkerRequest,
    return_catalog_hydration_needed: bool,
) -> CatalogWorkerInitialCache {
    if request == CatalogWorkerRequest::ForceBuild
        || return_catalog_hydration_needed
        || request != CatalogWorkerRequest::LoadOnly
    {
        CatalogWorkerInitialCache::ProbeNavigationThenSqlite
    } else {
        CatalogWorkerInitialCache::AlreadyLoadedReady
    }
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
) -> bool {
    let Some(system_index) = catalog
        .systems
        .iter()
        .position(|system| system.id.eq_ignore_ascii_case(system_id))
    else {
        return false;
    };
    nav.selected = system_index;
    nav.screen = Screen::Arcade;
    nav.arcade_filter.active = arcade_catalog::ArcadeFilter::All;
    nav.arcade_filter.drawer_open = false;
    nav.arcade_filter.level = launcher::ArcadeFilterLevel::Top;
    nav.arcade.reset();
    ui_frame_target::apply_forced_arcade_selected(nav, catalog);
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
    if nav.screen != Screen::Home || selected >= catalog.systems.len() {
        print_startup_event(
            start,
            "launcher_home_selected_index_ignored",
            format!(
                "value={} screen={} systems={}",
                selected,
                screen_label(nav.screen),
                catalog.systems.len()
            ),
        );
        return;
    }
    nav.selected = selected;
    keep_bench_home_visible(&mut nav.scroll_x, nav.selected, catalog.systems.len());
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

        assert!(apply_start_system_from_env(&mut nav, &catalog, "neogeo"));

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
        std::env::set_var("MISTER_ARCADE_SELECTED_INDEX", "1");

        let applied = apply_start_system_from_env(&mut nav, &catalog, "neogeo");

        std::env::remove_var("MISTER_ARCADE_SELECTED_INDEX");
        assert!(applied);
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.arcade.selected, 1);
    }

    #[test]
    fn start_system_env_matches_case_insensitively() {
        let catalog = catalog_for_media_systems(&["arcade", "neogeo", "saturn"]);
        let mut nav = LauncherNav::new();

        assert!(apply_start_system_from_env(&mut nav, &catalog, "SATURN"));

        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.selected, 2);
    }

    #[test]
    fn start_system_env_fails_without_changing_nav_for_missing_system() {
        let catalog = catalog_for_media_systems(&["arcade", "neogeo", "saturn"]);
        let mut nav = LauncherNav::new();

        assert!(!apply_start_system_from_env(&mut nav, &catalog, "psx"));

        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.selected, 0);
    }

    fn ready_catalog_message() -> CatalogWorkerMessage {
        CatalogWorkerMessage::Ready {
            catalog: catalog_for_media_systems(&["arcade"]),
            summary: None,
            load_us: 42,
            source: CatalogSource::FullSqlite,
            durable_save_pending: false,
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
    pub(super) fn summary_projection_is_not_ready_for_arcade_navigation() {
        let catalog = summary_catalog_for_media_systems(&["arcade", "amiga"]);

        assert!(active_system_games_loading(&catalog, &LauncherNav::new()));
        assert!(!arcade_catalog_rows_ready(&catalog));
        assert!(!arcade_navigation_ready(true, &catalog));
        assert_eq!(
            effective_lock_screen(Some(Screen::Arcade), true, &catalog),
            None
        );
    }

    #[test]
    pub(super) fn summary_hot_arcade_rows_are_ready_for_arcade_navigation() {
        let full_catalog = catalog_for_media_systems(&["arcade", "amiga"]);
        let stamp = mister_magik_catalog::catalog_stamp::CatalogStamp::from_lines(vec![
            "root\t/media/fat".to_string(),
        ]);
        let summary =
            catalog_summary::CatalogSummaryProjection::from_catalog(&full_catalog, &stamp);
        let catalog = catalog_from_summary("/media/fat/_Arcade", &summary);
        let mut nav = LauncherNav::new();
        nav.selected = catalog
            .systems
            .iter()
            .position(|system| system.id == "arcade")
            .expect("arcade system");

        assert!(!active_system_games_loading(&catalog, &nav));
        assert!(arcade_catalog_rows_ready(&catalog));
        assert!(arcade_navigation_ready(true, &catalog));
        assert_eq!(catalog.system_game_count("arcade"), 1);
        assert_eq!(
            catalog
                .system_game_at("arcade", 0)
                .map(|game| game.title.as_ref()),
            Some("arcade game")
        );
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

        assert!(should_draw_arcade_overlay(&nav, false, false));
    }

    #[test]
    pub(super) fn arcade_overlay_draws_filter_list_while_filter_view_is_open() {
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade_filter.drawer_open = true;

        assert!(should_draw_arcade_overlay(&nav, false, false));
    }

    #[test]
    pub(super) fn arcade_overlay_stays_hidden_while_loading_or_launching() {
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;

        assert!(!should_draw_arcade_overlay(&nav, true, false));
        assert!(!should_draw_arcade_overlay(&nav, false, true));
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
    pub(super) fn launcher_present_backend_retired_values_fall_back_to_fb0_dirty() {
        assert_eq!(
            LauncherPresentBackend::from_env_values(Some(&["main", "flip-v1"].join("-"))),
            LauncherPresentBackend::Fb0Dirty
        );
        assert_eq!(
            LauncherPresentBackend::from_env_values(Some(&["main", "vsync-hidden"].join("-"))),
            LauncherPresentBackend::Fb0Dirty
        );
        assert_eq!(
            LauncherPresentBackend::from_env_values(Some(
                &["plugin", "main", "vsync-hidden"].join("-")
            )),
            LauncherPresentBackend::Fb0Dirty
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
                LauncherPresentStatus::Unsupported,
            ),
            "Mode=/dev/fb0"
        );
        assert_eq!(
            present_mode_label_for_backend_status(
                LauncherPresentBackend::Fb0Dirty,
                LauncherPresentStatus::None,
            ),
            "Mode=/dev/fb0"
        );
        assert_eq!(
            present_mode_label_for_backend_status(
                LauncherPresentBackend::None,
                LauncherPresentStatus::None,
            ),
            "Mode=/dev/fb0"
        );
    }

    #[test]
    pub(super) fn arcade_filter_list_item_cache_reuses_rows_until_menu_key_changes() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("Alpha")
                    .path("/media/fat/_Arcade/alpha.mra")
                    .year(1986)
                    .manufacturer("Capcom")
                    .category("Shooter")
                    .build(),
                arcade_game("Beta")
                    .path("/media/fat/_Arcade/beta.mra")
                    .year(1991)
                    .manufacturer("Namco")
                    .category("Maze")
                    .build(),
            ],
            vec![arcade_system("arcade", 2)],
        );
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade_filter.drawer_open = true;
        let mut cache = ArcadeFilterListItemCache::default();

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
        assert!(nav
            .handle_input(&right, start + LIBRARY_CHANGED_TEST_ACTION_SETTLE, &catalog)
            .is_none());
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
        assert!(nav
            .handle_input(
                &release,
                start + LIBRARY_CHANGED_TEST_ACTION_SETTLE + Duration::from_millis(16),
                &catalog,
            )
            .is_none());

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
    pub(super) fn ready_catalog_rebuild_progress_uses_background_badge() {
        for title in ["Indexing library", "Loading library"] {
            let full_visible = catalog_scan_progress_visible(true, Screen::Home, title, false);
            assert!(!full_visible, "{title} should not cover a ready catalog");
            assert!(catalog_background_scan_progress_visible(
                true,
                full_visible,
                title
            ));
        }
    }

    #[test]
    pub(super) fn catalog_scan_redraw_throttles_indeterminate_animation() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now,
            period: Duration::from_millis(66),
        };
        assert!(!redraw.should_request(true, false, -1, now + Duration::from_millis(20)));
        assert!(redraw.should_request(true, false, -1, now + Duration::from_millis(70)));
    }

    #[test]
    pub(super) fn catalog_scan_redraw_skips_determinate_periodic_frames() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now - Duration::from_secs(1),
            period: Duration::from_millis(66),
        };
        assert!(!redraw.should_request(true, false, 90, now));
        assert!(!redraw.should_request(false, false, -1, now));
    }

    #[test]
    pub(super) fn catalog_scan_redraw_animates_background_badge() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now - Duration::from_secs(1),
            period: Duration::from_millis(66),
        };
        assert!(redraw.should_request(false, true, -1, now));
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
            LauncherWakeReasons::CATALOG_SCAN_VISIBLE,
            LauncherWakeReasons::CATALOG_BACKGROUND_SCAN_VISIBLE,
            LauncherWakeReasons::CATALOG_SCAN_REDRAW_DUE,
            LauncherWakeReasons::CATALOG_GAMES_FOUND_DETAIL_CHANGED,
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
            assert!(!LauncherRenderIntent {
                first_visible_copy_done: true,
                startup_input_enabled: true,
                wake_reasons: reason,
            }
            .can_sleep());
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
            assert!(!LauncherRenderIntent {
                first_visible_copy_done: true,
                startup_input_enabled: true,
                wake_reasons: reasons,
            }
            .can_sleep());
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
    pub(super) fn summary_warm_validation_hydrates_navigation_in_worker() {
        assert_eq!(
            summary_seed_catalog_worker_initial_cache(CatalogWorkerRequest::CheckStamp, false),
            CatalogWorkerInitialCache::ProbeNavigationThenSqlite
        );
        assert_eq!(
            summary_seed_catalog_worker_initial_cache(CatalogWorkerRequest::LoadOnly, true),
            CatalogWorkerInitialCache::ProbeNavigationThenSqlite
        );
        assert_eq!(
            summary_seed_catalog_worker_initial_cache(CatalogWorkerRequest::ForceBuild, false),
            CatalogWorkerInitialCache::ProbeNavigationThenSqlite
        );
    }

    #[test]
    pub(super) fn ui_catalog_probe_state_is_preserved_for_the_worker() {
        assert_eq!(
            catalog_worker_initial_cache_after_ui_probe(CatalogUiCacheProbe::Empty),
            CatalogWorkerInitialCache::AlreadyProbedEmpty
        );
        assert_eq!(
            catalog_worker_initial_cache_after_ui_probe(CatalogUiCacheProbe::LoadFailed {
                sqlite_exists: false,
            }),
            CatalogWorkerInitialCache::AlreadyProbedMissing
        );
        assert_eq!(
            catalog_worker_initial_cache_after_ui_probe(CatalogUiCacheProbe::LoadFailed {
                sqlite_exists: true,
            }),
            CatalogWorkerInitialCache::ProbeSqlite
        );
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
                after_copy.foreground,
                CatalogWorkerRequest::ForceBuild,
            ),
            LauncherLifecycleInput::CatalogBuilding {
                foreground: true,
                has_stale_catalog: false,
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
                allowed.foreground,
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
}
