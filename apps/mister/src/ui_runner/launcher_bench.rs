// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

const START_SCREEN: &str = "MISTER_LAUNCHER_START_SCREEN";
const START_SYSTEM: &str = "MISTER_LAUNCHER_START_SYSTEM";
const SYSTEM_ENTRY_BENCHMARK_SYSTEM: &str = "MISTER_SYSTEM_ENTRY_BENCHMARK_SYSTEM";
const LOCK_SCREEN: &str = "MISTER_LAUNCHER_LOCK_SCREEN";
const HOME_SELECTED_INDEX: &str = "MISTER_HOME_SELECTED_INDEX";
const AUTO_LAUNCH_SELECTED: &str = "MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED";
const LAUNCH_RETURN_PMU_HANDOFF_OUT: &str = "MISTER_LAUNCH_RETURN_PMU_HANDOFF_OUT";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LauncherBenchmarkConfig {
    start_screen: Option<Screen>,
    start_system: Option<String>,
    system_entry_system: Option<String>,
    lock_screen: Option<Screen>,
    home_selected: Option<Result<usize, String>>,
    auto_launch_selected: bool,
    launch_return_pmu_handoff_out: Option<String>,
}

impl Default for LauncherBenchmarkConfig {
    fn default() -> Self {
        Self::capture_with(|_| None)
    }
}

impl LauncherBenchmarkConfig {
    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a str>) -> Self {
        Self {
            start_screen: launcher_screen_from_value(get(START_SCREEN)),
            start_system: normalized_nonempty(get(START_SYSTEM)),
            system_entry_system: normalized_nonempty(get(SYSTEM_ENTRY_BENCHMARK_SYSTEM)),
            lock_screen: launcher_screen_from_value(get(LOCK_SCREEN)),
            home_selected: get(HOME_SELECTED_INDEX)
                .map(|value| value.parse::<usize>().map_err(|_| value.to_owned())),
            auto_launch_selected: get(AUTO_LAUNCH_SELECTED).is_some_and(benchmark_flag),
            launch_return_pmu_handoff_out: get(LAUNCH_RETURN_PMU_HANDOFF_OUT).map(str::to_owned),
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
}

fn normalized_nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn benchmark_flag(value: &str) -> bool {
    matches!(value, "1" | "on" | "true" | "yes")
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
