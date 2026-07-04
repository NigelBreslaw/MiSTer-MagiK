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
    load_cabinet_image(&bridge);
    bridge.set_startup_visible(true);
    bridge.set_screen_mode(0);
    bridge.set_build_label(build_label().into());
    bridge.set_selected_index(0);
    bridge.set_settings_focused(false);
    bridge.set_settings_selected(0);
    bridge.set_simple_joystick_handling(false);
    bridge.set_confirm_visible(false);
    bridge.set_confirm_title("".into());
    bridge.set_confirm_message("".into());
    bridge.set_confirm_left_label("".into());
    bridge.set_confirm_right_label("".into());
    bridge.set_confirm_selected(0);
    bridge.set_game_systems(ModelRc::new(VecModel::from(Vec::<
        slint_ui::launcher::GameSystem,
    >::new())));
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

fn build_label() -> String {
    let build_number = env!("MISTER_MAGIK_BUILD_NUMBER");
    let build_time = env!("MISTER_MAGIK_BUILD_TIME");
    format!("Build {build_number}  {build_time}")
}

fn load_cabinet_image(bridge: &slint_ui::launcher::MisterBridge) {
    const DEFAULT_PATH: &str = "/media/fat/mister-magik/art/arcade-cabinet-preview.rgba";
    let path = std::env::var("MISTER_CABINET_IMAGE_PATH").unwrap_or_else(|_| DEFAULT_PATH.into());
    match load_raw_rgba_image(std::path::Path::new(&path)) {
        Ok(image) => bridge.set_arcade_cabinet_image(image),
        Err(error) => crate::ui_errln!("launcher: failed to load cabinet image {path}: {error}"),
    }
}

fn load_raw_rgba_image(path: &std::path::Path) -> Result<slint::Image, String> {
    const MAGIC: &[u8] = b"MISTER_MAGIK_RGBA_RLE\n";
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if !bytes.starts_with(MAGIC) {
        return Err("bad raw image header".into());
    }
    let size_start = MAGIC.len();
    let size_end = bytes[size_start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map(|offset| size_start + offset)
        .ok_or_else(|| "missing raw image size".to_string())?;
    let size = std::str::from_utf8(&bytes[size_start..size_end])
        .map_err(|e| format!("invalid raw image size: {e}"))?;
    let mut parts = size.split_whitespace();
    let width = parts
        .next()
        .ok_or_else(|| "missing raw image width".to_string())?
        .parse::<u32>()
        .map_err(|e| format!("invalid raw image width: {e}"))?;
    let height = parts
        .next()
        .ok_or_else(|| "missing raw image height".to_string())?
        .parse::<u32>()
        .map_err(|e| format!("invalid raw image height: {e}"))?;
    if parts.next().is_some() || width == 0 || height == 0 {
        return Err("invalid raw image dimensions".into());
    }
    let chunks = &bytes[size_end + 1..];
    let expected_len = width as usize * height as usize * 4;
    let mut pixels = Vec::with_capacity(expected_len);
    for chunk in chunks.chunks_exact(6) {
        let count = u16::from_le_bytes([chunk[0], chunk[1]]) as usize;
        let pixel = &chunk[2..6];
        for _ in 0..count {
            pixels.extend_from_slice(pixel);
        }
        if pixels.len() > expected_len {
            return Err("raw image RLE expands past expected dimensions".into());
        }
    }
    if chunks.len() % 6 != 0 {
        return Err("raw image RLE has a partial chunk".into());
    }
    if pixels.len() != expected_len {
        return Err(format!(
            "raw image expands to {} RGBA bytes, expected {expected_len}",
            pixels.len()
        ));
    }

    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    buffer.make_mut_bytes().copy_from_slice(&pixels);
    Ok(slint::Image::from_rgba8(buffer))
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

fn sync_arcade_list_geometry_bridge(bridge: &slint_ui::launcher::MisterBridge, nav: &LauncherNav) {
    let geometry = if nav.arcade_search.is_active(&nav.arcade_filter.active) {
        ArcadeListGeometry::SEARCH
    } else {
        ArcadeListGeometry::NORMAL
    };
    bridge.set_arcade_list_x(geometry.x as i32);
    bridge.set_arcade_list_y(geometry.y as i32);
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
            left_label: "Exit to MiSTer",
            right_label: "Return to MiSTer MagiK",
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
    setup: &SetupNav,
    loading_message: &str,
    loading_detail: &str,
    catalog: Option<&ArcadeCatalog>,
    preview: &mut PreviewState,
    models: &mut LauncherBridgeModels,
    catalog_version: usize,
    defer_selected_preview: bool,
) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_startup_visible(false);
    sync_bridge_pad_launcher(&bridge, pad);
    bridge.set_screen_mode(match nav.screen {
        Screen::Home => 0,
        Screen::Controller => 1,
        Screen::Arcade => 2,
        Screen::Settings => 3,
    });
    bridge.set_clock_text(launcher_clock_text().into());
    bridge.set_selected_index(nav.selected as i32);
    bridge.set_home_scroll_x(nav.scroll_x);
    bridge.set_settings_focused(nav.settings_focused);
    bridge.set_settings_selected(nav.settings_selected as i32);
    bridge.set_simple_joystick_handling(nav.settings.simple_joystick_handling);
    sync_arcade_list_geometry_bridge(&bridge, nav);
    if !(defer_selected_preview && nav.screen == Screen::Arcade) {
        bridge.set_arcade_selected(nav.arcade.selected as i32);
        bridge.set_arcade_scroll_y(nav.arcade.scroll_y);
    }
    let mut active_games_for_preview: Option<ArcadeGameView<'_>> = None;
    let mut active_games_loading = false;
    if let Some(catalog) = catalog {
        let games = active_system_game_view(catalog, nav);
        let system = active_system(catalog, nav);
        let base_title = system
            .map(|system| system.title.clone())
            .unwrap_or_else(|| "Games".to_string());
        let filter_label = nav.arcade_filter.active_label();
        let title = if filter_label == "Games A-Z" {
            base_title
        } else {
            format!("{base_title} - {filter_label}")
        };
        let count = system
            .map(|system| nav.active_arcade_game_count(catalog, &system.id))
            .unwrap_or_else(|| games.len());
        active_games_loading = active_system_games_loading(catalog, nav);
        bridge.set_game_systems(models.game_systems(catalog, catalog_version));
        bridge.set_active_system_title(title.into());
        bridge.set_active_system_count(count as i32);
        active_games_for_preview = Some(games);
    }
    bridge.set_arcade_games_loading(active_games_loading);
    sync_arcade_search_bridge(&bridge, nav);
    bridge.set_confirm_visible(nav.confirm_action.is_some());
    bridge.set_confirm_selected(nav.confirm_selected as i32);
    sync_confirm_bridge(&bridge, nav.confirm_action);
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
        );
    } else {
        preview.clear(&bridge);
    }
    sync_setup_bridge(&bridge, pad, setup);
}

