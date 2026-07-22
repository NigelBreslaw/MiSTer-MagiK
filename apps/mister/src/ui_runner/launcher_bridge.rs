// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

macro_rules! set_bridge_if_changed {
    ($bridge:expr, $getter:ident, $setter:ident, $value:expr) => {{
        let value = $value;
        if $bridge.$getter() != value {
            $bridge.$setter(value);
        }
    }};
}

macro_rules! set_bridge_string_if_changed {
    ($bridge:expr, $getter:ident, $setter:ident, $value:expr) => {{
        let value: SharedString = ($value).into();
        if $bridge.$getter() != value {
            $bridge.$setter(value);
        }
    }};
}

pub(super) fn open_pads() -> PadPool {
    PadPool::open_all().unwrap_or_else(|e| {
        crate::ui_errln!("failed to initialize gamepad input: {e}");
        std::process::exit(1);
    })
}

pub(super) fn init_launcher_bridge(app: &slint_ui::launcher::Launcher, pad: &PadPool) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_startup_visible(true);
    bridge.set_screen_mode(0);
    bridge.set_build_label(build_label().into());
    bridge.set_present_mode_label("Mode=/dev/fb0".into());
    bridge.set_info_kernel_version(kernel_version().into());
    bridge.set_info_database_build(last_database_build().into());
    bridge.set_selected_index(0);
    bridge.set_settings_focused(false);
    bridge.set_settings_selected(0);
    bridge.set_about_selected(0);
    bridge.set_display_options(ModelRc::new(VecModel::from(
        mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
            .iter()
            .map(|mode| SharedString::from(mode.label))
            .collect::<Vec<_>>(),
    )));
    bridge.set_simple_joystick_handling(false);
    bridge.set_licenses_selected(0);
    bridge.set_licenses_expanded(false);
    bridge.set_licenses_scroll_y(0);
    bridge.set_license_lines(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
    bridge.set_confirm_visible(false);
    bridge.set_confirm_title("".into());
    bridge.set_confirm_message("".into());
    bridge.set_confirm_left_label("".into());
    bridge.set_confirm_right_label("".into());
    bridge.set_confirm_selected(0);
    bridge.set_menu_title("MiSTer MagiK".into());
    bridge.set_menu_breadcrumb("".into());
    bridge.set_dev_mode(
        mister_magik_catalog::device_layout::DeviceLayout::current()
            == mister_magik_catalog::device_layout::DeviceLayout::Dev,
    );
    bridge.set_menu_items(ModelRc::new(VecModel::from(Vec::<
        slint_ui::launcher::MenuItem,
    >::new())));
    bridge.set_home_scroll_repeat_active(false);
    bridge.set_home_scroll_held(false);
    bridge.set_home_scroll_x(0);
    bridge.set_active_system_title("".into());
    bridge.set_active_system_count(0);
    bridge.set_arcade_games(ModelRc::new(VecModel::from(Vec::<
        slint_ui::launcher::ArcadeGame,
    >::new())));
    bridge.set_arcade_selected(0);
    bridge.set_arcade_scroll_y(0);
    sync_launcher_arcade_geometry_bridge(&bridge);
    bridge.set_arcade_games_loading(false);
    bridge.set_arcade_search_keys(ModelRc::new(VecModel::from(
        crate::launcher::ARCADE_SEARCH_KEYS
            .iter()
            .map(|key| SharedString::from(key.label))
            .collect::<Vec<_>>(),
    )));
    bridge.set_arcade_preview_placeholder_visible(true);
    bridge.set_arcade_preview_status(PreviewStatus::Empty);
    bridge.set_arcade_preview_title("".into());
    bridge.set_arcade_preview_run_label(preview_run_label().into());
    bridge.set_arcade_preview_source_width(0);
    bridge.set_arcade_preview_source_height(0);
    bridge.set_arcade_preview_display_width(0);
    bridge.set_arcade_preview_display_height(0);
    LauncherStatusPresenter::new(&bridge).init();
    sync_bridge_pad_launcher(&bridge, pad);
}

pub(super) fn sync_settings_bridge(
    app: &slint_ui::launcher::Launcher,
    nav: &LauncherNav,
    lifecycle: &LauncherLifecycle,
) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    set_bridge_if_changed!(
        bridge,
        get_screen_mode,
        set_screen_mode,
        match nav.screen {
            Screen::Home => 0,
            Screen::Controller => 1,
            Screen::Arcade => 2,
            Screen::Settings => 3,
            Screen::About => 4,
            Screen::Licenses => 5,
            Screen::Info => 6,
            Screen::Screensaver => 7,
        }
    );
    set_bridge_if_changed!(
        bridge,
        get_settings_selected,
        set_settings_selected,
        nav.settings_selected as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_about_selected,
        set_about_selected,
        nav.about_selected as i32
    );
    let active_label = mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
        .get(nav.display_selected)
        .map_or("Custom/current mode", |mode| mode.label);
    set_bridge_string_if_changed!(
        bridge,
        get_display_active_label,
        set_display_active_label,
        active_label
    );
    set_bridge_if_changed!(
        bridge,
        get_display_combo_open,
        set_display_combo_open,
        nav.display_combo_open
    );
    set_bridge_if_changed!(
        bridge,
        get_display_selected,
        set_display_selected,
        nav.display_selected as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_display_highlighted,
        set_display_highlighted,
        nav.display_highlighted as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_display_confirm_remaining,
        set_display_confirm_remaining,
        nav.display_confirm_remaining as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_simple_joystick_handling,
        set_simple_joystick_handling,
        nav.settings.simple_joystick_handling
    );
    set_bridge_if_changed!(
        bridge,
        get_screensaver_settings_selected,
        set_screensaver_settings_selected,
        nav.screensaver_selected as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_screensaver_enabled,
        set_screensaver_enabled,
        nav.settings.screensaver_enabled
    );
    set_bridge_if_changed!(
        bridge,
        get_screensaver_delay_minutes,
        set_screensaver_delay_minutes,
        nav.settings.screensaver_delay_minutes as i32
    );
    if matches!(nav.screen, Screen::Settings | Screen::Screensaver)
        || matches!(
            nav.confirm_action,
            Some(
                launcher::ConfirmAction::DisplayResolution
                    | launcher::ConfirmAction::DisplayResolutionError
            )
        )
    {
        sync_launcher_confirm_bridge(&bridge, nav, lifecycle);
    }
}

fn build_label() -> String {
    let version = env!("MISTER_MAGIK_VERSION");
    let build_time = env!("MISTER_MAGIK_BUILD_TIME");
    format_build_label(version, build_time)
}

fn format_build_label(version: &str, build_time: &str) -> String {
    format!("Version {version}  {build_time}")
}

