// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::input_state::PadState;

const BENCH_SCENARIO: &str = "MISTER_LAUNCHER_BENCH_SCENARIO";
const START_SCREEN: &str = "MISTER_LAUNCHER_START_SCREEN";
const START_SYSTEM: &str = "MISTER_LAUNCHER_START_SYSTEM";
const SYSTEM_ENTRY_BENCHMARK_SYSTEM: &str = "MISTER_SYSTEM_ENTRY_BENCHMARK_SYSTEM";
const START_MENU: &str = "MISTER_LAUNCHER_START_MENU";
const LOCK_SCREEN: &str = "MISTER_LAUNCHER_LOCK_SCREEN";
const BENCH_AFTER_INPUT_SCRIPT: &str = "MISTER_LAUNCHER_BENCH_AFTER_INPUT_SCRIPT";
const PREVIEW_STEP_HOLD_SECS: &str = "MISTER_PREVIEW_STEP_HOLD_SECS";
const HUMAN_TURBO_IDLE_FRAMES: &str = "MISTER_HUMAN_TURBO_IDLE_FRAMES";
const HUMAN_TURBO_NORMAL_FRAMES: &str = "MISTER_HUMAN_TURBO_NORMAL_FRAMES";
const HUMAN_TURBO_PAUSE_FRAMES: &str = "MISTER_HUMAN_TURBO_PAUSE_FRAMES";
const HOME_SELECTED_INDEX: &str = "MISTER_HOME_SELECTED_INDEX";
const AUTO_LAUNCH_SELECTED: &str = "MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED";
const ORIENTATION_PMU_COMPLETE: &str = "MISTER_ORIENTATION_PMU_COMPLETE";
const LAUNCH_RETURN_PMU_HANDOFF_OUT: &str = "MISTER_LAUNCH_RETURN_PMU_HANDOFF_OUT";

#[derive(Clone, Debug)]
pub struct LauncherBenchmarkConfig {
    scenario: Option<LauncherBenchScenario>,
    start_screen: Option<Screen>,
    start_system: Option<String>,
    system_entry_system: Option<String>,
    start_menu: Option<String>,
    lock_screen: Option<Screen>,
    after_input_script: bool,
    preview_step_hold_frames: usize,
    human_turbo_idle_frames: usize,
    human_turbo_normal_frames: usize,
    human_turbo_pause_frames: usize,
    home_selected: Option<Result<usize, String>>,
    auto_launch_selected: bool,
    orientation_pmu_completion: Option<String>,
    launch_return_pmu_handoff_out: Option<String>,
}

impl Default for LauncherBenchmarkConfig {
    fn default() -> Self {
        Self {
            scenario: None,
            start_screen: None,
            start_system: None,
            system_entry_system: None,
            start_menu: None,
            lock_screen: None,
            after_input_script: false,
            preview_step_hold_frames: 300,
            human_turbo_idle_frames: 30,
            human_turbo_normal_frames: 30,
            human_turbo_pause_frames: 30,
            home_selected: None,
            auto_launch_selected: false,
            orientation_pmu_completion: None,
            launch_return_pmu_handoff_out: None,
        }
    }
}

