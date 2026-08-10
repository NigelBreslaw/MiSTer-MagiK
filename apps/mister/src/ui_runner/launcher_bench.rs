// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use crate::input_state::PadState;

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
    pub(super) fn from_env() -> Option<Self> {
        #[cfg(not(feature = "bench-tools"))]
        {
            None
        }
        #[cfg(feature = "bench-tools")]
        {
            match std::env::var("MISTER_LAUNCHER_BENCH_SCENARIO")
                .ok()?
                .to_ascii_lowercase()
                .as_str()
            {
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

pub(super) fn launcher_start_screen_from_env() -> Option<Screen> {
    launcher_screen_from_env("MISTER_LAUNCHER_START_SCREEN")
}

pub(super) fn launcher_start_system_from_env() -> Option<String> {
    std::env::var("MISTER_LAUNCHER_START_SYSTEM")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

pub(super) fn launcher_system_entry_benchmark_system_from_env() -> Option<String> {
    std::env::var("MISTER_SYSTEM_ENTRY_BENCHMARK_SYSTEM")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

pub(super) fn launcher_system_entry_benchmark_direct_from_env() -> bool {
    launcher_env_flag("MISTER_SYSTEM_ENTRY_BENCHMARK_DIRECT")
}

pub(super) fn launcher_start_menu_from_env() -> Option<String> {
    std::env::var("MISTER_LAUNCHER_START_MENU")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| {
            matches!(
                value.as_str(),
                "consoles" | "handhelds" | "computers" | "snk-neogeo"
            )
        })
}

pub(super) fn launcher_lock_screen_from_env() -> Option<Screen> {
    launcher_screen_from_env("MISTER_LAUNCHER_LOCK_SCREEN")
}

pub(super) fn launcher_bench_after_input_script_enabled() -> bool {
    matches!(
        std::env::var("MISTER_LAUNCHER_BENCH_AFTER_INPUT_SCRIPT")
            .ok()
            .as_deref(),
        Some("1") | Some("on") | Some("true") | Some("yes")
    )
}

fn launcher_screen_from_env(name: &str) -> Option<Screen> {
    match std::env::var(name).ok()?.to_ascii_lowercase().as_str() {
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

pub(super) fn preview_step_hold_frames() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        let secs = std::env::var("MISTER_PREVIEW_STEP_HOLD_SECS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(5)
            .clamp(1, 60);
        secs.saturating_mul(60).max(1)
    })
}

fn human_turbo_idle_frames() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_HUMAN_TURBO_IDLE_FRAMES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(30)
            .min(180)
    })
}

fn human_turbo_normal_frames() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_HUMAN_TURBO_NORMAL_FRAMES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(30)
            .min(300)
    })
}

