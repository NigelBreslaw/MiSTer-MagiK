//! Launcher navigation and arcade game launch.

use crate::arcade_catalog::{
    ArcadeCatalog, ARCADE_ROW_HEIGHT, HOME_LIST_VISIBLE_W, HOME_TILE_GAP, HOME_TILE_WIDTH,
};
use crate::input::PadState;
use crate::input_repeat::RepeatNav;
use crate::library_bench;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const CMD_FIFO: &str = "/dev/MiSTer_cmd";
const MISTER_BIN: &str = "/media/fat/MiSTer_MagiK";
const MISTER_PROCESS_NAMES: &[&str] = &["MiSTer_MagiK", "MiSTer"];
const ARCADE_ROW_STEP_PX_PER_FRAME: i32 = 6;
const ARCADE_HOLD_CONTINUOUS_DELAY: Duration = Duration::from_millis(180);

const LAUNCH_IDLE: u8 = 0;
const LAUNCH_SENT: u8 = 1;

static LAUNCH_STATE: AtomicU8 = AtomicU8::new(LAUNCH_IDLE);

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherAction {
    LaunchGame,
    ExitToMister,
    ResetDatabase,
    Restart,
}

pub struct LauncherEvent {
    pub action: LauncherAction,
    pub path: Option<String>,
}

pub struct ArcadeNav {
    pub selected: usize,
    pub scroll_y: i32,
    pub visual_index: f32,
    last_update_at: Option<Instant>,
    last_motion_dir: i32,
    hold_started_at: Option<Instant>,
    hold_dir: i32,
}