impl LauncherBenchmarkConfig {
    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        let scenario = LauncherBenchScenario::from_value(get(BENCH_SCENARIO));
        Self {
            scenario,
            start_screen: launcher_screen_from_value(get(START_SCREEN)),
            start_system: normalized_nonempty(get(START_SYSTEM)),
            system_entry_system: normalized_nonempty(get(SYSTEM_ENTRY_BENCHMARK_SYSTEM)),
            start_menu: normalized_nonempty(get(START_MENU)).filter(|value| {
                matches!(
                    value.as_str(),
                    "consoles" | "handhelds" | "computers" | "snk-neogeo"
                )
            }),
            lock_screen: launcher_screen_from_value(get(LOCK_SCREEN)),
            after_input_script: scenario.is_some()
                && get(BENCH_AFTER_INPUT_SCRIPT).is_some_and(benchmark_flag),
            preview_step_hold_frames: get(PREVIEW_STEP_HOLD_SECS)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(5)
                .clamp(1, 60)
                .saturating_mul(60)
                .max(1),
            human_turbo_idle_frames: bounded_frames(get(HUMAN_TURBO_IDLE_FRAMES), 30, 180),
            human_turbo_normal_frames: bounded_frames(get(HUMAN_TURBO_NORMAL_FRAMES), 30, 300),
            human_turbo_pause_frames: bounded_frames(get(HUMAN_TURBO_PAUSE_FRAMES), 30, 300),
            home_selected: get(HOME_SELECTED_INDEX)
                .map(|value| value.parse::<usize>().map_err(|_| value.to_owned())),
            auto_launch_selected: get(AUTO_LAUNCH_SELECTED).is_some_and(benchmark_flag),
            orientation_pmu_completion: get(ORIENTATION_PMU_COMPLETE).map(str::to_owned),
            launch_return_pmu_handoff_out: get(LAUNCH_RETURN_PMU_HANDOFF_OUT).map(str::to_owned),
        }
    }

    pub(super) fn scenario(&self) -> Option<LauncherBenchScenario> {
        self.scenario
    }
    pub(super) fn start_screen(&self) -> Option<Screen> {
        self.start_screen
    }
    pub(super) fn start_system(&self) -> Option<&str> {
        self.start_system.as_deref()
    }
    pub(super) fn system_entry_system(&self) -> Option<&str> {
        self.system_entry_system.as_deref()
    }
    pub(super) fn start_menu(&self) -> Option<&str> {
        self.start_menu.as_deref()
    }
    pub(super) fn lock_screen(&self) -> Option<Screen> {
        self.lock_screen
    }
    pub(super) fn after_input_script(&self) -> bool {
        self.after_input_script
    }
    pub(super) fn home_selected(&self) -> Option<&Result<usize, String>> {
        self.home_selected.as_ref()
    }
    pub(super) fn auto_launch_selected(&self) -> bool {
        self.auto_launch_selected
    }
    pub(super) fn orientation_pmu_completion(&self) -> Option<&str> {
        self.orientation_pmu_completion.as_deref()
    }
    pub(super) fn launch_return_pmu_handoff_out(&self) -> Option<&str> {
        self.launch_return_pmu_handoff_out.as_deref()
    }
}

fn normalized_nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn bounded_frames(value: Option<&str>, default: usize, maximum: usize) -> usize {
    value
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
        .min(maximum)
}

