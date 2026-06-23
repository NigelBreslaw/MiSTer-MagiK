//! Launcher navigation and arcade game launch.

use crate::arcade_catalog::{
    ArcadeCatalog, ARCADE_ROW_HEIGHT, HOME_LIST_VISIBLE_W, HOME_TILE_GAP, HOME_TILE_WIDTH,
};
use crate::input_repeat::RepeatNav;
use crate::input_state::PadState;
use crate::launch_preparation;
use crate::library_db;
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

const CMD_FIFO: &str = "/dev/MiSTer_cmd";
const MISTER_BIN: &str = "/media/fat/MiSTer_MagiK";
const MISTER_PROCESS_NAMES: &[&str] = &["MiSTer_MagiK", "MiSTer"];
pub const LIBRARY_REBUILD_ON_NEXT_BOOT_PATH: &str = "/media/fat/mister-magik/rebuild-on-next-boot";
#[cfg(test)]
const STATE_FILENAME: &str = mister_magik_catalog::media_identity::SCREENSHOT_MEDIA_STATE_FILENAME;
const ARCADE_NORMAL_PX_PER_FRAME: i32 = 6;
const ARCADE_TURBO_PX_PER_FRAME: i32 = 12;
const ARCADE_QUICK_TAP_MAX: Duration = Duration::from_millis(220);
const ARCADE_TURBO_REPRESS_WINDOW: Duration = Duration::from_millis(350);
const FIFO_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const FIFO_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const MISTER_START_TIMEOUT: Duration = Duration::from_secs(15);
pub const LAUNCH_RETURN_STATE_PATH: &str = "/tmp/mister-magik/launcher-return-state.json";
const LAUNCH_RETURN_STATE_SCHEMA: u32 = 1;

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
pub enum LibraryChangedTestAction {
    Continue,
    Rebuild,
}

impl LibraryChangedTestAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Rebuild => "rebuild",
        }
    }
}

pub fn parse_library_changed_test_action(
    value: &str,
) -> Result<Option<LibraryChangedTestAction>, String> {
    match value.trim() {
        "" => Ok(None),
        "continue" => Ok(Some(LibraryChangedTestAction::Continue)),
        "rebuild" => Ok(Some(LibraryChangedTestAction::Rebuild)),
        other => Err(format!(
            "unknown MISTER_MAGIK_TEST_LIBRARY_CHANGED_ACTION={other:?}; use continue|rebuild"
        )),
    }
}

pub fn library_changed_test_action_event(
    confirm_action: Option<ConfirmAction>,
    action: LibraryChangedTestAction,
) -> Option<LauncherEvent> {
    if confirm_action != Some(ConfirmAction::LibraryChanged) {
        return None;
    }
    Some(LauncherEvent {
        action: match action {
            LibraryChangedTestAction::Continue => LauncherAction::ContinueWithStaleLibrary,
            LibraryChangedTestAction::Rebuild => LauncherAction::RebuildLibrary,
        },
        path: None,
    })
}

pub struct ArcadeNav {
    pub selected: usize,
    pub scroll_y: i32,
    pub visual_index: f32,
    scroll: ArcadeScrollState,
}

#[derive(Clone, Copy, Debug, Default)]
struct ArcadeScrollState {
    visual_px: i32,
    target_index: usize,
    intent_queue: i32,
    held_dir: i32,
    hold_started_at: Option<Instant>,
    last_quick_tap_dir: i32,
    last_quick_tap_released_at: Option<Instant>,
    turbo_candidate: bool,
    turbo_active: bool,
}

impl Default for ArcadeNav {
    fn default() -> Self {
        Self::new()
    }
}