pub(super) fn sync_bridge_launcher_light(
    app: &slint_ui::launcher::Launcher,
    nav: &LauncherNav,
    setup: &SetupNav,
    loading_message: &str,
    loading_detail: &str,
    catalog: &ArcadeCatalog,
    active_arcade_games: Option<ArcadeGameView<'_>>,
    preview: &mut PreviewState,
    defer_arcade_overlay_bridge: bool,
    defer_selected_preview: bool,
) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let active_games_loading = active_system_games_loading(catalog, nav);
    set_bridge_if_changed!(
        bridge,
        get_screen_mode,
        set_screen_mode,
        match nav.screen {
            Screen::Home => 0,
            Screen::Controller => 1,
            Screen::Arcade => 2,
            Screen::Settings => 3,
        }
    );
    set_bridge_if_changed!(
        bridge,
        get_selected_index,
        set_selected_index,
        nav.selected as i32
    );
    set_bridge_if_changed!(bridge, get_home_scroll_x, set_home_scroll_x, nav.scroll_x);
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
        get_simple_joystick_handling,
        set_simple_joystick_handling,
        nav.settings.simple_joystick_handling
    );
    sync_arcade_list_geometry_bridge_if_changed(&bridge, nav);
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
    sync_confirm_bridge(&bridge, nav.confirm_action);
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