fn kernel_version() -> String {
    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|version| format!("Linux {}", version.trim()))
        .filter(|version| version != "Linux ")
        .unwrap_or_else(|| "Kernel version unavailable".to_string())
}

fn last_database_build() -> String {
    mister_magik_catalog::catalog_build_record::read_completed_build_duration(
        &mister_magik_catalog::catalog_state::default_path(),
    )
    .ok()
    .flatten()
    .map(mister_magik_catalog::catalog_build_record::format_duration)
    .unwrap_or_else(|| "No completed database build recorded yet".to_string())
}

pub(super) fn sync_launcher_arcade_geometry_bridge(bridge: &slint_ui::launcher::MisterBridge) {
    bridge.set_arcade_list_x(ARCADE_LIST_X as i32);
    bridge.set_arcade_list_y(ARCADE_LIST_Y as i32);
    bridge.set_arcade_list_width(ARCADE_LIST_W as i32);
    bridge.set_arcade_list_height(ARCADE_LIST_H as i32);
    bridge.set_arcade_list_visible(true);
    bridge.set_arcade_preview_box_x(ARCADE_PREVIEW_BOX_X as i32);
    bridge.set_arcade_preview_box_y(ARCADE_PREVIEW_BOX_Y as i32);
    bridge.set_arcade_preview_box_width(ARCADE_PREVIEW_BOX_W as i32);
    bridge.set_arcade_preview_box_height(ARCADE_PREVIEW_BOX_H as i32);
}

fn sync_arcade_list_geometry_bridge(
    bridge: &slint_ui::launcher::MisterBridge,
    nav: &LauncherNav,
    render_w: usize,
) {
    let geometry = if nav.arcade_search.is_active(&nav.arcade_filter.active) {
        ArcadeListGeometry::search_for_render_w(render_w)
    } else {
        ArcadeListGeometry::NORMAL
    };
    bridge.set_arcade_list_x(geometry.x as i32);
    bridge.set_arcade_list_y(geometry.y as i32);
    bridge.set_arcade_list_width(geometry.width as i32);
}

pub(super) struct CatalogScanBridgeStatus {
    visible: bool,
    background_visible: bool,
    message: SharedString,
    title: SharedString,
    detail: SharedString,
    percent: i32,
}

impl CatalogScanBridgeStatus {
    pub(super) fn new(
        visible: bool,
        background_visible: bool,
        message: impl Into<SharedString>,
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        percent: i32,
    ) -> Self {
        Self {
            visible,
            background_visible,
            message: message.into(),
            title: title.into(),
            detail: detail.into(),
            percent,
        }
    }

    #[cfg(test)]
    pub(super) fn visible(&self) -> bool {
        self.visible
    }

    #[cfg(test)]
    pub(super) fn background_visible(&self) -> bool {
        self.background_visible
    }

    #[cfg(test)]
    pub(super) fn title(&self) -> &str {
        self.title.as_str()
    }

    #[cfg(test)]
    pub(super) fn detail(&self) -> &str {
        self.detail.as_str()
    }
}

pub(super) struct LauncherStatusPresenter<'a, 'b> {
    bridge: &'a slint_ui::launcher::MisterBridge<'b>,
}

impl<'a, 'b> LauncherStatusPresenter<'a, 'b> {
    pub(super) fn new(bridge: &'a slint_ui::launcher::MisterBridge<'b>) -> Self {
        Self { bridge }
    }

    pub(super) fn init(&self) {
        self.sync_loading("", "");
        self.sync_catalog_scan(CatalogScanBridgeStatus::new(
            false,
            false,
            FIRST_LIBRARY_SCAN_MESSAGE,
            "",
            "",
            -1,
        ));
        self.sync_media_progresses(empty_media_pack_progress_model(), "");
        self.sync_setup_visible(false);
        self.bridge.set_setup_phase(0);
    }

    pub(super) fn sync_loading(
        &self,
        message: impl Into<SharedString>,
        detail: impl Into<SharedString>,
    ) {
        set_bridge_string_if_changed!(
            self.bridge,
            get_loading_message,
            set_loading_message,
            message
        );
        set_bridge_string_if_changed!(self.bridge, get_loading_detail, set_loading_detail, detail);
    }

    pub(super) fn sync_catalog_scan(&self, status: CatalogScanBridgeStatus) {
        self.bridge.set_catalog_scan_visible(status.visible);
        self.bridge
            .set_catalog_background_scan_visible(status.background_visible);
        self.bridge.set_catalog_scan_message(status.message);
        self.bridge.set_catalog_scan_title(status.title);
        self.bridge.set_catalog_scan_detail(status.detail);
        self.bridge.set_catalog_scan_percent(status.percent);
    }

    pub(super) fn clear_catalog_scan(&self) {
        self.bridge.set_catalog_scan_visible(false);
        self.bridge.set_catalog_background_scan_visible(false);
        self.bridge.set_catalog_scan_title("".into());
        self.bridge.set_catalog_scan_detail("".into());
        self.bridge.set_catalog_scan_percent(-1);
    }

    pub(super) fn sync_catalog_background_scan_visible(&self, visible: bool) {
        self.bridge.set_catalog_background_scan_visible(visible);
    }

    pub(super) fn sync_catalog_scan_detail(&self, detail: impl Into<SharedString>) {
        self.bridge.set_catalog_scan_detail(detail.into());
    }

    pub(super) fn sync_media_progresses(
        &self,
        progresses: ModelRc<slint_ui::launcher::ScreenshotPackProgress>,
        summary: impl Into<SharedString>,
    ) {
        self.bridge.set_media_pack_progresses(progresses);
        self.bridge.set_media_pack_summary(summary.into());
    }

    pub(super) fn sync_setup_visible(&self, visible: bool) {
        self.bridge.set_setup_visible(visible);
    }
}

fn empty_media_pack_progress_model() -> ModelRc<slint_ui::launcher::ScreenshotPackProgress> {
    ModelRc::new(VecModel::from(Vec::<
        slint_ui::launcher::ScreenshotPackProgress,
    >::new()))
}

pub(super) fn sync_bridge(app: &slint_ui::controller::ControllerTest, pad: &PadPool) {
    sync_bridge_pad_controller(&app.global::<slint_ui::controller::MisterBridge>(), pad);
}

pub(super) fn sync_confirm_bridge(
    bridge: &slint_ui::launcher::MisterBridge,
    action: Option<launcher::ConfirmAction>,
) {
    let text = confirm_bridge_text(action);
    set_bridge_string_if_changed!(bridge, get_confirm_title, set_confirm_title, text.title);
    set_bridge_string_if_changed!(
        bridge,
        get_confirm_message,
        set_confirm_message,
        text.message
    );
    set_bridge_string_if_changed!(
        bridge,
        get_confirm_left_label,
        set_confirm_left_label,
        text.left_label
    );
    set_bridge_string_if_changed!(
        bridge,
        get_confirm_right_label,
        set_confirm_right_label,
        text.right_label
    );
}

