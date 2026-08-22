// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;

const LAUNCHER_UI_ACTION_CAPACITY: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum LauncherUiAction {
    Navigate(slint_ui::launcher::NavigationDirection),
    Activate,
    Back,
    Home,
    SelectMenuItem(String),
    SelectSystemHub(slint_ui::launcher::SystemHubSection),
    SelectSettingsSection(slint_ui::launcher::SettingsSection),
    SelectDisplayOption(String),
    SelectOrientation(slint_ui::launcher::ScreenOrientation),
    SelectScreensaverSetting(slint_ui::launcher::ScreensaverSetting),
    SetScreensaverEnabled(bool),
    SetScreensaverDelay(i32),
    SetReduceMotion(bool),
    SetSimpleJoystickHandling(bool),
    SelectAboutSection(slint_ui::launcher::AboutSection),
    SelectLicense(usize),
    SelectArcadeGame(String),
    SelectArcadeSearchPane(slint_ui::launcher::ArcadeSearchPane),
    SelectArcadeSearchKey(usize),
    SelectSetupEntry(usize),
    ChooseConfirmation(slint_ui::launcher::DialogChoice),
    DismissOverlay,
    Quit,
}

#[derive(Default)]
struct LauncherUiActionState {
    pending: VecDeque<LauncherUiAction>,
    rejected: u64,
}

pub(super) struct LauncherUiActionsAdapter {
    _state: Rc<RefCell<LauncherUiActionState>>,
}

impl LauncherUiActionsAdapter {
    pub(super) fn install(app: &slint_ui::launcher::Launcher) -> Self {
        let state = Rc::new(RefCell::new(LauncherUiActionState::default()));
        let actions = app.global::<slint_ui::launcher::LauncherActions>();

        bind_simple_action(&actions, &state);
        bind_stable_selection_actions(app, &actions, &state);
        bind_settings_actions(app, &actions, &state);
        bind_indexed_actions(app, &actions, &state);

        Self { _state: state }
    }

    #[cfg(test)]
    fn drain_for_test(&self) -> Vec<LauncherUiAction> {
        self._state.borrow_mut().pending.drain(..).collect()
    }

    #[cfg(test)]
    fn rejected_for_test(&self) -> u64 {
        self._state.borrow().rejected
    }
}

fn enqueue(state: &Rc<RefCell<LauncherUiActionState>>, action: LauncherUiAction) {
    let mut state = state.borrow_mut();
    if state.pending.len() >= LAUNCHER_UI_ACTION_CAPACITY {
        state.rejected = state.rejected.saturating_add(1);
        return;
    }
    state.pending.push_back(action);
}

fn reject(state: &Rc<RefCell<LauncherUiActionState>>) {
    let mut state = state.borrow_mut();
    state.rejected = state.rejected.saturating_add(1);
}

fn bind_simple_action(
    actions: &slint_ui::launcher::LauncherActions,
    state: &Rc<RefCell<LauncherUiActionState>>,
) {
    let queue = state.clone();
    actions.on_navigate(move |direction| enqueue(&queue, LauncherUiAction::Navigate(direction)));
    let queue = state.clone();
    actions.on_activate(move || enqueue(&queue, LauncherUiAction::Activate));
    let queue = state.clone();
    actions.on_back(move || enqueue(&queue, LauncherUiAction::Back));
    let queue = state.clone();
    actions.on_home(move || enqueue(&queue, LauncherUiAction::Home));
    let queue = state.clone();
    actions.on_select_system_hub(move |section| {
        enqueue(&queue, LauncherUiAction::SelectSystemHub(section));
    });
    let queue = state.clone();
    actions.on_select_settings_section(move |section| {
        enqueue(&queue, LauncherUiAction::SelectSettingsSection(section));
    });
    let queue = state.clone();
    actions.on_select_orientation(move |orientation| {
        enqueue(&queue, LauncherUiAction::SelectOrientation(orientation));
    });
    let queue = state.clone();
    actions.on_select_screensaver_setting(move |setting| {
        enqueue(&queue, LauncherUiAction::SelectScreensaverSetting(setting));
    });
    let queue = state.clone();
    actions.on_set_screensaver_enabled(move |enabled| {
        enqueue(&queue, LauncherUiAction::SetScreensaverEnabled(enabled));
    });
    let queue = state.clone();
    actions.on_set_reduce_motion(move |enabled| {
        enqueue(&queue, LauncherUiAction::SetReduceMotion(enabled));
    });
    let queue = state.clone();
    actions.on_set_simple_joystick_handling(move |enabled| {
        enqueue(&queue, LauncherUiAction::SetSimpleJoystickHandling(enabled));
    });
    let queue = state.clone();
    actions.on_select_about_section(move |section| {
        enqueue(&queue, LauncherUiAction::SelectAboutSection(section));
    });
    let queue = state.clone();
    actions.on_select_arcade_search_pane(move |pane| {
        enqueue(&queue, LauncherUiAction::SelectArcadeSearchPane(pane));
    });
    let queue = state.clone();
    actions.on_choose_confirmation(move |choice| {
        enqueue(&queue, LauncherUiAction::ChooseConfirmation(choice));
    });
    let queue = state.clone();
    actions.on_dismiss_overlay(move || enqueue(&queue, LauncherUiAction::DismissOverlay));
    let queue = state.clone();
    actions.on_quit(move || enqueue(&queue, LauncherUiAction::Quit));
}

