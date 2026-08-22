// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_worker_intents::{MediaProgressDisplay, apply_launcher_worker_ui_intent};
use super::*;
use serde_json::json;

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
        let source = $value;
        let source = AsRef::<str>::as_ref(&source);
        if $bridge.$getter().as_str() != source {
            crate::launcher_presentation::bridge_churn_record_shared_strings(1);
            $bridge.$setter(SharedString::from(source));
        }
    }};
}

fn load_snes_artwork_image() -> Option<slint::Image> {
    let active = mister_magik_fb::snes_artwork::active_asset_path();
    let artwork = mister_magik_fb::snes_artwork::SnesArtwork::load(&active).or_else(|active_error| {
        #[cfg(feature = "ui-preview")]
        {
            let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/snes/snes-small-v1.rgb565a");
            return mister_magik_fb::snes_artwork::SnesArtwork::load(&repository).map_err(|repository_error| {
                crate::ui_errln!(
                    "SNES artwork unavailable: active={active_error}; repository={repository_error}"
                );
                repository_error
            });
        }
        #[cfg(not(feature = "ui-preview"))]
        {
            crate::ui_errln!("SNES artwork unavailable: {active_error}");
            Err(active_error)
        }
    }).ok()?;
    let pixels = artwork.rgba8_bytes();
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &pixels,
        artwork.width as u32,
        artwork.height as u32,
    );
    Some(slint::Image::from_rgba8(buffer))
}

pub(super) fn open_pads() -> PadPool {
    PadPool::open_all().unwrap_or_else(|e| {
        crate::ui_errln!("failed to initialize gamepad input: {e}");
        std::process::exit(1);
    })
}

pub(super) fn init_launcher_bridge(app: &slint_ui::launcher::Launcher, pad: &PadPool) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let navigation = app.global::<slint_ui::launcher::NavigationView>();
    let information = app.global::<slint_ui::launcher::InformationView>();
    bridge.set_startup_visible(true);
    navigation.set_screen(slint_ui::launcher::LauncherScreen::Home);
    if let Some(image) = load_snes_artwork_image() {
        navigation.set_system_artwork(image);
        navigation.set_system_artwork_available(true);
    } else {
        navigation.set_system_artwork_available(false);
    }
    let build_label = SharedString::from(build_label());
    navigation.set_build_label(build_label.clone());
    information.set_build_label(build_label);
    navigation.set_present_mode_label("Mode=/dev/fb0".into());
    information.set_present_mode_label("Mode=/dev/fb0".into());
    let kernel_version = SharedString::from(kernel_version());
    bridge.set_info_kernel_version(kernel_version.clone());
    information.set_kernel_version(kernel_version);
    let database_build = SharedString::from(last_database_build());
    bridge.set_info_database_build(database_build.clone());
    information.set_database_build(database_build);
    bridge.set_settings_selected(0);
    bridge.set_about_selected(0);
    bridge.set_display_options(ModelRc::new(VecModel::from(
        launcher::settings_display_resolutions()
            .map(|mode| SharedString::from(mode.label))
            .collect::<Vec<_>>(),
    )));
    bridge.set_orientation_options(ModelRc::new(VecModel::from(
        crate::settings::ScreenOrientation::ALL
            .iter()
            .map(|orientation| SharedString::from(orientation.label()))
            .collect::<Vec<_>>(),
    )));
    bridge.set_simple_joystick_handling(false);
    bridge.set_reduce_motion(false);
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
    let development_build = mister_magik_catalog::device_layout::DeviceLayout::current()
        == mister_magik_catalog::device_layout::DeviceLayout::Dev;
    navigation.set_development_build(development_build);
    let menu_items = ModelRc::new(VecModel::from(Vec::<slint_ui::launcher::MenuItem>::new()));
    let menu_item_presentation = ModelRc::new(VecModel::from(Vec::<
        slint_ui::launcher::MenuItemPresentation,
    >::new()));
    navigation.set_menu_items(menu_items);
    navigation.set_menu_item_presentation(menu_item_presentation);
    bridge.set_active_system_title("".into());
    bridge.set_active_system_count(0);
    bridge.set_arcade_games(ModelRc::new(VecModel::from(Vec::<
        slint_ui::launcher::ArcadeGame,
    >::new())));
    bridge.set_arcade_selected(0);
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
    let navigation = app.global::<slint_ui::launcher::NavigationView>();
    let screen = crate::launcher_view_types::launcher_screen(nav.screen);
    if navigation.get_screen() != screen {
        navigation.set_screen(screen);
    }
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
        launcher::settings_display_selection_index(nav.display_selected)
            .map_or(-1, |index| index as i32)
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
    set_bridge_string_if_changed!(
        bridge,
        get_orientation_active_label,
        set_orientation_active_label,
        nav.settings.screen_orientation.label()
    );
    set_bridge_if_changed!(
        bridge,
        get_orientation_combo_open,
        set_orientation_combo_open,
        nav.orientation_combo_open
    );
    set_bridge_if_changed!(
        bridge,
        get_orientation_selected,
        set_orientation_selected,
        nav.orientation_selected as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_orientation_highlighted,
        set_orientation_highlighted,
        nav.orientation_highlighted as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_orientation_confirm_remaining,
        set_orientation_confirm_remaining,
        nav.orientation_confirm_remaining as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_simple_joystick_handling,
        set_simple_joystick_handling,
        nav.settings.simple_joystick_handling
    );
    set_bridge_if_changed!(
        bridge,
        get_reduce_motion,
        set_reduce_motion,
        nav.settings.reduce_motion
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
                    | launcher::ConfirmAction::ScreenOrientation
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
    ui: &UiDisplay,
) {
    // Rust and Slint consume one route-owned PAL geometry contract.
    let (geometry, visible_height) = arcade_list_layout(nav, ui);
    bridge.set_arcade_list_x(geometry.x as i32);
    bridge.set_arcade_list_y(geometry.y as i32);
    bridge.set_arcade_list_width(geometry.width as i32);
    bridge.set_arcade_list_height(visible_height as i32);
    sync_crt_arcade_layout_bridge(bridge, nav, ui);
    sync_arcade_preview_geometry_bridge(bridge, nav, ui);
}