fn human_turbo_pause_frames() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_HUMAN_TURBO_PAUSE_FRAMES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(30)
            .min(300)
    })
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
            let idle_frames = human_turbo_idle_frames();
            let normal_frames = human_turbo_normal_frames();
            let pause_frames = human_turbo_pause_frames();
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
            if state.step % preview_step_hold_frames() == 0 {
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
    bridge: &slint_ui::launcher::MisterBridge,
    pad: &PadPool,
    setup: &SetupNav,
) {
    let info = setup_pad_info(pad, setup);
    let db = pad.db();
    let active = setup.phase != SetupPhase::None;
    bridge.set_setup_visible(active);
    bridge.set_setup_phase(setup.phase as i32);
    if active {
        bridge.set_setup_title(setup.title().into());
        bridge.set_setup_selected(setup.list_index as i32);
        let js_path = setup
            .target_device
            .as_ref()
            .and_then(|device| pad.path_for_device(device))
            .unwrap_or("(controller disconnected)");

        if setup.phase == SetupPhase::Configure {
            let fields = SetupNav::configure_fields(info, js_path, db);
            let labels: Vec<SharedString> = fields.iter().map(|(k, _)| k.clone().into()).collect();
            let values: Vec<SharedString> = fields.iter().map(|(_, v)| v.clone().into()).collect();
            bridge.set_setup_config_labels(ModelRc::new(VecModel::from(labels)));
            bridge.set_setup_config_values(ModelRc::new(VecModel::from(values)));
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            let live = setup
                .target_device
                .as_ref()
                .and_then(|device| pad.state_for_device(device))
                .map(SetupNav::configure_live_hint)
                .unwrap_or_else(|| "Controller disconnected".into());
            bridge.set_setup_subtitle(live.into());
            bridge.set_setup_name(String::new().into());
            bridge.set_setup_kind_label(String::new().into());
        } else if setup.phase == SetupPhase::NameKind {
            bridge.set_setup_subtitle(setup.subtitle(info, db).into());
            bridge.set_setup_name(setup.draft_label.clone().into());
            bridge.set_setup_kind_label(setup.draft_kind_label().into());
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_labels(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_values(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
        } else if setup.phase == SetupPhase::PickExisting {
            bridge.set_setup_subtitle(setup.subtitle(info, db).into());
            bridge
                .set_setup_config_labels(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_values(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            let rows: Vec<SharedString> = db
                .list_entries()
                .iter()
                .map(|item| {
                    let port = if item.last_usb_port.is_empty() {
                        "unknown port".to_string()
                    } else {
                        format!("was {}", item.last_usb_port)
                    };
                    format!("{} — {}", item.label, port).into()
                })
                .collect();
            bridge.set_setup_list(ModelRc::new(VecModel::from(rows)));
        } else {
            bridge.set_setup_subtitle(setup.subtitle(info, db).into());
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_labels(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_values(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge.set_setup_name(String::new().into());
            bridge.set_setup_kind_label(String::new().into());
        }
    }
}

pub(super) fn sync_bridge_pad_controller(
    bridge: &slint_ui::controller::MisterBridge,
    pad: &PadPool,
) {
    let state = pad.state();
    let info = pad.info();
    bridge.set_dpad_up(state.dpad_up);
    bridge.set_dpad_down(state.dpad_down);
    bridge.set_dpad_left(state.dpad_left);
    bridge.set_dpad_right(state.dpad_right);
    bridge.set_btn_a(state.btn_a);
    bridge.set_btn_b(state.btn_b);
    bridge.set_btn_x(state.btn_x);
    bridge.set_btn_y(state.btn_y);
    bridge.set_btn_l(state.btn_l);
    bridge.set_btn_r(state.btn_r);
    bridge.set_btn_zl(state.btn_zl);
    bridge.set_btn_zr(state.btn_zr);
    bridge.set_btn_select(state.btn_select);
    bridge.set_btn_start(state.btn_start);
    bridge.set_btn_l3(state.btn_l3);
    bridge.set_btn_r3(state.btn_r3);
    bridge.set_btn_home(state.btn_home);
    bridge.set_btn_capture(state.btn_capture);
    bridge.set_capture_available(info.capture_available);
    bridge.set_left_x(state.left_x);
    bridge.set_left_y(state.left_y);
    bridge.set_right_x(state.right_x);
    bridge.set_right_y(state.right_y);
    sync_device_info_controller(bridge, info, pad.db(), pad.path(), pad.len());
    bridge.set_pressed_now(state.pressed_now.clone().into());
    bridge.set_last_event_label(state.last_event_label.clone().into());
    bridge.set_last_raw_event(state.last_raw.clone().into());
}

pub(super) fn sync_bridge_pad_launcher(bridge: &slint_ui::launcher::MisterBridge, pad: &PadPool) {
    let state = pad.state();
    let info = pad.info();
    bridge.set_dpad_up(state.dpad_up);
    bridge.set_dpad_down(state.dpad_down);
    bridge.set_dpad_left(state.dpad_left);
    bridge.set_dpad_right(state.dpad_right);
    bridge.set_btn_a(state.btn_a);
    bridge.set_btn_b(state.btn_b);
    bridge.set_btn_x(state.btn_x);
    bridge.set_btn_y(state.btn_y);
    bridge.set_btn_l(state.btn_l);
    bridge.set_btn_r(state.btn_r);
    bridge.set_btn_zl(state.btn_zl);
    bridge.set_btn_zr(state.btn_zr);
    bridge.set_btn_select(state.btn_select);
    bridge.set_btn_start(state.btn_start);
    bridge.set_btn_l3(state.btn_l3);
    bridge.set_btn_r3(state.btn_r3);
    bridge.set_btn_home(state.btn_home);
    bridge.set_btn_capture(state.btn_capture);
    bridge.set_capture_available(info.capture_available);
    bridge.set_left_x(state.left_x);
    bridge.set_left_y(state.left_y);
    bridge.set_right_x(state.right_x);
    bridge.set_right_y(state.right_y);
    sync_device_info_launcher(bridge, info, pad.db(), pad.path(), pad.len());
    bridge.set_pressed_now(state.pressed_now.clone().into());
    bridge.set_last_event_label(state.last_event_label.clone().into());
    bridge.set_last_raw_event(state.last_raw.clone().into());
}

pub(super) fn sync_device_info_controller(
    bridge: &slint_ui::controller::MisterBridge,
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
    bridge.set_device_label(label.into());
    bridge.set_device_name(db.display_label(info).into());
    bridge.set_usb_port(info.usb_port.clone().into());
    bridge.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    bridge.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    bridge.set_js_counts(
        format!(
            "js API: {} buttons, {} axes · evdev: {} keys, {} abs axes",
            info.js_buttons, info.js_axes, info.evdev_key_count, info.evdev_abs_count
        )
        .into(),
    );
}

pub(super) fn sync_device_info_launcher(
    bridge: &slint_ui::launcher::MisterBridge,
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
    bridge.set_device_label(label.into());
    bridge.set_device_name(db.display_label(info).into());
    bridge.set_usb_port(info.usb_port.clone().into());
    bridge.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    bridge.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    bridge.set_js_counts(
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