fn bind_stable_selection_actions(
    app: &slint_ui::launcher::Launcher,
    actions: &slint_ui::launcher::LauncherActions,
    state: &Rc<RefCell<LauncherUiActionState>>,
) {
    let weak = app.as_weak();
    let queue = state.clone();
    actions.on_select_menu_item(move |id| {
        let id = id.to_string();
        if weak
            .upgrade()
            .is_some_and(|app| projected_menu_id_exists(&app, &id))
        {
            enqueue(&queue, LauncherUiAction::SelectMenuItem(id));
        } else {
            reject(&queue);
        }
    });

    let weak = app.as_weak();
    let queue = state.clone();
    actions.on_select_display_option(move |id| {
        let id = id.to_string();
        if weak
            .upgrade()
            .is_some_and(|app| projected_display_id_exists(&app, &id))
        {
            enqueue(&queue, LauncherUiAction::SelectDisplayOption(id));
        } else {
            reject(&queue);
        }
    });

    let weak = app.as_weak();
    let queue = state.clone();
    actions.on_select_arcade_game(move |id| {
        let id = id.to_string();
        if weak
            .upgrade()
            .is_some_and(|app| projected_arcade_id_exists(&app, &id))
        {
            enqueue(&queue, LauncherUiAction::SelectArcadeGame(id));
        } else {
            reject(&queue);
        }
    });
}

fn bind_settings_actions(
    _app: &slint_ui::launcher::Launcher,
    actions: &slint_ui::launcher::LauncherActions,
    state: &Rc<RefCell<LauncherUiActionState>>,
) {
    let queue = state.clone();
    actions.on_set_screensaver_delay(move |minutes| {
        if (1..=10).contains(&minutes) {
            enqueue(&queue, LauncherUiAction::SetScreensaverDelay(minutes));
        } else {
            reject(&queue);
        }
    });
}

fn bind_indexed_actions(
    app: &slint_ui::launcher::Launcher,
    actions: &slint_ui::launcher::LauncherActions,
    state: &Rc<RefCell<LauncherUiActionState>>,
) {
    let weak = app.as_weak();
    let queue = state.clone();
    actions.on_select_license(move |index| {
        enqueue_projected_index(
            &weak,
            &queue,
            index,
            |app| {
                app.global::<slint_ui::launcher::SettingsView>()
                    .get_license_titles()
                    .row_count()
            },
            LauncherUiAction::SelectLicense,
        );
    });
    let weak = app.as_weak();
    let queue = state.clone();
    actions.on_select_arcade_search_key(move |index| {
        enqueue_projected_index(
            &weak,
            &queue,
            index,
            |app| {
                app.global::<slint_ui::launcher::ArcadeView>()
                    .get_search_keys()
                    .row_count()
            },
            LauncherUiAction::SelectArcadeSearchKey,
        );
    });
    let weak = app.as_weak();
    let queue = state.clone();
    actions.on_select_setup_entry(move |index| {
        enqueue_projected_index(
            &weak,
            &queue,
            index,
            |app| {
                app.global::<slint_ui::launcher::SetupView>()
                    .get_entries()
                    .row_count()
            },
            LauncherUiAction::SelectSetupEntry,
        );
    });
}

fn enqueue_projected_index<F, G>(
    app: &slint::Weak<slint_ui::launcher::Launcher>,
    state: &Rc<RefCell<LauncherUiActionState>>,
    index: i32,
    count: F,
    action: G,
) where
    F: FnOnce(&slint_ui::launcher::Launcher) -> usize,
    G: FnOnce(usize) -> LauncherUiAction,
{
    let index = usize::try_from(index).ok();
    let valid = app
        .upgrade()
        .and_then(|app| index.filter(|index| *index < count(&app)));
    if let Some(index) = valid {
        enqueue(state, action(index));
    } else {
        reject(state);
    }
}

