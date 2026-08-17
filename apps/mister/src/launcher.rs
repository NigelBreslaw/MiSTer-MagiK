// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Launcher navigation and arcade game launch.

use crate::arcade_button_overrides::{remove_button_overrides, write_button_overrides_for_mra};
#[cfg(test)]
use crate::arcade_catalog::StructuredLaunchPlan;
use crate::arcade_catalog::{
    ARCADE_ROW_HEIGHT, ArcadeCatalog, ArcadeFilter, ArcadeFilterOption, HOME_LIST_VISIBLE_W,
    HOME_TILE_GAP, HOME_TILE_WIDTH, LaunchTarget,
};
use crate::input_event::{InputEvent, InputPhase};
#[cfg(test)]
use crate::input_repeat::RepeatNav;
use crate::input_state::PadState;
use crate::launcher_taxonomy::{
    LauncherCollection, LauncherMenuItem, LauncherMenuItemKind, LauncherTaxonomy,
    LauncherTaxonomyToken, ROOT_MENU_ID,
};
#[cfg(test)]
use crate::library_db;
use crate::settings::{MagikSettings, ScreenOrientation};
use crate::spring_animation::{SpringAnimation, SpringConfiguration};
use mister_magik_catalog::media_identity::screenshot_reset_deletes_filename;
use mister_magik_core::launcher_effects::{
    DisplayControl, DisplayState as EffectDisplayState, DisplayStateRead,
    DisplayTransactionPhase as EffectDisplayTransactionPhase, InputPolicy, LaunchHandoff,
    LaunchHandoffOutcome, LaunchHandoffRequest, LaunchSelection as EffectLaunchSelection,
    LauncherEffectFailure, LauncherEffectFailureKind, LauncherPersistence, RuntimeState,
    StructuredLaunchSelection as EffectStructuredLaunchSelection,
};
use mister_magik_mister_runtime::display_resolution::{DISPLAY_RESOLUTIONS, DisplayResolution};
use mister_magik_mister_runtime::main_command::{self, MainCommand};
use mister_magik_mister_runtime::runtime_state::SystemRuntimeState;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const HOME_SCROLL_HOLD_DELAY: Duration = Duration::from_millis(200);
const HOME_SCROLL_SPEED_PX_PER_SECOND: f64 = 1440.0;
const HOME_SCROLL_ACCELERATION_PX_PER_SECOND_SQUARED: f64 = 6000.0;

const INPUT_POLICY_MARKER_PATH: &str = "/tmp/mister-magik/input-policy";

fn mister_bin() -> &'static str {
    mister_magik_catalog::device_layout::DeviceLayout::current().main_path()
}

fn magik_input_dir() -> PathBuf {
    mister_magik_catalog::device_layout::current_app_path("input")
}

fn library_rebuild_on_next_boot_path() -> PathBuf {
    mister_magik_catalog::device_layout::current_app_path("rebuild-on-next-boot")
}
#[cfg(test)]
const STATE_FILENAME: &str = mister_magik_catalog::media_identity::SCREENSHOT_MEDIA_STATE_FILENAME;
const ARCADE_NORMAL_PX_PER_SECOND: f64 = 360.0;
const ARCADE_TURBO_PX_PER_SECOND: f64 = 720.0;
const ARCADE_QUICK_TAP_MAX: Duration = Duration::from_millis(220);
const ARCADE_TURBO_REPRESS_WINDOW: Duration = Duration::from_millis(350);
const FIFO_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const MAIN_START_TIMEOUT: Duration = Duration::from_secs(15);
pub const DISPLAY_CONFIRM_SECONDS: u8 = 20;
pub const LAUNCH_RETURN_STATE_PATH: &str = "/tmp/mister-magik/launcher-return-state.json";
const LAUNCH_RETURN_STATE_SCHEMA: u32 = 3;
const SETTINGS_DISPLAY_SELECTED: usize = 0;
const SETTINGS_ORIENTATION_SELECTED: usize = 1;
const SETTINGS_SCREENSAVER_SELECTED: usize = 2;
const SETTINGS_REDUCE_MOTION_SELECTED: usize = 3;
const SETTINGS_EXIT_SELECTED: usize = 4;
const SETTINGS_REBUILD_SELECTED: usize = 5;
const SETTINGS_ABOUT_SELECTED: usize = 6;
const SETTINGS_MAX_SELECTED: usize = SETTINGS_ABOUT_SELECTED;
const ABOUT_MAX_SELECTED: usize = 1;
const SCREENSAVER_SETTINGS_MAX_SELECTED: usize = 2;
const LICENSES_MAX_SELECTED: usize = crate::licenses::LICENSE_TITLES.len() - 1;
const LICENSE_SCROLL_LINE_PX: f64 = 22.0;
pub const ARCADE_SEARCH_KEY_COLUMNS: usize = 8;
const SETTINGS_HIDDEN_DISPLAY_RESOLUTION_IDS: [&str; 2] = ["crt-480p60", "crt-576p50"];

pub fn settings_display_resolutions() -> impl Iterator<Item = &'static DisplayResolution> {
    DISPLAY_RESOLUTIONS
        .iter()
        .filter(|mode| !SETTINGS_HIDDEN_DISPLAY_RESOLUTION_IDS.contains(&mode.id))
}

pub fn settings_display_resolution(index: usize) -> Option<&'static DisplayResolution> {
    settings_display_resolutions().nth(index)
}

pub fn settings_display_resolution_index(id: &str) -> Option<usize> {
    settings_display_resolutions().position(|mode| mode.id == id)
}

pub fn settings_display_selection_index(display_resolution_index: usize) -> Option<usize> {
    DISPLAY_RESOLUTIONS
        .get(display_resolution_index)
        .and_then(|mode| settings_display_resolution_index(mode.id))
}