fn benchmark_flag(value: &str) -> bool {
    matches!(value, "1" | "on" | "true" | "yes")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LauncherBenchScenario {
    Idle,
    PreviewIdle,
    HomeNav,
    HomeRepeatHold,
    QuickTap,
    RapidTaps,
    HeldScroll,
    HumanTurboHold,
    TurboHold,
    PreviewStepHold,
    ModelSync,
    LaunchHandoff,
    ScreensaverShow,
}

impl LauncherBenchScenario {
    fn from_value(value: Option<&str>) -> Option<Self> {
        #[cfg(not(feature = "bench-tools"))]
        {
            let _ = value;
            None
        }
        #[cfg(feature = "bench-tools")]
        {
            match value?.to_ascii_lowercase().as_str() {
                "idle" => Some(Self::Idle),
                "preview-idle" | "preview_idle" => Some(Self::PreviewIdle),
                "home-nav" | "home_nav" => Some(Self::HomeNav),
                "home-repeat-hold" | "home_repeat_hold" | "home-hold-repeat"
                | "home_hold_repeat" => Some(Self::HomeRepeatHold),
                "velocity-scroll" | "velocity_scroll" => Some(Self::HeldScroll),
                "quick-tap" | "quick_tap" => Some(Self::QuickTap),
                "rapid-taps" | "rapid_taps" => Some(Self::RapidTaps),
                "held-scroll" | "held_scroll" => Some(Self::HeldScroll),
                "human-turbo-hold" | "human_turbo_hold" | "human-turbo" | "human_turbo" => {
                    Some(Self::HumanTurboHold)
                }
                "turbo-hold" | "turbo_hold" => Some(Self::TurboHold),
                "preview-step-hold" | "preview_step_hold" | "step-hold" | "step_hold" => {
                    Some(Self::PreviewStepHold)
                }
                "model-sync" | "model_sync" => Some(Self::ModelSync),
                "launch-handoff" | "launch_handoff" => Some(Self::LaunchHandoff),
                "screensaver-show" | "screensaver_show" => Some(Self::ScreensaverShow),
                _ => None,
            }
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::PreviewIdle => "preview-idle",
            Self::HomeNav => "home-nav",
            Self::HomeRepeatHold => "home-repeat-hold",
            Self::QuickTap => "quick-tap",
            Self::RapidTaps => "rapid-taps",
            Self::HeldScroll => "held-scroll",
            Self::HumanTurboHold => "human-turbo-hold",
            Self::TurboHold => "turbo-hold",
            Self::PreviewStepHold => "preview-step-hold",
            Self::ModelSync => "model-sync",
            Self::LaunchHandoff => "launch-handoff",
            Self::ScreensaverShow => "screensaver-show",
        }
    }

    pub(super) fn period(self) -> Duration {
        match self {
            Self::Idle | Self::PreviewIdle | Self::LaunchHandoff | Self::ScreensaverShow => {
                Duration::MAX
            }
            Self::HomeNav => Duration::from_millis(300),
            Self::HomeRepeatHold => Duration::ZERO,
            Self::ModelSync => Duration::from_millis(300),
            Self::QuickTap
            | Self::RapidTaps
            | Self::HeldScroll
            | Self::HumanTurboHold
            | Self::TurboHold
            | Self::PreviewStepHold => Duration::ZERO,
        }
    }

    pub(super) fn starts_on_arcade(self) -> bool {
        matches!(
            self,
            Self::QuickTap
                | Self::RapidTaps
                | Self::HeldScroll
                | Self::HumanTurboHold
                | Self::TurboHold
                | Self::PreviewIdle
                | Self::PreviewStepHold
                | Self::LaunchHandoff
        )
    }
}

fn launcher_screen_from_value(value: Option<&str>) -> Option<Screen> {
    match value?.to_ascii_lowercase().as_str() {
        "home" => Some(Screen::Home),
        "system-hub" | "snes-hub" => Some(Screen::SystemHub),
        "arcade" => Some(Screen::Arcade),
        "controller" | "controller-test" | "controller_test" => Some(Screen::Controller),
        "settings" => Some(Screen::Settings),
        "about" => Some(Screen::About),
        "licenses" => Some(Screen::Licenses),
        "info" => Some(Screen::Info),
        "screensaver" | "screensaver-settings" => Some(Screen::Screensaver),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub(super) struct LauncherBenchState {
    step: usize,
    home_repeat_dir: i32,
}

impl Default for LauncherBenchState {
    fn default() -> Self {
        Self {
            step: 0,
            home_repeat_dir: 1,
        }
    }
}

impl LauncherBenchState {
    pub(super) fn advance_if(&mut self, step_ran: bool) {
        if step_ran {
            self.step = self.step.wrapping_add(1);
        }
    }
}

pub(super) fn launcher_bench_step(
    scenario: LauncherBenchScenario,
    config: &LauncherBenchmarkConfig,
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    active_game_count: Option<usize>,
    state: &mut LauncherBenchState,
    now: Instant,
) -> bool {
    nav.sync_launcher_taxonomy(catalog);
    match scenario {
        LauncherBenchScenario::Idle
        | LauncherBenchScenario::PreviewIdle
        | LauncherBenchScenario::LaunchHandoff
        | LauncherBenchScenario::ScreensaverShow => false,
        LauncherBenchScenario::HomeNav => {
            let count = nav.current_menu_count();
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Home;
            nav.settings_focused = false;
            let selected = state.step % count;
            if selected < nav.selected {
                nav.scroll_x = 0;
            }
            nav.selected = selected;
            keep_bench_home_visible(&mut nav.scroll_x, nav.selected, count);
            true
        }
        LauncherBenchScenario::HomeRepeatHold => {
            let count = nav.current_menu_count();
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Home;
            nav.settings_focused = false;
            if nav.selected >= count {
                nav.selected = count - 1;
                keep_bench_home_visible(&mut nav.scroll_x, nav.selected, count);
            }

            if nav.selected == 0 {
                state.home_repeat_dir = 1;
            } else if nav.selected + 1 >= count {
                state.home_repeat_dir = -1;
            }
            let mut input = PadState::default();
            if state.home_repeat_dir < 0 {
                input.dpad_left = true;
            } else {
                input.dpad_right = true;
            }
            let _ = nav.handle_held_tick_with_navigation_intents(&input, now, catalog);
            true
        }
        LauncherBenchScenario::ModelSync => {
            nav.go_root();
            let count = nav.current_menu_count();
            if count == 0 {
                return false;
            }
            let selected = (state.step / 2) % count;
            if selected < nav.selected {
                nav.scroll_x = 0;
            }
            nav.selected = selected;
            nav.settings_focused = false;
            if state.step % 2 == 0 {
                nav.screen = Screen::Home;
                keep_bench_home_visible(&mut nav.scroll_x, nav.selected, count);
            } else {
                if !nav.open_default_arcade(catalog) {
                    return false;
                }
                let game_count = nav
                    .active_collection_id()
                    .map(|id| catalog.system_game_count(id))
                    .unwrap_or(0);
                nav.arcade.selected = nav.arcade.selected.min(game_count.saturating_sub(1));
                nav.arcade.snap_to_selected();
                keep_bench_arcade_visible(
                    &mut nav.arcade.scroll_y,
                    nav.arcade.selected,
                    game_count,
                );
            }
            true
        }
        LauncherBenchScenario::HeldScroll => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let previous_dir = if state.step == 0 { 0 } else { 1 };
            nav.arcade.bench_direction_tick(1, previous_dir, count, now);
            true
        }
        LauncherBenchScenario::HumanTurboHold => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let idle_frames = config.human_turbo_idle_frames;
            let normal_frames = config.human_turbo_normal_frames;
            let pause_frames = config.human_turbo_pause_frames;
            if state.step < idle_frames {
                nav.arcade.bench_direction_tick(0, 0, count, now);
            } else if state.step < idle_frames.saturating_add(normal_frames) {
                let previous_dir = if state.step == idle_frames { 0 } else { 1 };
                nav.arcade.bench_direction_tick(1, previous_dir, count, now);
            } else if state.step
                < idle_frames
                    .saturating_add(normal_frames)
                    .saturating_add(pause_frames)
            {
                let previous_dir = if state.step == idle_frames.saturating_add(normal_frames) {
                    1
                } else {
                    0
                };
                nav.arcade.bench_direction_tick(0, previous_dir, count, now);
            } else {
                nav.arcade.bench_turbo_bounce_tick(count, now);
            }
            true
        }
        LauncherBenchScenario::PreviewStepHold => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            if state.step % config.preview_step_hold_frames == 0 {
                nav.arcade.handle_direction_input(1, 0, now, count);
            }
            nav.arcade.tick(count, now);
            true
        }
        LauncherBenchScenario::QuickTap => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let (dir, previous_dir) = match state.step {
                0 => (1, 0),
                1 => (0, 1),
                _ => (0, 0),
            };
            nav.arcade
                .bench_direction_tick(dir, previous_dir, count, now);
            true
        }
        LauncherBenchScenario::RapidTaps => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            let (dir, previous_dir) = if state.step < 10 {
                if state.step % 2 == 0 { (1, 0) } else { (0, 1) }
            } else {
                (0, 0)
            };
            nav.arcade
                .bench_direction_tick(dir, previous_dir, count, now);
            true
        }
        LauncherBenchScenario::TurboHold => {
            let Some(count) = launcher_bench_active_game_count(catalog, nav, active_game_count)
            else {
                return false;
            };
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Arcade;
            nav.arcade.bench_turbo_bounce_tick(count, now);
            true
        }
    }
}