impl ArcadeNav {
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll_y: 0,
            visual_index: 0.0,
            last_update_at: None,
            last_motion_dir: 0,
            hold_started_at: None,
            hold_dir: 0,
        }
    }

    pub fn reset(&mut self) {
        self.selected = 0;
        self.scroll_y = 0;
        self.visual_index = 0.0;
        self.last_update_at = None;
        self.last_motion_dir = 0;
        self.hold_started_at = None;
        self.hold_dir = 0;
    }

    pub fn snap_to_selected(&mut self) {
        self.visual_index = self.selected as f32;
        self.last_update_at = None;
        self.last_motion_dir = 0;
        self.hold_started_at = None;
        self.hold_dir = 0;
        self.scroll_y = self.selected as i32 * ARCADE_ROW_HEIGHT;
    }

    fn max_scroll_y(count: usize) -> i32 {
        count.saturating_sub(1) as i32 * ARCADE_ROW_HEIGHT
    }

    fn set_visual_px(&mut self, px: i32, count: usize) {
        let px = px.clamp(0, Self::max_scroll_y(count));
        self.scroll_y = px;
        self.visual_index = px as f32 / ARCADE_ROW_HEIGHT as f32;
    }

    fn continuous_hold_dir(&mut self, raw_dir: i32, now: Instant) -> i32 {
        if raw_dir == 0 {
            self.hold_started_at = None;
            self.hold_dir = 0;
            return 0;
        }
        let raw_dir = raw_dir.signum();
        if self.hold_dir != raw_dir {
            self.hold_dir = raw_dir;
            self.hold_started_at = Some(now);
            return 0;
        }
        let started = self.hold_started_at.unwrap_or(now);
        self.hold_started_at = Some(started);
        if now.saturating_duration_since(started) >= ARCADE_HOLD_CONTINUOUS_DELAY {
            raw_dir
        } else {
            0
        }
    }

    fn tick_scroll(&mut self, held_dir: i32, count: usize, now: Instant) {
        if count == 0 {
            self.reset();
            return;
        }
        let max_scroll_y = Self::max_scroll_y(count);
        if self.selected >= count {
            self.selected = count - 1;
            self.snap_to_selected();
            self.last_update_at = Some(now);
            return;
        }

        self.last_update_at = Some(now);

        if held_dir != 0 {
            self.last_motion_dir = held_dir.signum();
            self.set_visual_px(
                self.scroll_y + self.last_motion_dir * ARCADE_ROW_STEP_PX_PER_FRAME,
                count,
            );
            self.selected = if self.last_motion_dir > 0 {
                ((self.scroll_y + ARCADE_ROW_HEIGHT - 1) / ARCADE_ROW_HEIGHT) as usize
            } else {
                (self.scroll_y / ARCADE_ROW_HEIGHT) as usize
            }
            .min(count - 1);

            if self.scroll_y <= 0 && held_dir < 0 {
                self.selected = 0;
                self.last_update_at = None;
                self.last_motion_dir = 0;
            } else if self.scroll_y >= max_scroll_y && held_dir > 0 {
                self.selected = count - 1;
                self.last_update_at = None;
                self.last_motion_dir = 0;
            }
            return;
        }

        if self.last_motion_dir > 0 && self.scroll_y < max_scroll_y {
            self.selected = ((self.scroll_y + ARCADE_ROW_HEIGHT - 1) / ARCADE_ROW_HEIGHT) as usize;
        } else if self.last_motion_dir < 0 && self.scroll_y > 0 {
            self.selected = (self.scroll_y / ARCADE_ROW_HEIGHT) as usize;
        }

        let target_px = self.selected as i32 * ARCADE_ROW_HEIGHT;
        let delta = target_px - self.scroll_y;
        if delta.abs() <= ARCADE_ROW_STEP_PX_PER_FRAME {
            self.set_visual_px(target_px, count);
            self.last_update_at = None;
            self.last_motion_dir = 0;
        } else {
            self.set_visual_px(
                self.scroll_y + delta.signum() * ARCADE_ROW_STEP_PX_PER_FRAME,
                count,
            );
        }

        if self.scroll_y <= 0 {
            self.selected = 0;
            if self.last_motion_dir <= 0 {
                self.last_update_at = None;
                self.last_motion_dir = 0;
            }
        } else if self.scroll_y >= max_scroll_y {
            self.selected = count - 1;
            if self.last_motion_dir >= 0 {
                self.last_update_at = None;
                self.last_motion_dir = 0;
            }
        }
    }

    fn step_target(&mut self, dir: i32, count: usize, now: Instant) {
        if count == 0 {
            self.reset();
            return;
        }
        let next = if dir > 0 {
            self.selected.saturating_add(1).min(count - 1)
        } else if dir < 0 {
            self.selected.saturating_sub(1)
        } else {
            self.selected
        };
        if next != self.selected {
            self.selected = next;
            self.last_motion_dir = dir.signum();
            if self.last_update_at.is_none() {
                self.last_update_at = Some(now);
            }
        }
    }

    pub fn bench_hold_tick(&mut self, dir: i32, count: usize, now: Instant, first_tick: bool) {
        if first_tick {
            self.step_target(dir, count, now);
        }
        let continuous_dir = self.continuous_hold_dir(dir, now);
        if continuous_dir != 0 || !self.is_settled() {
            self.tick_scroll(continuous_dir, count, now);
        }
    }

    fn is_settled(&self) -> bool {
        (self.visual_index - self.selected as f32).abs() < 0.001
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
    repeat: RepeatNav,
    prev: PadState,
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
                Screen::Home => self.handle_home(now, frame_now, catalog.systems.len()),
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
        system_count: usize,
    ) -> Option<LauncherEvent> {
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
            self.arcade.reset();
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
            self.screen = Screen::Home;
            self.arcade.reset();
            return None;
        }

        if count == 0 {
            return None;
        }

        if self.arcade.selected >= count {
            self.arcade.selected = count - 1;
            self.arcade.snap_to_selected();
        }

        let raw_held_dir = if now.dpad_down && !now.dpad_up {
            1
        } else if now.dpad_up && !now.dpad_down {
            -1
        } else {
            0
        };
        if rising(now.dpad_down, self.prev.dpad_down) && !now.dpad_up {
            self.arcade.step_target(1, count, frame_now);
        } else if rising(now.dpad_up, self.prev.dpad_up) && !now.dpad_down {
            self.arcade.step_target(-1, count, frame_now);
        }
        let continuous_dir = self.arcade.continuous_hold_dir(raw_held_dir, frame_now);
        if continuous_dir != 0 || !self.arcade.is_settled() {
            self.arcade.tick_scroll(continuous_dir, count, frame_now);
        }

        if rising(now.btn_a, self.prev.btn_a) {
            return catalog
                .system_game_at(system_id, self.arcade.selected)
                .map(|game| LauncherEvent {
                    action: LauncherAction::LaunchGame,
                    path: Some(game.mra_path.clone()),
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
            let confirmed = match action {
                Some(ConfirmAction::ExitToMister) => self.confirm_selected == 0,
                _ => self.confirm_selected == 1,
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
                    None => None,
                };
            }
        }
        None
    }
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

