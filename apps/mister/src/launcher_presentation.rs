// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable projection of launcher navigation state into the compiled Slint UI.
//!
//! Device lifecycle, controller discovery, preview loading, and scanout remain
//! in `ui_runner`; this presenter is deliberately shared with the macOS host.

use crate::arcade_catalog::{ArcadeCatalog, ArcadeGameView};
use crate::launcher::{
    ArcadeSearchPane, ArcadeSearchStatus, CatalogMenuItemStatus, LauncherNav, Screen,
};
use crate::launcher_taxonomy::LauncherMenuItemKind;
use crate::launcher_view_types::{
    home_focus, home_scroll_phase, launcher_screen, system_hub_section,
};
use mister_magik_ui::launcher::{
    FeedbackView, Launcher, MenuItem, MenuItemKind, MenuItemPresentation, MenuItemStatus,
    MisterBridge, NavigationView,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const SELECTION_FEEDBACK_MIN_VISIBLE: Duration = Duration::from_millis(80);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionFeedbackTarget {
    pub surface: String,
    pub item: String,
}

impl SelectionFeedbackTarget {
    pub fn new(surface: impl Into<String>, item: impl Into<String>) -> Self {
        Self {
            surface: surface.into(),
            item: item.into(),
        }
    }

    pub fn home(nav: &LauncherNav) -> Option<Self> {
        (nav.screen == Screen::Home)
            .then(|| Self {
                surface: nav.current_menu_id().to_string(),
                item: if nav.settings_focused {
                    "__settings".to_string()
                } else {
                    nav.current_menu_selected_item_id().to_string()
                },
            })
            .filter(|target| !target.item.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionFeedbackStamp {
    pub revision: u64,
    pub entries: Vec<SelectionFeedbackEntryStamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionFeedbackEntryStamp {
    pub event_id: u64,
    pub target: SelectionFeedbackTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionFeedbackConfirmation {
    Visible {
        event_id: u64,
        target: SelectionFeedbackTarget,
        confirmed_at: Instant,
    },
    Hidden {
        event_id: u64,
        target: SelectionFeedbackTarget,
        visible_for: Duration,
        confirmed_at: Instant,
    },
    Cancelled {
        event_id: u64,
        target: SelectionFeedbackTarget,
        confirmed_at: Instant,
    },
}

#[derive(Clone, Debug)]
struct ActiveSelectionFeedback {
    event_id: u64,
    target: SelectionFeedbackTarget,
    visible_since: Option<Instant>,
}

#[derive(Clone, Debug)]
struct PendingSelectionFeedbackRemoval {
    event_id: u64,
    target: SelectionFeedbackTarget,
    visible_since: Option<Instant>,
    requested_revision: u64,
}

#[derive(Default)]
struct SelectionFeedback {
    revision: u64,
    next_event_id: u64,
    surface: Option<String>,
    active: Vec<ActiveSelectionFeedback>,
    pending_removals: Vec<PendingSelectionFeedbackRemoval>,
}

impl SelectionFeedback {
    fn sync_surface(&mut self, target: Option<&SelectionFeedbackTarget>) -> bool {
        let surface = target.map(|target| target.surface.as_str());
        if self.surface.as_deref() == surface {
            return false;
        }
        let changed = self.retire_active();
        self.surface = surface.map(str::to_string);
        changed
    }

    fn register(&mut self, target: SelectionFeedbackTarget) -> bool {
        if self.surface.as_deref() != Some(target.surface.as_str()) {
            self.sync_surface(Some(&target));
        }
        self.next_event_id = self.next_event_id.wrapping_add(1).max(1);
        let event_id = self.next_event_id;
        let replaced = self
            .active
            .iter()
            .position(|entry| entry.target == target)
            .map(|index| self.active.remove(index));
        self.active.push(ActiveSelectionFeedback {
            event_id,
            target,
            visible_since: None,
        });
        self.bump_revision();
        if let Some(replaced) = replaced {
            self.pending_removals.push(PendingSelectionFeedbackRemoval {
                event_id: replaced.event_id,
                target: replaced.target,
                visible_since: replaced.visible_since,
                requested_revision: self.revision,
            });
        }
        true
    }

    fn retire_active(&mut self) -> bool {
        if self.active.is_empty() {
            return false;
        }
        self.bump_revision();
        let requested_revision = self.revision;
        self.pending_removals
            .extend(
                self.active
                    .drain(..)
                    .map(|entry| PendingSelectionFeedbackRemoval {
                        event_id: entry.event_id,
                        target: entry.target,
                        visible_since: entry.visible_since,
                        requested_revision,
                    }),
            );
        true
    }

    fn expire_due(&mut self, now: Instant) -> bool {
        let mut expired = Vec::new();
        self.active.retain(|entry| {
            let due = entry.visible_since.is_some_and(|since| {
                now.saturating_duration_since(since) >= SELECTION_FEEDBACK_MIN_VISIBLE
            });
            if due {
                expired.push(entry.clone());
            }
            !due
        });
        if expired.is_empty() {
            return false;
        }
        self.bump_revision();
        self.pending_removals
            .extend(
                expired
                    .into_iter()
                    .map(|entry| PendingSelectionFeedbackRemoval {
                        event_id: entry.event_id,
                        target: entry.target,
                        visible_since: entry.visible_since,
                        requested_revision: self.revision,
                    }),
            );
        true
    }

    fn stamp(&self) -> SelectionFeedbackStamp {
        SelectionFeedbackStamp {
            revision: self.revision,
            entries: self
                .active
                .iter()
                .map(|entry| SelectionFeedbackEntryStamp {
                    event_id: entry.event_id,
                    target: entry.target.clone(),
                })
                .collect(),
        }
    }

    fn confirm(
        &mut self,
        stamp: &SelectionFeedbackStamp,
        confirmed_at: Instant,
    ) -> Vec<SelectionFeedbackConfirmation> {
        let mut confirmations = Vec::new();
        for stamped in &stamp.entries {
            if let Some(active) = self
                .active
                .iter_mut()
                .find(|entry| entry.event_id == stamped.event_id && entry.target == stamped.target)
                && active.visible_since.is_none()
            {
                active.visible_since = Some(confirmed_at);
                confirmations.push(SelectionFeedbackConfirmation::Visible {
                    event_id: active.event_id,
                    target: active.target.clone(),
                    confirmed_at,
                });
            } else if let Some(pending) = self.pending_removals.iter_mut().find(|entry| {
                entry.event_id == stamped.event_id
                    && entry.target == stamped.target
                    && entry.requested_revision > stamp.revision
                    && entry.visible_since.is_none()
            }) {
                pending.visible_since = Some(confirmed_at);
                confirmations.push(SelectionFeedbackConfirmation::Visible {
                    event_id: pending.event_id,
                    target: pending.target.clone(),
                    confirmed_at,
                });
            }
        }
        self.pending_removals.retain(|entry| {
            let removed = entry.requested_revision <= stamp.revision
                && !stamp.entries.iter().any(|stamped| {
                    stamped.event_id == entry.event_id && stamped.target == entry.target
                });
            if removed {
                if let Some(visible_since) = entry.visible_since {
                    confirmations.push(SelectionFeedbackConfirmation::Hidden {
                        event_id: entry.event_id,
                        target: entry.target.clone(),
                        visible_for: confirmed_at.saturating_duration_since(visible_since),
                        confirmed_at,
                    });
                } else {
                    confirmations.push(SelectionFeedbackConfirmation::Cancelled {
                        event_id: entry.event_id,
                        target: entry.target.clone(),
                        confirmed_at,
                    });
                }
            }
            !removed
        });
        confirmations
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }
}

macro_rules! set_if_changed {
    ($bridge:expr, $getter:ident, $setter:ident, $value:expr) => {{
        let value = $value;
        if $bridge.$getter() != value {
            $bridge.$setter(value);
        }
    }};
}

macro_rules! set_string_if_changed {
    ($bridge:expr, $getter:ident, $setter:ident, $value:expr) => {{
        let source = $value;
        let source = AsRef::<str>::as_ref(&source);
        if $bridge.$getter().as_str() != source {
            bridge_churn_record_shared_strings(1);
            $bridge.$setter(SharedString::from(source));
        }
    }};
}

macro_rules! set_view_string_if_changed {
    ($view:expr, $getter:ident, $setter:ident, $value:expr) => {{
        let source = $value;
        let source = AsRef::<str>::as_ref(&source);
        if $view.$getter().as_str() != source {
            $view.$setter(SharedString::from(source));
        }
    }};
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct BridgeChurnCounters {
    pub(crate) model_replacements: u64,
    pub(crate) row_mutations: u64,
    pub(crate) row_allocations: u64,
    pub(crate) shared_string_constructions: u64,
    pub(crate) model_allocation_us: u64,
}

impl BridgeChurnCounters {
    #[cfg(feature = "ui")]
    pub(crate) fn saturating_sub(self, earlier: Self) -> Self {
        Self {
            model_replacements: self
                .model_replacements
                .saturating_sub(earlier.model_replacements),
            row_mutations: self.row_mutations.saturating_sub(earlier.row_mutations),
            row_allocations: self.row_allocations.saturating_sub(earlier.row_allocations),
            shared_string_constructions: self
                .shared_string_constructions
                .saturating_sub(earlier.shared_string_constructions),
            model_allocation_us: self
                .model_allocation_us
                .saturating_sub(earlier.model_allocation_us),
        }
    }
}

thread_local! {
    static BRIDGE_CHURN_ENABLED: Cell<bool> = const { Cell::new(false) };
    static BRIDGE_CHURN_COUNTERS: RefCell<BridgeChurnCounters> = const {
        RefCell::new(BridgeChurnCounters {
            model_replacements: 0,
            row_mutations: 0,
            row_allocations: 0,
            shared_string_constructions: 0,
            model_allocation_us: 0,
        })
    };
}

#[cfg(feature = "ui")]
pub(crate) fn bridge_churn_begin() {
    BRIDGE_CHURN_COUNTERS.with(|counters| *counters.borrow_mut() = BridgeChurnCounters::default());
    BRIDGE_CHURN_ENABLED.with(|enabled| enabled.set(true));
}

#[cfg(feature = "ui")]
pub(crate) fn bridge_churn_end() -> BridgeChurnCounters {
    BRIDGE_CHURN_ENABLED.with(|enabled| enabled.set(false));
    bridge_churn_snapshot()
}

#[cfg(feature = "ui")]
pub(crate) fn bridge_churn_snapshot() -> BridgeChurnCounters {
    BRIDGE_CHURN_COUNTERS.with(|counters| *counters.borrow())
}

pub(crate) fn bridge_churn_record_model_replacements(count: u64) {
    bridge_churn_record(|counters| {
        counters.model_replacements = counters.model_replacements.saturating_add(count);
    });
}

pub(crate) fn bridge_churn_record_row_mutations(count: u64) {
    bridge_churn_record(|counters| {
        counters.row_mutations = counters.row_mutations.saturating_add(count);
    });
}

pub(crate) fn bridge_churn_record_row_allocations(count: u64) {
    bridge_churn_record(|counters| {
        counters.row_allocations = counters.row_allocations.saturating_add(count);
    });
}

pub(crate) fn bridge_churn_record_shared_strings(count: u64) {
    bridge_churn_record(|counters| {
        counters.shared_string_constructions =
            counters.shared_string_constructions.saturating_add(count);
    });
}

pub(crate) fn bridge_churn_record_model_allocation_us(elapsed_us: u128) {
    bridge_churn_record(|counters| {
        counters.model_allocation_us = counters
            .model_allocation_us
            .saturating_add(elapsed_us.min(u128::from(u64::MAX)) as u64);
    });
}

fn bridge_churn_record(update: impl FnOnce(&mut BridgeChurnCounters)) {
    BRIDGE_CHURN_ENABLED.with(|enabled| {
        if enabled.get() {
            BRIDGE_CHURN_COUNTERS.with(|counters| update(&mut counters.borrow_mut()));
        }
    });
}

#[derive(Default)]
pub struct LauncherBridgePresenter {
    menu_items_key: Option<(usize, String)>,
    menu_items: Option<Rc<VecModel<MenuItem>>>,
    menu_item_presentation: Option<Rc<VecModel<MenuItemPresentation>>>,
    projected_selected_index: Option<usize>,
    license_lines_index: Option<usize>,
    license_lines: Option<Rc<VecModel<SharedString>>>,
    selection_feedback: SelectionFeedback,
    projected_selection_feedback: SelectionFeedbackStamp,
    published_selection_feedback: Rc<RefCell<SelectionFeedbackStamp>>,
    selection_feedback_callback_installed: bool,
}

impl LauncherBridgePresenter {
    pub fn sync(
        &mut self,
        app: &Launcher,
        nav: &LauncherNav,
        catalog: &ArcadeCatalog,
        catalog_version: Option<usize>,
        defer_arcade_overlay: bool,
    ) {
        let bridge = app.global::<MisterBridge>();
        let navigation = app.global::<NavigationView>();
        set_if_changed!(
            navigation,
            get_screen,
            set_screen,
            launcher_screen(nav.screen)
        );
        set_if_changed!(
            navigation,
            get_home_selected_index,
            set_home_selected_index,
            nav.selected as i32
        );
        set_if_changed!(
            navigation,
            get_home_focus,
            set_home_focus,
            home_focus(nav.settings_focused)
        );
        set_if_changed!(
            navigation,
            get_home_scroll_phase,
            set_home_scroll_phase,
            home_scroll_phase(
                nav.home_horizontal_held(),
                nav.home_horizontal_repeat_active()
            )
        );
        set_if_changed!(
            navigation,
            get_home_scroll_x,
            set_home_scroll_x,
            nav.scroll_x
        );
        set_view_string_if_changed!(
            navigation,
            get_menu_title,
            set_menu_title,
            nav.current_menu_title()
        );
        set_view_string_if_changed!(
            navigation,
            get_menu_breadcrumb,
            set_menu_breadcrumb,
            nav.current_menu_breadcrumb()
        );
        set_if_changed!(
            navigation,
            get_system_hub_section,
            set_system_hub_section,
            system_hub_section(nav.system_hub_selected)
        );
        set_if_changed!(
            navigation,
            get_system_hub_games_count,
            set_system_hub_games_count,
            catalog.system_game_count("snes") as i32
        );
        set_if_changed!(
            navigation,
            get_system_hub_recent_count,
            set_system_hub_recent_count,
            nav.recent_count() as i32
        );
        set_if_changed!(
            navigation,
            get_system_hub_favourites_count,
            set_system_hub_favourites_count,
            nav.favourite_count() as i32
        );
        set_if_changed!(
            bridge,
            get_screen_mode,
            set_screen_mode,
            screen_mode(nav.screen)
        );
        set_if_changed!(
            bridge,
            get_selected_index,
            set_selected_index,
            nav.selected as i32
        );
        set_if_changed!(
            bridge,
            get_home_scroll_held,
            set_home_scroll_held,
            nav.home_horizontal_held()
        );
        set_if_changed!(
            bridge,
            get_home_scroll_repeat_active,
            set_home_scroll_repeat_active,
            nav.home_horizontal_repeat_active()
        );
        set_if_changed!(bridge, get_home_scroll_x, set_home_scroll_x, nav.scroll_x);
        set_string_if_changed!(
            bridge,
            get_menu_title,
            set_menu_title,
            nav.current_menu_title()
        );
        set_string_if_changed!(
            bridge,
            get_menu_breadcrumb,
            set_menu_breadcrumb,
            nav.current_menu_breadcrumb()
        );
        set_if_changed!(
            bridge,
            get_settings_focused,
            set_settings_focused,
            nav.settings_focused
        );
        set_if_changed!(
            bridge,
            get_settings_selected,
            set_settings_selected,
            nav.settings_selected as i32
        );
        set_if_changed!(
            bridge,
            get_about_selected,
            set_about_selected,
            nav.about_selected as i32
        );
        set_if_changed!(
            bridge,
            get_display_combo_open,
            set_display_combo_open,
            nav.display_combo_open
        );
        set_if_changed!(
            bridge,
            get_display_selected,
            set_display_selected,
            nav.display_selected as i32
        );
        set_if_changed!(
            bridge,
            get_display_highlighted,
            set_display_highlighted,
            nav.display_highlighted as i32
        );
        set_if_changed!(
            bridge,
            get_display_confirm_remaining,
            set_display_confirm_remaining,
            nav.display_confirm_remaining as i32
        );
        set_if_changed!(
            bridge,
            get_simple_joystick_handling,
            set_simple_joystick_handling,
            nav.settings.simple_joystick_handling
        );
        set_if_changed!(
            bridge,
            get_reduce_motion,
            set_reduce_motion,
            nav.settings.reduce_motion
        );
        set_if_changed!(
            bridge,
            get_screensaver_settings_selected,
            set_screensaver_settings_selected,
            nav.screensaver_selected as i32
        );
        set_if_changed!(
            bridge,
            get_screensaver_enabled,
            set_screensaver_enabled,
            nav.settings.screensaver_enabled
        );
        set_if_changed!(
            bridge,
            get_screensaver_delay_minutes,
            set_screensaver_delay_minutes,
            nav.settings.screensaver_delay_minutes as i32
        );
        set_if_changed!(
            bridge,
            get_licenses_selected,
            set_licenses_selected,
            nav.licenses_selected as i32
        );
        set_if_changed!(
            bridge,
            get_licenses_expanded,
            set_licenses_expanded,
            nav.licenses_expanded
        );
        set_if_changed!(
            bridge,
            get_licenses_scroll_y,
            set_licenses_scroll_y,
            nav.licenses_scroll_y()
        );
        if self.license_lines_index != Some(nav.licenses_selected) {
            bridge.set_license_lines(self.license_lines(nav.licenses_selected));
        }

        if let Some(catalog_version) = catalog_version {
            let key = (catalog_version, nav.current_menu_id().to_string());
            if self.menu_items_key.as_ref() != Some(&key) {
                let menu_items = self.menu_items(nav, catalog_version);
                let menu_item_presentation = self.menu_item_presentation();
                bridge_churn_record_model_replacements(2);
                navigation.set_menu_item_presentation(menu_item_presentation.clone());
                navigation.set_menu_items(menu_items.clone());
                bridge.set_menu_item_presentation(menu_item_presentation);
                bridge.set_menu_items(menu_items);
            }
        }
        self.sync_menu_item_state(nav);
        self.publish_selection_feedback(&bridge, &app.global::<FeedbackView>());

        let games = active_game_view(catalog, nav);
        let (title, count) = active_header(catalog, nav, games.len());
        set_string_if_changed!(
            bridge,
            get_active_system_title,
            set_active_system_title,
            title
        );
        set_if_changed!(
            bridge,
            get_active_system_count,
            set_active_system_count,
            count as i32
        );
        set_if_changed!(
            bridge,
            get_arcade_games_loading,
            set_arcade_games_loading,
            active_games_loading(catalog, nav)
        );
        if !(defer_arcade_overlay && nav.screen == Screen::Arcade) {
            set_if_changed!(
                bridge,
                get_arcade_selected,
                set_arcade_selected,
                nav.arcade.selected as i32
            );
        }
        sync_search(&bridge, nav);
    }

    pub fn menu_items(&mut self, nav: &LauncherNav, catalog_version: usize) -> ModelRc<MenuItem> {
        let key = (catalog_version, nav.current_menu_id().to_string());
        if self.menu_items_key.as_ref() != Some(&key) {
            let feedback = self.selection_feedback.stamp();
            self.menu_items = Some(build_menu_items(nav));
            self.menu_item_presentation = Some(build_menu_item_presentation(nav, &feedback));
            self.menu_items_key = Some(key);
            self.projected_selected_index = Some(nav.selected);
            self.projected_selection_feedback = feedback;
        }
        ModelRc::from(
            self.menu_items
                .as_ref()
                .expect("launcher menu model initialized")
                .clone(),
        )
    }

    pub fn menu_item_presentation(&self) -> ModelRc<MenuItemPresentation> {
        ModelRc::from(
            self.menu_item_presentation
                .as_ref()
                .expect("launcher menu presentation initialized")
                .clone(),
        )
    }

    #[cfg(feature = "ui")]
    pub(crate) fn republish_cached_menu_models(&self, app: &Launcher) {
        let (Some(items), Some(presentation)) = (
            self.menu_items.as_ref(),
            self.menu_item_presentation.as_ref(),
        ) else {
            return;
        };
        let bridge = app.global::<MisterBridge>();
        bridge_churn_record_model_replacements(2);
        bridge.set_menu_items(ModelRc::from(items.clone()));
        bridge.set_menu_item_presentation(ModelRc::from(presentation.clone()));
    }

    pub fn license_lines(&mut self, index: usize) -> ModelRc<SharedString> {
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

    pub fn sync_selection_feedback_surface(
        &mut self,
        target: Option<&SelectionFeedbackTarget>,
    ) -> bool {
        self.selection_feedback.sync_surface(target)
    }

    pub fn note_selection_feedback_change(
        &mut self,
        before: Option<&SelectionFeedbackTarget>,
        after: Option<&SelectionFeedbackTarget>,
    ) -> bool {
        let Some(after) = after else {
            return false;
        };
        if before.is_some_and(|before| before.surface == after.surface && before.item != after.item)
        {
            self.selection_feedback.register(after.clone())
        } else {
            false
        }
    }

    pub fn expire_selection_feedback(&mut self, now: Instant) -> bool {
        self.selection_feedback.expire_due(now)
    }

    pub fn selection_feedback_stamp(&self) -> SelectionFeedbackStamp {
        self.projected_selection_feedback.clone()
    }

    pub fn confirm_selection_feedback(
        &mut self,
        stamp: &SelectionFeedbackStamp,
        confirmed_at: Instant,
    ) -> Vec<SelectionFeedbackConfirmation> {
        self.selection_feedback.confirm(stamp, confirmed_at)
    }

    fn sync_menu_item_state(&mut self, nav: &LauncherNav) {
        let stamp = self.selection_feedback.stamp();
        if self.projected_selected_index == Some(nav.selected)
            && self.projected_selection_feedback == stamp
        {
            return;
        }
        if let Some(model) = self.menu_item_presentation.as_ref() {
            if self.projected_selection_feedback == stamp {
                if let Some(previous) = self.projected_selected_index {
                    sync_menu_item_presentation_row(model, nav, &stamp, previous);
                }
                if self.projected_selected_index != Some(nav.selected) {
                    sync_menu_item_presentation_row(model, nav, &stamp, nav.selected);
                }
            } else {
                for index in 0..model.row_count() {
                    sync_menu_item_presentation_row(model, nav, &stamp, index);
                }
            }
        }
        self.projected_selected_index = Some(nav.selected);
        self.projected_selection_feedback = stamp;
    }

    fn publish_selection_feedback(&mut self, bridge: &MisterBridge, feedback: &FeedbackView) {
        if !self.selection_feedback_callback_installed {
            let published = self.published_selection_feedback.clone();
            bridge.on_selection_feedback_query(move |surface, item, _revision| {
                published.borrow().entries.iter().any(|entry| {
                    (surface.is_empty() || entry.target.surface == surface.as_str())
                        && entry.target.item == item.as_str()
                })
            });
            let published = self.published_selection_feedback.clone();
            feedback.on_acknowledged(move |surface, item, _revision| {
                published.borrow().entries.iter().any(|entry| {
                    entry.target.surface == surface.as_str() && entry.target.item == item.as_str()
                })
            });
            self.selection_feedback_callback_installed = true;
        }
        if *self.published_selection_feedback.borrow() != self.projected_selection_feedback {
            *self.published_selection_feedback.borrow_mut() =
                self.projected_selection_feedback.clone();
            bridge
                .set_selection_feedback_revision(self.projected_selection_feedback.revision as i32);
            feedback.set_revision(self.projected_selection_feedback.revision as i32);
        }
    }
}

fn sync_menu_item_presentation_row(
    model: &VecModel<MenuItemPresentation>,
    nav: &LauncherNav,
    stamp: &SelectionFeedbackStamp,
    index: usize,
) {
    let Some(mut row) = model.row_data(index) else {
        return;
    };
    let selected = index == nav.selected;
    let item_id = nav
        .current_menu_items()
        .get(index)
        .map(|item| item.id.as_str())
        .unwrap_or_default();
    let acknowledged = stamp
        .entries
        .iter()
        .any(|entry| entry.target.surface == nav.current_menu_id() && entry.target.item == item_id);
    if row.selected != selected || row.acknowledged != acknowledged {
        row.selected = selected;
        row.acknowledged = acknowledged;
        bridge_churn_record_row_mutations(1);
        model.set_row_data(index, row);
    }
}

pub fn screen_mode(screen: Screen) -> i32 {
    match screen {
        Screen::Home => 0,
        Screen::SystemHub => 8,
        Screen::Controller => 1,
        Screen::Arcade => 2,
        Screen::Settings => 3,
        Screen::About => 4,
        Screen::Licenses => 5,
        Screen::Info => 6,
        Screen::Screensaver => 7,
    }
}

fn build_menu_items(nav: &LauncherNav) -> Rc<VecModel<MenuItem>> {
    let allocation_started = Instant::now();
    let rows = nav
        .current_menu_items()
        .iter()
        .map(|item| {
            let presentation = nav.menu_item_catalog_presentation(item);
            MenuItem {
                id: item.id.clone().into(),
                label: item.title.clone().into(),
                subtitle: match presentation.status {
                    CatalogMenuItemStatus::Scanning => match item.kind {
                        LauncherMenuItemKind::Menu => {
                            let systems = nav.menu_discovered_system_count(&item.id);
                            format!(
                                "{systems} system{} found",
                                if systems == 1 { "" } else { "s" }
                            )
                            .into()
                        }
                        LauncherMenuItemKind::Collection => {
                            if presentation.available {
                                format!("{} games available", item.count).into()
                            } else {
                                "".into()
                            }
                        }
                    },
                    CatalogMenuItemStatus::Partial => "Some items failed".into(),
                    CatalogMenuItemStatus::UpdateFailed if presentation.available => {
                        format!("Update failed • {} games", item.count).into()
                    }
                    CatalogMenuItemStatus::UpdateFailed => "Update failed".into(),
                    CatalogMenuItemStatus::LoadFailed => "Load failed — A to retry".into(),
                    CatalogMenuItemStatus::Ready => format!("{} games", item.count).into(),
                },
                available: presentation.available,
                node_kind: match item.kind {
                    LauncherMenuItemKind::Menu => MenuItemKind::Group,
                    LauncherMenuItemKind::Collection => MenuItemKind::Collection,
                },
                status: match presentation.status {
                    CatalogMenuItemStatus::Ready => MenuItemStatus::Ready,
                    CatalogMenuItemStatus::Scanning => MenuItemStatus::Scanning,
                    CatalogMenuItemStatus::Partial => MenuItemStatus::Partial,
                    CatalogMenuItemStatus::UpdateFailed if presentation.available => {
                        MenuItemStatus::UpdateFailed
                    }
                    CatalogMenuItemStatus::UpdateFailed | CatalogMenuItemStatus::LoadFailed => {
                        MenuItemStatus::Failed
                    }
                },
            }
        })
        .collect::<Vec<_>>();
    bridge_churn_record_row_allocations(rows.len() as u64);
    bridge_churn_record_shared_strings(rows.len().saturating_mul(3) as u64);
    bridge_churn_record_model_allocation_us(allocation_started.elapsed().as_micros());
    Rc::new(VecModel::from(rows))
}

fn build_menu_item_presentation(
    nav: &LauncherNav,
    feedback: &SelectionFeedbackStamp,
) -> Rc<VecModel<MenuItemPresentation>> {
    let allocation_started = Instant::now();
    let rows = nav
        .current_menu_items()
        .iter()
        .enumerate()
        .map(|(index, item)| MenuItemPresentation {
            selected: index == nav.selected,
            acknowledged: feedback.entries.iter().any(|entry| {
                entry.target.surface == nav.current_menu_id() && entry.target.item == item.id
            }),
        })
        .collect::<Vec<_>>();
    bridge_churn_record_row_allocations(rows.len() as u64);
    bridge_churn_record_model_allocation_us(allocation_started.elapsed().as_micros());
    Rc::new(VecModel::from(rows))
}

fn active_game_view<'a>(catalog: &'a ArcadeCatalog, nav: &'a LauncherNav) -> ArcadeGameView<'a> {
    nav.active_collection()
        .map(|collection| nav.active_arcade_game_view(catalog, &collection.id))
        .unwrap_or_else(ArcadeGameView::empty)
}

fn active_header(
    catalog: &ArcadeCatalog,
    nav: &LauncherNav,
    fallback_count: usize,
) -> (String, usize) {
    let Some(collection) = nav.active_collection() else {
        return ("Games".to_string(), fallback_count);
    };
    let filter_label = nav.arcade_filter.active_label();
    let title = if filter_label == "Games A-Z" {
        collection.title.clone()
    } else {
        format!("{} - {filter_label}", collection.title)
    };
    let hydrated_count = nav.active_arcade_game_count(catalog, &collection.id);
    let count = if hydrated_count == 0 && collection.count > 0 {
        collection.count
    } else {
        hydrated_count
    };
    (title, count)
}

fn active_games_loading(catalog: &ArcadeCatalog, nav: &LauncherNav) -> bool {
    nav.active_collection().is_some_and(|collection| {
        collection.count > 0 && catalog.system_game_count(&collection.id) < collection.count
    })
}

fn sync_search(bridge: &MisterBridge, nav: &LauncherNav) {
    set_if_changed!(
        bridge,
        get_arcade_search_active,
        set_arcade_search_active,
        nav.arcade_search.is_active(&nav.arcade_filter.active)
    );
    set_string_if_changed!(
        bridge,
        get_arcade_search_query,
        set_arcade_search_query,
        nav.arcade_search.query.clone()
    );
    set_string_if_changed!(
        bridge,
        get_arcade_search_suggestion,
        set_arcade_search_suggestion,
        nav.arcade_search.suggestion.clone()
    );
    set_if_changed!(
        bridge,
        get_arcade_search_status,
        set_arcade_search_status,
        match nav.arcade_search.status {
            ArcadeSearchStatus::Idle => 0,
            ArcadeSearchStatus::Searching => 1,
            ArcadeSearchStatus::Ready => 2,
            ArcadeSearchStatus::Failed => 3,
        }
    );
    set_if_changed!(
        bridge,
        get_arcade_search_key_selected,
        set_arcade_search_key_selected,
        nav.arcade_search.selected_key as i32
    );
    set_if_changed!(
        bridge,
        get_arcade_search_pane,
        set_arcade_search_pane,
        match nav.arcade_search.pane {
            ArcadeSearchPane::Keyboard => 0,
            ArcadeSearchPane::Results => 1,
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(item: &str) -> SelectionFeedbackTarget {
        SelectionFeedbackTarget {
            surface: "menu:computers".to_string(),
            item: item.to_string(),
        }
    }

    #[test]
    fn feedback_clock_starts_on_exact_visible_confirmation() {
        let mut feedback = SelectionFeedback::default();
        feedback.register(target("apple-ii"));
        let visible_stamp = feedback.stamp();
        let origin = Instant::now();

        assert!(
            feedback
                .confirm(&SelectionFeedbackStamp::default(), origin)
                .is_empty()
        );
        assert!(!feedback.expire_due(origin + Duration::from_secs(1)));

        let confirmations = feedback.confirm(&visible_stamp, origin);
        assert!(matches!(
            confirmations.as_slice(),
            [SelectionFeedbackConfirmation::Visible { event_id: 1, .. }]
        ));
        assert!(!feedback.expire_due(origin + Duration::from_millis(79)));
        assert!(feedback.expire_due(origin + SELECTION_FEEDBACK_MIN_VISIBLE));

        let removal_stamp = feedback.stamp();
        assert!(
            feedback
                .confirm(&visible_stamp, origin + Duration::from_millis(81))
                .is_empty()
        );
        let confirmations = feedback.confirm(&removal_stamp, origin + Duration::from_millis(83));
        assert!(matches!(
            confirmations.as_slice(),
            [SelectionFeedbackConfirmation::Hidden { visible_for, .. }]
                if *visible_for >= SELECTION_FEEDBACK_MIN_VISIBLE
        ));
    }

    #[test]
    fn feedback_overlaps_and_reentry_rearms_from_confirmation() {
        let mut feedback = SelectionFeedback::default();
        let origin = Instant::now();
        feedback.register(target("apple-ii"));
        let first_stamp = feedback.stamp();
        feedback.confirm(&first_stamp, origin);

        feedback.register(target("commodore"));
        let overlap_stamp = feedback.stamp();
        assert_eq!(overlap_stamp.entries.len(), 2);
        feedback.confirm(&overlap_stamp, origin + Duration::from_millis(50));

        feedback.register(target("apple-ii"));
        let reentry_stamp = feedback.stamp();
        let apple_event = reentry_stamp
            .entries
            .iter()
            .find(|entry| entry.target.item == "apple-ii")
            .expect("re-entered Apple II feedback");
        assert_eq!(apple_event.event_id, 3);
        let confirmations = feedback.confirm(&reentry_stamp, origin + Duration::from_millis(70));
        assert!(matches!(
            confirmations.as_slice(),
            [
                SelectionFeedbackConfirmation::Visible { event_id: 3, .. },
                SelectionFeedbackConfirmation::Hidden { event_id: 1, .. }
            ]
        ));

        assert!(!feedback.expire_due(origin + Duration::from_millis(129)));
        assert!(feedback.expire_due(origin + Duration::from_millis(150)));
        let remaining = feedback.stamp();
        assert!(remaining.entries.is_empty());
    }

    #[test]
    fn replacing_surface_cancels_feedback_proven_never_visible() {
        let mut feedback = SelectionFeedback::default();
        feedback.register(target("apple-ii"));
        assert_eq!(feedback.stamp().entries.len(), 1);

        let other = SelectionFeedbackTarget {
            surface: "menu:consoles".to_string(),
            item: "nintendo".to_string(),
        };
        assert!(feedback.sync_surface(Some(&other)));
        let removal_stamp = feedback.stamp();
        assert!(removal_stamp.entries.is_empty());
        assert!(matches!(
            feedback.confirm(&removal_stamp, Instant::now()).as_slice(),
            [SelectionFeedbackConfirmation::Cancelled { event_id: 1, .. }]
        ));
    }

    #[test]
    fn replacing_surface_retires_physically_visible_feedback() {
        let mut feedback = SelectionFeedback::default();
        let origin = Instant::now();
        feedback.register(target("apple-ii"));
        let visible_stamp = feedback.stamp();
        feedback.confirm(&visible_stamp, origin);

        let other = SelectionFeedbackTarget::new("menu:consoles", "nintendo");
        assert!(feedback.sync_surface(Some(&other)));
        let removal_stamp = feedback.stamp();
        assert!(matches!(
            feedback
                .confirm(&removal_stamp, origin + Duration::from_millis(5))
                .as_slice(),
            [SelectionFeedbackConfirmation::Hidden { event_id: 1, .. }]
        ));
    }

    #[test]
    fn in_flight_visibility_survives_surface_retirement() {
        let mut feedback = SelectionFeedback::default();
        let origin = Instant::now();
        feedback.register(target("apple-ii"));
        let visible_stamp = feedback.stamp();

        let other = SelectionFeedbackTarget::new("menu:consoles", "nintendo");
        assert!(feedback.sync_surface(Some(&other)));
        let removal_stamp = feedback.stamp();
        assert!(matches!(
            feedback.confirm(&visible_stamp, origin).as_slice(),
            [SelectionFeedbackConfirmation::Visible { event_id: 1, .. }]
        ));
        assert!(matches!(
            feedback
                .confirm(&removal_stamp, origin + Duration::from_millis(5))
                .as_slice(),
            [SelectionFeedbackConfirmation::Hidden { event_id: 1, .. }]
        ));
    }

    #[test]
    fn repeated_surface_changes_preserve_pending_retirement() {
        let mut feedback = SelectionFeedback::default();
        let origin = Instant::now();
        feedback.register(target("apple-ii"));
        let visible_stamp = feedback.stamp();
        feedback.confirm(&visible_stamp, origin);

        let consoles = SelectionFeedbackTarget::new("menu:consoles", "nintendo");
        let settings = SelectionFeedbackTarget::new("settings", "audio");
        assert!(feedback.sync_surface(Some(&consoles)));
        assert!(!feedback.sync_surface(Some(&settings)));
        let removal_stamp = feedback.stamp();
        assert!(matches!(
            feedback
                .confirm(&removal_stamp, origin + Duration::from_millis(5))
                .as_slice(),
            [SelectionFeedbackConfirmation::Hidden { event_id: 1, .. }]
        ));
    }

    #[test]
    fn complete_computers_path_keeps_every_destination_pending() {
        let mut feedback = SelectionFeedback::default();
        let path = [
            "apple-ii",
            "commodore",
            "atari",
            "sinclair",
            "coco2",
            "dos",
            "japanese",
            "other",
        ];
        for item in path {
            feedback.register(target(item));
        }

        let stamp = feedback.stamp();
        assert_eq!(stamp.entries.len(), path.len());
        assert_eq!(
            stamp
                .entries
                .iter()
                .map(|entry| entry.target.item.as_str())
                .collect::<Vec<_>>(),
            path
        );
    }

    #[test]
    fn unchanged_boundaries_and_replaced_surfaces_do_not_register_feedback() {
        let mut presenter = LauncherBridgePresenter::default();
        let apple = target("apple-ii");
        let consoles = SelectionFeedbackTarget::new("menu:consoles", "nintendo");

        assert!(!presenter.note_selection_feedback_change(Some(&apple), Some(&apple)));
        assert!(!presenter.note_selection_feedback_change(None, Some(&apple)));
        assert!(!presenter.note_selection_feedback_change(Some(&apple), None));
        assert!(!presenter.note_selection_feedback_change(Some(&apple), Some(&consoles)));
        assert!(presenter.selection_feedback_stamp().entries.is_empty());
    }
}