pub(super) fn launcher_bench_active_game_count(
    catalog: &ArcadeCatalog,
    nav: &LauncherNav,
    active_game_count: Option<usize>,
) -> Option<usize> {
    if let Some(count) = active_game_count {
        return Some(count);
    }
    Some(catalog.system_game_count(nav.active_collection_id()?))
}

pub(super) fn keep_bench_home_visible(scroll_x: &mut i32, selected: usize, count: usize) {
    let item_w = HOME_TILE_WIDTH + HOME_TILE_GAP;
    let selected_x = selected as i32 * item_w;
    let selected_right = selected_x + HOME_TILE_WIDTH;
    if selected_x < *scroll_x {
        *scroll_x = selected_right - HOME_LIST_VISIBLE_W;
    } else if selected_right > *scroll_x + HOME_LIST_VISIBLE_W {
        *scroll_x = selected_x;
    }
    let max_scroll = (count as i32 * item_w - HOME_TILE_GAP - HOME_LIST_VISIBLE_W).max(0);
    *scroll_x = (*scroll_x).clamp(0, max_scroll);
}

pub(super) fn keep_bench_arcade_visible(scroll_y: &mut i32, selected: usize, count: usize) {
    let selected_y = selected as i32 * ARCADE_ROW_HEIGHT;
    let selected_bottom = selected_y + ARCADE_ROW_HEIGHT;
    if selected_y < *scroll_y {
        *scroll_y = selected_y;
    }
    if selected_bottom > *scroll_y + ARCADE_LIST_VISIBLE_H {
        *scroll_y = selected_bottom - ARCADE_LIST_VISIBLE_H;
    }
    let max_scroll = (count as i32 * ARCADE_ROW_HEIGHT - ARCADE_LIST_VISIBLE_H).max(0);
    *scroll_y = (*scroll_y).clamp(0, max_scroll);
}