fn projected_menu_id_exists(app: &slint_ui::launcher::Launcher, id: &str) -> bool {
    let model = app
        .global::<slint_ui::launcher::NavigationView>()
        .get_menu_items();
    (0..model.row_count()).any(|index| {
        model
            .row_data(index)
            .is_some_and(|row| row.id.as_str() == id)
    })
}

fn projected_display_id_exists(app: &slint_ui::launcher::Launcher, id: &str) -> bool {
    let model = app
        .global::<slint_ui::launcher::SettingsView>()
        .get_display_options();
    (0..model.row_count()).any(|index| {
        model
            .row_data(index)
            .is_some_and(|row| row.id.as_str() == id)
    })
}

fn projected_arcade_id_exists(app: &slint_ui::launcher::Launcher, id: &str) -> bool {
    let model = app.global::<slint_ui::launcher::ArcadeView>().get_games();
    (0..model.row_count()).any(|index| {
        model
            .row_data(index)
            .is_some_and(|row| row.mra_path.as_str() == id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn launcher() -> slint_ui::launcher::Launcher {
        let window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let fixed_time = Some(Rc::new(Cell::new(Duration::ZERO)));
        let _ = slint::platform::set_platform(Box::new(MisterPlatform::new(window, fixed_time)));
        slint_ui::launcher::Launcher::new().expect("launcher component")
    }

    #[test]
    fn adapters_validate_stable_ids_and_dynamic_indices_without_projecting_state() {
        let app = launcher();
        let navigation = app.global::<slint_ui::launcher::NavigationView>();
        navigation.set_screen(slint_ui::launcher::LauncherScreen::Home);
        navigation.set_menu_items(ModelRc::new(VecModel::from(vec![
            slint_ui::launcher::MenuItem {
                id: "arcade".into(),
                ..Default::default()
            },
        ])));
        app.global::<slint_ui::launcher::SettingsView>()
            .set_display_options(ModelRc::new(VecModel::from(vec![
                slint_ui::launcher::ChoiceOption {
                    id: "1080p60".into(),
                    label: "1080p 60 Hz".into(),
                },
            ])));
        app.global::<slint_ui::launcher::ArcadeView>()
            .set_games(ModelRc::new(VecModel::from(vec![
                slint_ui::launcher::ArcadeGame {
                    mra_path: "/media/fat/_Arcade/Out Run.mra".into(),
                    ..Default::default()
                },
            ])));
        app.global::<slint_ui::launcher::SettingsView>()
            .set_license_titles(ModelRc::new(VecModel::from(vec!["GPL-3.0".into()])));

        let adapter = LauncherUiActionsAdapter::install(&app);
        let actions = app.global::<slint_ui::launcher::LauncherActions>();
        actions.invoke_select_menu_item("arcade".into());
        actions.invoke_select_menu_item("stale-menu".into());
        actions.invoke_select_display_option("1080p60".into());
        actions.invoke_select_arcade_game("/media/fat/_Arcade/Out Run.mra".into());
        actions.invoke_select_arcade_game("Out Run".into());
        actions.invoke_select_license(0);
        actions.invoke_select_license(1);

        assert_eq!(
            navigation.get_screen(),
            slint_ui::launcher::LauncherScreen::Home
        );
        assert_eq!(
            adapter.drain_for_test(),
            vec![
                LauncherUiAction::SelectMenuItem("arcade".to_string()),
                LauncherUiAction::SelectDisplayOption("1080p60".to_string()),
                LauncherUiAction::SelectArcadeGame("/media/fat/_Arcade/Out Run.mra".to_string()),
                LauncherUiAction::SelectLicense(0),
            ]
        );
        assert_eq!(adapter.rejected_for_test(), 3);
    }

    #[test]
    fn action_queue_is_bounded_and_rejects_invalid_numeric_settings() {
        let app = launcher();
        let adapter = LauncherUiActionsAdapter::install(&app);
        let actions = app.global::<slint_ui::launcher::LauncherActions>();
        actions.invoke_set_screensaver_delay(0);
        actions.invoke_set_screensaver_delay(11);
        for _ in 0..=LAUNCHER_UI_ACTION_CAPACITY {
            actions.invoke_activate();
        }

        assert_eq!(adapter.drain_for_test().len(), LAUNCHER_UI_ACTION_CAPACITY);
        assert_eq!(adapter.rejected_for_test(), 3);
    }
}