fn sync_arcade_preview_geometry_bridge(
    bridge: &slint_ui::launcher::MisterBridge,
    nav: &LauncherNav,
    ui: &UiDisplay,
) {
    let (width, height) = if nav.uses_portrait_layout() {
        (ui.render_h(), ui.render_w())
    } else {
        (ui.render_w(), ui.render_h())
    };
    let rect = mister_magik_fb::visual_composition::hdmi_preview_rect(width, height);
    bridge.set_arcade_preview_box_x(rect.x0 as i32);
    bridge.set_arcade_preview_box_y(rect.y0 as i32);
    bridge.set_arcade_preview_box_width(rect.width() as i32);
    bridge.set_arcade_preview_box_height(rect.rows() as i32);
}

pub(super) fn arcade_list_layout(nav: &LauncherNav, ui: &UiDisplay) -> (ArcadeListGeometry, usize) {
    let search = nav.arcade_search.is_active(&nav.arcade_filter.active);
    if nav.uses_crt_layout() {
        let layout =
            crate::ui_display::UiLayoutGeometry::for_display(ui, nav.settings.screen_orientation);
        let arcade = CrtArcadeLayout::for_layout(layout, CrtUiMetrics::for_display(ui), search);
        return (arcade.list_geometry(), arcade.list.height);
    }
    let geometry = if nav.uses_portrait_layout() {
        ArcadeListGeometry::portrait(ui.render_h(), ui.render_w(), search)
    } else if search {
        ArcadeListGeometry::search_for_render_w(ui.render_w())
    } else {
        ArcadeListGeometry::NORMAL
    };
    let render_h = if nav.uses_portrait_layout() && search {
        geometry.y + ui.render_w() * 34 / 100 + 16
    } else if nav.uses_portrait_layout() {
        ui.render_w()
    } else {
        ui.render_h()
    };
    (geometry, geometry.visible_height(render_h))
}

