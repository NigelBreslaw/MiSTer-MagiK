use super::*;

pub(super) fn open_pads() -> PadPool {
    PadPool::open_all().unwrap_or_else(|e| {
        eprintln!("failed to initialize gamepad input: {e}");
        std::process::exit(1);
    })
}

pub(super) fn init_launcher_bridge(app: &slint_ui::launcher::Launcher, pad: &PadPool) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_startup_visible(true);
    bridge.set_screen_mode(0);
    bridge.set_selected_index(0);
    bridge.set_settings_focused(false);
    bridge.set_settings_selected(0);
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
    bridge.set_arcade_selected(0);
    bridge.set_arcade_scroll_y(0);
    sync_launcher_arcade_geometry_bridge(&bridge);
    bridge.set_arcade_preview_placeholder_visible(true);
    bridge.set_arcade_preview_status(PreviewStatus::Empty);
    bridge.set_arcade_preview_title("".into());
    bridge.set_arcade_preview_run_label(preview_run_label().into());
    bridge.set_arcade_preview_source_width(0);
    bridge.set_arcade_preview_source_height(0);
    bridge.set_arcade_preview_display_width(0);
    bridge.set_arcade_preview_display_height(0);
    bridge.set_catalog_scan_visible(false);
    bridge.set_catalog_scan_title("".into());
    bridge.set_catalog_scan_detail("".into());
    bridge.set_catalog_scan_percent(-1);
    bridge.set_catalog_background_scan_visible(false);
    bridge.set_setup_visible(false);
    bridge.set_setup_phase(0);
    sync_bridge_pad_launcher(&bridge, pad);
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

pub(super) fn sync_bridge(app: &slint_ui::controller::ControllerTest, pad: &PadPool) {
    sync_bridge_pad_controller(&app.global::<slint_ui::controller::MisterBridge>(), pad);
}

