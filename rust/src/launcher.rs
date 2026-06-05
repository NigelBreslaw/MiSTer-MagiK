//! Launcher navigation and arcade game launch.

use crate::arcade_catalog::{ArcadeCatalog, ARCADE_LIST_VISIBLE_H, ARCADE_ROW_HEIGHT};
use crate::input::PadState;
use crate::input_repeat::RepeatNav;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const CMD_FIFO: &str = "/dev/MiSTer_cmd";
const MISTER_BIN: &str = "/media/fat/MiSTer";

const LAUNCH_IDLE: u8 = 0;
const LAUNCH_SENT: u8 = 1;

static LAUNCH_STATE: AtomicU8 = AtomicU8::new(LAUNCH_IDLE);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Controller,
    Arcade,
}

pub struct ArcadeNav {
    pub selected: usize,
    pub scroll_y: i32,
}

impl ArcadeNav {
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll_y: 0,
        }
    }

    pub fn reset(&mut self) {
        self.selected = 0;
        self.scroll_y = 0;
    }
}

pub struct LauncherNav {
    pub screen: Screen,
    pub selected: usize,
    pub arcade: ArcadeNav,
    repeat: RepeatNav,
    prev: PadState,
}

impl LauncherNav {
    pub fn new() -> Self {
        Self {
            screen: Screen::Home,
            selected: 0,
            arcade: ArcadeNav::new(),
            repeat: RepeatNav::default(),
            prev: PadState::default(),
        }
    }

    /// Returns `Some(mra_path)` when a game launch was requested.
    pub fn handle_input(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<String> {
        let result = match self.screen {
            Screen::Home => self.handle_home(now, frame_now),
            Screen::Controller => {
                if rising(now.btn_home, self.prev.btn_home) || rising(now.btn_b, self.prev.btn_b) {
                    self.screen = Screen::Home;
                }
                None
            }
            Screen::Arcade => self.handle_arcade(now, frame_now, catalog),
        };
        self.prev = now.clone();
        result
    }

    fn handle_home(&mut self, now: &PadState, frame_now: Instant) -> Option<String> {
        if self.repeat.tick_left(now.dpad_left, frame_now) && self.selected > 0 {
            self.selected -= 1;
        }
        if self.repeat.tick_right(now.dpad_right, frame_now) && self.selected < 1 {
            self.selected += 1;
        }
        if self.repeat.tick_up(now.dpad_up, frame_now) && self.selected > 0 {
            self.selected -= 1;
        }
        if self.repeat.tick_down(now.dpad_down, frame_now) && self.selected < 1 {
            self.selected += 1;
        }

        if rising(now.btn_a, self.prev.btn_a) {
            return match self.selected {
                0 => {
                    self.screen = Screen::Controller;
                    None
                }
                1 => {
                    self.arcade.reset();
                    self.screen = Screen::Arcade;
                    None
                }
                _ => None,
            };
        }

        None
    }

    fn handle_arcade(
        &mut self,
        now: &PadState,
        frame_now: Instant,
        catalog: &ArcadeCatalog,
    ) -> Option<String> {
        let count = catalog.len();

        if rising(now.btn_home, self.prev.btn_home) || rising(now.btn_b, self.prev.btn_b) {
            self.screen = Screen::Home;
            self.arcade.reset();
            return None;
        }

        if count == 0 {
            return None;
        }

        if self.repeat.tick_down(now.dpad_down, frame_now) && self.arcade.selected + 1 < count {
            self.arcade.selected += 1;
            self.arcade.scroll_y =
                (self.arcade.scroll_y + ARCADE_ROW_HEIGHT).clamp(0, arcade_max_scroll(count));
            keep_arcade_visible(&mut self.arcade, count);
        }
        if self.repeat.tick_up(now.dpad_up, frame_now) && self.arcade.selected > 0 {
            self.arcade.selected -= 1;
            self.arcade.scroll_y =
                (self.arcade.scroll_y - ARCADE_ROW_HEIGHT).clamp(0, arcade_max_scroll(count));
            keep_arcade_visible(&mut self.arcade, count);
        }

        if rising(now.btn_a, self.prev.btn_a) {
            return catalog.path_at(self.arcade.selected).map(|p| p.to_string());
        }

        None
    }
}

fn arcade_max_scroll(count: usize) -> i32 {
    let content = count as i32 * ARCADE_ROW_HEIGHT;
    (content - ARCADE_LIST_VISIBLE_H).max(0)
}

fn keep_arcade_visible(arcade: &mut ArcadeNav, count: usize) {
    let row_top = arcade.selected as i32 * ARCADE_ROW_HEIGHT;
    let row_bottom = (arcade.selected as i32 + 1) * ARCADE_ROW_HEIGHT;
    if row_top < arcade.scroll_y {
        arcade.scroll_y = row_top;
    }
    if row_bottom > arcade.scroll_y + ARCADE_LIST_VISIBLE_H {
        arcade.scroll_y = row_bottom - ARCADE_LIST_VISIBLE_H;
    }
    arcade.scroll_y = arcade.scroll_y.clamp(0, arcade_max_scroll(count));
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

fn write_load_core(mra_path: &str) -> Result<(), String> {
    let cmd = format!("load_core {mra_path}\n");
    std::fs::OpenOptions::new()
        .write(true)
        .open(CMD_FIFO)
        .and_then(|mut f| f.write_all(cmd.as_bytes()))
        .map_err(|e| format!("failed to write {CMD_FIFO}: {e}"))
}

fn mister_running() -> bool {
    Command::new("pidof")
        .arg("MiSTer")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Stop stock MiSTer so Slint owns SPI, HDMI routing, and evdev (no grab).
pub fn stop_mister() {
    let _ = Command::new("sh")
        .arg("-c")
        .arg("kill -9 $(pidof MiSTer) 2>/dev/null")
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

/// True while Slint should keep the loading screen up.
pub fn launch_in_progress() -> bool {
    LAUNCH_STATE.load(Ordering::Acquire) == LAUNCH_SENT
}

/// MiSTer is running an arcade core (argv contains `.rbf`, not `menu.rbf`).
pub fn mister_running_arcade_core() -> bool {
    let output = Command::new("sh")
        .arg("-c")
        .arg(
            "pid=$(pidof MiSTer 2>/dev/null); [ -n \"$pid\" ] && tr '\\0' ' ' < /proc/$pid/cmdline",
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

/// Launch via fifo `load_core`. Spawns MiSTer if Slint owns the device (normal boot).
/// Returns `true` if MiSTer was spawned for this launch (caller should stop it on failure).
pub fn execute_game_launch(mra_path: &str) -> Result<bool, String> {
    if !Path::new(mra_path).exists() {
        return Err(format!("MRA not found: {mra_path}"));
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

    println!("launch: load_core {mra_path}");
    write_load_core(mra_path)?;

    LAUNCH_STATE.store(LAUNCH_SENT, Ordering::Release);
    Ok(spawned)
}

pub fn reset_launch() {
    LAUNCH_STATE.store(LAUNCH_IDLE, Ordering::Release);
}