impl ArcadeNav {
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll_y: 0,
            visual_index: 0.0,
            scroll: ArcadeScrollState::default(),
        }
    }

    pub fn reset(&mut self) {
        self.selected = 0;
        self.scroll_y = 0;
        self.visual_index = 0.0;
        self.scroll = ArcadeScrollState::default();
    }

    pub fn snap_to_selected(&mut self) {
        self.scroll.target_index = self.selected;
        self.scroll.intent_queue = 0;
        self.scroll.visual_px = self.selected as i32 * ARCADE_ROW_HEIGHT;
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
        self.scroll.visual_px = scroll_y.clamp(0, Self::max_scroll_y(count));
        self.sync_visual_from_px();
    }

    fn max_scroll_y(count: usize) -> i32 {
        count.saturating_sub(1) as i32 * ARCADE_ROW_HEIGHT
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
            self.record_release(previous_dir, now);
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

    pub fn tick(&mut self, count: usize) {
        if count == 0 {
            self.reset();
            return;
        }
        if self.selected >= count {
            self.selected = count - 1;
            self.snap_to_selected();
            return;
        }
        let target_px = self.scroll.target_index as i32 * ARCADE_ROW_HEIGHT;
        let delta = target_px - self.scroll.visual_px;
        if delta == 0 {
            self.scroll.intent_queue = 0;
            self.sync_visual_from_px();
            return;
        }
        let step = if self.scroll.turbo_active {
            ARCADE_TURBO_PX_PER_FRAME
        } else {
            ARCADE_NORMAL_PX_PER_FRAME
        }
        .saturating_div(arcade_scroll_speed_div())
        .max(1);
        let movement = delta.signum() * step.min(delta.abs());
        let before_row = self.scroll.visual_px.div_euclid(ARCADE_ROW_HEIGHT);
        self.scroll.visual_px =
            (self.scroll.visual_px + movement).clamp(0, Self::max_scroll_y(count));
        let after_row = self.scroll.visual_px.div_euclid(ARCADE_ROW_HEIGHT);
        if after_row != before_row && self.scroll.intent_queue != 0 {
            self.scroll.intent_queue -= self.scroll.intent_queue.signum();
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
        self.tick(count);
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
            self.tick(count);
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
        self.tick(count);
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

    fn record_release(&mut self, dir: i32, now: Instant) {
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
    }

    pub fn is_settled(&self) -> bool {
        self.scroll.visual_px == self.scroll.target_index as i32 * ARCADE_ROW_HEIGHT
    }

    #[cfg(test)]
    pub fn is_scroll_active(&self) -> bool {
        !self.is_settled() || self.scroll.held_dir != 0 || self.scroll.intent_queue != 0
    }

    #[cfg(test)]
    pub fn has_scroll_motion_or_queue(&self) -> bool {
        !self.is_settled() || self.scroll.intent_queue != 0
    }

    fn enqueue_step(&mut self, dir: i32, count: usize) {
        if count == 0 || dir == 0 {
            return;
        }
        let next = if dir > 0 {
            self.scroll.target_index.saturating_add(1).min(count - 1)
        } else {
            self.scroll.target_index.saturating_sub(1)
        };
        if next == self.scroll.target_index {
            return;
        }
        self.scroll.target_index = next;
        self.selected = next;
        self.scroll.intent_queue += dir.signum();
    }

    fn sync_visual_from_px(&mut self) {
        self.scroll_y = self.scroll.visual_px;
        self.visual_index = self.scroll.visual_px as f32 / ARCADE_ROW_HEIGHT as f32;
    }
}

pub struct LauncherNav {
    pub screen: Screen,
    pub selected: usize,
    pub scroll_x: i32,
    pub settings_focused: bool,
    pub settings_selected: usize,
    pub confirm_action: Option<ConfirmAction>,
    pub confirm_selected: usize,
    pub arcade: ArcadeNav,
    game_list_memory: HashMap<String, GameListMemory>,
    repeat: RepeatNav,
    prev: PadState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GameListMemory {
    selected: usize,
    scroll_y: i32,
}

impl Default for LauncherNav {
    fn default() -> Self {
        Self::new()
    }
}

impl LauncherNav {
    pub fn new() -> Self {
        Self {
            screen: Screen::Home,
            selected: 0,
            scroll_x: 0,
            settings_focused: false,
            settings_selected: 0,
            confirm_action: None,
            confirm_selected: 0,
            arcade: ArcadeNav::new(),
            game_list_memory: HashMap::new(),
            repeat: RepeatNav::default(),
            prev: PadState::default(),
        }
    }

    /// Returns an event when a launch or system action was requested.
    pub fn handle_input(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        let result = if self.confirm_action.is_some() {
            self.handle_confirm(now, frame_now)
        } else {
            match self.screen {
                Screen::Home => self.handle_home(now, frame_now, catalog),
                Screen::Controller => {
                    if rising(now.btn_home, self.prev.btn_home)
                        || rising(now.btn_b, self.prev.btn_b)
                    {
                        self.screen = Screen::Home;
                    }
                    None
                }
                Screen::Arcade => self.handle_arcade(now, frame_now, catalog),
                Screen::Settings => self.handle_settings(now, frame_now),
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
        let system_count = catalog.systems.len();
        if self.repeat.tick_up(now.dpad_up, frame_now) {
            self.settings_focused = true;
        }
        if self.repeat.tick_down(now.dpad_down, frame_now) {
            self.settings_focused = false;
        }
        if self.settings_focused {
            if rising(now.btn_a, self.prev.btn_a) {
                self.settings_selected = 0;
                self.screen = Screen::Settings;
            }
            return None;
        }

        if system_count == 0 {
            return None;
        }

        if self.selected >= system_count {
            self.selected = system_count - 1;
            keep_home_visible(self.selected, &mut self.scroll_x, system_count);
        }
        if self.repeat.tick_left(now.dpad_left, frame_now) && self.selected > 0 {
            self.selected -= 1;
            keep_home_visible(self.selected, &mut self.scroll_x, system_count);
        }
        if self.repeat.tick_right(now.dpad_right, frame_now) && self.selected + 1 < system_count {
            self.selected += 1;
            keep_home_visible(self.selected, &mut self.scroll_x, system_count);
        }

        if rising(now.btn_a, self.prev.btn_a) {
            if let Some(system) = catalog.systems.get(self.selected) {
                let count = catalog.system_game_count(&system.id);
                self.restore_game_list_state(&system.id, count);
            } else {
                self.arcade.reset();
            }
            self.screen = Screen::Arcade;
        }

        None
    }

    fn handle_arcade(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<LauncherEvent> {
        let system_id = catalog
            .systems
            .get(self.selected)
            .map(|system| system.id.as_str())
            .unwrap_or("");
        let count = catalog.system_game_count(system_id);

        if rising(now.btn_home, self.prev.btn_home) || rising(now.btn_b, self.prev.btn_b) {
            if !system_id.is_empty() {
                self.save_game_list_state(system_id);
            }
            self.screen = Screen::Home;
            return None;
        }

        if count == 0 {
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
        self.arcade.tick(count);

        if rising(now.btn_a, self.prev.btn_a) {
            return catalog
                .system_game_at(system_id, self.arcade.selected)
                .map(|game| LauncherEvent {
                    action: LauncherAction::LaunchGame,
                    path: Some(game.mra_path.to_string()),
                });
        }

        None
    }

    fn handle_settings(&mut self, now: &PadState, frame_now: Instant) -> Option<LauncherEvent> {
        if rising(now.btn_home, self.prev.btn_home) || rising(now.btn_b, self.prev.btn_b) {
            self.screen = Screen::Home;
            return None;
        }
        if self.repeat.tick_down(now.dpad_down, frame_now) && self.settings_selected < 3 {
            self.settings_selected += 1;
        }
        if self.repeat.tick_up(now.dpad_up, frame_now) && self.settings_selected > 0 {
            self.settings_selected -= 1;
        }
        if rising(now.btn_a, self.prev.btn_a) {
            if self.settings_selected == 1 {
                self.screen = Screen::Controller;
                return None;
            }
            self.confirm_selected = if self.settings_selected == 0 { 1 } else { 0 };
            self.confirm_action = Some(match self.settings_selected {
                0 => ConfirmAction::ExitToMister,
                2 => ConfirmAction::ResetDatabase,
                _ => ConfirmAction::Restart,
            });
        }
        None
    }

    fn handle_confirm(&mut self, now: &PadState, frame_now: Instant) -> Option<LauncherEvent> {
        if rising(now.btn_b, self.prev.btn_b) || rising(now.btn_home, self.prev.btn_home) {
            if self.confirm_action == Some(ConfirmAction::LibraryChanged) {
                self.confirm_action = None;
                self.confirm_selected = 0;
                return Some(LauncherEvent {
                    action: LauncherAction::ContinueWithStaleLibrary,
                    path: None,
                });
            }
            self.confirm_action = None;
            self.confirm_selected = 0;
            return None;
        }
        if self.repeat.tick_left(now.dpad_left, frame_now) && self.confirm_selected > 0 {
            self.confirm_selected -= 1;
        }
        if self.repeat.tick_right(now.dpad_right, frame_now) && self.confirm_selected < 1 {
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
        self.game_list_memory.insert(
            system_id.to_string(),
            GameListMemory {
                selected: self.arcade.selected,
                scroll_y: self.arcade.scroll_y,
            },
        );
    }

    fn restore_game_list_state(&mut self, system_id: &str, count: usize) {
        if let Some(memory) = self.game_list_memory.get(system_id).copied() {
            self.arcade
                .restore_position(memory.selected, memory.scroll_y, count);
        } else {
            self.arcade.reset();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchReturnState {
    schema_version: u32,
    screen: String,
    system_id: String,
    system_index: usize,
    game_path: String,
    game_index: usize,
}

pub fn capture_launch_return_state(
    nav: &LauncherNav,
    catalog: &ArcadeCatalog,
    game_path: &str,
) -> Option<LaunchReturnState> {
    if nav.screen != Screen::Arcade {
        return None;
    }
    let system = catalog.systems.get(nav.selected)?;
    let games = catalog.system_game_slice(&system.id);
    let game_index = games
        .iter()
        .position(|game| game.mra_path.as_ref() == game_path)
        .unwrap_or(nav.arcade.selected.min(games.len().saturating_sub(1)));
    Some(LaunchReturnState {
        schema_version: LAUNCH_RETURN_STATE_SCHEMA,
        screen: "arcade".to_string(),
        system_id: system.id.clone(),
        system_index: nav.selected,
        game_path: game_path.to_string(),
        game_index,
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
        Err(e) => eprintln!(
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
            if state.schema_version == LAUNCH_RETURN_STATE_SCHEMA && state.screen == "arcade" =>
        {
            Some(state)
        }
        Ok(_) => None,
        Err(e) => {
            eprintln!("invalid launch return state {}: {e}", path.display());
            None
        }
    }
}

pub fn apply_launch_return_state(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    state: LaunchReturnState,
) -> bool {
    let Some(system_index) = resolve_system_index(catalog, &state) else {
        return false;
    };
    let system_id = &catalog.systems[system_index].id;
    let games = catalog.system_game_slice(system_id);
    if games.is_empty() {
        return false;
    }
    let game_index = games
        .iter()
        .position(|game| game.mra_path.as_ref() == state.game_path)
        .unwrap_or_else(|| state.game_index.min(games.len() - 1));

    nav.selected = system_index;
    nav.screen = Screen::Arcade;
    nav.arcade.restore_position(
        game_index,
        game_index as i32 * ARCADE_ROW_HEIGHT,
        games.len(),
    );
    true
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

fn keep_home_visible(selected: usize, scroll_x: &mut i32, count: usize) {
    let tile_left = selected as i32 * (HOME_TILE_WIDTH + HOME_TILE_GAP);
    let tile_right = tile_left + HOME_TILE_WIDTH;
    if tile_left < *scroll_x {
        *scroll_x = tile_left;
    }
    if tile_right > *scroll_x + HOME_LIST_VISIBLE_W {
        *scroll_x = tile_right - HOME_LIST_VISIBLE_W;
    }
    *scroll_x = (*scroll_x).clamp(0, home_max_scroll(count));
}

fn rising(now: bool, prev: bool) -> bool {
    now && !prev
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
    fn prepare_launch_ref(&mut self, launch_ref: &str) -> Result<String, String>;
    fn target_exists(&mut self, path: &str) -> bool;
    fn mister_running(&mut self) -> bool;
    fn magik_running(&mut self) -> bool;
    fn start_mister(&mut self) -> Result<(), String>;
    fn wait_for_started_mister(&mut self) -> bool;
    fn wait_for_command_fifo(&mut self) -> bool;
    fn write_mister_command(&mut self, cmd: &str) -> Result<(), String>;
}

struct SystemLaunchIo;

impl LaunchIo for SystemLaunchIo {
    fn prepare_launch_ref(&mut self, launch_ref: &str) -> Result<String, String> {
        launch_preparation::prepare_launch_ref(launch_ref)
    }

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

    fn write_mister_command(&mut self, cmd: &str) -> Result<(), String> {
        write_mister_command_nonblocking(cmd)
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
    eprintln!(
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

/// Launch via fifo. Prefer the Magik-aware Main command when the fork owns the device.
/// Returns `true` if Main was spawned for this launch (caller should stop it on failure).
pub fn execute_game_launch(launch_ref: &str) -> Result<bool, LaunchError> {
    let mut io = SystemLaunchIo;
    execute_game_launch_with(launch_ref, &mut io)
}

fn execute_game_launch_with(launch_ref: &str, io: &mut impl LaunchIo) -> Result<bool, LaunchError> {
    let launch_target = io
        .prepare_launch_ref(launch_ref)
        .map_err(|e| LaunchError::new(e, false))?;
    if !io.target_exists(&launch_target) {
        return Err(LaunchError::new(
            format!("launch target not found: {launch_target}"),
            false,
        ));
    }

    let spawned = if io.mister_running() {
        false
    } else {
        println!("launch: starting {MISTER_BIN} for load_core");
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

    let cmd = if io.magik_running() {
        format!("mister_magik_launch {launch_target}\n")
    } else {
        format!("load_core {launch_target}\n")
    };
    println!("launch: {}", cmd.trim_end());
    io.write_mister_command(&cmd)
        .map_err(|e| LaunchError::new(e, spawned))?;

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
    fs::write(path, b"rebuild\n").map_err(|e| format!("write rebuild marker: {e}"))
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
    use crate::arcade_catalog::{ArcadeGameEntry, GameSystemEntry};
    use std::sync::Mutex;

    static LAUNCH_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct FakeLaunchIo {
        target_exists: bool,
        mister_running: bool,
        magik_running: bool,
        start_result: Result<(), String>,
        started_ready: bool,
        fifo_ready: bool,
        write_result: Result<(), String>,
        prepared_launch_ref: Option<String>,
        prepare_result: Result<(), String>,
        start_calls: usize,
        commands: Vec<String>,
    }

    impl LaunchIo for FakeLaunchIo {
        fn prepare_launch_ref(&mut self, launch_ref: &str) -> Result<String, String> {
            self.prepared_launch_ref = Some(launch_ref.to_string());
            self.prepare_result.clone().map(|_| {
                if launch_ref.starts_with("magik-plan:") {
                    "/tmp/mister-magik-virtual-test.mgl".to_string()
                } else {
                    launch_ref.to_string()
                }
            })
        }

        fn target_exists(&mut self, _path: &str) -> bool {
            self.target_exists
        }

        fn mister_running(&mut self) -> bool {
            self.mister_running
        }

        fn magik_running(&mut self) -> bool {
            self.magik_running
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

        fn write_mister_command(&mut self, cmd: &str) -> Result<(), String> {
            self.commands.push(cmd.to_string());
            self.write_result.clone()
        }
    }

    fn launch_io() -> FakeLaunchIo {
        FakeLaunchIo {
            target_exists: true,
            mister_running: true,
            magik_running: true,
            start_result: Ok(()),
            started_ready: true,
            fifo_ready: true,
            write_result: Ok(()),
            prepared_launch_ref: None,
            prepare_result: Ok(()),
            start_calls: 0,
            commands: Vec::new(),
        }
    }

    fn input(nav: &mut ArcadeNav, dir: i32, previous_dir: i32, count: usize, now: Instant) {
        nav.handle_direction_input(dir, previous_dir, now, count);
        nav.tick(count);
    }

    fn settle(nav: &mut ArcadeNav, count: usize, start: Instant) {
        for frame in 1..=64 {
            nav.tick(count);
            if nav.is_settled() {
                return;
            }
            let _ = start + Duration::from_millis(frame * 16);
        }
        assert!(nav.is_settled(), "arcade nav did not settle");
    }

    fn image_less_amiga_catalog() -> ArcadeCatalog {
        ArcadeCatalog::new(
            Path::new("/media/fat/_Arcade").to_path_buf(),
            vec![ArcadeGameEntry {
                title: "Agony".into(),
                mra_path: "magik-plan:amiga-agony".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "amiga".into(),
                is_new: false,
            }],
            vec![GameSystemEntry {
                id: "amiga".into(),
                title: "Amiga".into(),
                count: 1,
            }],
        )
    }

    fn multi_system_catalog() -> ArcadeCatalog {
        ArcadeCatalog::new(
            Path::new("/media/fat/_Arcade").to_path_buf(),
            vec![
                ArcadeGameEntry {
                    title: "1942".into(),
                    mra_path: "/media/fat/_Arcade/1942.mra".into(),
                    preview_archive_path:
                        "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b".into(),
                    preview_asset_key: "1942".into(),
                    has_preview: true,
                    system_id: "arcade".into(),
                    is_new: false,
                },
                ArcadeGameEntry {
                    title: "Agony".into(),
                    mra_path: "magik-plan:amiga-agony".into(),
                    preview_archive_path: "".into(),
                    preview_asset_key: "".into(),
                    has_preview: false,
                    system_id: "amiga".into(),
                    is_new: false,
                },
            ],
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1,
                },
                GameSystemEntry {
                    id: "amiga".into(),
                    title: "Amiga".into(),
                    count: 1,
                },
            ],
        )
    }

    fn multi_game_catalog() -> ArcadeCatalog {
        let mut games = Vec::new();
        for i in 0..5 {
            games.push(ArcadeGameEntry {
                title: format!("Arcade {i}").into(),
                mra_path: format!("/media/fat/_Arcade/arcade-{i}.mra").into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "arcade".into(),
                is_new: false,
            });
        }
        for i in 0..3 {
            games.push(ArcadeGameEntry {
                title: format!("Amiga {i}").into(),
                mra_path: format!("magik-plan:amiga-{i}").into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "amiga".into(),
                is_new: false,
            });
        }
        ArcadeCatalog::new(
            Path::new("/media/fat/_Arcade").to_path_buf(),
            games,
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 5,
                },
                GameSystemEntry {
                    id: "amiga".into(),
                    title: "Amiga".into(),
                    count: 3,
                },
            ],
        )
    }

    fn reordered_arcade_catalog() -> ArcadeCatalog {
        ArcadeCatalog::new(
            Path::new("/media/fat/_Arcade").to_path_buf(),
            vec![
                ArcadeGameEntry {
                    title: "Arcade 4".into(),
                    mra_path: "/media/fat/_Arcade/arcade-4.mra".into(),
                    preview_archive_path: "".into(),
                    preview_asset_key: "".into(),
                    has_preview: false,
                    system_id: "arcade".into(),
                    is_new: false,
                },
                ArcadeGameEntry {
                    title: "Arcade 2".into(),
                    mra_path: "/media/fat/_Arcade/arcade-2.mra".into(),
                    preview_archive_path: "".into(),
                    preview_asset_key: "".into(),
                    has_preview: false,
                    system_id: "arcade".into(),
                    is_new: false,
                },
                ArcadeGameEntry {
                    title: "Arcade 0".into(),
                    mra_path: "/media/fat/_Arcade/arcade-0.mra".into(),
                    preview_archive_path: "".into(),
                    preview_asset_key: "".into(),
                    has_preview: false,
                    system_id: "arcade".into(),
                    is_new: false,
                },
            ],
            vec![GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 3,
            }],
        )
    }

    fn pad_with(mut set: impl FnMut(&mut PadState)) -> PadState {
        let mut pad = PadState::default();
        set(&mut pad);
        pad
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
        let catalog = ArcadeCatalog::new(
            Path::new("/media/fat/_Arcade").to_path_buf(),
            vec![],
            vec![],
        );
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);

        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());

        assert_eq!(nav.screen, Screen::Home);
        assert_eq!(nav.selected, 0);
        assert_eq!(nav.scroll_x, 0);
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
    fn launcher_settings_can_open_controller_and_return_home() {
        let catalog = multi_system_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();

        let up = pad_with(|pad| pad.dpad_up = true);
        assert!(nav.handle_input(&up, t0, &catalog).is_none());
        assert!(nav.settings_focused);
        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(16),
                &catalog,
            )
            .is_none());

        let press_a = pad_with(|pad| pad.btn_a = true);
        assert!(nav
            .handle_input(&press_a, t0 + Duration::from_millis(32), &catalog)
            .is_none());
        assert_eq!(nav.screen, Screen::Settings);
        assert_eq!(nav.settings_selected, 0);
        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(48),
                &catalog,
            )
            .is_none());

        let down = pad_with(|pad| pad.dpad_down = true);
        assert!(nav
            .handle_input(&down, t0 + Duration::from_millis(64), &catalog)
            .is_none());
        assert_eq!(nav.settings_selected, 1);
        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(80),
                &catalog,
            )
            .is_none());

        assert!(nav
            .handle_input(&press_a, t0 + Duration::from_millis(96), &catalog)
            .is_none());
        assert_eq!(nav.screen, Screen::Controller);
        assert!(nav
            .handle_input(
                &PadState::default(),
                t0 + Duration::from_millis(112),
                &catalog,
            )
            .is_none());

        let back = pad_with(|pad| pad.btn_b = true);
        assert!(nav
            .handle_input(&back, t0 + Duration::from_millis(128), &catalog)
            .is_none());
        assert_eq!(nav.screen, Screen::Home);
    }

    #[test]
    fn launcher_reopens_system_at_in_memory_arcade_position() {
        let catalog = multi_game_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();
        let press_a = pad_with(|pad| pad.btn_a = true);
        let back = pad_with(|pad| pad.btn_b = true);

        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
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
    fn launch_return_state_captures_arcade_location() {
        let catalog = multi_game_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.selected = 0;
        nav.arcade.selected = 2;
        nav.arcade.snap_to_selected();

        let state = capture_launch_return_state(&nav, &catalog, "/media/fat/_Arcade/arcade-2.mra")
            .expect("state should capture");

        assert_eq!(state.schema_version, LAUNCH_RETURN_STATE_SCHEMA);
        assert_eq!(state.screen, "arcade");
        assert_eq!(state.system_id, "arcade");
        assert_eq!(state.system_index, 0);
        assert_eq!(state.game_path, "/media/fat/_Arcade/arcade-2.mra");
        assert_eq!(state.game_index, 2);
    }

    #[test]
    fn launch_return_state_restores_by_path_after_catalog_reorder() {
        let catalog = multi_game_catalog();
        let mut nav = LauncherNav::new();
        nav.screen = Screen::Arcade;
        nav.selected = 0;
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
        assert_eq!(restored.selected, 0);
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
            game_path: "/missing.mra".into(),
            game_index: 99,
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
            game_path: "/media/fat/_Arcade/arcade-2.mra".into(),
            game_index: 2,
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
            game_path: "/media/fat/_Arcade/arcade-2.mra".into(),
            game_index: 2,
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
    fn library_changed_test_action_parser_accepts_only_continue_or_rebuild() {
        assert_eq!(
            parse_library_changed_test_action("continue").expect("parse continue"),
            Some(LibraryChangedTestAction::Continue)
        );
        assert_eq!(
            parse_library_changed_test_action("rebuild").expect("parse rebuild"),
            Some(LibraryChangedTestAction::Rebuild)
        );
        assert_eq!(
            parse_library_changed_test_action("").expect("parse empty"),
            None
        );
        assert!(parse_library_changed_test_action("reset").is_err());
    }

    #[test]
    fn library_changed_test_action_only_fires_for_library_changed_dialog() {
        assert!(library_changed_test_action_event(
            Some(ConfirmAction::ResetDatabase),
            LibraryChangedTestAction::Continue,
        )
        .is_none());
        let event = library_changed_test_action_event(
            Some(ConfirmAction::LibraryChanged),
            LibraryChangedTestAction::Rebuild,
        )
        .expect("library changed hook should emit event");
        assert_eq!(event.action, LauncherAction::RebuildLibrary);
        assert_eq!(event.path, None);
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
        assert!(nav.handle_input(&press_a, t0, &catalog).is_none());
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
        assert_eq!(nav.visual_index, 1.0);
        input(&mut nav, 1, 1, 10, t0 + Duration::from_millis(128));
        assert_eq!(nav.selected, 2);
        assert!(nav.visual_index > 1.0);
    }

    #[test]
    fn arcade_scroll_stays_active_while_direction_is_held_between_steps() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        settle(&mut nav, 10, t0);

        assert!(nav.is_settled());
        assert!(nav.is_scroll_active());

        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(140));

        assert!(!nav.is_scroll_active());
    }

    #[test]
    fn arcade_scroll_motion_is_idle_while_direction_is_held_between_steps() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);

        assert!(nav.has_scroll_motion_or_queue());

        settle(&mut nav, 10, t0);

        assert!(nav.is_settled());
        assert!(nav.is_scroll_active());
        assert!(!nav.has_scroll_motion_or_queue());
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
        settle(&mut nav, 10, t0);
        assert_eq!(nav.visual_index, 0.0);
    }

    #[test]
    fn arcade_turbo_repress_uses_12_px_steps() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(50));

        input(&mut nav, 1, 0, 10, t0 + Duration::from_millis(120));
        assert!(!nav.scroll.turbo_active);
        assert_eq!(nav.selected, 2);

        let visual_before_turbo = nav.scroll.visual_px;
        input(&mut nav, 1, 1, 10, t0 + Duration::from_millis(360));
        assert!(nav.scroll.turbo_active);
        assert_eq!(
            nav.scroll.visual_px,
            visual_before_turbo + ARCADE_TURBO_PX_PER_FRAME
        );
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
        let visual_before_repress = nav.scroll.visual_px;
        input(&mut nav, 1, 0, 10, t0 + Duration::from_millis(450));
        assert!(!nav.scroll.turbo_active);
        assert_eq!(
            nav.scroll.visual_px,
            visual_before_repress + ARCADE_NORMAL_PX_PER_FRAME
        );
    }

    #[test]
    fn arcade_opposite_repress_does_not_turbo() {
        let mut nav = ArcadeNav::new();
        nav.selected = 5;
        nav.snap_to_selected();
        let t0 = Instant::now();
        input(&mut nav, 1, 0, 10, t0);
        input(&mut nav, 0, 1, 10, t0 + Duration::from_millis(50));
        let visual_before_repress = nav.scroll.visual_px;
        input(&mut nav, -1, 0, 10, t0 + Duration::from_millis(120));
        assert!(!nav.scroll.turbo_active);
        assert_eq!(
            nav.scroll.visual_px,
            visual_before_repress - ARCADE_NORMAL_PX_PER_FRAME
        );
    }

    #[test]
    fn launch_missing_target_does_not_spawn_or_require_recovery() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();
        io.target_exists = false;

        let err = execute_game_launch_with("/missing.mra", &mut io).expect_err("launch fails");

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

        let err = execute_game_launch_with("/media/fat/_Arcade/test.mra", &mut io)
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

        let err = execute_game_launch_with("/media/fat/_Arcade/test.mra", &mut io)
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

        let spawned = execute_game_launch_with("/media/fat/_Arcade/test.mra", &mut io)
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
    fn virtual_launch_ref_is_materialized_before_fifo_command() {
        let _guard = LAUNCH_TEST_LOCK.lock().unwrap();
        reset_launch();
        let mut io = launch_io();

        let spawned = execute_game_launch_with("magik-plan:payload-saturn-test", &mut io)
            .expect("launch succeeds");

        assert!(!spawned);
        assert_eq!(
            io.prepared_launch_ref.as_deref(),
            Some("magik-plan:payload-saturn-test")
        );
        assert_eq!(
            io.commands,
            vec!["mister_magik_launch /tmp/mister-magik-virtual-test.mgl\n"]
        );
        reset_launch();
    }
}