fn sync_launcher_confirm_bridge(
    bridge: &slint_ui::launcher::MisterBridge,
    nav: &LauncherNav,
    lifecycle: &LauncherLifecycle,
) {
    if let Some(dialog) = lifecycle.view().launch_failure_dialog() {
        set_bridge_if_changed!(bridge, get_confirm_visible, set_confirm_visible, true);
        set_bridge_if_changed!(bridge, get_confirm_selected, set_confirm_selected, 0);
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_title,
            set_confirm_title,
            dialog.title.as_str()
        );
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_message,
            set_confirm_message,
            dialog.message
        );
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_left_label,
            set_confirm_left_label,
            "Back"
        );
        set_bridge_string_if_changed!(bridge, get_confirm_right_label, set_confirm_right_label, "");
        return;
    }
    if let Some(dialog) = lifecycle.view().catalog_recovery_dialog() {
        set_bridge_if_changed!(bridge, get_confirm_visible, set_confirm_visible, true);
        set_bridge_if_changed!(
            bridge,
            get_confirm_selected,
            set_confirm_selected,
            dialog.selected.selected_index()
        );
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_title,
            set_confirm_title,
            "Library failed to load."
        );
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_message,
            set_confirm_message,
            dialog.error.as_str()
        );
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_left_label,
            set_confirm_left_label,
            "Retry"
        );
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_right_label,
            set_confirm_right_label,
            "Rebuild"
        );
        return;
    }
    set_bridge_if_changed!(
        bridge,
        get_confirm_visible,
        set_confirm_visible,
        nav.confirm_action.is_some()
    );
    set_bridge_if_changed!(
        bridge,
        get_confirm_selected,
        set_confirm_selected,
        nav.confirm_selected as i32
    );
    sync_confirm_bridge(bridge, nav.confirm_action);
    if nav.confirm_action == Some(launcher::ConfirmAction::DisplayResolution) {
        let label = format!("Cancel ({})", nav.display_confirm_remaining);
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_left_label,
            set_confirm_left_label,
            &label
        );
        if nav.display_confirm_busy {
            set_bridge_string_if_changed!(
                bridge,
                get_confirm_message,
                set_confirm_message,
                "Saving the new resolution…"
            );
            set_bridge_string_if_changed!(
                bridge,
                get_confirm_right_label,
                set_confirm_right_label,
                "Saving…"
            );
        } else if let Some(error) = nav.display_error.as_deref() {
            let message = format!("Could not save the resolution: {error}. Retry or cancel.");
            set_bridge_string_if_changed!(
                bridge,
                get_confirm_title,
                set_confirm_title,
                "Resolution change failed"
            );
            set_bridge_string_if_changed!(
                bridge,
                get_confirm_message,
                set_confirm_message,
                &message
            );
            set_bridge_string_if_changed!(
                bridge,
                get_confirm_right_label,
                set_confirm_right_label,
                "Retry"
            );
        }
    } else if nav.confirm_action == Some(launcher::ConfirmAction::DisplayResolutionError) {
        if let Some(error) = nav.display_error.as_deref() {
            set_bridge_string_if_changed!(bridge, get_confirm_message, set_confirm_message, error);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConfirmBridgeText {
    title: &'static str,
    message: &'static str,
    left_label: &'static str,
    right_label: &'static str,
}

fn confirm_bridge_text(action: Option<launcher::ConfirmAction>) -> ConfirmBridgeText {
    match action {
        Some(launcher::ConfirmAction::ExitToMister) => ConfirmBridgeText {
            title: "Exit to MiSTer",
            message: "Use the stock MiSTer menu until reboot.",
            left_label: "Cancel",
            right_label: "Exit to MiSTer",
        },
        Some(launcher::ConfirmAction::ResetDatabase) => ConfirmBridgeText {
            title: "Reset Database?",
            message: "Delete the library database, screenshot packs, and reboot the MiSTer?",
            left_label: "Cancel",
            right_label: "Confirm",
        },
        Some(launcher::ConfirmAction::Restart) => ConfirmBridgeText {
            title: "Restart MiSTer?",
            message: "Reboot the MiSTer now?",
            left_label: "Cancel",
            right_label: "Confirm",
        },
        Some(launcher::ConfirmAction::LibraryChanged) => ConfirmBridgeText {
            title: "Library changed",
            message: "New games detected. Continue with the current library or rebuild now.",
            left_label: "Continue",
            right_label: "Rebuild",
        },
        Some(launcher::ConfirmAction::LibraryUpdateFailed) => ConfirmBridgeText {
            title: "Library update failed",
            message: "Continuing with the current library. Try rebuilding again later.",
            left_label: "OK",
            right_label: "",
        },
        Some(launcher::ConfirmAction::DisplayResolution) => ConfirmBridgeText {
            title: "Confirm new resolution works",
            message: "Keep this display resolution? It will be restored automatically if you cannot see this dialog.",
            left_label: "Cancel (10)",
            right_label: "Confirm",
        },
        Some(launcher::ConfirmAction::DisplayResolutionError) => ConfirmBridgeText {
            title: "Resolution change failed",
            message: "The display resolution could not be changed.",
            left_label: "OK",
            right_label: "",
        },
        None => ConfirmBridgeText {
            title: "",
            message: "",
            left_label: "",
            right_label: "",
        },
    }
}

pub(super) fn sync_bridge_launcher(
    app: &slint_ui::launcher::Launcher,
    pad: &PadPool,
    nav: &LauncherNav,
    lifecycle: &LauncherLifecycle,
    setup: &SetupNav,
    loading_message: &str,
    loading_detail: &str,
    catalog: Option<&ArcadeCatalog>,
    preview: &mut PreviewState,
    models: &mut LauncherBridgeModels,
    catalog_version: usize,
    defer_selected_preview: bool,
    render_w: usize,
) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_startup_visible(false);
    sync_bridge_pad_launcher(&bridge, pad);
    bridge.set_screen_mode(match nav.screen {
        Screen::Home => 0,
        Screen::Controller => 1,
        Screen::Arcade => 2,
        Screen::Settings => 3,
        Screen::About => 4,
        Screen::Licenses => 5,
        Screen::Info => 6,
        Screen::Screensaver => 7,
    });
    bridge.set_clock_text(launcher_clock_text().into());
    bridge.set_selected_index(nav.selected as i32);
    bridge.set_home_scroll_held(nav.home_horizontal_held());
    bridge.set_home_scroll_repeat_active(nav.home_horizontal_repeat_active());
    bridge.set_home_scroll_x(nav.scroll_x);
    bridge.set_menu_title(nav.current_menu_title().into());
    bridge.set_menu_breadcrumb(nav.current_menu_breadcrumb().into());
    bridge.set_settings_focused(nav.settings_focused);
    bridge.set_settings_selected(nav.settings_selected as i32);
    bridge.set_about_selected(nav.about_selected as i32);
    let active_label = mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
        .get(nav.display_selected)
        .map_or("Custom/current mode", |mode| mode.label);
    bridge.set_display_active_label(active_label.into());
    bridge.set_display_combo_open(nav.display_combo_open);
    bridge.set_display_selected(nav.display_selected as i32);
    bridge.set_display_highlighted(nav.display_highlighted as i32);
    bridge.set_display_confirm_remaining(nav.display_confirm_remaining as i32);
    bridge.set_simple_joystick_handling(nav.settings.simple_joystick_handling);
    bridge.set_screensaver_settings_selected(nav.screensaver_selected as i32);
    bridge.set_screensaver_enabled(nav.settings.screensaver_enabled);
    bridge.set_screensaver_delay_minutes(nav.settings.screensaver_delay_minutes as i32);
    bridge.set_licenses_selected(nav.licenses_selected as i32);
    bridge.set_licenses_expanded(nav.licenses_expanded);
    bridge.set_licenses_scroll_y(nav.licenses_scroll_y());
    bridge.set_license_lines(models.license_lines(nav.licenses_selected));
    sync_arcade_list_geometry_bridge(&bridge, nav, render_w);
    if !(defer_selected_preview && nav.screen == Screen::Arcade) {
        bridge.set_arcade_selected(nav.arcade.selected as i32);
        bridge.set_arcade_scroll_y(nav.arcade.scroll_y);
    }
    let mut active_games_for_preview: Option<ArcadeGameView<'_>> = None;
    let mut active_games_loading = false;
    if let Some(catalog) = catalog {
        let games = active_system_game_view(catalog, nav);
        let header = active_system_header(catalog, nav, games.len());
        active_games_loading = active_system_games_loading(catalog, nav);
        bridge.set_menu_items(models.menu_items(nav, catalog_version));
        bridge.set_active_system_title(header.title.into());
        bridge.set_active_system_count(header.count as i32);
        active_games_for_preview = Some(games);
    }
    bridge.set_arcade_games_loading(active_games_loading);
    sync_arcade_search_bridge(&bridge, nav);
    sync_launcher_confirm_bridge(&bridge, nav, lifecycle);
    LauncherStatusPresenter::new(&bridge).sync_loading(loading_message, loading_detail);
    if nav.screen == Screen::Arcade
        && (active_games_loading || nav.arcade_search.is_active(&nav.arcade_filter.active))
    {
        preview.clear(&bridge);
    } else if nav.screen == Screen::Arcade {
        let games = active_games_for_preview
            .or_else(|| catalog.map(|catalog| active_system_game_view(catalog, nav)))
            .unwrap_or_else(ArcadeGameView::empty);
        let _ = request_arcade_preview_window(
            &bridge,
            games,
            nav.arcade.selected,
            preview,
            defer_selected_preview,
            nav.arcade.is_scroll_active(),
            nav.arcade.is_turbo_active(),
        );
    } else {
        preview.clear(&bridge);
    }
    sync_setup_bridge(&bridge, pad, setup);
}

pub(super) fn sync_bridge_launcher_light(
    app: &slint_ui::launcher::Launcher,
    nav: &LauncherNav,
    lifecycle: &LauncherLifecycle,
    models: &mut LauncherBridgeModels,
    setup: &SetupNav,
    loading_message: &str,
    loading_detail: &str,
    catalog: &ArcadeCatalog,
    active_arcade_games: Option<ArcadeGameView<'_>>,
    preview: &mut PreviewState,
    defer_arcade_overlay_bridge: bool,
    defer_selected_preview: bool,
    render_w: usize,
) {
    models.sync_menu_item_focus(nav.selected);
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let active_games_loading = active_system_games_loading(catalog, nav);
    let active_games_len = active_arcade_games.as_ref().map_or_else(
        || active_system_game_view(catalog, nav).len(),
        |games| games.len(),
    );
    let header = active_system_header(catalog, nav, active_games_len);
    set_bridge_string_if_changed!(
        bridge,
        get_active_system_title,
        set_active_system_title,
        header.title
    );
    set_bridge_if_changed!(
        bridge,
        get_active_system_count,
        set_active_system_count,
        header.count as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_screen_mode,
        set_screen_mode,
        match nav.screen {
            Screen::Home => 0,
            Screen::Controller => 1,
            Screen::Arcade => 2,
            Screen::Settings => 3,
            Screen::About => 4,
            Screen::Licenses => 5,
            Screen::Info => 6,
            Screen::Screensaver => 7,
        }
    );
    set_bridge_if_changed!(
        bridge,
        get_selected_index,
        set_selected_index,
        nav.selected as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_home_scroll_held,
        set_home_scroll_held,
        nav.home_horizontal_held()
    );
    set_bridge_if_changed!(
        bridge,
        get_home_scroll_repeat_active,
        set_home_scroll_repeat_active,
        nav.home_horizontal_repeat_active()
    );
    set_bridge_if_changed!(bridge, get_home_scroll_x, set_home_scroll_x, nav.scroll_x);
    set_bridge_string_if_changed!(
        bridge,
        get_menu_title,
        set_menu_title,
        nav.current_menu_title()
    );
    set_bridge_string_if_changed!(
        bridge,
        get_menu_breadcrumb,
        set_menu_breadcrumb,
        nav.current_menu_breadcrumb()
    );
    set_bridge_if_changed!(
        bridge,
        get_settings_focused,
        set_settings_focused,
        nav.settings_focused
    );
    set_bridge_if_changed!(
        bridge,
        get_settings_selected,
        set_settings_selected,
        nav.settings_selected as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_about_selected,
        set_about_selected,
        nav.about_selected as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_simple_joystick_handling,
        set_simple_joystick_handling,
        nav.settings.simple_joystick_handling
    );
    set_bridge_if_changed!(
        bridge,
        get_screensaver_settings_selected,
        set_screensaver_settings_selected,
        nav.screensaver_selected as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_screensaver_enabled,
        set_screensaver_enabled,
        nav.settings.screensaver_enabled
    );
    set_bridge_if_changed!(
        bridge,
        get_screensaver_delay_minutes,
        set_screensaver_delay_minutes,
        nav.settings.screensaver_delay_minutes as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_licenses_selected,
        set_licenses_selected,
        nav.licenses_selected as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_licenses_expanded,
        set_licenses_expanded,
        nav.licenses_expanded
    );
    set_bridge_if_changed!(
        bridge,
        get_licenses_scroll_y,
        set_licenses_scroll_y,
        nav.licenses_scroll_y()
    );
    if models.license_lines_index() != Some(nav.licenses_selected) {
        bridge.set_license_lines(models.license_lines(nav.licenses_selected));
    }
    sync_arcade_list_geometry_bridge_if_changed(&bridge, nav, render_w);
    if !(defer_arcade_overlay_bridge && nav.screen == Screen::Arcade) {
        set_bridge_if_changed!(
            bridge,
            get_arcade_selected,
            set_arcade_selected,
            nav.arcade.selected as i32
        );
        set_bridge_if_changed!(
            bridge,
            get_arcade_scroll_y,
            set_arcade_scroll_y,
            nav.arcade.scroll_y
        );
    }
    set_bridge_if_changed!(
        bridge,
        get_arcade_games_loading,
        set_arcade_games_loading,
        active_games_loading
    );
    sync_arcade_search_bridge_if_changed(&bridge, nav);
    sync_launcher_confirm_bridge(&bridge, nav, lifecycle);
    let status_presenter = LauncherStatusPresenter::new(&bridge);
    status_presenter.sync_loading(loading_message, loading_detail);
    if nav.screen == Screen::Arcade
        && (active_games_loading || nav.arcade_search.is_active(&nav.arcade_filter.active))
    {
        preview.clear(&bridge);
    } else if nav.screen == Screen::Arcade {
        let games = active_arcade_games.unwrap_or_else(|| active_system_game_view(catalog, nav));
        schedule_arcade_preview_window(
            &bridge,
            games,
            nav.arcade.selected,
            preview,
            defer_selected_preview,
            nav.arcade.is_scroll_active(),
            nav.arcade.is_turbo_active(),
        );
    } else {
        preview.clear(&bridge);
    }
    status_presenter.sync_setup_visible(setup.is_active());
}

pub(super) fn launcher_clock_text() -> String {
    // SAFETY: time writes to a valid time_t, zeroed tm is valid storage for
    // localtime_r, and both pointers remain live for the duration of the calls.
    unsafe {
        let mut now: libc::time_t = 0;
        if libc::time(&mut now) == -1 {
            return "--:--".to_string();
        }
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "--:--".to_string();
        }
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

pub(super) fn slint_menu_items(nav: &LauncherNav) -> Rc<VecModel<slint_ui::launcher::MenuItem>> {
    let rows: Vec<slint_ui::launcher::MenuItem> = nav
        .current_menu_items()
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let (scanning, failed, available) = nav.menu_item_catalog_presentation(item);
            let partial = !scanning
                && item.kind == crate::launcher_taxonomy::LauncherMenuItemKind::Menu
                && failed;
            slint_ui::launcher::MenuItem {
                id: item.id.clone().into(),
                label: item.title.clone().into(),
                subtitle: if scanning {
                    match item.kind {
                        crate::launcher_taxonomy::LauncherMenuItemKind::Menu => {
                            let systems = nav.menu_discovered_system_count(&item.id);
                            format!(
                                "{systems} system{} found",
                                if systems == 1 { "" } else { "s" }
                            )
                            .into()
                        }
                        crate::launcher_taxonomy::LauncherMenuItemKind::Collection => "".into(),
                    }
                } else if partial {
                    "Some items failed".into()
                } else if failed {
                    "Load failed — A to retry".into()
                } else {
                    format!("{} games", item.count).into()
                },
                focused: index == nav.selected,
                available,
                node_kind: match item.kind {
                    crate::launcher_taxonomy::LauncherMenuItemKind::Menu => {
                        slint_ui::launcher::MenuItemKind::Group
                    }
                    crate::launcher_taxonomy::LauncherMenuItemKind::Collection => {
                        slint_ui::launcher::MenuItemKind::Collection
                    }
                },
                status: if scanning {
                    slint_ui::launcher::MenuItemStatus::Scanning
                } else if partial {
                    slint_ui::launcher::MenuItemStatus::Partial
                } else if failed {
                    slint_ui::launcher::MenuItemStatus::Failed
                } else {
                    slint_ui::launcher::MenuItemStatus::Ready
                },
            }
        })
        .collect();
    Rc::new(VecModel::from(rows))
}

