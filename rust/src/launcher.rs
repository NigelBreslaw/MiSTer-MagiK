//! Launcher navigation and arcade game launch via `/dev/MiSTer_cmd`.

use crate::input::PadState;
use std::io::Write;
use std::path::Path;
use std::thread;
use std::time::Duration;

const MISTER_BIN: &str = "/media/fat/MiSTer";
const CMD_FIFO: &str = "/dev/MiSTer_cmd";

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
                        1 => 0, // Donkey Kong (top-right)
                        2 => 1, // Pac-Man (bottom-left)
                        3 => 2, // Galaga (bottom-right)
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

/// Spawn stock MiSTer, send `load_core`, and exit so MiSTer owns the display.
pub fn launch_mra(mra_path: &str) -> ! {
    println!("launch_mra: {mra_path}");

    if !Path::new(mra_path).exists() {
        eprintln!("MRA not found: {mra_path}");
        std::process::exit(1);
    }

    std::process::Command::new(MISTER_BIN)
        .spawn()
        .unwrap_or_else(|e| {
            eprintln!("failed to spawn {MISTER_BIN}: {e}");
            std::process::exit(1);
        });

    let fifo = Path::new(CMD_FIFO);
    for attempt in 0..50 {
        if fifo.exists() {
            break;
        }
        if attempt == 49 {
            eprintln!("timed out waiting for {CMD_FIFO}");
            std::process::exit(1);
        }
        thread::sleep(Duration::from_millis(100));
    }

    let cmd = format!("load_core {mra_path}\n");
    if let Err(e) = std::fs::OpenOptions::new()
        .write(true)
        .open(CMD_FIFO)
        .and_then(|mut f| f.write_all(cmd.as_bytes()))
    {
        eprintln!("failed to write {CMD_FIFO}: {e}");
        std::process::exit(1);
    }

    println!("load_core sent; handing off to MiSTer");
    std::process::exit(0);
}