pub fn game_title(catalog: &ArcadeCatalog, mra_path: &str) -> String {
    catalog.title_for_path(mra_path).to_string()
}

fn wait_for_fifo() -> bool {
    for _ in 0..50 {
        if Path::new(CMD_FIFO).exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
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

/// Stop Main so Slint owns SPI, HDMI routing, and evdev (no grab).
pub fn stop_mister() {
    let _ = Command::new("sh")
        .arg("-c")
        .arg("kill -9 $(pidof MiSTer_MagiK) 2>/dev/null; kill -9 $(pidof MiSTer) 2>/dev/null")
        .status();
    for _ in 0..30 {
        if !mister_running() {
            thread::sleep(Duration::from_millis(50));
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_mister() -> Result<(), String> {
    Command::new(MISTER_BIN)
        .spawn()
        .map_err(|e| format!("failed to spawn {MISTER_BIN}: {e}"))?;
    for _ in 0..150 {
        if Path::new(CMD_FIFO).exists() && mister_running() {
            thread::sleep(Duration::from_millis(200));
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
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
    std::fs::OpenOptions::new()
        .write(true)
        .open(CMD_FIFO)
        .and_then(|mut f| f.write_all(cmd.as_bytes()))
        .map_err(|e| format!("failed to write {CMD_FIFO}: {e}"))
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
pub fn execute_game_launch(launch_ref: &str) -> Result<bool, String> {
    if !Path::new(launch_ref).exists() {
        return Err(format!("launch target not found: {launch_ref}"));
    }

    let spawned = if mister_running() {
        false
    } else {
        println!("launch: starting {MISTER_BIN} for load_core");
        spawn_mister()?;
        true
    };

    if !wait_for_fifo() {
        return Err(format!("timed out waiting for {CMD_FIFO}"));
    }

    let cmd = if Command::new("pidof")
        .arg("MiSTer_MagiK")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        format!("mister_magik_launch {launch_ref}\n")
    } else {
        format!("load_core {launch_ref}\n")
    };
    println!("launch: {}", cmd.trim_end());
    write_mister_command(&cmd)?;

    LAUNCH_STATE.store(LAUNCH_SENT, Ordering::Release);
    Ok(spawned)
}

pub fn reset_launch() {
    LAUNCH_STATE.store(LAUNCH_IDLE, Ordering::Release);
}

pub fn reset_database_and_reboot() -> Result<(), String> {
    library_bench::remove_default_sqlite_database()?;
    reboot_mister()
}

pub fn reboot_mister() -> Result<(), String> {
    Command::new("reboot")
        .spawn()
        .map(|_| ())
        .or_else(|_| {
            Command::new("sh")
                .arg("-c")
                .arg("reboot -f")
                .spawn()
                .map(|_| ())
        })
        .map_err(|e| format!("failed to reboot MiSTer: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcade_opens_with_first_row_centered() {
        let nav = ArcadeNav::new();
        assert_eq!(nav.selected, 0);
        assert_eq!(nav.visual_index, 0.0);
        assert_eq!(nav.scroll_y, 0);
    }

    #[test]
    fn arcade_velocity_scrolls_continuously() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        nav.tick_scroll(1, 10, t0);
        nav.tick_scroll(1, 10, t0 + Duration::from_millis(50));
        nav.tick_scroll(1, 10, t0 + Duration::from_millis(100));
        nav.tick_scroll(1, 10, t0 + Duration::from_millis(125));
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.visual_index, 1.0);
        assert_eq!(nav.scroll_y, ARCADE_ROW_HEIGHT);
    }

    #[test]
    fn arcade_single_press_commits_next_target() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        nav.step_target(1, 10, t0);
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.visual_index, 0.0);
        nav.tick_scroll(0, 10, t0 + Duration::from_millis(20));
        assert_eq!(nav.selected, 1);
        assert!(nav.visual_index > 0.0);
        nav.tick_scroll(0, 10, t0 + Duration::from_millis(70));
        nav.tick_scroll(0, 10, t0 + Duration::from_millis(104));
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.visual_index, 1.0);
    }

    #[test]
    fn arcade_hold_continues_after_tap_target() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        nav.step_target(1, 10, t0);
        for frame in 1..=8 {
            nav.tick_scroll(1, 10, t0 + Duration::from_millis(frame * 50));
        }
        assert!(nav.visual_index > 1.0);
        assert!(nav.selected > 1);
    }

    #[test]
    fn arcade_release_settles_to_selected_row() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        nav.tick_scroll(1, 10, t0);
        nav.tick_scroll(1, 10, t0 + Duration::from_millis(50));
        nav.tick_scroll(1, 10, t0 + Duration::from_millis(80));
        assert_eq!(nav.selected, 1);
        assert!(nav.visual_index > 0.5 && nav.visual_index < 1.0);
        nav.tick_scroll(0, 10, t0 + Duration::from_millis(120));
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.visual_index, 1.0);
    }

    #[test]
    fn arcade_release_after_tiny_downward_motion_commits_forward() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        nav.tick_scroll(1, 10, t0);
        nav.tick_scroll(1, 10, t0 + Duration::from_millis(10));
        assert_eq!(nav.selected, 1);
        assert!(nav.visual_index > 0.0 && nav.visual_index < 0.5);

        nav.tick_scroll(0, 10, t0 + Duration::from_millis(11));
        assert_eq!(nav.selected, 1);

        nav.tick_scroll(0, 10, t0 + Duration::from_millis(120));
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.visual_index, 1.0);
    }

    #[test]
    fn arcade_release_after_tiny_upward_motion_commits_backward() {
        let mut nav = ArcadeNav::new();
        nav.selected = 5;
        nav.snap_to_selected();
        let t0 = Instant::now();
        nav.tick_scroll(-1, 10, t0);
        nav.tick_scroll(-1, 10, t0 + Duration::from_millis(10));
        assert_eq!(nav.selected, 4);
        assert!(nav.visual_index > 4.5 && nav.visual_index < 5.0);

        nav.tick_scroll(0, 10, t0 + Duration::from_millis(11));
        assert_eq!(nav.selected, 4);

        nav.tick_scroll(0, 10, t0 + Duration::from_millis(120));
        assert_eq!(nav.selected, 4);
        assert_eq!(nav.visual_index, 4.0);
    }

    #[test]
    fn arcade_scroll_clamps_at_edges() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        nav.tick_scroll(-1, 10, t0);
        nav.tick_scroll(-1, 10, t0 + Duration::from_millis(250));
        assert_eq!(nav.selected, 0);
        assert_eq!(nav.visual_index, 0.0);
        nav.selected = 9;
        nav.snap_to_selected();
        nav.tick_scroll(1, 10, t0);
        nav.tick_scroll(1, 10, t0 + Duration::from_millis(250));
        assert_eq!(nav.selected, 9);
        assert_eq!(nav.visual_index, 9.0);
    }

    #[test]
    fn arcade_tap_does_not_continue_into_second_row_before_hold_delay() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        nav.step_target(1, 10, t0);
        for frame in 0..=17 {
            let now = t0 + Duration::from_millis(frame * 10);
            let dir = nav.continuous_hold_dir(1, now);
            assert_eq!(dir, 0);
            if dir != 0 || !nav.is_settled() {
                nav.tick_scroll(dir, 10, now);
            }
        }
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.visual_index, 1.0);
        assert_eq!(nav.scroll_y, ARCADE_ROW_HEIGHT);
    }

    #[test]
    fn arcade_hold_becomes_continuous_after_delay() {
        let mut nav = ArcadeNav::new();
        let t0 = Instant::now();
        nav.step_target(1, 10, t0);
        for frame in 0..=17 {
            let now = t0 + Duration::from_millis(frame * 10);
            let dir = nav.continuous_hold_dir(1, now);
            if dir != 0 || !nav.is_settled() {
                nav.tick_scroll(dir, 10, now);
            }
        }
        assert_eq!(nav.selected, 1);
        assert_eq!(nav.visual_index, 1.0);

        let dir = nav.continuous_hold_dir(1, t0 + Duration::from_millis(210));
        assert_eq!(dir, 1);
        nav.tick_scroll(dir, 10, t0 + Duration::from_millis(210));
        nav.tick_scroll(1, 10, t0 + Duration::from_millis(230));
        assert!(nav.visual_index > 1.0);
    }
}
