// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

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
const ORIENTATION_TRANSITIONS_BENCHMARK: &str = "MISTER_ORIENTATION_TRANSITIONS_BENCHMARK";
const ORIENTATION_TRANSITION_EFFECT: &str = "MISTER_ORIENTATION_TRANSITION_EFFECT";
const ORIENTATION_TRANSITIONS_REQUIRE_ANALYTICS: &str =
    "MISTER_ORIENTATION_TRANSITIONS_REQUIRE_ANALYTICS";
const SETTINGS_NAVIGATION_BENCHMARK: &str = "MISTER_SETTINGS_NAVIGATION_BENCHMARK";
const ARCADE_BENCHMARK_ORIENTATION: &str = "MISTER_ARCADE_BENCHMARK_ORIENTATION";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LauncherBenchmarkConfig {
    scenario: Option<LauncherBenchScenario>,
    start_screen: Option<Screen>,
    start_page_mode: launcher::SystemPageMode,
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
    orientation_transitions: bool,
    orientation_transition_effect: Option<OrientationTransitionEffect>,
    orientation_requires_analytics: bool,
    settings_navigation: bool,
    arcade_orientation: Option<ScreenOrientation>,
}

impl Default for LauncherBenchmarkConfig {
    fn default() -> Self {
        Self::capture_with(|_| None)
    }
}

impl LauncherBenchmarkConfig {
    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        let scenario = LauncherBenchScenario::from_value(get(BENCH_SCENARIO));
        Self {
            scenario,
            start_screen: launcher_screen_from_value(get(START_SCREEN)),
            start_page_mode: if matches!(get(START_SCREEN), Some("system-hub" | "snes-hub")) {
                launcher::SystemPageMode::Hub
            } else {
                launcher::SystemPageMode::List
            },
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
            orientation_transitions: get(ORIENTATION_TRANSITIONS_BENCHMARK)
                .is_some_and(benchmark_flag),
            orientation_transition_effect: get(ORIENTATION_TRANSITION_EFFECT)
                .and_then(OrientationTransitionEffect::from_id),
            orientation_requires_analytics: get(ORIENTATION_TRANSITIONS_REQUIRE_ANALYTICS)
                .is_some_and(benchmark_flag),
            settings_navigation: get(SETTINGS_NAVIGATION_BENCHMARK).is_some_and(benchmark_flag),
            arcade_orientation: get(ARCADE_BENCHMARK_ORIENTATION)
                .and_then(ScreenOrientation::parse),
        }
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
    pub(super) fn lock_screen(&self) -> Option<Screen> {
        self.lock_screen
    }
    pub(super) fn home_selected(&self) -> Option<&Result<usize, String>> {
        self.home_selected.as_ref()
    }
    pub(super) fn auto_launch_selected(&self) -> bool {
        self.auto_launch_selected
    }
    pub(super) fn launch_return_pmu_handoff_out(&self) -> Option<&str> {
        self.launch_return_pmu_handoff_out.as_deref()
    }
    pub(super) fn orientation_transitions(&self) -> bool {
        self.orientation_transitions
    }
    pub(super) fn settings_navigation(&self) -> bool {
        self.settings_navigation
    }
    pub(super) fn arcade_orientation(&self) -> Option<ScreenOrientation> {
        self.arcade_orientation
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
    #[cfg_attr(
        not(any(feature = "bench-tools", feature = "diagnostics")),
        allow(dead_code)
    )]
    Idle,
    #[cfg_attr(
        not(any(feature = "bench-tools", feature = "diagnostics")),
        allow(dead_code)
    )]
    PreviewIdle,
    #[cfg_attr(
        not(any(feature = "bench-tools", feature = "diagnostics")),
        allow(dead_code)
    )]
    HomeNav,
    #[cfg_attr(
        not(any(feature = "bench-tools", feature = "diagnostics")),
        allow(dead_code)
    )]
    HeldScroll,
    #[cfg_attr(
        not(any(feature = "bench-tools", feature = "diagnostics")),
        allow(dead_code)
    )]
    TurboHold,
    #[cfg_attr(
        not(any(feature = "bench-tools", feature = "diagnostics")),
        allow(dead_code)
    )]
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
}

fn launcher_screen_from_value(value: Option<&str>) -> Option<Screen> {
    match value?.to_ascii_lowercase().as_str() {
        "home" => Some(Screen::Home),
        "system-hub" | "snes-hub" => Some(Screen::Arcade),
        "arcade" => Some(Screen::Arcade),
        "controller" | "controller-test" | "controller_test" => Some(Screen::Controller),
        "settings" => Some(Screen::Settings),
        "about" => Some(Screen::About),
        "licenses" => Some(Screen::Licenses),
        "license-text" => Some(Screen::LicenseText),
        _ => None,
    }
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