pub(super) fn sync_setup_bridge(
    app: &slint_ui::launcher::Launcher,
    pad: &PadPool,
    setup: &SetupNav,
) {
    let info = setup_pad_info(pad, setup);
    let db = pad.db();
    let active = setup.phase != SetupPhase::None;
    let view = app.global::<slint_ui::launcher::SetupView>();
    view.set_phase(crate::launcher_view_types::setup_phase(setup.phase));
    view.set_selected_entry_index(setup.list_index as i32);
    if active {
        view.set_title(setup.title().into());
        let js_path = setup
            .target_device
            .as_ref()
            .and_then(|device| pad.path_for_device(device))
            .unwrap_or("(controller disconnected)");

        if setup.phase == SetupPhase::Configure {
            let fields = SetupNav::configure_fields(info, js_path, db);
            view.set_fields(ModelRc::new(VecModel::from(
                fields
                    .into_iter()
                    .map(|(label, value)| slint_ui::launcher::SetupField {
                        label: label.into(),
                        value: value.into(),
                    })
                    .collect::<Vec<_>>(),
            )));
            view.set_entries(ModelRc::new(VecModel::from(Vec::new())));
            let live = setup
                .target_device
                .as_ref()
                .and_then(|device| pad.state_for_device(device))
                .map(SetupNav::configure_live_hint)
                .unwrap_or_else(|| "Controller disconnected".into());
            view.set_subtitle(live.into());
            view.set_name(String::new().into());
            view.set_kind_label(String::new().into());
        } else if setup.phase == SetupPhase::NameKind {
            view.set_subtitle(setup.subtitle(info, db).into());
            view.set_name(setup.draft_label.clone().into());
            view.set_kind_label(setup.draft_kind_label().into());
            view.set_entries(ModelRc::new(VecModel::from(Vec::new())));
            view.set_fields(ModelRc::new(VecModel::from(Vec::new())));
        } else if setup.phase == SetupPhase::PickExisting {
            view.set_subtitle(setup.subtitle(info, db).into());
            view.set_fields(ModelRc::new(VecModel::from(Vec::new())));
            let rows = db
                .list_entries()
                .iter()
                .map(|item| {
                    let port = if item.last_usb_port.is_empty() {
                        "unknown port".to_string()
                    } else {
                        format!("was {}", item.last_usb_port)
                    };
                    slint_ui::launcher::SetupEntry {
                        id: item.id.clone().into(),
                        label: format!("{} — {}", item.label, port).into(),
                    }
                })
                .collect::<Vec<_>>();
            view.set_entries(ModelRc::new(VecModel::from(rows)));
            view.set_name(String::new().into());
            view.set_kind_label(String::new().into());
        } else {
            view.set_subtitle(setup.subtitle(info, db).into());
            view.set_entries(ModelRc::new(VecModel::from(Vec::new())));
            view.set_fields(ModelRc::new(VecModel::from(Vec::new())));
            view.set_name(String::new().into());
            view.set_kind_label(String::new().into());
        }
    } else {
        view.set_title(String::new().into());
        view.set_subtitle(String::new().into());
        view.set_entries(ModelRc::new(VecModel::from(Vec::new())));
        view.set_fields(ModelRc::new(VecModel::from(Vec::new())));
        view.set_name(String::new().into());
        view.set_kind_label(String::new().into());
    }
}