pub(super) fn slint_sharded_menu_items(
    tiles: &[&mister_magik_catalog::launcher_catalog_session::LauncherSystemTile],
    selected: usize,
) -> Rc<VecModel<slint_ui::launcher::MenuItem>> {
    use mister_magik_catalog::launcher_catalog_session::LauncherSystemState;

    let rows = tiles
        .iter()
        .enumerate()
        .map(|(index, tile)| {
            let (subtitle, status) = match &tile.state {
                LauncherSystemState::Queued | LauncherSystemState::Scanning => {
                    (String::new(), slint_ui::launcher::MenuItemStatus::Scanning)
                }
                LauncherSystemState::Ready { games, .. } => (
                    format!("{games} games"),
                    slint_ui::launcher::MenuItemStatus::Ready,
                ),
                LauncherSystemState::Failed { .. } => (
                    "Scan failed".to_string(),
                    slint_ui::launcher::MenuItemStatus::Failed,
                ),
            };
            slint_ui::launcher::MenuItem {
                id: tile.system_id.as_str().into(),
                label: tile.display_title.clone().into(),
                subtitle: subtitle.into(),
                focused: index == selected,
                available: matches!(tile.state, LauncherSystemState::Ready { .. }),
                node_kind: slint_ui::launcher::MenuItemKind::Collection,
                status,
            }
        })
        .collect::<Vec<_>>();
    Rc::new(VecModel::from(rows))
}

