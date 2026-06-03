//! Launcher navigation and arcade game launch.

use crate::input::PadState;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::Duration;

const CMD_FIFO: &str = "/dev/MiSTer_cmd";
const MISTER_BIN: &str = "/media/fat/MiSTer";

pub const GAMES: &[(&str, &str)] = &[
    (
        "Donkey Kong",
        "/media/fat/_Arcade/Donkey Kong (US, Set 1).mra",
    ),
    (
        "Pac-Man",
        "/media/fat/_Arcade/Pac-Man - Puck Man (JP, Set 1).mra",
    ),
    (
        "Galaga",
        "/media/fat/_Arcade/Galaga (Midway, Set 1).mra",
    ),
];

const LAUNCH_IDLE: u8 = 0;
const LAUNCH_SENT: u8 = 1;

static LAUNCH_STATE: AtomicU8 = AtomicU8::new(LAUNCH_IDLE);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Controller,
}

pub struct LauncherNav {
    pub screen: Screen,
    pub selected: usize,
    prev: PadState,
}

impl LauncherNav {
    pub fn new() -> Self {
        Self {
            screen: Screen::Home,
            selected: 0,
            prev: PadState::default(),
        }
    }

    /// Returns `Some(mra_path)` when a game launch was requested.
    pub fn handle_input(&mut self, now: &PadState) -> Option<&'static str> {
        let result = match self.screen {
            Screen::Home => self.handle_home(now),
            Screen::Controller => {
                if rising(now.btn_home, self.prev.btn_home) {
                    self.screen = Screen::Home;
                }
                None
            }
        };
        self.prev = now.clone();
        result
    }

    fn handle_home(&mut self, now: &PadState) -> Option<&'static str> {
        if rising(now.dpad_up, self.prev.dpad_up) {
            self.selected = if self.selected >= 2 {
                self.selected - 2
            } else {
                self.selected + 2
            };
        }
        if rising(now.dpad_down, self.prev.dpad_down) {
            self.selected = if self.selected < 2 {
                self.selected + 2
            } else {
                self.selected - 2
            };
        }
        if rising(now.dpad_left, self.prev.dpad_left) {
            self.selected = if self.selected % 2 == 1 {
                self.selected - 1
            } else {
                self.selected + 1
            };
        }
        if rising(now.dpad_right, self.prev.dpad_right) {
            self.selected = if self.selected % 2 == 0 {
                self.selected + 1
            } else {
                self.selected - 1
            };
        }

        if rising(now.btn_a, self.prev.btn_a) {
            return match self.selected {
                0 => {
                    self.screen = Screen::Controller;
                    None
                }
                1..=3 => {
                    let game_idx = match self.selected {
                        1 => 0,
                        2 => 1,
                        3 => 2,
                        _ => unreachable!(),
                    };
                    Some(GAMES[game_idx].1)
                }
                _ => None,
            };
        }

        None
    }
}

fn rising(now: bool, prev: bool) -> bool {
    now && !prev
}

pub fn game_title(mra_path: &str) -> &str {
    GAMES
        .iter()
        .find(|(_, path)| *path == mra_path)
        .map(|(title, _)| *title)
        .unwrap_or("Game")
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
        .arg("pid=$(pidof MiSTer 2>/dev/null); [ -n \"$pid\" ] && tr '\\0' ' ' < /proc/$pid/cmdline")
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