pub(super) fn sync_confirm_bridge(
    bridge: &slint_ui::launcher::MisterBridge,
    action: Option<launcher::ConfirmAction>,
) {
    match action {
        Some(launcher::ConfirmAction::ExitToMister) => {
            bridge.set_confirm_title("Exit to MiSTer".into());
            bridge.set_confirm_message("Use the stock MiSTer menu until reboot.".into());
            bridge.set_confirm_left_label("Exit to MiSTer".into());
            bridge.set_confirm_right_label("Return to MiSTer MagiK".into());
        }
        Some(launcher::ConfirmAction::ResetDatabase) => {
            bridge.set_confirm_title("Reset Database?".into());
            bridge.set_confirm_message("Delete the library database and reboot the MiSTer?".into());
            bridge.set_confirm_left_label("Cancel".into());
            bridge.set_confirm_right_label("Confirm".into());
        }
        Some(launcher::ConfirmAction::Restart) => {
            bridge.set_confirm_title("Restart MiSTer?".into());
            bridge.set_confirm_message("Reboot the MiSTer now?".into());
            bridge.set_confirm_left_label("Cancel".into());
            bridge.set_confirm_right_label("Confirm".into());
        }
        None => {
            bridge.set_confirm_title("".into());
            bridge.set_confirm_message("".into());
            bridge.set_confirm_left_label("".into());
            bridge.set_confirm_right_label("".into());
        }
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
    if !(defer_selected_preview && nav.screen == Screen::Arcade) {
        bridge.set_arcade_selected(nav.arcade.selected as i32);
        bridge.set_arcade_scroll_y(nav.arcade.scroll_y);
    }
    let mut active_games_for_preview: Option<&[ArcadeGameEntry]> = None;
    if let Some(catalog) = catalog {
        let games = active_system_game_slice(catalog, nav);
        let title = active_system(catalog, nav)
            .map(|system| system.title.clone())
            .unwrap_or_else(|| "Games".to_string());
        bridge.set_game_systems(models.game_systems(catalog, catalog_version));
        bridge.set_active_system_title(title.into());
        bridge.set_arcade_games(models.arcade_games(catalog, nav, catalog_version));
        active_games_for_preview = Some(games);
    }
    bridge.set_confirm_visible(nav.confirm_action.is_some());
    bridge.set_confirm_selected(nav.confirm_selected as i32);
    sync_confirm_bridge(&bridge, nav.confirm_action);
    bridge.set_loading_message(loading_message.into());
    bridge.set_loading_detail(loading_detail.into());
    if nav.screen == Screen::Arcade {
        let games = active_games_for_preview
            .or_else(|| catalog.map(|catalog| active_system_game_slice(catalog, nav)))
            .unwrap_or(&[]);
        let _ = request_arcade_preview_window(
            &bridge,
            games,
            nav.arcade.selected,
            preview,
            defer_selected_preview,
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
    active_arcade_games: Option<&[ArcadeGameEntry]>,
    preview: &mut PreviewState,
    defer_selected_preview: bool,
) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_screen_mode(match nav.screen {
        Screen::Home => 0,
        Screen::Controller => 1,
        Screen::Arcade => 2,
        Screen::Settings => 3,
    });
    bridge.set_selected_index(nav.selected as i32);
    bridge.set_home_scroll_x(nav.scroll_x);
    bridge.set_settings_focused(nav.settings_focused);
    bridge.set_settings_selected(nav.settings_selected as i32);
    if !(defer_selected_preview && nav.screen == Screen::Arcade) {
        bridge.set_arcade_selected(nav.arcade.selected as i32);
        bridge.set_arcade_scroll_y(nav.arcade.scroll_y);
    }
    bridge.set_confirm_visible(nav.confirm_action.is_some());
    bridge.set_confirm_selected(nav.confirm_selected as i32);
    sync_confirm_bridge(&bridge, nav.confirm_action);
    bridge.set_loading_message(loading_message.into());
    bridge.set_loading_detail(loading_detail.into());
    if nav.screen == Screen::Arcade {
        let games = active_arcade_games.unwrap_or_else(|| active_system_game_slice(catalog, nav));
        schedule_arcade_preview_window(
            &bridge,
            games,
            nav.arcade.selected,
            preview,
            defer_selected_preview,
        );
    } else {
        preview.clear(&bridge);
    }
    bridge.set_setup_visible(setup.is_active());
}

pub(super) fn launcher_clock_text() -> String {
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

pub(super) fn slint_arcade_games(
    games: &[ArcadeGameEntry],
) -> ModelRc<slint_ui::launcher::ArcadeGame> {
    let rows: Vec<slint_ui::launcher::ArcadeGame> = games
        .iter()
        .map(|g| slint_ui::launcher::ArcadeGame {
            title: g.title.as_ref().into(),
            mra_path: g.mra_path.as_ref().into(),
            preview_archive_path: g.preview_archive_path.as_ref().into(),
            preview_asset_key: g.preview_asset_key.as_ref().into(),
            has_preview: g.has_preview,
            system_id: g.system_id.as_ref().into(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
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
            count: catalog.system_game_count(&system.id) as i32,
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

pub(super) fn active_system_game_slice<'a>(
    catalog: &'a ArcadeCatalog,
    nav: &LauncherNav,
) -> &'a [ArcadeGameEntry] {
    active_system(catalog, nav)
        .map(|system| catalog.system_game_slice(&system.id))
        .unwrap_or(&[])
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
    confirm_action: Option<launcher::ConfirmAction>,
    confirm_selected: usize,
    arcade_selected: usize,
}

impl LauncherBridgeKey {
    pub(super) fn from_nav(nav: &LauncherNav) -> Self {
        Self {
            screen: nav.screen,
            selected: nav.selected,
            scroll_x: nav.scroll_x,
            settings_focused: nav.settings_focused,
            settings_selected: nav.settings_selected,
            confirm_action: nav.confirm_action,
            confirm_selected: nav.confirm_selected,
            arcade_selected: nav.arcade.selected,
        }
    }
}

#[derive(Default)]
pub(super) struct LauncherBridgeModels {
    game_systems_key: Option<usize>,
    game_systems: Option<ModelRc<slint_ui::launcher::GameSystem>>,
    arcade_games_key: Option<(usize, usize)>,
    arcade_games: Option<ModelRc<slint_ui::launcher::ArcadeGame>>,
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

    pub(super) fn arcade_games(
        &mut self,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
        catalog_version: usize,
    ) -> ModelRc<slint_ui::launcher::ArcadeGame> {
        let key = (catalog_version, nav.selected);
        if self.arcade_games_key != Some(key) {
            self.arcade_games = Some(slint_arcade_games(active_system_game_slice(catalog, nav)));
            self.arcade_games_key = Some(key);
        }
        self.arcade_games
            .as_ref()
            .expect("arcade game model should be initialized")
            .clone()
    }
}