pub(super) fn empty_arcade_catalog(root: &str) -> ArcadeCatalog {
    ArcadeCatalog::new(PathBuf::from(root), Vec::new(), Vec::new())
}

pub(super) fn active_system<'a>(
    _catalog: &ArcadeCatalog,
    nav: &'a LauncherNav,
) -> Option<&'a crate::launcher_taxonomy::LauncherCollection> {
    nav.active_collection()
}

pub(super) fn active_system_game_view<'a>(
    catalog: &'a ArcadeCatalog,
    nav: &'a LauncherNav,
) -> ArcadeGameView<'a> {
    active_system(catalog, nav)
        .map(|system| nav.active_arcade_game_view(catalog, &system.id))
        .unwrap_or_else(ArcadeGameView::empty)
}

struct ActiveSystemHeader {
    title: String,
    count: usize,
}

fn active_system_header(
    catalog: &ArcadeCatalog,
    nav: &LauncherNav,
    fallback_count: usize,
) -> ActiveSystemHeader {
    let Some(system) = active_system(catalog, nav) else {
        return ActiveSystemHeader {
            title: "Games".to_string(),
            count: fallback_count,
        };
    };
    let base_title = system.title.clone();
    let filter_label = nav.arcade_filter.active_label();
    let title = if filter_label == "Games A-Z" {
        base_title
    } else {
        format!("{base_title} - {filter_label}")
    };
    let hydrated_count = nav.active_arcade_game_count(catalog, &system.id);
    ActiveSystemHeader {
        title,
        count: if hydrated_count == 0 && system.count > 0 {
            system.count
        } else {
            hydrated_count
        },
    }
}

