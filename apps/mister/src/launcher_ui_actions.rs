// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::arcade_catalog::{ArcadeCatalog, ArcadeGameView};
use crate::input_event::{InputPhase, InputSourceKind, LogicalAction};
use crate::launcher::{self, LauncherAction, LauncherNav};
use crate::ui_display::ScreenOrientation;
use mister_magik_ui as slint_ui;
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

const LAUNCHER_UI_ACTION_CAPACITY: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum LauncherUiAction {
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

pub struct LauncherUiActionsAdapter {
    state: Rc<RefCell<LauncherUiActionState>>,
}

impl LauncherUiActionsAdapter {
    pub fn install(app: &slint_ui::launcher::Launcher) -> Self {
        let state = Rc::new(RefCell::new(LauncherUiActionState::default()));
        let actions = app.global::<slint_ui::launcher::LauncherActions>();

        bind_simple_action(&actions, &state);
        bind_stable_selection_actions(app, &actions, &state);
        bind_settings_actions(app, &actions, &state);
        bind_indexed_actions(app, &actions, &state);

        Self { state }
    }

    pub fn has_pending(&self) -> bool {
        !self.state.borrow().pending.is_empty()
    }

    pub fn pop_front(&self) -> Option<LauncherUiAction> {
        self.state.borrow_mut().pending.pop_front()
    }

    pub fn pop_routable(&self, allowed: bool) -> Option<LauncherUiAction> {
        let mut state = self.state.borrow_mut();
        let action = state.pending.pop_front();
        if action.is_some() && !allowed {
            state.rejected = state.rejected.saturating_add(1);
            return None;
        }
        action
    }

    pub fn deny_all(&self) {
        let mut state = self.state.borrow_mut();
        state.rejected = state
            .rejected
            .saturating_add(state.pending.len().try_into().unwrap_or(u64::MAX));
        state.pending.clear();
    }

    pub fn discard_all(&self) {
        self.state.borrow_mut().pending.clear();
    }

    #[cfg(test)]
    fn drain_for_test(&self) -> Vec<LauncherUiAction> {
        self.state.borrow_mut().pending.drain(..).collect()
    }

    #[cfg(test)]
    fn rejected_for_test(&self) -> u64 {
        self.state.borrow().rejected
    }
}

impl LauncherUiAction {
    pub fn input_event(
        &self,
        sequence: u64,
        captured_at_us: u64,
    ) -> Option<crate::input_event::InputEvent> {
        let action = match self {
            Self::Navigate(slint_ui::launcher::NavigationDirection::Up) => LogicalAction::Up,
            Self::Navigate(slint_ui::launcher::NavigationDirection::Down) => LogicalAction::Down,
            Self::Navigate(slint_ui::launcher::NavigationDirection::Left) => LogicalAction::Left,
            Self::Navigate(slint_ui::launcher::NavigationDirection::Right) => LogicalAction::Right,
            Self::Activate => LogicalAction::Activate,
            Self::Back | Self::DismissOverlay => LogicalAction::Back,
            Self::Home => LogicalAction::Home,
            _ => return None,
        };
        Some(crate::input_event::InputEvent {
            source: crate::input_event::InputSourceId {
                kind: InputSourceKind::Ui,
                instance: 0,
            },
            source_epoch: crate::input_event::SourceEpoch(0),
            sequence,
            press_id: crate::input_event::PressId(sequence),
            captured_at_us,
            action,
            phase: InputPhase::Pressed,
        })
    }
}

pub fn apply_navigation_action(
    action: LauncherUiAction,
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    frame_now: Instant,
) -> Option<launcher::LauncherEvent> {
    match action {
        LauncherUiAction::SelectMenuItem(id) => {
            if let Some(index) = nav
                .current_menu_items()
                .iter()
                .position(|item| item.id == id)
            {
                nav.selected = index;
            }
            None
        }
        LauncherUiAction::SelectSystemHub(section) => {
            nav.system_hub_selected = match section {
                slint_ui::launcher::SystemHubSection::Games => 0,
                slint_ui::launcher::SystemHubSection::Recent => 1,
                slint_ui::launcher::SystemHubSection::Favourites => 2,
                slint_ui::launcher::SystemHubSection::Information => 3,
            };
            None
        }
        LauncherUiAction::SelectSettingsSection(section) => {
            nav.settings_selected = match section {
                slint_ui::launcher::SettingsSection::Display => 0,
                slint_ui::launcher::SettingsSection::Orientation => 1,
                slint_ui::launcher::SettingsSection::Screensaver => 2,
                slint_ui::launcher::SettingsSection::ReduceMotion => 3,
                slint_ui::launcher::SettingsSection::Exit => 4,
                slint_ui::launcher::SettingsSection::Refresh => 5,
                slint_ui::launcher::SettingsSection::About => 6,
            };
            None
        }
        LauncherUiAction::SelectDisplayOption(id) => {
            let highlighted = launcher::settings_display_resolution_index(&id)?;
            nav.display_combo_open = false;
            nav.display_highlighted = highlighted;
            let mode = launcher::settings_display_resolution(highlighted)?;
            if mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
                .get(nav.display_selected)
                .is_some_and(|selected| selected.id == mode.id)
            {
                return None;
            }
            Some(launcher::LauncherEvent {
                action: LauncherAction::ApplyDisplayResolution,
                path: Some(mode.id.to_string()),
                settings: None,
            })
        }
        LauncherUiAction::SelectOrientation(orientation) => {
            let index = match orientation {
                slint_ui::launcher::ScreenOrientation::Normal => 0,
                slint_ui::launcher::ScreenOrientation::MonitorClockwise => 1,
                slint_ui::launcher::ScreenOrientation::MonitorCounterclockwise => 2,
            };
            nav.orientation_combo_open = false;
            nav.orientation_highlighted = index;
            if nav.orientation_selected == index {
                return None;
            }
            Some(launcher::LauncherEvent {
                action: LauncherAction::ApplyScreenOrientation,
                path: Some(ScreenOrientation::ALL[index].id().to_string()),
                settings: None,
            })
        }
        LauncherUiAction::SelectScreensaverSetting(setting) => {
            nav.screensaver_selected = match setting {
                slint_ui::launcher::ScreensaverSetting::Enabled => 0,
                slint_ui::launcher::ScreensaverSetting::Delay => 1,
                slint_ui::launcher::ScreensaverSetting::Preview => 2,
            };
            None
        }
        LauncherUiAction::SetScreensaverEnabled(enabled) => {
            persist_settings(nav, |settings| settings.screensaver_enabled = enabled)
        }
        LauncherUiAction::SetScreensaverDelay(minutes) => persist_settings(nav, |settings| {
            settings.screensaver_delay_minutes = minutes as u8;
        }),
        LauncherUiAction::SetReduceMotion(enabled) => {
            persist_settings(nav, |settings| settings.reduce_motion = enabled)
        }
        LauncherUiAction::SetSimpleJoystickHandling(enabled) => persist_settings(nav, |settings| {
            settings.simple_joystick_handling = enabled;
        }),
        LauncherUiAction::SelectAboutSection(section) => {
            nav.about_selected = match section {
                slint_ui::launcher::AboutSection::Information => 0,
                slint_ui::launcher::AboutSection::Licenses => 1,
            };
            None
        }
        LauncherUiAction::SelectLicense(index) => {
            nav.licenses_selected = index;
            None
        }
        LauncherUiAction::SelectArcadeGame(id) => {
            let collection_id = nav.active_collection_scope_id(catalog).to_string();
            let games: ArcadeGameView<'_> = nav.active_arcade_game_view(catalog, &collection_id);
            if let Some(index) = games.iter().position(|game| game.mra_path.as_ref() == id) {
                nav.arcade.restore_position(
                    index,
                    index as i32 * nav.arcade.row_height(),
                    games.len(),
                );
            }
            None
        }
        LauncherUiAction::SelectArcadeSearchPane(pane) => {
            nav.arcade_search.pane = match pane {
                slint_ui::launcher::ArcadeSearchPane::Keyboard => {
                    launcher::ArcadeSearchPane::Keyboard
                }
                slint_ui::launcher::ArcadeSearchPane::Results => {
                    launcher::ArcadeSearchPane::Results
                }
            };
            None
        }
        LauncherUiAction::SelectArcadeSearchKey(index) => {
            nav.arcade_search.selected_key = index;
            None
        }
        LauncherUiAction::ChooseConfirmation(choice) => {
            nav.confirm_selected = match choice {
                slint_ui::launcher::DialogChoice::Cancel => 0,
                slint_ui::launcher::DialogChoice::Confirm => 1,
            };
            let event = crate::input_event::InputEvent {
                source: crate::input_event::InputSourceId {
                    kind: InputSourceKind::Ui,
                    instance: 0,
                },
                source_epoch: crate::input_event::SourceEpoch(0),
                sequence: 0,
                press_id: crate::input_event::PressId(0),
                captured_at_us: 0,
                action: LogicalAction::Activate,
                phase: InputPhase::Pressed,
            };
            nav.handle_action_with_navigation_intents(&event, frame_now, catalog)
        }
        LauncherUiAction::Quit => Some(launcher::LauncherEvent {
            action: LauncherAction::ExitToMister,
            path: None,
            settings: None,
        }),
        LauncherUiAction::SelectSetupEntry(_)
        | LauncherUiAction::Navigate(_)
        | LauncherUiAction::Activate
        | LauncherUiAction::Back
        | LauncherUiAction::Home
        | LauncherUiAction::DismissOverlay => None,
    }
}

fn persist_settings(
    nav: &mut LauncherNav,
    update: impl FnOnce(&mut crate::settings::MagikSettings),
) -> Option<launcher::LauncherEvent> {
    let mut next = nav.settings.clone();
    update(&mut next);
    if next == nav.settings {
        return None;
    }
    nav.settings = next.clone();
    Some(launcher::LauncherEvent {
        action: LauncherAction::PersistSettings,
        path: None,
        settings: Some(next),
    })
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
    use crate::visual_platform::{MisterPlatform, MisterSoftwareWindow};
    use slint::platform::software_renderer::RepaintBufferType;
    use slint::{ModelRc, VecModel};
    use std::cell::Cell;
    use std::time::Duration;

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

    #[test]
    fn transition_focus_denies_actions_and_navigation_uses_the_ui_source() {
        let app = launcher();
        let adapter = LauncherUiActionsAdapter::install(&app);
        let actions = app.global::<slint_ui::launcher::LauncherActions>();
        actions.invoke_activate();
        assert_eq!(adapter.pop_routable(false), None);
        assert_eq!(adapter.rejected_for_test(), 1);

        actions.invoke_navigate(slint_ui::launcher::NavigationDirection::Left);
        let action = adapter.pop_routable(true).expect("routable UI action");
        let event = action.input_event(7, 11).expect("navigation input event");
        assert_eq!(event.source.kind, InputSourceKind::Ui);
        assert_eq!(event.sequence, 7);
        assert_eq!(event.captured_at_us, 11);
        assert_eq!(event.action, LogicalAction::Left);
        assert_eq!(event.phase, InputPhase::Pressed);
    }
}