#[cfg(not(mister_ui_scope_launcher))]
pub(super) fn sync_bridge_pad_controller(view: &slint_ui::controller::InputView, pad: &PadPool) {
    let state = pad.state();
    let info = pad.info();
    view.set_dpad_up(state.dpad_up);
    view.set_dpad_down(state.dpad_down);
    view.set_dpad_left(state.dpad_left);
    view.set_dpad_right(state.dpad_right);
    view.set_button_a(state.btn_a);
    view.set_button_b(state.btn_b);
    view.set_button_x(state.btn_x);
    view.set_button_y(state.btn_y);
    view.set_button_l(state.btn_l);
    view.set_button_r(state.btn_r);
    view.set_button_zl(state.btn_zl);
    view.set_button_zr(state.btn_zr);
    view.set_button_select(state.btn_select);
    view.set_button_start(state.btn_start);
    view.set_button_l3(state.btn_l3);
    view.set_button_r3(state.btn_r3);
    view.set_button_home(state.btn_home);
    view.set_button_capture(state.btn_capture);
    view.set_capture_availability(if info.capture_available {
        slint_ui::controller::InputAvailability::Available
    } else {
        slint_ui::controller::InputAvailability::Unavailable
    });
    view.set_input_availability(slint_ui::controller::InputAvailability::Available);
    view.set_fault_notice(String::new().into());
    view.set_left_x(state.left_x);
    view.set_left_y(state.left_y);
    view.set_right_x(state.right_x);
    view.set_right_y(state.right_y);
    sync_device_info_controller(view, info, pad.db(), pad.path(), pad.len());
    view.set_pressed_now(state.pressed_now.clone().into());
    view.set_last_event_label(state.last_event_label.clone().into());
    view.set_last_raw_event(state.last_raw.clone().into());
}