fn sync_crt_arcade_layout_bridge(
    bridge: &slint_ui::launcher::MisterBridge,
    nav: &LauncherNav,
    ui: &UiDisplay,
) {
    if !nav.uses_crt_layout() {
        return;
    }
    let geometry = UiLayoutGeometry::for_display(ui, nav.settings.screen_orientation);
    let content = geometry.content_rect();
    let arcade = CrtArcadeLayout::for_layout(
        geometry,
        CrtUiMetrics::for_display(ui),
        nav.arcade_search.is_active(&nav.arcade_filter.active),
    );
    let relative = |rect: crate::ui_display::CrtContentRect| {
        (
            rect.x.saturating_sub(content.x) as i32,
            rect.y.saturating_sub(content.y) as i32,
            rect.width as i32,
            rect.height as i32,
        )
    };
    let (header_x, header_y, header_width, _) = relative(arcade.header);
    let (footer_x, footer_y, footer_width, _) = relative(arcade.footer);
    let (keyboard_x, keyboard_y, keyboard_width, keyboard_height) =
        arcade.search_keyboard.map(relative).unwrap_or_default();
    bridge.set_arcade_crt_header_x(header_x);
    bridge.set_arcade_crt_header_y(header_y);
    bridge.set_arcade_crt_header_width(header_width);
    bridge.set_arcade_crt_footer_x(footer_x);
    bridge.set_arcade_crt_footer_y(footer_y);
    bridge.set_arcade_crt_footer_width(footer_width);
    bridge.set_arcade_crt_keyboard_x(keyboard_x);
    bridge.set_arcade_crt_keyboard_y(keyboard_y);
    bridge.set_arcade_crt_keyboard_width(keyboard_width);
    bridge.set_arcade_crt_keyboard_height(keyboard_height);
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

    pub(super) fn sync_loading(&self, message: impl AsRef<str>, detail: impl AsRef<str>) {
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
        summary: impl AsRef<str>,
    ) {
        crate::launcher_presentation::bridge_churn_record_model_replacements(1);
        crate::launcher_presentation::bridge_churn_record_shared_strings(1);
        self.bridge.set_media_pack_progresses(progresses);
        self.bridge
            .set_media_pack_summary(SharedString::from(summary.as_ref()));
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
        set_bridge_string_if_changed!(bridge, get_confirm_title, set_confirm_title, dialog.title);
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_message,
            set_confirm_message,
            dialog.message.as_str()
        );
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_left_label,
            set_confirm_left_label,
            dialog.left_label
        );
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_right_label,
            set_confirm_right_label,
            dialog.right_label
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
    } else if nav.confirm_action == Some(launcher::ConfirmAction::ScreenOrientation) {
        let label = if nav.orientation_error.is_some() {
            "Cancel".to_string()
        } else {
            format!("Cancel ({})", nav.orientation_confirm_remaining)
        };
        set_bridge_string_if_changed!(
            bridge,
            get_confirm_left_label,
            set_confirm_left_label,
            &label
        );
        if nav.orientation_confirm_busy {
            set_bridge_string_if_changed!(
                bridge,
                get_confirm_message,
                set_confirm_message,
                "Saving the launcher and MiSTer OSD orientation…"
            );
            set_bridge_string_if_changed!(
                bridge,
                get_confirm_right_label,
                set_confirm_right_label,
                "Saving…"
            );
        } else if let Some(error) = nav.orientation_error.as_deref() {
            let message =
                format!("Could not save the screen orientation: {error}. Retry or cancel.");
            set_bridge_string_if_changed!(
                bridge,
                get_confirm_title,
                set_confirm_title,
                "Orientation change failed"
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
        Some(launcher::ConfirmAction::RebuildDatabase) => ConfirmBridgeText {
            title: "Rebuild Database?",
            message: "Rebuild all library systems in the background? Games and screenshots remain available.",
            left_label: "Cancel",
            right_label: "Rebuild",
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
        Some(launcher::ConfirmAction::ScreenOrientation) => ConfirmBridgeText {
            title: "Confirm screen orientation",
            message: "Is the launcher upright on the rotated monitor?",
            left_label: "Cancel (20)",
            right_label: "Confirm",
        },
        Some(launcher::ConfirmAction::AddFavourite) => ConfirmBridgeText {
            title: "Game Options",
            message: "Add this game to Favorites?",
            left_label: "Cancel",
            right_label: "Add Favorite",
        },
        Some(launcher::ConfirmAction::RemoveFavourite) => ConfirmBridgeText {
            title: "Game Options",
            message: "Remove this game from Favorites?",
            left_label: "Cancel",
            right_label: "Remove Favorite",
        },
        None => ConfirmBridgeText {
            title: "",
            message: "",
            left_label: "",
            right_label: "",
        },
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct LauncherBridgeSyncTiming {
    pub(super) model_projection_us: u128,
}

pub(super) fn sync_bridge_launcher(
    app: &slint_ui::launcher::Launcher,
    pad: &PadPool,
    nav: &LauncherNav,
    lifecycle: &LauncherLifecycle,
    setup: &SetupNav,
    loading_message: &str,
    loading_detail: &str,
    catalog: &ArcadeCatalog,
    preview: &mut PreviewState,
    models: &mut LauncherBridgeModels,
    catalog_version: usize,
    defer_selected_preview: bool,
    measure_model_projection: bool,
    ui: &UiDisplay,
) -> LauncherBridgeSyncTiming {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let model_started = measure_model_projection.then(Instant::now);
    models.sync(
        app,
        nav,
        catalog,
        Some(catalog_version),
        defer_selected_preview,
    );
    let model_projection_us = model_started
        .map(|started| started.elapsed().as_micros())
        .unwrap_or(0);
    bridge.set_startup_visible(false);
    sync_bridge_pad_launcher(&bridge, pad);
    let clock_text = SharedString::from(launcher_clock_text());
    app.global::<slint_ui::launcher::NavigationView>()
        .set_clock_text(clock_text);
    let active_label = mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
        .get(nav.display_selected)
        .map_or("Custom/current mode", |mode| mode.label);
    bridge.set_display_active_label(active_label.into());
    sync_arcade_list_geometry_bridge(&bridge, nav, ui);
    let active_games_loading = active_system_games_loading(catalog, nav);
    sync_launcher_confirm_bridge(&bridge, nav, lifecycle);
    LauncherStatusPresenter::new(&bridge).sync_loading(loading_message, loading_detail);
    if nav.screen == Screen::Arcade
        && !nav.uses_crt_layout()
        && !active_games_loading
        && !nav.arcade_search.is_active(&nav.arcade_filter.active)
    {
        let games = active_system_game_view(catalog, nav);
        let _ = request_arcade_preview_window(
            &bridge,
            games,
            nav.arcade.selected,
            preview,
            defer_selected_preview,
            nav.arcade.is_scroll_active(),
            nav.arcade.is_turbo_active(),
        );
    }
    sync_setup_bridge(&bridge, pad, setup);
    LauncherBridgeSyncTiming {
        model_projection_us,
    }
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
    measure_model_projection: bool,
    ui: &UiDisplay,
) -> LauncherBridgeSyncTiming {
    let model_started = measure_model_projection.then(Instant::now);
    models.sync(app, nav, catalog, None, defer_arcade_overlay_bridge);
    let model_projection_us = model_started
        .map(|started| started.elapsed().as_micros())
        .unwrap_or(0);
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let active_games_loading = active_system_games_loading(catalog, nav);
    sync_arcade_list_geometry_bridge_if_changed(&bridge, nav, ui);
    sync_launcher_confirm_bridge(&bridge, nav, lifecycle);
    let status_presenter = LauncherStatusPresenter::new(&bridge);
    status_presenter.sync_loading(loading_message, loading_detail);
    if nav.screen == Screen::Arcade
        && !nav.uses_crt_layout()
        && !active_games_loading
        && !nav.arcade_search.is_active(&nav.arcade_filter.active)
    {
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
    }
    status_presenter.sync_setup_visible(setup.is_active());
    LauncherBridgeSyncTiming {
        model_projection_us,
    }
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

fn sync_arcade_list_geometry_bridge_if_changed(
    bridge: &slint_ui::launcher::MisterBridge,
    nav: &LauncherNav,
    ui: &UiDisplay,
) {
    let (geometry, visible_height) = arcade_list_layout(nav, ui);
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
    set_bridge_if_changed!(
        bridge,
        get_arcade_list_height,
        set_arcade_list_height,
        visible_height as i32
    );
    sync_crt_arcade_layout_bridge(bridge, nav, ui);
    let (width, height) = if nav.uses_portrait_layout() {
        (ui.render_h(), ui.render_w())
    } else {
        (ui.render_w(), ui.render_h())
    };
    let preview = mister_magik_fb::visual_composition::hdmi_preview_rect(width, height);
    set_bridge_if_changed!(
        bridge,
        get_arcade_preview_box_x,
        set_arcade_preview_box_x,
        preview.x0 as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_arcade_preview_box_y,
        set_arcade_preview_box_y,
        preview.y0 as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_arcade_preview_box_width,
        set_arcade_preview_box_width,
        preview.width() as i32
    );
    set_bridge_if_changed!(
        bridge,
        get_arcade_preview_box_height,
        set_arcade_preview_box_height,
        preview.rows() as i32
    );
}

pub(super) fn active_system_games_loading(catalog: &ArcadeCatalog, nav: &LauncherNav) -> bool {
    active_system(catalog, nav).is_some_and(|system| {
        system.count > 0 && catalog.system_game_count(&system.id) < system.count
    })
}

pub(super) fn setup_pad_info<'a>(pad: &'a PadPool, setup: &SetupNav) -> &'a PadInfo {
    setup
        .target_device
        .as_ref()
        .and_then(|device| pad.info_for_device(device))
        .unwrap_or_else(|| pad.info())
}

#[derive(PartialEq, Eq)]
pub(super) struct SetupBridgeKey {
    phase: SetupPhase,
    trigger_status: crate::controller_db::PadRegistryStatus,
    target_device: Option<crate::input_event::DeviceInstanceId>,
    list_index: usize,
    draft_label: String,
    draft_kind: crate::controller_db::ControllerKind,
}

impl SetupBridgeKey {
    pub(super) fn from_setup(setup: &SetupNav) -> Self {
        Self {
            phase: setup.phase,
            trigger_status: setup.trigger_status,
            target_device: setup.target_device.clone(),
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
    system_hub_selected: usize,
    arcade_user_list_mode: crate::launcher::ArcadeUserListMode,
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
    arcade_search_status: launcher::ArcadeSearchStatus,
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
            system_hub_selected: nav.system_hub_selected,
            arcade_user_list_mode: nav.arcade_user_list_mode(),
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
            arcade_search_status: nav.arcade_search.status,
            arcade_search_selected_key: nav.arcade_search.selected_key,
            arcade_search_pane: nav.arcade_search.pane,
        }
    }
}

pub(super) type LauncherBridgeModels = LauncherBridgePresenter;

const BRIDGE_CHURN_MEDIA_UPDATES: usize = 60;
const BRIDGE_CHURN_MENU_ROWS: usize = 128;
const BRIDGE_CHURN_MENU_UPDATES: usize = 64;
const BRIDGE_CHURN_LIGHT_UPDATES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BridgeChurnPlaybackStage {
    Idle,
    MediaProgress,
    MenuSelection,
    LightBridge,
    Restore,
    Complete,
}

pub(super) enum BridgeChurnPlaybackTransition {
    Advance {
        completed: GuiProfilePhase,
        next: GuiProfilePhase,
    },
    Finish {
        completed: GuiProfilePhase,
        summary: serde_json::Value,
    },
}

pub(super) struct BridgeChurnPlayback {
    enabled: bool,
    stage: BridgeChurnPlaybackStage,
    step: usize,
    pending_presentation: bool,
    phase_started: crate::launcher_presentation::BridgeChurnCounters,
    restore_started: crate::launcher_presentation::BridgeChurnCounters,
    phase_results: Vec<serde_json::Value>,
    media: MediaProgressDisplay,
    media_terminal: Option<serde_json::Value>,
    media_bridge_terminal: Option<serde_json::Value>,
    menu_items: Option<Rc<VecModel<slint_ui::launcher::MenuItem>>>,
    menu_presentation: Option<Rc<VecModel<slint_ui::launcher::MenuItemPresentation>>>,
    menu_selected: usize,
    menu_terminal: Option<serde_json::Value>,
}

impl BridgeChurnPlayback {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            stage: BridgeChurnPlaybackStage::Idle,
            step: 0,
            pending_presentation: false,
            phase_started: Default::default(),
            restore_started: Default::default(),
            phase_results: Vec::with_capacity(3),
            media: MediaProgressDisplay::default(),
            media_terminal: None,
            media_bridge_terminal: None,
            menu_items: None,
            menu_presentation: None,
            menu_selected: 0,
            menu_terminal: None,
        }
    }

    pub(super) fn apply(
        &mut self,
        phase: Option<GuiProfilePhase>,
        app: &slint_ui::launcher::Launcher,
        nav: &LauncherNav,
        models: &LauncherBridgeModels,
        full_bridge_dirty: &mut bool,
        light_bridge_dirty: &mut bool,
    ) {
        if !self.enabled || self.pending_presentation {
            return;
        }
        if self.stage == BridgeChurnPlaybackStage::Idle {
            if phase != Some(GuiProfilePhase::MediaProgress) {
                return;
            }
            crate::launcher_presentation::bridge_churn_begin();
            self.phase_started = crate::launcher_presentation::bridge_churn_snapshot();
            self.stage = BridgeChurnPlaybackStage::MediaProgress;
        }
        match self.stage {
            BridgeChurnPlaybackStage::Idle | BridgeChurnPlaybackStage::Complete => {}
            BridgeChurnPlaybackStage::MediaProgress => {
                let event = bridge_churn_media_event(self.step);
                apply_launcher_worker_ui_intent(
                    app,
                    self.media.progress_intent(&event),
                    full_bridge_dirty,
                );
                if self.step + 1 == BRIDGE_CHURN_MEDIA_UPDATES {
                    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                    let model = bridge.get_media_pack_progresses();
                    self.media_bridge_terminal = Some(json!({
                        "rows": (0..model.row_count()).filter_map(|index| model.row_data(index)).map(|row| json!({
                            "system": row.system.as_str(),
                            "phase": row.phase.as_str(),
                            "percent": row.percent,
                            "pack_position": row.pack_position.as_str(),
                        })).collect::<Vec<_>>(),
                        "summary": bridge.get_media_pack_summary().as_str(),
                    }));
                }
                self.pending_presentation = true;
            }
            BridgeChurnPlaybackStage::MenuSelection => {
                self.apply_menu_selection(app);
                *full_bridge_dirty = true;
                self.pending_presentation = true;
            }
            BridgeChurnPlaybackStage::LightBridge => {
                *light_bridge_dirty = true;
                self.pending_presentation = true;
            }
            BridgeChurnPlaybackStage::Restore => {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                LauncherStatusPresenter::new(&bridge)
                    .sync_media_progresses(empty_media_pack_progress_model(), "");
                models.republish_cached_menu_models(app);
                app.global::<slint_ui::launcher::NavigationView>()
                    .set_home_selected_index(nav.selected as i32);
                *full_bridge_dirty = true;
                self.pending_presentation = true;
            }
        }
    }

    pub(super) fn note_presented(&mut self) -> Option<BridgeChurnPlaybackTransition> {
        if !self.enabled || !std::mem::take(&mut self.pending_presentation) {
            return None;
        }
        match self.stage {
            BridgeChurnPlaybackStage::Idle | BridgeChurnPlaybackStage::Complete => None,
            BridgeChurnPlaybackStage::MediaProgress => {
                self.step = self.step.saturating_add(1);
                if self.step < BRIDGE_CHURN_MEDIA_UPDATES {
                    return None;
                }
                self.media_terminal = Some(json!({
                    "rows": self.media.active.values().map(|row| json!({
                        "system": row.system,
                        "phase": row.phase,
                        "percent": row.percent,
                        "pack_position": row.pack_position,
                    })).collect::<Vec<_>>(),
                    "summary": self.media.summary(),
                }));
                self.finish_phase("media-progress", BRIDGE_CHURN_MEDIA_UPDATES);
                self.stage = BridgeChurnPlaybackStage::MenuSelection;
                self.step = 0;
                self.phase_started = crate::launcher_presentation::bridge_churn_snapshot();
                Some(BridgeChurnPlaybackTransition::Advance {
                    completed: GuiProfilePhase::MediaProgress,
                    next: GuiProfilePhase::MenuSelection,
                })
            }
            BridgeChurnPlaybackStage::MenuSelection => {
                self.step = self.step.saturating_add(1);
                if self.step < BRIDGE_CHURN_MENU_UPDATES {
                    return None;
                }
                self.menu_terminal = self.menu_terminal_snapshot();
                self.finish_phase("menu-selection", BRIDGE_CHURN_MENU_UPDATES);
                self.stage = BridgeChurnPlaybackStage::LightBridge;
                self.step = 0;
                self.phase_started = crate::launcher_presentation::bridge_churn_snapshot();
                Some(BridgeChurnPlaybackTransition::Advance {
                    completed: GuiProfilePhase::MenuSelection,
                    next: GuiProfilePhase::LightBridge,
                })
            }
            BridgeChurnPlaybackStage::LightBridge => {
                self.step = self.step.saturating_add(1);
                if self.step < BRIDGE_CHURN_LIGHT_UPDATES {
                    return None;
                }
                self.finish_phase("light-bridge", BRIDGE_CHURN_LIGHT_UPDATES);
                self.stage = BridgeChurnPlaybackStage::Restore;
                self.restore_started = crate::launcher_presentation::bridge_churn_snapshot();
                None
            }
            BridgeChurnPlaybackStage::Restore => {
                let total = crate::launcher_presentation::bridge_churn_end();
                let restoration = total.saturating_sub(self.restore_started);
                self.stage = BridgeChurnPlaybackStage::Complete;
                Some(BridgeChurnPlaybackTransition::Finish {
                    completed: GuiProfilePhase::LightBridge,
                    summary: json!({
                        "schema": "mister-magik-bridge-churn-playback-v1",
                        "media_terminal": self.media_terminal,
                        "media_bridge_terminal": self.media_bridge_terminal,
                        "menu_terminal": self.menu_terminal,
                        "phase_results": self.phase_results,
                        "restoration": bridge_churn_counter_json(restoration),
                        "total": bridge_churn_counter_json(total),
                        "terminal": {
                            "media_rows": 0,
                            "media_summary": "",
                            "menu_restored": true,
                        },
                    }),
                })
            }
        }
    }

    fn apply_menu_selection(&mut self, app: &slint_ui::launcher::Launcher) {
        if self.menu_items.is_none() {
            let allocation_started = Instant::now();
            let items = (0..BRIDGE_CHURN_MENU_ROWS)
                .map(|index| slint_ui::launcher::MenuItem {
                    id: format!("bridge-bench-{index:03}").into(),
                    label: format!("Bridge benchmark row {index:03}").into(),
                    subtitle: format!("Deterministic production row {index:03}").into(),
                    available: true,
                    node_kind: slint_ui::launcher::MenuItemKind::Collection,
                    status: slint_ui::launcher::MenuItemStatus::Ready,
                })
                .collect::<Vec<_>>();
            let presentation = (0..BRIDGE_CHURN_MENU_ROWS)
                .map(|index| slint_ui::launcher::MenuItemPresentation {
                    selected: index == 0,
                    acknowledged: false,
                })
                .collect::<Vec<_>>();
            crate::launcher_presentation::bridge_churn_record_row_allocations(
                items.len().saturating_add(presentation.len()) as u64,
            );
            crate::launcher_presentation::bridge_churn_record_shared_strings(
                items.len().saturating_mul(3) as u64,
            );
            crate::launcher_presentation::bridge_churn_record_model_allocation_us(
                allocation_started.elapsed().as_micros(),
            );
            self.menu_items = Some(Rc::new(VecModel::from(items)));
            self.menu_presentation = Some(Rc::new(VecModel::from(presentation)));
            let navigation = app.global::<slint_ui::launcher::NavigationView>();
            crate::launcher_presentation::bridge_churn_record_model_replacements(2);
            navigation.set_menu_items(ModelRc::from(
                self.menu_items.as_ref().expect("benchmark items").clone(),
            ));
            navigation.set_menu_item_presentation(ModelRc::from(
                self.menu_presentation
                    .as_ref()
                    .expect("benchmark presentation")
                    .clone(),
            ));
        }
        let selected = self.step % BRIDGE_CHURN_MENU_ROWS;
        if let Some(model) = self.menu_presentation.as_ref() {
            bridge_churn_sync_benchmark_menu_row(model, self.menu_selected, selected);
            if self.menu_selected != selected {
                bridge_churn_sync_benchmark_menu_row(model, selected, selected);
            }
        }
        self.menu_selected = selected;
        app.global::<slint_ui::launcher::NavigationView>()
            .set_home_selected_index(selected as i32);
    }

    fn menu_terminal_snapshot(&self) -> Option<serde_json::Value> {
        let model = self.menu_presentation.as_ref()?;
        let selected = (0..model.row_count())
            .filter(|index| model.row_data(*index).is_some_and(|row| row.selected))
            .collect::<Vec<_>>();
        let acknowledged = (0..model.row_count())
            .filter(|index| model.row_data(*index).is_some_and(|row| row.acknowledged))
            .collect::<Vec<_>>();
        Some(json!({
            "rows": model.row_count(),
            "selected_rows": selected,
            "acknowledged_rows": acknowledged,
        }))
    }

    fn finish_phase(&mut self, phase: &'static str, updates: usize) {
        let counters = crate::launcher_presentation::bridge_churn_snapshot()
            .saturating_sub(self.phase_started);
        self.phase_results.push(json!({
            "phase": phase,
            "updates": updates,
            "counters": bridge_churn_counter_json(counters),
        }));
    }
}

fn bridge_churn_sync_benchmark_menu_row(
    model: &VecModel<slint_ui::launcher::MenuItemPresentation>,
    index: usize,
    selected: usize,
) {
    let Some(mut row) = model.row_data(index) else {
        return;
    };
    let expected = index == selected;
    if row.selected != expected {
        row.selected = expected;
        crate::launcher_presentation::bridge_churn_record_row_mutations(1);
        model.set_row_data(index, row);
    }
}

fn bridge_churn_counter_json(
    counters: crate::launcher_presentation::BridgeChurnCounters,
) -> serde_json::Value {
    json!({
        "model_replacements": counters.model_replacements,
        "row_mutations": counters.row_mutations,
        "row_allocations": counters.row_allocations,
        "shared_string_constructions": counters.shared_string_constructions,
        "model_allocation_us": counters.model_allocation_us,
    })
}

fn bridge_churn_media_event(step: usize) -> MediaProgressEvent {
    const SYSTEMS: [&str; 3] = ["snes", "megadrive", "neogeo"];
    let system_index = if step + SYSTEMS.len() >= BRIDGE_CHURN_MEDIA_UPDATES {
        step + SYSTEMS.len() - BRIDGE_CHURN_MEDIA_UPDATES
    } else {
        step % SYSTEMS.len()
    };
    let terminal = step + SYSTEMS.len() >= BRIDGE_CHURN_MEDIA_UPDATES;
    let failed = terminal && system_index == SYSTEMS.len() - 1;
    let bytes_total = 100_000_000u64;
    let bytes_done = if terminal {
        if failed { 75_000_000 } else { bytes_total }
    } else {
        ((step / SYSTEMS.len()) as u64 + 1)
            .saturating_mul(5_000_000)
            .min(95_000_000)
    };
    MediaProgressEvent {
        system: SYSTEMS[system_index].to_string(),
        image_size: "320x240".to_string(),
        variant: "identity".to_string(),
        phase: if failed {
            "failed".to_string()
        } else if terminal {
            "download_done".to_string()
        } else if step < SYSTEMS.len() {
            "download_start".to_string()
        } else {
            "download".to_string()
        },
        bytes_done,
        bytes_total,
        pack_index: system_index + 1,
        pack_count: SYSTEMS.len(),
        download_mbps: Some(8.0),
        detail: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_catalog::{ArcadeCatalog, DEFAULT_ARCADE_ROOT, GameSystemEntry};
    use crate::input_state::PadState;
    use crate::launcher_presentation::SelectionFeedbackTarget;
    use crate::test_support::arcade_game;
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    #[test]
    fn info_build_label_uses_version_without_repeating_build_number() {
        assert_eq!(
            format_build_label("0.2.323", "14.7.2026 18:47"),
            "Version 0.2.323  14.7.2026 18:47"
        );
    }

    #[test]
    fn summary_only_system_reports_loading_while_rows_hydrate() {
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
    }

    #[test]
    fn arcade_collection_reports_missing_registry_rows_as_loading() {
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
        assert_eq!(active_system_game_view(&catalog, &nav).len(), 1);
        assert!(active_system_games_loading(&catalog, &nav));
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
        nav.settings.reduce_motion = true;

        assert!(before == LauncherBridgeKey::from_nav(&nav));

        let lifecycle = LauncherLifecycle::new(
            LauncherLifecycleConfig {
                catalog_worker_enabled: false,
            },
            Instant::now(),
        );
        sync_settings_bridge(&app, &nav, &lifecycle);

        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        assert_eq!(
            app.global::<slint_ui::launcher::NavigationView>()
                .get_screen(),
            slint_ui::launcher::LauncherScreen::ScreensaverSettings
        );
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
        assert!(bridge.get_reduce_motion());
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
        nav.handle_held_tick_with_navigation_intents(&held, start, &catalog);
        let pressed = LauncherBridgeKey::from_nav(&nav);
        assert!(pressed.home_scroll_held);
        assert!(!pressed.home_scroll_repeat_active);
        nav.handle_held_tick_with_navigation_intents(
            &held,
            start + Duration::from_millis(199),
            &catalog,
        );
        assert!(!LauncherBridgeKey::from_nav(&nav).home_scroll_repeat_active);
        nav.handle_held_tick_with_navigation_intents(
            &held,
            start + Duration::from_millis(200),
            &catalog,
        );

        let repeating = LauncherBridgeKey::from_nav(&nav);
        assert!(repeating.home_scroll_held);
        assert!(repeating.home_scroll_repeat_active);

        nav.handle_held_tick_with_navigation_intents(
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
        let window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let fixed_time = Some(Rc::new(Cell::new(Duration::ZERO)));
        let _ = slint::platform::set_platform(Box::new(MisterPlatform::new(window, fixed_time)));
    }

    #[test]
    fn menu_item_state_mutates_only_the_previous_and_next_rows() {
        init_test_slint_platform();
        let app = slint_ui::launcher::Launcher::new().expect("launcher component");
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
                GameSystemEntry {
                    id: "gb".into(),
                    title: "Game Boy".into(),
                    count: 4,
                },
            ],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        let mut models = LauncherBridgeModels::default();
        let rows = models.menu_items(&nav, 1);
        let presentation = models.menu_item_presentation();

        assert_eq!(rows.row_count(), 4);
        let before = (0..rows.row_count())
            .map(|index| rows.row_data(index).expect("initial launcher row"))
            .collect::<Vec<_>>();
        let presentation_before = (0..presentation.row_count())
            .map(|index| {
                presentation
                    .row_data(index)
                    .expect("initial launcher presentation row")
            })
            .collect::<Vec<_>>();

        let feedback_before = SelectionFeedbackTarget::home(&nav);
        nav.selected = 1;
        let feedback_after = SelectionFeedbackTarget::home(&nav);
        assert!(
            models
                .note_selection_feedback_change(feedback_before.as_ref(), feedback_after.as_ref(),)
        );
        models.sync(&app, &nav, &catalog, Some(1), false);
        let updated_rows = models.menu_items(&nav, 1);
        let updated_presentation = models.menu_item_presentation();

        let after = (0..updated_rows.row_count())
            .map(|index| updated_rows.row_data(index).expect("retained launcher row"))
            .collect::<Vec<_>>();
        let presentation_after = (0..updated_presentation.row_count())
            .map(|index| {
                updated_presentation
                    .row_data(index)
                    .expect("retained launcher presentation row")
            })
            .collect::<Vec<_>>();
        assert_eq!(before, after);
        assert!(presentation_before[0].selected);
        assert!(!presentation_after[0].selected);
        assert!(!presentation_after[0].acknowledged);
        assert!(presentation_after[1].selected);
        assert!(presentation_after[1].acknowledged);
        assert_eq!(presentation_before[2], presentation_after[2]);
        assert_eq!(presentation_before[3], presentation_after[3]);
        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        assert!(bridge.invoke_selection_feedback_query(
            "".into(),
            after[1].id.clone(),
            bridge.get_selection_feedback_revision(),
        ));
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
    fn nintendo_menu_keeps_lazy_hydration_tiles_ready() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from(DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![
                GameSystemEntry {
                    id: "nes".into(),
                    title: "Nintendo Entertainment System".into(),
                    count: 2,
                },
                GameSystemEntry {
                    id: "snes".into(),
                    title: "Super Nintendo".into(),
                    count: 3,
                },
                GameSystemEntry {
                    id: "n64".into(),
                    title: "Nintendo 64".into(),
                    count: 4,
                },
            ],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("consoles"));
        assert!(nav.open_menu("menu:consoles:nintendo"));
        nav.catalog_system_hydration_started("snes");
        nav.catalog_system_hydration_started("n64");

        let mut models = LauncherBridgeModels::default();
        let rows = models.menu_items(&nav, 1);

        assert!(rows.row_count() >= 3);
        for index in 0..rows.row_count() {
            let row = rows.row_data(index).expect("Nintendo launcher row");
            assert_eq!(row.status, slint_ui::launcher::MenuItemStatus::Ready);
        }

        nav.catalog_system_hydration_failed("snes");
        let failed_rows = models.menu_items(&nav, 2);
        let failed_snes = (0..failed_rows.row_count())
            .filter_map(|index| failed_rows.row_data(index))
            .find(|row| row.id.as_str() == "snes")
            .expect("failed SNES launcher row");
        assert_eq!(
            failed_snes.status,
            slint_ui::launcher::MenuItemStatus::Failed
        );
        assert_eq!(failed_snes.subtitle.as_str(), "Load failed — A to retry");
        assert!(!failed_snes.available);

        nav.catalog_system_hydration_started("snes");
        let retry_rows = models.menu_items(&nav, 3);
        let retry_snes = (0..retry_rows.row_count())
            .filter_map(|index| retry_rows.row_data(index))
            .find(|row| row.id.as_str() == "snes")
            .expect("retrying SNES launcher row");
        assert_eq!(retry_snes.status, slint_ui::launcher::MenuItemStatus::Ready);
        assert_eq!(retry_snes.subtitle.as_str(), "3 games");
        assert!(retry_snes.available);
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
        let ui = UiDisplay::for_framebuffer(960, 540);

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
            false,
            &ui,
        );

        assert_eq!(bridge.get_active_system_title().as_str(), "Arcade");
        assert_eq!(bridge.get_active_system_count(), 2);

        let mut effects = LifecycleEffects::new();
        lifecycle.after_boot_splash_presented(
            StartupCatalogState::LoadFailed {
                error: "database disk image is malformed".to_string(),
                has_stale_catalog: false,
                transient: false,
            },
            &mut effects,
        );
        sync_launcher_confirm_bridge(&bridge, &nav, &lifecycle);
        assert!(bridge.get_confirm_visible());
        assert_eq!(bridge.get_confirm_title().as_str(), "Catalog unavailable");
        assert!(
            bridge
                .get_confirm_message()
                .as_str()
                .contains("database disk image is malformed")
        );
        assert_eq!(bridge.get_confirm_left_label().as_str(), "Exit to MiSTer");
        assert_eq!(bridge.get_confirm_right_label().as_str(), "Full rebuild");
        assert_eq!(bridge.get_confirm_selected(), 0);
    }

    #[test]
    fn full_and_light_bridge_sync_leave_every_static_view_detached() {
        init_test_slint_platform();
        let app = slint_ui::launcher::Launcher::new().expect("launcher component");
        let catalog =
            ArcadeCatalog::new(PathBuf::from(DEFAULT_ARCADE_ROOT), Vec::new(), Vec::new());
        let pad = PadPool::from_test_states(Vec::new());
        let lifecycle = LauncherLifecycle::new(
            LauncherLifecycleConfig {
                catalog_worker_enabled: false,
            },
            Instant::now(),
        );
        let setup = SetupNav::new();
        let ui = UiDisplay::for_framebuffer(960, 540);

        for (index, screen) in [
            Screen::Home,
            Screen::Controller,
            Screen::Settings,
            Screen::Screensaver,
            Screen::About,
            Screen::Licenses,
            Screen::Info,
        ]
        .into_iter()
        .enumerate()
        {
            let mut nav = LauncherNav::new();
            nav.screen = screen;
            nav.sync_launcher_taxonomy(&catalog);
            let mut preview = PreviewState::new();
            let mut models = LauncherBridgeModels::default();

            for catalog_version in [index, index + 1] {
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
                    &mut models,
                    catalog_version,
                    false,
                    false,
                    &ui,
                );
                sync_bridge_launcher_light(
                    &app,
                    &nav,
                    &lifecycle,
                    &mut models,
                    &setup,
                    "",
                    "",
                    &catalog,
                    None,
                    &mut preview,
                    false,
                    false,
                    false,
                    &ui,
                );
            }

            assert_eq!(
                preview.presentation_state(),
                PreviewPresentationState::Detached,
                "static screen {screen:?}"
            );
            assert_eq!(preview.frame_intent(), PreviewFrameIntent::None);
            assert!(!preview.raw_dirty());
        }
    }
}