fn sync_arcade_search_bridge(bridge: &slint_ui::launcher::MisterBridge, nav: &LauncherNav) {
    bridge.set_arcade_search_active(nav.arcade_search.is_active(&nav.arcade_filter.active));
    bridge.set_arcade_search_query(nav.arcade_search.query.clone().into());
    bridge.set_arcade_search_suggestion(nav.arcade_search.suggestion.clone().into());
    bridge.set_arcade_search_preparing(nav.arcade_search.preparing);
    bridge.set_arcade_search_key_selected(nav.arcade_search.selected_key as i32);
    bridge.set_arcade_search_pane(match nav.arcade_search.pane {
        launcher::ArcadeSearchPane::Keyboard => 0,
        launcher::ArcadeSearchPane::Results => 1,
    });
}

fn sync_arcade_list_geometry_bridge_if_changed(
    bridge: &slint_ui::launcher::MisterBridge,
    nav: &LauncherNav,
    render_w: usize,
) {
    let geometry = if nav.arcade_search.is_active(&nav.arcade_filter.active) {
        ArcadeListGeometry::search_for_render_w(render_w)
    } else {
        ArcadeListGeometry::NORMAL
    };
    set_bridge_if_changed!(
        bridge,
        get_arcade_list_x,
        set_arcade_list_x,
        geometry.x as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_arcade_list_y,
        set_arcade_list_y,
        geometry.y as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_arcade_list_width,
        set_arcade_list_width,
        geometry.width as i32
    );
}

fn sync_arcade_search_bridge_if_changed(
    bridge: &slint_ui::launcher::MisterBridge,
    nav: &LauncherNav,
) {
    set_bridge_if_changed!(
        bridge,
        get_arcade_search_active,
        set_arcade_search_active,
        nav.arcade_search.is_active(&nav.arcade_filter.active)
    );
    set_bridge_string_if_changed!(
        bridge,
        get_arcade_search_query,
        set_arcade_search_query,
        nav.arcade_search.query.clone()
    );
    set_bridge_string_if_changed!(
        bridge,
        get_arcade_search_suggestion,
        set_arcade_search_suggestion,
        nav.arcade_search.suggestion.clone()
    );
    set_bridge_if_changed!(
        bridge,
        get_arcade_search_preparing,
        set_arcade_search_preparing,
        nav.arcade_search.preparing
    );
    set_bridge_if_changed!(
        bridge,
        get_arcade_search_key_selected,
        set_arcade_search_key_selected,
        nav.arcade_search.selected_key as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_arcade_search_pane,
        set_arcade_search_pane,
        match nav.arcade_search.pane {
            launcher::ArcadeSearchPane::Keyboard => 0,
            launcher::ArcadeSearchPane::Results => 1,
        }
    );
}

pub(super) fn active_system_games_loading(catalog: &ArcadeCatalog, nav: &LauncherNav) -> bool {
    active_system(catalog, nav).is_some_and(|system| {
        system.count > 0 && catalog.system_game_count(&system.id) < system.count
    })
}