pub(super) fn sync_bridge_pad_launcher(app: &slint_ui::launcher::Launcher, pad: &PadPool) {
    let view = app.global::<slint_ui::launcher::InputView>();
    let state = pad.state();
    let info = pad.info();
    view.set_dpad_up(state.dpad_up);
    view.set_dpad_down(state.dpad_down);
    view.set_dpad_left(state.dpad_left);
    view.set_dpad_right(state.dpad_right);
    view.set_button_a(state.btn_a);
    view.set_button_b(state.btn_b);
    view.set_button_x(state.btn_x);
    view.set_button_y(state.btn_y);
    view.set_button_l(state.btn_l);
    view.set_button_r(state.btn_r);
    view.set_button_zl(state.btn_zl);
    view.set_button_zr(state.btn_zr);
    view.set_button_select(state.btn_select);
    view.set_button_start(state.btn_start);
    view.set_button_l3(state.btn_l3);
    view.set_button_r3(state.btn_r3);
    view.set_button_home(state.btn_home);
    view.set_button_capture(state.btn_capture);
    view.set_capture_availability(if info.capture_available {
        slint_ui::launcher::InputAvailability::Available
    } else {
        slint_ui::launcher::InputAvailability::Unavailable
    });
    view.set_left_x(state.left_x);
    view.set_left_y(state.left_y);
    view.set_right_x(state.right_x);
    view.set_right_y(state.right_y);
    view.set_device_label(
        if pad.len() > 1 {
            format!("{} ({} pads)", pad.path(), pad.len())
        } else {
            pad.path().to_string()
        }
        .into(),
    );
    view.set_device_name(pad.db().display_label(info).into());
    view.set_usb_port(info.usb_port.clone().into());
    view.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    view.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    view.set_js_counts(
        format!(
            "js API: {} buttons, {} axes · evdev: {} keys, {} abs axes",
            info.js_buttons, info.js_axes, info.evdev_key_count, info.evdev_abs_count
        )
        .into(),
    );
    view.set_pressed_now(state.pressed_now.clone().into());
    view.set_last_event_label(state.last_event_label.clone().into());
    view.set_last_raw_event(state.last_raw.clone().into());
}

