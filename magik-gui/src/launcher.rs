//! Launcher navigation and arcade game launch.

use crate::arcade_button_overrides::{remove_button_overrides, write_button_overrides_for_mra};
use crate::arcade_catalog::{
    ArcadeCatalog, ArcadeFilter, ArcadeFilterOption, LaunchTarget, StructuredLaunchPlan,
    ARCADE_ROW_HEIGHT, HOME_LIST_VISIBLE_W, HOME_TILE_GAP, HOME_TILE_WIDTH,
};
use crate::input_repeat::RepeatNav;
use crate::input_state::PadState;
use crate::launcher_taxonomy::{
    LauncherCollection, LauncherMenuItem, LauncherMenuItemKind, LauncherTaxonomy,
    LauncherTaxonomyToken, ROOT_MENU_ID,
};
use crate::library_db;
use crate::settings::MagikSettings;
use crate::spring_animation::{SpringAnimation, SpringConfiguration};
use mister_magik_catalog::media_identity::{
    screenshot_reset_deletes_filename, DEFAULT_SCREENSHOT_ASSET_DIR as DEFAULT_ASSET_DIR,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const HOME_SCROLL_HOLD_DELAY: Duration = Duration::from_millis(200);
const HOME_SCROLL_SPEED_PX_PER_SECOND: f64 = 1440.0;
const HOME_SCROLL_ACCELERATION_PX_PER_SECOND_SQUARED: f64 = 6000.0;

const CMD_FIFO: &str = "/dev/MiSTer_cmd";
const MISTER_BIN: &str = "/media/fat/MiSTer_MagiK";
const MISTER_PROCESS_NAMES: &[&str] = &["MiSTer_MagiK", "MiSTer"];
const MAIN_STATUS_PATH: &str = "/tmp/mister-magik/main-status.json";
const INPUT_POLICY_MARKER_PATH: &str = "/tmp/mister-magik/input-policy";
const MAGIK_INPUT_DIR: &str = "/media/fat/mister-magik/input";
pub const LIBRARY_REBUILD_ON_NEXT_BOOT_PATH: &str = "/media/fat/mister-magik/rebuild-on-next-boot";
#[cfg(test)]
const STATE_FILENAME: &str = mister_magik_catalog::media_identity::SCREENSHOT_MEDIA_STATE_FILENAME;
const ARCADE_NORMAL_PX_PER_SECOND: f64 = 360.0;
const ARCADE_TURBO_PX_PER_SECOND: f64 = 720.0;
const ARCADE_QUICK_TAP_MAX: Duration = Duration::from_millis(220);
const ARCADE_TURBO_REPRESS_WINDOW: Duration = Duration::from_millis(350);
const FIFO_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const FIFO_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const MISTER_START_TIMEOUT: Duration = Duration::from_secs(15);
const MAGIK_HANDOFF_ACK_TIMEOUT: Duration = Duration::from_millis(750);
pub const LAUNCH_RETURN_STATE_PATH: &str = "/tmp/mister-magik/launcher-return-state.json";
const LAUNCH_RETURN_STATE_SCHEMA: u32 = 3;
const SETTINGS_MAX_SELECTED: usize = 5;
const LICENSES_MAX_SELECTED: usize = 2;
const LICENSE_SCROLL_LINE_PX: f64 = 10.0;
pub const ARCADE_SEARCH_KEY_COLUMNS: usize = 8;
pub const ARCADE_SEARCH_KEYS: [&str; 43] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "-", ".",
    "'", "&", "SPACE", "DEL", "CLEAR",
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
    message: String,
    spawned_mister: bool,
}

impl LaunchError {
    fn new(message: impl Into<String>, spawned_mister: bool) -> Self {
        Self {
            message: message.into(),
            spawned_mister,
        }
    }