pub(super) fn setup_pad_info<'a>(pad: &'a PadPool, setup: &SetupNav) -> &'a PadInfo {
    if setup.is_active() {
        pad.info_at(setup.target_pad_idx)
    } else {
        pad.info()
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct SetupBridgeKey {
    phase: SetupPhase,
    trigger_status: crate::controller_db::PadRegistryStatus,
    target_pad_idx: usize,
    list_index: usize,
    draft_label: String,
    draft_kind: crate::controller_db::ControllerKind,
}

impl SetupBridgeKey {
    pub(super) fn from_setup(setup: &SetupNav) -> Self {
        Self {
            phase: setup.phase,
            trigger_status: setup.trigger_status,
            target_pad_idx: setup.target_pad_idx,
            list_index: setup.list_index,
            draft_label: setup.draft_label.clone(),
            draft_kind: setup.draft_kind,
        }
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct LauncherBridgeKey {
    pub(super) screen: Screen,
    pub(super) menu_id: String,
    active_collection_id: Option<String>,
    selected: usize,
    scroll_x: i32,
    home_scroll_repeat_active: bool,
    home_scroll_held: bool,
    settings_focused: bool,
    licenses_selected: usize,
    licenses_expanded: bool,
    licenses_scroll_y: i32,
    confirm_action: Option<launcher::ConfirmAction>,
    confirm_selected: usize,
    arcade_selected: usize,
    arcade_filter_open: bool,
    arcade_filter_level: launcher::ArcadeFilterLevel,
    arcade_filter_selected: usize,
    arcade_filter_active: arcade_catalog::ArcadeFilter,
    arcade_search_query: String,
    arcade_search_suggestion: String,
    arcade_search_preparing: bool,
    arcade_search_selected_key: usize,
    arcade_search_pane: launcher::ArcadeSearchPane,
}

impl LauncherBridgeKey {
    pub(super) fn from_nav(nav: &LauncherNav) -> Self {
        Self {
            screen: nav.screen,
            menu_id: nav.current_menu_id().to_string(),
            active_collection_id: nav.active_collection_id().map(str::to_string),
            selected: nav.selected,
            scroll_x: nav.scroll_x,
            home_scroll_repeat_active: nav.home_horizontal_repeat_active(),
            home_scroll_held: nav.home_horizontal_held(),
            settings_focused: nav.settings_focused,
            licenses_selected: nav.licenses_selected,
            licenses_expanded: nav.licenses_expanded,
            licenses_scroll_y: nav.licenses_scroll_y(),
            confirm_action: nav.confirm_action,
            confirm_selected: nav.confirm_selected,
            arcade_selected: nav.arcade.selected,
            arcade_filter_open: nav.arcade_filter.drawer_open,
            arcade_filter_level: nav.arcade_filter.level,
            arcade_filter_selected: nav.arcade_filter.selected,
            arcade_filter_active: nav.arcade_filter.active.clone(),
            arcade_search_query: nav.arcade_search.query.clone(),
            arcade_search_suggestion: nav.arcade_search.suggestion.clone(),
            arcade_search_preparing: nav.arcade_search.preparing,
            arcade_search_selected_key: nav.arcade_search.selected_key,
            arcade_search_pane: nav.arcade_search.pane,
        }
    }
}

#[derive(Default)]
pub(super) struct LauncherBridgeModels {
    menu_items_key: Option<(usize, String)>,
    menu_items: Option<Rc<VecModel<slint_ui::launcher::MenuItem>>>,
    menu_items_selected: Option<usize>,
    license_lines_index: Option<usize>,
    license_lines: Option<Rc<VecModel<SharedString>>>,
}

impl LauncherBridgeModels {
    fn license_lines_index(&self) -> Option<usize> {
        self.license_lines_index
    }

    fn license_lines(&mut self, index: usize) -> ModelRc<SharedString> {
        if self.license_lines_index != Some(index) {
            let lines = crate::licenses::wrapped_lines(index)
                .iter()
                .map(|line| SharedString::from(line.as_str()))
                .collect::<Vec<_>>();
            self.license_lines = Some(Rc::new(VecModel::from(lines)));
            self.license_lines_index = Some(index);
        }
        ModelRc::from(
            self.license_lines
                .as_ref()
                .expect("license line model initialized")
                .clone(),
        )
    }
}

impl LauncherBridgeModels {
    pub(super) fn menu_items(
        &mut self,
        nav: &LauncherNav,
        catalog_version: usize,
    ) -> ModelRc<slint_ui::launcher::MenuItem> {
        let key = (catalog_version, nav.current_menu_id().to_string());
        if self.menu_items_key.as_ref() != Some(&key) {
            self.menu_items = Some(slint_menu_items(nav));
            self.menu_items_key = Some(key);
            self.menu_items_selected =
                (nav.selected < nav.current_menu_count()).then_some(nav.selected);
        } else {
            self.sync_menu_item_focus(nav.selected);
        }
        ModelRc::from(
            self.menu_items
                .as_ref()
                .expect("launcher menu model should be initialized")
                .clone(),
        )
    }

    fn sync_menu_item_focus(&mut self, selected: usize) {
        let model = self
            .menu_items
            .as_ref()
            .expect("launcher menu model should be initialized");
        let selected = (selected < model.row_count()).then_some(selected);
        if self.menu_items_selected == selected {
            return;
        }

        if let Some(previous) = self.menu_items_selected {
            if let Some(mut row) = model.row_data(previous) {
                row.focused = false;
                model.set_row_data(previous, row);
            }
        }
        if let Some(current) = selected {
            if let Some(mut row) = model.row_data(current) {
                row.focused = true;
                model.set_row_data(current, row);
            }
        }
        self.menu_items_selected = selected;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_catalog::{ArcadeCatalog, GameSystemEntry, DEFAULT_ARCADE_ROOT};
    use crate::input_state::PadState;
    use crate::test_support::arcade_game;
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::Once;
    use std::time::{Duration, Instant};

    #[test]
    fn info_build_label_uses_version_without_repeating_build_number() {
        assert_eq!(
            format_build_label("0.2.323", "14.7.2026 18:47"),
            "Version 0.2.323  14.7.2026 18:47"
        );
    }

    #[test]
    fn summary_only_system_header_keeps_known_game_count_while_rows_load() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from(DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![GameSystemEntry {
                id: "pet2001".into(),
                title: "PET 2001".into(),
                count: 15,
            }],
        );
        let mut nav = LauncherNav::new();
        assert!(nav.open_system(&catalog, "pet2001"));

        assert!(active_system_games_loading(&catalog, &nav));
        assert_eq!(active_system_header(&catalog, &nav, 0).count, 15);
    }

    #[test]
    fn arcade_collection_uses_loaded_rows_as_count_authority() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from(DEFAULT_ARCADE_ROOT),
            vec![arcade_game("Arcade One").system_id("arcade").build()],
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "cps1".into(),
                    title: "CPS-1".into(),
                    count: 1,
                },
            ],
        );
        let mut nav = LauncherNav::new();
        assert!(nav.open_default_arcade(&catalog));

        assert_eq!(
            catalog.system_game_count(arcade_catalog::MENU_ARCADE_SYSTEM_ID),
            1
        );
        assert_eq!(active_system_header(&catalog, &nav, 0).count, 1);
        assert!(!active_system_games_loading(&catalog, &nav));
    }

    #[test]
    fn launcher_bridge_key_tracks_arcade_search_suggestion() {
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade_filter.active = arcade_catalog::ArcadeFilter::Search;

        let before = LauncherBridgeKey::from_nav(&nav);
        nav.arcade_search.suggestion = "street".to_string();
        let after = LauncherBridgeKey::from_nav(&nav);

        assert!(before != after);
    }

    #[test]
    fn settings_sync_does_not_depend_on_launcher_bridge_key() {
        init_test_slint_platform();
        let app = slint_ui::launcher::Launcher::new().expect("launcher component");
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Screensaver;
        let before = LauncherBridgeKey::from_nav(&nav);

        nav.settings_selected = 3;
        nav.display_combo_open = true;
        nav.display_selected = 1;
        nav.display_highlighted = 2;
        nav.screensaver_selected = 1;
        nav.settings.screensaver_enabled = !nav.settings.screensaver_enabled;
        nav.settings.screensaver_delay_minutes += 1;
        nav.settings.simple_joystick_handling = true;

        assert!(before == LauncherBridgeKey::from_nav(&nav));

        let lifecycle = LauncherLifecycle::new(
            LauncherLifecycleConfig {
                catalog_worker_enabled: false,
            },
            Instant::now(),
        );
        sync_settings_bridge(&app, &nav, &lifecycle);

        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        assert_eq!(bridge.get_screen_mode(), 7);
        assert_eq!(bridge.get_settings_selected(), 3);
        assert!(bridge.get_display_combo_open());
        assert_eq!(bridge.get_display_selected(), 1);
        assert_eq!(bridge.get_display_highlighted(), 2);
        assert_eq!(bridge.get_screensaver_settings_selected(), 1);
        assert_eq!(
            bridge.get_screensaver_enabled(),
            nav.settings.screensaver_enabled
        );
        assert_eq!(
            bridge.get_screensaver_delay_minutes(),
            nav.settings.screensaver_delay_minutes as i32
        );
        assert!(bridge.get_simple_joystick_handling());
    }

    #[test]
    fn launcher_bridge_key_tracks_home_repeat_release() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from(DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![
                GameSystemEntry {
                    id: "one".into(),
                    title: "One".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "amiga".into(),
                    title: "Amiga".into(),
                    count: 1,
                },
            ],
        );
        let mut nav = LauncherNav::new();
        let mut held = PadState::default();
        held.dpad_right = true;
        let start = Instant::now();
        nav.handle_input(&held, start, &catalog);
        let pressed = LauncherBridgeKey::from_nav(&nav);
        assert!(pressed.home_scroll_held);
        assert!(!pressed.home_scroll_repeat_active);
        nav.handle_input(&held, start + Duration::from_millis(199), &catalog);
        assert!(!LauncherBridgeKey::from_nav(&nav).home_scroll_repeat_active);
        nav.handle_input(&held, start + Duration::from_millis(200), &catalog);

        let repeating = LauncherBridgeKey::from_nav(&nav);
        assert!(repeating.home_scroll_held);
        assert!(repeating.home_scroll_repeat_active);

        nav.handle_input(
            &PadState::default(),
            start + Duration::from_millis(201),
            &catalog,
        );
        let released = LauncherBridgeKey::from_nav(&nav);
        assert!(!released.home_scroll_held);
        assert!(!released.home_scroll_repeat_active);
        assert!(repeating != released);
    }

    #[test]
    fn library_update_failed_uses_single_ok_button() {
        let text = confirm_bridge_text(Some(launcher::ConfirmAction::LibraryUpdateFailed));

        assert_eq!(text.left_label, "OK");
        assert_eq!(text.right_label, "");
    }

    #[test]
    fn exit_to_mister_uses_cancel_first() {
        let text = confirm_bridge_text(Some(launcher::ConfirmAction::ExitToMister));

        assert_eq!(text.left_label, "Cancel");
        assert_eq!(text.right_label, "Exit to MiSTer");
    }

    fn init_test_slint_platform() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
            let fixed_time = Some(Rc::new(Cell::new(Duration::ZERO)));
            let _ = slint::platform::set_platform(Box::new(MisterPlatform {
                window,
                start: Instant::now(),
                fixed_time,
            }));
        });
    }

    #[test]
    fn menu_item_focus_updates_rows_without_replacing_the_model() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from(DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "neogeo".into(),
                    title: "NeoGeo".into(),
                    count: 2,
                },
                GameSystemEntry {
                    id: "amiga".into(),
                    title: "Amiga".into(),
                    count: 3,
                },
            ],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        let mut models = LauncherBridgeModels::default();
        let rows = models.menu_items(&nav, 1);

        assert!(rows.row_data(0).expect("first row").focused);
        assert!(!rows.row_data(1).expect("second row").focused);

        nav.selected = 1;
        let updated_rows = models.menu_items(&nav, 1);

        assert!(!rows.row_data(0).expect("first row").focused);
        assert!(rows.row_data(1).expect("second row").focused);
        assert!(
            updated_rows
                .row_data(1)
                .expect("updated second row")
                .focused
        );
    }

    #[test]
    fn menu_item_model_is_replaced_when_the_menu_level_changes() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from(DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "neogeo".into(),
                    title: "NeoGeo".into(),
                    count: 2,
                },
                GameSystemEntry {
                    id: "amiga".into(),
                    title: "Amiga".into(),
                    count: 3,
                },
            ],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        let mut models = LauncherBridgeModels::default();
        let root_rows = models.menu_items(&nav, 1);

        assert!(nav.open_menu("computers"));
        let computer_rows = models.menu_items(&nav, 1);

        assert_ne!(root_rows.row_count(), computer_rows.row_count());
        assert_eq!(computer_rows.row_count(), 1);
        assert_eq!(
            computer_rows
                .row_data(0)
                .expect("flattened computer collection")
                .label,
            "Commodore Amiga"
        );
    }

    #[test]
    fn light_bridge_sync_refreshes_active_system_header() {
        init_test_slint_platform();
        let app = slint_ui::launcher::Launcher::new().expect("launcher component");
        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        bridge.set_active_system_title("AcornAtom".into());
        bridge.set_active_system_count(0);

        let catalog = ArcadeCatalog::new(
            PathBuf::from(DEFAULT_ARCADE_ROOT),
            vec![
                arcade_game("1941").system_id("arcade").build(),
                arcade_game("1942").system_id("arcade").build(),
            ],
            vec![
                GameSystemEntry {
                    id: "acornatom".into(),
                    title: "AcornAtom".into(),
                    count: 0,
                },
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 2,
                },
            ],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_default_arcade(&catalog));
        let mut preview = PreviewState::new();
        let mut models = LauncherBridgeModels::default();
        let mut lifecycle = LauncherLifecycle::new(
            LauncherLifecycleConfig {
                catalog_worker_enabled: true,
            },
            Instant::now(),
        );
        let _ = models.menu_items(&nav, 1);

        sync_bridge_launcher_light(
            &app,
            &nav,
            &lifecycle,
            &mut models,
            &SetupNav::new(),
            "",
            "",
            &catalog,
            None,
            &mut preview,
            false,
            false,
            960,
        );

        assert_eq!(bridge.get_active_system_title().as_str(), "Arcade");
        assert_eq!(bridge.get_active_system_count(), 2);

        let mut effects = LifecycleEffects::new();
        lifecycle.after_boot_splash_presented(
            StartupCatalogState::LoadFailed {
                error: "database disk image is malformed".to_string(),
                has_stale_catalog: false,
            },
            &mut effects,
        );
        sync_launcher_confirm_bridge(&bridge, &nav, &lifecycle);
        assert!(bridge.get_confirm_visible());
        assert_eq!(
            bridge.get_confirm_title().as_str(),
            "Library failed to load."
        );
        assert_eq!(
            bridge.get_confirm_message().as_str(),
            "database disk image is malformed"
        );
        assert_eq!(bridge.get_confirm_left_label().as_str(), "Retry");
        assert_eq!(bridge.get_confirm_right_label().as_str(), "Rebuild");
        assert_eq!(bridge.get_confirm_selected(), 0);
    }

    #[test]
    fn sharded_tiles_map_progressive_states_without_game_models() {
        use mister_magik_catalog::catalog_classify::SystemId;
        use mister_magik_catalog::launcher_catalog_session::{
            LauncherSystemState, LauncherSystemTile,
        };

        let arcade = LauncherSystemTile {
            system_id: SystemId::parse("arcade").unwrap(),
            display_title: "Arcade".to_string(),
            section: "Arcade".to_string(),
            family: "Arcade".to_string(),
            order: 0,
            state: LauncherSystemState::Ready {
                generation: 1,
                games: 42,
            },
        };
        let snes = LauncherSystemTile {
            system_id: SystemId::parse("snes").unwrap(),
            display_title: "SNES".to_string(),
            section: "Consoles".to_string(),
            family: "Nintendo".to_string(),
            order: 1,
            state: LauncherSystemState::Scanning,
        };
        let rows = slint_sharded_menu_items(&[&arcade, &snes], 0);
        assert_eq!(rows.row_count(), 2);
        let ready = rows.row_data(0).unwrap();
        assert_eq!(ready.subtitle.as_str(), "42 games");
        assert_eq!(ready.status, slint_ui::launcher::MenuItemStatus::Ready);
        assert!(ready.available);
        let scanning = rows.row_data(1).unwrap();
        assert_eq!(scanning.subtitle.as_str(), "");
        assert_eq!(
            scanning.status,
            slint_ui::launcher::MenuItemStatus::Scanning
        );
        assert!(!scanning.available);
    }
}