#[cfg(not(mister_ui_scope_launcher))]
pub(super) fn sync_device_info_controller(
    view: &slint_ui::controller::InputView,
    info: &PadInfo,
    db: &ControllerDb,
    js_path: &str,
    pad_count: usize,
) {
    let label = if pad_count > 1 {
        format!("{js_path} ({pad_count} pads)")
    } else {
        js_path.to_string()
    };
    view.set_device_label(label.into());
    view.set_device_name(db.display_label(info).into());
    view.set_usb_port(info.usb_port.clone().into());
    view.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    view.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    view.set_js_counts(
        format!(
            "js API: {} buttons, {} axes · evdev: {} keys, {} abs axes",
            info.js_buttons, info.js_axes, info.evdev_key_count, info.evdev_abs_count
        )
        .into(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_config_captures_start_state_and_bounded_timing() {
        let values = std::collections::BTreeMap::from([
            (START_SCREEN, "system-hub"),
            (START_SYSTEM, "  NeoGeo  "),
            (START_MENU, "consoles"),
            (LOCK_SCREEN, "arcade"),
            (PREVIEW_STEP_HOLD_SECS, "999"),
            (HUMAN_TURBO_IDLE_FRAMES, "999"),
            (HOME_SELECTED_INDEX, "7"),
            (AUTO_LAUNCH_SELECTED, "yes"),
        ]);
        let config = LauncherBenchmarkConfig::capture_with(|name| values.get(name).copied());

        assert_eq!(config.start_screen(), Some(Screen::SystemHub));
        assert_eq!(config.start_system(), Some("neogeo"));
        assert_eq!(config.start_menu(), Some("consoles"));
        assert_eq!(config.lock_screen(), Some(Screen::Arcade));
        assert_eq!(config.preview_step_hold_frames, 3_600);
        assert_eq!(config.human_turbo_idle_frames, 180);
        assert_eq!(config.home_selected(), Some(&Ok(7)));
        assert!(config.auto_launch_selected());
    }

    #[test]
    #[cfg(not(feature = "bench-tools"))]
    fn production_config_cannot_arm_a_benchmark_scenario() {
        let values = std::collections::BTreeMap::from([(BENCH_SCENARIO, "rapid-taps")]);
        let config = LauncherBenchmarkConfig::capture_with(|name| values.get(name).copied());

        assert_eq!(config.scenario(), None);
    }

    fn empty_catalog() -> ArcadeCatalog {
        ArcadeCatalog::new(PathBuf::from("/media/fat/_Arcade"), Vec::new(), Vec::new())
    }

    fn system(id: &str) -> arcade_catalog::GameSystemEntry {
        arcade_catalog::GameSystemEntry {
            id: id.to_string(),
            title: id.to_string(),
            count: 1,
        }
    }

    #[test]
    fn held_scroll_keeps_initial_press_when_summary_has_no_rows() {
        let catalog = empty_catalog();
        let mut nav = LauncherNav::new();
        let mut state = LauncherBenchState::default();
        let t0 = Instant::now();

        let ran_without_rows = launcher_bench_step(
            LauncherBenchScenario::HeldScroll,
            &LauncherBenchmarkConfig::default(),
            &mut nav,
            &catalog,
            Some(0),
            &mut state,
            t0,
        );
        state.advance_if(ran_without_rows);

        assert!(!ran_without_rows);
        assert_eq!(state.step, 0);
        assert_eq!(nav.arcade.selected, 0);
        assert!(!nav.arcade.is_scroll_active());

        let ran_with_rows = launcher_bench_step(
            LauncherBenchScenario::HeldScroll,
            &LauncherBenchmarkConfig::default(),
            &mut nav,
            &catalog,
            Some(10),
            &mut state,
            t0 + Duration::from_millis(16),
        );
        state.advance_if(ran_with_rows);

        assert!(ran_with_rows);
        assert_eq!(state.step, 1);
        assert_eq!(nav.arcade.selected, 1);
        assert!(nav.arcade.is_scroll_active());
    }

    #[test]
    fn home_repeat_hold_reverses_only_at_list_edges() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            Vec::new(),
            vec![system("arcade"), system("neogeo"), system("amiga")],
        );
        let mut nav = LauncherNav::new();
        let mut state = LauncherBenchState::default();
        let t0 = Instant::now();

        let ran = launcher_bench_step(
            LauncherBenchScenario::HomeRepeatHold,
            &LauncherBenchmarkConfig::default(),
            &mut nav,
            &catalog,
            None,
            &mut state,
            t0,
        );
        state.advance_if(ran);
        assert_eq!(nav.selected, 1);
        assert_eq!(state.home_repeat_dir, 1);

        let mut saw_right_edge = false;
        let mut saw_left_reversal = false;
        for frame in 1..100 {
            let previous_dir = state.home_repeat_dir;
            let previous_selected = nav.selected;
            let ran = launcher_bench_step(
                LauncherBenchScenario::HomeRepeatHold,
                &LauncherBenchmarkConfig::default(),
                &mut nav,
                &catalog,
                None,
                &mut state,
                t0 + Duration::from_millis(frame * 16),
            );
            state.advance_if(ran);
            if previous_dir > 0 && state.home_repeat_dir < 0 {
                assert_eq!(previous_selected, nav.current_menu_count() - 1);
                saw_right_edge = true;
            }
            if previous_dir < 0 && state.home_repeat_dir > 0 {
                assert_eq!(previous_selected, 0);
                saw_left_reversal = true;
            }
        }
        assert!(saw_right_edge);
        assert!(saw_left_reversal);
    }

    #[test]
    fn preview_idle_starts_on_arcade_without_running_steps() {
        let catalog = empty_catalog();
        let mut nav = LauncherNav::new();
        let mut state = LauncherBenchState::default();
        let ran = launcher_bench_step(
            LauncherBenchScenario::PreviewIdle,
            &LauncherBenchmarkConfig::default(),
            &mut nav,
            &catalog,
            Some(10),
            &mut state,
            Instant::now(),
        );

        assert!(LauncherBenchScenario::PreviewIdle.starts_on_arcade());
        assert_eq!(LauncherBenchScenario::PreviewIdle.period(), Duration::MAX);
        assert!(!ran);
        assert_eq!(nav.arcade.selected, 0);
        assert!(!nav.arcade.is_scroll_active());
    }
}