    #[cfg(feature = "ui")]
    pub fn preparation(message: impl Into<String>) -> Self {
        Self::new(message, false)
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
    Controller,
    Arcade,
    Settings,
    About,
    Licenses,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    ExitToMister,
    ResetDatabase,
    Restart,
    LibraryChanged,
    LibraryUpdateFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherAction {
    LaunchGame,
    ExitToMister,
    ResetDatabase,
    Restart,
    ContinueWithStaleLibrary,
    RebuildLibrary,
}

pub struct LauncherEvent {
    pub action: LauncherAction,
    pub path: Option<String>,
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

    fn restore_position(&mut self, selected: usize, scroll_y: i32, count: usize) {
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
        if let Some(started) = self.scroll.hold_started_at {
            if self.scroll.held_dir == dir {
                if now.saturating_duration_since(started) <= ARCADE_QUICK_TAP_MAX {
                    self.scroll.last_quick_tap_dir = dir;
                    self.scroll.last_quick_tap_released_at = Some(now);
                } else {
                    self.scroll.last_quick_tap_dir = 0;
                    self.scroll.last_quick_tap_released_at = None;
                }
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

pub struct LauncherNav {
    pub screen: Screen,
    pub selected: usize,
    pub scroll_x: i32,
    pub settings_focused: bool,
    pub settings_selected: usize,
    pub settings: MagikSettings,
    pub licenses_selected: usize,
    pub licenses_expanded: bool,
    licenses_scroll: ArcadeNav,
    pub confirm_action: Option<ConfirmAction>,
    pub confirm_selected: usize,
    pub arcade: ArcadeNav,
    pub arcade_filter: ArcadeFilterState,
    pub arcade_search: ArcadeSearchState,
    game_list_memory: HashMap<String, GameListMemory>,
    collection_filters: HashMap<String, ArcadeFilter>,
    collection_search_queries: HashMap<String, String>,
    taxonomy: LauncherTaxonomy,
    taxonomy_token: LauncherTaxonomyToken,
    menu_path: Vec<String>,
    menu_memory: HashMap<String, MenuViewportMemory>,
    active_collection_id: Option<String>,
    repeat: RepeatNav,
    home_scroll: HomeScrollState,
    home_scroll_animation: SpringAnimation,
    prev: PadState,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcadeFilterLevel {
    Alphabet,
    Top,
    Decades,
    Manufacturers,
    Categories,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArcadeSearchPane {
    Keyboard,
    Results,
}

#[derive(Clone, Debug)]
pub struct ArcadeSearchState {
    pub query: String,
    pub suggestion: String,
    pub preparing: bool,
    pub selected_key: usize,
    pub pane: ArcadeSearchPane,
    results: Vec<usize>,
    result_system_id: String,
    result_query: String,
    suggestion_system_id: String,
    suggestion_query: String,
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
            preparing: false,
            selected_key: 0,
            pane: ArcadeSearchPane::Keyboard,
            results: Vec::new(),
            result_system_id: String::new(),
            result_query: String::new(),
            suggestion_system_id: String::new(),
            suggestion_query: String::new(),
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
            scroll: ArcadeNav::with_row_height(ARCADE_ROW_HEIGHT),
        }
    }

    pub fn title(&self) -> &'static str {
        match self.level {
            ArcadeFilterLevel::Alphabet => "Games A-Z",
            ArcadeFilterLevel::Top => "Filters",
            ArcadeFilterLevel::Decades => "Decades",
            ArcadeFilterLevel::Manufacturers => "Manufacturers",
            ArcadeFilterLevel::Categories => "Categories",
        }
    }

    pub fn active_label(&self) -> String {
        match &self.active {
            ArcadeFilter::All => "Games A-Z".to_string(),
            ArcadeFilter::Search => "Search".to_string(),
            ArcadeFilter::Decade(decade) => format!("{decade}'s"),
            ArcadeFilter::Manufacturer(manufacturer) => manufacturer.clone(),
            ArcadeFilter::Category(category) => category.clone(),
        }
    }

    pub fn is_scroll_active(&self) -> bool {
        self.scroll.is_scroll_active()
    }

    fn active_group_index(&self) -> usize {
        match self.active {
            ArcadeFilter::All => 0,
            ArcadeFilter::Search => 1,
            ArcadeFilter::Decade(_) => 2,
            ArcadeFilter::Manufacturer(_) => 3,
            ArcadeFilter::Category(_) => 4,
        }
    }

    fn active_level(&self) -> ArcadeFilterLevel {
        match self.active {
            ArcadeFilter::All | ArcadeFilter::Search => ArcadeFilterLevel::Top,
            ArcadeFilter::Decade(_) => ArcadeFilterLevel::Decades,
            ArcadeFilter::Manufacturer(_) => ArcadeFilterLevel::Manufacturers,
            ArcadeFilter::Category(_) => ArcadeFilterLevel::Categories,
        }
    }
}

impl Default for LauncherNav {
    fn default() -> Self {
        Self::new()
    }
}

impl LauncherNav {
    pub fn home_horizontal_held(&self) -> bool {
        self.screen == Screen::Home && !self.settings_focused && self.home_scroll.held_dir != 0
    }

    pub fn home_horizontal_repeat_active(&self) -> bool {
        self.screen == Screen::Home && !self.settings_focused && self.home_scroll.active
    }

    pub fn new() -> Self {
        Self {
            screen: Screen::Home,
            selected: 0,
            scroll_x: 0,
            settings_focused: false,
            settings_selected: 0,
            settings: MagikSettings::default(),
            licenses_selected: 0,
            licenses_expanded: false,
            licenses_scroll: ArcadeNav::with_row_height_and_step(LICENSE_SCROLL_LINE_PX as i32, 3),
            confirm_action: None,
            confirm_selected: 0,
            arcade: ArcadeNav::new(),
            arcade_filter: ArcadeFilterState::new(),
            arcade_search: ArcadeSearchState::new(),
            game_list_memory: HashMap::new(),
            collection_filters: HashMap::new(),
            collection_search_queries: HashMap::new(),
            taxonomy: LauncherTaxonomy::default(),
            taxonomy_token: LauncherTaxonomyToken::default(),
            menu_path: vec![ROOT_MENU_ID.to_string()],
            menu_memory: HashMap::new(),
            active_collection_id: None,
            repeat: RepeatNav::default(),
            home_scroll: HomeScrollState::default(),
            home_scroll_animation: SpringAnimation::new(0.0, SpringConfiguration::smooth()),
            prev: PadState::default(),
        }
    }

    /// Rebuilds the cached launcher hierarchy when the catalog allocation or
    /// system projection changes. Call this after publishing a catalog before
    /// reading menu or active-collection state.
    pub fn sync_launcher_taxonomy(&mut self, catalog: &ArcadeCatalog) -> bool {
        let token = LauncherTaxonomyToken::from_catalog(catalog);
        if self.taxonomy_token == token && self.taxonomy.matches_catalog(catalog) {
            return false;
        }

        if self.screen == Screen::Home {
            self.remember_current_menu_view();
        }
        let old_path = self.menu_path.clone();
        let old_collection = self.active_collection_id.clone();
        let had_active_collection = old_collection.is_some();
        self.taxonomy = LauncherTaxonomy::from_catalog(catalog);
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
        if let Some(collection_id) = self.active_collection_id.clone() {
            if !self
                .taxonomy
                .collection_path_is_valid(&self.menu_path, &collection_id)
            {
                if let Some(destination) = self
                    .taxonomy
                    .primary_destination_for_collection(&collection_id)
                {
                    self.menu_path = destination.menu_path.clone();
                }
            }
        }

        if self.screen == Screen::Arcade && self.active_collection_id.is_none() {
            if had_active_collection {
                self.screen = Screen::Home;
                self.restore_current_menu_view();
                return true;
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
        true
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

    pub fn current_menu_game_count(&self) -> usize {
        self.taxonomy
            .menu(self.current_menu_id())
            .map(|menu| menu.count)
            .unwrap_or(0)
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

    fn activate_collection(&mut self, catalog: &ArcadeCatalog, collection_id: &str) -> bool {
        let Some(collection) = self.taxonomy.collection(collection_id).cloned() else {
            return false;
        };
        if self.screen == Screen::Home {
            self.remember_current_menu_view();
        }
        self.active_collection_id = Some(collection.id.clone());
        let filter = self
            .collection_filters
            .get(&collection.id)
            .cloned()
            .unwrap_or_else(|| default_filter_for_system(catalog, &collection.id));
        self.arcade_filter.active = filter;
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

    /// Returns an event when a launch or system action was requested.
    pub fn handle_input(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        self.sync_launcher_taxonomy(catalog);
        let result = if self.confirm_action.is_some() {
            self.handle_confirm(now, frame_now)
        } else {
            match self.screen {
                Screen::Home => self.handle_home(now, frame_now, catalog),
                Screen::Controller => {
                    if rising(now.btn_home, self.prev.btn_home) {
                        self.go_root();
                    } else if rising(now.btn_b, self.prev.btn_b) {
                        self.screen = Screen::Home;
                        self.restore_current_menu_view();
                    }
                    None
                }
                Screen::Arcade => self.handle_arcade(now, frame_now, catalog),
                Screen::Settings => self.handle_settings(now, frame_now),
                Screen::About | Screen::Info => {
                    self.handle_settings_subscreen(now);
                    None
                }
                Screen::Licenses => self.handle_licenses(now, frame_now),
            }
        };
        self.prev = now.clone();
        result
    }

    fn handle_home(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        if rising(now.btn_home, self.prev.btn_home) {
            self.go_root();
            return None;
        }
        if rising(now.btn_b, self.prev.btn_b) {
            self.pop_menu();
            return None;
        }

        let item_count = self.current_menu_count();
        if self.repeat.tick_up(now.dpad_up, frame_now) {
            self.settings_focused = true;
        }
        if self.repeat.tick_down(now.dpad_down, frame_now) {
            self.settings_focused = false;
        }
        if self.settings_focused {
            self.home_scroll = HomeScrollState::default();
            self.home_scroll_animation.snap_to(self.scroll_x as f64);
            if rising(now.btn_a, self.prev.btn_a) {
                self.remember_current_menu_view();
                self.settings_selected = 0;
                self.screen = Screen::Settings;
            }
            return None;
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
        self.update_home_scroll(now, frame_now, item_count);

        if rising(now.btn_a, self.prev.btn_a) {
            let item = self.current_menu_items().get(self.selected).cloned();
            if let Some(item) = item {
                match item.kind {
                    LauncherMenuItemKind::Menu => {
                        self.open_menu(&item.id);
                    }
                    LauncherMenuItemKind::Collection => {
                        self.activate_collection(catalog, &item.id);
                    }
                }
            }
        }

        None
    }

    fn update_home_scroll(&mut self, now: &PadState, frame_now: Instant, count: usize) {
        let delta = self
            .home_scroll
            .last_frame_at
            .map_or(Duration::ZERO, |previous| {
                frame_now.saturating_duration_since(previous)
            });
        self.home_scroll.last_frame_at = Some(frame_now);

        let dir = i32::from(now.dpad_right) - i32::from(now.dpad_left);
        let previous_dir = i32::from(self.prev.dpad_right) - i32::from(self.prev.dpad_left);
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
            self.home_scroll_animation.set_target(target as f64);
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
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        let collection_id = self.active_collection_scope_id(catalog).to_string();
        let count = self.active_arcade_game_count(catalog, &collection_id);

        if self.arcade_filter.drawer_open {
            return self.handle_arcade_filter(now, frame_now, catalog, &collection_id);
        }

        if self.arcade_search.is_active(&self.arcade_filter.active) {
            return self.handle_arcade_search(now, frame_now, catalog, &collection_id);
        }

        if rising(now.btn_home, self.prev.btn_home) {
            self.leave_arcade(true, &collection_id);
            return None;
        }
        if rising(now.btn_b, self.prev.btn_b) {
            self.leave_arcade(false, &collection_id);
            return None;
        }

        if count == 0 {
            if rising(now.dpad_left, self.prev.dpad_left) {
                self.open_arcade_filter(catalog, &collection_id);
            }
            return None;
        }

        if rising(now.dpad_left, self.prev.dpad_left) {
            self.open_arcade_alphabet(catalog, &collection_id);
            return None;
        }

        if self.arcade.selected >= count {
            self.arcade.selected = count - 1;
            self.arcade.snap_to_selected();
        }

        let dir = arcade_dpad_dir(now);
        let previous_dir = arcade_dpad_dir(&self.prev);
        self.arcade
            .handle_direction_input(dir, previous_dir, frame_now, count);
        self.arcade.tick(count, frame_now);

        if rising(now.btn_a, self.prev.btn_a) {
            return self
                .active_arcade_game_at(catalog, &collection_id, self.arcade.selected)
                .map(|game| LauncherEvent {
                    action: LauncherAction::LaunchGame,
                    path: Some(game.mra_path.to_string()),
                });
        }

        None
    }

    fn handle_arcade_search(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> Option<LauncherEvent> {
        self.ensure_arcade_search_results(catalog, system_id);
        let count = self.active_arcade_game_count(catalog, system_id);
        if rising(now.btn_home, self.prev.btn_home) {
            self.leave_arcade(true, system_id);
            return None;
        }
        match self.arcade_search.pane {
            ArcadeSearchPane::Keyboard => {
                if rising(now.btn_b, self.prev.btn_b) {
                    if self.arcade_search.query.is_empty() {
                        self.apply_arcade_filter(catalog, system_id, ArcadeFilter::All);
                    } else {
                        self.arcade_search.query.pop();
                        self.refresh_arcade_search_results(catalog, system_id);
                    }
                    return None;
                }
                if rising(now.btn_y, self.prev.btn_y) {
                    self.accept_arcade_search_suggestion(catalog, system_id);
                    return None;
                }
                if self.repeat.tick_left(now.dpad_left, frame_now) {
                    self.move_arcade_search_key(-1, 0);
                }
                if self.repeat.tick_right(now.dpad_right, frame_now) {
                    if search_key_is_row_end(self.arcade_search.selected_key) && count > 0 {
                        self.arcade_search.pane = ArcadeSearchPane::Results;
                    } else {
                        self.move_arcade_search_key(1, 0);
                    }
                }
                if self.repeat.tick_up(now.dpad_up, frame_now) {
                    self.move_arcade_search_key(0, -1);
                }
                if self.repeat.tick_down(now.dpad_down, frame_now) {
                    self.move_arcade_search_key(0, 1);
                }
                if rising(now.btn_a, self.prev.btn_a) {
                    self.activate_arcade_search_key(catalog, system_id);
                }
            }
            ArcadeSearchPane::Results => {
                if rising(now.btn_b, self.prev.btn_b) || rising(now.dpad_left, self.prev.dpad_left)
                {
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
                let dir = arcade_dpad_dir(now);
                let previous_dir = arcade_dpad_dir(&self.prev);
                self.arcade
                    .handle_direction_input(dir, previous_dir, frame_now, count);
                self.arcade.tick(count, frame_now);
                if rising(now.btn_a, self.prev.btn_a) {
                    return self
                        .active_arcade_game_at(catalog, system_id, self.arcade.selected)
                        .map(|game| LauncherEvent {
                            action: LauncherAction::LaunchGame,
                            path: Some(game.mra_path.to_string()),
                        });
                }
            }
        }
        None
    }

    fn handle_arcade_filter(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> Option<LauncherEvent> {
        let items = self.arcade_filter_items(catalog, system_id);
        if rising(now.dpad_right, self.prev.dpad_right) {
            self.activate_arcade_filter_selection(catalog, system_id, &items);
            return None;
        }
        if rising(now.btn_home, self.prev.btn_home) {
            self.close_arcade_filter();
            self.leave_arcade(true, system_id);
            return None;
        }
        if rising(now.btn_b, self.prev.btn_b) {
            self.back_out_of_arcade_filter_level(catalog, system_id, true);
            return None;
        }
        if rising(now.dpad_left, self.prev.dpad_left) {
            self.back_out_of_arcade_filter_level(catalog, system_id, false);
            return None;
        }
        if !items.is_empty() {
            let dir = arcade_dpad_dir(now);
            let previous_dir = arcade_dpad_dir(&self.prev);
            self.arcade_filter.scroll.handle_direction_input(
                dir,
                previous_dir,
                frame_now,
                items.len(),
            );
            self.arcade_filter.scroll.tick(items.len(), frame_now);
            self.sync_arcade_filter_from_scroll();
        } else {
            self.arcade_filter.scroll.reset();
            self.sync_arcade_filter_from_scroll();
        }
        if rising(now.btn_a, self.prev.btn_a) {
            self.activate_arcade_filter_selection(catalog, system_id, &items);
        }
        None
    }

    fn handle_settings(&mut self, now: &PadState, frame_now: Instant) -> Option<LauncherEvent> {
        if rising(now.btn_home, self.prev.btn_home) {
            self.go_root();
            return None;
        }
        if rising(now.btn_b, self.prev.btn_b) {
            self.screen = Screen::Home;
            self.restore_current_menu_view();
            return None;
        }
        if self.repeat.tick_down(now.dpad_down, frame_now)
            && self.settings_selected < SETTINGS_MAX_SELECTED
        {
            self.settings_selected += 1;
        }
        if self.repeat.tick_up(now.dpad_up, frame_now) && self.settings_selected > 0 {
            self.settings_selected -= 1;
        }
        if rising(now.btn_a, self.prev.btn_a) {
            if self.settings_selected == 1 {
                let mut next_settings = self.settings.clone();
                next_settings.simple_joystick_handling = !next_settings.simple_joystick_handling;
                match next_settings.save() {
                    Ok(()) => self.settings = next_settings,
                    Err(e) => {
                        crate::ui_errln!("settings: failed to save simple joystick toggle: {e}")
                    }
                }
                return None;
            }
            if self.settings_selected == 3 {
                self.screen = Screen::About;
                return None;
            }
            if self.settings_selected == 4 {
                self.licenses_selected = 0;
                self.licenses_expanded = false;
                self.licenses_scroll.reset();
                self.screen = Screen::Licenses;
                return None;
            }
            if self.settings_selected == 5 {
                self.screen = Screen::Info;
                return None;
            }
            self.confirm_selected = if self.settings_selected == 0 { 1 } else { 0 };
            self.confirm_action = Some(match self.settings_selected {
                0 => ConfirmAction::ExitToMister,
                _ => ConfirmAction::ResetDatabase,
            });
        }
        None
    }

    fn handle_licenses(&mut self, now: &PadState, frame_now: Instant) -> Option<LauncherEvent> {
        if rising(now.btn_home, self.prev.btn_home) {
            self.licenses_expanded = false;
            self.licenses_scroll.reset();
            self.go_root();
            return None;
        }
        if self.licenses_expanded {
            if rising(now.btn_a, self.prev.btn_a) || rising(now.btn_b, self.prev.btn_b) {
                self.licenses_expanded = false;
                self.licenses_scroll.reset();
            } else {
                let count = crate::licenses::max_scroll_line(self.licenses_selected) + 1;
                self.licenses_scroll.handle_direction_input(
                    arcade_dpad_dir(now),
                    arcade_dpad_dir(&self.prev),
                    frame_now,
                    count,
                );
                self.licenses_scroll.tick(count, frame_now);
            }
            return None;
        }
        if rising(now.btn_b, self.prev.btn_b) {
            self.screen = Screen::Settings;
            self.licenses_scroll.reset();
            return None;
        }
        if self.repeat.tick_down(now.dpad_down, frame_now)
            && self.licenses_selected < LICENSES_MAX_SELECTED
        {
            self.licenses_selected += 1;
        }
        if self.repeat.tick_up(now.dpad_up, frame_now) && self.licenses_selected > 0 {
            self.licenses_selected -= 1;
        }
        if rising(now.btn_a, self.prev.btn_a) {
            self.licenses_expanded = true;
            self.licenses_scroll.reset();
        }
        None
    }

    fn handle_settings_subscreen(&mut self, now: &PadState) {
        if rising(now.btn_home, self.prev.btn_home) {
            self.go_root();
        } else if rising(now.btn_b, self.prev.btn_b) {
            self.screen = Screen::Settings;
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

    fn handle_confirm(&mut self, now: &PadState, frame_now: Instant) -> Option<LauncherEvent> {
        let home_pressed = rising(now.btn_home, self.prev.btn_home);
        if rising(now.btn_b, self.prev.btn_b) || home_pressed {
            if self.confirm_action == Some(ConfirmAction::LibraryChanged) {
                self.confirm_action = None;
                self.confirm_selected = 0;
                if home_pressed {
                    self.go_root();
                }
                return Some(LauncherEvent {
                    action: LauncherAction::ContinueWithStaleLibrary,
                    path: None,
                });
            }
            self.confirm_action = None;
            self.confirm_selected = 0;
            if home_pressed {
                self.go_root();
            }
            return None;
        }
        let max_selected = confirm_max_selected(self.confirm_action);
        if self.confirm_selected > max_selected {
            self.confirm_selected = max_selected;
        }
        if self.repeat.tick_left(now.dpad_left, frame_now) && self.confirm_selected > 0 {
            self.confirm_selected -= 1;
        }
        if self.repeat.tick_right(now.dpad_right, frame_now) && self.confirm_selected < max_selected
        {
            self.confirm_selected += 1;
        }
        if rising(now.btn_a, self.prev.btn_a) {
            let action = self.confirm_action;
            let selected = self.confirm_selected;
            let confirmed = match action {
                Some(ConfirmAction::ExitToMister) => selected == 0,
                Some(ConfirmAction::LibraryChanged) => true,
                Some(ConfirmAction::LibraryUpdateFailed) => false,
                _ => selected == 1,
            };
            self.confirm_action = None;
            self.confirm_selected = 0;
            if confirmed {
                return match action {
                    Some(ConfirmAction::ExitToMister) => Some(LauncherEvent {
                        action: LauncherAction::ExitToMister,
                        path: None,
                    }),
                    Some(ConfirmAction::ResetDatabase) => Some(LauncherEvent {
                        action: LauncherAction::ResetDatabase,
                        path: None,
                    }),
                    Some(ConfirmAction::Restart) => Some(LauncherEvent {
                        action: LauncherAction::Restart,
                        path: None,
                    }),
                    Some(ConfirmAction::LibraryChanged) => Some(LauncherEvent {
                        action: if selected == 0 {
                            LauncherAction::ContinueWithStaleLibrary
                        } else {
                            LauncherAction::RebuildLibrary
                        },
                        path: None,
                    }),
                    Some(ConfirmAction::LibraryUpdateFailed) => None,
                    None => None,
                };
            }
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

    pub fn active_arcade_game_view<'a>(
        &'a self,
        catalog: &'a ArcadeCatalog,
        system_id: &str,
    ) -> crate::arcade_catalog::ArcadeGameView<'a> {
        let system_id = self.effective_collection_id(system_id);
        if self.arcade_search.is_active(&self.arcade_filter.active)
            && !self.arcade_search.query.is_empty()
            && self.arcade_search.result_system_id == system_id
            && self.arcade_search.result_query == self.arcade_search.query
        {
            crate::arcade_catalog::ArcadeGameView::indexed(
                &catalog.games,
                &self.arcade_search.results,
            )
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
            ArcadeFilterLevel::Categories => filter_option_items(
                catalog.category_options(system_id),
                |label| Some(ArcadeFilter::Category(label.to_string())),
                &self.arcade_filter.active,
            ),
        }
    }

    fn arcade_filter_top_items(
        &self,
        catalog: &ArcadeCatalog,
        system_id: &str,
    ) -> Vec<ArcadeDrawerItem> {
        vec![
            ArcadeDrawerItem {
                label: "Games A-Z".to_string(),
                count: catalog.system_game_count(system_id),
                active: self.arcade_filter.active == ArcadeFilter::All,
            },
            ArcadeDrawerItem {
                label: "Search".to_string(),
                count: catalog.system_game_count(system_id),
                active: matches!(self.arcade_filter.active, ArcadeFilter::Search),
            },
            ArcadeDrawerItem {
                label: "Decades".to_string(),
                count: catalog.decade_option_count(system_id),
                active: matches!(self.arcade_filter.active, ArcadeFilter::Decade(_)),
            },
            ArcadeDrawerItem {
                label: "Manufacturer".to_string(),
                count: catalog.manufacturer_option_count(system_id),
                active: matches!(self.arcade_filter.active, ArcadeFilter::Manufacturer(_)),
            },
            ArcadeDrawerItem {
                label: "Categories".to_string(),
                count: catalog.category_option_count(system_id),
                active: matches!(self.arcade_filter.active, ArcadeFilter::Category(_)),
            },
        ]
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
        self.arcade_filter.drawer_open = true;
        self.arcade_filter.level = self.arcade_filter.active_level();
        self.arcade_filter.selected = if self.arcade_filter.level == ArcadeFilterLevel::Top {
            self.arcade_filter.active_group_index()
        } else {
            0
        };
        let items = self.arcade_filter_items(catalog, system_id);
        if self.arcade_filter.level != ArcadeFilterLevel::Top {
            if let Some(active_idx) = items.iter().position(|item| item.active) {
                self.arcade_filter.selected = active_idx;
            }
        }
        self.snap_arcade_filter_scroll(items.len());
    }

    fn close_arcade_filter(&mut self) {
        self.arcade_filter.drawer_open = false;
        self.arcade_filter.level = ArcadeFilterLevel::Top;
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
        } else {
            self.arcade_filter.level = ArcadeFilterLevel::Top;
            self.arcade_filter.selected = self.arcade_filter.active_group_index();
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
            ArcadeFilterLevel::Top => match self.arcade_filter.selected {
                0 => self.apply_arcade_filter(catalog, system_id, ArcadeFilter::All),
                1 => self.enter_arcade_search(catalog, system_id),
                2 => self.enter_arcade_filter_level(catalog, system_id, ArcadeFilterLevel::Decades),
                3 => self.enter_arcade_filter_level(
                    catalog,
                    system_id,
                    ArcadeFilterLevel::Manufacturers,
                ),
                4 => self.enter_arcade_filter_level(
                    catalog,
                    system_id,
                    ArcadeFilterLevel::Categories,
                ),
                _ => {}
            },
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
            ArcadeFilterLevel::Categories => {
                self.apply_arcade_filter(
                    catalog,
                    system_id,
                    ArcadeFilter::Category(items[self.arcade_filter.selected].label.clone()),
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
            || self.arcade_search.preparing
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
        self.arcade_search.preparing = false;
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
            self.arcade_search.results.clear();
            self.arcade_search.suggestion.clear();
            self.arcade_search.result_system_id = system_id.to_string();
            self.arcade_search.result_query = self.arcade_search.query.clone();
            self.arcade_search.suggestion_system_id = system_id.to_string();
            self.arcade_search.suggestion_query = self.arcade_search.query.clone();
            self.arcade_search.preparing = true;
            self.arcade_search.pane = ArcadeSearchPane::Keyboard;
            self.arcade.reset();
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
            self.arcade_search.preparing = true;
            self.arcade_search.pane = ArcadeSearchPane::Keyboard;
            self.arcade.reset();
            return;
        };
        self.arcade_search.results = results;
        self.arcade_search.suggestion = suggestion;
        self.arcade_search.preparing = false;
        self.arcade_search.result_system_id = system_id.to_string();
        self.arcade_search.result_query = self.arcade_search.query.clone();
        self.arcade_search.suggestion_system_id = system_id.to_string();
        self.arcade_search.suggestion_query = self.arcade_search.query.clone();
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
        match key {
            "SPACE" => self.arcade_search.query.push(' '),
            "DEL" => {
                self.arcade_search.query.pop();
            }
            "CLEAR" => self.arcade_search.query.clear(),
            value => self.arcade_search.query.push_str(value),
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

fn filter_memory_key(filter: &ArcadeFilter) -> String {
    match filter {
        ArcadeFilter::All => "all".to_string(),
        ArcadeFilter::Search => "search".to_string(),
        ArcadeFilter::Decade(decade) => format!("decade:{decade}"),
        ArcadeFilter::Manufacturer(manufacturer) => format!("manufacturer:{manufacturer}"),
        ArcadeFilter::Category(category) => format!("category:{category}"),
    }
}

fn collection_filter_memory_key(collection_id: &str, filter: &ArcadeFilter) -> String {
    format!("{collection_id}\0{}", filter_memory_key(filter))
}

fn default_filter_for_system(catalog: &ArcadeCatalog, system_id: &str) -> ArcadeFilter {
    let games = ArcadeFilter::Category("Games".to_string());
    if system_id == "amiga" && catalog.filtered_game_count(system_id, &games) > 0 {
        games
    } else {
        ArcadeFilter::All
    }
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
    filter_kind: Option<String>,
    filter_value: Option<String>,
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
        filter_kind: Some(filter_kind),
        filter_value,
    })
}

pub fn save_launch_return_state(state: &LaunchReturnState) -> Result<(), String> {
    save_launch_return_state_at(Path::new(LAUNCH_RETURN_STATE_PATH), state)
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
    remove_launch_return_state_at(Path::new(LAUNCH_RETURN_STATE_PATH));
}

fn remove_launch_return_state_at(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => crate::ui_errln!(
            "failed to remove launch return state {}: {e}",
            path.display()
        ),
    }
}

pub fn take_launch_return_state() -> Option<LaunchReturnState> {
    take_launch_return_state_at(Path::new(LAUNCH_RETURN_STATE_PATH))
}

fn take_launch_return_state_at(path: &Path) -> Option<LaunchReturnState> {
    let text = fs::read_to_string(path).ok()?;
    remove_launch_return_state_at(path);
    match serde_json::from_str::<LaunchReturnState>(&text) {
        Ok(state)
            if (1..=LAUNCH_RETURN_STATE_SCHEMA).contains(&state.schema_version)
                && state.screen == "arcade" =>
        {
            Some(state)
        }
        Ok(_) => None,
        Err(e) => {
            crate::ui_errln!("invalid launch return state {}: {e}", path.display());
            None
        }
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
        nav.ensure_arcade_search_results(catalog, &collection_id);
    }
    let (game_index, game_count) = {
        let games = nav.active_arcade_game_view(catalog, &collection_id);
        if games.is_empty() {
            return false;
        }
        (
            games
                .iter()
                .position(|game| game.mra_path.as_ref() == state.game_path)
                .unwrap_or_else(|| state.game_index.min(games.len() - 1)),
            games.len(),
        )
    };

    nav.screen = Screen::Arcade;
    nav.arcade_filter.active = filter;
    nav.arcade_filter.drawer_open = false;
    nav.arcade_filter.level = ArcadeFilterLevel::Top;
    nav.arcade.restore_position(
        game_index,
        game_index as i32 * ARCADE_ROW_HEIGHT,
        game_count,
    );
    true
}

fn resolve_return_destination(
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
    state: &LaunchReturnState,
) -> Option<(Vec<String>, String)> {
    if let Some(collection_id) = state.collection_id.as_deref() {
        if nav.taxonomy.collection(collection_id).is_some() {
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
        ArcadeFilter::Decade(decade) => ("decade".to_string(), Some(decade.to_string())),
        ArcadeFilter::Manufacturer(manufacturer) => {
            ("manufacturer".to_string(), Some(manufacturer.clone()))
        }
        ArcadeFilter::Category(category) => ("category".to_string(), Some(category.clone())),
    }
}

fn deserialize_arcade_filter(kind: &str, value: Option<&str>) -> Option<ArcadeFilter> {
    match kind {
        "all" => Some(ArcadeFilter::All),
        "search" => Some(ArcadeFilter::Search),
        "decade" => value
            .and_then(|value| value.parse::<u16>().ok())
            .map(ArcadeFilter::Decade),
        "manufacturer" => value.map(|value| ArcadeFilter::Manufacturer(value.to_string())),
        "category" => value.map(|value| ArcadeFilter::Category(value.to_string())),
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
    let next_tile_preview = HOME_TILE_GAP + (HOME_TILE_WIDTH + 1) / 2;
    if visible_left < *scroll_x {
        *scroll_x = visible_left;
    }
    if visible_right + next_tile_preview > *scroll_x + HOME_LIST_VISIBLE_W {
        *scroll_x = visible_right + next_tile_preview - HOME_LIST_VISIBLE_W;
    }
    *scroll_x = (*scroll_x).clamp(0, home_max_scroll(count));
}

fn rising(now: bool, prev: bool) -> bool {
    now && !prev
}

fn confirm_max_selected(action: Option<ConfirmAction>) -> usize {
    match action {
        Some(ConfirmAction::LibraryUpdateFailed) => 0,
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

fn wait_until(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    ready()
}

fn wait_for_fifo() -> bool {
    wait_until(FIFO_WAIT_TIMEOUT, || Path::new(CMD_FIFO).exists())
}

fn wait_for_mister_and_fifo() -> bool {
    wait_until(MISTER_START_TIMEOUT, || {
        Path::new(CMD_FIFO).exists() && mister_running()
    })
}

fn write_mister_command_nonblocking(cmd: &str) -> Result<(), String> {
    let start = Instant::now();
    let mut last_error = None;
    while start.elapsed() < FIFO_WRITE_TIMEOUT {
        match std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(CMD_FIFO)
        {
            Ok(mut f) => {
                let bytes = cmd.as_bytes();
                let mut written = 0usize;
                while written < bytes.len() && start.elapsed() < FIFO_WRITE_TIMEOUT {
                    match f.write(&bytes[written..]) {
                        Ok(0) => {
                            last_error = Some("zero-length FIFO write".to_string());
                            break;
                        }
                        Ok(n) => written += n,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            last_error = Some(e.to_string());
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(e) => {
                            return Err(format!("failed to write {CMD_FIFO}: {e}"));
                        }
                    }
                }
                if written == bytes.len() {
                    return Ok(());
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.raw_os_error() == Some(libc::ENXIO) =>
            {
                last_error = Some(e.to_string());
            }
            Err(e) => return Err(format!("failed to open {CMD_FIFO}: {e}")),
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "timed out writing {CMD_FIFO}: {}",
        last_error.unwrap_or_else(|| "no reader".to_string())
    ))
}

trait LaunchIo {
    fn target_exists(&mut self, path: &str) -> bool;
    fn mister_running(&mut self) -> bool;
    fn magik_running(&mut self) -> bool;
    fn simple_joystick_handling(&mut self) -> bool;
    fn prepare_simple_input_profiles(&mut self) -> Result<(), String>;
    fn start_mister(&mut self) -> Result<(), String>;
    fn wait_for_started_mister(&mut self) -> bool;
    fn wait_for_command_fifo(&mut self) -> bool;
    fn write_input_policy_marker(&mut self, simple_joystick_handling: bool) -> Result<(), String>;
    fn write_button_overrides(
        &mut self,
        launch_target: &LaunchTarget,
        simple_joystick_handling: bool,
    ) -> Result<(), String>;
    fn write_mister_command(&mut self, cmd: &str) -> Result<(), String>;
    fn wait_for_magik_handoff_ack(&mut self, before: Option<MagikMainStatusSnapshot>) -> bool;
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
        Command::new("pidof")
            .arg("MiSTer_MagiK")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn simple_joystick_handling(&mut self) -> bool {
        MagikSettings::load().simple_joystick_handling
    }

    fn prepare_simple_input_profiles(&mut self) -> Result<(), String> {
        write_builtin_simple_input_profiles()
    }

    fn start_mister(&mut self) -> Result<(), String> {
        Command::new(MISTER_BIN)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("failed to spawn {MISTER_BIN}: {e}"))
    }

    fn wait_for_started_mister(&mut self) -> bool {
        wait_for_mister_and_fifo()
    }

    fn wait_for_command_fifo(&mut self) -> bool {
        wait_for_fifo()
    }

    fn write_input_policy_marker(&mut self, simple_joystick_handling: bool) -> Result<(), String> {
        write_input_policy_marker(simple_joystick_handling)
    }

    fn write_button_overrides(
        &mut self,
        launch_target: &LaunchTarget,
        simple_joystick_handling: bool,
    ) -> Result<(), String> {
        write_button_overrides_for_launch(launch_target, simple_joystick_handling)
    }

    fn write_mister_command(&mut self, cmd: &str) -> Result<(), String> {
        write_mister_command_nonblocking(cmd)
    }

    fn wait_for_magik_handoff_ack(&mut self, before: Option<MagikMainStatusSnapshot>) -> bool {
        wait_for_magik_handoff_ack(before)
    }
}

fn mister_running() -> bool {
    MISTER_PROCESS_NAMES.iter().any(|name| {
        Command::new("pidof")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
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
    if io.wait_for_started_mister() {
        thread::sleep(Duration::from_millis(200));
        return Ok(());
    }
    Err(format!("timed out waiting for {MISTER_BIN} + {CMD_FIFO}"))
}

fn restore_menu_wallpaper() {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(
            "[ -f /media/fat/mister-magik/.menu.png.boot-hide ] && mv /media/fat/mister-magik/.menu.png.boot-hide /media/fat/menu.png 2>/dev/null || true",
        )
        .status();
}

fn write_mister_command(cmd: &str) -> Result<(), String> {
    write_mister_command_nonblocking(cmd)
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
    launch_target: &LaunchTarget,
    simple_joystick_handling: bool,
) -> Result<(), String> {
    if !simple_joystick_handling {
        return remove_button_overrides();
    }

    match launch_target {
        LaunchTarget::Path(path) if path.to_ascii_lowercase().ends_with(".mra") => {
            write_button_overrides_for_mra(Path::new(path.as_ref()))
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
    fs::create_dir_all(MAGIK_INPUT_DIR)
        .map_err(|e| format!("failed to create {MAGIK_INPUT_DIR}: {e}"))?;
    let path = Path::new(MAGIK_INPUT_DIR).join(name);
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

fn encode_launch_plan(plan: &StructuredLaunchPlan) -> String {
    let mount_index = plan.mount_index.to_string();
    let delay_secs = plan.delay_secs.to_string();
    let fields = [
        ("schema", "1"),
        ("launch_ref", plan.launch_ref.as_ref()),
        ("title", plan.title.as_ref()),
        ("system_id", plan.system_id.as_ref()),
        ("core_path", plan.core_path.as_ref()),
        ("payload_path", plan.payload_path.as_ref()),
        ("mount_kind", plan.mount_kind.as_ref()),
        ("mount_index", mount_index.as_str()),
        ("delay_secs", delay_secs.as_str()),
    ];
    fields
        .into_iter()
        .map(|(key, value)| format!("{key}={}", percent_encode_plan_value(value)))
        .collect::<Vec<_>>()
        .join("&")
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
        if !wait_for_fifo() {
            return Err(format!("timed out waiting for {CMD_FIFO}"));
        }
        write_mister_command("mister_magik_exit_to_menu\n")?;
    } else {
        spawn_mister()?;
    }

    Ok(())
}

/// True while Slint should keep the loading screen up.
pub fn launch_in_progress() -> bool {
    LAUNCH_STATE.load(Ordering::Acquire) == LAUNCH_SENT
}

/// Main is running an arcade core (argv contains `.rbf`, not `menu.rbf`).
pub fn mister_running_arcade_core() -> bool {
    let output = Command::new("sh")
        .arg("-c")
        .arg(
            "pid=$(pidof MiSTer_MagiK 2>/dev/null || pidof MiSTer 2>/dev/null); [ -n \"$pid\" ] && tr '\\0' ' ' < /proc/$pid/cmdline",
        )
        .output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let cmdline = String::from_utf8_lossy(&output.stdout);
    cmdline.contains(".rbf") && !cmdline.contains("menu.rbf")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MagikMainStatusSnapshot {
    ts_boot_ms: u64,
    handoff_acknowledged: bool,
}

fn read_magik_main_status_snapshot() -> Option<MagikMainStatusSnapshot> {
    let text = fs::read_to_string(MAIN_STATUS_PATH).ok()?;
    magik_main_status_snapshot_from_text(&text)
}

fn wait_for_magik_handoff_ack(before: Option<MagikMainStatusSnapshot>) -> bool {
    let start = Instant::now();
    while start.elapsed() < MAGIK_HANDOFF_ACK_TIMEOUT {
        if magik_main_status_acknowledged_handoff_after(before) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn magik_main_status_acknowledged_handoff_after(before: Option<MagikMainStatusSnapshot>) -> bool {
    let Some(snapshot) = read_magik_main_status_snapshot() else {
        return false;
    };
    if !snapshot.handoff_acknowledged {
        return false;
    }
    magik_handoff_ack_is_newer(before, snapshot)
}

fn magik_main_status_snapshot_from_text(text: &str) -> Option<MagikMainStatusSnapshot> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return None;
    };
    let ts_boot_ms = value.get("ts_boot_ms").and_then(|value| value.as_u64())?;
    let state = value
        .get("launcher_state")
        .and_then(|state| state.as_str())?;
    Some(MagikMainStatusSnapshot {
        ts_boot_ms,
        handoff_acknowledged: matches!(state, "HandoffToGame" | "Unconfigured"),
    })
}

fn magik_handoff_ack_is_newer(
    before: Option<MagikMainStatusSnapshot>,
    snapshot: MagikMainStatusSnapshot,
) -> bool {
    before.is_none_or(|before| snapshot.ts_boot_ms > before.ts_boot_ms)
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

        fn wait_for_started_mister(&mut self) -> bool {
            true
        }

        fn wait_for_command_fifo(&mut self) -> bool {
            let start = Instant::now();
            thread::sleep(self.fifo_delay);
            self.handoff_us = self
                .handoff_us
                .saturating_add(start.elapsed().as_micros() as u64);
            self.mode == LaunchHandoffBenchMode::Success
        }

        fn write_input_policy_marker(
            &mut self,
            _simple_joystick_handling: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn write_button_overrides(
            &mut self,
            _launch_target: &LaunchTarget,
            _simple_joystick_handling: bool,
        ) -> Result<(), String> {
            Ok(())
        }

        fn write_mister_command(&mut self, _cmd: &str) -> Result<(), String> {
            if self.mode == LaunchHandoffBenchMode::Success {
                Ok(())
            } else {
                Err("benchmark handoff does not write the real MiSTer FIFO".to_string())
            }
        }

        fn wait_for_magik_handoff_ack(&mut self, _before: Option<MagikMainStatusSnapshot>) -> bool {
            self.mode == LaunchHandoffBenchMode::Success
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
    if let LaunchTarget::Path(path) = launch_target {
        if !io.target_exists(path) {
            return Err(LaunchError::new(
                format!("launch target not found: {path}"),
                false,
            ));
        }
    }

    let spawned = if io.mister_running() {
        false
    } else {
        crate::ui_logln!("launch: starting {MISTER_BIN} for load_core");
        io.start_mister().map_err(|e| LaunchError::new(e, false))?;
        if !io.wait_for_started_mister() {
            return Err(LaunchError::new(
                format!("timed out waiting for {MISTER_BIN} + {CMD_FIFO}"),
                true,
            ));
        }
        true
    };

    if !io.wait_for_command_fifo() {
        return Err(LaunchError::new(
            format!("timed out waiting for {CMD_FIFO}"),
            spawned,
        ));
    }

    let magik_running = io.magik_running();
    let main_status_before_handoff = magik_running
        .then(read_magik_main_status_snapshot)
        .flatten();
    if magik_running {
        let simple_joystick_handling = io.simple_joystick_handling();
        if simple_joystick_handling {
            io.prepare_simple_input_profiles()
                .map_err(|e| LaunchError::new(e, spawned))?;
        }
        io.write_button_overrides(launch_target, simple_joystick_handling)
            .map_err(|e| LaunchError::new(e, spawned))?;
        io.write_input_policy_marker(simple_joystick_handling)
            .map_err(|e| LaunchError::new(e, spawned))?;
    }
    let cmd = match (magik_running, launch_target) {
        (true, LaunchTarget::Path(path)) => format!("mister_magik_launch {path}\n"),
        (true, LaunchTarget::Structured(plan)) => {
            format!("mister_magik_launch_plan_v1 {}\n", encode_launch_plan(plan))
        }
        (true, LaunchTarget::Prepared(selection)) => {
            return Err(LaunchError::new(
                format!("prepared launch unresolved: {}", selection.launch_ref),
                spawned,
            ));
        }
        (true, LaunchTarget::MissingStructured(launch_ref)) => {
            return Err(LaunchError::new(
                format!("structured launch plan missing from catalog: {launch_ref}"),
                spawned,
            ));
        }
        (false, LaunchTarget::Path(path)) => format!("load_core {path}\n"),
        (false, LaunchTarget::Structured(_)) => {
            return Err(LaunchError::new(
                "structured launch plan requires MiSTer_MagiK".to_string(),
                spawned,
            ));
        }
        (false, LaunchTarget::Prepared(selection)) => {
            return Err(LaunchError::new(
                format!("prepared launch unresolved: {}", selection.launch_ref),
                spawned,
            ));
        }
        (false, LaunchTarget::MissingStructured(launch_ref)) => {
            return Err(LaunchError::new(
                format!("structured launch plan missing from catalog: {launch_ref}"),
                spawned,
            ));
        }
    };
    crate::ui_logln!("launch: {}", cmd.trim_end());
    if let Err(e) = io.write_mister_command(&cmd) {
        if magik_running {
            let _ = io.write_input_policy_marker(false);
        }
        return Err(LaunchError::new(e, spawned));
    }
    if magik_running && !io.wait_for_magik_handoff_ack(main_status_before_handoff) {
        let _ = io.write_input_policy_marker(false);
        return Err(LaunchError::new(
            format!(
                "timed out waiting for MiSTer_MagiK launch acknowledgement in {MAIN_STATUS_PATH}"
            ),
            spawned,
        ));
    }

    LAUNCH_STATE.store(LAUNCH_SENT, Ordering::Release);
    Ok(spawned)
}

pub fn reset_launch() {
    LAUNCH_STATE.store(LAUNCH_IDLE, Ordering::Release);
}

pub fn reset_database_and_reboot() -> Result<(), String> {
    library_db::remove_default_sqlite_database()?;
    delete_screenshot_packs()?;
    reboot_mister()
}

pub fn delete_screenshot_packs() -> Result<usize, String> {
    delete_screenshot_packs_at(Path::new(DEFAULT_ASSET_DIR))
}

fn delete_screenshot_packs_at(asset_dir: &Path) -> Result<usize, String> {
    let entries = match fs::read_dir(asset_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(format!(
                "read screenshot asset dir {}: {e}",
                asset_dir.display()
            ))
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
            mister_magik_catalog::fs_fault::maybe_fault(
                "reset_delete.screenshot_asset.after_remove",
                &path,
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
    Path::new(LIBRARY_REBUILD_ON_NEXT_BOOT_PATH).exists()
}

pub fn request_library_rebuild_on_next_boot() -> Result<(), String> {
    request_library_rebuild_on_next_boot_at(Path::new(LIBRARY_REBUILD_ON_NEXT_BOOT_PATH))
}

pub fn consume_library_rebuild_on_next_boot() -> Result<bool, String> {
    consume_library_rebuild_on_next_boot_at(Path::new(LIBRARY_REBUILD_ON_NEXT_BOOT_PATH))
}

fn request_library_rebuild_on_next_boot_at(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create rebuild marker dir: {e}"))?;
    }
    fs::write(path, b"rebuild\n").map_err(|e| format!("write rebuild marker: {e}"))?;
    mister_magik_catalog::fs_fault::maybe_fault("launcher.rebuild_marker.after_write", path);
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
    if !io.wait_for_command_fifo() {
        return Err(format!("timed out waiting for {CMD_FIFO}"));
    }
    io.write_mister_command("mister_magik_reboot\n")
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

    struct FakeLaunchIo {
        target_exists: bool,
        mister_running: bool,
        magik_running: bool,
        simple_joystick_handling: bool,
        start_result: Result<(), String>,
        started_ready: bool,
        fifo_ready: bool,
        write_result: Result<(), String>,
        handoff_ack: bool,
        start_calls: usize,
        prepare_simple_input_profile_calls: usize,
        input_policy_markers: Vec<bool>,
        button_override_writes: Vec<String>,
        commands: Vec<String>,
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
            Ok(())
        }

        fn start_mister(&mut self) -> Result<(), String> {
            self.start_calls += 1;
            self.start_result.clone()
        }

        fn wait_for_started_mister(&mut self) -> bool {
            self.started_ready
        }

        fn wait_for_command_fifo(&mut self) -> bool {
            self.fifo_ready
        }

        fn write_input_policy_marker(
            &mut self,
            simple_joystick_handling: bool,
        ) -> Result<(), String> {
            self.input_policy_markers.push(simple_joystick_handling);
            Ok(())
        }

        fn write_button_overrides(
            &mut self,
            launch_target: &LaunchTarget,
            simple_joystick_handling: bool,
        ) -> Result<(), String> {
            let action = match (simple_joystick_handling, launch_target) {
                (true, LaunchTarget::Path(path)) if path.to_ascii_lowercase().ends_with(".mra") => {
                    format!("write:{path}")
                }
                _ => "remove".to_string(),
            };
            self.button_override_writes.push(action);
            Ok(())
        }

        fn write_mister_command(&mut self, cmd: &str) -> Result<(), String> {
            self.commands.push(cmd.to_string());
            self.write_result.clone()
        }

        fn wait_for_magik_handoff_ack(&mut self, _before: Option<MagikMainStatusSnapshot>) -> bool {
            self.handoff_ack
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
            handoff_ack: true,
            start_calls: 0,
            prepare_simple_input_profile_calls: 0,
            input_policy_markers: Vec::new(),
            button_override_writes: Vec::new(),
            commands: Vec::new(),
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
            vec![arcade_game("Agony")
                .path("magik-plan:amiga-agony")
                .system_id("amiga")
                .build()],
            vec![arcade_system("amiga", 1)],
        )
    }

    fn amiga_games_and_demos_catalog() -> ArcadeCatalog {
        arcade_catalog(
            vec![
                arcade_game("Agony")
                    .path("magik-amigavision:games:Agony")
                    .system_id("amiga")
                    .category("Games")
                    .build(),
                arcade_game("Alien Breed")
                    .path("magik-amigavision:games:Alien%20Breed")
                    .system_id("amiga")
                    .category("Games")
                    .build(),
                arcade_game("State of the Art")
                    .path("magik-amigavision:demos:State%20of%20the%20Art")
                    .system_id("amiga")
                    .category("Demos")
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
                .category("Shooter / Gallery")
                .build(),
            arcade_game("Battle 1981")
                .path("/media/fat/_Arcade/battle-1981.mra")
                .year(1981)
                .manufacturer("Capcom")
                .category("Shooter / Vertical")
                .build(),
            arcade_game("Brawl 1988")
                .path("/media/fat/_Arcade/brawl-1988.mra")
                .year(1988)
                .manufacturer("Capcom")
                .category("Fighter / 2D")
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
                    .category("Fighter / 2D")
                    .build(),
                arcade_game("Pac-Man")
                    .path("/media/fat/_Arcade/pacman.mra")
                    .year(1980)
                    .manufacturer("Namco")
                    .category("Maze")
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
                    .category("Test")
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
    fn launcher_ignores_home_launch_when_catalog_has_no_systems() {
        let catalog = arcade_catalog(vec![], vec![]);
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);

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
    fn home_viewport_shows_four_tiles_and_half_of_the_next_until_the_end() {
        let half_tile = (HOME_TILE_WIDTH + 1) / 2;
        assert_eq!(
            4 * HOME_TILE_WIDTH + 4 * HOME_TILE_GAP + half_tile,
            HOME_LIST_VISIBLE_W
        );

        let mut scroll_x = 0;
        keep_home_visible(4, &mut scroll_x, 10);
        assert_eq!(scroll_x, home_tile_pitch());

        let next_tile_left = 5 * home_tile_pitch();
        assert_eq!(scroll_x + HOME_LIST_VISIBLE_W - next_tile_left, half_tile);

        keep_home_visible(9, &mut scroll_x, 10);
        assert_eq!(scroll_x, home_max_scroll(10));

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
    fn launcher_enters_arcade_when_summary_projection_has_no_game_rows() {
        let catalog = arcade_catalog(vec![], vec![arcade_system("arcade", 911)]);
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);

        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());

        assert_eq!(nav.screen, Screen::Arcade);
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
        assert!(nav.arcade_search.preparing);
        assert_eq!(nav.arcade_search.pane, ArcadeSearchPane::Keyboard);
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 0);

        assert!(catalog.ensure_text_indexes_ready());
        nav.refresh_arcade_search_if_active(&catalog, "arcade");

        assert!(!nav.arcade_search.preparing);
        assert_eq!(nav.active_arcade_game_count(&catalog, "arcade"), 1);
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
                    .category("Fighter / 2D")
                    .build(),
                arcade_game("Street Hoop")
                    .path("/media/fat/_Arcade/strhoop.mra")
                    .year(1994)
                    .manufacturer("Data East")
                    .category("Sports")
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
            filter_kind: Some("search".to_string()),
            filter_value: Some("battle".to_string()),
        };
        let mut nav = LauncherNav::new();

        assert!(!apply_launch_return_state(
            &mut nav,
            &catalog,
            state.clone()
        ));
        assert!(nav.arcade_search.preparing);
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
            (
                ArcadeFilterLevel::Categories,
                4usize,
                ArcadeFilter::Category("Fighter / 2D".to_string()),
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
            assert_eq!(nav.arcade_filter.level, ArcadeFilterLevel::Alphabet);
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
                ArcadeFilter::Category("Shooter / Vertical".to_string()),
                ArcadeFilterLevel::Categories,
                "Shooter / Vertical",
                4,
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

        assert!(nav
            .handle_input(&PadState::default(), Instant::now(), &catalog)
            .is_none());

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
    fn hierarchy_catalog_shrink_and_empty_catalog_return_to_a_valid_root() {
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
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
        assert_eq!(nav.current_menu_count(), 0);
        assert!(nav.active_collection().is_none());
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
    fn settings_b_returns_to_originating_menu_while_home_returns_root() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("menu:consoles:nintendo"));
        nav.settings_focused = true;

        let _ = nav.handle_input(&pad_with(|pad| pad.btn_a = true), t0, &catalog);
        assert_eq!(nav.screen, Screen::Settings);
        release(&mut nav, &catalog, t0, 16);
        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_b = true),
            t0 + Duration::from_millis(32),
            &catalog,
        );
        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.current_menu_id(), "menu:consoles:nintendo");
        release(&mut nav, &catalog, t0, 48);

        let _ = nav.handle_input(
            &pad_with(|pad| pad.btn_home = true),
            t0 + Duration::from_millis(64),
            &catalog,
        );
        assert_eq!(nav.current_menu_id(), ROOT_MENU_ID);
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
    fn launcher_settings_opens_and_navigates_licenses() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Settings;
        nav.settings_selected = 4;
        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert_eq!(nav.screen, Screen::Licenses);
        release(&mut nav, &catalog, t0, 16);
        let down = pad_with(|pad| pad.dpad_down = true);
        assert!(nav
            .handle_input(&down, t0 + Duration::from_millis(32), &catalog)
            .is_none());
        assert_eq!(nav.licenses_selected, 1);
        release(&mut nav, &catalog, t0, 48);
        assert!(nav
            .handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
            .is_none());
        assert!(nav.licenses_expanded);
        release(&mut nav, &catalog, t0, 80);
        assert!(nav
            .handle_input(&down, t0 + Duration::from_millis(96), &catalog)
            .is_none());
        assert_eq!(nav.licenses_scroll.selected, 3);
        assert!(nav.licenses_scroll_active());
        assert_eq!(
            nav.licenses_scroll.scroll_animation.configuration(),
            SpringConfiguration::smooth()
        );
        release(&mut nav, &catalog, t0, 112);
        assert!(nav.licenses_scroll.scroll_animation.value() > 0.0);
        let back = pad_with(|pad| pad.btn_b = true);
        assert!(nav
            .handle_input(&back, t0 + Duration::from_millis(128), &catalog)
            .is_none());
        assert!(!nav.licenses_expanded);
        assert_eq!(nav.licenses_scroll.selected, 0);
        release(&mut nav, &catalog, t0, 144);
        assert!(nav
            .handle_input(&back, t0 + Duration::from_millis(160), &catalog)
            .is_none());
        assert_eq!(nav.screen, Screen::Settings);
    }

    #[test]
    fn launcher_settings_opens_about_and_info_and_b_returns() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let press_b = pad_with(|pad| pad.btn_b = true);

        nav.screen = Screen::Settings;
        nav.settings_selected = 3;
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert_eq!(nav.screen, Screen::About);
        release(&mut nav, &catalog, t0, 16);
        assert!(nav
            .handle_input(&press_b, t0 + Duration::from_millis(32), &catalog)
            .is_none());
        assert_eq!(nav.screen, Screen::Settings);
        release(&mut nav, &catalog, t0, 48);

        nav.settings_selected = 5;
        assert!(nav
            .handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
            .is_none());
        assert_eq!(nav.screen, Screen::Info);
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

        assert!(nav
            .handle_input(
                &down,
                t0 + ARCADE_QUICK_TAP_MAX + Duration::from_millis(1),
                &catalog,
            )
            .is_none());
        assert!(nav.licenses_scroll.scroll.continuous_active);
        assert!(nav.licenses_scroll.scroll_animation.velocity() > 0.0);
        assert_eq!(
            nav.licenses_scroll.row_height,
            LICENSE_SCROLL_LINE_PX as i32
        );
        assert_eq!(nav.licenses_scroll.step_rows, 3);

        let release_at = t0 + ARCADE_QUICK_TAP_MAX + Duration::from_millis(17);
        assert!(nav
            .handle_input(&PadState::default(), release_at, &catalog)
            .is_none());
        assert!(!nav.licenses_scroll.scroll.continuous_active);
        for frame in 1..=60 {
            assert!(nav
                .handle_input(
                    &PadState::default(),
                    release_at + Duration::from_millis(frame * 16),
                    &catalog,
                )
                .is_none());
        }
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
        assert!(nav
            .handle_input(&up, t0 + Duration::from_millis(32), &catalog)
            .is_none());
        assert_eq!(nav.licenses_scroll.selected, 0);
        release(&mut nav, &catalog, t0, 48);
        assert!(nav
            .handle_input(&up, t0 + Duration::from_millis(64), &catalog)
            .is_none());
        assert_eq!(nav.licenses_scroll.selected, 0);

        let count = crate::licenses::max_scroll_line(nav.licenses_selected) + 1;
        nav.licenses_scroll.selected = count - 1;
        nav.licenses_scroll.snap_to_selected();
        release(&mut nav, &catalog, t0, 80);
        assert!(nav
            .handle_input(&down, t0 + Duration::from_millis(96), &catalog)
            .is_none());
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
        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog
            )
            .is_none());

        nav.arcade.selected = 3;
        nav.arcade.snap_to_selected();
        assert!(nav
            .handle_input(&back, t0 + Duration::from_millis(32), &catalog)
            .is_none());
        assert_eq!(nav.screen, Screen::Home);
        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(48),
                &catalog
            )
            .is_none());

        assert!(nav
            .handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
            .is_none());
        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(nav.arcade.selected, 3);
        assert_eq!(nav.arcade.scroll_y, 3 * ARCADE_ROW_HEIGHT);
    }

    #[test]
    fn amiga_opens_with_games_category_instead_of_games_and_demos() {
        let catalog = amiga_games_and_demos_catalog();
        let mut nav = LauncherNav::new();

        assert!(nav.open_system(&catalog, "amiga"));

        assert_eq!(nav.screen, Screen::Arcade);
        assert_eq!(
            nav.arcade_filter.active,
            ArcadeFilter::Category("Games".to_string())
        );
        let visible = nav.active_arcade_game_view(&catalog, "amiga");
        assert_eq!(visible.len(), 2);
        assert!(visible.iter().all(|game| game.category.as_ref() == "Games"));
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
    fn launch_return_state_falls_back_to_clamped_indices() {
        let state = LaunchReturnState {
            schema_version: LAUNCH_RETURN_STATE_SCHEMA,
            screen: "arcade".into(),
            system_id: "missing-system".into(),
            system_index: 99,
            collection_id: None,
            menu_path: Vec::new(),
            game_path: "/missing.mra".into(),
            game_index: 99,
            filter_kind: Some("all".into()),
            filter_value: None,
        };
        let catalog = reordered_arcade_catalog();
        let mut restored = LauncherNav::new();

        assert!(apply_launch_return_state(&mut restored, &catalog, state));

        assert_eq!(restored.screen, Screen::Arcade);
        assert_eq!(restored.selected, 0);
        assert_eq!(restored.arcade.selected, 2);
        assert_eq!(restored.arcade.scroll_y, 2 * ARCADE_ROW_HEIGHT);
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
            filter_kind: Some("all".into()),
            filter_value: None,
        };

        save_launch_return_state_at(&path, &state).expect("save return state");
        assert_eq!(take_launch_return_state_at(&path), Some(state));
        assert!(!path.exists());

        std::fs::write(&path, "{not-json").expect("write invalid state");
        assert_eq!(take_launch_return_state_at(&path), None);
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
            filter_kind: Some("all".into()),
            filter_value: None,
        };

        save_launch_return_state_at(&path, &state).expect("save return state");
        assert!(path.exists());
        remove_launch_return_state_at(&path);

        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launcher_confirm_defaults_cancel_destructive_actions_until_confirmed() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Settings;
        nav.settings_selected = 2;

        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert_eq!(nav.confirm_action, Some(ConfirmAction::ResetDatabase));
        assert_eq!(nav.confirm_selected, 0);
        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog,
            )
            .is_none());

        let right = pad_with(|pad| pad.dpad_right = true);
        assert!(nav
            .handle_input(&right, t0 + Duration::from_millis(32), &catalog)
            .is_none());
        assert_eq!(nav.confirm_selected, 1);
        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(48),
                &catalog,
            )
            .is_none());

        let event = nav
            .handle_input(&press_a, t0 + Duration::from_millis(64), &catalog)
            .expect("confirmed reset should emit event");
        assert_eq!(event.action, LauncherAction::ResetDatabase);
        assert_eq!(event.path, None);
        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.confirm_selected, 0);
    }

    #[test]
    fn launcher_exit_confirmation_defaults_to_cancel() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.screen = Screen::Settings;
        nav.settings_selected = 0;

        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
        assert_eq!(nav.confirm_action, Some(ConfirmAction::ExitToMister));
        assert_eq!(nav.confirm_selected, 1);
        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog,
            )
            .is_none());

        assert!(nav
            .handle_input(&press_a, t0 + Duration::from_millis(32), &catalog)
            .is_none());
        assert_eq!(nav.confirm_action, None);
        assert_eq!(nav.confirm_selected, 0);
    }

    #[test]
    fn home_closes_confirmation_and_returns_to_hierarchy_root() {
        let catalog = hierarchy_catalog();
        let mut nav = LauncherNav::new();
        nav.sync_launcher_taxonomy(&catalog);
        assert!(nav.open_menu("menu:consoles:nintendo"));
        nav.screen = Screen::Settings;
        nav.confirm_action = Some(ConfirmAction::ResetDatabase);

        assert!(nav
            .handle_input(
                &pad_with(|pad| pad.btn_home = true),
                Instant::now(),
                &catalog,
            )
            .is_none());

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
        request_library_rebuild_on_next_boot_at(&path).expect("write marker");
        assert!(path.exists());
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

        let removed = delete_screenshot_packs_at(&root).expect("delete screenshot packs");

        assert_eq!(removed, 4);
        assert!(!root.join("arcade-screenshots-320x320.mmlz4b").exists());
        assert!(!root.join("neogeo-screenshots.mmlz4b").exists());
        assert!(!root.join("saturn-screenshots-240x240.mmlz4b.tmp").exists());
        assert!(!root.join(STATE_FILENAME).exists());
        assert!(root.join("pcengine-screenshots.mmlz4b").exists());
        assert!(root.join("arcade-screenshots-large.mmlz4b").exists());
        assert!(root.join("manual.pdf").exists());
        assert!(root.join("arcade-screenshots-320x320.mmlz4b.dir").exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn library_update_failed_confirmation_dismisses_without_action() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        nav.confirm_action = Some(ConfirmAction::LibraryUpdateFailed);

        assert!(nav
            .handle_input(&pad_with(|pad| pad.btn_a = true), t0, &catalog)
            .is_none());
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

        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog
            )
            .is_none());
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
            vec!["mister_magik_launch /media/fat/_Arcade/test.mra\n"]
        );
        assert_eq!(err.to_string(), "fifo write failed");
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
        assert_eq!(io.commands, vec!["load_core /media/fat/_Arcade/test.mra\n"]);
        assert!(launch_in_progress());
        reset_launch();
    }

    #[test]
    fn reboot_mister_requests_supervised_main_reboot() {
        let mut io = launch_io();

        reboot_mister_with(&mut io).unwrap();

        assert_eq!(io.commands, vec!["mister_magik_reboot\n"]);
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

        assert!(err.contains(CMD_FIFO));
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
            vec![
                "mister_magik_launch_plan_v1 schema=1&launch_ref=magik-plan:test%20game&title=Test%20Game&system_id=neogeo&core_path=NeoGeo&payload_path=/media/fat/games/NEOGEO/Test%20Game.neo&mount_kind=mount-image&mount_index=0&delay_secs=1\n"
            ]
        );
        reset_launch();
    }

    #[test]
    fn magik_launch_requires_post_write_handoff_ack() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.handoff_ack = false;

        let err = execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect_err("missing Main acknowledgement should fail launch");

        assert_eq!(
            io.commands,
            vec!["mister_magik_launch /media/fat/_Arcade/test.mra\n"]
        );
        assert!(err
            .to_string()
            .contains("MiSTer_MagiK launch acknowledgement"));
        assert_eq!(io.input_policy_markers, vec![false, false]);
        assert!(!launch_in_progress());
    }

    #[test]
    fn stock_main_launch_does_not_wait_for_magik_ack() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.magik_running = false;
        io.handoff_ack = false;

        execute_game_launch_with(&path_target("/media/fat/_Arcade/test.mra"), &mut io)
            .expect("stock Main launch does not use MagiK status ack");

        assert_eq!(io.commands, vec!["load_core /media/fat/_Arcade/test.mra\n"]);
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
    fn magik_main_status_ack_accepts_known_handoff_states_only() {
        let handoff = magik_main_status_snapshot_from_text(
            r#"{"ts_boot_ms":10,"launcher_state":"HandoffToGame"}"#,
        )
        .expect("parse handoff status");
        assert!(handoff.handoff_acknowledged);

        let unconfigured = magik_main_status_snapshot_from_text(
            r#"{"ts_boot_ms":11,"launcher_state":"Unconfigured"}"#,
        )
        .expect("parse unconfigured status");
        assert!(unconfigured.handoff_acknowledged);

        let active = magik_main_status_snapshot_from_text(
            r#"{"ts_boot_ms":12,"launcher_state":"LauncherActive"}"#,
        )
        .expect("parse active status");
        assert!(!active.handoff_acknowledged);

        let crashed = magik_main_status_snapshot_from_text(
            r#"{"ts_boot_ms":13,"launcher_state":"LauncherCrashed"}"#,
        )
        .expect("parse crash status");
        assert!(!crashed.handoff_acknowledged);

        assert!(magik_main_status_snapshot_from_text("{}").is_none());
    }

    #[test]
    fn magik_main_status_ack_requires_newer_status_timestamp() {
        let before = magik_main_status_snapshot_from_text(
            r#"{"ts_boot_ms":42,"launcher_state":"LauncherActive"}"#,
        )
        .expect("parse before status");
        let stale = magik_main_status_snapshot_from_text(
            r#"{"ts_boot_ms":42,"launcher_state":"HandoffToGame"}"#,
        )
        .expect("parse stale handoff status");
        let fresh = magik_main_status_snapshot_from_text(
            r#"{"ts_boot_ms":43,"launcher_state":"HandoffToGame"}"#,
        )
        .expect("parse fresh handoff status");

        assert!(stale.handoff_acknowledged);
        assert!(!magik_handoff_ack_is_newer(Some(before), stale));
        assert!(magik_handoff_ack_is_newer(Some(before), fresh));
        assert!(magik_handoff_ack_is_newer(None, fresh));
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
}