pub(super) fn slint_game_systems(
    catalog: &ArcadeCatalog,
) -> ModelRc<slint_ui::launcher::GameSystem> {
    let rows: Vec<slint_ui::launcher::GameSystem> = catalog
        .systems
        .iter()
        .map(|system| slint_ui::launcher::GameSystem {
            id: system.id.clone().into(),
            title: system.title.clone().into(),
            count: system.count as i32,
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

pub(super) fn empty_arcade_catalog(root: &str) -> ArcadeCatalog {
    ArcadeCatalog::new(PathBuf::from(root), Vec::new(), Vec::new())
}

pub(super) fn active_system<'a>(
    catalog: &'a ArcadeCatalog,
    nav: &LauncherNav,
) -> Option<&'a arcade_catalog::GameSystemEntry> {
    catalog.systems.get(nav.selected)
}

pub(super) fn active_system_game_view<'a>(
    catalog: &'a ArcadeCatalog,
    nav: &'a LauncherNav,
) -> ArcadeGameView<'a> {
    active_system(catalog, nav)
        .map(|system| nav.active_arcade_game_view(catalog, &system.id))
        .unwrap_or_else(ArcadeGameView::empty)
}

fn sync_arcade_search_bridge(bridge: &slint_ui::launcher::MisterBridge, nav: &LauncherNav) {
    bridge.set_arcade_search_active(nav.arcade_search.is_active(&nav.arcade_filter.active));
    bridge.set_arcade_search_query(nav.arcade_search.query.clone().into());
    bridge.set_arcade_search_suggestion(nav.arcade_search.suggestion.clone().into());
    bridge.set_arcade_search_key_selected(nav.arcade_search.selected_key as i32);
    bridge.set_arcade_search_pane(match nav.arcade_search.pane {
        launcher::ArcadeSearchPane::Keyboard => 0,
        launcher::ArcadeSearchPane::Results => 1,
    });
}

fn sync_arcade_list_geometry_bridge_if_changed(
    bridge: &slint_ui::launcher::MisterBridge,
    nav: &LauncherNav,
) {
    let geometry = if nav.arcade_search.is_active(&nav.arcade_filter.active) {
        ArcadeListGeometry::SEARCH
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
    active_system(catalog, nav)
        .is_some_and(|system| system.count > 0 && catalog.system_game_count(&system.id) == 0)
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
    selected: usize,
    scroll_x: i32,
    settings_focused: bool,
    settings_selected: usize,
    simple_joystick_handling: bool,
    confirm_action: Option<launcher::ConfirmAction>,
    confirm_selected: usize,
    arcade_selected: usize,
    arcade_filter_open: bool,
    arcade_filter_level: launcher::ArcadeFilterLevel,
    arcade_filter_selected: usize,
    arcade_filter_active: arcade_catalog::ArcadeFilter,
    arcade_search_query: String,
    arcade_search_suggestion: String,
    arcade_search_selected_key: usize,
    arcade_search_pane: launcher::ArcadeSearchPane,
}

impl LauncherBridgeKey {
    pub(super) fn from_nav(nav: &LauncherNav) -> Self {
        Self {
            screen: nav.screen,
            selected: nav.selected,
            scroll_x: nav.scroll_x,
            settings_focused: nav.settings_focused,
            settings_selected: nav.settings_selected,
            simple_joystick_handling: nav.settings.simple_joystick_handling,
            confirm_action: nav.confirm_action,
            confirm_selected: nav.confirm_selected,
            arcade_selected: nav.arcade.selected,
            arcade_filter_open: nav.arcade_filter.drawer_open,
            arcade_filter_level: nav.arcade_filter.level,
            arcade_filter_selected: nav.arcade_filter.selected,
            arcade_filter_active: nav.arcade_filter.active.clone(),
            arcade_search_query: nav.arcade_search.query.clone(),
            arcade_search_suggestion: nav.arcade_search.suggestion.clone(),
            arcade_search_selected_key: nav.arcade_search.selected_key,
            arcade_search_pane: nav.arcade_search.pane,
        }
    }
}

#[derive(Default)]
pub(super) struct LauncherBridgeModels {
    game_systems_key: Option<usize>,
    game_systems: Option<ModelRc<slint_ui::launcher::GameSystem>>,
}

impl LauncherBridgeModels {
    pub(super) fn game_systems(
        &mut self,
        catalog: &ArcadeCatalog,
        catalog_version: usize,
    ) -> ModelRc<slint_ui::launcher::GameSystem> {
        if self.game_systems_key != Some(catalog_version) {
            self.game_systems = Some(slint_game_systems(catalog));
            self.game_systems_key = Some(catalog_version);
        }
        self.game_systems
            .as_ref()
            .expect("game system model should be initialized")
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn library_update_failed_uses_single_ok_button() {
        let text = confirm_bridge_text(Some(launcher::ConfirmAction::LibraryUpdateFailed));

        assert_eq!(text.left_label, "OK");
        assert_eq!(text.right_label, "");
    }
}
