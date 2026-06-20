use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LauncherBenchScenario {
    Idle,
    HomeNav,
    QuickTap,
    RapidTaps,
    HeldScroll,
    TurboHold,
    PreviewStepHold,
    ModelSync,
}

impl LauncherBenchScenario {
    pub(super) fn from_env() -> Option<Self> {
        match std::env::var("MISTER_LAUNCHER_BENCH_SCENARIO")
            .ok()?
            .to_ascii_lowercase()
            .as_str()
        {
            "idle" => Some(Self::Idle),
            "home-nav" | "home_nav" => Some(Self::HomeNav),
            "velocity-scroll" | "velocity_scroll" => Some(Self::HeldScroll),
            "quick-tap" | "quick_tap" => Some(Self::QuickTap),
            "rapid-taps" | "rapid_taps" => Some(Self::RapidTaps),
            "held-scroll" | "held_scroll" => Some(Self::HeldScroll),
            "turbo-hold" | "turbo_hold" => Some(Self::TurboHold),
            "preview-step-hold" | "preview_step_hold" | "step-hold" | "step_hold" => {
                Some(Self::PreviewStepHold)
            }
            "model-sync" | "model_sync" => Some(Self::ModelSync),
            _ => None,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::HomeNav => "home-nav",
            Self::QuickTap => "quick-tap",
            Self::RapidTaps => "rapid-taps",
            Self::HeldScroll => "held-scroll",
            Self::TurboHold => "turbo-hold",
            Self::PreviewStepHold => "preview-step-hold",
            Self::ModelSync => "model-sync",
        }
    }

    pub(super) fn period(self) -> Duration {
        match self {
            Self::Idle => Duration::MAX,
            Self::HomeNav => Duration::from_millis(300),
            Self::ModelSync => Duration::from_millis(300),
            Self::QuickTap
            | Self::RapidTaps
            | Self::HeldScroll
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
                | Self::TurboHold
                | Self::PreviewStepHold
        )
    }
}

pub(super) fn launcher_start_screen_from_env() -> Option<Screen> {
    launcher_screen_from_env("MISTER_LAUNCHER_START_SCREEN")
}

pub(super) fn launcher_lock_screen_from_env() -> Option<Screen> {
    launcher_screen_from_env("MISTER_LAUNCHER_LOCK_SCREEN")
}

fn launcher_screen_from_env(name: &str) -> Option<Screen> {
    match std::env::var(name).ok()?.to_ascii_lowercase().as_str() {
        "home" => Some(Screen::Home),
        "arcade" => Some(Screen::Arcade),
        "controller" | "controller-test" | "controller_test" => Some(Screen::Controller),
        "settings" => Some(Screen::Settings),
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

pub(super) fn launcher_bench_step(
    scenario: LauncherBenchScenario,
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    active_game_count: Option<usize>,
    step: usize,
    now: Instant,
) -> bool {
    match scenario {
        LauncherBenchScenario::Idle => false,
        LauncherBenchScenario::HomeNav => {
            let count = catalog.systems.len();
            if count == 0 {
                return false;
            }
            nav.screen = Screen::Home;
            nav.settings_focused = false;
            let selected = step % count;
            if selected < nav.selected {
                nav.scroll_x = 0;
            }
            nav.selected = selected;
            keep_bench_home_visible(&mut nav.scroll_x, nav.selected, count);
            true
        }
        LauncherBenchScenario::ModelSync => {
            let count = catalog.systems.len();
            if count == 0 {
                return false;
            }
            let selected = (step / 2) % count;
            if selected < nav.selected {
                nav.scroll_x = 0;
            }
            nav.selected = selected;
            nav.settings_focused = false;
            if step % 2 == 0 {
                nav.screen = Screen::Home;
                keep_bench_home_visible(&mut nav.scroll_x, nav.selected, count);
            } else {
                nav.screen = Screen::Arcade;
                let game_count = catalog.system_game_count(&catalog.systems[selected].id);
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
            let previous_dir = if step == 0 { 0 } else { 1 };
            nav.arcade.bench_direction_tick(1, previous_dir, count, now);
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
            if step % preview_step_hold_frames() == 0 {
                nav.arcade.handle_direction_input(1, 0, now, count);
            }
            nav.arcade.tick(count);
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
            let (dir, previous_dir) = match step {
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
            let (dir, previous_dir) = if step < 10 {
                if step % 2 == 0 {
                    (1, 0)
                } else {
                    (0, 1)
                }
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
    let system = catalog.systems.get(nav.selected)?;
    Some(catalog.system_game_count(&system.id))
}

pub(super) fn keep_bench_home_visible(scroll_x: &mut i32, selected: usize, count: usize) {
    let item_w = HOME_TILE_WIDTH + HOME_TILE_GAP;
    let selected_x = selected as i32 * item_w;
    let selected_right = selected_x + HOME_TILE_WIDTH;
    if selected_x < *scroll_x {
        *scroll_x = selected_x;
    }
    if selected_right > *scroll_x + HOME_LIST_VISIBLE_W {
        *scroll_x = selected_right - HOME_LIST_VISIBLE_W;
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
        let idx = setup.target_pad_idx;
        let js_path = pad.path_at(idx);

        if setup.phase == SetupPhase::Configure {
            let fields = SetupNav::configure_fields(info, js_path, db);
            let labels: Vec<SharedString> = fields.iter().map(|(k, _)| k.clone().into()).collect();
            let values: Vec<SharedString> = fields.iter().map(|(_, v)| v.clone().into()).collect();
            bridge.set_setup_config_labels(ModelRc::new(VecModel::from(labels)));
            bridge.set_setup_config_values(ModelRc::new(VecModel::from(values)));
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            let live = SetupNav::configure_live_hint(pad.state_at(idx));
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