fn settings_display_resolution_count() -> usize {
    settings_display_resolutions().count()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcadeSearchKeyAction {
    Append(&'static str),
    Space,
    Delete,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArcadeSearchKey {
    pub label: &'static str,
    pub action: ArcadeSearchKeyAction,
}

impl ArcadeSearchKey {
    const fn append(label: &'static str) -> Self {
        Self {
            label,
            action: ArcadeSearchKeyAction::Append(label),
        }
    }
}

pub const ARCADE_SEARCH_KEYS: [ArcadeSearchKey; 43] = [
    ArcadeSearchKey::append("A"),
    ArcadeSearchKey::append("B"),
    ArcadeSearchKey::append("C"),
    ArcadeSearchKey::append("D"),
    ArcadeSearchKey::append("E"),
    ArcadeSearchKey::append("F"),
    ArcadeSearchKey::append("G"),
    ArcadeSearchKey::append("H"),
    ArcadeSearchKey::append("I"),
    ArcadeSearchKey::append("J"),
    ArcadeSearchKey::append("K"),
    ArcadeSearchKey::append("L"),
    ArcadeSearchKey::append("M"),
    ArcadeSearchKey::append("N"),
    ArcadeSearchKey::append("O"),
    ArcadeSearchKey::append("P"),
    ArcadeSearchKey::append("Q"),
    ArcadeSearchKey::append("R"),
    ArcadeSearchKey::append("S"),
    ArcadeSearchKey::append("T"),
    ArcadeSearchKey::append("U"),
    ArcadeSearchKey::append("V"),
    ArcadeSearchKey::append("W"),
    ArcadeSearchKey::append("X"),
    ArcadeSearchKey::append("Y"),
    ArcadeSearchKey::append("Z"),
    ArcadeSearchKey::append("0"),
    ArcadeSearchKey::append("1"),
    ArcadeSearchKey::append("2"),
    ArcadeSearchKey::append("3"),
    ArcadeSearchKey::append("4"),
    ArcadeSearchKey::append("5"),
    ArcadeSearchKey::append("6"),
    ArcadeSearchKey::append("7"),
    ArcadeSearchKey::append("8"),
    ArcadeSearchKey::append("9"),
    ArcadeSearchKey::append("-"),
    ArcadeSearchKey::append("."),
    ArcadeSearchKey::append("'"),
    ArcadeSearchKey::append("&"),
    ArcadeSearchKey {
        label: "SPACE",
        action: ArcadeSearchKeyAction::Space,
    },
    ArcadeSearchKey {
        label: "DEL",
        action: ArcadeSearchKeyAction::Delete,
    },
    ArcadeSearchKey {
        label: "CLEAR",
        action: ArcadeSearchKeyAction::Clear,
    },
];

const LAUNCH_IDLE: u8 = 0;
const LAUNCH_SENT: u8 = 1;
static LAUNCH_STATE: AtomicU8 = AtomicU8::new(LAUNCH_IDLE);

fn arcade_scroll_speed_div() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_ARCADE_SCROLL_SPEED_DIV")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .map(|value| value.clamp(1, 12))
            .unwrap_or(1)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchError {
    kind: LaunchFailureKind,
    message: String,
    spawned_mister: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchFailureKind {
    UnreadablePayload,
    DamagedArchive,
    MissingCore,
    HandoffRejected,
    Internal,
}

impl LaunchError {
    fn new(message: impl Into<String>, spawned_mister: bool) -> Self {
        Self {
            kind: LaunchFailureKind::HandoffRejected,
            message: message.into(),
            spawned_mister,
        }
    }

    #[cfg(feature = "ui")]
    pub fn preparation(error: crate::launch_preparation::LaunchPreparationError) -> Self {
        let kind = match error.kind {
            crate::launch_preparation::LaunchPreparationFailureKind::MissingPayload
            | crate::launch_preparation::LaunchPreparationFailureKind::UnreadablePayload => {
                LaunchFailureKind::UnreadablePayload
            }
            crate::launch_preparation::LaunchPreparationFailureKind::DamagedArchive
            | crate::launch_preparation::LaunchPreparationFailureKind::UnsupportedArchive
            | crate::launch_preparation::LaunchPreparationFailureKind::OversizedArchiveMember => {
                LaunchFailureKind::DamagedArchive
            }
        };
        Self {
            kind,
            message: error.detail,
            spawned_mister: false,
        }
    }

    #[cfg(feature = "ui")]
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: LaunchFailureKind::Internal,
            message: message.into(),
            spawned_mister: false,
        }
    }

    pub fn kind(&self) -> LaunchFailureKind {
        self.kind
    }

    fn with_kind(mut self, kind: LaunchFailureKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn spawned_mister(&self) -> bool {
        self.spawned_mister
    }
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for LaunchError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    SystemHub,
    Controller,
    Arcade,
    Settings,
    Screensaver,
    About,
    Licenses,
    Info,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArcadeUserListMode {
    #[default]
    Games,
    Recent,
    Favourites,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    ExitToMister,
    RebuildDatabase,
    Restart,
    LibraryChanged,
    LibraryUpdateFailed,
    DisplayResolution,
    DisplayResolutionError,
    ScreenOrientation,
    AddFavourite,
    RemoveFavourite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherAction {
    OpenMenu,
    OpenCollection,
    NavigateBack,
    NavigateHome,
    LaunchGame,
    AddFavourite,
    RemoveFavourite,
    ExitToMister,
    RebuildDatabase,
    Restart,
    ContinueWithStaleLibrary,
    RebuildLibrary,
    PreviewScreensaver,
    ApplyDisplayResolution,
    ConfirmDisplayResolution,
    CancelDisplayResolution,
    ApplyScreenOrientation,
    ConfirmScreenOrientation,
    CancelScreenOrientation,
    PersistSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogSystemUpdateState {
    Queued,
    Scanning,
    Prepared,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogSystemHydrationState {
    Loading,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogMenuItemStatus {
    Ready,
    Scanning,
    Partial,
    UpdateFailed,
    LoadFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CatalogMenuItemPresentation {
    pub status: CatalogMenuItemStatus,
    pub available: bool,
    pub retryable: bool,
}

#[derive(Clone)]
pub struct LauncherEvent {
    pub action: LauncherAction,
    pub path: Option<String>,
    pub settings: Option<MagikSettings>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryChangedTestDialogChoice {
    Continue,
    Rebuild,
}

impl LibraryChangedTestDialogChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Rebuild => "rebuild",
        }
    }
}

pub fn parse_library_changed_test_dialog_choice(
    value: &str,
) -> Result<Option<LibraryChangedTestDialogChoice>, String> {
    match value.trim() {
        "" => Ok(None),
        "continue" => Ok(Some(LibraryChangedTestDialogChoice::Continue)),
        "rebuild" => Ok(Some(LibraryChangedTestDialogChoice::Rebuild)),
        other => Err(format!(
            "unknown MISTER_MAGIK_TEST_LIBRARY_CHANGED_DIALOG_CHOICE={other:?}; use continue|rebuild"
        )),
    }
}

#[derive(Clone, Debug)]
pub struct ArcadeNav {
    pub selected: usize,
    pub scroll_y: i32,
    pub visual_index: f32,
    row_height: i32,
    step_rows: usize,
    scroll: ArcadeScrollState,
    scroll_animation: SpringAnimation,
    scroll_velocity_animation: SpringAnimation,
}

#[derive(Clone, Copy, Debug, Default)]
struct ArcadeScrollState {
    target_index: usize,
    intent_queue: i32,
    held_dir: i32,
    hold_started_at: Option<Instant>,
    last_quick_tap_dir: i32,
    last_quick_tap_released_at: Option<Instant>,
    turbo_candidate: bool,
    turbo_active: bool,
    last_frame_at: Option<Instant>,
    settle_direction: i32,
    continuous_active: bool,
}

impl Default for ArcadeNav {
    fn default() -> Self {
        Self::new()
    }
}

impl ArcadeNav {
    pub fn new() -> Self {
        Self::with_row_height(ARCADE_ROW_HEIGHT)
    }

    fn with_row_height(row_height: i32) -> Self {
        Self::with_row_height_and_step(row_height, 1)
    }

    fn with_row_height_and_step(row_height: i32, step_rows: usize) -> Self {
        Self {
            selected: 0,
            scroll_y: 0,
            visual_index: 0.0,
            row_height: row_height.max(1),
            step_rows: step_rows.max(1),
            scroll: ArcadeScrollState::default(),
            scroll_animation: SpringAnimation::new(0.0, SpringConfiguration::smooth()),
            scroll_velocity_animation: SpringAnimation::new(0.0, SpringConfiguration::smooth()),
        }
    }

    pub fn reset(&mut self) {
        self.selected = 0;
        self.scroll_y = 0;
        self.visual_index = 0.0;
        self.scroll = ArcadeScrollState::default();
        self.scroll_animation.snap_to(0.0);
        self.scroll_velocity_animation.snap_to(0.0);
    }

    pub fn snap_to_selected(&mut self) {
        self.scroll.target_index = self.selected;
        self.scroll.intent_queue = 0;
        self.scroll_animation
            .snap_to(self.selected as f64 * self.row_height as f64);
        self.scroll_velocity_animation.snap_to(0.0);
        self.sync_visual_from_px();
    }

    pub fn row_height(&self) -> i32 {
        self.row_height
    }

    pub fn is_settled_at_selected(&self) -> bool {
        self.scroll_y == self.selected as i32 * self.row_height
            && (self.visual_index - self.selected as f32).abs() < 0.001
            && !self.is_scroll_active()
    }

    pub fn restore_position(&mut self, selected: usize, scroll_y: i32, count: usize) {
        if count == 0 {
            self.reset();
            return;
        }
        self.selected = selected.min(count - 1);
        self.scroll = ArcadeScrollState::default();
        self.scroll.target_index = self.selected;
        let selected_px = self.selected as i32 * self.row_height;
        let saved_scroll_y = scroll_y.clamp(0, self.max_scroll_y(count));
        let visual_px = if saved_scroll_y == selected_px {
            saved_scroll_y
        } else {
            selected_px
        };
        self.scroll_animation.snap_to(visual_px as f64);
        self.scroll_velocity_animation.snap_to(0.0);
        self.sync_visual_from_px();
    }

    fn max_scroll_y(&self, count: usize) -> i32 {
        count.saturating_sub(1) as i32 * self.row_height
    }

    pub fn handle_direction_input(
        &mut self,
        dir: i32,
        previous_dir: i32,
        now: Instant,
        count: usize,
    ) {
        if count == 0 {
            self.reset();
            return;
        }
        if self.selected >= count {
            self.selected = count - 1;
            self.snap_to_selected();
            return;
        }

        let dir = dir.signum();
        let previous_dir = previous_dir.signum();
        if previous_dir != 0 && previous_dir != dir {
            self.record_release(previous_dir, now, count);
        }
        if dir == 0 {
            self.scroll.held_dir = 0;
            self.scroll.hold_started_at = None;
            self.scroll.turbo_active = false;
            return;
        }
        if previous_dir != dir {
            self.begin_press(dir, now);
            self.enqueue_step(dir, count);
            return;
        }

        self.update_turbo(now);

        if self.scroll.hold_started_at.is_some() && self.is_settled() {
            self.enqueue_step(dir, count);
        }
    }

    pub fn tick(&mut self, count: usize, now: Instant) {
        if count == 0 {
            self.reset();
            return;
        }
        if self.selected >= count {
            self.selected = count - 1;
            self.snap_to_selected();
            return;
        }
        let delta = self
            .scroll
            .last_frame_at
            .map_or(Duration::from_secs_f64(1.0 / 60.0), |previous| {
                now.saturating_duration_since(previous)
            });
        self.scroll.last_frame_at = Some(now);

        let before_row = (self.scroll_animation.value() / self.row_height as f64).floor() as i32;
        let continuous_active = self.scroll.held_dir != 0
            && self.scroll.hold_started_at.is_some_and(|started| {
                now.saturating_duration_since(started) > ARCADE_QUICK_TAP_MAX
            });
        if continuous_active {
            let target_speed = if self.scroll.turbo_active {
                ARCADE_TURBO_PX_PER_SECOND
            } else {
                ARCADE_NORMAL_PX_PER_SECOND
            } / arcade_scroll_speed_div() as f64;
            if !self.scroll.continuous_active {
                self.scroll_velocity_animation
                    .snap_to(self.scroll_animation.velocity());
            }
            self.scroll_velocity_animation
                .set_target(self.scroll.held_dir as f64 * target_speed);
            let velocity = self.scroll_velocity_animation.advance(delta);
            let value = (self.scroll_animation.value() + velocity * delta.as_secs_f64())
                .clamp(0.0, self.max_scroll_y(count) as f64);
            self.scroll_animation.set_state(value, velocity);
            self.scroll.continuous_active = true;

            let row = if self.scroll.held_dir > 0 {
                (value / self.row_height as f64).ceil()
            } else {
                (value / self.row_height as f64).floor()
            }
            .clamp(0.0, count.saturating_sub(1) as f64) as usize;
            self.scroll.target_index = row;
            self.selected = row;
            self.scroll.settle_direction = self.scroll.held_dir;
            self.scroll_animation
                .set_target(row as f64 * self.row_height as f64);
        } else {
            self.scroll.continuous_active = false;
            self.scroll_animation.advance(delta);
            clamp_arcade_spring_at_target(&mut self.scroll_animation, self.scroll.settle_direction);
        }
        let after_row = (self.scroll_animation.value() / self.row_height as f64).floor() as i32;
        if after_row != before_row && self.scroll.intent_queue != 0 {
            self.scroll.intent_queue -= self.scroll.intent_queue.signum();
        }
        if self.is_settled() {
            self.scroll.intent_queue = 0;
        }
        self.sync_visual_from_px();
    }

    pub fn bench_direction_tick(
        &mut self,
        dir: i32,
        previous_dir: i32,
        count: usize,
        now: Instant,
    ) {
        self.handle_direction_input(dir, previous_dir, now, count);
        self.tick(count, now);
    }

    // Benchmark-only turbo motion: keep stressing preview scroll by bouncing at
    // list edges instead of parking after the selection clamps.
    pub fn bench_turbo_bounce_tick(&mut self, count: usize, now: Instant) {
        if count == 0 {
            self.reset();
            return;
        }
        if count == 1 {
            self.scroll.held_dir = 0;
            self.scroll.turbo_candidate = false;
            self.scroll.turbo_active = false;
            self.tick(count, now);
            return;
        }
        if self.selected >= count {
            self.selected = count - 1;
            self.snap_to_selected();
            return;
        }

        let current_dir = self.scroll.held_dir.signum();
        let mut dir = if current_dir == 0 { 1 } else { current_dir };
        if self.scroll.target_index + 1 >= count && dir > 0 {
            dir = -1;
        } else if self.scroll.target_index == 0 && dir < 0 {
            dir = 1;
        }

        if current_dir != dir {
            self.scroll.held_dir = dir;
            self.scroll.hold_started_at = Some(now);
            self.scroll.intent_queue = 0;
        }
        self.scroll.turbo_candidate = false;
        self.scroll.turbo_active = true;

        if self.is_settled() {
            self.enqueue_step(dir, count);
        }
        self.tick(count, now);
    }

    fn begin_press(&mut self, dir: i32, now: Instant) {
        self.scroll.held_dir = dir;
        self.scroll.hold_started_at = Some(now);
        self.scroll.turbo_candidate = self.scroll.last_quick_tap_dir == dir
            && self
                .scroll
                .last_quick_tap_released_at
                .is_some_and(|released| {
                    now.saturating_duration_since(released) <= ARCADE_TURBO_REPRESS_WINDOW
                });
        self.scroll.turbo_active = false;
        if self.scroll.last_quick_tap_dir != dir {
            self.scroll.last_quick_tap_released_at = None;
        }
    }

    fn update_turbo(&mut self, now: Instant) {
        if self.scroll.turbo_active || !self.scroll.turbo_candidate {
            return;
        }
        if self
            .scroll
            .hold_started_at
            .is_some_and(|started| now.saturating_duration_since(started) > ARCADE_QUICK_TAP_MAX)
        {
            self.scroll.turbo_active = true;
        }
    }

    fn record_release(&mut self, dir: i32, now: Instant, count: usize) {
        if let Some(started) = self.scroll.hold_started_at
            && self.scroll.held_dir == dir
        {
            if now.saturating_duration_since(started) <= ARCADE_QUICK_TAP_MAX {
                self.scroll.last_quick_tap_dir = dir;
                self.scroll.last_quick_tap_released_at = Some(now);
            } else {
                self.scroll.last_quick_tap_dir = 0;
                self.scroll.last_quick_tap_released_at = None;
            }
        }
        self.scroll.held_dir = 0;
        self.scroll.hold_started_at = None;
        self.scroll.turbo_candidate = false;
        self.scroll.turbo_active = false;
        if self.scroll.continuous_active {
            let target = directional_row_spring_target(
                self.scroll_animation.value(),
                self.scroll_animation.velocity(),
                count,
                dir,
                self.row_height,
                self.scroll_animation.configuration().angular_frequency(),
            );
            let target_index = (target / self.row_height as f64).round() as usize;
            self.scroll.target_index = target_index;
            self.selected = target_index;
            self.scroll.settle_direction = dir;
            self.scroll_animation.set_target(target);
            self.scroll_velocity_animation
                .snap_to(self.scroll_animation.velocity());
            self.scroll.continuous_active = false;
        }
    }

    pub fn is_settled(&self) -> bool {
        self.scroll_animation.is_settled()
    }

    pub fn is_scroll_active(&self) -> bool {
        !self.is_settled() || self.scroll.held_dir != 0 || self.scroll.intent_queue != 0
    }

    pub fn is_turbo_active(&self) -> bool {
        self.scroll.turbo_active
    }

    pub fn has_scroll_motion_or_queue(&self) -> bool {
        !self.is_settled() || self.scroll.intent_queue != 0
    }

    fn enqueue_step(&mut self, dir: i32, count: usize) {
        if count == 0 || dir == 0 {
            return;
        }
        let next = if dir > 0 {
            self.scroll
                .target_index
                .saturating_add(self.step_rows)
                .min(count - 1)
        } else {
            self.scroll.target_index.saturating_sub(self.step_rows)
        };
        if next == self.scroll.target_index {
            return;
        }
        self.scroll.target_index = next;
        self.selected = next;
        self.scroll.intent_queue += dir.signum();
        self.scroll.settle_direction = dir.signum();
        self.scroll_animation
            .set_target(next as f64 * self.row_height as f64);
    }

    fn sync_visual_from_px(&mut self) {
        let visual_px = self.scroll_animation.value();
        self.scroll_y = visual_px.round() as i32;
        self.visual_index = visual_px as f32 / self.row_height as f32;
    }
}

fn clamp_arcade_spring_at_target(animation: &mut SpringAnimation, direction: i32) {
    let target = animation.target();
    if (direction > 0 && animation.value() >= target)
        || (direction < 0 && animation.value() <= target)
    {
        animation.snap_to(target);
    }
}

fn directional_row_spring_target(
    value: f64,
    velocity: f64,
    count: usize,
    direction: i32,
    row_height: i32,
    angular_frequency: f64,
) -> f64 {
    let pitch = row_height.max(1) as f64;
    let max_scroll = count.saturating_sub(1) as f64 * pitch;
    if direction == 0 {
        return (value / pitch).round() * pitch;
    }

    // Give the critically damped spring enough forward runway to absorb the
    // incoming cruise velocity without crossing its target and recoiling.
    let minimum_distance = velocity.abs() / angular_frequency.max(f64::EPSILON);
    let mut target = if direction > 0 {
        (value / pitch).ceil() * pitch
    } else {
        (value / pitch).floor() * pitch
    };
    if direction > 0 {
        while target - value < minimum_distance && target < max_scroll {
            target += pitch;
        }
    } else {
        while value - target < minimum_distance && target > 0.0 {
            target -= pitch;
        }
    }
    target.clamp(0.0, max_scroll)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[doc(hidden)]
pub struct LauncherTaxonomySyncTiming {
    pub changed: bool,
    pub taxonomy_build_us: u128,
    pub navigation_reconcile_us: u128,
}

pub struct LauncherNav {
    crt_layout: bool,
    portrait_layout: bool,
    pub screen: Screen,
    pub selected: usize,
    pub system_hub_selected: usize,
    pub scroll_x: i32,
    pub settings_focused: bool,
    pub settings_selected: usize,
    pub about_selected: usize,
    pub display_combo_open: bool,
    pub display_selected: usize,
    pub display_highlighted: usize,
    pub display_confirm_remaining: u8,
    pub display_confirm_busy: bool,
    pub display_error: Option<String>,
    pub orientation_combo_open: bool,
    pub orientation_selected: usize,
    pub orientation_highlighted: usize,
    pub orientation_confirm_remaining: u8,
    pub screensaver_selected: usize,
    pub settings: MagikSettings,
    pub licenses_selected: usize,
    pub licenses_expanded: bool,
    licenses_scroll: ArcadeNav,
    pub confirm_action: Option<ConfirmAction>,
    pub confirm_selected: usize,
    pub arcade: ArcadeNav,
    pub arcade_filter: ArcadeFilterState,
    pub arcade_search: ArcadeSearchState,
    favourite_launch_refs: HashSet<String>,
    favourite_launch_refs_revision: u64,
    recent_launch_refs: Vec<String>,
    arcade_user_list_mode: ArcadeUserListMode,
    user_list_indexes: Vec<usize>,
    pending_game_action_path: Option<String>,
    game_list_memory: HashMap<String, GameListMemory>,
    collection_filters: HashMap<String, ArcadeFilter>,
    collection_search_queries: HashMap<String, String>,
    catalog_build_active: bool,
    catalog_update_states: HashMap<String, CatalogSystemUpdateState>,
    catalog_hydration_states: HashMap<String, CatalogSystemHydrationState>,
    taxonomy: LauncherTaxonomy,
    taxonomy_token: LauncherTaxonomyToken,
    menu_path: Vec<String>,
    menu_memory: HashMap<String, MenuViewportMemory>,
    active_collection_id: Option<String>,
    active_collection_source: Option<HomeViewState>,
    arcade_exit_locked: bool,
    home_scroll: HomeScrollState,
    home_scroll_animation: SpringAnimation,
    #[cfg(test)]
    test_repeat: RepeatNav,
    #[cfg(test)]
    test_prev: PadState,
}

#[derive(Clone, Copy, Debug, Default)]
struct HomeScrollState {
    held_dir: i32,
    hold_started_at: Option<Instant>,
    last_frame_at: Option<Instant>,
    active: bool,
    cursor_px: f64,
    motion_velocity: f64,
    settle_direction: i32,
}

#[derive(Clone, Copy)]
struct NavigationInput<'a> {
    pressed: &'a PadState,
    released: &'a PadState,
    held: &'a PadState,
    tick_continuous: bool,
    frame_now: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GameListMemory {
    selected: usize,
    scroll_y: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MenuViewportMemory {
    selected_item_id: Option<String>,
    selected: usize,
    scroll_x: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeViewState {
    menu_path: Vec<String>,
    selected: usize,
    scroll_x: i32,
}

#[derive(Clone, Debug)]
pub struct NavigationTransitionState {
    screen: Screen,
    selected: usize,
    system_hub_selected: usize,
    scroll_x: i32,
    settings_focused: bool,
    settings_selected: usize,
    about_selected: usize,
    display_combo_open: bool,
    display_selected: usize,
    display_highlighted: usize,
    display_confirm_remaining: u8,
    display_confirm_busy: bool,
    display_error: Option<String>,
    orientation_combo_open: bool,
    orientation_selected: usize,
    orientation_highlighted: usize,
    orientation_confirm_remaining: u8,
    screensaver_selected: usize,
    licenses_selected: usize,
    licenses_expanded: bool,
    licenses_scroll: ArcadeNav,
    confirm_action: Option<ConfirmAction>,
    confirm_selected: usize,
    arcade: ArcadeNav,
    arcade_filter: ArcadeFilterState,
    arcade_search: ArcadeSearchState,
    favourite_launch_refs: HashSet<String>,
    favourite_launch_refs_revision: u64,
    recent_launch_refs: Vec<String>,
    arcade_user_list_mode: ArcadeUserListMode,
    user_list_indexes: Vec<usize>,
    pending_game_action_path: Option<String>,
    game_list_memory: HashMap<String, GameListMemory>,
    collection_filters: HashMap<String, ArcadeFilter>,
    collection_search_queries: HashMap<String, String>,
    menu_path: Vec<String>,
    menu_memory: HashMap<String, MenuViewportMemory>,
    taxonomy: LauncherTaxonomy,
    taxonomy_token: LauncherTaxonomyToken,
    active_collection_id: Option<String>,
    active_collection_source: Option<HomeViewState>,
    arcade_exit_locked: bool,
    home_scroll: HomeScrollState,
    home_scroll_animation: SpringAnimation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcadeFilterLevel {
    Alphabet,
    Top,
    Categories,
    Decades,
    Manufacturers,
    Players,
    Controls,
}

impl ArcadeFilterLevel {
    fn parent(self) -> Option<Self> {
        match self {
            Self::Top => None,
            Self::Alphabet => None,
            Self::Categories
            | Self::Decades
            | Self::Manufacturers
            | Self::Players
            | Self::Controls => Some(Self::Top),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArcadeFilterGroup {
    Games,
    Search,
    Categories,
    Decades,
    Manufacturers,
    Players,
    Controls,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArcadeTopDrawerItem {
    group: ArcadeFilterGroup,
    item: ArcadeDrawerItem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcadeSearchPane {
    Keyboard,
    Results,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArcadeSearchStatus {
    #[default]
    Idle,
    Searching,
    Ready,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArcadeSearchRequest {
    pub request_id: u64,
    pub catalog_version: usize,
    pub collection_id: String,
    pub system_ids: Vec<String>,
    pub query: String,
}

#[derive(Clone, Debug)]
pub struct ArcadeSearchState {
    pub query: String,
    pub suggestion: String,
    pub status: ArcadeSearchStatus,
    pub selected_key: usize,
    pub pane: ArcadeSearchPane,
    results: Vec<usize>,
    result_system_id: String,
    result_query: String,
    suggestion_system_id: String,
    suggestion_query: String,
    request_id: u64,
    request_pending: bool,
}

impl Default for ArcadeSearchState {
    fn default() -> Self {
        Self::new()
    }
}

impl ArcadeSearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            suggestion: String::new(),
            status: ArcadeSearchStatus::Idle,
            selected_key: 0,
            pane: ArcadeSearchPane::Keyboard,
            results: Vec::new(),
            result_system_id: String::new(),
            result_query: String::new(),
            suggestion_system_id: String::new(),
            suggestion_query: String::new(),
            request_id: 0,
            request_pending: false,
        }
    }

    pub fn is_active(&self, filter: &ArcadeFilter) -> bool {
        matches!(filter, ArcadeFilter::Search)
    }
}

#[derive(Clone, Debug)]
pub struct ArcadeFilterState {
    pub drawer_open: bool,
    pub level: ArcadeFilterLevel,
    pub selected: usize,
    pub scroll_y: i32,
    pub visual_index: f32,
    pub active: ArcadeFilter,
    // A branch transition consumes activation until both activation controls
    // are released. One physical press can therefore cross only one edge.
    activation_release_required: bool,
    scroll: ArcadeNav,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArcadeDrawerItem {
    pub label: String,
    pub count: usize,
    pub active: bool,
}

impl Default for ArcadeFilterState {
    fn default() -> Self {
        Self::new()
    }
}

impl ArcadeFilterState {
    pub fn new() -> Self {
        Self {
            drawer_open: false,
            level: ArcadeFilterLevel::Top,
            selected: 0,
            scroll_y: 0,
            visual_index: 0.0,
            active: ArcadeFilter::All,
            activation_release_required: false,
            scroll: ArcadeNav::with_row_height(ARCADE_ROW_HEIGHT),
        }
    }

    pub fn title(&self) -> &'static str {
        match self.level {
            ArcadeFilterLevel::Alphabet => "Games A-Z",
            ArcadeFilterLevel::Top => "Filters",
            ArcadeFilterLevel::Categories => "Categories",
            ArcadeFilterLevel::Decades => "Decades",
            ArcadeFilterLevel::Manufacturers => "Manufacturers",
            ArcadeFilterLevel::Players => "Players",
            ArcadeFilterLevel::Controls => "Controls",
        }
    }

    pub fn active_label(&self) -> String {
        match &self.active {
            ArcadeFilter::All => "Games A-Z".to_string(),
            ArcadeFilter::Search => "Search".to_string(),
            ArcadeFilter::Category(category) => category.clone(),
            ArcadeFilter::Decade(decade) => format!("{decade}'s"),
            ArcadeFilter::Manufacturer(manufacturer) => manufacturer.clone(),
            ArcadeFilter::Players(players) => player_count_label(*players),
            ArcadeFilter::Control(control) => control.clone(),
        }
    }

    pub fn is_scroll_active(&self) -> bool {
        self.scroll.is_scroll_active()
    }

    fn active_group(&self) -> ArcadeFilterGroup {
        match self.active {
            ArcadeFilter::All => ArcadeFilterGroup::Games,
            ArcadeFilter::Search => ArcadeFilterGroup::Search,
            ArcadeFilter::Category(_) => ArcadeFilterGroup::Categories,
            ArcadeFilter::Decade(_) => ArcadeFilterGroup::Decades,
            ArcadeFilter::Manufacturer(_) => ArcadeFilterGroup::Manufacturers,
            ArcadeFilter::Players(_) => ArcadeFilterGroup::Players,
            ArcadeFilter::Control(_) => ArcadeFilterGroup::Controls,
        }
    }

    fn active_level(&self) -> ArcadeFilterLevel {
        match self.active {
            ArcadeFilter::All | ArcadeFilter::Search => ArcadeFilterLevel::Top,
            ArcadeFilter::Category(_) => ArcadeFilterLevel::Categories,
            ArcadeFilter::Decade(_) => ArcadeFilterLevel::Decades,
            ArcadeFilter::Manufacturer(_) => ArcadeFilterLevel::Manufacturers,
            ArcadeFilter::Players(_) => ArcadeFilterLevel::Players,
            ArcadeFilter::Control(_) => ArcadeFilterLevel::Controls,
        }
    }
}

impl Default for LauncherNav {
    fn default() -> Self {
        Self::new()
    }
}

impl LauncherNav {
    pub fn for_crt_layout(crt_layout: bool) -> Self {
        Self::for_crt_layout_with_row_height(crt_layout, ARCADE_ROW_HEIGHT)
    }

    pub fn for_crt_layout_with_row_height(crt_layout: bool, row_height: i32) -> Self {
        let mut nav = Self::new();
        nav.crt_layout = crt_layout;
        if crt_layout {
            nav.arcade = ArcadeNav::with_row_height(row_height);
            nav.arcade_filter.scroll = ArcadeNav::with_row_height(row_height);
        }
        nav
    }

    pub fn uses_crt_layout(&self) -> bool {
        self.crt_layout
    }

    pub fn set_portrait_layout(&mut self, portrait_layout: bool) {
        self.portrait_layout = portrait_layout;
        self.settings_focused = false;
        self.home_scroll = HomeScrollState::default();
        self.home_scroll_animation.snap_to(self.scroll_x as f64);
    }

    pub fn uses_portrait_layout(&self) -> bool {
        self.portrait_layout
    }

    pub fn sync_orientation_selection(&mut self) {
        self.orientation_selected = ScreenOrientation::ALL
            .iter()
            .position(|orientation| *orientation == self.settings.screen_orientation)
            .unwrap_or(0);
        self.orientation_highlighted = self.orientation_selected;
    }

    pub fn home_horizontal_held(&self) -> bool {
        self.screen == Screen::Home
            && !self.portrait_layout
            && !self.settings_focused
            && self.home_scroll.held_dir != 0
    }

    pub fn home_horizontal_repeat_active(&self) -> bool {
        self.screen == Screen::Home
            && !self.portrait_layout
            && !self.settings_focused
            && self.home_scroll.active
    }

    pub fn arcade_uses_menu_repeat(&self) -> bool {
        self.screen == Screen::Arcade
            && self.arcade_search.is_active(&self.arcade_filter.active)
            && self.arcade_search.pane == ArcadeSearchPane::Keyboard
    }

    pub fn new() -> Self {
        Self {
            crt_layout: false,
            portrait_layout: false,
            screen: Screen::Home,
            selected: 0,
            system_hub_selected: 0,
            scroll_x: 0,
            settings_focused: false,
            settings_selected: 0,
            about_selected: 0,
            display_combo_open: false,
            display_selected: usize::MAX,
            display_highlighted: 0,
            display_confirm_remaining: 0,
            display_confirm_busy: false,
            display_error: None,
            orientation_combo_open: false,
            orientation_selected: 0,
            orientation_highlighted: 0,
            orientation_confirm_remaining: 0,
            screensaver_selected: 0,
            settings: MagikSettings::default(),
            licenses_selected: 0,
            licenses_expanded: false,
            licenses_scroll: ArcadeNav::with_row_height_and_step(LICENSE_SCROLL_LINE_PX as i32, 3),
            confirm_action: None,
            confirm_selected: 0,
            arcade: ArcadeNav::new(),
            arcade_filter: ArcadeFilterState::new(),
            arcade_search: ArcadeSearchState::new(),
            favourite_launch_refs: HashSet::new(),
            favourite_launch_refs_revision: 0,
            recent_launch_refs: Vec::new(),
            arcade_user_list_mode: ArcadeUserListMode::Games,
            user_list_indexes: Vec::new(),
            pending_game_action_path: None,
            game_list_memory: HashMap::new(),
            collection_filters: HashMap::new(),
            collection_search_queries: HashMap::new(),
            catalog_build_active: false,
            catalog_update_states: HashMap::new(),
            catalog_hydration_states: HashMap::new(),
            taxonomy: LauncherTaxonomy::default(),
            taxonomy_token: LauncherTaxonomyToken::default(),
            menu_path: vec![ROOT_MENU_ID.to_string()],
            menu_memory: HashMap::new(),
            active_collection_id: None,
            active_collection_source: None,
            arcade_exit_locked: false,
            home_scroll: HomeScrollState::default(),
            home_scroll_animation: SpringAnimation::new(0.0, SpringConfiguration::smooth()),
            #[cfg(test)]
            test_repeat: RepeatNav::default(),
            #[cfg(test)]
            test_prev: PadState::default(),
        }
    }

    /// Rebuilds the cached launcher hierarchy when the catalog allocation or
    /// system projection changes. Call this after publishing a catalog before
    /// reading menu or active-collection state.
    pub fn sync_launcher_taxonomy(&mut self, catalog: &ArcadeCatalog) -> bool {
        self.sync_launcher_taxonomy_impl(catalog, false, None)
            .changed
    }

    #[doc(hidden)]
    pub fn sync_launcher_taxonomy_with_timing(
        &mut self,
        catalog: &ArcadeCatalog,
        taxonomy_built: &mut dyn FnMut(),
    ) -> LauncherTaxonomySyncTiming {
        self.sync_launcher_taxonomy_impl(catalog, true, Some(taxonomy_built))
    }

    fn sync_launcher_taxonomy_impl(
        &mut self,
        catalog: &ArcadeCatalog,
        measure: bool,
        mut taxonomy_built: Option<&mut dyn FnMut()>,
    ) -> LauncherTaxonomySyncTiming {
        let token = LauncherTaxonomyToken::from_catalog(catalog);
        if self.taxonomy_token == token && self.taxonomy.matches_catalog(catalog) {
            return LauncherTaxonomySyncTiming::default();
        }

        if self.screen == Screen::Home {
            self.remember_current_menu_view();
        }
        let old_path = self.menu_path.clone();
        let old_collection = self.active_collection_id.clone();
        let had_active_collection = old_collection.is_some();
        let build_systems = self.catalog_update_states.keys().cloned().collect();
        let taxonomy_started = measure.then(Instant::now);
        self.taxonomy = LauncherTaxonomy::from_catalog_with_shells(catalog, &build_systems);
        let taxonomy_build_us = taxonomy_started
            .map(|started| started.elapsed().as_micros())
            .unwrap_or(0);
        if let Some(taxonomy_built) = taxonomy_built.as_mut() {
            taxonomy_built();
        }
        let navigation_started = measure.then(Instant::now);
        self.taxonomy_token = token;
        for diagnostic in self.taxonomy.diagnostics() {
            crate::ui_errln!("{diagnostic}");
        }

        self.menu_path = self.valid_menu_path_prefix(&old_path);
        if self.menu_path.is_empty() {
            self.menu_path.push(ROOT_MENU_ID.to_string());
        }
        self.active_collection_id = old_collection
            .filter(|collection_id| self.taxonomy.collection(collection_id).is_some());
        if let Some(collection_id) = self.active_collection_id.clone()
            && !self
                .taxonomy
                .collection_path_is_valid(&self.menu_path, &collection_id)
            && let Some(destination) = self
                .taxonomy
                .primary_destination_for_collection(&collection_id)
        {
            self.menu_path = destination.menu_path.clone();
        }

        if self.screen == Screen::Arcade && self.active_collection_id.is_none() {
            if had_active_collection {
                self.screen = Screen::Home;
                self.restore_current_menu_view();
                return LauncherTaxonomySyncTiming {
                    changed: true,
                    taxonomy_build_us,
                    navigation_reconcile_us: navigation_started
                        .map(|started| started.elapsed().as_micros())
                        .unwrap_or(0),
                };
            }
            // Startup and benchmark paths can select the Arcade screen before
            // the catalog is hydrated. That screen always means the root
            // Arcade aggregate; `open_system` is the explicit compatibility
            // path for selecting an individual legacy system.
            let preserved_filter = self.arcade_filter.clone();
            let preserved_search = self.arcade_search.clone();
            let preserved_selected = self.arcade.selected;
            let preserved_scroll_y = self.arcade.scroll_y;
            if self.open_default_arcade_synced(catalog) {
                let collection_id = self
                    .active_collection_id
                    .clone()
                    .expect("default Arcade collection activated");
                self.arcade_filter = preserved_filter;
                self.arcade_search = preserved_search;
                if self.arcade_search.is_active(&self.arcade_filter.active) {
                    self.ensure_arcade_search_results(catalog, &collection_id);
                }
                let count = self.active_arcade_game_count(catalog, &collection_id);
                self.arcade
                    .restore_position(preserved_selected, preserved_scroll_y, count);
            } else {
                self.screen = Screen::Home;
                self.active_collection_id = None;
                self.restore_current_menu_view();
            }
        } else if self.screen == Screen::Home {
            self.restore_current_menu_view();
        }
        if self.screen == Screen::Arcade
            && let Some(collection_id) = self.active_collection_id.clone()
            && self.resolve_arcade_filter_for_collection(catalog, &collection_id)
        {
            self.arcade.reset();
        }
        LauncherTaxonomySyncTiming {
            changed: true,
            taxonomy_build_us,
            navigation_reconcile_us: navigation_started
                .map(|started| started.elapsed().as_micros())
                .unwrap_or(0),
        }
    }

    pub fn launcher_taxonomy_token(&self) -> LauncherTaxonomyToken {
        self.taxonomy_token
    }

    pub fn current_menu_id(&self) -> &str {
        self.menu_path
            .last()
            .map(String::as_str)
            .unwrap_or(ROOT_MENU_ID)
    }

    pub fn current_menu_title(&self) -> &str {
        self.taxonomy
            .menu(self.current_menu_id())
            .map(|menu| menu.title.as_str())
            .unwrap_or("MiSTer MagiK")
    }

    pub fn current_menu_breadcrumb(&self) -> &str {
        let Some(parent_id) = self
            .taxonomy
            .menu(self.current_menu_id())
            .and_then(|menu| menu.parent_id.as_deref())
        else {
            return "";
        };
        self.taxonomy
            .menu(parent_id)
            .map(|menu| menu.title.as_str())
            .unwrap_or("")
    }

    pub fn current_menu_items(&self) -> &[LauncherMenuItem] {
        self.taxonomy
            .menu(self.current_menu_id())
            .map(|menu| menu.items.as_slice())
            .unwrap_or(&[])
    }

    pub fn current_menu_count(&self) -> usize {
        self.current_menu_items().len()
    }

    pub fn current_menu_selected_item_id(&self) -> &str {
        self.current_menu_items()
            .get(self.selected)
            .map(|item| item.id.as_str())
            .unwrap_or("")
    }

    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    pub fn home_scroll_max(&self) -> i32 {
        home_max_scroll(self.current_menu_count())
    }

    pub fn current_menu_game_count(&self) -> usize {
        self.taxonomy
            .menu(self.current_menu_id())
            .map(|menu| menu.count)
            .unwrap_or(0)
    }

    pub fn catalog_build_started(&mut self) {
        self.catalog_update_states.clear();
        self.catalog_build_active = true;
    }

    pub fn catalog_system_discovered(&mut self, system_id: &str) {
        if !self.catalog_build_active {
            self.catalog_build_started();
        }
        self.catalog_build_active = true;
        self.catalog_update_states
            .insert(system_id.to_string(), CatalogSystemUpdateState::Queued);
    }

    pub fn catalog_reconciliation_plan(
        &mut self,
        catalog: &ArcadeCatalog,
        system_ids: &[String],
        all_published_systems: bool,
    ) {
        self.catalog_build_active = true;
        let systems = if all_published_systems {
            catalog
                .systems
                .iter()
                .map(|system| system.id.clone())
                .collect::<Vec<_>>()
        } else {
            system_ids.to_vec()
        };
        self.catalog_update_states = systems
            .into_iter()
            .map(|system_id| (system_id, CatalogSystemUpdateState::Queued))
            .collect();
    }

    pub fn catalog_system_scanning(&mut self, system_id: &str) {
        self.catalog_build_active = true;
        self.catalog_update_states
            .insert(system_id.to_string(), CatalogSystemUpdateState::Scanning);
    }

    pub fn catalog_system_prepared(&mut self, system_id: &str) {
        self.catalog_update_states
            .insert(system_id.to_string(), CatalogSystemUpdateState::Prepared);
    }

    pub fn catalog_system_update_ready(&mut self, system_id: &str) {
        self.catalog_update_states.remove(system_id);
    }

    pub fn catalog_system_update_failed(&mut self, system_id: &str) {
        self.catalog_update_states
            .insert(system_id.to_string(), CatalogSystemUpdateState::Failed);
    }

    pub fn catalog_system_hydration_started(&mut self, system_id: &str) {
        self.catalog_hydration_states
            .insert(system_id.to_string(), CatalogSystemHydrationState::Loading);
    }

    pub fn catalog_system_hydration_failed(&mut self, system_id: &str) {
        self.catalog_hydration_states
            .insert(system_id.to_string(), CatalogSystemHydrationState::Failed);
    }

    pub fn catalog_system_hydration_finished(&mut self, system_id: &str) {
        self.catalog_hydration_states.remove(system_id);
    }

    pub fn catalog_hydration_reset(&mut self) {
        self.catalog_hydration_states.clear();
    }

    pub fn catalog_system_update_has_failed(&self, system_id: &str) -> bool {
        self.catalog_update_states.get(system_id) == Some(&CatalogSystemUpdateState::Failed)
    }

    pub fn catalog_system_hydration_has_failed(&self, system_id: &str) -> bool {
        self.catalog_hydration_states.get(system_id) == Some(&CatalogSystemHydrationState::Failed)
    }

    pub fn catalog_system_hydration_is_loading(&self, system_id: &str) -> bool {
        self.catalog_hydration_states.get(system_id) == Some(&CatalogSystemHydrationState::Loading)
    }

    pub fn catalog_build_finished(&mut self, catalog: &ArcadeCatalog) {
        self.catalog_build_active = false;
        let authoritative_systems = catalog
            .systems
            .iter()
            .filter(|system| system.count > 0 || catalog.system_game_count(&system.id) > 0)
            .map(|system| system.id.as_str())
            .collect::<HashSet<_>>();
        self.catalog_update_states.retain(|system_id, state| {
            *state == CatalogSystemUpdateState::Failed
                && authoritative_systems.contains(system_id.as_str())
        });
        self.catalog_hydration_states
            .retain(|system_id, _| authoritative_systems.contains(system_id.as_str()));
    }

    pub fn catalog_with_build_shells(&self, mut catalog: ArcadeCatalog) -> ArcadeCatalog {
        for system_id in self.catalog_update_states.keys() {
            catalog = catalog.with_system_placeholder(system_id);
        }
        catalog
    }

    pub(crate) fn menu_item_catalog_presentation(
        &self,
        item: &LauncherMenuItem,
    ) -> CatalogMenuItemPresentation {
        match item.kind {
            LauncherMenuItemKind::Menu => {
                let partial = self.menu_contains_failed_descendant(&item.id);
                let scanning =
                    self.catalog_build_active && self.menu_contains_scanning_descendant(&item.id);
                CatalogMenuItemPresentation {
                    status: if scanning {
                        CatalogMenuItemStatus::Scanning
                    } else if partial {
                        CatalogMenuItemStatus::Partial
                    } else {
                        CatalogMenuItemStatus::Ready
                    },
                    available: true,
                    retryable: false,
                }
            }
            LauncherMenuItemKind::Collection => {
                let system_id = self
                    .taxonomy
                    .collection(&item.id)
                    .map(|collection| collection.legacy_system_id.as_str())
                    .unwrap_or(item.id.as_str());
                let update = self.catalog_update_states.get(system_id);
                let scanning = self.catalog_build_active
                    && matches!(
                        update,
                        Some(
                            CatalogSystemUpdateState::Queued
                                | CatalogSystemUpdateState::Scanning
                                | CatalogSystemUpdateState::Prepared
                        )
                    );
                let load_failed = self.catalog_system_hydration_has_failed(system_id);
                let update_failed = update == Some(&CatalogSystemUpdateState::Failed);
                CatalogMenuItemPresentation {
                    status: if load_failed {
                        CatalogMenuItemStatus::LoadFailed
                    } else if scanning {
                        CatalogMenuItemStatus::Scanning
                    } else if update_failed {
                        CatalogMenuItemStatus::UpdateFailed
                    } else {
                        CatalogMenuItemStatus::Ready
                    },
                    available: item.count > 0 && !load_failed,
                    retryable: load_failed,
                }
            }
        }
    }

    pub fn menu_discovered_system_count(&self, menu_id: &str) -> usize {
        self.taxonomy.menu(menu_id).map_or(0, |menu| {
            menu.items
                .iter()
                .map(|item| match item.kind {
                    LauncherMenuItemKind::Menu => self.menu_discovered_system_count(&item.id),
                    LauncherMenuItemKind::Collection => {
                        usize::from(self.catalog_update_states.contains_key(&item.id))
                    }
                })
                .sum()
        })
    }

    fn menu_contains_failed_descendant(&self, menu_id: &str) -> bool {
        self.taxonomy.menu(menu_id).is_some_and(|menu| {
            menu.items.iter().any(|item| match item.kind {
                LauncherMenuItemKind::Menu => self.menu_contains_failed_descendant(&item.id),
                LauncherMenuItemKind::Collection => {
                    self.catalog_update_states.get(&item.id)
                        == Some(&CatalogSystemUpdateState::Failed)
                        || self.catalog_hydration_states.get(&item.id)
                            == Some(&CatalogSystemHydrationState::Failed)
                }
            })
        })
    }

    fn menu_contains_scanning_descendant(&self, menu_id: &str) -> bool {
        self.taxonomy.menu(menu_id).is_some_and(|menu| {
            menu.items.iter().any(|item| match item.kind {
                LauncherMenuItemKind::Menu => self.menu_contains_scanning_descendant(&item.id),
                LauncherMenuItemKind::Collection => {
                    matches!(
                        self.catalog_update_states.get(&item.id),
                        Some(
                            CatalogSystemUpdateState::Queued
                                | CatalogSystemUpdateState::Scanning
                                | CatalogSystemUpdateState::Prepared
                        )
                    )
                }
            })
        })
    }

    pub fn menu_path(&self) -> &[String] {
        &self.menu_path
    }

    pub fn active_collection(&self) -> Option<&LauncherCollection> {
        self.active_collection_id
            .as_deref()
            .and_then(|id| self.taxonomy.collection(id))
    }

    pub fn active_collection_id(&self) -> Option<&str> {
        self.active_collection()
            .map(|collection| collection.id.as_str())
    }

    pub fn active_collection_scope_id<'a>(&'a self, catalog: &'a ArcadeCatalog) -> &'a str {
        self.active_collection_id().unwrap_or_else(|| {
            catalog
                .systems
                .get(self.selected)
                .map(|system| system.id.as_str())
                .unwrap_or("")
        })
    }

    fn effective_collection_id<'a>(&'a self, requested: &'a str) -> &'a str {
        let Some(collection) = self.active_collection() else {
            return requested;
        };
        if requested == collection.id
            || requested == collection.legacy_system_id
            || collection.system_id.as_deref() == Some(requested)
        {
            &collection.id
        } else {
            requested
        }
    }

    pub fn open_menu(&mut self, menu_id: &str) -> bool {
        let Some(path) = self.taxonomy.path_to_menu(menu_id) else {
            return false;
        };
        if self.screen == Screen::Home {
            self.remember_current_menu_view();
        }
        self.menu_path = path;
        self.active_collection_id = None;
        self.screen = Screen::Home;
        self.settings_focused = false;
        self.restore_current_menu_view();
        true
    }

    pub fn open_system(&mut self, catalog: &ArcadeCatalog, system_id: &str) -> bool {
        self.sync_launcher_taxonomy(catalog);
        self.open_system_synced(catalog, system_id)
    }

    pub fn open_default_arcade(&mut self, catalog: &ArcadeCatalog) -> bool {
        self.sync_launcher_taxonomy(catalog);
        self.open_default_arcade_synced(catalog)
    }

    pub fn go_root(&mut self) {
        if self.screen == Screen::Home {
            self.remember_current_menu_view();
        }
        self.menu_path.clear();
        self.menu_path.push(ROOT_MENU_ID.to_string());
        self.active_collection_id = None;
        self.screen = Screen::Home;
        self.settings_focused = false;
        self.restore_current_menu_view();
    }

    fn open_system_synced(&mut self, catalog: &ArcadeCatalog, system_id: &str) -> bool {
        let Some(destination) = self
            .taxonomy
            .primary_destination_for_system(system_id)
            .cloned()
        else {
            return false;
        };
        self.open_destination(catalog, destination.menu_path, &destination.collection_id)
    }

    fn open_default_arcade_synced(&mut self, catalog: &ArcadeCatalog) -> bool {
        let Some(destination) = self
            .taxonomy
            .primary_destination_for_collection(crate::arcade_catalog::MENU_ARCADE_SYSTEM_ID)
            .cloned()
        else {
            return false;
        };
        self.open_destination(catalog, destination.menu_path, &destination.collection_id)
    }

    fn open_destination(
        &mut self,
        catalog: &ArcadeCatalog,
        menu_path: Vec<String>,
        collection_id: &str,
    ) -> bool {
        if !self
            .taxonomy
            .collection_path_is_valid(&menu_path, collection_id)
        {
            return false;
        }
        if self.screen == Screen::Home {
            self.remember_current_menu_view();
        }
        self.menu_path = menu_path;
        self.restore_current_menu_view();
        if let Some(index) = self
            .current_menu_items()
            .iter()
            .position(|item| item.id == collection_id)
        {
            self.selected = index;
            let menu_count = self.current_menu_count();
            keep_home_visible(self.selected, &mut self.scroll_x, menu_count);
            self.remember_current_menu_view();
        }
        self.activate_collection(catalog, collection_id)
    }

    pub fn activate_collection(&mut self, catalog: &ArcadeCatalog, collection_id: &str) -> bool {
        let Some(collection) = self.taxonomy.collection(collection_id).cloned() else {
            return false;
        };
        if self.screen == Screen::Home {
            self.active_collection_source = Some(self.home_view_state());
            self.remember_current_menu_view();
        }
        self.active_collection_id = Some(collection.id.clone());
        self.arcade_user_list_mode = ArcadeUserListMode::Games;
        self.user_list_indexes.clear();
        let filter = self
            .collection_filters
            .get(&collection.id)
            .cloned()
            .unwrap_or_else(|| default_filter_for_system(catalog, &collection.id));
        self.arcade_filter.active = self.available_arcade_filter(catalog, &collection.id, &filter);
        self.collection_filters
            .insert(collection.id.clone(), self.arcade_filter.active.clone());
        if matches!(self.arcade_filter.active, ArcadeFilter::Search) {
            self.arcade_search.query = self
                .collection_search_queries
                .get(&collection.id)
                .cloned()
                .unwrap_or_default();
            self.ensure_arcade_search_results(catalog, &collection.id);
        }
        let count = self.active_arcade_game_count(catalog, &collection.id);
        self.restore_game_list_state(&collection.id, count);
        self.screen = Screen::Arcade;
        if collection
            .system_id
            .as_deref()
            .unwrap_or(&collection.legacy_system_id)
            .eq_ignore_ascii_case("snes")
        {
            self.screen = Screen::SystemHub;
            self.system_hub_selected = 0;
        }
        if let Some(system_index) = catalog.systems.iter().position(|system| {
            collection
                .system_id
                .as_deref()
                .unwrap_or(&collection.legacy_system_id)
                == system.id
        }) {
            // Transitional compatibility for loop/bridge code that still
            // treats selected as a catalog-system index on the game screen.
            self.selected = system_index;
        }
        true
    }

    fn valid_menu_path_prefix(&self, requested: &[String]) -> Vec<String> {
        let mut valid = vec![ROOT_MENU_ID.to_string()];
        for menu_id in requested.iter().skip(1) {
            let parent = valid.last().expect("root menu path");
            if self.taxonomy.menu(menu_id).is_none()
                || !self.taxonomy.menu_contains_item(parent, menu_id)
            {
                break;
            }
            valid.push(menu_id.clone());
        }
        valid
    }

    fn remember_current_menu_view(&mut self) {
        let menu_id = self.current_menu_id().to_string();
        let selected_item_id = self
            .current_menu_items()
            .get(self.selected)
            .map(|item| item.id.clone());
        self.menu_memory.insert(
            menu_id,
            MenuViewportMemory {
                selected_item_id,
                selected: self.selected,
                scroll_x: self.scroll_x,
            },
        );
    }

    pub fn home_view_state(&self) -> HomeViewState {
        HomeViewState {
            menu_path: self.menu_path.clone(),
            selected: self.selected,
            scroll_x: self.scroll_x,
        }
    }

    pub fn navigation_transition_state(&self) -> NavigationTransitionState {
        NavigationTransitionState {
            screen: self.screen,
            selected: self.selected,
            system_hub_selected: self.system_hub_selected,
            scroll_x: self.scroll_x,
            settings_focused: self.settings_focused,
            settings_selected: self.settings_selected,
            about_selected: self.about_selected,
            display_combo_open: self.display_combo_open,
            display_selected: self.display_selected,
            display_highlighted: self.display_highlighted,
            display_confirm_remaining: self.display_confirm_remaining,
            display_confirm_busy: self.display_confirm_busy,
            display_error: self.display_error.clone(),
            orientation_combo_open: self.orientation_combo_open,
            orientation_selected: self.orientation_selected,
            orientation_highlighted: self.orientation_highlighted,
            orientation_confirm_remaining: self.orientation_confirm_remaining,
            screensaver_selected: self.screensaver_selected,
            licenses_selected: self.licenses_selected,
            licenses_expanded: self.licenses_expanded,
            licenses_scroll: self.licenses_scroll.clone(),
            confirm_action: self.confirm_action,
            confirm_selected: self.confirm_selected,
            arcade: self.arcade.clone(),
            arcade_filter: self.arcade_filter.clone(),
            arcade_search: self.arcade_search.clone(),
            favourite_launch_refs: self.favourite_launch_refs.clone(),
            favourite_launch_refs_revision: self.favourite_launch_refs_revision,
            recent_launch_refs: self.recent_launch_refs.clone(),
            arcade_user_list_mode: self.arcade_user_list_mode,
            user_list_indexes: self.user_list_indexes.clone(),
            pending_game_action_path: self.pending_game_action_path.clone(),
            game_list_memory: self.game_list_memory.clone(),
            collection_filters: self.collection_filters.clone(),
            collection_search_queries: self.collection_search_queries.clone(),
            menu_path: self.menu_path.clone(),
            menu_memory: self.menu_memory.clone(),
            taxonomy: self.taxonomy.clone(),
            taxonomy_token: self.taxonomy_token,
            active_collection_id: self.active_collection_id.clone(),
            active_collection_source: self.active_collection_source.clone(),
            arcade_exit_locked: self.arcade_exit_locked,
            home_scroll: self.home_scroll,
            home_scroll_animation: self.home_scroll_animation,
        }
    }

    pub fn restore_navigation_transition_state(&mut self, state: NavigationTransitionState) {
        self.screen = state.screen;
        self.selected = state.selected;
        self.system_hub_selected = state.system_hub_selected;
        self.scroll_x = state.scroll_x;
        self.settings_focused = state.settings_focused;
        self.settings_selected = state.settings_selected;
        self.about_selected = state.about_selected;
        self.display_combo_open = state.display_combo_open;
        self.display_selected = state.display_selected;
        self.display_highlighted = state.display_highlighted;
        self.display_confirm_remaining = state.display_confirm_remaining;
        self.display_confirm_busy = state.display_confirm_busy;
        self.display_error = state.display_error;
        self.orientation_combo_open = state.orientation_combo_open;
        self.orientation_selected = state.orientation_selected;
        self.orientation_highlighted = state.orientation_highlighted;
        self.orientation_confirm_remaining = state.orientation_confirm_remaining;
        self.screensaver_selected = state.screensaver_selected;
        self.licenses_selected = state.licenses_selected;
        self.licenses_expanded = state.licenses_expanded;
        self.licenses_scroll = state.licenses_scroll;
        self.confirm_action = state.confirm_action;
        self.confirm_selected = state.confirm_selected;
        self.arcade = state.arcade;
        self.arcade_filter = state.arcade_filter;
        self.arcade_search = state.arcade_search;
        self.favourite_launch_refs = state.favourite_launch_refs;
        self.favourite_launch_refs_revision = state.favourite_launch_refs_revision;
        self.recent_launch_refs = state.recent_launch_refs;
        self.arcade_user_list_mode = state.arcade_user_list_mode;
        self.user_list_indexes = state.user_list_indexes;
        self.pending_game_action_path = state.pending_game_action_path;
        self.game_list_memory = state.game_list_memory;
        self.collection_filters = state.collection_filters;
        self.collection_search_queries = state.collection_search_queries;
        self.menu_path = state.menu_path;
        self.menu_memory = state.menu_memory;
        self.taxonomy = state.taxonomy;
        self.taxonomy_token = state.taxonomy_token;
        self.active_collection_id = state.active_collection_id;
        self.active_collection_source = state.active_collection_source;
        self.arcade_exit_locked = state.arcade_exit_locked;
        self.home_scroll = state.home_scroll;
        self.home_scroll_animation = state.home_scroll_animation;
    }

    fn restore_home_view_state(&mut self, source: HomeViewState) {
        self.menu_path = self.valid_menu_path_prefix(&source.menu_path);
        self.selected = source.selected;
        self.scroll_x = source.scroll_x;
        self.remember_current_menu_view();
        self.restore_current_menu_view();
    }

    pub fn restore_pending_home_view(&mut self, source: HomeViewState) {
        self.active_collection_id = None;
        self.active_collection_source = None;
        self.screen = Screen::Home;
        self.settings_focused = false;
        self.restore_home_view_state(source);
    }

    fn restore_current_menu_view(&mut self) {
        let menu_id = self.current_menu_id().to_string();
        let count = self.current_menu_count();
        let memory = self.menu_memory.get(&menu_id).cloned().unwrap_or_default();
        if count == 0 {
            self.selected = 0;
            self.scroll_x = 0;
        } else {
            self.selected = memory
                .selected_item_id
                .as_deref()
                .and_then(|selected_id| {
                    self.current_menu_items()
                        .iter()
                        .position(|item| item.id == selected_id)
                })
                .unwrap_or(memory.selected.min(count - 1));
            self.scroll_x = memory.scroll_x;
            keep_home_visible(self.selected, &mut self.scroll_x, count);
        }
        self.home_scroll = HomeScrollState::default();
        self.home_scroll_animation.snap_to(self.scroll_x as f64);
        self.home_scroll.cursor_px = self.selected as f64 * home_tile_pitch() as f64;
    }

    fn pop_menu(&mut self) -> bool {
        if self.menu_path.len() <= 1 {
            return false;
        }
        self.remember_current_menu_view();
        self.menu_path.pop();
        self.active_collection_id = None;
        self.settings_focused = false;
        self.restore_current_menu_view();
        true
    }

    fn leave_arcade(&mut self, to_root: bool, collection_id: &str) {
        if self.arcade_exit_locked {
            return;
        }
        if !collection_id.is_empty() {
            self.save_game_list_state(collection_id);
            self.collection_filters
                .insert(collection_id.to_string(), self.arcade_filter.active.clone());
            if matches!(self.arcade_filter.active, ArcadeFilter::Search) {
                self.collection_search_queries
                    .insert(collection_id.to_string(), self.arcade_search.query.clone());
            }
        }
        self.active_collection_id = None;
        if to_root {
            self.go_root();
        } else {
            self.screen = Screen::Home;
            self.settings_focused = false;
            self.restore_current_menu_view();
        }
    }

    pub fn recover_empty_collection_to_home(&mut self) {
        self.active_collection_id = None;
        self.screen = Screen::Home;
        self.settings_focused = false;
        if let Some(source) = self.active_collection_source.take() {
            self.restore_home_view_state(source);
        } else {
            self.restore_current_menu_view();
        }
    }

    pub fn set_arcade_exit_locked(&mut self, locked: bool) {
        self.arcade_exit_locked = locked;
    }

    /// Snapshot adapter retained only for the existing reducer test suite.
    #[cfg(test)]
    pub fn handle_input(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        self.handle_test_snapshot(now, frame_now, catalog, false, false)
    }

    #[cfg(test)]
    pub fn handle_input_with_collection_intents(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        self.handle_test_snapshot(now, frame_now, catalog, true, false)
    }

    #[cfg(test)]
    pub fn handle_input_with_navigation_intents(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        self.handle_test_snapshot(now, frame_now, catalog, true, true)
    }

    pub fn handle_action_with_navigation_intents(
        &mut self,
        event: &InputEvent,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        let mut pressed = PadState::default();
        let mut released = PadState::default();
        match event.phase {
            InputPhase::Pressed => pressed.set_logical_action(event.action, true),
            InputPhase::Released => released.set_logical_action(event.action, true),
        }
        let held = PadState::default();
        self.handle_input_internal(
            NavigationInput {
                pressed: &pressed,
                released: &released,
                held: &held,
                tick_continuous: false,
                frame_now,
            },
            catalog,
            true,
            true,
        )
    }

    pub fn handle_held_tick_with_navigation_intents(
        &mut self,
        held: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        let pressed = PadState::default();
        let released = PadState::default();
        self.handle_input_internal(
            NavigationInput {
                pressed: &pressed,
                released: &released,
                held,
                tick_continuous: true,
                frame_now,
            },
            catalog,
            true,
            true,
        )
    }

    #[cfg(test)]
    fn handle_test_snapshot(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
        emit_collection_intents: bool,
        emit_navigation_intents: bool,
    ) -> Option<LauncherEvent> {
        let previous = self.test_prev.clone();
        let mut pressed = PadState::default();
        let mut released = PadState::default();
        for action in crate::input_event::LogicalAction::ALL {
            let is_held = pad_action_held(now, action);
            let was_held = pad_action_held(&previous, action);
            let fire = match action {
                crate::input_event::LogicalAction::Up => {
                    self.test_repeat.tick_up(is_held, frame_now)
                }
                crate::input_event::LogicalAction::Down => {
                    self.test_repeat.tick_down(is_held, frame_now)
                }
                crate::input_event::LogicalAction::Left => {
                    self.test_repeat.tick_left(is_held, frame_now)
                }
                crate::input_event::LogicalAction::Right => {
                    self.test_repeat.tick_right(is_held, frame_now)
                }
                _ => is_held && !was_held,
            };
            if fire {
                pressed.set_logical_action(action, true);
            }
            if was_held && !is_held {
                released.set_logical_action(action, true);
            }
        }
        self.test_prev = now.clone();
        self.handle_input_internal(
            NavigationInput {
                pressed: &pressed,
                released: &released,
                held: now,
                tick_continuous: true,
                frame_now,
            },
            catalog,
            emit_collection_intents,
            emit_navigation_intents,
        )
    }

    #[cfg(test)]
    pub fn reset_test_snapshot(&mut self, now: &PadState) {
        self.test_prev = now.clone();
        self.test_repeat = RepeatNav::default();
    }

    pub fn commit_navigation_intent(
        &mut self,
        event: &LauncherEvent,
        catalog: &ArcadeCatalog,
    ) -> bool {
        match event.action {
            LauncherAction::OpenMenu => event
                .path
                .as_deref()
                .is_some_and(|menu_id| self.open_menu(menu_id)),
            LauncherAction::OpenCollection => event
                .path
                .as_deref()
                .is_some_and(|collection_id| self.activate_collection(catalog, collection_id)),
            LauncherAction::NavigateBack if self.screen == Screen::Home => self.pop_menu(),
            LauncherAction::NavigateBack if self.screen == Screen::Arcade => {
                if self.return_arcade_to_system_hub() {
                    return true;
                }
                let collection_id = self.active_collection_scope_id(catalog).to_string();
                let before = self.screen;
                self.leave_arcade(false, &collection_id);
                self.screen != before
            }
            LauncherAction::NavigateHome if self.screen == Screen::Home => {
                let before = self.current_menu_id().to_string();
                self.go_root();
                self.current_menu_id() != before
            }
            LauncherAction::NavigateHome if self.screen == Screen::Arcade => {
                let collection_id = self.active_collection_scope_id(catalog).to_string();
                let before = self.screen;
                self.leave_arcade(true, &collection_id);
                self.screen != before
            }
            _ => false,
        }
    }

    fn handle_input_internal(
        &mut self,
        input: NavigationInput<'_>,
        catalog: &ArcadeCatalog,
        emit_collection_intents: bool,
        emit_navigation_intents: bool,
    ) -> Option<LauncherEvent> {
        let NavigationInput {
            pressed,
            held,
            tick_continuous,
            frame_now,
            ..
        } = input;
        self.sync_launcher_taxonomy(catalog);
        if self.confirm_action.is_some() {
            self.handle_confirm(pressed)
        } else {
            match self.screen {
                Screen::Home => self.handle_home(
                    input,
                    catalog,
                    emit_collection_intents,
                    emit_navigation_intents,
                ),
                Screen::SystemHub => self.handle_system_hub(pressed, catalog),
                Screen::Controller => {
                    if pressed.btn_home {
                        self.go_root();
                    } else if pressed.btn_b {
                        self.screen = Screen::Home;
                        self.restore_current_menu_view();
                    }
                    None
                }
                Screen::Arcade => self.handle_arcade(input, catalog, emit_navigation_intents),
                Screen::Settings => self.handle_settings(pressed),
                Screen::Screensaver => self.handle_screensaver_settings(pressed),
                Screen::About => self.handle_about(pressed),
                Screen::Info => {
                    self.handle_settings_subscreen(pressed);
                    None
                }
                Screen::Licenses => self.handle_licenses(pressed, held, tick_continuous, frame_now),
            }
        }
    }

    fn handle_system_hub(
        &mut self,
        pressed: &PadState,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        if pressed.btn_home {
            self.go_root();
            return None;
        }
        if pressed.btn_b {
            self.active_collection_id = None;
            self.screen = Screen::Home;
            self.settings_focused = false;
            if let Some(source) = self.active_collection_source.take() {
                self.restore_home_view_state(source);
            } else {
                self.restore_current_menu_view();
            }
            return None;
        }
        if pressed.dpad_right && matches!(self.system_hub_selected, 0 | 2) {
            self.system_hub_selected += 1;
        }
        if pressed.dpad_left && matches!(self.system_hub_selected, 1 | 3) {
            self.system_hub_selected -= 1;
        }
        if pressed.dpad_down && self.system_hub_selected < 2 {
            self.system_hub_selected += 2;
        }
        if pressed.dpad_up && self.system_hub_selected >= 2 {
            self.system_hub_selected -= 2;
        }
        if pressed.btn_a && self.system_hub_selected < 3 {
            let mode = match self.system_hub_selected {
                0 => ArcadeUserListMode::Games,
                1 => ArcadeUserListMode::Recent,
                2 => ArcadeUserListMode::Favourites,
                _ => unreachable!(),
            };
            self.set_arcade_user_list_mode(catalog, mode);
            self.screen = Screen::Arcade;
        }
        None
    }

    fn handle_home(
        &mut self,
        input: NavigationInput<'_>,
        catalog: &ArcadeCatalog,
        emit_collection_intents: bool,
        emit_navigation_intents: bool,
    ) -> Option<LauncherEvent> {
        let NavigationInput {
            pressed,
            held,
            tick_continuous,
            frame_now,
            ..
        } = input;
        if pressed.btn_home {
            if (self.crt_layout || self.portrait_layout) && self.current_menu_id() == ROOT_MENU_ID {
                self.remember_current_menu_view();
                self.settings_selected = 0;
                self.settings_focused = false;
                self.screen = Screen::Settings;
            } else if emit_navigation_intents && self.menu_path.len() > 1 {
                return Some(LauncherEvent {
                    action: LauncherAction::NavigateHome,
                    path: None,
                    settings: None,
                });
            } else {
                self.go_root();
            }
            return None;
        }
        if pressed.btn_b {
            if emit_navigation_intents && self.menu_path.len() > 1 {
                return Some(LauncherEvent {
                    action: LauncherAction::NavigateBack,
                    path: None,
                    settings: None,
                });
            }
            self.pop_menu();
            return None;
        }

        let item_count = self.current_menu_count();
        if !self.crt_layout && !self.portrait_layout {
            if pressed.dpad_up {
                self.settings_focused = true;
            }
            if pressed.dpad_down {
                self.settings_focused = false;
            }
            if self.settings_focused {
                self.home_scroll = HomeScrollState::default();
                self.home_scroll_animation.snap_to(self.scroll_x as f64);
                if pressed.btn_a {
                    self.remember_current_menu_view();
                    self.settings_selected = 0;
                    self.screen = Screen::Settings;
                }
                return None;
            }
        }
        if item_count == 0 {
            self.home_scroll = HomeScrollState::default();
            self.scroll_x = 0;
            self.home_scroll_animation.snap_to(0.0);
            return None;
        }

        if self.selected >= item_count {
            self.selected = item_count - 1;
            keep_home_visible(self.selected, &mut self.scroll_x, item_count);
            self.home_scroll_animation.snap_to(self.scroll_x as f64);
            self.home_scroll.cursor_px = self.selected as f64 * home_tile_pitch() as f64;
        }
        if tick_continuous {
            self.update_home_scroll(held, frame_now, item_count);
        }

        if pressed.btn_a {
            let item = self.current_menu_items().get(self.selected).cloned();
            if let Some(item) = item {
                match item.kind {
                    LauncherMenuItemKind::Menu => {
                        if emit_navigation_intents {
                            return Some(LauncherEvent {
                                action: LauncherAction::OpenMenu,
                                path: Some(item.id),
                                settings: None,
                            });
                        }
                        self.open_menu(&item.id);
                    }
                    LauncherMenuItemKind::Collection => {
                        let presentation = self.menu_item_catalog_presentation(&item);
                        if presentation.available
                            || (emit_collection_intents && presentation.retryable)
                        {
                            if emit_collection_intents {
                                return Some(LauncherEvent {
                                    action: LauncherAction::OpenCollection,
                                    path: Some(item.id),
                                    settings: None,
                                });
                            }
                            self.activate_collection(catalog, &item.id);
                        }
                    }
                }
            }
        }

        None
    }

    fn update_home_scroll(&mut self, held: &PadState, frame_now: Instant, count: usize) {
        let delta = self
            .home_scroll
            .last_frame_at
            .map_or(Duration::ZERO, |previous| {
                frame_now.saturating_duration_since(previous)
            });
        self.home_scroll.last_frame_at = Some(frame_now);

        let dir = if self.crt_layout || self.portrait_layout {
            i32::from(held.dpad_down || held.dpad_right) - i32::from(held.dpad_up || held.dpad_left)
        } else {
            i32::from(held.dpad_right) - i32::from(held.dpad_left)
        };
        let previous_dir = self.home_scroll.held_dir;
        if dir == 0 {
            let settle_direction = if previous_dir != 0 {
                previous_dir
            } else {
                self.home_scroll.settle_direction
            };
            if previous_dir != 0 && self.home_scroll.active {
                let target = home_directional_spring_target(
                    self.home_scroll_animation.value(),
                    self.home_scroll_animation.velocity(),
                    count,
                    previous_dir,
                    self.home_scroll_animation
                        .configuration()
                        .angular_frequency(),
                );
                retarget_home_spring_monotonically(&mut self.home_scroll_animation, target);
            }
            self.home_scroll = HomeScrollState {
                last_frame_at: Some(frame_now),
                cursor_px: self.selected as f64 * home_tile_pitch() as f64,
                settle_direction,
                ..HomeScrollState::default()
            };
            self.home_scroll_animation.advance(delta);
            clamp_home_spring_at_target(
                &mut self.home_scroll_animation,
                self.home_scroll.settle_direction,
            );
            self.scroll_x = self
                .home_scroll_animation
                .value()
                .round()
                .clamp(0.0, home_max_scroll(count) as f64) as i32;
            return;
        }

        if dir != previous_dir {
            if (self.home_scroll_animation.value() - self.scroll_x as f64).abs() > 1.0 {
                self.home_scroll_animation.snap_to(self.scroll_x as f64);
            }
            self.home_scroll = HomeScrollState {
                held_dir: dir,
                hold_started_at: Some(frame_now),
                last_frame_at: Some(frame_now),
                active: false,
                cursor_px: self.selected as f64 * home_tile_pitch() as f64,
                motion_velocity: self.home_scroll_animation.velocity(),
                settle_direction: 0,
            };
            if dir < 0 && self.selected > 0 {
                self.selected -= 1;
            } else if dir > 0 && self.selected + 1 < count {
                self.selected += 1;
            }
            self.home_scroll.cursor_px = self.selected as f64 * home_tile_pitch() as f64;
            let mut target = self.home_scroll_animation.target().round() as i32;
            keep_home_visible(self.selected, &mut target, count);
            // Selection is authoritative immediately. Keep ordinary moves
            // animation-free, but smoothly move the retained rail when the
            // focus crosses a viewport edge and every visible card must shift.
            if target == self.scroll_x {
                self.home_scroll_animation.snap_to(target as f64);
            } else {
                retarget_home_spring_monotonically(&mut self.home_scroll_animation, target as f64);
            }
            return;
        }

        if !self.home_scroll.active {
            self.home_scroll_animation.advance(delta);
            self.scroll_x = self.home_scroll_animation.value().round() as i32;
        }

        if !self.home_scroll.active
            && self.home_scroll.hold_started_at.is_some_and(|started| {
                frame_now.saturating_duration_since(started) >= HOME_SCROLL_HOLD_DELAY
            })
        {
            self.home_scroll.active = true;
            self.home_scroll.cursor_px = self.selected as f64 * home_tile_pitch() as f64;
            self.home_scroll.motion_velocity = self.home_scroll_animation.velocity();
        }
        if !self.home_scroll.active {
            return;
        }

        let seconds = delta.as_secs_f64().clamp(0.0, 0.1);
        let desired_velocity = self.home_scroll.held_dir as f64 * HOME_SCROLL_SPEED_PX_PER_SECOND;
        let velocity_delta = desired_velocity - self.home_scroll.motion_velocity;
        let max_velocity_delta = HOME_SCROLL_ACCELERATION_PX_PER_SECOND_SQUARED * seconds;
        let motion_velocity = self.home_scroll.motion_velocity
            + velocity_delta.clamp(-max_velocity_delta, max_velocity_delta);
        self.home_scroll.motion_velocity = motion_velocity;
        let max_scroll = home_max_scroll(count) as f64;
        let value =
            (self.home_scroll_animation.value() + motion_velocity * seconds).clamp(0.0, max_scroll);
        let velocity = if value == 0.0 || value == max_scroll {
            0.0
        } else {
            motion_velocity
        };
        self.home_scroll_animation.set_state(value, velocity);
        self.home_scroll_animation.set_target(value);
        self.scroll_x = value.round() as i32;

        let max_cursor = count.saturating_sub(1) as f64 * home_tile_pitch() as f64;
        self.home_scroll.cursor_px =
            (self.home_scroll.cursor_px + motion_velocity * seconds).clamp(0.0, max_cursor);
        self.selected = ((self.home_scroll.cursor_px + home_tile_pitch() as f64 / 2.0)
            / home_tile_pitch() as f64)
            .floor()
            .clamp(0.0, count.saturating_sub(1) as f64) as usize;
    }

    fn handle_arcade(
        &mut self,
        input: NavigationInput<'_>,
        catalog: &ArcadeCatalog,
        emit_navigation_intents: bool,
    ) -> Option<LauncherEvent> {
        let NavigationInput {
            pressed,
            released: _,
            held,
            tick_continuous,
            frame_now,
        } = input;
        let collection_id = self.active_collection_scope_id(catalog).to_string();
        let count = self.active_arcade_game_count(catalog, &collection_id);

        if self.arcade_filter.drawer_open {
            return self.handle_arcade_filter(input, catalog, &collection_id);
        }

        if self.arcade_search.is_active(&self.arcade_filter.active) {
            return self.handle_arcade_search(
                pressed,
                held,
                tick_continuous,
                frame_now,
                catalog,
                &collection_id,
            );
        }

        if pressed.btn_home {
            if emit_navigation_intents {
                return Some(LauncherEvent {
                    action: LauncherAction::NavigateHome,
                    path: None,
                    settings: None,
                });
            }
            self.leave_arcade(true, &collection_id);
            return None;
        }
        if pressed.btn_b {
            if emit_navigation_intents {
                return Some(LauncherEvent {
                    action: LauncherAction::NavigateBack,
                    path: None,
                    settings: None,
                });
            }
            if self.return_arcade_to_system_hub() {
                return None;
            }
            self.leave_arcade(false, &collection_id);
            return None;
        }

        if count == 0 {
            if pressed.dpad_left {
                self.open_arcade_filter(catalog, &collection_id);
            }
            return None;
        }

        if pressed.dpad_left {
            self.open_arcade_alphabet(catalog, &collection_id);
            return None;
        }

        if self.arcade.selected >= count {
            self.arcade.selected = count - 1;
            self.arcade.snap_to_selected();
        }

        if tick_continuous {
            let dir = arcade_dpad_dir(held);
            let previous_dir = self.arcade.scroll.held_dir;
            self.arcade
                .handle_direction_input(dir, previous_dir, frame_now, count);
            self.arcade.tick(count, frame_now);
        }

        if pressed.btn_a {
            return self
                .active_arcade_game_at(catalog, &collection_id, self.arcade.selected)
                .map(|game| LauncherEvent {
                    action: LauncherAction::LaunchGame,
                    path: Some(game.mra_path.to_string()),
                    settings: None,
                });
        }

        if pressed.btn_x
            && let Some(game) =
                self.active_arcade_game_at(catalog, &collection_id, self.arcade.selected)
        {
            let launch_ref = game.mra_path.to_string();
            self.confirm_action = Some(if self.favourite_launch_refs.contains(&launch_ref) {
                ConfirmAction::RemoveFavourite
            } else {
                ConfirmAction::AddFavourite
            });
            self.confirm_selected = 0;
            self.pending_game_action_path = Some(launch_ref);
        }

        None
    }

    fn handle_arcade_search(
        &mut self,
        pressed: &PadState,
        held: &PadState,
        tick_continuous: bool,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> Option<LauncherEvent> {
        self.ensure_arcade_search_results(catalog, system_id);
        let count = self.active_arcade_game_count(catalog, system_id);
        if pressed.btn_home {
            self.leave_arcade(true, system_id);
            return None;
        }
        match self.arcade_search.pane {
            ArcadeSearchPane::Keyboard => {
                if pressed.btn_b {
                    if self.arcade_search.query.is_empty() {
                        self.apply_arcade_filter(catalog, system_id, ArcadeFilter::All);
                    } else {
                        self.arcade_search.query.pop();
                        self.refresh_arcade_search_results(catalog, system_id);
                    }
                    return None;
                }
                if pressed.btn_y {
                    self.accept_arcade_search_suggestion(catalog, system_id);
                    return None;
                }
                if pressed.dpad_left {
                    self.move_arcade_search_key(-1, 0);
                }
                if pressed.dpad_right {
                    if search_key_is_row_end(self.arcade_search.selected_key) && count > 0 {
                        self.arcade_search.pane = ArcadeSearchPane::Results;
                    } else {
                        self.move_arcade_search_key(1, 0);
                    }
                }
                if pressed.dpad_up {
                    if self.portrait_layout
                        && self.arcade_search.selected_key < ARCADE_SEARCH_KEY_COLUMNS
                        && count > 0
                    {
                        self.arcade_search.pane = ArcadeSearchPane::Results;
                    } else {
                        self.move_arcade_search_key(0, -1);
                    }
                }
                if pressed.dpad_down {
                    self.move_arcade_search_key(0, 1);
                }
                if pressed.btn_a {
                    self.activate_arcade_search_key(catalog, system_id);
                }
            }
            ArcadeSearchPane::Results => {
                if pressed.btn_b || pressed.dpad_left {
                    self.arcade_search.pane = ArcadeSearchPane::Keyboard;
                    return None;
                }
                if count == 0 {
                    self.arcade_search.pane = ArcadeSearchPane::Keyboard;
                    self.arcade.reset();
                    return None;
                }
                if self.arcade.selected >= count {
                    self.arcade.selected = count - 1;
                    self.arcade.snap_to_selected();
                }
                if self.portrait_layout && self.arcade.selected + 1 >= count && pressed.dpad_down {
                    self.arcade_search.pane = ArcadeSearchPane::Keyboard;
                    return None;
                }
                if tick_continuous {
                    let dir = arcade_dpad_dir(held);
                    let previous_dir = self.arcade.scroll.held_dir;
                    self.arcade
                        .handle_direction_input(dir, previous_dir, frame_now, count);
                    self.arcade.tick(count, frame_now);
                }
                if pressed.btn_a {
                    return self
                        .active_arcade_game_at(catalog, system_id, self.arcade.selected)
                        .map(|game| LauncherEvent {
                            action: LauncherAction::LaunchGame,
                            path: Some(game.mra_path.to_string()),
                            settings: None,
                        });
                }
            }
        }
        None
    }

    fn handle_arcade_filter(
        &mut self,
        input: NavigationInput<'_>,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> Option<LauncherEvent> {
        let NavigationInput {
            pressed,
            released,
            held,
            tick_continuous,
            frame_now,
        } = input;
        let items = self.arcade_filter_items(catalog, system_id);
        if self.arcade_filter.activation_release_required && (released.btn_a || released.dpad_right)
        {
            self.arcade_filter.activation_release_required = false;
        }
        if pressed.btn_home {
            self.close_arcade_filter();
            self.leave_arcade(true, system_id);
            return None;
        }
        if pressed.btn_b {
            self.back_out_of_arcade_filter_level(catalog, system_id, true);
            return None;
        }
        if pressed.dpad_left {
            self.back_out_of_arcade_filter_level(catalog, system_id, false);
            return None;
        }
        let activation_requested = !self.arcade_filter.activation_release_required
            && (pressed.dpad_right || pressed.btn_a);
        if activation_requested {
            self.activate_arcade_filter_selection(catalog, system_id, &items);
            return None;
        }
        if !items.is_empty() && tick_continuous {
            let dir = arcade_dpad_dir(held);
            let previous_dir = self.arcade_filter.scroll.scroll.held_dir;
            self.arcade_filter.scroll.handle_direction_input(
                dir,
                previous_dir,
                frame_now,
                items.len(),
            );
            self.arcade_filter.scroll.tick(items.len(), frame_now);
            self.sync_arcade_filter_from_scroll();
        } else if items.is_empty() {
            self.arcade_filter.scroll.reset();
            self.sync_arcade_filter_from_scroll();
        }
        None
    }

    fn handle_settings(&mut self, pressed: &PadState) -> Option<LauncherEvent> {
        if self.display_combo_open {
            let count = settings_display_resolution_count();
            if pressed.btn_b {
                self.display_combo_open = false;
                self.display_highlighted =
                    settings_display_selection_index(self.display_selected).unwrap_or(0);
                return None;
            }
            if pressed.dpad_down && self.display_highlighted + 1 < count {
                self.display_highlighted += 1;
            }
            if pressed.dpad_up && self.display_highlighted > 0 {
                self.display_highlighted -= 1;
            }
            if pressed.btn_a {
                self.display_combo_open = false;
                let highlighted = settings_display_resolution(self.display_highlighted)
                    .expect("highlighted display resolution remains in range");
                if DISPLAY_RESOLUTIONS
                    .get(self.display_selected)
                    .is_some_and(|selected| selected.id == highlighted.id)
                {
                    return None;
                }
                return Some(LauncherEvent {
                    action: LauncherAction::ApplyDisplayResolution,
                    path: Some(highlighted.id.to_owned()),
                    settings: None,
                });
            }
            return None;
        }
        if self.orientation_combo_open {
            let count = ScreenOrientation::ALL.len();
            if pressed.btn_b {
                self.orientation_combo_open = false;
                self.orientation_highlighted = self.orientation_selected.min(count - 1);
                return None;
            }
            if pressed.dpad_down && self.orientation_highlighted + 1 < count {
                self.orientation_highlighted += 1;
            }
            if pressed.dpad_up && self.orientation_highlighted > 0 {
                self.orientation_highlighted -= 1;
            }
            if pressed.btn_a {
                self.orientation_combo_open = false;
                if self.orientation_highlighted == self.orientation_selected {
                    return None;
                }
                return Some(LauncherEvent {
                    action: LauncherAction::ApplyScreenOrientation,
                    path: Some(
                        ScreenOrientation::ALL[self.orientation_highlighted]
                            .id()
                            .into(),
                    ),
                    settings: None,
                });
            }
            return None;
        }
        if pressed.btn_home {
            self.go_root();
            return None;
        }
        if pressed.btn_b {
            self.screen = Screen::Home;
            self.restore_current_menu_view();
            return None;
        }
        if pressed.dpad_down && self.settings_selected < SETTINGS_MAX_SELECTED {
            self.settings_selected += 1;
        }
        if pressed.dpad_up && self.settings_selected > 0 {
            self.settings_selected -= 1;
        }
        if pressed.btn_a {
            if self.settings_selected == SETTINGS_DISPLAY_SELECTED {
                self.display_combo_open = true;
                self.display_highlighted =
                    settings_display_selection_index(self.display_selected).unwrap_or(0);
                return None;
            }
            if self.settings_selected == SETTINGS_ORIENTATION_SELECTED {
                self.orientation_combo_open = true;
                self.orientation_highlighted = self.orientation_selected;
                return None;
            }
            if self.settings_selected == SETTINGS_SCREENSAVER_SELECTED {
                self.screensaver_selected = 0;
                self.screen = Screen::Screensaver;
                return None;
            }
            if self.settings_selected == SETTINGS_REDUCE_MOTION_SELECTED {
                let mut next = self.settings.clone();
                next.reduce_motion = !next.reduce_motion;
                self.settings = next.clone();
                return Some(LauncherEvent {
                    action: LauncherAction::PersistSettings,
                    path: None,
                    settings: Some(next),
                });
            }
            if self.settings_selected == SETTINGS_ABOUT_SELECTED {
                self.about_selected = 0;
                self.screen = Screen::About;
                return None;
            }
            self.confirm_selected = 0;
            self.confirm_action = Some(match self.settings_selected {
                SETTINGS_EXIT_SELECTED => ConfirmAction::ExitToMister,
                SETTINGS_REBUILD_SELECTED => ConfirmAction::RebuildDatabase,
                _ => return None,
            });
        }
        None
    }

    fn handle_about(&mut self, pressed: &PadState) -> Option<LauncherEvent> {
        if pressed.btn_home {
            self.go_root();
            return None;
        }
        if pressed.btn_b {
            self.screen = Screen::Settings;
            return None;
        }
        if pressed.dpad_down && self.about_selected < ABOUT_MAX_SELECTED {
            self.about_selected += 1;
        }
        if pressed.dpad_up && self.about_selected > 0 {
            self.about_selected -= 1;
        }
        if pressed.btn_a {
            if self.about_selected == 0 {
                self.screen = Screen::Info;
            } else {
                self.licenses_selected = 0;
                self.licenses_expanded = false;
                self.licenses_scroll.reset();
                self.screen = Screen::Licenses;
            }
        }
        None
    }

    fn handle_screensaver_settings(&mut self, pressed: &PadState) -> Option<LauncherEvent> {
        if pressed.btn_home {
            self.go_root();
            return None;
        }
        if pressed.btn_b {
            self.screen = Screen::Settings;
            return None;
        }
        if pressed.dpad_down && self.screensaver_selected < SCREENSAVER_SETTINGS_MAX_SELECTED {
            self.screensaver_selected += 1;
        }
        if pressed.dpad_up && self.screensaver_selected > 0 {
            self.screensaver_selected -= 1;
        }
        if !pressed.btn_a {
            return None;
        }
        if self.screensaver_selected == 2 {
            return Some(LauncherEvent {
                action: LauncherAction::PreviewScreensaver,
                path: None,
                settings: None,
            });
        }
        let mut next = self.settings.clone();
        if self.screensaver_selected == 0 {
            next.screensaver_enabled = !next.screensaver_enabled;
        } else if next.screensaver_enabled {
            next.screensaver_delay_minutes = next.screensaver_delay_minutes % 10 + 1;
        } else {
            return None;
        }
        self.settings = next.clone();
        Some(LauncherEvent {
            action: LauncherAction::PersistSettings,
            path: None,
            settings: Some(next),
        })
    }

    fn handle_licenses(
        &mut self,
        pressed: &PadState,
        held: &PadState,
        tick_continuous: bool,
        frame_now: Instant,
    ) -> Option<LauncherEvent> {
        if pressed.btn_home {
            self.licenses_expanded = false;
            self.licenses_scroll.reset();
            self.go_root();
            return None;
        }
        if self.licenses_expanded {
            if pressed.btn_a || pressed.btn_b {
                self.licenses_expanded = false;
                self.licenses_scroll.reset();
            } else {
                let count = crate::licenses::max_scroll_line(self.licenses_selected) + 1;
                if tick_continuous {
                    let previous_dir = self.licenses_scroll.scroll.held_dir;
                    self.licenses_scroll.handle_direction_input(
                        arcade_dpad_dir(held),
                        previous_dir,
                        frame_now,
                        count,
                    );
                    self.licenses_scroll.tick(count, frame_now);
                }
            }
            return None;
        }
        if pressed.btn_b {
            self.screen = Screen::About;
            self.licenses_scroll.reset();
            return None;
        }
        if pressed.dpad_down && self.licenses_selected < LICENSES_MAX_SELECTED {
            self.licenses_selected += 1;
        }
        if pressed.dpad_up && self.licenses_selected > 0 {
            self.licenses_selected -= 1;
        }
        if pressed.btn_a {
            self.licenses_expanded = true;
            self.licenses_scroll.reset();
        }
        None
    }

    fn handle_settings_subscreen(&mut self, pressed: &PadState) {
        if pressed.btn_home {
            self.go_root();
        } else if pressed.btn_b {
            self.screen = Screen::About;
        }
    }

    pub fn licenses_scroll_y(&self) -> i32 {
        self.licenses_scroll.scroll_y
    }

    pub fn licenses_scroll_active(&self) -> bool {
        self.screen == Screen::Licenses
            && self.licenses_expanded
            && self.licenses_scroll.is_scroll_active()
    }

    fn handle_confirm(&mut self, pressed: &PadState) -> Option<LauncherEvent> {
        let home_pressed = pressed.btn_home;
        if self.confirm_action == Some(ConfirmAction::DisplayResolutionError) {
            if pressed.btn_a || pressed.btn_b || home_pressed {
                self.confirm_action = None;
                self.confirm_selected = 0;
                self.display_error = None;
            }
            return None;
        }
        if pressed.btn_b || home_pressed {
            if self.confirm_action == Some(ConfirmAction::DisplayResolution) {
                self.confirm_action = None;
                self.confirm_selected = 0;
                return Some(LauncherEvent {
                    action: LauncherAction::CancelDisplayResolution,
                    path: None,
                    settings: None,
                });
            }
            if self.confirm_action == Some(ConfirmAction::ScreenOrientation) {
                self.confirm_action = None;
                self.confirm_selected = 0;
                return Some(LauncherEvent {
                    action: LauncherAction::CancelScreenOrientation,
                    path: None,
                    settings: None,
                });
            }
            if self.confirm_action == Some(ConfirmAction::LibraryChanged) {
                self.confirm_action = None;
                self.confirm_selected = 0;
                if home_pressed {
                    self.go_root();
                }
                return Some(LauncherEvent {
                    action: LauncherAction::ContinueWithStaleLibrary,
                    path: None,
                    settings: None,
                });
            }
            self.confirm_action = None;
            self.confirm_selected = 0;
            self.pending_game_action_path = None;
            if home_pressed {
                self.go_root();
            }
            return None;
        }
        let max_selected = confirm_max_selected(self.confirm_action);
        if self.confirm_selected > max_selected {
            self.confirm_selected = max_selected;
        }
        if pressed.dpad_left && self.confirm_selected > 0 {
            self.confirm_selected -= 1;
        }
        if pressed.dpad_right && self.confirm_selected < max_selected {
            self.confirm_selected += 1;
        }
        if pressed.btn_a {
            let action = self.confirm_action;
            let selected = self.confirm_selected;
            let confirmed = match action {
                Some(ConfirmAction::ExitToMister) => selected == 1,
                Some(ConfirmAction::LibraryChanged) => true,
                Some(ConfirmAction::LibraryUpdateFailed) => false,
                _ => selected == 1,
            };
            if action == Some(ConfirmAction::DisplayResolution)
                && confirmed
                && self.display_confirm_busy
            {
                return None;
            }
            self.confirm_action = None;
            self.confirm_selected = 0;
            if confirmed {
                return match action {
                    Some(ConfirmAction::ExitToMister) => Some(LauncherEvent {
                        action: LauncherAction::ExitToMister,
                        path: None,
                        settings: None,
                    }),
                    Some(ConfirmAction::RebuildDatabase) => Some(LauncherEvent {
                        action: LauncherAction::RebuildDatabase,
                        path: None,
                        settings: None,
                    }),
                    Some(ConfirmAction::Restart) => Some(LauncherEvent {
                        action: LauncherAction::Restart,
                        path: None,
                        settings: None,
                    }),
                    Some(ConfirmAction::LibraryChanged) => Some(LauncherEvent {
                        action: if selected == 0 {
                            LauncherAction::ContinueWithStaleLibrary
                        } else {
                            LauncherAction::RebuildLibrary
                        },
                        path: None,
                        settings: None,
                    }),
                    Some(ConfirmAction::LibraryUpdateFailed) => None,
                    Some(ConfirmAction::DisplayResolution) => Some(LauncherEvent {
                        action: LauncherAction::ConfirmDisplayResolution,
                        path: None,
                        settings: None,
                    }),
                    Some(ConfirmAction::DisplayResolutionError) => None,
                    Some(ConfirmAction::ScreenOrientation) => Some(LauncherEvent {
                        action: LauncherAction::ConfirmScreenOrientation,
                        path: None,
                        settings: None,
                    }),
                    Some(ConfirmAction::AddFavourite) => Some(LauncherEvent {
                        action: LauncherAction::AddFavourite,
                        path: self.pending_game_action_path.take(),
                        settings: None,
                    }),
                    Some(ConfirmAction::RemoveFavourite) => Some(LauncherEvent {
                        action: LauncherAction::RemoveFavourite,
                        path: self.pending_game_action_path.take(),
                        settings: None,
                    }),
                    None => None,
                };
            }
            if action == Some(ConfirmAction::DisplayResolution) {
                return Some(LauncherEvent {
                    action: LauncherAction::CancelDisplayResolution,
                    path: None,
                    settings: None,
                });
            }
            if action == Some(ConfirmAction::ScreenOrientation) {
                return Some(LauncherEvent {
                    action: LauncherAction::CancelScreenOrientation,
                    path: None,
                    settings: None,
                });
            }
            self.pending_game_action_path = None;
        }
        None
    }

    fn save_game_list_state(&mut self, system_id: &str) {
        let memory = GameListMemory {
            selected: self.arcade.selected,
            scroll_y: self.arcade.selected as i32 * self.arcade.row_height,
        };
        self.game_list_memory.insert(
            collection_filter_memory_key(system_id, &self.arcade_filter.active),
            memory,
        );
        self.collection_filters
            .insert(system_id.to_string(), self.arcade_filter.active.clone());
        if matches!(self.arcade_filter.active, ArcadeFilter::Search) {
            self.collection_search_queries
                .insert(system_id.to_string(), self.arcade_search.query.clone());
        }
    }

    fn restore_game_list_state(&mut self, system_id: &str, count: usize) {
        self.arcade_filter.drawer_open = false;
        self.arcade_filter.level = ArcadeFilterLevel::Top;
        let key = collection_filter_memory_key(system_id, &self.arcade_filter.active);
        if let Some(memory) = self.game_list_memory.get(&key).copied() {
            self.arcade
                .restore_position(memory.selected, memory.scroll_y, count);
        } else {
            self.arcade.reset();
        }
    }

    pub fn active_arcade_game_count(&self, catalog: &ArcadeCatalog, system_id: &str) -> usize {
        if self.arcade_user_list_mode != ArcadeUserListMode::Games {
            return self.user_list_indexes.len();
        }
        let system_id = self.effective_collection_id(system_id);
        if self.arcade_search.is_active(&self.arcade_filter.active)
            && !self.arcade_search.query.is_empty()
            && self.arcade_search.result_system_id == system_id
            && self.arcade_search.result_query == self.arcade_search.query
        {
            self.arcade_search.results.len()
        } else {
            catalog.filtered_game_count(system_id, &self.arcade_filter.active)
        }
    }

    pub fn set_favourite_launch_refs(&mut self, refs: impl IntoIterator<Item = String>) {
        let refs = refs.into_iter().collect::<HashSet<_>>();
        if self.favourite_launch_refs != refs {
            self.favourite_launch_refs = refs;
            self.favourite_launch_refs_revision =
                self.favourite_launch_refs_revision.wrapping_add(1);
        }
    }

    pub fn set_user_game_refs(
        &mut self,
        catalog: &ArcadeCatalog,
        favourites: impl IntoIterator<Item = String>,
        recents: Vec<String>,
    ) {
        self.set_favourite_launch_refs(favourites);
        self.recent_launch_refs = recents;
        self.rebuild_user_list_indexes(catalog);
    }

    pub fn set_arcade_user_list_mode(&mut self, catalog: &ArcadeCatalog, mode: ArcadeUserListMode) {
        self.arcade_user_list_mode = mode;
        self.rebuild_user_list_indexes(catalog);
        self.arcade_filter.active = ArcadeFilter::All;
        self.arcade_filter.drawer_open = false;
        self.arcade_search = ArcadeSearchState::new();
        self.arcade.reset();
    }

    pub fn return_arcade_to_system_hub(&mut self) -> bool {
        if self
            .active_collection_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case("snes"))
        {
            self.screen = Screen::SystemHub;
            true
        } else {
            false
        }
    }

    pub fn arcade_user_list_mode(&self) -> ArcadeUserListMode {
        self.arcade_user_list_mode
    }

    fn rebuild_user_list_indexes(&mut self, catalog: &ArcadeCatalog) {
        let snes_games = catalog.system_game_view("snes");
        let refs: Vec<&str> = match self.arcade_user_list_mode {
            ArcadeUserListMode::Games => Vec::new(),
            ArcadeUserListMode::Recent => {
                self.recent_launch_refs.iter().map(String::as_str).collect()
            }
            ArcadeUserListMode::Favourites => snes_games
                .iter()
                .map(|game| game.mra_path.as_ref())
                .filter(|launch_ref| self.favourite_launch_refs.contains(*launch_ref))
                .collect(),
        };
        self.user_list_indexes = refs
            .into_iter()
            .filter_map(|launch_ref| {
                snes_games
                    .iter()
                    .position(|game| game.mra_path.as_ref() == launch_ref)
            })
            .collect();
    }

    pub fn favourite_count(&self) -> usize {
        self.favourite_launch_refs.len()
    }

    pub fn recent_count(&self) -> usize {
        self.recent_launch_refs.len()
    }

    pub fn apply_favourite_state(&mut self, launch_ref: &str, favourite: bool) {
        let changed = if favourite {
            self.favourite_launch_refs.insert(launch_ref.to_string())
        } else {
            self.favourite_launch_refs.remove(launch_ref)
        };
        if changed {
            self.favourite_launch_refs_revision =
                self.favourite_launch_refs_revision.wrapping_add(1);
        }
    }

    pub fn reconcile_favourite_state(
        &mut self,
        catalog: &ArcadeCatalog,
        launch_ref: &str,
        favourite: bool,
    ) {
        self.apply_favourite_state(launch_ref, favourite);
        self.rebuild_user_list_indexes(catalog);
    }

    pub fn is_favourite_launch_ref(&self, launch_ref: &str) -> bool {
        self.favourite_launch_refs.contains(launch_ref)
    }

    pub fn favourite_launch_refs(&self) -> impl Iterator<Item = &str> {
        self.favourite_launch_refs.iter().map(String::as_str)
    }

    pub fn favourite_launch_refs_revision(&self) -> u64 {
        self.favourite_launch_refs_revision
    }

    #[must_use]
    pub fn arcade_search_result_count(&self) -> usize {
        self.arcade_search.results.len()
    }

    pub fn active_arcade_game_view<'a>(
        &'a self,
        catalog: &'a ArcadeCatalog,
        system_id: &str,
    ) -> crate::arcade_catalog::ArcadeGameView<'a> {
        if self.arcade_user_list_mode != ArcadeUserListMode::Games {
            return catalog.indexed_system_game_view("snes", &self.user_list_indexes);
        }
        let system_id = self.effective_collection_id(system_id);
        if self.arcade_search.is_active(&self.arcade_filter.active)
            && !self.arcade_search.query.is_empty()
            && self.arcade_search.result_system_id == system_id
            && self.arcade_search.result_query == self.arcade_search.query
        {
            catalog.indexed_system_game_view(system_id, &self.arcade_search.results)
        } else {
            catalog.filtered_game_view(system_id, &self.arcade_filter.active)
        }
    }

    pub fn active_arcade_game_at<'a>(
        &'a self,
        catalog: &'a ArcadeCatalog,
        system_id: &str,
        index: usize,
    ) -> Option<&'a crate::arcade_catalog::ArcadeGameEntry> {
        self.active_arcade_game_view(catalog, system_id).get(index)
    }

    pub fn arcade_filter_items(
        &self,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> Vec<ArcadeDrawerItem> {
        let system_id = self.effective_collection_id(system_id);
        match self.arcade_filter.level {
            ArcadeFilterLevel::Alphabet => self.arcade_alphabet_items(catalog, system_id),
            ArcadeFilterLevel::Top => self.arcade_filter_top_items(catalog, system_id),
            ArcadeFilterLevel::Categories => filter_option_items(
                catalog.category_options(system_id),
                |label| Some(ArcadeFilter::Category(label.to_string())),
                &self.arcade_filter.active,
            ),
            ArcadeFilterLevel::Decades => filter_option_items(
                catalog.decade_options(system_id),
                |label| decade_from_label(label).map(ArcadeFilter::Decade),
                &self.arcade_filter.active,
            ),
            ArcadeFilterLevel::Manufacturers => filter_option_items(
                catalog.manufacturer_options(system_id),
                |label| Some(ArcadeFilter::Manufacturer(label.to_string())),
                &self.arcade_filter.active,
            ),
            ArcadeFilterLevel::Players => filter_option_items(
                catalog.player_options(system_id),
                |label| player_count_from_label(label).map(ArcadeFilter::Players),
                &self.arcade_filter.active,
            ),
            ArcadeFilterLevel::Controls => filter_option_items(
                catalog.control_options(system_id),
                |label| Some(ArcadeFilter::Control(label.to_string())),
                &self.arcade_filter.active,
            ),
        }
    }

    fn arcade_filter_top_items(
        &self,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> Vec<ArcadeDrawerItem> {
        self.arcade_filter_top_group_items(catalog, system_id)
            .into_iter()
            .map(|row| row.item)
            .collect()
    }

    fn arcade_filter_top_group_items(
        &self,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> Vec<ArcadeTopDrawerItem> {
        let mut items = vec![
            ArcadeTopDrawerItem {
                group: ArcadeFilterGroup::Games,
                item: ArcadeDrawerItem {
                    label: "Games A-Z".to_string(),
                    count: catalog.system_game_count(system_id),
                    active: self.arcade_filter.active == ArcadeFilter::All,
                },
            },
            ArcadeTopDrawerItem {
                group: ArcadeFilterGroup::Search,
                item: ArcadeDrawerItem {
                    label: "Search".to_string(),
                    count: catalog.system_game_count(system_id),
                    active: matches!(self.arcade_filter.active, ArcadeFilter::Search),
                },
            },
        ];
        if catalog.category_option_count(system_id) > 1 {
            items.push(ArcadeTopDrawerItem {
                group: ArcadeFilterGroup::Categories,
                item: ArcadeDrawerItem {
                    label: "Categories".to_string(),
                    count: catalog.category_option_count(system_id),
                    active: matches!(self.arcade_filter.active, ArcadeFilter::Category(_)),
                },
            });
        }
        if catalog.decade_option_count(system_id) > 1 {
            items.push(ArcadeTopDrawerItem {
                group: ArcadeFilterGroup::Decades,
                item: ArcadeDrawerItem {
                    label: "Decades".to_string(),
                    count: catalog.decade_option_count(system_id),
                    active: matches!(self.arcade_filter.active, ArcadeFilter::Decade(_)),
                },
            });
        }
        if catalog.manufacturer_option_count(system_id) > 1 {
            items.push(ArcadeTopDrawerItem {
                group: ArcadeFilterGroup::Manufacturers,
                item: ArcadeDrawerItem {
                    label: "Manufacturer".to_string(),
                    count: catalog.manufacturer_option_count(system_id),
                    active: matches!(self.arcade_filter.active, ArcadeFilter::Manufacturer(_)),
                },
            });
        }
        if catalog.player_option_count(system_id) > 1 {
            items.push(ArcadeTopDrawerItem {
                group: ArcadeFilterGroup::Players,
                item: ArcadeDrawerItem {
                    label: "Players".to_string(),
                    count: catalog.player_option_count(system_id),
                    active: matches!(self.arcade_filter.active, ArcadeFilter::Players(_)),
                },
            });
        }
        if catalog.control_option_count(system_id) > 1 {
            items.push(ArcadeTopDrawerItem {
                group: ArcadeFilterGroup::Controls,
                item: ArcadeDrawerItem {
                    label: "Controls".to_string(),
                    count: catalog.control_option_count(system_id),
                    active: matches!(self.arcade_filter.active, ArcadeFilter::Control(_)),
                },
            });
        }
        items
    }

    fn arcade_filter_top_group_index(
        &self,
        catalog: &ArcadeCatalog,
        system_id: &str,
        group: ArcadeFilterGroup,
    ) -> usize {
        self.arcade_filter_top_group_items(catalog, system_id)
            .iter()
            .position(|row| row.group == group)
            .unwrap_or(0)
    }

    fn available_arcade_filter(
        &self,
        catalog: &ArcadeCatalog,
        system_id: &str,
        filter: &ArcadeFilter,
    ) -> ArcadeFilter {
        let Some(resolved) = catalog.resolve_filter(system_id, filter) else {
            return ArcadeFilter::All;
        };
        let group_is_visible = match resolved {
            ArcadeFilter::All | ArcadeFilter::Search => true,
            ArcadeFilter::Category(_) => catalog.category_option_count(system_id) > 1,
            ArcadeFilter::Decade(_) => catalog.decade_option_count(system_id) > 1,
            ArcadeFilter::Manufacturer(_) => catalog.manufacturer_option_count(system_id) > 1,
            ArcadeFilter::Players(_) => catalog.player_option_count(system_id) > 1,
            ArcadeFilter::Control(_) => catalog.control_option_count(system_id) > 1,
        };
        if group_is_visible {
            resolved
        } else {
            ArcadeFilter::All
        }
    }

    fn resolve_arcade_filter_for_collection(
        &mut self,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> bool {
        let resolved = self.available_arcade_filter(catalog, system_id, &self.arcade_filter.active);
        if resolved == self.arcade_filter.active {
            return false;
        }
        self.arcade_filter.active = resolved.clone();
        self.collection_filters
            .insert(system_id.to_string(), resolved);
        true
    }

    fn open_arcade_alphabet(&mut self, catalog: &ArcadeCatalog, system_id: &str) {
        if self.active_arcade_game_count(catalog, system_id) == 0 {
            self.open_arcade_filter(catalog, system_id);
            return;
        }
        self.save_game_list_state(system_id);
        self.arcade_filter.drawer_open = true;
        self.arcade_filter.level = ArcadeFilterLevel::Alphabet;
        self.arcade_filter.selected =
            self.arcade_alphabet_selected_for_current_game(catalog, system_id);
        let items = self.arcade_alphabet_items(catalog, system_id);
        self.snap_arcade_filter_scroll(items.len());
    }

    fn open_arcade_filter(&mut self, catalog: &ArcadeCatalog, system_id: &str) {
        self.save_game_list_state(system_id);
        self.resolve_arcade_filter_for_collection(catalog, system_id);
        self.arcade_filter.drawer_open = true;
        self.arcade_filter.level = self.arcade_filter.active_level();
        self.arcade_filter.selected = if self.arcade_filter.level == ArcadeFilterLevel::Top {
            self.arcade_filter_top_group_index(
                catalog,
                system_id,
                self.arcade_filter.active_group(),
            )
        } else {
            0
        };
        let items = self.arcade_filter_items(catalog, system_id);
        if self.arcade_filter.level != ArcadeFilterLevel::Top
            && let Some(active_idx) = items.iter().position(|item| item.active)
        {
            self.arcade_filter.selected = active_idx;
        }
        self.snap_arcade_filter_scroll(items.len());
    }

    fn close_arcade_filter(&mut self) {
        self.arcade_filter.drawer_open = false;
        self.arcade_filter.level = ArcadeFilterLevel::Top;
        self.arcade_filter.activation_release_required = false;
    }

    fn back_out_of_arcade_filter_level(
        &mut self,
        catalog: &ArcadeCatalog,
        system_id: &str,
        leave_arcade_from_top: bool,
    ) {
        if self.arcade_filter.level == ArcadeFilterLevel::Top {
            if leave_arcade_from_top {
                self.close_arcade_filter();
                self.leave_arcade(false, system_id);
            }
        } else if self.arcade_filter.level == ArcadeFilterLevel::Alphabet {
            self.open_arcade_filter(catalog, system_id);
        } else if let Some(parent) = self.arcade_filter.level.parent() {
            self.arcade_filter.level = parent;
            self.arcade_filter.selected = self.arcade_filter_top_group_index(
                catalog,
                system_id,
                self.arcade_filter.active_group(),
            );
            let top_count = self.arcade_filter_items(catalog, system_id).len();
            self.snap_arcade_filter_scroll(top_count);
        }
    }

    fn activate_arcade_filter_selection(
        &mut self,
        catalog: &ArcadeCatalog,
        system_id: &str,
        items: &[ArcadeDrawerItem],
    ) {
        if items.is_empty() || self.arcade_filter.selected >= items.len() {
            return;
        }
        match self.arcade_filter.level {
            ArcadeFilterLevel::Alphabet => {
                let label = items[self.arcade_filter.selected].label.clone();
                self.jump_arcade_to_alphabet_group(catalog, system_id, &label);
            }
            ArcadeFilterLevel::Top => match self
                .arcade_filter_top_group_items(catalog, system_id)
                .get(self.arcade_filter.selected)
                .map(|row| row.group)
            {
                Some(ArcadeFilterGroup::Games) => {
                    self.apply_arcade_filter(catalog, system_id, ArcadeFilter::All)
                }
                Some(ArcadeFilterGroup::Search) => self.enter_arcade_search(catalog, system_id),
                Some(ArcadeFilterGroup::Categories) => self.enter_arcade_filter_level(
                    catalog,
                    system_id,
                    ArcadeFilterLevel::Categories,
                ),
                Some(ArcadeFilterGroup::Decades) => {
                    self.enter_arcade_filter_level(catalog, system_id, ArcadeFilterLevel::Decades)
                }
                Some(ArcadeFilterGroup::Manufacturers) => self.enter_arcade_filter_level(
                    catalog,
                    system_id,
                    ArcadeFilterLevel::Manufacturers,
                ),
                Some(ArcadeFilterGroup::Players) => {
                    self.enter_arcade_filter_level(catalog, system_id, ArcadeFilterLevel::Players)
                }
                Some(ArcadeFilterGroup::Controls) => {
                    self.enter_arcade_filter_level(catalog, system_id, ArcadeFilterLevel::Controls)
                }
                _ => {}
            },
            ArcadeFilterLevel::Categories => {
                self.apply_arcade_filter(
                    catalog,
                    system_id,
                    ArcadeFilter::Category(items[self.arcade_filter.selected].label.clone()),
                );
            }
            ArcadeFilterLevel::Decades => {
                if let Some(decade) = decade_from_label(&items[self.arcade_filter.selected].label) {
                    self.apply_arcade_filter(catalog, system_id, ArcadeFilter::Decade(decade));
                }
            }
            ArcadeFilterLevel::Manufacturers => {
                self.apply_arcade_filter(
                    catalog,
                    system_id,
                    ArcadeFilter::Manufacturer(items[self.arcade_filter.selected].label.clone()),
                );
            }
            ArcadeFilterLevel::Players => {
                if let Some(players) =
                    player_count_from_label(&items[self.arcade_filter.selected].label)
                {
                    self.apply_arcade_filter(catalog, system_id, ArcadeFilter::Players(players));
                }
            }
            ArcadeFilterLevel::Controls => {
                self.apply_arcade_filter(
                    catalog,
                    system_id,
                    ArcadeFilter::Control(items[self.arcade_filter.selected].label.clone()),
                );
            }
        }
    }

    fn enter_arcade_filter_level(
        &mut self,
        catalog: &ArcadeCatalog,
        system_id: &str,
        level: ArcadeFilterLevel,
    ) {
        self.arcade_filter.level = level;
        self.arcade_filter.activation_release_required = true;
        self.arcade_filter.selected = 0;
        let items = self.arcade_filter_items(catalog, system_id);
        if let Some(active_idx) = items.iter().position(|item| item.active) {
            self.arcade_filter.selected = active_idx;
        }
        self.snap_arcade_filter_scroll(items.len());
    }

    fn arcade_alphabet_items(
        &self,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> Vec<ArcadeDrawerItem> {
        let mut digit_count = 0usize;
        let mut letter_counts = [0usize; 26];
        for game in self.active_arcade_game_view(catalog, system_id).iter() {
            match arcade_title_group(&game.title) {
                Some(ArcadeTitleGroup::Digits) => digit_count += 1,
                Some(ArcadeTitleGroup::Letter(letter)) => {
                    letter_counts[(letter as u8 - b'A') as usize] += 1;
                }
                None => {}
            }
        }

        let mut items = Vec::new();
        if digit_count > 0 {
            items.push(ArcadeDrawerItem {
                label: "0-9".to_string(),
                count: digit_count,
                active: false,
            });
        }
        for (idx, count) in letter_counts.into_iter().enumerate() {
            if count == 0 {
                continue;
            }
            let letter = (b'A' + idx as u8) as char;
            items.push(ArcadeDrawerItem {
                label: letter.to_string(),
                count,
                active: false,
            });
        }
        items
    }

    fn arcade_alphabet_selected_for_current_game(
        &self,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> usize {
        let Some(game) = self.active_arcade_game_at(catalog, system_id, self.arcade.selected)
        else {
            return 0;
        };
        let Some(group) = arcade_title_group(&game.title) else {
            return 0;
        };
        let label = group.label();
        self.arcade_alphabet_items(catalog, system_id)
            .iter()
            .position(|item| item.label == label)
            .unwrap_or(0)
    }

    fn jump_arcade_to_alphabet_group(
        &mut self,
        catalog: &ArcadeCatalog,
        system_id: &str,
        label: &str,
    ) {
        let Some(target_group) = ArcadeTitleGroup::from_label(label) else {
            return;
        };
        let count = self.active_arcade_game_count(catalog, system_id);
        if let Some(index) = self
            .active_arcade_game_view(catalog, system_id)
            .iter()
            .enumerate()
            .find_map(|(index, game)| {
                (arcade_title_group(&game.title) == Some(target_group)).then_some(index)
            })
        {
            self.arcade
                .restore_position(index, index as i32 * ARCADE_ROW_HEIGHT, count);
        }
        self.close_arcade_filter();
    }

    fn apply_arcade_filter(
        &mut self,
        catalog: &ArcadeCatalog,
        system_id: &str,
        filter: ArcadeFilter,
    ) {
        self.save_game_list_state(system_id);
        self.arcade_filter.active = filter;
        self.collection_filters
            .insert(system_id.to_string(), self.arcade_filter.active.clone());
        let count = catalog.filtered_game_count(system_id, &self.arcade_filter.active);
        let key = collection_filter_memory_key(system_id, &self.arcade_filter.active);
        if let Some(memory) = self.game_list_memory.get(&key).copied() {
            self.arcade
                .restore_position(memory.selected, memory.scroll_y, count);
        } else {
            self.arcade.reset();
        }
        self.close_arcade_filter();
    }

    fn enter_arcade_search(&mut self, catalog: &ArcadeCatalog, system_id: &str) {
        self.save_game_list_state(system_id);
        self.arcade_filter.active = ArcadeFilter::Search;
        self.collection_filters
            .insert(system_id.to_string(), ArcadeFilter::Search);
        self.arcade_search.pane = ArcadeSearchPane::Keyboard;
        self.arcade_search.query = self
            .collection_search_queries
            .get(system_id)
            .cloned()
            .unwrap_or_default();
        self.clear_arcade_search_results(system_id);
        let search_count = catalog.filtered_game_count(system_id, &self.arcade_filter.active);
        let key = collection_filter_memory_key(system_id, &self.arcade_filter.active);
        if let Some(memory) = self.game_list_memory.get(&key).copied() {
            self.arcade
                .restore_position(memory.selected, memory.scroll_y, search_count);
        } else {
            self.arcade.reset();
        }
        self.close_arcade_filter();
    }

    fn ensure_arcade_search_results(&mut self, catalog: &ArcadeCatalog, system_id: &str) {
        if self.arcade_search.query.is_empty() {
            self.clear_arcade_search_results(system_id);
            return;
        }
        if self.arcade_search.result_system_id != system_id
            || self.arcade_search.result_query != self.arcade_search.query
            || self.arcade_search.suggestion_system_id != system_id
            || self.arcade_search.suggestion_query != self.arcade_search.query
        {
            self.refresh_arcade_search_results(catalog, system_id);
        }
    }

    fn clear_arcade_search_results(&mut self, system_id: &str) {
        self.arcade_search.results.clear();
        self.arcade_search.suggestion.clear();
        self.arcade_search.result_system_id.clear();
        self.arcade_search.result_query.clear();
        self.arcade_search.suggestion_system_id = system_id.to_string();
        self.arcade_search.suggestion_query.clear();
        self.arcade_search.status = ArcadeSearchStatus::Idle;
        self.arcade_search.request_pending = false;
        self.arcade_search.request_id = self.arcade_search.request_id.wrapping_add(1);
        self.arcade_search.pane = ArcadeSearchPane::Keyboard;
    }

    fn refresh_arcade_search_results(&mut self, catalog: &ArcadeCatalog, system_id: &str) {
        let effective_collection_id = self.effective_collection_id(system_id).to_string();
        let system_id = effective_collection_id.as_str();
        if self.arcade_search.query.is_empty() {
            self.clear_arcade_search_results(system_id);
            return;
        }
        let Some(results) = catalog.try_search_game_indexes(system_id, &self.arcade_search.query)
        else {
            self.queue_arcade_search_request(system_id);
            return;
        };
        let Some(suggestion) =
            catalog.try_autocomplete_search_word(system_id, &self.arcade_search.query)
        else {
            self.arcade_search.results.clear();
            self.arcade_search.suggestion.clear();
            self.arcade_search.result_system_id = system_id.to_string();
            self.arcade_search.result_query = self.arcade_search.query.clone();
            self.arcade_search.suggestion_system_id = system_id.to_string();
            self.arcade_search.suggestion_query = self.arcade_search.query.clone();
            self.arcade_search.status = ArcadeSearchStatus::Searching;
            self.arcade_search.pane = ArcadeSearchPane::Keyboard;
            self.arcade.reset();
            return;
        };
        self.arcade_search.results = results;
        self.arcade_search.suggestion = suggestion;
        self.arcade_search.status = ArcadeSearchStatus::Ready;
        self.arcade_search.result_system_id = system_id.to_string();
        self.arcade_search.result_query = self.arcade_search.query.clone();
        self.arcade_search.suggestion_system_id = system_id.to_string();
        self.arcade_search.suggestion_query = self.arcade_search.query.clone();
        self.arcade_search.request_pending = false;
        let count = self.arcade_search.results.len();
        if count == 0 {
            self.arcade.reset();
            self.arcade_search.pane = ArcadeSearchPane::Keyboard;
        } else if self.arcade.selected >= count {
            self.arcade.selected = count - 1;
            self.arcade.snap_to_selected();
        } else {
            self.arcade.snap_to_selected();
        }
    }

    fn queue_arcade_search_request(&mut self, system_id: &str) {
        self.arcade_search.results.clear();
        self.arcade_search.suggestion.clear();
        self.arcade_search.result_system_id = system_id.to_string();
        self.arcade_search.result_query = self.arcade_search.query.clone();
        self.arcade_search.suggestion_system_id = system_id.to_string();
        self.arcade_search.suggestion_query = self.arcade_search.query.clone();
        self.arcade_search.status = ArcadeSearchStatus::Searching;
        self.arcade_search.request_id = self.arcade_search.request_id.wrapping_add(1);
        self.arcade_search.request_pending = true;
        self.arcade_search.pane = ArcadeSearchPane::Keyboard;
        self.arcade.reset();
    }

    pub fn take_arcade_search_request(
        &mut self,
        catalog: &ArcadeCatalog,
        catalog_version: usize,
    ) -> Option<ArcadeSearchRequest> {
        if !self.arcade_search.request_pending || self.arcade_search.query.is_empty() {
            return None;
        }
        self.arcade_search.request_pending = false;
        let collection_id = self.arcade_search.result_system_id.clone();
        Some(ArcadeSearchRequest {
            request_id: self.arcade_search.request_id,
            catalog_version,
            system_ids: catalog.search_source_system_ids(&collection_id),
            collection_id,
            query: self.arcade_search.query.clone(),
        })
    }

    pub fn apply_arcade_search_result(
        &mut self,
        catalog: &ArcadeCatalog,
        request: &ArcadeSearchRequest,
        result: mister_magik_catalog::persisted_search::PersistedCollectionSearchResult,
    ) -> bool {
        if request.request_id != self.arcade_search.request_id
            || request.collection_id != self.arcade_search.result_system_id
            || request.query != self.arcade_search.query
        {
            return false;
        }
        let collection_indexes = catalog.collection_game_index_set(&request.collection_id);
        self.arcade_search.results = result
            .matches
            .into_iter()
            .filter_map(|entry| {
                catalog.resolve_system_game_ordinal(&entry.system_id, entry.ordinal)
            })
            .filter(|index| collection_indexes.contains(index))
            .collect();
        self.arcade_search.suggestion = result
            .autocomplete
            .map(|candidate| candidate.word)
            .unwrap_or_default();
        self.arcade_search.status = ArcadeSearchStatus::Ready;
        self.arcade_search.request_pending = false;
        let count = self.arcade_search.results.len();
        if count == 0 {
            self.arcade.reset();
            self.arcade_search.pane = ArcadeSearchPane::Keyboard;
        } else if self.arcade.selected >= count {
            self.arcade.selected = count - 1;
            self.arcade.snap_to_selected();
        } else {
            self.arcade.snap_to_selected();
        }
        true
    }

    pub fn fail_arcade_search_request(&mut self, request: &ArcadeSearchRequest) -> bool {
        if request.request_id != self.arcade_search.request_id
            || request.collection_id != self.arcade_search.result_system_id
            || request.query != self.arcade_search.query
        {
            return false;
        }
        self.arcade_search.results.clear();
        self.arcade_search.suggestion.clear();
        self.arcade_search.status = ArcadeSearchStatus::Failed;
        self.arcade_search.request_pending = false;
        self.arcade.reset();
        true
    }

    pub fn refresh_arcade_search_if_active(&mut self, catalog: &ArcadeCatalog, system_id: &str) {
        if self.arcade_search.is_active(&self.arcade_filter.active) {
            self.refresh_arcade_search_results(catalog, system_id);
        }
    }

    fn move_arcade_search_key(&mut self, dx: isize, dy: isize) {
        let current = self
            .arcade_search
            .selected_key
            .min(ARCADE_SEARCH_KEYS.len() - 1);
        let row = current / ARCADE_SEARCH_KEY_COLUMNS;
        let col = current % ARCADE_SEARCH_KEY_COLUMNS;
        let max_row = (ARCADE_SEARCH_KEYS.len() - 1) / ARCADE_SEARCH_KEY_COLUMNS;
        let new_row = (row as isize + dy).clamp(0, max_row as isize) as usize;
        let row_len = search_row_len(new_row);
        let new_col = (col as isize + dx).clamp(0, row_len.saturating_sub(1) as isize) as usize;
        self.arcade_search.selected_key =
            (new_row * ARCADE_SEARCH_KEY_COLUMNS + new_col).min(ARCADE_SEARCH_KEYS.len() - 1);
    }

    fn activate_arcade_search_key(&mut self, catalog: &ArcadeCatalog, system_id: &str) {
        let key = ARCADE_SEARCH_KEYS[self
            .arcade_search
            .selected_key
            .min(ARCADE_SEARCH_KEYS.len() - 1)];
        match key.action {
            ArcadeSearchKeyAction::Space => self.arcade_search.query.push(' '),
            ArcadeSearchKeyAction::Delete => {
                self.arcade_search.query.pop();
            }
            ArcadeSearchKeyAction::Clear => self.arcade_search.query.clear(),
            ArcadeSearchKeyAction::Append(value) => self.arcade_search.query.push_str(value),
        }
        if self.arcade_search.query.is_empty() {
            self.clear_arcade_search_results(system_id);
        } else {
            self.refresh_arcade_search_results(catalog, system_id);
        }
    }

    fn accept_arcade_search_suggestion(&mut self, catalog: &ArcadeCatalog, system_id: &str) {
        if self.arcade_search.suggestion.is_empty() {
            return;
        }
        let suggestion = self.arcade_search.suggestion.clone();
        replace_current_search_word(&mut self.arcade_search.query, &suggestion);
        self.refresh_arcade_search_results(catalog, system_id);
    }

    fn snap_arcade_filter_scroll(&mut self, count: usize) {
        if count == 0 {
            self.arcade_filter.scroll.reset();
        } else {
            self.arcade_filter.selected = self.arcade_filter.selected.min(count - 1);
            self.arcade_filter.scroll.selected = self.arcade_filter.selected;
            self.arcade_filter.scroll.snap_to_selected();
        }
        self.sync_arcade_filter_from_scroll();
    }

    fn sync_arcade_filter_from_scroll(&mut self) {
        self.arcade_filter.selected = self.arcade_filter.scroll.selected;
        self.arcade_filter.scroll_y = self.arcade_filter.scroll.scroll_y;
        self.arcade_filter.visual_index = self.arcade_filter.scroll.visual_index;
    }
}

fn filter_option_items(
    options: Vec<ArcadeFilterOption>,
    filter_for_label: impl Fn(&str) -> Option<ArcadeFilter>,
    active_filter: &ArcadeFilter,
) -> Vec<ArcadeDrawerItem> {
    options
        .into_iter()
        .map(|option| {
            let active = filter_for_label(&option.label).as_ref() == Some(active_filter);
            ArcadeDrawerItem {
                label: option.label,
                count: option.count,
                active,
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArcadeTitleGroup {
    Digits,
    Letter(char),
}

impl ArcadeTitleGroup {
    fn label(self) -> String {
        match self {
            Self::Digits => "0-9".to_string(),
            Self::Letter(letter) => letter.to_string(),
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        if label == "0-9" {
            return Some(Self::Digits);
        }
        let mut chars = label.chars();
        let letter = chars.next()?;
        if chars.next().is_none() && letter.is_ascii_alphabetic() {
            Some(Self::Letter(letter.to_ascii_uppercase()))
        } else {
            None
        }
    }
}

fn arcade_title_group(title: &str) -> Option<ArcadeTitleGroup> {
    let first = title.trim_start().chars().next()?;
    if first.is_ascii_digit() {
        Some(ArcadeTitleGroup::Digits)
    } else if first.is_ascii_alphabetic() {
        Some(ArcadeTitleGroup::Letter(first.to_ascii_uppercase()))
    } else {
        None
    }
}

fn decade_from_label(label: &str) -> Option<u16> {
    label.strip_suffix("'s")?.parse::<u16>().ok()
}

fn player_count_label(players: u8) -> String {
    if players == 0 {
        "Unkown".to_string()
    } else if players == 1 {
        "1 Player".to_string()
    } else {
        format!("{players} Players")
    }
}

fn player_count_from_label(label: &str) -> Option<u8> {
    if label == "Unkown" {
        Some(0)
    } else {
        label.split_whitespace().next()?.parse().ok()
    }
}

fn filter_memory_key(filter: &ArcadeFilter) -> String {
    match filter {
        ArcadeFilter::All => "all".to_string(),
        ArcadeFilter::Search => "search".to_string(),
        ArcadeFilter::Category(category) => format!("category:{category}"),
        ArcadeFilter::Decade(decade) => format!("decade:{decade}"),
        ArcadeFilter::Manufacturer(manufacturer) => format!("manufacturer:{manufacturer}"),
        ArcadeFilter::Players(players) => format!("players:{players}"),
        ArcadeFilter::Control(control) => format!("control:{control}"),
    }
}

fn collection_filter_memory_key(collection_id: &str, filter: &ArcadeFilter) -> String {
    format!("{collection_id}\0{}", filter_memory_key(filter))
}

fn default_filter_for_system(_catalog: &ArcadeCatalog, _system_id: &str) -> ArcadeFilter {
    ArcadeFilter::All
}

fn search_row_len(row: usize) -> usize {
    let start = row * ARCADE_SEARCH_KEY_COLUMNS;
    ARCADE_SEARCH_KEYS
        .len()
        .saturating_sub(start)
        .min(ARCADE_SEARCH_KEY_COLUMNS)
}

fn search_key_is_row_end(index: usize) -> bool {
    let row = index / ARCADE_SEARCH_KEY_COLUMNS;
    let col = index % ARCADE_SEARCH_KEY_COLUMNS;
    col + 1 >= search_row_len(row)
}

fn replace_current_search_word(query: &mut String, suggestion: &str) {
    let trim_end_len = query.trim_end_matches(char::is_whitespace).len();
    query.truncate(trim_end_len);
    let start = query
        .rfind(char::is_whitespace)
        .map(|index| index + 1)
        .unwrap_or(0);
    query.truncate(start);
    query.push_str(suggestion);
    query.push(' ');
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchReturnState {
    schema_version: u32,
    screen: String,
    system_id: String,
    system_index: usize,
    #[serde(default)]
    collection_id: Option<String>,
    #[serde(default)]
    menu_path: Vec<String>,
    game_path: String,
    game_index: usize,
    #[serde(default)]
    scroll_y: Option<i32>,
    filter_kind: Option<String>,
    filter_value: Option<String>,
}

impl LaunchReturnState {
    pub fn collection_id(&self) -> Option<&str> {
        self.collection_id.as_deref()
    }

    pub fn system_id(&self) -> &str {
        &self.system_id
    }

    pub fn game_path(&self) -> &str {
        &self.game_path
    }

    pub fn game_index(&self) -> usize {
        self.game_index
    }

    pub fn scroll_y(&self) -> Option<i32> {
        self.scroll_y
    }
}

pub fn capture_launch_return_state(
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
    game_path: &str,
) -> Option<LaunchReturnState> {
    if nav.screen != Screen::Arcade {
        return None;
    }
    let collection_id = nav.active_collection_scope_id(catalog);
    if collection_id.is_empty() {
        return None;
    }
    let games = nav.active_arcade_game_view(catalog, collection_id);
    let game_index = games
        .iter()
        .position(|game| game.mra_path.as_ref() == game_path)
        .unwrap_or(nav.arcade.selected.min(games.len().saturating_sub(1)));
    let (filter_kind, filter_value) = if matches!(nav.arcade_filter.active, ArcadeFilter::Search) {
        ("search".to_string(), Some(nav.arcade_search.query.clone()))
    } else {
        serialize_arcade_filter(&nav.arcade_filter.active)
    };
    let legacy_system_id = nav
        .active_collection()
        .map(|collection| collection.legacy_system_id.as_str())
        .or_else(|| {
            catalog
                .systems
                .get(nav.selected)
                .map(|system| system.id.as_str())
        })
        .unwrap_or("arcade");
    let system_index = catalog
        .systems
        .iter()
        .position(|system| system.id == legacy_system_id)
        .unwrap_or(nav.selected.min(catalog.systems.len().saturating_sub(1)));
    Some(LaunchReturnState {
        schema_version: LAUNCH_RETURN_STATE_SCHEMA,
        screen: "arcade".to_string(),
        system_id: legacy_system_id.to_string(),
        system_index,
        collection_id: Some(collection_id.to_string()),
        menu_path: nav.menu_path.clone(),
        game_path: game_path.to_string(),
        game_index,
        scroll_y: Some(nav.arcade.scroll_y),
        filter_kind: Some(filter_kind),
        filter_value,
    })
}

pub fn save_launch_return_state(state: &LaunchReturnState) -> Result<(), String> {
    SystemLauncherPersistence
        .save_return_state(state)
        .map_err(|failure| failure.detail().to_string())
}

fn save_launch_return_state_at(path: &Path, state: &LaunchReturnState) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("launch return state path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("create launch return state dir: {e}"))?;
    let tmp = temp_state_path(path);
    let text = serde_json::to_string_pretty(state)
        .map_err(|e| format!("serialize launch return state: {e}"))?;
    fs::write(&tmp, text).map_err(|e| format!("write launch return state temp: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("install launch return state: {e}"))?;
    Ok(())
}

pub fn remove_launch_return_state() {
    if let Err(failure) = SystemLauncherPersistence.clear_return_state() {
        crate::ui_errln!("{}", failure.detail());
    }
}

pub fn take_launch_return_state() -> Option<LaunchReturnState> {
    let mut persistence = SystemLauncherPersistence;
    let state = persistence.load_return_state();
    let should_clear = !matches!(
        &state,
        Err(failure) if failure.kind() == LauncherEffectFailureKind::Unavailable
    );
    if should_clear && let Err(failure) = persistence.clear_return_state() {
        crate::ui_errln!("{}", failure.detail());
    }
    match state {
        Ok(state) => state,
        Err(failure) => {
            crate::ui_errln!("{}", failure.detail());
            None
        }
    }
}

struct SystemLauncherPersistence;

impl SystemLauncherPersistence {
    fn unavailable(detail: impl Into<String>) -> LauncherEffectFailure {
        LauncherEffectFailure::new(LauncherEffectFailureKind::Unavailable, detail)
    }

    fn load_return_state_at(
        path: &Path,
    ) -> Result<Option<LaunchReturnState>, LauncherEffectFailure> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(Self::unavailable(format!(
                    "read launch return state {}: {error}",
                    path.display()
                )));
            }
        };
        match serde_json::from_str::<LaunchReturnState>(&text) {
            Ok(state)
                if (1..=LAUNCH_RETURN_STATE_SCHEMA).contains(&state.schema_version)
                    && state.screen == "arcade" =>
            {
                Ok(Some(state))
            }
            Ok(_) => Ok(None),
            Err(error) => Err(LauncherEffectFailure::new(
                LauncherEffectFailureKind::MalformedResponse,
                format!("invalid launch return state {}: {error}", path.display()),
            )),
        }
    }

    fn clear_return_state_at(path: &Path) -> Result<(), LauncherEffectFailure> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Self::unavailable(format!(
                "failed to remove launch return state {}: {error}",
                path.display()
            ))),
        }
    }

    fn library_rebuild_pending(&self) -> bool {
        library_rebuild_on_next_boot_path().exists()
    }
}

impl LauncherPersistence for SystemLauncherPersistence {
    type ReturnState = LaunchReturnState;
    type Settings = MagikSettings;

    fn load_return_state(&mut self) -> Result<Option<Self::ReturnState>, LauncherEffectFailure> {
        Self::load_return_state_at(Path::new(LAUNCH_RETURN_STATE_PATH))
    }

    fn save_return_state(
        &mut self,
        state: &Self::ReturnState,
    ) -> Result<(), LauncherEffectFailure> {
        save_launch_return_state_at(Path::new(LAUNCH_RETURN_STATE_PATH), state)
            .map_err(Self::unavailable)
    }

    fn clear_return_state(&mut self) -> Result<(), LauncherEffectFailure> {
        Self::clear_return_state_at(Path::new(LAUNCH_RETURN_STATE_PATH))
    }

    fn load_settings(&mut self) -> Result<Self::Settings, LauncherEffectFailure> {
        Ok(MagikSettings::load())
    }

    fn save_settings(&mut self, settings: &Self::Settings) -> Result<(), LauncherEffectFailure> {
        settings
            .save()
            .map_err(|error| Self::unavailable(format!("save launcher settings: {error}")))
    }

    fn set_input_policy(&mut self, policy: InputPolicy) -> Result<(), LauncherEffectFailure> {
        write_input_policy_marker(matches!(policy, InputPolicy::Simple)).map_err(Self::unavailable)
    }

    fn request_library_rebuild(&mut self) -> Result<(), LauncherEffectFailure> {
        request_library_rebuild_on_next_boot_at(&library_rebuild_on_next_boot_path())
            .map_err(Self::unavailable)
    }

    fn consume_library_rebuild(&mut self) -> Result<bool, LauncherEffectFailure> {
        consume_library_rebuild_on_next_boot_at(&library_rebuild_on_next_boot_path())
            .map_err(Self::unavailable)
    }
}

pub fn apply_launch_return_state(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    state: LaunchReturnState,
) -> bool {
    nav.sync_launcher_taxonomy(catalog);
    let Some((menu_path, collection_id)) = resolve_return_destination(nav, catalog, &state) else {
        return false;
    };
    if !catalog
        .system_game_view(&collection_id)
        .iter()
        .any(|game| game.mra_path.as_ref() == state.game_path)
    {
        return false;
    }
    if !nav.open_destination(catalog, menu_path, &collection_id) {
        return false;
    }
    let filter = state
        .filter_kind
        .as_deref()
        .and_then(|kind| deserialize_arcade_filter(kind, state.filter_value.as_deref()))
        .filter(|filter| catalog.filtered_game_count(&collection_id, filter) > 0)
        .unwrap_or(ArcadeFilter::All);
    nav.arcade_filter.active = filter.clone();
    nav.collection_filters
        .insert(collection_id.clone(), filter.clone());
    if matches!(filter, ArcadeFilter::Search) {
        nav.arcade_search.query = state.filter_value.clone().unwrap_or_default();
        nav.collection_search_queries
            .insert(collection_id.clone(), nav.arcade_search.query.clone());
        nav.refresh_arcade_search_results(catalog, &collection_id);
    }
    let (game_index, game_count) = {
        let games = nav.active_arcade_game_view(catalog, &collection_id);
        if games.is_empty() {
            return false;
        }
        let Some(game_index) = games
            .iter()
            .position(|game| game.mra_path.as_ref() == state.game_path)
        else {
            return false;
        };
        (game_index, games.len())
    };

    nav.screen = Screen::Arcade;
    nav.arcade_filter.active = filter;
    nav.arcade_filter.drawer_open = false;
    nav.arcade_filter.level = ArcadeFilterLevel::Top;
    let settled_scroll_y = state
        .scroll_y
        .filter(|_| game_index == state.game_index)
        .unwrap_or(game_index as i32 * nav.arcade.row_height());
    nav.arcade
        .restore_position(game_index, settled_scroll_y, game_count);
    true
}

fn resolve_return_destination(
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
    state: &LaunchReturnState,
) -> Option<(Vec<String>, String)> {
    if let Some(collection_id) = state.collection_id.as_deref()
        && nav.taxonomy.collection(collection_id).is_some()
    {
        if nav
            .taxonomy
            .collection_path_is_valid(&state.menu_path, collection_id)
        {
            return Some((state.menu_path.clone(), collection_id.to_string()));
        }
        if let Some(destination) = nav
            .taxonomy
            .primary_destination_for_collection(collection_id)
        {
            return Some((
                destination.menu_path.clone(),
                destination.collection_id.clone(),
            ));
        }
    }

    let system_index = resolve_system_index(catalog, state)?;
    let system_id = &catalog.systems[system_index].id;
    let destination = nav.taxonomy.primary_destination_for_system(system_id)?;
    Some((
        destination.menu_path.clone(),
        destination.collection_id.clone(),
    ))
}

fn serialize_arcade_filter(filter: &ArcadeFilter) -> (String, Option<String>) {
    match filter {
        ArcadeFilter::All => ("all".to_string(), None),
        ArcadeFilter::Search => ("search".to_string(), None),
        ArcadeFilter::Category(category) => ("category".to_string(), Some(category.clone())),
        ArcadeFilter::Decade(decade) => ("decade".to_string(), Some(decade.to_string())),
        ArcadeFilter::Manufacturer(manufacturer) => {
            ("manufacturer".to_string(), Some(manufacturer.clone()))
        }
        ArcadeFilter::Players(players) => ("players".to_string(), Some(players.to_string())),
        ArcadeFilter::Control(control) => ("control".to_string(), Some(control.clone())),
    }
}

fn deserialize_arcade_filter(kind: &str, value: Option<&str>) -> Option<ArcadeFilter> {
    match kind {
        "all" => Some(ArcadeFilter::All),
        "search" => Some(ArcadeFilter::Search),
        "category" => value.map(|value| ArcadeFilter::Category(value.to_string())),
        "decade" => value
            .and_then(|value| value.parse::<u16>().ok())
            .map(ArcadeFilter::Decade),
        "manufacturer" => value.map(|value| ArcadeFilter::Manufacturer(value.to_string())),
        "players" => value
            .and_then(|value| value.parse::<u8>().ok())
            .map(ArcadeFilter::Players),
        "control" => value.map(|value| ArcadeFilter::Control(value.to_string())),
        _ => None,
    }
}

fn resolve_system_index(catalog: &ArcadeCatalog, state: &LaunchReturnState) -> Option<usize> {
    catalog
        .systems
        .iter()
        .position(|system| system.id == state.system_id)
        .or_else(|| {
            (!catalog.systems.is_empty()).then(|| state.system_index.min(catalog.systems.len() - 1))
        })
}

fn temp_state_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("launcher-return-state.json");
    tmp.set_file_name(format!("{file_name}.tmp"));
    tmp
}

fn home_max_scroll(count: usize) -> i32 {
    if count == 0 {
        return 0;
    }
    let content = count as i32 * HOME_TILE_WIDTH + (count.saturating_sub(1) as i32 * HOME_TILE_GAP);
    (content - HOME_LIST_VISIBLE_W).max(0)
}

fn home_tile_pitch() -> i32 {
    HOME_TILE_WIDTH + HOME_TILE_GAP
}

fn home_directional_spring_target(
    value: f64,
    velocity: f64,
    count: usize,
    direction: i32,
    angular_frequency: f64,
) -> f64 {
    let pitch = home_tile_pitch() as f64;
    let max_scroll = home_max_scroll(count) as f64;
    if direction == 0 {
        return value.clamp(0.0, max_scroll);
    }

    // A critically damped spring remains monotonic when the remaining distance
    // is at least |velocity| / angular_frequency. Advance by another pitch when
    // needed instead of allowing a release settle to cross and recoil.
    let minimum_distance = velocity.abs() / angular_frequency.max(f64::EPSILON);
    let mut target = if direction > 0 {
        (value / pitch).ceil() * pitch
    } else {
        (value / pitch).floor() * pitch
    };
    if direction > 0 {
        while target - value < minimum_distance && target < max_scroll {
            target += pitch;
        }
    } else {
        while value - target < minimum_distance && target > 0.0 {
            target -= pitch;
        }
    }
    target.clamp(0.0, max_scroll)
}

fn clamp_home_spring_at_target(animation: &mut SpringAnimation, direction: i32) {
    let crossed = (direction > 0 && animation.value() >= animation.target())
        || (direction < 0 && animation.value() <= animation.target());
    if crossed {
        animation.snap_to(animation.target());
    }
}

fn retarget_home_spring_monotonically(animation: &mut SpringAnimation, target: f64) {
    animation.set_target(target);
    let distance = target - animation.value();
    let max_velocity = distance.abs() * animation.configuration().angular_frequency();
    let velocity = animation.velocity();
    if velocity.signum() == distance.signum() && velocity.abs() > max_velocity {
        animation.set_state(animation.value(), distance.signum() * max_velocity);
    }
}

fn keep_home_visible(selected: usize, scroll_x: &mut i32, count: usize) {
    let visible_left = selected as i32 * home_tile_pitch();
    let visible_right = visible_left + HOME_TILE_WIDTH;
    if visible_left < *scroll_x {
        *scroll_x = visible_right - HOME_LIST_VISIBLE_W;
    } else if visible_right > *scroll_x + HOME_LIST_VISIBLE_W {
        *scroll_x = visible_left;
    }
    *scroll_x = (*scroll_x).clamp(0, home_max_scroll(count));
}

#[cfg(test)]
fn pad_action_held(state: &PadState, action: crate::input_event::LogicalAction) -> bool {
    match action {
        crate::input_event::LogicalAction::Up => state.dpad_up,
        crate::input_event::LogicalAction::Down => state.dpad_down,
        crate::input_event::LogicalAction::Left => state.dpad_left,
        crate::input_event::LogicalAction::Right => state.dpad_right,
        crate::input_event::LogicalAction::Activate => state.btn_a,
        crate::input_event::LogicalAction::Back => state.btn_b,
        crate::input_event::LogicalAction::Home => state.btn_home,
        crate::input_event::LogicalAction::X => state.btn_x,
        crate::input_event::LogicalAction::Y => state.btn_y,
        crate::input_event::LogicalAction::L => state.btn_l,
        crate::input_event::LogicalAction::R => state.btn_r,
        crate::input_event::LogicalAction::Select => state.btn_select,
        crate::input_event::LogicalAction::Start => state.btn_start,
    }
}

fn confirm_max_selected(action: Option<ConfirmAction>) -> usize {
    match action {
        Some(ConfirmAction::LibraryUpdateFailed | ConfirmAction::DisplayResolutionError) => 0,
        Some(_) => 1,
        None => 0,
    }
}

fn arcade_dpad_dir(state: &PadState) -> i32 {
    if state.dpad_down && !state.dpad_up {
        1
    } else if state.dpad_up && !state.dpad_down {
        -1
    } else {
        0
    }
}

pub fn game_title(catalog: &ArcadeCatalog, mra_path: &str) -> String {
    catalog.title_for_path(mra_path).to_string()
}

pub fn request_supervised_launcher_restart() -> Result<(), String> {
    static REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if REQUESTED.swap(true, Ordering::AcqRel) {
        return Err("supervised launcher restart already requested".to_string());
    }
    execute_main_command(&MainCommand::SupervisedLauncherRestart).map(|_| ())
}

fn execute_main_command(command: &MainCommand) -> Result<Option<String>, String> {
    let fifo_pmu = mister_magik_perf_events::sampled_span("launch.fifo-request");
    let result = main_command::execute(command).map_err(|error| error.to_string());
    drop(fifo_pmu);
    result
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisplayCommandState {
    pub active: String,
    pub pending: Option<String>,
    pub remaining: u8,
    pub phase: DisplayTransactionPhase,
    pub error: Option<String>,
    pub return_to_settings: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DisplayTransactionPhase {
    #[default]
    Idle,
    Provisional,
    Persisting,
    Failed,
}

impl From<EffectDisplayTransactionPhase> for DisplayTransactionPhase {
    fn from(phase: EffectDisplayTransactionPhase) -> Self {
        match phase {
            EffectDisplayTransactionPhase::Idle => Self::Idle,
            EffectDisplayTransactionPhase::Provisional => Self::Provisional,
            EffectDisplayTransactionPhase::Persisting => Self::Persisting,
            EffectDisplayTransactionPhase::Failed => Self::Failed,
        }
    }
}

impl From<EffectDisplayState> for DisplayCommandState {
    fn from(state: EffectDisplayState) -> Self {
        Self {
            active: state.active_mode,
            pending: state.pending_mode,
            remaining: state.remaining_secs,
            phase: state.phase.into(),
            error: state.error,
            return_to_settings: state.return_to_settings,
        }
    }
}

pub fn display_state() -> Result<DisplayCommandState, String> {
    mister_magik_mister_runtime::display_control::MainDisplayControl
        .state(DisplayStateRead::Wait)
        .map(DisplayCommandState::from)
        .map_err(|failure| failure.detail().to_string())
}

pub fn try_display_state() -> Result<DisplayCommandState, String> {
    mister_magik_mister_runtime::display_control::MainDisplayControl
        .state(DisplayStateRead::Try)
        .map(DisplayCommandState::from)
        .map_err(|failure| failure.detail().to_string())
}

#[cfg(test)]
fn parse_display_state_response(response: &str) -> Result<DisplayCommandState, String> {
    mister_magik_mister_runtime::display_control::parse_state_response(response)
        .map(DisplayCommandState::from)
}

pub fn apply_display_resolution(id: &str) -> Result<(), String> {
    mister_magik_mister_runtime::display_control::MainDisplayControl
        .apply(id)
        .map_err(|failure| failure.detail().to_string())
}

pub fn confirm_display_resolution() -> Result<(), String> {
    mister_magik_mister_runtime::display_control::MainDisplayControl
        .confirm()
        .map_err(|failure| failure.detail().to_string())
}

pub fn confirm_display_resolution_and_wait(
    timeout: Duration,
) -> Result<DisplayCommandState, String> {
    let mut display = mister_magik_mister_runtime::display_control::MainDisplayControl;
    display
        .confirm()
        .map_err(|failure| failure.detail().to_string())?;
    let started = Instant::now();
    while started.elapsed() < timeout {
        let state = display
            .state(DisplayStateRead::Wait)
            .map(DisplayCommandState::from)
            .map_err(|failure| failure.detail().to_string())?;
        match state.phase {
            DisplayTransactionPhase::Idle if state.pending.is_none() => return Ok(state),
            DisplayTransactionPhase::Failed => return Ok(state),
            DisplayTransactionPhase::Idle
            | DisplayTransactionPhase::Provisional
            | DisplayTransactionPhase::Persisting => {}
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("display persistence timed out".to_string())
}

pub fn cancel_display_resolution() -> Result<(), String> {
    mister_magik_mister_runtime::display_control::MainDisplayControl
        .cancel()
        .map_err(|failure| failure.detail().to_string())
}

trait LaunchIo {
    fn target_exists(&mut self, path: &str) -> bool;
    fn mister_running(&mut self) -> bool;
    fn magik_running(&mut self) -> bool;
    fn simple_joystick_handling(&mut self) -> bool;
    fn prepare_simple_input_profiles(&mut self) -> Result<(), String>;
    fn start_mister(&mut self) -> Result<(), String>;
    fn wait_for_started_mister(&mut self) -> Result<(), String>;
    fn wait_for_command_fifo(&mut self) -> Result<(), String>;
    fn write_input_policy_marker(&mut self, simple_joystick_handling: bool) -> Result<(), String>;
    fn write_button_overrides(
        &mut self,
        selection: &EffectLaunchSelection,
        simple_joystick_handling: bool,
    ) -> Result<(), String>;
    fn write_mister_command(&mut self, command: &MainCommand) -> Result<(), String>;
}

struct SystemLaunchIo;

impl LaunchIo for SystemLaunchIo {
    fn target_exists(&mut self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn mister_running(&mut self) -> bool {
        mister_running()
    }

    fn magik_running(&mut self) -> bool {
        SystemRuntimeState
            .main_state()
            .is_ok_and(|state| state.magik_owned)
    }

    fn simple_joystick_handling(&mut self) -> bool {
        SystemLauncherPersistence
            .load_settings()
            .map(|settings| settings.simple_joystick_handling)
            .unwrap_or(false)
    }

    fn prepare_simple_input_profiles(&mut self) -> Result<(), String> {
        write_builtin_simple_input_profiles()
    }

    fn start_mister(&mut self) -> Result<(), String> {
        Command::new(mister_bin())
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("failed to spawn {}: {e}", mister_bin()))
    }

    fn wait_for_started_mister(&mut self) -> Result<(), String> {
        main_command::wait_for_running_main_and_fifo(mister_bin(), MAIN_START_TIMEOUT)
            .map_err(|error| error.to_string())
    }

    fn wait_for_command_fifo(&mut self) -> Result<(), String> {
        main_command::wait_for_command_fifo(FIFO_WAIT_TIMEOUT).map_err(|error| error.to_string())
    }

    fn write_input_policy_marker(&mut self, simple_joystick_handling: bool) -> Result<(), String> {
        SystemLauncherPersistence
            .set_input_policy(if simple_joystick_handling {
                InputPolicy::Simple
            } else {
                InputPolicy::Stock
            })
            .map_err(|failure| failure.detail().to_string())
    }

    fn write_button_overrides(
        &mut self,
        selection: &EffectLaunchSelection,
        simple_joystick_handling: bool,
    ) -> Result<(), String> {
        write_button_overrides_for_launch(selection, simple_joystick_handling)
    }

    fn write_mister_command(&mut self, command: &MainCommand) -> Result<(), String> {
        execute_main_command(command).map(|_| ())
    }
}

fn mister_running() -> bool {
    SystemRuntimeState
        .main_state()
        .is_ok_and(|state| state.running)
}

/// Main owns HDMI/OSD/input state; do not kill it from Slint.
pub fn stop_mister() {
    crate::ui_errln!(
        "refusing to kill MiSTer/MiSTer_MagiK; reboot or hand off through Main to recover display ownership"
    );
}

fn spawn_mister() -> Result<(), String> {
    let mut io = SystemLaunchIo;
    io.start_mister()?;
    io.wait_for_started_mister()?;
    thread::sleep(Duration::from_millis(200));
    Ok(())
}

fn restore_menu_wallpaper() {
    let hidden = mister_magik_catalog::device_layout::current_app_path(".menu.png.boot-hide");
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "[ -f '{}' ] && mv '{}' /media/fat/menu.png 2>/dev/null || true",
            hidden.display(),
            hidden.display()
        ))
        .status();
}

fn write_input_policy_marker(simple_joystick_handling: bool) -> Result<(), String> {
    let path = Path::new(INPUT_POLICY_MARKER_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create input policy marker dir {}: {e}",
                parent.display()
            )
        })?;
    }
    if simple_joystick_handling {
        fs::write(path, "simple\n")
            .map_err(|e| format!("failed to write {INPUT_POLICY_MARKER_PATH}: {e}"))
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("failed to remove {INPUT_POLICY_MARKER_PATH}: {e}")),
        }
    }
}

fn write_button_overrides_for_launch(
    selection: &EffectLaunchSelection,
    simple_joystick_handling: bool,
) -> Result<(), String> {
    if !simple_joystick_handling {
        return remove_button_overrides();
    }

    match selection {
        EffectLaunchSelection::CatalogPath { target }
            if target.to_ascii_lowercase().ends_with(".mra") =>
        {
            write_button_overrides_for_mra(Path::new(target))
        }
        _ => remove_button_overrides(),
    }
}

fn write_builtin_simple_input_profiles() -> Result<(), String> {
    const RETRO_BIT_A2_MAP: [u32; 32] = [
        0x0000_0321,
        0x0000_0320,
        0x0000_0323,
        0x0000_0322,
        0x0000_0132,
        0x0000_0131,
        0x0000_0133,
        0x0000_0130,
        0x0000_0134,
        0x0000_0135,
        0x0000_0138,
        0x0000_0139,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0x0000_013c,
        0x0000_013c,
        0x0130_0000,
        0x0002_0000,
        0x0002_0001,
        0x0002_0002,
        0x0002_0005,
        0x0002_0000,
        0x0002_0001,
        0,
        0,
    ];
    write_simple_input_profile("input_2563_0575_v3.map", &RETRO_BIT_A2_MAP)
}

fn write_simple_input_profile(name: &str, map: &[u32; 32]) -> Result<(), String> {
    let input_dir = magik_input_dir();
    fs::create_dir_all(&input_dir)
        .map_err(|e| format!("failed to create {}: {e}", input_dir.display()))?;
    let path = input_dir.join(name);
    let mut bytes = Vec::with_capacity(map.len() * 4);
    for value in map {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    if fs::read(&path).ok().as_deref() == Some(bytes.as_slice()) {
        return Ok(());
    }
    let tmp = path.with_extension("map.tmp");
    fs::write(&tmp, &bytes).map_err(|e| format!("failed to write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|e| format!("failed to install {}: {e}", path.display()))
}

#[cfg(test)]
fn encode_launch_plan(plan: &StructuredLaunchPlan) -> String {
    encode_launch_fields(
        plan.launch_ref.as_ref(),
        plan.title.as_ref(),
        plan.system_id.as_ref(),
        plan.core_path.as_ref(),
        plan.payload_path.as_ref(),
        plan.mount_kind.as_ref(),
        plan.mount_index,
        plan.delay_secs,
    )
}

fn encode_effect_launch_plan(plan: &EffectStructuredLaunchSelection) -> String {
    encode_launch_fields(
        &plan.launch_ref,
        &plan.title,
        &plan.system_id,
        &plan.core,
        &plan.payload,
        &plan.mount_kind,
        plan.mount_index,
        plan.delay_secs,
    )
}

#[allow(clippy::too_many_arguments)]
fn encode_launch_fields(
    launch_ref: &str,
    title: &str,
    system_id: &str,
    core_path: &str,
    payload_path: &str,
    mount_kind: &str,
    mount_index: u8,
    delay_secs: u8,
) -> String {
    let mount_index = mount_index.to_string();
    let delay_secs = delay_secs.to_string();
    let core_path = logical_core_path(core_path);
    let fields = [
        ("schema", "1"),
        ("launch_ref", launch_ref),
        ("title", title),
        ("system_id", system_id),
        ("core_path", core_path),
        ("payload_path", payload_path),
        ("mount_kind", mount_kind),
        ("mount_index", mount_index.as_str()),
        ("delay_secs", delay_secs.as_str()),
    ];
    fields
        .into_iter()
        .map(|(key, value)| format!("{key}={}", percent_encode_plan_value(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Converts a catalog-era physical RBF selector back to Main's logical selector.
///
/// Older catalogs persist paths such as `_Console/SNES_20240408`. Passing that
/// exact selector prevents Main's existing RBF resolver from selecting a newer
/// `SNES_YYYYMMDD.rbf` installed by Update All. Normalizing at handoff keeps
/// those catalogs compatible without rebuilding them.
fn logical_core_path(path: &str) -> &str {
    let Some((logical, suffix)) = path.rsplit_once('_') else {
        return path;
    };
    if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        logical
    } else {
        path
    }
}

fn percent_encode_plan_value(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b':') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

pub fn exit_to_mister() -> Result<(), String> {
    restore_menu_wallpaper();

    if mister_running() {
        main_command::wait_for_command_fifo(FIFO_WAIT_TIMEOUT)
            .map_err(|error| error.to_string())?;
        execute_main_command(&MainCommand::ExitToMenu)?;
    } else {
        spawn_mister()?;
    }

    Ok(())
}

/// True while Slint should keep the loading screen up.
pub fn launch_in_progress() -> bool {
    LAUNCH_STATE.load(Ordering::Acquire) == LAUNCH_SENT
}

#[allow(dead_code)]
#[doc(hidden)]
pub fn mark_launch_sent_for_test() {
    LAUNCH_STATE.store(LAUNCH_SENT, Ordering::Release);
}

/// Main is running an arcade core (argv contains `.rbf`, not `menu.rbf`).
pub fn mister_running_arcade_core() -> bool {
    SystemRuntimeState
        .main_state()
        .is_ok_and(|state| state.arcade_core)
}

/// Launch via fifo. Prefer the Magik-aware Main command when the fork owns the device.
/// Returns `true` if Main was spawned for this launch (caller should stop it on failure).
pub fn execute_game_launch(launch_target: &LaunchTarget) -> Result<bool, LaunchError> {
    let mut io = SystemLaunchIo;
    execute_game_launch_with(launch_target, &mut io)
}

#[derive(Debug)]
pub struct LaunchHandoffBenchResult {
    pub result: Result<bool, LaunchError>,
    pub prepare_us: u64,
    pub handoff_us: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchHandoffBenchMode {
    SlowFail,
    Success,
}

pub fn execute_game_launch_handoff_bench(
    launch_target: &LaunchTarget,
    fifo_delay: Duration,
    mode: LaunchHandoffBenchMode,
) -> LaunchHandoffBenchResult {
    struct BenchLaunchIo {
        fifo_delay: Duration,
        mode: LaunchHandoffBenchMode,
        handoff_us: u64,
    }

    impl LaunchIo for BenchLaunchIo {
        fn target_exists(&mut self, path: &str) -> bool {
            Path::new(path).exists()
        }

        fn mister_running(&mut self) -> bool {
            true
        }

        fn magik_running(&mut self) -> bool {
            true
        }

        fn simple_joystick_handling(&mut self) -> bool {
            false
        }

        fn prepare_simple_input_profiles(&mut self) -> Result<(), String> {
            Ok(())
        }

        fn start_mister(&mut self) -> Result<(), String> {
            Ok(())
        }

        fn wait_for_started_mister(&mut self) -> Result<(), String> {
            Ok(())
        }

        fn wait_for_command_fifo(&mut self) -> Result<(), String> {
            let start = Instant::now();
            thread::sleep(self.fifo_delay);
            self.handoff_us = self
                .handoff_us
                .saturating_add(start.elapsed().as_micros() as u64);
            if self.mode == LaunchHandoffBenchMode::Success {
                Ok(())
            } else {
                Err("benchmark command FIFO timeout".to_string())
            }
        }

        fn write_input_policy_marker(
            &mut self,
            _simple_joystick_handling: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn write_button_overrides(
            &mut self,
            _selection: &EffectLaunchSelection,
            _simple_joystick_handling: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn write_mister_command(&mut self, _command: &MainCommand) -> Result<(), String> {
            if self.mode == LaunchHandoffBenchMode::Success {
                Ok(())
            } else {
                Err("benchmark handoff does not write the real MiSTer FIFO".to_string())
            }
        }
    }

    let mut io = BenchLaunchIo {
        fifo_delay,
        mode,
        handoff_us: 0,
    };
    let prepare = Instant::now();
    let result = execute_game_launch_with(launch_target, &mut io);
    LaunchHandoffBenchResult {
        result,
        prepare_us: prepare.elapsed().as_micros() as u64,
        handoff_us: io.handoff_us,
    }
}

fn effect_selection_from_launch_target(
    launch_target: &LaunchTarget,
) -> Result<EffectLaunchSelection, LaunchError> {
    match launch_target {
        LaunchTarget::Path(path) => Ok(EffectLaunchSelection::CatalogPath {
            target: path.to_string(),
        }),
        LaunchTarget::Structured(plan) => Ok(EffectLaunchSelection::Structured(
            EffectStructuredLaunchSelection {
                launch_ref: plan.launch_ref.to_string(),
                title: plan.title.to_string(),
                system_id: plan.system_id.to_string(),
                core: plan.core_path.to_string(),
                payload: plan.payload_path.to_string(),
                mount_kind: plan.mount_kind.to_string(),
                mount_index: plan.mount_index,
                delay_secs: plan.delay_secs,
            },
        )),
        LaunchTarget::Prepared(selection) => Err(LaunchError::new(
            format!(
                "prepared {} launch must be resolved before Main handoff: {}",
                selection.collection_id, selection.launch_ref
            ),
            false,
        )),
        LaunchTarget::MissingStructured(launch_ref) => Err(LaunchError::new(
            format!("structured launch plan missing from catalog: {launch_ref}"),
            false,
        )),
    }
}

struct LaunchIoHandoff<'a, I> {
    io: &'a mut I,
    magik_running: bool,
    started_main: bool,
}

impl<I: LaunchIo> LaunchIoHandoff<'_, I> {
    fn failure(
        &self,
        kind: LauncherEffectFailureKind,
        detail: impl Into<String>,
    ) -> LauncherEffectFailure {
        LauncherEffectFailure::new(kind, detail).with_recovery_required(self.started_main)
    }
}

impl<I: LaunchIo> LaunchHandoff for LaunchIoHandoff<'_, I> {
    fn handoff(
        &mut self,
        request: &LaunchHandoffRequest,
    ) -> Result<LaunchHandoffOutcome, LauncherEffectFailure> {
        if self.magik_running {
            if request.simple_joystick_handling {
                self.io
                    .prepare_simple_input_profiles()
                    .map_err(|error| self.failure(LauncherEffectFailureKind::Unavailable, error))?;
            }
            self.io
                .write_button_overrides(&request.selection, request.simple_joystick_handling)
                .map_err(|error| self.failure(LauncherEffectFailureKind::Unavailable, error))?;
            self.io
                .write_input_policy_marker(request.simple_joystick_handling)
                .map_err(|error| self.failure(LauncherEffectFailureKind::Unavailable, error))?;
        }

        let command = match (&request.selection, self.magik_running) {
            (EffectLaunchSelection::CatalogPath { target }, true) => MainCommand::LaunchPath {
                target: target.clone(),
            },
            (EffectLaunchSelection::CatalogPath { target }, false) => MainCommand::LoadCore {
                target: target.clone(),
            },
            (EffectLaunchSelection::Structured(plan), true) => MainCommand::StructuredLaunch {
                fields: encode_effect_launch_plan(plan),
            },
            (EffectLaunchSelection::Structured(_), false) => {
                return Err(self.failure(
                    LauncherEffectFailureKind::Rejected,
                    "structured launch plan requires MiSTer_MagiK",
                ));
            }
        };
        crate::ui_logln!("launch: {command:?}");
        if let Err(error) = self.io.write_mister_command(&command) {
            if self.magik_running {
                let _ = self.io.write_input_policy_marker(false);
            }
            return Err(self.failure(LauncherEffectFailureKind::Rejected, error));
        }
        Ok(LaunchHandoffOutcome {
            started_main: self.started_main,
        })
    }
}

fn launch_error_from_effect(failure: LauncherEffectFailure) -> LaunchError {
    LaunchError::new(failure.detail(), failure.recovery_required())
}

fn execute_game_launch_with(
    launch_target: &LaunchTarget,
    io: &mut impl LaunchIo,
) -> Result<bool, LaunchError> {
    if let LaunchTarget::MissingStructured(launch_ref) = launch_target {
        return Err(LaunchError::new(
            format!("structured launch plan missing from catalog: {launch_ref}"),
            false,
        ));
    }
    if let LaunchTarget::Prepared(selection) = launch_target {
        return Err(LaunchError::new(
            format!(
                "prepared {} launch must be resolved before Main handoff: {}",
                selection.collection_id, selection.launch_ref
            ),
            false,
        ));
    }
    if let LaunchTarget::Path(path) = launch_target
        && !io.target_exists(path)
    {
        return Err(
            LaunchError::new(format!("launch target not found: {path}"), false)
                .with_kind(LaunchFailureKind::UnreadablePayload),
        );
    }

    let spawned = if io.mister_running() {
        false
    } else {
        crate::ui_logln!("launch: starting {} for load_core", mister_bin());
        io.start_mister().map_err(|e| LaunchError::new(e, false))?;
        io.wait_for_started_mister()
            .map_err(|error| LaunchError::new(error, true))?;
        true
    };

    io.wait_for_command_fifo()
        .map_err(|error| LaunchError::new(error, spawned))?;

    let magik_running = io.magik_running();
    let request = LaunchHandoffRequest {
        selection: effect_selection_from_launch_target(launch_target)?,
        simple_joystick_handling: magik_running && io.simple_joystick_handling(),
    };
    let mut handoff = LaunchIoHandoff {
        io,
        magik_running,
        started_main: spawned,
    };
    let outcome = handoff
        .handoff(&request)
        .map_err(launch_error_from_effect)?;
    LAUNCH_STATE.store(LAUNCH_SENT, Ordering::Release);
    Ok(outcome.started_main)
}

pub fn reset_launch() {
    LAUNCH_STATE.store(LAUNCH_IDLE, Ordering::Release);
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PurgeLibraryDataOutcome {
    pub catalog_artifacts_removed: usize,
    pub screenshot_artifacts_removed: usize,
}

pub fn purge_library_data() -> Result<PurgeLibraryDataOutcome, String> {
    let asset_dir = std::env::var("MISTER_MEDIA_ASSET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| mister_magik_catalog::device_layout::current_app_path("assets"));
    purge_library_data_with(&asset_dir, || {
        mister_magik_catalog::builder_service::remove_default_production_catalog_artifacts()
    })
}

fn purge_library_data_with(
    asset_dir: &Path,
    remove_catalog: impl FnOnce() -> Result<usize, String>,
) -> Result<PurgeLibraryDataOutcome, String> {
    let catalog_artifacts_removed = remove_catalog()?;
    let screenshot_artifacts_removed = delete_screenshot_packs_at(asset_dir)?;
    Ok(PurgeLibraryDataOutcome {
        catalog_artifacts_removed,
        screenshot_artifacts_removed,
    })
}

pub fn delete_screenshot_packs() -> Result<usize, String> {
    let asset_dir = std::env::var("MISTER_MEDIA_ASSET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| mister_magik_catalog::device_layout::current_app_path("assets"));
    delete_screenshot_packs_at(&asset_dir)
}

fn delete_screenshot_packs_at(asset_dir: &Path) -> Result<usize, String> {
    let mut fault_control =
        mister_magik_mister_runtime::direct_reset_fault::process_fault_control();
    delete_screenshot_packs_at_with_fault_control(asset_dir, &mut fault_control)
}

fn delete_screenshot_packs_at_with_fault_control(
    asset_dir: &Path,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<usize, String> {
    let entries = match fs::read_dir(asset_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(format!(
                "read screenshot asset dir {}: {e}",
                asset_dir.display()
            ));
        }
    };
    let mut removed = 0usize;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read screenshot asset entry: {e}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("stat screenshot asset {}: {e}", path.display()))?;
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if screenshot_reset_deletes_file(name) {
            fs::remove_file(&path)
                .map_err(|e| format!("delete screenshot asset {}: {e}", path.display()))?;
            mister_magik_catalog::fs_fault::maybe_fault_with_control(
                "reset_delete.screenshot_asset.after_remove",
                &path,
                fault_control,
            );
            removed += 1;
        }
    }
    Ok(removed)
}

fn screenshot_reset_deletes_file(name: &str) -> bool {
    screenshot_reset_deletes_filename(name)
}

pub fn library_rebuild_on_next_boot_pending() -> bool {
    SystemLauncherPersistence.library_rebuild_pending()
}

pub fn request_library_rebuild_on_next_boot() -> Result<(), String> {
    SystemLauncherPersistence
        .request_library_rebuild()
        .map_err(|failure| failure.detail().to_string())
}

pub fn consume_library_rebuild_on_next_boot() -> Result<bool, String> {
    SystemLauncherPersistence
        .consume_library_rebuild()
        .map_err(|failure| failure.detail().to_string())
}

fn request_library_rebuild_on_next_boot_at(path: &Path) -> Result<(), String> {
    let mut fault_control =
        mister_magik_mister_runtime::direct_reset_fault::process_fault_control();
    request_library_rebuild_on_next_boot_at_with_fault_control(path, &mut fault_control)
}

fn request_library_rebuild_on_next_boot_at_with_fault_control(
    path: &Path,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create rebuild marker dir: {e}"))?;
    }
    fs::write(path, b"rebuild\n").map_err(|e| format!("write rebuild marker: {e}"))?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "launcher.rebuild_marker.after_write",
        path,
        fault_control,
    );
    Ok(())
}

fn consume_library_rebuild_on_next_boot_at(path: &Path) -> Result<bool, String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("remove rebuild marker: {e}")),
    }
}

fn reboot_mister_with(io: &mut impl LaunchIo) -> Result<(), String> {
    if !io.magik_running() {
        return Err("MiSTer_MagiK is not running; refusing raw reboot from Slint".into());
    }
    io.wait_for_command_fifo()?;
    io.write_mister_command(&MainCommand::Reboot)
}

pub fn reboot_mister() -> Result<(), String> {
    let mut io = SystemLaunchIo;
    reboot_mister_with(&mut io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{arcade_catalog, arcade_game, arcade_system};
    use std::sync::Mutex;

    static LAUNCH_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct RecordingFaultControl {
        points: Vec<String>,
    }

    impl mister_magik_catalog::fs_fault::DirectResetFaultControl for RecordingFaultControl {
        fn request_direct_reset(
            &mut self,
            request: &mister_magik_catalog::fs_fault::DirectResetFaultRequest,
        ) -> mister_magik_catalog::fs_fault::DirectResetFaultOutcome {
            self.points.push(request.point().to_string());
            mister_magik_catalog::fs_fault::DirectResetFaultOutcome::Noop
        }
    }

    fn catalog_presentation(
        status: CatalogMenuItemStatus,
        available: bool,
        retryable: bool,
    ) -> CatalogMenuItemPresentation {
        CatalogMenuItemPresentation {
            status,
            available,
            retryable,
        }
    }

    struct FakeLaunchIo {
        target_exists: bool,
        mister_running: bool,
        magik_running: bool,
        simple_joystick_handling: bool,
        start_result: Result<(), String>,
        started_ready: bool,
        fifo_ready: bool,
        write_result: Result<(), String>,
        prepare_result: Result<(), String>,
        input_policy_result: Result<(), String>,
        button_override_result: Result<(), String>,
        start_calls: usize,
        prepare_simple_input_profile_calls: usize,
        input_policy_markers: Vec<bool>,
        button_override_writes: Vec<String>,
        commands: Vec<MainCommand>,
        effects: Vec<String>,
    }

    impl LaunchIo for FakeLaunchIo {
        fn target_exists(&mut self, _path: &str) -> bool {
            self.target_exists
        }

        fn mister_running(&mut self) -> bool {
            self.mister_running
        }

        fn magik_running(&mut self) -> bool {
            self.magik_running
        }

        fn simple_joystick_handling(&mut self) -> bool {
            self.simple_joystick_handling
        }

        fn prepare_simple_input_profiles(&mut self) -> Result<(), String> {
            self.prepare_simple_input_profile_calls += 1;
            self.effects.push("prepare-input-profiles".to_string());
            self.prepare_result.clone()
        }

        fn start_mister(&mut self) -> Result<(), String> {
            self.start_calls += 1;
            self.effects.push("start-main".to_string());
            self.start_result.clone()
        }

        fn wait_for_started_mister(&mut self) -> Result<(), String> {
            if self.started_ready {
                Ok(())
            } else {
                Err(format!(
                    "timed out waiting for {} + /dev/MiSTer_cmd",
                    mister_bin()
                ))
            }
        }

        fn wait_for_command_fifo(&mut self) -> Result<(), String> {
            if self.fifo_ready {
                Ok(())
            } else {
                Err("timed out waiting for /dev/MiSTer_cmd".to_string())
            }
        }

        fn write_input_policy_marker(
            &mut self,
            simple_joystick_handling: bool,
        ) -> Result<(), String> {
            self.input_policy_markers.push(simple_joystick_handling);
            self.effects
                .push(format!("input-policy:{simple_joystick_handling}"));
            self.input_policy_result.clone()
        }

        fn write_button_overrides(
            &mut self,
            selection: &EffectLaunchSelection,
            simple_joystick_handling: bool,
        ) -> Result<(), String> {
            let action = match (simple_joystick_handling, selection) {
                (true, EffectLaunchSelection::CatalogPath { target })
                    if target.to_ascii_lowercase().ends_with(".mra") =>
                {
                    format!("write:{target}")
                }
                _ => "remove".to_string(),
            };
            self.button_override_writes.push(action);
            self.effects.push(format!(
                "button-overrides:{}",
                self.button_override_writes
                    .last()
                    .expect("button override action was recorded")
            ));
            self.button_override_result.clone()
        }

        fn write_mister_command(&mut self, command: &MainCommand) -> Result<(), String> {
            self.commands.push(command.clone());
            let effect = match command {
                MainCommand::LaunchPath { target } => format!("main-command:launch:{target}"),
                MainCommand::StructuredLaunch { .. } => "main-command:structured".to_string(),
                MainCommand::LoadCore { target } => format!("main-command:load-core:{target}"),
                MainCommand::Reboot => "main-command:reboot".to_string(),
                command => format!("main-command:{command:?}"),
            };
            self.effects.push(effect);
            self.write_result.clone()
        }
    }

    fn launch_io() -> FakeLaunchIo {
        FakeLaunchIo {
            target_exists: true,
            mister_running: true,
            magik_running: true,
            simple_joystick_handling: false,
            start_result: Ok(()),
            started_ready: true,
            fifo_ready: true,
            write_result: Ok(()),
            prepare_result: Ok(()),
            input_policy_result: Ok(()),
            button_override_result: Ok(()),
            start_calls: 0,
            prepare_simple_input_profile_calls: 0,
            input_policy_markers: Vec::new(),
            button_override_writes: Vec::new(),
            commands: Vec::new(),
            effects: Vec::new(),
        }
    }

    fn path_target(path: &str) -> LaunchTarget {
        LaunchTarget::Path(path.into())
    }

    fn structured_target() -> LaunchTarget {
        LaunchTarget::Structured(StructuredLaunchPlan {
            launch_ref: "magik-plan:test game".into(),
            title: "Test Game".into(),
            system_id: "neogeo".into(),
            core_path: "NeoGeo".into(),
            payload_path: "/media/fat/games/NEOGEO/Test Game.neo".into(),
            mount_kind: "mount-image".into(),
            mount_index: 0,
            delay_secs: 1,
        })
    }

    fn input(nav: &mut ArcadeNav, dir: i32, previous_dir: i32, count: usize, now: Instant) {
        nav.handle_direction_input(dir, previous_dir, now, count);
        nav.tick(count, now);
    }

    fn settle(nav: &mut ArcadeNav, count: usize, start: Instant) {
        for frame in 1..=180 {
            nav.tick(count, start + Duration::from_millis(frame * 16));
            if nav.is_settled() {
                return;
            }
        }
        assert!(nav.is_settled(), "arcade nav did not settle");
    }

    fn image_less_amiga_catalog() -> ArcadeCatalog {
        arcade_catalog(
            vec![
                arcade_game("Agony")
                    .path("magik-plan:amiga-agony")
                    .system_id("amiga")
                    .build(),
            ],
            vec![arcade_system("amiga", 1)],
        )
    }

    fn amiga_games_and_demos_catalog() -> ArcadeCatalog {
        arcade_catalog(
            vec![
                arcade_game("Agony")
                    .path("magik-amigavision:games:Agony")
                    .system_id("amiga")
                    .build(),
                arcade_game("Alien Breed")
                    .path("magik-amigavision:games:Alien%20Breed")
                    .system_id("amiga")
                    .build(),
                arcade_game("State of the Art")
                    .path("magik-amigavision:demos:State%20of%20the%20Art")
                    .system_id("amiga")
                    .build(),
            ],
            vec![arcade_system("amiga", 3)],
        )
    }

    fn multi_system_catalog() -> ArcadeCatalog {
        arcade_catalog(
            vec![
                arcade_game("1942")
                    .path("/media/fat/_Arcade/1942.mra")
                    .preview("1942")
                    .build(),
                arcade_game("Agony")
                    .path("magik-plan:amiga-agony")
                    .system_id("amiga")
                    .build(),
            ],
            vec![arcade_system("arcade", 1), arcade_system("amiga", 1)],
        )
    }

    fn hierarchy_catalog() -> ArcadeCatalog {
        arcade_catalog(
            vec![
                arcade_game("Metal Slug")
                    .manufacturer("SNK (Rock-Ola license)")
                    .build(),
                arcade_game("Super Mario Bros").system_id("nes").build(),
                arcade_game("Super Mario 64").system_id("n64").build(),
                arcade_game("Pocket Tennis")
                    .system_id("neogeopocket")
                    .build(),
                arcade_game("Agony").system_id("amiga").build(),
                arcade_game("Sonic").system_id("gamegear").build(),
                arcade_game("Metal Slug AES").system_id("neogeo").build(),
            ],
            vec![
                arcade_system("arcade", 1),
                arcade_system("neogeo", 1),
                arcade_system("nes", 1),
                arcade_system("n64", 1),
                arcade_system("neogeopocket", 1),
                arcade_system("gamegear", 1),
                arcade_system("amiga", 1),
            ],
        )
    }

    fn multi_game_catalog() -> ArcadeCatalog {
        let mut games = Vec::new();
        for i in 0..5 {
            games.push(
                arcade_game(format!("Arcade {i}"))
                    .path(format!("/media/fat/_Arcade/arcade-{i}.mra"))
                    .build(),
            );
        }
        for i in 0..3 {
            games.push(
                arcade_game(format!("Amiga {i}"))
                    .path(format!("magik-plan:amiga-{i}"))
                    .system_id("amiga")
                    .build(),
            );
        }
        arcade_catalog(
            games,
            vec![arcade_system("arcade", 5), arcade_system("amiga", 3)],
        )
    }

    fn catalog_system_index(catalog: &ArcadeCatalog, system_id: &str) -> usize {
        catalog
            .systems
            .iter()
            .position(|system| system.id == system_id)
            .expect("system should exist")
    }

    fn reordered_arcade_catalog() -> ArcadeCatalog {
        arcade_catalog(
            vec![
                arcade_game("Arcade 4")
                    .path("/media/fat/_Arcade/arcade-4.mra")
                    .build(),
                arcade_game("Arcade 2")
                    .path("/media/fat/_Arcade/arcade-2.mra")
                    .build(),
                arcade_game("Arcade 0")
                    .path("/media/fat/_Arcade/arcade-0.mra")
                    .build(),
            ],
            vec![arcade_system("arcade", 3)],
        )
    }

    fn filter_catalog() -> ArcadeCatalog {
        let games = vec![
            arcade_game("Astro 1978")
                .path("/media/fat/_Arcade/astro-1978.mra")
                .year(1978)
                .manufacturer("Atari")
                .players(1)
                .control("lightgun")
                .build(),
            arcade_game("Battle 1981")
                .path("/media/fat/_Arcade/battle-1981.mra")
                .year(1981)
                .manufacturer("Capcom")
                .players(2)
                .control("joy")
                .build(),
            arcade_game("Brawl 1988")
                .path("/media/fat/_Arcade/brawl-1988.mra")
                .year(1988)
                .manufacturer("Capcom")
                .players(4)
                .control("only_buttons")
                .build(),
        ];
        arcade_catalog(games, vec![arcade_system("arcade", 3)])
    }

    fn alphabet_catalog() -> ArcadeCatalog {
        let games = [
            "1942",
            "Asteroids",
            "Bubble Bobble",
            "Horizon",
            "Hydra",
            "Pac-Man",
        ]
        .into_iter()
        .map(|title| arcade_game(title).build())
        .collect();
        arcade_catalog(games, vec![arcade_system("arcade", 6)])
    }

    fn deferred_search_catalog() -> ArcadeCatalog {
        ArcadeCatalog::new_with_deferred_text_indexes(
            Path::new("/media/fat/_Arcade").to_path_buf(),
            vec![
                arcade_game("Street Fighter II")
                    .path("/media/fat/_Arcade/sf2.mra")
                    .year(1991)
                    .manufacturer("Capcom")
                    .control("doublejoy")
                    .build(),
                arcade_game("Pac-Man")
                    .path("/media/fat/_Arcade/pacman.mra")
                    .year(1980)
                    .manufacturer("Namco")
                    .control("trackball")
                    .build(),
            ],
            vec![arcade_system("arcade", 2)],
            Vec::new(),
        )
    }

    fn many_manufacturer_catalog() -> ArcadeCatalog {
        let games = (0..12)
            .map(|idx| {
                arcade_game(format!("Maker Game {idx}"))
                    .path(format!("/media/fat/_Arcade/maker-game-{idx}.mra"))
                    .year(1980 + idx as u16)
                    .manufacturer(format!("Maker {idx:02}"))
                    .control("Test")
                    .build()
            })
            .collect();
        arcade_catalog(games, vec![arcade_system("arcade", 12)])
    }

    fn pad_with(mut set: impl FnMut(&mut PadState)) -> PadState {
        let mut pad = PadState::default();
        set(&mut pad);
        pad
    }

    fn ordered_event(
        sequence: u64,
        press_id: u64,
        action: crate::input_event::LogicalAction,
        phase: InputPhase,
    ) -> InputEvent {
        InputEvent {
            source: crate::input_event::InputSourceId {
                kind: crate::input_event::InputSourceKind::Preview,
                instance: 1,
            },
            source_epoch: crate::input_event::SourceEpoch(1),
            sequence,
            press_id: crate::input_event::PressId(press_id),
            captured_at_us: sequence,
            action,
            phase,
        }
    }

    #[test]
    fn ordered_menu_taps_are_not_collapsed() {
        let catalog = image_less_amiga_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Settings;
        let now = Instant::now();
        for (sequence, press_id, phase) in [
            (1, 1, InputPhase::Pressed),
            (2, 1, InputPhase::Released),
            (3, 2, InputPhase::Pressed),
            (4, 2, InputPhase::Released),
        ] {
            nav.handle_action_with_navigation_intents(
                &ordered_event(
                    sequence,
                    press_id,
                    crate::input_event::LogicalAction::Down,
                    phase,
                ),
                now,
                &catalog,
            );
        }
        assert_eq!(nav.settings_selected, 2);
    }

    fn release(nav: &mut LauncherNav, catalog: &ArcadeCatalog, t: Instant, ms: u64) {
        let _ = nav.handle_input(&PadState::default(), t + Duration::from_millis(ms), catalog);
    }

    fn tap(
        nav: &mut LauncherNav,
        catalog: &ArcadeCatalog,
        t: Instant,
        ms: &mut u64,
        pad: &PadState,
    ) {
        let _ = nav.handle_input(pad, t + Duration::from_millis(*ms), catalog);
        *ms += 16;
        release(nav, catalog, t, *ms);
        *ms += 16;
    }

    fn open_filter_drawer(nav: &mut LauncherNav, catalog: &ArcadeCatalog, t: Instant, ms: u64) {
        let press_left = pad_with(|pad| pad.dpad_left = true);
        let _ = nav.handle_input(&press_left, t + Duration::from_millis(ms), catalog);
        release(nav, catalog, t, ms + 16);
        let _ = nav.handle_input(&press_left, t + Duration::from_millis(ms + 32), catalog);
        release(nav, catalog, t, ms + 48);
    }

    fn assert_no_catalog_loads_during(action: impl FnOnce()) {
        library_db::reset_catalog_load_counters();
        action();
        assert_eq!(
            library_db::catalog_load_counters(),
            library_db::CatalogLoadCounters::default()
        );
    }

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("mister-magik-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn arcade_opens_with_first_row_selected() {
        let nav = ArcadeNav::new();
        assert_eq!(nav.selected, 0);
        assert_eq!(nav.visual_index, 0.0);
        assert_eq!(nav.scroll_y, 0);
    }

    #[test]
    fn launcher_keeps_the_empty_arcade_shell_unavailable() {
        let catalog = arcade_catalog(vec![], vec![]);
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);

        nav.sync_launcher_taxonomy(&catalog);
        assert_eq!(nav.current_menu_count(), 1);
        assert_eq!(
            nav.current_menu_items()[0].id,
            crate::arcade_catalog::MENU_ARCADE_SYSTEM_ID
        );
        assert!(
            !nav.menu_item_catalog_presentation(&nav.current_menu_items()[0])
                .available
        );

        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());

        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.selected, 0);
        assert_eq!(nav.scroll_x, 0);
    }

    #[test]
    fn launcher_home_hold_accelerates_to_constant_speed_then_spring_settles_forward() {
        let catalog = arcade_catalog(
            Vec::new(),
            (0..10)
                .map(|index| arcade_system(format!("system-{index}"), 1))
                .collect(),
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("menu:consoles:other"));
        let held_right = pad_with(|pad| pad.dpad_right = true);
        let start = Instant::now();

        nav.handle_input(&held_right, start, &catalog);
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.scroll_x, 0);
        assert!(!nav.home_horizontal_repeat_active());

        nav.handle_input(&held_right, start + Duration::from_millis(199), &catalog);
        assert_eq!(nav.scroll_x, 0);
        assert!(!nav.home_horizontal_repeat_active());

        let mut previous_scroll = nav.scroll_x;
        for frame in 0..30 {
            nav.handle_input(
                &held_right,
                start + Duration::from_millis(200 + frame * 16),
                &catalog,
            );
            assert!(nav.home_horizontal_repeat_active());
            assert!(nav.scroll_x >= previous_scroll);
            previous_scroll = nav.scroll_x;

            let selected_left = nav.selected as i32 * home_tile_pitch();
            let selected_right = selected_left + HOME_TILE_WIDTH;
            assert!(selected_left >= nav.scroll_x);
            assert!(selected_right <= nav.scroll_x + HOME_LIST_VISIBLE_W);
        }
        assert!(
            (nav.home_scroll_animation.velocity() - HOME_SCROLL_SPEED_PX_PER_SECOND).abs() < 1e-9
        );

        nav.handle_input(
            &PadState::default(),
            start + Duration::from_millis(680),
            &catalog,
        );
        assert!(!nav.home_horizontal_repeat_active());
        assert!(nav.scroll_x >= previous_scroll);
        let target = nav.home_scroll_animation.target();
        assert!(target >= nav.scroll_x as f64);

        let mut previous = nav.scroll_x;
        for frame in 1..=120 {
            nav.handle_input(
                &PadState::default(),
                start + Duration::from_millis(680 + frame * 16),
                &catalog,
            );
            assert!(nav.scroll_x >= previous);
            previous = nav.scroll_x;
        }
        assert!(nav.home_scroll_animation.is_settled());
        assert_eq!(nav.scroll_x as f64, target);
    }

    #[test]
    fn launcher_home_discrete_moves_animate_only_when_the_viewport_pages() {
        let catalog = arcade_catalog(
            Vec::new(),
            (0..10)
                .map(|index| arcade_system(format!("system-{index}"), 1))
                .collect(),
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("menu:consoles:other"));
        let held_right = pad_with(|pad| pad.dpad_right = true);
        let start = Instant::now();

        for step in 0..3 {
            let pressed_at = start + Duration::from_millis(step * 100);
            nav.handle_input(&held_right, pressed_at, &catalog);
            nav.handle_input(
                &PadState::default(),
                pressed_at + Duration::from_millis(40),
                &catalog,
            );
        }

        assert_eq!(nav.selected, 3);
        assert_eq!(nav.scroll_x, 0);
        assert!(nav.home_scroll_animation.is_settled());

        let page_press_at = start + Duration::from_millis(300);
        nav.handle_input(&held_right, page_press_at, &catalog);
        assert_eq!(nav.selected, 4);
        assert_eq!(nav.scroll_x, 0);
        assert_eq!(
            nav.home_scroll_animation.target(),
            (4 * home_tile_pitch()) as f64
        );
        assert!(!nav.home_scroll_animation.is_settled());

        let mut previous_scroll = nav.scroll_x;
        for frame in 1..=120 {
            nav.handle_input(
                &PadState::default(),
                page_press_at + Duration::from_millis(frame * 16),
                &catalog,
            );
            assert!(nav.scroll_x >= previous_scroll);
            previous_scroll = nav.scroll_x;
        }
        assert_eq!(nav.scroll_x, 4 * home_tile_pitch());
        assert!(nav.home_scroll_animation.is_settled());
    }

    #[test]
    fn home_viewport_pages_only_when_focus_crosses_an_edge() {
        let half_tile = (HOME_TILE_WIDTH + 1) / 2;
        assert_eq!(
            4 * HOME_TILE_WIDTH + 4 * HOME_TILE_GAP + half_tile,
            HOME_LIST_VISIBLE_W
        );

        let mut scroll_x = 0;
        keep_home_visible(3, &mut scroll_x, 10);
        assert_eq!(scroll_x, 0);

        keep_home_visible(4, &mut scroll_x, 10);
        assert_eq!(scroll_x, 4 * home_tile_pitch());

        keep_home_visible(7, &mut scroll_x, 10);
        assert_eq!(scroll_x, 4 * home_tile_pitch());

        keep_home_visible(8, &mut scroll_x, 10);
        assert_eq!(scroll_x, home_max_scroll(10));

        keep_home_visible(5, &mut scroll_x, 10);
        assert_eq!(
            scroll_x,
            5 * home_tile_pitch() + HOME_TILE_WIDTH - HOME_LIST_VISIBLE_W
        );

        keep_home_visible(1, &mut scroll_x, 10);
        assert_eq!(scroll_x, 0);

        let between_tiles = (2 * home_tile_pitch() + 40) as f64;
        let omega = SpringConfiguration::smooth().angular_frequency();
        assert_eq!(
            home_directional_spring_target(between_tiles, 0.0, 10, 1, omega),
            (3 * home_tile_pitch()) as f64
        );
        assert_eq!(
            home_directional_spring_target(between_tiles, 0.0, 10, -1, omega),
            (2 * home_tile_pitch()) as f64
        );
    }

    #[test]
    fn home_release_at_end_caps_velocity_and_never_recoils() {
        let target = home_max_scroll(10) as f64;
        let mut spring = SpringAnimation::new(target - 10.0, SpringConfiguration::smooth());
        spring.set_state(target - 10.0, HOME_SCROLL_SPEED_PX_PER_SECOND);
        retarget_home_spring_monotonically(&mut spring, target);

        let mut previous = spring.value();
        for _ in 0..120 {
            spring.advance(Duration::from_secs_f64(1.0 / 60.0));
            clamp_home_spring_at_target(&mut spring, 1);
            assert!(spring.value() >= previous);
            assert!(spring.value() <= target);
            previous = spring.value();
        }
        assert!(spring.is_settled());
        assert_eq!(spring.value(), target);
    }

    #[test]
    fn registry_only_arcade_activation_waits_for_the_runtime_commit() {
        let catalog = arcade_catalog(vec![], vec![arcade_system("arcade", 911)]);
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);

        let event = nav
            .handle_input_with_collection_intents(&press_a, t0, &catalog)
            .expect("registry-only collection intent");

        assert_eq!(event.action, LauncherAction::OpenCollection);
        assert_eq!(
            event.path.as_deref(),
            Some(crate::arcade_catalog::MENU_ARCADE_SYSTEM_ID)
        );
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.selected, 0);
        assert_eq!(nav.arcade.selected, 0);
    }

    #[test]
    fn arcade_alphabet_left_opens_and_right_jumps_to_highlighted_group() {
        let catalog = alphabet_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_left = pad_with(|pad| pad.dpad_left = true);
        let press_right = pad_with(|pad| pad.dpad_right = true);

        let _ = nav.handle_input(&press_a, t0, &catalog);
        release(&mut nav, &catalog, t0, 16);
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.arcade_filter.active, ArcadeFilter::All);

        let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(32), &catalog);
        release(&mut nav, &catalog, t0, 48);
        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Alphabet);
        assert_eq!(nav.arcade_filter.selected, 0);
        assert_eq!(
            nav.arcade_filter_items(&catalog, "arcade"),
            vec![
                ArcadeDrawerItem {
                    label: "0-9".to_string(),
                    count: 1,
                    active: false,
                },
                ArcadeDrawerItem {
                    label: "A".to_string(),
                    count: 1,
                    active: false,
                },
                ArcadeDrawerItem {
                    label: "B".to_string(),
                    count: 1,
                    active: false,
                },
                ArcadeDrawerItem {
                    label: "H".to_string(),
                    count: 2,
                    active: false,
                },
                ArcadeDrawerItem {
                    label: "P".to_string(),
                    count: 1,
                    active: false,
                },
            ]
        );

        let _ = nav.handle_input(&press_right, t0 + Duration::from_millis(64), &catalog);
        assert_eq!(nav.arcade_filter.active, ArcadeFilter::All);
        assert!(!nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade.selected, 0);
    }

    #[test]
    fn arcade_alphabet_lists_digits_and_existing_letters_only() {
        let catalog = alphabet_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;

        nav.open_arcade_alphabet(&catalog, "arcade");

        assert_eq!(
            nav.arcade_filter_items(&catalog, "arcade"),
            vec![
                ArcadeDrawerItem {
                    label: "0-9".to_string(),
                    count: 1,
                    active: false,
                },
                ArcadeDrawerItem {
                    label: "A".to_string(),
                    count: 1,
                    active: false,
                },
                ArcadeDrawerItem {
                    label: "B".to_string(),
                    count: 1,
                    active: false,
                },
                ArcadeDrawerItem {
                    label: "H".to_string(),
                    count: 2,
                    active: false,
                },
                ArcadeDrawerItem {
                    label: "P".to_string(),
                    count: 1,
                    active: false,
                },
            ]
        );
    }

    #[test]
    fn arcade_alphabet_opens_at_current_game_letter_and_jumps() {
        let catalog = alphabet_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Arcade;
        nav.arcade.restore_position(3, 3 * ARCADE_ROW_HEIGHT, 6);

        nav.open_arcade_alphabet(&catalog, "arcade");

        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Alphabet);
        assert_eq!(nav.arcade_filter.selected, 3);
        assert_eq!(nav.arcade_filter.scroll_y, 3 * ARCADE_ROW_HEIGHT);

        let _ = nav.handle_input(
            &pad_with(|pad| pad.dpad_right = true),
            t0 + Duration::from_millis(16),
            &catalog,
        );

        assert!(!nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade.selected, 3);
        assert_eq!(
            nav.active_arcade_game_at(&catalog, "arcade", nav.arcade.selected)
                .map(|game| game.title.as_ref()),
            Some("Horizon")
        );
    }

    #[test]
    fn repeated_arcade_entry_and_filter_navigation_do_not_query_catalog_storage() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_b = pad_with(|pad| pad.btn_b = true);
        let press_left = pad_with(|pad| pad.dpad_left = true);
        let press_down = pad_with(|pad| pad.dpad_down = true);
        let press_right = pad_with(|pad| pad.dpad_right = true);

        assert_no_catalog_loads_during(|| {
            let _ = nav.handle_input(&press_a, t0, &catalog);
            release(&mut nav, &catalog, t0, 16);
            let _ = nav.handle_input(&press_b, t0 + Duration::from_millis(32), &catalog);
            release(&mut nav, &catalog, t0, 48);
            let _ = nav.handle_input(&press_a, t0 + Duration::from_millis(64), &catalog);
            release(&mut nav, &catalog, t0, 80);
            let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(96), &catalog);
            release(&mut nav, &catalog, t0, 112);
            let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(128), &catalog);
            release(&mut nav, &catalog, t0, 144);
            let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(160), &catalog);
            release(&mut nav, &catalog, t0, 176);
            let _ = nav.handle_input(&press_right, t0 + Duration::from_millis(192), &catalog);
            release(&mut nav, &catalog, t0, 208);
            let _ = nav.handle_input(&press_a, t0 + Duration::from_millis(224), &catalog);
        });
    }

    #[test]
    fn launch_return_restore_does_not_query_catalog_storage() {
        let catalog = filter_catalog();
        let state = LaunchReturnState {
            schema_version: LAUNCH_RETURN_STATE_SCHEMA,
            screen: "arcade".to_string(),
            system_id: "arcade".to_string(),
            system_index: 0,
            collection_id: None,
            menu_path: Vec::new(),
            game_path: "/media/fat/_Arcade/battle-1981.mra".to_string(),
            game_index: 0,
            scroll_y: None,
            filter_kind: Some("decade".to_string()),
            filter_value: Some("1980".to_string()),
        };
        let mut nav = LauncherNav::new();

        assert_no_catalog_loads_during(|| {
            assert!(apply_launch_return_state(&mut nav, &catalog, state));
        });
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.arcade_filter.active, ArcadeFilter::Decade(1980));
    }

    #[test]
    fn arcade_search_is_top_level_filter_and_matches_without_catalog_storage() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_left = pad_with(|pad| pad.dpad_left = true);
        let press_down = pad_with(|pad| pad.dpad_down = true);

        assert_no_catalog_loads_during(|| {
            let _ = nav.handle_input(&press_a, t0, &catalog);
            release(&mut nav, &catalog, t0, 16);
            let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(32), &catalog);
            release(&mut nav, &catalog, t0, 48);
            let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(64), &catalog);
            release(&mut nav, &catalog, t0, 80);
            let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(96), &catalog);
            release(&mut nav, &catalog, t0, 112);
            assert_eq!(nav.arcade_filter.selected, 1);
            let _ = nav.handle_input(&press_a, t0 + Duration::from_millis(128), &catalog);
        });

        assert_eq!(nav.arcade_filter.active, ArcadeFilter::Search);
        assert_eq!(nav.arcade_search.pane, ArcadeSearchPane::Keyboard);
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 3);

        nav.arcade_search.query = "battle".to_string();
        nav.refresh_arcade_search_results(&catalog, "arcade");
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 1);
        assert_eq!(
            nav.active_arcade_game_at(&catalog, "arcade", 0)
                .map(|game| game.title.as_ref()),
            Some("Battle 1981")
        );
    }

    #[test]
    fn arcade_search_entry_with_empty_query_does_not_build_deferred_text_indexes() {
        let catalog = deferred_search_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;

        assert!(!catalog.text_indexes_ready());
        nav.enter_arcade_search(&catalog, "arcade");
        nav.ensure_arcade_search_results(&catalog, "arcade");

        assert_eq!(nav.arcade_filter.active, ArcadeFilter::Search);
        assert_eq!(nav.arcade_search.pane, ArcadeSearchPane::Keyboard);
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 2);
        assert!(!catalog.text_indexes_ready());
    }

    #[test]
    fn arcade_search_first_non_empty_query_waits_without_building_then_refreshes() {
        let catalog = deferred_search_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.enter_arcade_search(&catalog, "arcade");

        nav.arcade_search.query = "capcom".to_string();
        nav.ensure_arcade_search_results(&catalog, "arcade");

        assert!(!catalog.text_indexes_ready());
        assert_eq!(nav.arcade_search.status, ArcadeSearchStatus::Searching);
        assert_eq!(nav.arcade_search.pane, ArcadeSearchPane::Keyboard);
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 0);

        assert!(catalog.ensure_text_indexes_ready());
        nav.refresh_arcade_search_if_active(&catalog, "arcade");

        assert_eq!(nav.arcade_search.status, ArcadeSearchStatus::Ready);
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 1);
        assert_eq!(
            nav.active_arcade_game_at(&catalog, "arcade", 0)
                .map(|game| game.title.as_ref()),
            Some("Street Fighter II")
        );
    }

    #[test]
    fn persisted_arcade_search_applies_only_the_current_request() {
        let catalog = deferred_search_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.enter_arcade_search(&catalog, "arcade");
        nav.arcade_search.query = "capcom".to_string();
        nav.ensure_arcade_search_results(&catalog, "arcade");
        let request = nav
            .take_arcade_search_request(&catalog, 7)
            .expect("persisted search request");
        nav.ensure_arcade_search_results(&catalog, "arcade");
        assert!(
            nav.take_arcade_search_request(&catalog, 7).is_none(),
            "repeated UI frames must not replace an in-flight request"
        );
        let mut stale = request.clone();
        stale.request_id = stale.request_id.wrapping_add(1);

        assert!(!nav.apply_arcade_search_result(
            &catalog,
            &stale,
            mister_magik_catalog::persisted_search::PersistedCollectionSearchResult::default(),
        ));
        assert!(nav.apply_arcade_search_result(
            &catalog,
            &request,
            mister_magik_catalog::persisted_search::PersistedCollectionSearchResult {
                matches: vec![
                    mister_magik_catalog::persisted_search::PersistedCollectionMatch {
                        system_id: "arcade".to_string(),
                        ordinal: 0,
                        rank: -1.0,
                    },
                ],
                ..Default::default()
            },
        ));
        assert_eq!(nav.arcade_search.status, ArcadeSearchStatus::Ready);
        assert_eq!(
            nav.active_arcade_game_at(&catalog, "arcade", 0)
                .map(|game| game.title.as_ref()),
            Some("Street Fighter II")
        );
    }

    #[test]
    fn arcade_search_result_focus_launches_selected_match() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.enter_arcade_search(&catalog, "arcade");
        nav.arcade_search.query = "brawl".to_string();
        nav.refresh_arcade_search_results(&catalog, "arcade");
        nav.arcade_search.pane = ArcadeSearchPane::Results;

        let event = nav
            .handle_input(&pad_with(|pad| pad.btn_a = true), Instant::now(), &catalog)
            .expect("search result launch");

        assert_eq!(event.action, LauncherAction::LaunchGame);
        assert_eq!(
            event.path.as_deref(),
            Some("/media/fat/_Arcade/brawl-1988.mra")
        );
    }

    #[test]
    fn arcade_search_accepts_autocomplete_word_with_y() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("Street Fighter II")
                    .path("/media/fat/_Arcade/sf2.mra")
                    .year(1991)
                    .manufacturer("Capcom")
                    .control("doublejoy")
                    .build(),
                arcade_game("Street Hoop")
                    .path("/media/fat/_Arcade/strhoop.mra")
                    .year(1994)
                    .manufacturer("Data East")
                    .control("only_buttons")
                    .build(),
            ],
            vec![arcade_system("arcade", 2)],
        );
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Arcade;
        nav.enter_arcade_search(&catalog, "arcade");

        nav.arcade_search.query = "str".to_string();
        nav.refresh_arcade_search_results(&catalog, "arcade");
        assert_eq!(nav.arcade_search.suggestion, "street");
        let _ = nav.handle_input(&pad_with(|pad| pad.btn_y = true), t0, &catalog);
        assert_eq!(nav.arcade_search.query, "street ");

        release(&mut nav, &catalog, t0, 16);
        nav.arcade_search.query = "street f".to_string();
        nav.refresh_arcade_search_results(&catalog, "arcade");
        assert_eq!(nav.arcade_search.suggestion, "fighter");
        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_y = true),
            t0 + Duration::from_millis(32),
            &catalog,
        );
        assert_eq!(nav.arcade_search.query, "street fighter ");
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 1);
    }

    #[test]
    fn arcade_search_y_does_nothing_without_suggestion() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.enter_arcade_search(&catalog, "arcade");
        nav.arcade_search.query = "x".to_string();
        nav.refresh_arcade_search_results(&catalog, "arcade");

        let _ = nav.handle_input(&pad_with(|pad| pad.btn_y = true), Instant::now(), &catalog);

        assert_eq!(nav.arcade_search.query, "x");
        assert_eq!(nav.arcade_search.suggestion, "");
    }

    #[test]
    fn arcade_search_right_from_short_final_row_enters_results() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.enter_arcade_search(&catalog, "arcade");
        nav.arcade_search.selected_key = ARCADE_SEARCH_KEYS.len() - 1;

        let _ = nav.handle_input(
            &pad_with(|pad| pad.dpad_right = true),
            Instant::now(),
            &catalog,
        );

        assert_eq!(nav.arcade_search.pane, ArcadeSearchPane::Results);
    }

    #[test]
    fn arcade_search_key_actions_are_independent_of_display_labels() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.enter_arcade_search(&catalog, "arcade");

        for (action, expected) in [
            (ArcadeSearchKeyAction::Append("A"), "A"),
            (ArcadeSearchKeyAction::Space, "A "),
            (ArcadeSearchKeyAction::Delete, "A"),
            (ArcadeSearchKeyAction::Clear, ""),
        ] {
            nav.arcade_search.selected_key = ARCADE_SEARCH_KEYS
                .iter()
                .position(|key| key.action == action)
                .expect("search key action");
            nav.activate_arcade_search_key(&catalog, "arcade");
            assert_eq!(nav.arcade_search.query, expected);
        }
    }

    #[test]
    fn arcade_search_backspace_and_empty_query_exit_to_all_games() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Arcade;
        nav.enter_arcade_search(&catalog, "arcade");
        nav.arcade_search.query = "b".to_string();
        nav.refresh_arcade_search_results(&catalog, "arcade");

        let _ = nav.handle_input(&pad_with(|pad| pad.btn_b = true), t0, &catalog);
        assert_eq!(nav.arcade_search.query, "");
        release(&mut nav, &catalog, t0, 16);
        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_b = true),
            t0 + Duration::from_millis(32),
            &catalog,
        );

        assert_eq!(nav.arcade_filter.active, ArcadeFilter::All);
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 3);
    }

    #[test]
    fn launch_return_state_restores_search_query() {
        let catalog = filter_catalog();
        let state = LaunchReturnState {
            schema_version: LAUNCH_RETURN_STATE_SCHEMA,
            screen: "arcade".to_string(),
            system_id: "arcade".to_string(),
            system_index: 0,
            collection_id: None,
            menu_path: Vec::new(),
            game_path: "/media/fat/_Arcade/battle-1981.mra".to_string(),
            game_index: 0,
            scroll_y: None,
            filter_kind: Some("search".to_string()),
            filter_value: Some("battle".to_string()),
        };
        let mut nav = LauncherNav::new();

        assert!(apply_launch_return_state(&mut nav, &catalog, state));

        assert_eq!(nav.arcade_filter.active, ArcadeFilter::Search);
        assert_eq!(nav.arcade_search.query, "battle");
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 1);
        assert_eq!(nav.arcade.selected, 0);
    }

    #[test]
    fn launch_return_state_retries_exact_search_position_after_deferred_indexes() {
        let eager = filter_catalog();
        let catalog = ArcadeCatalog::new_with_deferred_text_indexes(
            eager.root.clone(),
            eager.games.as_ref().clone(),
            eager.systems.clone(),
            Vec::new(),
        );
        let state = LaunchReturnState {
            schema_version: LAUNCH_RETURN_STATE_SCHEMA,
            screen: "arcade".to_string(),
            system_id: "arcade".to_string(),
            system_index: 0,
            collection_id: None,
            menu_path: Vec::new(),
            game_path: "/media/fat/_Arcade/battle-1981.mra".to_string(),
            game_index: 0,
            scroll_y: None,
            filter_kind: Some("search".to_string()),
            filter_value: Some("battle".to_string()),
        };
        let mut nav = LauncherNav::new();

        assert!(!apply_launch_return_state(
            &mut nav,
            &catalog,
            state.clone()
        ));
        assert_eq!(nav.arcade_search.status, ArcadeSearchStatus::Searching);
        assert_eq!(nav.arcade_search.query, "battle");

        assert!(catalog.ensure_text_indexes_ready());
        assert!(apply_launch_return_state(&mut nav, &catalog, state));
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.arcade_filter.active, ArcadeFilter::Search);
        assert_eq!(nav.arcade.selected, 0);
        assert_eq!(
            nav.active_arcade_game_at(&catalog, "arcade", 0)
                .map(|game| game.mra_path.as_ref()),
            Some("/media/fat/_Arcade/battle-1981.mra")
        );
    }

    #[test]
    fn arcade_filter_right_enters_submenu() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_down = pad_with(|pad| pad.dpad_down = true);
        let press_right = pad_with(|pad| pad.dpad_right = true);

        let _ = nav.handle_input(&press_a, t0, &catalog);
        release(&mut nav, &catalog, t0, 16);
        open_filter_drawer(&mut nav, &catalog, t0, 32);
        let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(96), &catalog);
        release(&mut nav, &catalog, t0, 112);
        let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(128), &catalog);
        release(&mut nav, &catalog, t0, 144);
        let _ = nav.handle_input(&press_right, t0 + Duration::from_millis(160), &catalog);

        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Decades);
    }

    #[test]
    fn arcade_filter_one_activation_cannot_cross_two_hierarchy_edges() {
        let catalog = filter_catalog();
        let t0 = Instant::now();
        let cases = [
            (
                ArcadeFilterGroup::Decades,
                ArcadeFilterLevel::Decades,
                ArcadeFilter::Decade(1970),
            ),
            (
                ArcadeFilterGroup::Manufacturers,
                ArcadeFilterLevel::Manufacturers,
                ArcadeFilter::Manufacturer("Atari".to_string()),
            ),
            (
                ArcadeFilterGroup::Players,
                ArcadeFilterLevel::Players,
                ArcadeFilter::Players(1),
            ),
            (
                ArcadeFilterGroup::Controls,
                ArcadeFilterLevel::Controls,
                ArcadeFilter::Control("Buttons Only".to_string()),
            ),
        ];

        for use_right in [false, true] {
            for (group, level, expected_filter) in &cases {
                let activation = pad_with(|pad| {
                    if use_right {
                        pad.dpad_right = true;
                    } else {
                        pad.btn_a = true;
                    }
                });
                let mut nav = LauncherNav::new();
                nav.screen = Screen::Arcade;
                nav.arcade_filter.drawer_open = true;
                nav.arcade_filter.level = ArcadeFilterLevel::Top;
                nav.arcade_filter.selected =
                    nav.arcade_filter_top_group_index(&catalog, "arcade", *group);

                let _ = nav.handle_input(&activation, t0, &catalog);
                assert_eq!(nav.arcade_filter.level, *level);

                // Even if a runtime boundary loses edge history while the
                // control is held, the hierarchy transition stays consumed.
                nav.reset_test_snapshot(&PadState::default());
                let _ = nav.handle_input(&activation, t0 + Duration::from_millis(16), &catalog);
                assert!(nav.arcade_filter.drawer_open);
                assert_eq!(nav.arcade_filter.level, *level);
                assert_eq!(nav.arcade_filter.active, ArcadeFilter::All);

                release(&mut nav, &catalog, t0, 32);
                let _ = nav.handle_input(&activation, t0 + Duration::from_millis(48), &catalog);
                assert!(!nav.arcade_filter.drawer_open);
                assert_eq!(nav.arcade_filter.active, expected_filter.clone());
            }
        }
    }

    #[test]
    fn arcade_filter_top_hides_empty_and_singleton_dimensions() {
        let empty = arcade_catalog(
            vec![arcade_game("Plain").system_id("arcade").build()],
            vec![arcade_system("arcade", 1)],
        );
        let singleton = arcade_catalog(
            vec![
                arcade_game("Known")
                    .system_id("arcade")
                    .year(1984)
                    .manufacturer("Capcom")
                    .control("Shooter")
                    .build(),
            ],
            vec![arcade_system("arcade", 1)],
        );
        let two_metadata_filters = arcade_catalog(
            vec![
                arcade_game("Shooter")
                    .system_id("arcade")
                    .players(1)
                    .control("Shooter")
                    .build(),
                arcade_game("Maze")
                    .system_id("arcade")
                    .players(2)
                    .control("Maze")
                    .build(),
            ],
            vec![arcade_system("arcade", 2)],
        );
        let nav = LauncherNav::new();

        for catalog in [&empty, &singleton] {
            assert_eq!(
                nav.arcade_filter_top_items(catalog, "arcade")
                    .into_iter()
                    .map(|item| item.label)
                    .collect::<Vec<_>>(),
                vec!["Games A-Z", "Search"]
            );
        }
        assert_eq!(
            nav.arcade_filter_top_items(&two_metadata_filters, "arcade")
                .into_iter()
                .map(|item| item.label)
                .collect::<Vec<_>>(),
            vec!["Games A-Z", "Search", "Players", "Controls"]
        );
    }

    #[test]
    fn unknown_player_label_round_trips_to_zero() {
        assert_eq!(player_count_label(0), "Unkown");
        assert_eq!(player_count_from_label("Unkown"), Some(0));
    }

    #[test]
    fn arcade_filter_top_visibility_covers_every_dimension_count_combination() {
        for decades in 0..=2 {
            for manufacturers in 0..=2 {
                for players in 0..=2 {
                    for controls in 0..=2 {
                        let mut first = arcade_game("First").system_id("arcade").build();
                        let mut second = arcade_game("Second").system_id("arcade").build();
                        if decades > 0 {
                            first.year = Some(1984);
                            second.year = Some(if decades == 1 { 1984 } else { 1994 });
                        }
                        if manufacturers > 0 {
                            first.manufacturer = "Capcom".into();
                            second.manufacturer = if manufacturers == 1 {
                                "Capcom".into()
                            } else {
                                "Sega".into()
                            };
                        }
                        if players > 0 {
                            first.players = Some(1);
                            second.players = Some(if players == 1 { 1 } else { 2 });
                        }
                        if controls > 0 {
                            first.control = "Shooter".into();
                            second.control = if controls == 1 {
                                "Shooter".into()
                            } else {
                                "Maze".into()
                            };
                        }
                        let catalog =
                            arcade_catalog(vec![first, second], vec![arcade_system("arcade", 2)]);
                        let nav = LauncherNav::new();
                        let rows = nav.arcade_filter_top_group_items(&catalog, "arcade");
                        let labels = rows
                            .iter()
                            .map(|row| row.item.label.clone())
                            .collect::<Vec<_>>();

                        assert_eq!(labels.contains(&"Decades".to_string()), decades == 2);
                        assert_eq!(
                            labels.contains(&"Manufacturer".to_string()),
                            manufacturers == 2
                        );
                        assert_eq!(labels.contains(&"Players".to_string()), players == 2);
                        assert_eq!(labels.contains(&"Controls".to_string()), controls == 2);
                        assert_eq!(rows[0].item.count, 2);
                        assert_eq!(rows[1].item.count, 2);
                        assert!(
                            rows.iter()
                                .skip(2)
                                .all(|row| row.item.count == 2 && !row.item.active)
                        );

                        for (selected, row) in rows.iter().enumerate().skip(2) {
                            let mut activated = LauncherNav::new();
                            activated.screen = Screen::Arcade;
                            activated.arcade_filter.drawer_open = true;
                            activated.arcade_filter.level = ArcadeFilterLevel::Top;
                            activated.arcade_filter.selected = selected;
                            let items = activated.arcade_filter_items(&catalog, "arcade");
                            activated.activate_arcade_filter_selection(&catalog, "arcade", &items);
                            let expected_level = match row.group {
                                ArcadeFilterGroup::Categories => ArcadeFilterLevel::Categories,
                                ArcadeFilterGroup::Decades => ArcadeFilterLevel::Decades,
                                ArcadeFilterGroup::Manufacturers => {
                                    ArcadeFilterLevel::Manufacturers
                                }
                                ArcadeFilterGroup::Players => ArcadeFilterLevel::Players,
                                ArcadeFilterGroup::Controls => ArcadeFilterLevel::Controls,
                                ArcadeFilterGroup::Games | ArcadeFilterGroup::Search => {
                                    unreachable!()
                                }
                            };
                            assert_eq!(activated.arcade_filter.level, expected_level);
                            assert_eq!(activated.arcade_filter.selected, 0);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn arcade_filter_top_dispatches_pruned_rows_by_group_identity() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("Shooter")
                    .system_id("arcade")
                    .control("Shooter")
                    .build(),
                arcade_game("Maze")
                    .system_id("arcade")
                    .control("Maze")
                    .build(),
            ],
            vec![arcade_system("arcade", 2)],
        );
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade_filter.drawer_open = true;
        nav.arcade_filter.level = ArcadeFilterLevel::Top;
        nav.arcade_filter.selected = 2;
        let items = nav.arcade_filter_items(&catalog, "arcade");

        nav.activate_arcade_filter_selection(&catalog, "arcade", &items);

        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Controls);
    }

    #[test]
    fn remembered_filter_resets_when_dimension_is_hidden() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("Only Shooter")
                    .system_id("arcade")
                    .control("Shooter")
                    .build(),
            ],
            vec![arcade_system("arcade", 1)],
        );
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.arcade_filter.active = ArcadeFilter::Control("Shooter".to_string());

        nav.open_arcade_filter(&catalog, "arcade");

        assert_eq!(nav.arcade_filter.active, ArcadeFilter::All);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Top);
        assert_eq!(nav.arcade_filter.selected, 0);
    }

    #[test]
    fn arcade_filter_applies_decade_and_launches_filtered_game() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_down = pad_with(|pad| pad.dpad_down = true);

        let _ = nav.handle_input(&press_a, t0, &catalog);
        release(&mut nav, &catalog, t0, 16);
        open_filter_drawer(&mut nav, &catalog, t0, 32);
        let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(96), &catalog);
        release(&mut nav, &catalog, t0, 112);
        let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(128), &catalog);
        release(&mut nav, &catalog, t0, 144);
        assert_eq!(nav.arcade_filter.selected, 2);
        let _ = nav.handle_input(&press_a, t0 + Duration::from_millis(160), &catalog);
        release(&mut nav, &catalog, t0, 176);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Decades);
        let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(192), &catalog);
        release(&mut nav, &catalog, t0, 208);
        let _ = nav.handle_input(&press_a, t0 + Duration::from_millis(224), &catalog);
        release(&mut nav, &catalog, t0, 240);

        assert_eq!(nav.arcade_filter.active, ArcadeFilter::Decade(1980));
        assert!(!nav.arcade_filter.drawer_open);
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 2);
        assert_eq!(nav.arcade.selected, 0);

        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(256), &catalog)
            .expect("filtered launch");
        assert_eq!(event.action, LauncherAction::LaunchGame);
        assert_eq!(
            event.path.as_deref(),
            Some("/media/fat/_Arcade/battle-1981.mra")
        );
    }

    #[test]
    fn arcade_search_opens_after_applying_any_structured_filter() {
        let catalog = filter_catalog();
        let cases = [
            (
                ArcadeFilterLevel::Decades,
                2usize,
                ArcadeFilter::Decade(1970),
            ),
            (
                ArcadeFilterLevel::Manufacturers,
                3usize,
                ArcadeFilter::Manufacturer("Atari".to_string()),
            ),
            (ArcadeFilterLevel::Players, 4usize, ArcadeFilter::Players(1)),
            (
                ArcadeFilterLevel::Controls,
                5usize,
                ArcadeFilter::Control("Buttons Only".to_string()),
            ),
        ];
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_left = pad_with(|pad| pad.dpad_left = true);
        let press_up = pad_with(|pad| pad.dpad_up = true);
        let press_down = pad_with(|pad| pad.dpad_down = true);

        for (level, top_index, expected_filter) in cases {
            let mut nav = LauncherNav::new();
            let t0 = Instant::now();
            let mut ms = 0;

            tap(&mut nav, &catalog, t0, &mut ms, &press_a);
            open_filter_drawer(&mut nav, &catalog, t0, ms);
            ms += 64;
            for _ in 0..top_index {
                tap(&mut nav, &catalog, t0, &mut ms, &press_down);
            }
            tap(&mut nav, &catalog, t0, &mut ms, &press_a);
            assert_eq!(nav.arcade_filter.level, level);
            assert_eq!(nav.arcade_filter.selected, 0);
            tap(&mut nav, &catalog, t0, &mut ms, &press_a);

            assert_eq!(nav.arcade_filter.active, expected_filter);
            assert!(!nav.arcade_filter.drawer_open);
            assert_eq!(nav.arcade.selected, 0);

            tap(&mut nav, &catalog, t0, &mut ms, &press_left);
            assert_eq!(
                nav.arcade_filter.level,
                ArcadeFilterLevel::Alphabet,
                "filter {expected_filter:?}"
            );
            tap(&mut nav, &catalog, t0, &mut ms, &press_left);
            assert_eq!(nav.arcade_filter.level, level);
            tap(&mut nav, &catalog, t0, &mut ms, &press_left);
            assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Top);
            assert_eq!(nav.arcade_filter.selected, top_index);

            for _ in 0..(top_index - 1) {
                tap(&mut nav, &catalog, t0, &mut ms, &press_up);
            }
            assert_eq!(nav.arcade_filter.selected, 1);
            tap(&mut nav, &catalog, t0, &mut ms, &press_a);

            assert_eq!(nav.screen, Screen::Arcade);
            assert_eq!(nav.arcade_filter.active, ArcadeFilter::Search);
            assert_eq!(nav.arcade_search.pane, ArcadeSearchPane::Keyboard);
            assert!(!nav.arcade_filter.drawer_open);
            assert_eq!(nav.arcade.selected, 0);
            assert_eq!(nav.arcade.scroll_y, 0);
            assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 3);
        }
    }

    #[test]
    fn arcade_filter_reopens_at_active_submenu_for_specific_filters() {
        let catalog = filter_catalog();
        let cases = [
            (
                ArcadeFilter::Decade(1980),
                ArcadeFilterLevel::Decades,
                "1980's",
                2,
            ),
            (
                ArcadeFilter::Manufacturer("Capcom".to_string()),
                ArcadeFilterLevel::Manufacturers,
                "Capcom",
                3,
            ),
            (
                ArcadeFilter::Players(1),
                ArcadeFilterLevel::Players,
                "1 Player",
                4,
            ),
            (
                ArcadeFilter::Control("Joystick".to_string()),
                ArcadeFilterLevel::Controls,
                "Joystick",
                5,
            ),
        ];
        let press_b = pad_with(|pad| pad.btn_b = true);
        let t0 = Instant::now();

        for (filter, level, label, top_index) in cases {
            let mut nav = LauncherNav::new();
            nav.screen = Screen::Arcade;
            nav.arcade_filter.active = filter;

            nav.open_arcade_filter(&catalog, "arcade");

            assert!(nav.arcade_filter.drawer_open);
            assert_eq!(nav.arcade_filter.level, level);
            let items = nav.arcade_filter_items(&catalog, "arcade");
            assert_eq!(items[nav.arcade_filter.selected].label, label);
            assert!(items[nav.arcade_filter.selected].active);

            let _ = nav.handle_input(&press_b, t0, &catalog);
            assert!(nav.arcade_filter.drawer_open);
            assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Top);
            assert_eq!(nav.arcade_filter.selected, top_index);
        }
    }

    #[test]
    fn arcade_filter_dpad_left_walks_back_through_filter_hierarchy() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_left = pad_with(|pad| pad.dpad_left = true);
        let press_right = pad_with(|pad| pad.dpad_right = true);
        let press_down = pad_with(|pad| pad.dpad_down = true);
        let press_b = pad_with(|pad| pad.btn_b = true);

        let _ = nav.handle_input(&press_a, t0, &catalog);
        release(&mut nav, &catalog, t0, 16);

        let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(32), &catalog);
        release(&mut nav, &catalog, t0, 48);
        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Alphabet);
        assert_eq!(nav.arcade_filter.selected, 0);

        let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(64), &catalog);
        release(&mut nav, &catalog, t0, 80);
        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Top);
        assert_eq!(nav.arcade_filter.selected, 0);

        let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(96), &catalog);
        release(&mut nav, &catalog, t0, 112);
        let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(128), &catalog);
        release(&mut nav, &catalog, t0, 144);
        assert_eq!(nav.arcade_filter.selected, 2);

        let _ = nav.handle_input(&press_right, t0 + Duration::from_millis(160), &catalog);
        release(&mut nav, &catalog, t0, 176);
        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Decades);
        assert_eq!(nav.arcade_filter.selected, 0);

        let _ = nav.handle_input(&press_right, t0 + Duration::from_millis(192), &catalog);
        release(&mut nav, &catalog, t0, 208);
        assert_eq!(nav.arcade_filter.active, ArcadeFilter::Decade(1970));
        assert!(!nav.arcade_filter.drawer_open);

        let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(224), &catalog);
        release(&mut nav, &catalog, t0, 240);
        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Alphabet);
        assert_eq!(nav.arcade_filter.selected, 0);

        let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(256), &catalog);
        release(&mut nav, &catalog, t0, 272);
        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Decades);
        assert_eq!(nav.arcade_filter.selected, 0);

        let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(288), &catalog);
        release(&mut nav, &catalog, t0, 304);
        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Top);
        assert_eq!(nav.arcade_filter.selected, 2);

        let _ = nav.handle_input(&press_left, t0 + Duration::from_millis(320), &catalog);
        release(&mut nav, &catalog, t0, 336);
        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Top);
        assert_eq!(nav.arcade_filter.selected, 2);
        assert_eq!(nav.screen, Screen::Arcade);

        let _ = nav.handle_input(&press_b, t0 + Duration::from_millis(352), &catalog);
        assert!(!nav.arcade_filter.drawer_open);
        assert_eq!(nav.screen, Screen::Home);
    }

    #[test]
    fn arcade_filter_manufacturer_list_uses_velocity_scroll() {
        let catalog = many_manufacturer_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Arcade;
        nav.open_arcade_filter(&catalog, "arcade");
        nav.enter_arcade_filter_level(&catalog, "arcade", ArcadeFilterLevel::Manufacturers);

        let hold_down = pad_with(|pad| pad.dpad_down = true);
        let _ = nav.handle_input(&hold_down, t0, &catalog);

        assert_eq!(nav.arcade_filter.selected, 1);
        assert!(nav.arcade_filter.scroll_y > 0);
        assert!(nav.arcade_filter.scroll_y < ARCADE_ROW_HEIGHT);
        assert!(nav.arcade_filter.visual_index > 0.0);
        assert!(nav.arcade_filter.visual_index < 1.0);

        for frame in 1..=20 {
            let _ = nav.handle_input(&hold_down, t0 + Duration::from_millis(frame * 16), &catalog);
        }

        assert!(nav.arcade_filter.selected > 1);
        assert!(nav.arcade_filter.scroll_y > ARCADE_ROW_HEIGHT);
    }

    #[test]
    fn arcade_filter_b_backs_out_of_submenu() {
        let catalog = filter_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_down = pad_with(|pad| pad.dpad_down = true);
        let press_b = pad_with(|pad| pad.btn_b = true);

        let _ = nav.handle_input(&press_a, t0, &catalog);
        release(&mut nav, &catalog, t0, 16);
        open_filter_drawer(&mut nav, &catalog, t0, 32);
        let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(96), &catalog);
        release(&mut nav, &catalog, t0, 112);
        let _ = nav.handle_input(&press_down, t0 + Duration::from_millis(128), &catalog);
        release(&mut nav, &catalog, t0, 144);
        let _ = nav.handle_input(&press_a, t0 + Duration::from_millis(160), &catalog);
        release(&mut nav, &catalog, t0, 176);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Decades);

        let _ = nav.handle_input(&press_b, t0 + Duration::from_millis(192), &catalog);
        release(&mut nav, &catalog, t0, 208);
        assert!(nav.arcade_filter.drawer_open);
        assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Top);
        assert_eq!(nav.arcade_filter.selected, 0);

        let _ = nav.handle_input(&press_b, t0 + Duration::from_millis(224), &catalog);
        assert!(!nav.arcade_filter.drawer_open);
        assert_eq!(nav.screen, Screen::Home);
    }

    #[test]
    fn launcher_home_selection_clamps_when_catalog_shrinks() {
        let catalog = image_less_amiga_catalog();
        let mut nav = LauncherNav::new();
        nav.selected = 8;
        nav.scroll_x = 9999;

        assert!(
            nav.handle_input(&PadState::default(), Instant::now(), &catalog)
                .is_none()
        );

        assert_eq!(nav.selected, 0);
        assert_eq!(nav.scroll_x, 0);
    }

    #[test]
    fn hierarchy_a_enters_b_returns_one_level_and_home_returns_root() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.sync_launcher_taxonomy(&catalog);

        nav.selected = nav
            .current_menu_items()
            .iter()
            .position(|item| item.id == "menu:consoles")
            .expect("Consoles root item");
        let _ = nav.handle_input(&pad_with(|pad| pad.btn_a = true), t0, &catalog);
        assert_eq!(nav.current_menu_id(), "menu:consoles");
        assert_eq!(nav.screen, Screen::Home);
        release(&mut nav, &catalog, t0, 16);

        nav.selected = nav
            .current_menu_items()
            .iter()
            .position(|item| item.id == "menu:consoles:nintendo")
            .expect("Nintendo menu item");
        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_a = true),
            t0 + Duration::from_millis(32),
            &catalog,
        );
        assert_eq!(nav.current_menu_id(), "menu:consoles:nintendo");
        assert_eq!(nav.current_menu_breadcrumb(), "Consoles");
        release(&mut nav, &catalog, t0, 48);

        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_b = true),
            t0 + Duration::from_millis(64),
            &catalog,
        );
        assert_eq!(nav.current_menu_id(), "menu:consoles");
        release(&mut nav, &catalog, t0, 80);

        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_home = true),
            t0 + Duration::from_millis(96),
            &catalog,
        );
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
    }

    #[test]
    fn collection_intent_keeps_home_unchanged_until_runtime_commits() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("menu:consoles"));
        assert!(nav.open_menu("menu:consoles:nintendo"));
        nav.selected = nav
            .current_menu_items()
            .iter()
            .position(|item| item.id == "n64")
            .expect("N64 item");

        let event = nav
            .handle_input_with_navigation_intents(
                &pad_with(|pad| pad.btn_a = true),
                Instant::now(),
                &catalog,
            )
            .expect("open collection intent");

        assert_eq!(event.action, LauncherAction::OpenCollection);
        assert_eq!(event.path.as_deref(), Some("n64"));
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.current_menu_id(), "menu:consoles:nintendo");
        assert!(nav.commit_navigation_intent(&event, &catalog));
        assert_eq!(nav.screen, Screen::Arcade);
    }

    #[test]
    fn menu_and_back_intents_defer_hierarchy_mutation() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.sync_launcher_taxonomy(&catalog);
        nav.selected = nav
            .current_menu_items()
            .iter()
            .position(|item| item.id == "menu:consoles")
            .expect("Consoles root item");

        let open = nav
            .handle_input_with_navigation_intents(&pad_with(|pad| pad.btn_a = true), t0, &catalog)
            .expect("open-menu intent");
        assert_eq!(open.action, LauncherAction::OpenMenu);
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
        assert!(nav.commit_navigation_intent(&open, &catalog));
        assert_eq!(nav.current_menu_id(), "menu:consoles");

        release(&mut nav, &catalog, t0, 16);
        let back = nav
            .handle_input_with_navigation_intents(
                &pad_with(|pad| pad.btn_b = true),
                t0 + Duration::from_millis(32),
                &catalog,
            )
            .expect("back intent");
        assert_eq!(back.action, LauncherAction::NavigateBack);
        assert_eq!(nav.current_menu_id(), "menu:consoles");
        assert!(nav.commit_navigation_intent(&back, &catalog));
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
    }

    #[test]
    fn home_intent_defers_direct_root_navigation() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("menu:consoles"));
        assert!(nav.open_menu("menu:consoles:nintendo"));

        let home = nav
            .handle_input_with_navigation_intents(
                &pad_with(|pad| pad.btn_home = true),
                t0,
                &catalog,
            )
            .expect("home intent");
        assert_eq!(home.action, LauncherAction::NavigateHome);
        assert_eq!(nav.current_menu_id(), "menu:consoles:nintendo");
        assert!(nav.commit_navigation_intent(&home, &catalog));
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
    }

    #[test]
    fn absorbed_back_press_does_not_fire_after_transition_settlement() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu(crate::launcher_taxonomy::CONSOLES_MENU_ID));
        let held_back = pad_with(|pad| pad.btn_b = true);

        nav.reset_test_snapshot(&held_back);
        assert!(
            nav.handle_input_with_navigation_intents(&held_back, Instant::now(), &catalog,)
                .is_none()
        );
        assert_eq!(
            nav.current_menu_id(),
            crate::launcher_taxonomy::CONSOLES_MENU_ID
        );
    }

    #[test]
    fn hierarchy_remembers_each_menu_view_independently() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![
                arcade_system("arcade", 1),
                arcade_system("atari2600", 1),
                arcade_system("sms", 1),
                arcade_system("psx", 1),
                arcade_system("nes", 1),
                arcade_system("tgfx16", 1),
                arcade_system("colecovision", 1),
                arcade_system("intellivision", 1),
                arcade_system("amiga", 1),
            ],
        );
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        nav.selected = nav
            .current_menu_items()
            .iter()
            .position(|item| item.id == "menu:consoles")
            .expect("Consoles root item");
        nav.scroll_x = home_max_scroll(nav.current_menu_count());
        assert!(nav.open_menu("menu:consoles"));

        nav.selected = nav.current_menu_count() - 1;
        nav.scroll_x = home_max_scroll(nav.current_menu_count());
        assert_eq!(
            nav.current_menu_items()[nav.selected].id,
            "menu:consoles:other"
        );
        assert!(nav.open_menu("menu:consoles:other"));

        assert!(nav.pop_menu());
        assert_eq!(nav.current_menu_id(), "menu:consoles");
        assert_eq!(nav.selected, nav.current_menu_count() - 1);
        assert_eq!(nav.scroll_x, home_max_scroll(nav.current_menu_count()));

        assert!(nav.pop_menu());
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
        assert_eq!(nav.current_menu_items()[nav.selected].id, "menu:consoles");
        assert_eq!(nav.scroll_x, home_max_scroll(nav.current_menu_count()));
    }

    #[test]
    fn leaving_collection_restores_the_highlighted_menu_item() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("Super Mario Bros").system_id("nes").build(),
                arcade_game("Super Mario 64").system_id("n64").build(),
            ],
            vec![arcade_system("nes", 1), arcade_system("n64", 1)],
        );
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("menu:consoles:nintendo"));
        nav.selected = nav
            .current_menu_items()
            .iter()
            .position(|item| item.id == "n64")
            .expect("Nintendo 64");

        let _ = nav.handle_input(&pad_with(|pad| pad.btn_a = true), t0, &catalog);
        assert_eq!(nav.screen, Screen::Arcade);
        release(&mut nav, &catalog, t0, 16);
        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_b = true),
            t0 + Duration::from_millis(32),
            &catalog,
        );

        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.current_menu_id(), "menu:consoles:nintendo");
        assert_eq!(nav.current_menu_items()[nav.selected].id, "n64");
    }

    #[test]
    fn hierarchy_catalog_shrink_returns_home_and_bare_arcade_uses_the_empty_shell() {
        let initial = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        assert!(nav.open_system(&initial, "neogeopocket"));
        assert_eq!(nav.screen, Screen::Arcade);

        let computers_only = ArcadeCatalog::new(
            PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT),
            Vec::new(),
            vec![arcade_system("amiga", 1)],
        );
        nav.sync_launcher_taxonomy(&computers_only);
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
        assert!(nav.active_collection().is_none());
        assert_eq!(nav.selected, 0);
        assert_eq!(nav.scroll_x, 0);

        let empty = ArcadeCatalog::new(
            PathBuf::from(crate::arcade_catalog::DEFAULT_ARCADE_ROOT),
            Vec::new(),
            Vec::new(),
        );
        nav.screen = Screen::Arcade;
        nav.sync_launcher_taxonomy(&empty);
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
        assert_eq!(nav.current_menu_count(), 1);
        assert_eq!(
            nav.active_collection()
                .map(|collection| collection.id.as_str()),
            Some(crate::arcade_catalog::MENU_ARCADE_SYSTEM_ID)
        );
    }

    #[test]
    fn bare_arcade_screen_startup_opens_the_root_arcade_collection() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.selected = catalog.systems.len().saturating_sub(1);

        nav.sync_launcher_taxonomy(&catalog);

        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(
            nav.active_collection_id(),
            Some(crate::arcade_catalog::MENU_ARCADE_SYSTEM_ID)
        );
    }

    #[test]
    fn home_returns_to_root_then_opens_settings_and_b_returns_root() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::for_crt_layout(true);
        let t0 = Instant::now();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("menu:consoles:nintendo"));

        let _ = nav.handle_input(&pad_with(|pad| pad.btn_home = true), t0, &catalog);
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
        release(&mut nav, &catalog, t0, 16);

        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_home = true),
            t0 + Duration::from_millis(32),
            &catalog,
        );
        assert_eq!(nav.screen, Screen::Settings);
        release(&mut nav, &catalog, t0, 48);

        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_b = true),
            t0 + Duration::from_millis(64),
            &catalog,
        );
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
    }

    #[test]
    fn crt_navigation_uses_the_renderer_row_height() {
        let mut nav = LauncherNav::for_crt_layout_with_row_height(true, 24);
        nav.arcade.restore_position(3, 72, 8);

        assert_eq!(nav.arcade.selected, 3);
        assert_eq!(nav.arcade.scroll_y, 72);
        assert_eq!(nav.arcade.visual_index, 3.0);
    }

    #[test]
    fn hdmi_home_keeps_the_previous_settings_focus_navigation() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.sync_launcher_taxonomy(&catalog);

        let _ = nav.handle_input(&pad_with(|pad| pad.btn_home = true), t0, &catalog);
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
        release(&mut nav, &catalog, t0, 16);

        let _ = nav.handle_input(
            &pad_with(|pad| pad.dpad_up = true),
            t0 + Duration::from_millis(32),
            &catalog,
        );
        assert!(nav.settings_focused);
        release(&mut nav, &catalog, t0, 48);

        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_a = true),
            t0 + Duration::from_millis(64),
            &catalog,
        );
        assert_eq!(nav.screen, Screen::Settings);
    }

    #[test]
    fn launch_return_restores_flattened_primary_path() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("handhelds"));
        nav.selected = nav
            .current_menu_items()
            .iter()
            .position(|item| item.id == "neogeopocket")
            .expect("SNK NeoGeo Pocket");
        let _ = nav.handle_input(&pad_with(|pad| pad.btn_a = true), Instant::now(), &catalog);
        let state =
            capture_launch_return_state(&nav, &catalog, "/media/fat/_Arcade/Pocket Tennis.mra")
                .expect("return state");
        assert_eq!(state.collection_id.as_deref(), Some("neogeopocket"));
        assert_eq!(state.menu_path, vec![ROOT_MENU_ID, "menu:handhelds"]);

        let mut restored = LauncherNav::new();
        assert!(apply_launch_return_state(
            &mut restored,
            &catalog,
            state.clone()
        ));
        assert_eq!(restored.menu_path(), &[ROOT_MENU_ID, "menu:handhelds"]);

        let mut legacy = state;
        legacy.schema_version = 2;
        legacy.collection_id = None;
        legacy.menu_path.clear();
        let mut legacy_restored = LauncherNav::new();
        assert!(apply_launch_return_state(
            &mut legacy_restored,
            &catalog,
            legacy
        ));
        assert_eq!(
            legacy_restored.menu_path(),
            &[ROOT_MENU_ID, "menu:handhelds"]
        );
    }

    #[test]
    fn launcher_about_opens_and_navigates_licenses() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::About;
        nav.about_selected = 1;
        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert_eq!(nav.screen, Screen::Licenses);
        release(&mut nav, &catalog, t0, 16);
        let down = pad_with(|pad| pad.dpad_down = true);
        assert!(
            nav.handle_input(&down, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        assert_eq!(nav.licenses_selected, 1);
        release(&mut nav, &catalog, t0, 48);
        assert!(
            nav.handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
                .is_none()
        );
        assert!(nav.licenses_expanded);
        release(&mut nav, &catalog, t0, 80);
        assert!(
            nav.handle_input(&down, t0 + Duration::from_millis(96), &catalog)
                .is_none()
        );
        assert_eq!(nav.licenses_scroll.selected, 3);
        assert!(nav.licenses_scroll_active());
        assert_eq!(
            nav.licenses_scroll.scroll_animation.configuration(),
            SpringConfiguration::smooth()
        );
        release(&mut nav, &catalog, t0, 112);
        assert!(nav.licenses_scroll.scroll_animation.value() > 0.0);
        let back = pad_with(|pad| pad.btn_b = true);
        assert!(
            nav.handle_input(&back, t0 + Duration::from_millis(128), &catalog)
                .is_none()
        );
        assert!(!nav.licenses_expanded);
        assert_eq!(nav.licenses_scroll.selected, 0);
        release(&mut nav, &catalog, t0, 144);
        assert!(
            nav.handle_input(&back, t0 + Duration::from_millis(160), &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::About);
    }

    #[test]
    fn launcher_settings_toggles_and_persists_reduce_motion() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Settings;
        nav.settings_selected = SETTINGS_REDUCE_MOTION_SELECTED;

        let event = nav
            .handle_input(&pad_with(|pad| pad.btn_a = true), Instant::now(), &catalog)
            .expect("reduce motion should persist");

        assert!(nav.settings.reduce_motion);
        assert_eq!(event.action, LauncherAction::PersistSettings);
        assert!(
            event
                .settings
                .is_some_and(|settings| settings.reduce_motion)
        );
    }

    #[test]
    fn launcher_settings_opens_about_then_info_and_b_returns_through_hierarchy() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_b = pad_with(|pad| pad.btn_b = true);

        nav.screen = Screen::Settings;
        nav.settings_selected = SETTINGS_ABOUT_SELECTED;
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert_eq!(nav.screen, Screen::About);
        release(&mut nav, &catalog, t0, 16);
        assert!(
            nav.handle_input(&press_b, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::Settings);
        release(&mut nav, &catalog, t0, 48);

        nav.settings_selected = SETTINGS_ABOUT_SELECTED;
        assert!(
            nav.handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::About);
        release(&mut nav, &catalog, t0, 80);
        assert!(
            nav.handle_input(&press_a, t0 + Duration::from_millis(96), &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::Info);
        release(&mut nav, &catalog, t0, 112);
        assert!(
            nav.handle_input(&press_b, t0 + Duration::from_millis(128), &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::About);
    }

    #[test]
    fn screensaver_settings_preview_emits_immediate_action() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Screensaver;
        nav.screensaver_selected = 2;

        let event = nav
            .handle_input(&pad_with(|pad| pad.btn_a = true), Instant::now(), &catalog)
            .expect("preview action");

        assert_eq!(event.action, LauncherAction::PreviewScreensaver);
        assert_eq!(event.path, None);
        assert_eq!(event.settings, None);
    }

    #[test]
    fn screensaver_setting_change_emits_persistence_effect() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Screensaver;
        nav.screensaver_selected = 0;
        let previous = nav.settings.screensaver_enabled;

        let event = nav
            .handle_input(&pad_with(|pad| pad.btn_a = true), Instant::now(), &catalog)
            .expect("settings persistence effect");

        assert_eq!(event.action, LauncherAction::PersistSettings);
        assert_eq!(nav.settings.screensaver_enabled, !previous);
        assert_eq!(event.settings, Some(nav.settings.clone()));
    }

    #[test]
    fn license_hold_uses_arcade_continuous_scroll_transition() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Licenses;
        nav.licenses_expanded = true;

        let down = pad_with(|pad| pad.dpad_down = true);
        assert!(nav.handle_input(&down, t0, &catalog).is_none());
        assert_eq!(nav.licenses_scroll.selected, 3);
        assert!(!nav.licenses_scroll.scroll.continuous_active);

        assert!(
            nav.handle_input(
                &down,
                t0 + ARCADE_QUICK_TAP_MAX + Duration::from_millis(1),
                &catalog,
            )
            .is_none()
        );
        assert!(nav.licenses_scroll.scroll.continuous_active);
        assert!(nav.licenses_scroll.scroll_animation.velocity() > 0.0);
        assert_eq!(
            nav.licenses_scroll.row_height,
            LICENSE_SCROLL_LINE_PX as i32
        );
        assert_eq!(nav.licenses_scroll.step_rows, 3);

        let release_at = t0 + ARCADE_QUICK_TAP_MAX + Duration::from_millis(17);
        assert!(
            nav.handle_input(&PadState::default(), release_at, &catalog)
                .is_none()
        );
        assert!(!nav.licenses_scroll.scroll.continuous_active);
        let count = crate::licenses::max_scroll_line(nav.licenses_selected) + 1;
        settle(&mut nav.licenses_scroll, count, release_at);
        assert!(nav.licenses_scroll.is_settled());
    }

    #[test]
    fn license_three_line_steps_respect_direction_and_bounds() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Licenses;
        nav.licenses_expanded = true;
        let down = pad_with(|pad| pad.dpad_down = true);
        let up = pad_with(|pad| pad.dpad_up = true);

        assert!(nav.handle_input(&down, t0, &catalog).is_none());
        assert_eq!(nav.licenses_scroll.selected, 3);
        release(&mut nav, &catalog, t0, 16);
        assert!(
            nav.handle_input(&up, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        assert_eq!(nav.licenses_scroll.selected, 0);
        release(&mut nav, &catalog, t0, 48);
        assert!(
            nav.handle_input(&up, t0 + Duration::from_millis(64), &catalog)
                .is_none()
        );
        assert_eq!(nav.licenses_scroll.selected, 0);

        let count = crate::licenses::max_scroll_line(nav.licenses_selected) + 1;
        nav.licenses_scroll.selected = count - 1;
        nav.licenses_scroll.snap_to_selected();
        release(&mut nav, &catalog, t0, 80);
        assert!(
            nav.handle_input(&down, t0 + Duration::from_millis(96), &catalog)
                .is_none()
        );
        assert_eq!(nav.licenses_scroll.selected, count - 1);
    }

    #[test]
    fn launcher_reopens_system_at_in_memory_arcade_position() {
        let catalog = multi_game_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let back = pad_with(|pad| pad.btn_b = true);

        assert!(nav.open_default_arcade(&catalog));
        assert_eq!(nav.screen, Screen::Arcade);
        assert!(
            nav.handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog
            )
            .is_none()
        );

        nav.arcade.selected = 3;
        nav.arcade.snap_to_selected();
        assert!(
            nav.handle_input(&back, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::Home);
        assert!(
            nav.handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(48),
                &catalog
            )
            .is_none()
        );

        assert!(
            nav.handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
                .is_none()
        );
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.arcade.selected, 3);
        assert_eq!(nav.arcade.scroll_y, 3 * ARCADE_ROW_HEIGHT);
    }

    #[test]
    fn amiga_opens_without_an_implicit_genre_filter() {
        let catalog = amiga_games_and_demos_catalog();
        let mut nav = LauncherNav::new();

        assert!(nav.open_system(&catalog, "amiga"));

        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.arcade_filter.active, ArcadeFilter::All);
        let visible = nav.active_arcade_game_view(&catalog, "amiga");
        assert_eq!(visible.len(), 3);
    }

    #[test]
    fn launcher_remembers_game_list_position_per_system() {
        let catalog = multi_game_catalog();
        let mut nav = LauncherNav::new();

        nav.selected = 0;
        nav.screen = Screen::Arcade;
        nav.arcade.selected = 4;
        nav.arcade.snap_to_selected();
        nav.save_game_list_state("arcade");

        nav.selected = 1;
        nav.restore_game_list_state("amiga", catalog.system_game_count("amiga"));
        assert_eq!(nav.arcade.selected, 0);

        nav.arcade.selected = 2;
        nav.arcade.snap_to_selected();
        nav.save_game_list_state("amiga");

        nav.selected = 0;
        nav.restore_game_list_state("arcade", catalog.system_game_count("arcade"));
        assert_eq!(nav.arcade.selected, 4);
        assert_eq!(nav.arcade.scroll_y, 4 * ARCADE_ROW_HEIGHT);

        nav.selected = 1;
        nav.restore_game_list_state("amiga", catalog.system_game_count("amiga"));
        assert_eq!(nav.arcade.selected, 2);
        assert_eq!(nav.arcade.scroll_y, 2 * ARCADE_ROW_HEIGHT);
    }

    #[test]
    fn launcher_restores_saved_game_position_without_sub_row_offset() {
        let catalog = multi_game_catalog();
        let mut nav = LauncherNav::new();

        nav.selected = catalog_system_index(&catalog, "arcade");
        nav.screen = Screen::Arcade;
        nav.arcade.selected = 3;
        nav.arcade.scroll_y = 3 * ARCADE_ROW_HEIGHT + 6;
        nav.save_game_list_state("arcade");

        nav.arcade.reset();
        nav.restore_game_list_state("arcade", catalog.system_game_count("arcade"));

        assert_eq!(nav.arcade.selected, 3);
        assert_eq!(nav.arcade.scroll_y, 3 * ARCADE_ROW_HEIGHT);
        assert_eq!(nav.arcade.visual_index, 3.0);
        assert!(nav.arcade.is_settled());
    }

    #[test]
    fn arcade_restore_position_treats_selected_row_as_authority() {
        let mut nav = ArcadeNav::new();

        nav.restore_position(4, 4 * ARCADE_ROW_HEIGHT - 5, 8);

        assert_eq!(nav.selected, 4);
        assert_eq!(nav.scroll_y, 4 * ARCADE_ROW_HEIGHT);
        assert_eq!(nav.visual_index, 4.0);
        assert!(nav.is_settled());
    }

    #[test]
    fn launch_return_state_captures_arcade_location() {
        let catalog = multi_game_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.selected = catalog_system_index(&catalog, "arcade");
        nav.arcade.selected = 2;
        nav.arcade.snap_to_selected();

        let state = capture_launch_return_state(&nav, &catalog, "/media/fat/_Arcade/arcade-2.mra")
            .expect("state should capture");

        assert_eq!(state.schema_version, LAUNCH_RETURN_STATE_SCHEMA);
        assert_eq!(state.screen, "arcade");
        assert_eq!(state.system_id, "arcade");
        assert_eq!(state.system_index, catalog_system_index(&catalog, "arcade"));
        assert_eq!(state.game_path, "/media/fat/_Arcade/arcade-2.mra");
        assert_eq!(state.game_index, 2);
    }

    #[test]
    fn launch_return_state_restores_by_path_after_catalog_reorder() {
        let catalog = multi_game_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.selected = catalog_system_index(&catalog, "arcade");
        nav.arcade.selected = 2;
        nav.arcade.snap_to_selected();
        let state = capture_launch_return_state(&nav, &catalog, "/media/fat/_Arcade/arcade-2.mra")
            .expect("state should capture");

        let mut restored = LauncherNav::new();
        assert!(apply_launch_return_state(
            &mut restored,
            &reordered_arcade_catalog(),
            state
        ));

        assert_eq!(restored.screen, Screen::Arcade);
        assert_eq!(
            restored.selected,
            catalog_system_index(&reordered_arcade_catalog(), "arcade")
        );
        assert_eq!(restored.arcade.selected, 1);
        assert_eq!(restored.arcade.scroll_y, ARCADE_ROW_HEIGHT);
    }

    #[test]
    fn launch_return_state_rejects_a_missing_exact_game() {
        let state = LaunchReturnState {
            schema_version: LAUNCH_RETURN_STATE_SCHEMA,
            screen: "arcade".into(),
            system_id: "missing-system".into(),
            system_index: 99,
            collection_id: None,
            menu_path: Vec::new(),
            game_path: "/missing.mra".into(),
            game_index: 99,
            scroll_y: None,
            filter_kind: Some("all".into()),
            filter_value: None,
        };
        let catalog = reordered_arcade_catalog();
        let mut restored = LauncherNav::new();

        assert!(!apply_launch_return_state(&mut restored, &catalog, state));
    }

    #[test]
    fn launch_return_state_file_is_consumed_and_invalid_state_is_removed() {
        let root = unique_temp_dir("launch-return-state");
        let path = root.join("state.json");
        let state = LaunchReturnState {
            schema_version: LAUNCH_RETURN_STATE_SCHEMA,
            screen: "arcade".into(),
            system_id: "arcade".into(),
            system_index: 0,
            collection_id: None,
            menu_path: Vec::new(),
            game_path: "/media/fat/_Arcade/arcade-2.mra".into(),
            game_index: 2,
            scroll_y: None,
            filter_kind: Some("all".into()),
            filter_value: None,
        };

        save_launch_return_state_at(&path, &state).expect("save return state");
        assert_eq!(
            SystemLauncherPersistence::load_return_state_at(&path).expect("load return state"),
            Some(state)
        );
        SystemLauncherPersistence::clear_return_state_at(&path).expect("clear return state");
        assert!(!path.exists());

        std::fs::write(&path, "{not-json").expect("write invalid state");
        assert_eq!(
            SystemLauncherPersistence::load_return_state_at(&path)
                .expect_err("invalid return state")
                .kind(),
            LauncherEffectFailureKind::MalformedResponse
        );
        SystemLauncherPersistence::clear_return_state_at(&path).expect("clear invalid state");
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launch_return_state_remove_deletes_pending_state() {
        let root = unique_temp_dir("launch-return-state-remove");
        let path = root.join("state.json");
        let state = LaunchReturnState {
            schema_version: LAUNCH_RETURN_STATE_SCHEMA,
            screen: "arcade".into(),
            system_id: "arcade".into(),
            system_index: 0,
            collection_id: None,
            menu_path: Vec::new(),
            game_path: "/media/fat/_Arcade/arcade-2.mra".into(),
            game_index: 2,
            scroll_y: None,
            filter_kind: Some("all".into()),
            filter_value: None,
        };

        save_launch_return_state_at(&path, &state).expect("save return state");
        assert!(path.exists());
        SystemLauncherPersistence::clear_return_state_at(&path).expect("clear return state");

        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launcher_confirm_defaults_cancel_destructive_actions_until_confirmed() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Settings;
        nav.settings_selected = SETTINGS_REBUILD_SELECTED;

        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert_eq!(nav.confirm_action, Some(ConfirmAction::RebuildDatabase));
        assert_eq!(nav.confirm_selected, 0);
        assert!(
            nav.handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog,
            )
            .is_none()
        );

        let right = pad_with(|pad| pad.dpad_right = true);
        assert!(
            nav.handle_input(&right, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        assert_eq!(nav.confirm_selected, 1);
        assert!(
            nav.handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(48),
                &catalog,
            )
            .is_none()
        );

        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
            .expect("confirmed reset should emit event");
        assert_eq!(event.action, LauncherAction::RebuildDatabase);
        assert_eq!(event.path, None);
        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.confirm_selected, 0);
    }

    #[test]
    fn display_combo_selects_a_new_mode_and_confirmation_can_cancel() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        assert_eq!(nav.display_selected, usize::MAX);
        let t0 = Instant::now();
        nav.screen = Screen::Settings;
        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert!(nav.display_combo_open);
        release(&mut nav, &catalog, t0, 16);
        let down = pad_with(|pad| pad.dpad_down = true);
        assert!(
            nav.handle_input(&down, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        release(&mut nav, &catalog, t0, 48);
        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
            .expect("new display mode");
        assert_eq!(event.action, LauncherAction::ApplyDisplayResolution);
        assert_eq!(event.path.as_deref(), Some("hdmi-1366x768p60"));

        nav.confirm_action = Some(ConfirmAction::DisplayResolution);
        nav.confirm_selected = 0;
        release(&mut nav, &catalog, t0, 80);
        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(96), &catalog)
            .expect("cancel display");
        assert_eq!(event.action, LauncherAction::CancelDisplayResolution);
    }

    #[test]
    fn orientation_combo_applies_and_confirmation_can_confirm_or_cancel() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Settings;
        nav.settings_selected = SETTINGS_ORIENTATION_SELECTED;
        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert!(nav.orientation_combo_open);
        release(&mut nav, &catalog, t0, 16);

        let down = pad_with(|pad| pad.dpad_down = true);
        assert!(
            nav.handle_input(&down, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        release(&mut nav, &catalog, t0, 48);
        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
            .expect("new orientation");
        assert_eq!(event.action, LauncherAction::ApplyScreenOrientation);
        assert_eq!(event.path.as_deref(), Some("monitor-clockwise"));

        nav.confirm_action = Some(ConfirmAction::ScreenOrientation);
        nav.confirm_selected = 1;
        release(&mut nav, &catalog, t0, 80);
        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(96), &catalog)
            .expect("confirm orientation");
        assert_eq!(event.action, LauncherAction::ConfirmScreenOrientation);

        nav.confirm_action = Some(ConfirmAction::ScreenOrientation);
        release(&mut nav, &catalog, t0, 112);
        let press_b = pad_with(|pad| pad.btn_b = true);
        let event = nav
            .handle_input(&press_b, t0 + Duration::from_millis(128), &catalog)
            .expect("cancel orientation");
        assert_eq!(event.action, LauncherAction::CancelScreenOrientation);
    }

    #[test]
    fn display_combo_navigates_to_the_last_selectable_mode() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let count = settings_display_resolution_count();
        assert_eq!(count, 7);
        nav.screen = Screen::Settings;
        nav.display_combo_open = true;
        nav.display_selected = 0;
        nav.display_highlighted = count - 2;

        let t0 = Instant::now();
        let down = pad_with(|pad| pad.dpad_down = true);
        assert!(nav.handle_input(&down, t0, &catalog).is_none());
        assert_eq!(nav.display_highlighted, count - 1);
        release(&mut nav, &catalog, t0, 16);
        assert!(
            nav.handle_input(&down, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        assert_eq!(nav.display_highlighted, count - 1);
        release(&mut nav, &catalog, t0, 48);

        let press_a = pad_with(|pad| pad.btn_a = true);
        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
            .expect("last display mode");
        assert_eq!(event.action, LauncherAction::ApplyDisplayResolution);
        assert_eq!(event.path.as_deref(), Some("crt-288p50"));
    }

    #[test]
    fn display_settings_hide_scandoubled_crt_modes_without_removing_runtime_support() {
        let ids = settings_display_resolutions()
            .map(|mode| mode.id)
            .collect::<Vec<_>>();

        assert_eq!(settings_display_resolution_index("crt-240p60"), Some(5));
        assert_eq!(settings_display_resolution_index("crt-288p50"), Some(6));
        for id in SETTINGS_HIDDEN_DISPLAY_RESOLUTION_IDS {
            let runtime_index = DISPLAY_RESOLUTIONS
                .iter()
                .position(|mode| mode.id == id)
                .expect("hidden display mode remains in the runtime catalog");
            assert!(!ids.contains(&id));
            assert_eq!(settings_display_selection_index(runtime_index), None);
            assert!(mister_magik_mister_runtime::display_resolution::find(id).is_some());
        }
    }

    #[test]
    fn display_confirmation_stays_cancellable_while_persistence_is_busy() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.confirm_action = Some(ConfirmAction::DisplayResolution);
        nav.confirm_selected = 1;
        nav.display_confirm_busy = true;

        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert_eq!(nav.confirm_action, Some(ConfirmAction::DisplayResolution));
        release(&mut nav, &catalog, t0, 16);

        let press_b = pad_with(|pad| pad.btn_b = true);
        let event = nav
            .handle_input(&press_b, t0 + Duration::from_millis(32), &catalog)
            .expect("busy confirmation remains cancellable");
        assert_eq!(event.action, LauncherAction::CancelDisplayResolution);
    }

    #[test]
    fn display_state_reply_requires_schema_and_known_pending_mode() {
        let state = parse_display_state_response(
            "ok DisplayV1 schema=1 active=custom pending=crt-240p60 remaining=22 phase=failed error=persist-failed return=settings",
        )
        .unwrap();
        assert_eq!(state.active, "custom");
        assert_eq!(state.pending.as_deref(), Some("crt-240p60"));
        assert_eq!(state.remaining, DISPLAY_CONFIRM_SECONDS);
        assert_eq!(state.phase, DisplayTransactionPhase::Failed);
        assert_eq!(state.error.as_deref(), Some("persist-failed"));
        assert!(state.return_to_settings);
        assert!(
            parse_display_state_response("ok DisplayV1 active=custom pending=none remaining=0")
                .is_err()
        );
        assert!(
            parse_display_state_response(
                "ok DisplayV1 schema=1 active=custom pending=unsafe remaining=10"
            )
            .is_err()
        );
    }

    #[test]
    fn launcher_exit_confirmation_defaults_to_cancel() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Settings;
        nav.settings_selected = SETTINGS_EXIT_SELECTED;

        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert_eq!(nav.confirm_action, Some(ConfirmAction::ExitToMister));
        assert_eq!(nav.confirm_selected, 0);
        assert!(
            nav.handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog,
            )
            .is_none()
        );

        assert!(
            nav.handle_input(&press_a, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.confirm_selected, 0);
    }

    #[test]
    fn launcher_exit_confirmation_exits_from_second_button() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Settings;
        nav.settings_selected = SETTINGS_EXIT_SELECTED;

        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert!(
            nav.handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog,
            )
            .is_none()
        );

        let right = pad_with(|pad| pad.dpad_right = true);
        assert!(
            nav.handle_input(&right, t0 + Duration::from_millis(32), &catalog)
                .is_none()
        );
        assert_eq!(nav.confirm_selected, 1);
        assert!(
            nav.handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(48),
                &catalog,
            )
            .is_none()
        );

        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
            .expect("exit button should emit event");
        assert_eq!(event.action, LauncherAction::ExitToMister);
        assert_eq!(event.path, None);
    }

    #[test]
    fn home_closes_confirmation_and_returns_to_hierarchy_root() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("menu:consoles:nintendo"));
        nav.screen = Screen::Settings;
        nav.confirm_action = Some(ConfirmAction::RebuildDatabase);

        assert!(
            nav.handle_input(
                &pad_with(|pad| pad.btn_home = true),
                Instant::now(),
                &catalog,
            )
            .is_none()
        );

        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
    }

    #[test]
    fn library_changed_confirmation_defaults_to_continue() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.confirm_action = Some(ConfirmAction::LibraryChanged);

        let event = nav
            .handle_input(&pad_with(|pad| pad.btn_a = true), t0, &catalog)
            .expect("library changed default should emit continue");
        assert_eq!(event.action, LauncherAction::ContinueWithStaleLibrary);
        assert_eq!(event.path, None);
        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.confirm_selected, 0);
    }

    #[test]
    fn library_changed_confirmation_right_button_rebuilds() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.confirm_action = Some(ConfirmAction::LibraryChanged);
        nav.confirm_selected = 1;

        let event = nav
            .handle_input(&pad_with(|pad| pad.btn_a = true), t0, &catalog)
            .expect("library changed rebuild should emit event");
        assert_eq!(event.action, LauncherAction::RebuildLibrary);
        assert_eq!(event.path, None);
        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.confirm_selected, 0);
    }

    #[test]
    fn library_changed_back_defers_rebuild() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.confirm_action = Some(ConfirmAction::LibraryChanged);
        nav.confirm_selected = 1;

        let event = nav
            .handle_input(&pad_with(|pad| pad.btn_b = true), t0, &catalog)
            .expect("back should continue with stale library");
        assert_eq!(event.action, LauncherAction::ContinueWithStaleLibrary);
        assert_eq!(event.path, None);
        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.confirm_selected, 0);
    }

    #[test]
    fn library_changed_test_dialog_choice_parser_accepts_only_continue_or_rebuild() {
        assert_eq!(
            parse_library_changed_test_dialog_choice("continue").expect("parse continue"),
            Some(LibraryChangedTestDialogChoice::Continue)
        );
        assert_eq!(
            parse_library_changed_test_dialog_choice("rebuild").expect("parse rebuild"),
            Some(LibraryChangedTestDialogChoice::Rebuild)
        );
        assert_eq!(
            parse_library_changed_test_dialog_choice("").expect("parse empty"),
            None
        );
        assert!(parse_library_changed_test_dialog_choice("reset").is_err());
    }

    #[test]
    fn library_rebuild_marker_is_one_shot() {
        let root = unique_temp_dir("library-rebuild-marker");
        let path = root.join("nested/rebuild-on-next-boot");

        assert!(!consume_library_rebuild_on_next_boot_at(&path).expect("missing marker"));
        let mut fault_control = RecordingFaultControl::default();
        request_library_rebuild_on_next_boot_at_with_fault_control(&path, &mut fault_control)
            .expect("write marker");
        assert!(path.exists());
        assert_eq!(
            fault_control.points,
            vec!["launcher.rebuild_marker.after_write"]
        );
        assert!(consume_library_rebuild_on_next_boot_at(&path).expect("consume marker"));
        assert!(!path.exists());
        assert!(!consume_library_rebuild_on_next_boot_at(&path).expect("consume absent marker"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reset_screenshot_pack_matcher_is_limited_to_supported_pack_files() {
        assert!(screenshot_reset_deletes_file(
            "arcade-screenshots-320x320.mmlz4b"
        ));
        assert!(screenshot_reset_deletes_file(
            "neogeo-screenshots-240x240.mmlz4b.tmp-123"
        ));
        assert!(screenshot_reset_deletes_file("nes-screenshots.mmlz4b"));
        assert!(screenshot_reset_deletes_file(STATE_FILENAME));
        assert!(screenshot_reset_deletes_file(
            ".screenshot-media-state.json.tmp-123"
        ));
        assert!(screenshot_reset_deletes_file(
            "arcade-screenshots-320x320.mmlz4b.idx"
        ));
        assert!(screenshot_reset_deletes_file(
            ".arcade-screenshots-320x320.mmlz4b.tmp-123"
        ));

        assert!(!screenshot_reset_deletes_file(
            "pcengine-screenshots.mmlz4b"
        ));
        assert!(!screenshot_reset_deletes_file(
            "arcade-screenshots-large.mmlz4b"
        ));
        assert!(!screenshot_reset_deletes_file(
            "arcade-preview-cache.raw565"
        ));
        assert!(!screenshot_reset_deletes_file("manual.pdf"));
    }

    #[test]
    fn reset_screenshot_pack_cleanup_removes_packs_and_state_only() {
        let root = unique_temp_dir("screenshot-pack-reset");
        for name in [
            "arcade-screenshots-320x320.mmlz4b",
            "neogeo-screenshots.mmlz4b",
            "saturn-screenshots-240x240.mmlz4b.tmp",
            "arcade-screenshots-320x320.mmlz4b.idx",
            ".neogeo-screenshots.mmlz4b.tmp-123",
            STATE_FILENAME,
        ] {
            std::fs::write(root.join(name), b"pack").expect("write removable asset");
        }
        for name in [
            "pcengine-screenshots.mmlz4b",
            "arcade-screenshots-large.mmlz4b",
            "manual.pdf",
        ] {
            std::fs::write(root.join(name), b"keep").expect("write retained asset");
        }
        std::fs::create_dir(root.join("arcade-screenshots-320x320.mmlz4b.dir"))
            .expect("write retained directory");

        let mut fault_control = RecordingFaultControl::default();
        let removed = delete_screenshot_packs_at_with_fault_control(&root, &mut fault_control)
            .expect("delete screenshot packs");

        assert_eq!(removed, 6);
        assert!(!root.join("arcade-screenshots-320x320.mmlz4b").exists());
        assert!(!root.join("neogeo-screenshots.mmlz4b").exists());
        assert!(!root.join("saturn-screenshots-240x240.mmlz4b.tmp").exists());
        assert!(!root.join("arcade-screenshots-320x320.mmlz4b.idx").exists());
        assert!(!root.join(".neogeo-screenshots.mmlz4b.tmp-123").exists());
        assert!(!root.join(STATE_FILENAME).exists());
        assert!(root.join("pcengine-screenshots.mmlz4b").exists());
        assert!(root.join("arcade-screenshots-large.mmlz4b").exists());
        assert!(root.join("manual.pdf").exists());
        assert!(root.join("arcade-screenshots-320x320.mmlz4b.dir").exists());
        assert_eq!(
            fault_control.points,
            vec!["reset_delete.screenshot_asset.after_remove"; 6]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn library_purge_reports_catalog_and_screenshot_counts_and_is_idempotent() {
        let root = unique_temp_dir("library-purge");
        std::fs::write(root.join("arcade-screenshots-320x320.mmlz4b"), b"pack")
            .expect("write screenshot pack");
        std::fs::write(root.join("manual.pdf"), b"keep").expect("write unrelated asset");

        let outcome =
            purge_library_data_with(&root, || Ok(7)).expect("purge catalog and screenshots");
        assert_eq!(
            outcome,
            PurgeLibraryDataOutcome {
                catalog_artifacts_removed: 7,
                screenshot_artifacts_removed: 1,
            }
        );
        assert!(root.join("manual.pdf").exists());

        let repeated = purge_library_data_with(&root, || Ok(0)).expect("repeat purge");
        assert_eq!(repeated, PurgeLibraryDataOutcome::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn library_update_failed_confirmation_dismisses_without_action() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.confirm_action = Some(ConfirmAction::LibraryUpdateFailed);

        assert!(
            nav.handle_input(&pad_with(|pad| pad.btn_a = true), t0, &catalog)
                .is_none()
        );
        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.confirm_selected, 0);
    }

    #[test]
    fn launcher_launches_image_less_system_games() {
        let catalog = image_less_amiga_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();

        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.open_system(&catalog, "amiga"));
        assert_eq!(nav.screen, Screen::Arcade);

        assert!(
            nav.handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog
            )
            .is_none()
        );
        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(32), &catalog)
            .expect("image-less game should launch");

        assert_eq!(event.action, LauncherAction::LaunchGame);
        assert_eq!(event.path.as_deref(), Some("magik-plan:amiga-agony"));
    }

    #[test]
    fn arcade_single_quick_down_press_moves_one() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        assert_eq!(nav.selected, 1);
        assert!(nav.visual_index > 0.0);
        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(40));
        settle(&mut nav, 10, t0);
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.visual_index, 1.0);
        assert_eq!(nav.scroll_y, ARCADE_ROW_HEIGHT);
    }

    #[test]
    fn arcade_single_quick_up_press_moves_one() {
        let mut nav = ArcadeNav::new();
        nav.selected = 5;
        nav.snap_to_selected();
        let t0 = Instant::now();
        input(&mut nav, -1, 0, 10, t0);
        assert_eq!(nav.selected, 4);
        assert!(nav.visual_index < 5.0);
        input(&mut nav, 0, -1, 10, t0 + Duration::from_millis(40));
        settle(&mut nav, 10, t0);
        assert_eq!(nav.selected, 4);
        assert_eq!(nav.visual_index, 4.0);
        assert_eq!(nav.scroll_y, 4 * ARCADE_ROW_HEIGHT);
    }

    #[test]
    fn arcade_release_after_tiny_downward_motion_commits_forward() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        assert_eq!(nav.selected, 1);
        assert!(nav.visual_index > 0.0 && nav.visual_index < 0.5);
        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(11));
        assert_eq!(nav.selected, 1);
        settle(&mut nav, 10, t0);
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.visual_index, 1.0);
    }

    #[test]
    fn arcade_release_after_tiny_upward_motion_commits_backward() {
        let mut nav = ArcadeNav::new();
        nav.selected = 5;
        nav.snap_to_selected();
        let t0 = Instant::now();
        input(&mut nav, -1, 0, 10, t0);
        assert_eq!(nav.selected, 4);
        assert!(nav.visual_index > 4.5 && nav.visual_index < 5.0);
        input(&mut nav, 0, -1, 10, t0 + Duration::from_millis(11));
        assert_eq!(nav.selected, 4);
        settle(&mut nav, 10, t0);
        assert_eq!(nav.selected, 4);
        assert_eq!(nav.visual_index, 4.0);
    }

    #[test]
    fn arcade_long_hold_feeds_next_row_without_delay() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        assert_eq!(nav.selected, 1);
        for frame in 1..=7 {
            input(&mut nav, 1, 1, 10, t0 + Duration::from_millis(frame * 16));
        }
        assert_eq!(nav.selected, 1);
        assert!(nav.visual_index > 0.0 && nav.visual_index < 1.0);
        for frame in 8..=30 {
            input(&mut nav, 1, 1, 10, t0 + Duration::from_millis(frame * 16));
        }
        assert!(nav.selected >= 2);
        assert!(nav.visual_index > 1.0);
    }

    #[test]
    fn arcade_scroll_stays_active_during_continuous_hold() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        for frame in 1..=30 {
            input(&mut nav, 1, 1, 10, t0 + Duration::from_millis(frame * 16));
        }
        assert!(nav.is_scroll_active());
        assert!(nav.selected > 1);

        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(500));
        settle(&mut nav, 10, t0 + Duration::from_millis(500));
        assert!(!nav.is_scroll_active());
    }

    #[test]
    fn arcade_scroll_motion_continues_while_direction_is_held() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);

        assert!(nav.has_scroll_motion_or_queue());

        for frame in 1..=30 {
            input(&mut nav, 1, 1, 10, t0 + Duration::from_millis(frame * 16));
        }
        assert!(nav.is_scroll_active());
        assert!(nav.scroll_animation.velocity() > 0.0);
    }

    #[test]
    fn arcade_rapid_taps_queue_every_press() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        for tap in 0..5 {
            let down_at = t0 + Duration::from_millis(tap * 30);
            input(&mut nav, 1, 0, 10, down_at);
            assert!(!nav.scroll.turbo_active);
            input(&mut nav, 0, 1, 10, down_at + Duration::from_millis(10));
            assert!(!nav.scroll.turbo_active);
        }
        assert_eq!(nav.selected, 5);
        assert!(nav.visual_index < 5.0);
        settle(&mut nav, 10, t0);
        assert_eq!(nav.selected, 5);
        assert_eq!(nav.visual_index, 5.0);
    }

    #[test]
    fn arcade_rapid_taps_clamp_at_edges() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        for tap in 0..12 {
            let down_at = t0 + Duration::from_millis(tap * 20);
            input(&mut nav, 1, 0, 10, down_at);
            input(&mut nav, 0, 1, 10, down_at + Duration::from_millis(5));
        }
        assert_eq!(nav.selected, 9);
        settle(&mut nav, 10, t0);
        assert_eq!(nav.visual_index, 9.0);

        for tap in 0..12 {
            let down_at = t0 + Duration::from_millis(400 + tap * 20);
            input(&mut nav, -1, 0, 10, down_at);
            input(&mut nav, 0, -1, 10, down_at + Duration::from_millis(5));
        }
        assert_eq!(nav.selected, 0);
        settle(&mut nav, 10, t0);
        assert_eq!(nav.visual_index, 0.0);
    }

    #[test]
    fn arcade_opposite_direction_retargets_safely() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(20));
        assert_eq!(nav.selected, 1);
        assert!(nav.visual_index > 0.0);
        input(&mut nav, -1, 0, 10, t0 + Duration::from_millis(40));
        assert_eq!(nav.selected, 0);
        input(&mut nav, 0, -1, 10, t0 + Duration::from_millis(60));
        settle(&mut nav, 10, t0 + Duration::from_millis(60));
        assert_eq!(nav.visual_index, 0.0);
    }

    #[test]
    fn arcade_turbo_repress_springs_toward_cruise_velocity() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(50));

        input(&mut nav, 1, 0, 10, t0 + Duration::from_millis(120));
        assert!(!nav.scroll.turbo_active);
        assert!(!nav.is_turbo_active());
        assert_eq!(nav.selected, 2);

        let visual_before_turbo = nav.scroll_animation.value();
        input(&mut nav, 1, 1, 10, t0 + Duration::from_millis(360));
        assert!(nav.scroll.turbo_active);
        assert!(nav.is_turbo_active());
        assert!(nav.scroll_animation.value() > visual_before_turbo);
        assert!(nav.scroll_animation.velocity() > 0.0);
        assert!(nav.scroll_animation.velocity() < ARCADE_TURBO_PX_PER_SECOND);
    }

    #[test]
    fn arcade_turbo_release_springs_forward_to_an_exact_row() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 100, t0);
        input(&mut nav, 0, 1, 100, t0 + Duration::from_millis(50));
        input(&mut nav, 1, 0, 100, t0 + Duration::from_millis(120));
        for frame in 23..=70 {
            input(&mut nav, 1, 1, 100, t0 + Duration::from_millis(frame * 16));
        }
        assert!(nav.is_turbo_active());
        let release_value = nav.scroll_animation.value();
        let release_velocity = nav.scroll_animation.velocity();
        let nearest_forward_row = (release_value / nav.row_height as f64).ceil() as usize;

        let release_at = t0 + Duration::from_millis(1136);
        input(&mut nav, 0, 1, 100, release_at);
        assert!(!nav.is_turbo_active());
        assert!(nav.scroll.target_index > nearest_forward_row);
        assert_eq!(
            nav.scroll_animation.target(),
            nav.scroll.target_index as f64 * nav.row_height as f64
        );

        let mut previous = nav.scroll_animation.value();
        for frame in 1..=180 {
            nav.tick(100, release_at + Duration::from_millis(frame * 16));
            assert!(nav.scroll_animation.value() >= previous);
            previous = nav.scroll_animation.value();
            if nav.is_settled() {
                break;
            }
        }
        assert!(release_velocity > 0.0);
        assert!(nav.is_settled());
        assert_eq!(nav.visual_index, nav.selected as f32);
    }

    #[test]
    fn arcade_normal_scroll_is_active_but_not_turbo() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();

        input(&mut nav, 1, 0, 10, t0);

        assert!(nav.is_scroll_active());
        assert!(!nav.is_turbo_active());
    }

    #[test]
    fn arcade_bench_turbo_bounce_reverses_at_edges() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        let mut saw_bottom = false;
        let mut saw_upward_after_bottom = false;
        let mut saw_top_after_bottom = false;
        let mut saw_downward_after_top = false;

        for frame in 0..160 {
            nav.bench_turbo_bounce_tick(5, t0 + Duration::from_millis(frame * 16));
            assert!(nav.is_turbo_active());
            if nav.selected == 4 {
                saw_bottom = true;
            }
            if saw_bottom && nav.scroll.held_dir < 0 {
                saw_upward_after_bottom = true;
            }
            if saw_upward_after_bottom && nav.selected == 0 {
                saw_top_after_bottom = true;
            }
            if saw_top_after_bottom && nav.scroll.held_dir > 0 {
                saw_downward_after_top = true;
                break;
            }
        }

        assert!(saw_bottom);
        assert!(saw_upward_after_bottom);
        assert!(saw_top_after_bottom);
        assert!(saw_downward_after_top);
    }

    #[test]
    fn arcade_long_second_press_consumes_turbo_candidate() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(50));
        input(&mut nav, 1, 0, 10, t0 + Duration::from_millis(120));
        input(&mut nav, 1, 1, 10, t0 + Duration::from_millis(360));
        assert!(nav.scroll.turbo_active);

        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(370));
        assert_eq!(nav.scroll.last_quick_tap_dir, 0);
        assert_eq!(nav.scroll.last_quick_tap_released_at, None);

        input(&mut nav, 1, 0, 10, t0 + Duration::from_millis(380));
        assert!(!nav.scroll.turbo_candidate);
        assert!(!nav.scroll.turbo_active);
    }

    #[test]
    fn arcade_late_repress_does_not_turbo() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(50));
        let visual_before_repress = nav.scroll_animation.value();
        input(&mut nav, 1, 0, 10, t0 + Duration::from_millis(450));
        assert!(!nav.scroll.turbo_active);
        assert!(nav.scroll_animation.value() > visual_before_repress);
    }

    #[test]
    fn arcade_opposite_repress_does_not_turbo() {
        let mut nav = ArcadeNav::new();
        nav.selected = 5;
        nav.snap_to_selected();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(50));
        let visual_before_repress = nav.scroll_animation.value();
        input(&mut nav, -1, 0, 10, t0 + Duration::from_millis(120));
        assert!(!nav.scroll.turbo_active);
        assert_eq!(nav.selected, 5);
        assert_ne!(nav.scroll_animation.value(), visual_before_repress);
        input(&mut nav, 0, -1, 10, t0 + Duration::from_millis(140));
        settle(&mut nav, 10, t0 + Duration::from_millis(140));
        assert_eq!(nav.visual_index, 5.0);
    }

    #[test]
    fn launch_missing_target_does_not_spawn_or_require_recovery() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.target_exists = false;

        let err = execute_game_launch_with(&path_target("/missing.mra"), &mut io)
            .expect_err("launch fails");

        assert!(!err.spawned_mister());
        assert_eq!(io.start_calls, 0);
        assert!(!launch_in_progress());
    }

    #[test]
    fn launch_fifo_timeout_after_spawn_reports_recovery_needed() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.mister_running = false;
        io.started_ready = false;

        let err = execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect_err("launch fails");

        assert!(err.spawned_mister());
        assert_eq!(io.start_calls, 1);
        assert!(err.to_string().contains("timed out waiting"));
        assert!(!launch_in_progress());
    }

    #[test]
    fn launch_write_failure_after_spawn_reports_recovery_needed() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.mister_running = false;
        io.write_result = Err("fifo write failed".to_string());

        let err = execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect_err("launch fails");

        assert!(err.spawned_mister());
        assert_eq!(io.start_calls, 1);
        assert_eq!(
            io.commands,
            [MainCommand::LaunchPath {
                target: "/media/fat/_Arcade/test.mra".to_string(),
            }]
        );
        assert_eq!(err.to_string(), "fifo write failed");
        assert!(!launch_in_progress());
    }

    #[test]
    fn magik_launch_effect_order_is_characterized() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.simple_joystick_handling = true;

        execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect("launch succeeds");

        assert_eq!(
            io.effects,
            [
                "prepare-input-profiles",
                "button-overrides:write:/media/fat/_Arcade/test.mra",
                "input-policy:true",
                "main-command:launch:/media/fat/_Arcade/test.mra",
            ]
        );
        reset_launch();
    }

    #[test]
    fn magik_launch_rejection_restores_stock_input_policy_after_command() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.write_result = Err("rejected LauncherCrashed".to_string());

        let error = execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect_err("Main rejection fails launch");

        assert_eq!(error.kind(), LaunchFailureKind::HandoffRejected);
        assert_eq!(error.to_string(), "rejected LauncherCrashed");
        assert_eq!(
            io.effects,
            [
                "button-overrides:remove",
                "input-policy:false",
                "main-command:launch:/media/fat/_Arcade/test.mra",
                "input-policy:false",
            ]
        );
        assert!(!launch_in_progress());
    }

    #[test]
    fn launch_fifo_unavailable_with_existing_main_does_not_require_recovery() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.fifo_ready = false;

        let error = execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect_err("missing FIFO fails launch");

        assert!(!error.spawned_mister());
        assert_eq!(error.kind(), LaunchFailureKind::HandoffRejected);
        assert_eq!(error.to_string(), "timed out waiting for /dev/MiSTer_cmd");
        assert!(io.effects.is_empty());
        assert!(!launch_in_progress());
    }

    #[test]
    fn launch_start_failure_preserves_text_without_recovery_flag() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.mister_running = false;
        io.start_result = Err("failed to spawn MiSTer_MagiKDev: permission denied".to_string());

        let error = execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect_err("Main start failure fails launch");

        assert!(!error.spawned_mister());
        assert_eq!(
            error.to_string(),
            "failed to spawn MiSTer_MagiKDev: permission denied"
        );
        assert_eq!(io.effects, ["start-main"]);
        assert!(!launch_in_progress());
    }

    #[test]
    fn launch_running_stock_main_uses_load_core_without_recovery_flag() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.magik_running = false;

        let spawned =
            execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
                .expect("launch succeeds");

        assert!(!spawned);
        assert_eq!(io.start_calls, 0);
        assert_eq!(
            io.commands,
            [MainCommand::LoadCore {
                target: "/media/fat/_Arcade/test.mra".to_string(),
            }]
        );
        assert!(launch_in_progress());
        reset_launch();
    }

    #[test]
    fn reboot_mister_requests_supervised_main_reboot() {
        let mut io = launch_io();

        reboot_mister_with(&mut io).unwrap();

        assert_eq!(io.commands, [MainCommand::Reboot]);
    }

    #[test]
    fn reboot_mister_refuses_raw_reboot_without_magik_main() {
        let mut io = launch_io();
        io.magik_running = false;

        let err = reboot_mister_with(&mut io).unwrap_err();

        assert!(err.contains("refusing raw reboot"));
        assert!(io.commands.is_empty());
    }

    #[test]
    fn reboot_mister_requires_command_fifo() {
        let mut io = launch_io();
        io.fifo_ready = false;

        let err = reboot_mister_with(&mut io).unwrap_err();

        assert!(err.contains("/dev/MiSTer_cmd"));
        assert!(io.commands.is_empty());
    }

    #[test]
    fn structured_launch_plan_writes_plan_command_without_materialization() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();

        let spawned =
            execute_game_launch_with(&structured_target(), &mut io).expect("launch succeeds");

        assert!(!spawned);
        assert_eq!(io.start_calls, 0);
        assert_eq!(
            io.commands,
            [MainCommand::StructuredLaunch {
                fields: "schema=1&launch_ref=magik-plan:test%20game&title=Test%20Game&system_id=neogeo&core_path=NeoGeo&payload_path=/media/fat/games/NEOGEO/Test%20Game.neo&mount_kind=mount-image&mount_index=0&delay_secs=1".to_string(),
            }]
        );
        reset_launch();
    }

    #[test]
    fn structured_launch_plan_normalizes_legacy_dated_core_path() {
        let mut target = structured_target();
        let LaunchTarget::Structured(plan) = &mut target else {
            unreachable!("structured target")
        };
        plan.system_id = "snes".into();
        plan.core_path = "_Console/SNES_20240408".into();
        plan.payload_path = "/media/fat/games/SNES/ActRaiser.sfc".into();
        plan.mount_kind = "load-file".into();
        plan.mount_index = 1;

        let LaunchTarget::Structured(plan) = target else {
            unreachable!("structured target")
        };
        let encoded = encode_launch_plan(&plan);

        assert!(encoded.contains("core_path=_Console/SNES&"));
        assert!(!encoded.contains("SNES_20240408"));
    }

    #[test]
    fn logical_core_path_preserves_non_version_suffixes() {
        assert_eq!(logical_core_path("_Console/SNES_20260603"), "_Console/SNES");
        assert_eq!(
            logical_core_path("_LLAPI/SNES_LLAPI_20251204"),
            "_LLAPI/SNES_LLAPI"
        );
        assert_eq!(
            logical_core_path("_Console/SNES_accuracy"),
            "_Console/SNES_accuracy"
        );
        assert_eq!(logical_core_path("NeoGeo"), "NeoGeo");
    }

    #[test]
    fn magik_launch_surfaces_direct_rejection() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.write_result = Err("rejected LauncherCrashed".to_string());

        let err = execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect_err("Main rejection should fail launch");

        assert_eq!(
            io.commands,
            [MainCommand::LaunchPath {
                target: "/media/fat/_Arcade/test.mra".to_string(),
            }]
        );
        assert!(err.to_string().contains("rejected LauncherCrashed"));
        assert_eq!(io.input_policy_markers, vec![false, false]);
        assert!(!launch_in_progress());
    }

    #[test]
    fn stock_main_launch_does_not_use_magik_reply() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.magik_running = false;

        execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect("stock Main launch does not use MagiK status ack");

        assert_eq!(
            io.commands,
            [MainCommand::LoadCore {
                target: "/media/fat/_Arcade/test.mra".to_string(),
            }]
        );
        assert!(launch_in_progress());
        reset_launch();
    }

    #[test]
    fn magik_launch_writes_stock_input_policy_marker_by_default() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();

        execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect("launch succeeds");

        assert_eq!(io.input_policy_markers, vec![false]);
        assert_eq!(io.button_override_writes, vec!["remove"]);
        assert_eq!(io.prepare_simple_input_profile_calls, 0);
        reset_launch();
    }

    #[test]
    fn magik_launch_writes_simple_input_policy_marker_when_enabled() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.simple_joystick_handling = true;

        execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect("launch succeeds");

        assert_eq!(io.prepare_simple_input_profile_calls, 1);
        assert_eq!(io.input_policy_markers, vec![true]);
        assert_eq!(
            io.button_override_writes,
            vec!["write:/media/fat/_Arcade/test.mra"]
        );
        reset_launch();
    }

    #[test]
    fn simple_non_mra_launch_removes_button_overrides() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.simple_joystick_handling = true;

        execute_game_launch_with(&structured_target(), &mut io).expect("launch succeeds");

        assert_eq!(io.prepare_simple_input_profile_calls, 1);
        assert_eq!(io.input_policy_markers, vec![true]);
        assert_eq!(io.button_override_writes, vec!["remove"]);
        reset_launch();
    }

    #[test]
    fn stock_main_launch_does_not_write_input_policy_marker() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.magik_running = false;
        io.simple_joystick_handling = true;

        execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect("launch succeeds");

        assert!(io.input_policy_markers.is_empty());
        assert!(io.button_override_writes.is_empty());
        assert_eq!(io.prepare_simple_input_profile_calls, 0);
        reset_launch();
    }

    #[test]
    fn structured_launch_plan_requires_magik_main() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.magik_running = false;

        let err =
            execute_game_launch_with(&structured_target(), &mut io).expect_err("launch fails");

        assert!(err.to_string().contains("requires MiSTer_MagiK"));
        assert!(io.commands.is_empty());
        assert!(!launch_in_progress());
    }

    #[test]
    fn missing_structured_launch_plan_does_not_fall_back_to_path_launch() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();

        let err = execute_game_launch_with(
            &LaunchTarget::MissingStructured("magik-plan:missing".into()),
            &mut io,
        )
        .expect_err("launch fails");

        assert!(err.to_string().contains("missing from catalog"));
        assert_eq!(io.start_calls, 0);
        assert!(io.commands.is_empty());
        assert!(!launch_in_progress());
    }

    #[test]
    fn partial_catalog_lock_keeps_navigation_in_the_restored_collection() {
        let catalog = multi_game_catalog();
        let mut nav = LauncherNav::new();
        assert!(nav.open_system(&catalog, "arcade"));
        nav.set_arcade_exit_locked(true);

        nav.leave_arcade(false, "arcade");
        assert_eq!(nav.screen, Screen::Arcade);

        nav.set_arcade_exit_locked(false);
        nav.leave_arcade(false, "arcade");
        assert_eq!(nav.screen, Screen::Home);
    }

    #[test]
    fn progressive_catalog_shell_is_busy_then_disappears_without_failure() {
        let catalog = arcade_catalog(Vec::new(), Vec::new()).with_system_placeholder("snes");
        let mut nav = LauncherNav::new();
        nav.catalog_system_discovered("snes");
        nav.sync_launcher_taxonomy(&catalog);

        let consoles = nav
            .current_menu_items()
            .iter()
            .find(|item| item.id == crate::launcher_taxonomy::CONSOLES_MENU_ID)
            .expect("discovered console publishes its parent")
            .clone();
        assert_eq!(
            nav.menu_item_catalog_presentation(&consoles),
            catalog_presentation(CatalogMenuItemStatus::Scanning, true, false)
        );
        assert_eq!(nav.menu_discovered_system_count(&consoles.id), 1);
        assert!(nav.open_menu(crate::launcher_taxonomy::CONSOLES_MENU_ID));
        let snes = nav
            .current_menu_items()
            .iter()
            .find(|item| item.id == "snes")
            .expect("discovered leaf is visible")
            .clone();
        assert_eq!(
            nav.menu_item_catalog_presentation(&snes),
            catalog_presentation(CatalogMenuItemStatus::Scanning, false, false)
        );

        nav.catalog_system_update_ready("snes");
        assert_eq!(
            nav.menu_item_catalog_presentation(&snes),
            catalog_presentation(CatalogMenuItemStatus::Ready, false, false)
        );

        nav.catalog_system_hydration_started("snes");
        nav.catalog_system_update_failed("snes");
        let catalog = catalog.without_empty_system_placeholders();
        nav.catalog_build_finished(&catalog);
        nav.sync_launcher_taxonomy(&catalog);
        nav.go_root();
        assert!(
            nav.current_menu_items()
                .iter()
                .all(|item| item.id != crate::launcher_taxonomy::CONSOLES_MENU_ID)
        );
        assert!(!nav.catalog_system_update_has_failed("snes"));
        assert!(!nav.catalog_update_states.contains_key("snes"));
        assert!(!nav.catalog_hydration_states.contains_key("snes"));
        assert!(
            nav.catalog_with_build_shells(catalog)
                .systems
                .iter()
                .all(|system| system.id != "snes"),
            "discarded build state must not reintroduce the placeholder"
        );
    }

    #[test]
    fn manifest_backed_system_failure_survives_build_reconciliation() {
        let catalog = arcade_catalog(Vec::new(), vec![arcade_system("snes", 1)]);
        let mut nav = LauncherNav::new();
        nav.catalog_system_discovered("snes");
        nav.catalog_system_update_failed("snes");
        nav.catalog_build_finished(&catalog);
        nav.sync_launcher_taxonomy(&catalog);
        nav.go_root();

        let consoles = nav
            .current_menu_items()
            .iter()
            .find(|item| item.id == crate::launcher_taxonomy::CONSOLES_MENU_ID)
            .expect("failed descendant keeps parent visible")
            .clone();
        assert_eq!(
            nav.menu_item_catalog_presentation(&consoles),
            catalog_presentation(CatalogMenuItemStatus::Partial, true, false)
        );
        assert!(nav.catalog_system_update_has_failed("snes"));
        assert!(nav.open_menu(crate::launcher_taxonomy::CONSOLES_MENU_ID));
        let snes = nav
            .current_menu_items()
            .iter()
            .find(|item| item.id == "snes")
            .expect("published failed system remains visible");
        assert_eq!(
            nav.menu_item_catalog_presentation(snes),
            catalog_presentation(CatalogMenuItemStatus::UpdateFailed, true, false),
            "an update failure must not revoke published availability"
        );
        nav.go_root();

        nav.catalog_build_started();
        assert_eq!(
            nav.menu_item_catalog_presentation(&consoles),
            catalog_presentation(CatalogMenuItemStatus::Ready, true, false),
            "a new build must not inherit failures or mark systems before its plan"
        );
    }

    #[test]
    fn lazy_hydration_is_visually_silent_and_does_not_clear_update_state() {
        let catalog = arcade_catalog(Vec::new(), vec![arcade_system("snes", 1)]);
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu(crate::launcher_taxonomy::CONSOLES_MENU_ID));
        let snes = nav
            .current_menu_items()
            .iter()
            .find(|item| item.id == "snes")
            .expect("published SNES system")
            .clone();

        assert_eq!(
            nav.menu_item_catalog_presentation(&snes),
            catalog_presentation(CatalogMenuItemStatus::Ready, true, false)
        );

        nav.catalog_system_hydration_started("snes");
        assert_eq!(
            nav.menu_item_catalog_presentation(&snes),
            catalog_presentation(CatalogMenuItemStatus::Ready, true, false),
            "loading published rows must not change tile presentation"
        );

        nav.catalog_reconciliation_plan(&catalog, &["snes".to_string()], false);
        assert_eq!(
            nav.menu_item_catalog_presentation(&snes),
            catalog_presentation(CatalogMenuItemStatus::Scanning, true, false)
        );
        nav.catalog_system_hydration_finished("snes");
        assert_eq!(
            nav.menu_item_catalog_presentation(&snes),
            catalog_presentation(CatalogMenuItemStatus::Scanning, true, false),
            "a shard-ready event must not clear concurrent update activity"
        );

        nav.catalog_system_update_ready("snes");
        nav.catalog_system_hydration_failed("snes");
        assert_eq!(
            nav.menu_item_catalog_presentation(&snes),
            catalog_presentation(CatalogMenuItemStatus::LoadFailed, false, true)
        );

        nav.catalog_system_hydration_started("snes");
        assert_eq!(
            nav.menu_item_catalog_presentation(&snes),
            catalog_presentation(CatalogMenuItemStatus::Ready, true, false),
            "an accepted retry is visually silent while it loads"
        );
    }

    #[test]
    fn progressive_warm_rebuild_keeps_published_systems_live_until_atomic_refresh() {
        let published = multi_system_catalog();
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&published);
        nav.go_root();
        let arcade = nav
            .current_menu_items()
            .iter()
            .find(|item| item.id == crate::arcade_catalog::MENU_ARCADE_SYSTEM_ID)
            .expect("published Arcade tile")
            .clone();
        let computers = nav
            .current_menu_items()
            .iter()
            .find(|item| item.id == crate::launcher_taxonomy::COMPUTERS_MENU_ID)
            .expect("published computer group")
            .clone();

        nav.catalog_reconciliation_plan(&published, &["amiga".to_string()], false);
        assert_eq!(
            nav.menu_item_catalog_presentation(&arcade),
            catalog_presentation(CatalogMenuItemStatus::Ready, true, false),
            "unaffected published systems remain unchanged"
        );
        assert_eq!(
            nav.menu_item_catalog_presentation(&computers),
            catalog_presentation(CatalogMenuItemStatus::Scanning, true, false)
        );
        assert!(nav.open_menu(crate::launcher_taxonomy::COMPUTERS_MENU_ID));
        let amiga = nav
            .current_menu_items()
            .iter()
            .find(|item| item.id == "amiga")
            .expect("published Amiga tile")
            .clone();

        nav.catalog_system_scanning("amiga");
        assert_eq!(
            nav.menu_item_catalog_presentation(&amiga),
            catalog_presentation(CatalogMenuItemStatus::Scanning, true, false),
            "published games remain available while scanning"
        );
        nav.catalog_system_prepared("amiga");
        assert_eq!(
            nav.menu_item_catalog_presentation(&amiga),
            catalog_presentation(CatalogMenuItemStatus::Scanning, true, false),
            "prepared candidates stay non-authoritative"
        );
        nav.catalog_system_update_failed("amiga");
        assert_eq!(
            nav.menu_item_catalog_presentation(&amiga),
            catalog_presentation(CatalogMenuItemStatus::UpdateFailed, true, false),
            "failed updates retain the published generation"
        );

        nav.go_root();
        nav.catalog_reconciliation_plan(&published, &[], true);
        assert_eq!(
            nav.menu_item_catalog_presentation(&arcade),
            catalog_presentation(CatalogMenuItemStatus::Scanning, true, false)
        );
        assert_eq!(
            nav.menu_item_catalog_presentation(&computers),
            catalog_presentation(CatalogMenuItemStatus::Scanning, true, false),
            "a Settings rebuild marks every published branch without disabling it"
        );

        assert!(nav.open_system(&published, "amiga"));
        nav.catalog_system_prepared("amiga");
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.active_collection_id.as_deref(), Some("amiga"));

        let published_without_amiga = arcade_catalog(
            vec![
                arcade_game("1942")
                    .path("/media/fat/_Arcade/1942.mra")
                    .preview("1942")
                    .build(),
            ],
            vec![arcade_system("arcade", 1)],
        );
        nav.catalog_build_finished(&published_without_amiga);
        nav.sync_launcher_taxonomy(&published_without_amiga);
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.active_collection_id, None);
    }

    #[test]
    fn x_options_emits_context_sensitive_favourite_actions() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("F-Zero")
                    .system_id("snes")
                    .path("/media/fat/games/SNES/F-Zero.sfc")
                    .build(),
            ],
            vec![arcade_system("snes", 1)],
        );
        let mut nav = LauncherNav::new();
        assert!(nav.open_system(&catalog, "snes"));
        nav.screen = Screen::Arcade;
        let now = Instant::now();

        assert!(
            nav.handle_input(&pad_with(|pad| pad.btn_x = true), now, &catalog)
                .is_none()
        );
        assert_eq!(nav.confirm_action, Some(ConfirmAction::AddFavourite));
        nav.handle_input(
            &pad_with(|pad| pad.dpad_right = true),
            now + Duration::from_millis(1),
            &catalog,
        );
        let add = nav
            .handle_input(
                &pad_with(|pad| pad.btn_a = true),
                now + Duration::from_millis(2),
                &catalog,
            )
            .unwrap();
        assert_eq!(add.action, LauncherAction::AddFavourite);
        assert_eq!(
            add.path.as_deref(),
            Some("/media/fat/games/SNES/F-Zero.sfc")
        );

        nav.apply_favourite_state(add.path.as_deref().unwrap(), true);
        nav.handle_input(
            &PadState::default(),
            now + Duration::from_millis(3),
            &catalog,
        );
        nav.handle_input(
            &pad_with(|pad| pad.btn_x = true),
            now + Duration::from_millis(4),
            &catalog,
        );
        assert_eq!(nav.confirm_action, Some(ConfirmAction::RemoveFavourite));
    }

    #[test]
    fn user_lists_preserve_recent_order_and_filter_favourites() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("F-Zero")
                    .system_id("snes")
                    .path("/media/fat/games/SNES/F-Zero.sfc")
                    .build(),
                arcade_game("Mario World")
                    .system_id("snes")
                    .path("/media/fat/games/SNES/Mario World.sfc")
                    .build(),
            ],
            vec![arcade_system("snes", 2)],
        );
        let mut nav = LauncherNav::new();
        assert!(nav.open_system(&catalog, "snes"));
        nav.set_user_game_refs(
            &catalog,
            ["/media/fat/games/SNES/F-Zero.sfc".to_string()],
            vec![
                "/media/fat/games/SNES/Mario World.sfc".to_string(),
                "/media/fat/games/SNES/F-Zero.sfc".to_string(),
            ],
        );

        nav.set_arcade_user_list_mode(&catalog, ArcadeUserListMode::Recent);
        let recent = nav.active_arcade_game_view(&catalog, "snes");
        assert_eq!(recent.len(), 2);
        assert_eq!(recent.get(0).unwrap().title.as_ref(), "Mario World");

        nav.set_arcade_user_list_mode(&catalog, ArcadeUserListMode::Favourites);
        let favourites = nav.active_arcade_game_view(&catalog, "snes");
        assert_eq!(favourites.len(), 1);
        assert_eq!(favourites.get(0).unwrap().title.as_ref(), "F-Zero");
    }

    #[test]
    fn snes_routes_through_hub_and_lists_return_to_it() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("F-Zero")
                    .system_id("snes")
                    .path("/media/fat/games/SNES/F-Zero.sfc")
                    .build(),
            ],
            vec![arcade_system("snes", 1)],
        );
        let mut nav = LauncherNav::new();
        assert!(nav.open_system(&catalog, "snes"));
        assert_eq!(nav.screen, Screen::SystemHub);

        let now = Instant::now();
        nav.handle_input(&pad_with(|pad| pad.btn_a = true), now, &catalog);
        assert_eq!(nav.screen, Screen::Arcade);
        nav.handle_input(
            &PadState::default(),
            now + Duration::from_millis(1),
            &catalog,
        );
        nav.handle_input(
            &pad_with(|pad| pad.btn_b = true),
            now + Duration::from_millis(2),
            &catalog,
        );
        assert_eq!(nav.screen, Screen::SystemHub);
    }

    #[test]
    fn snes_hub_opens_recent_and_favourite_lists() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("F-Zero")
                    .system_id("snes")
                    .path("/media/fat/games/SNES/F-Zero.sfc")
                    .build(),
            ],
            vec![arcade_system("snes", 1)],
        );
        let mut nav = LauncherNav::new();
        nav.set_user_game_refs(
            &catalog,
            ["/media/fat/games/SNES/F-Zero.sfc".to_string()],
            vec!["/media/fat/games/SNES/F-Zero.sfc".to_string()],
        );
        assert!(nav.open_system(&catalog, "snes"));

        let now = Instant::now();
        nav.handle_input(&pad_with(|pad| pad.dpad_right = true), now, &catalog);
        nav.handle_input(
            &PadState::default(),
            now + Duration::from_millis(1),
            &catalog,
        );
        nav.handle_input(
            &pad_with(|pad| pad.btn_a = true),
            now + Duration::from_millis(2),
            &catalog,
        );
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.arcade_user_list_mode(), ArcadeUserListMode::Recent);

        nav.return_arcade_to_system_hub();
        nav.system_hub_selected = 2;
        nav.handle_input(
            &PadState::default(),
            now + Duration::from_millis(3),
            &catalog,
        );
        nav.handle_input(
            &pad_with(|pad| pad.btn_a = true),
            now + Duration::from_millis(4),
            &catalog,
        );
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.arcade_user_list_mode(), ArcadeUserListMode::Favourites);
    }

    #[test]
    fn non_snes_system_still_routes_directly_to_its_game_list() {
        let catalog = arcade_catalog(
            vec![
                arcade_game("Impossible Mission")
                    .system_id("c64")
                    .path("/media/fat/games/C64/Impossible Mission.d64")
                    .build(),
            ],
            vec![arcade_system("c64", 1)],
        );
        let mut nav = LauncherNav::new();

        assert!(nav.open_system(&catalog, "c64"));
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.active_collection_id(), Some("c64"));
    }
}
