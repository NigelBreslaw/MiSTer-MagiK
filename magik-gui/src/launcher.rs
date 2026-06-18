//! Launcher navigation and arcade game launch.

use crate::arcade_catalog::{
    ArcadeCatalog, ARCADE_ROW_HEIGHT, HOME_LIST_VISIBLE_W, HOME_TILE_GAP, HOME_TILE_WIDTH,
};
use crate::input_repeat::RepeatNav;
use crate::input_state::PadState;
use crate::library_db;
use std::fmt;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const CMD_FIFO: &str = "/dev/MiSTer_cmd";
const MISTER_BIN: &str = "/media/fat/MiSTer_MagiK";
const MISTER_PROCESS_NAMES: &[&str] = &["MiSTer_MagiK", "MiSTer"];
const VIRTUAL_LAUNCH_CACHE_DIR: &str = "/media/fat/mister-magik/launch-cache";
const AMIGAVISION_GAME_LAUNCH_PREFIX: &str = "magik-amigavision:";
const AMIGAVISION_LAUNCHER_REF: &str = "magik-amigavision-launcher";
const AMIGAVISION_MGL_PATH: &str = "/media/fat/_Computer/Amiga.mgl";
const AMIGAVISION_HDF_PATH: &str = "/media/fat/games/Amiga/AmigaVision.hdf";
const AMIGAVISION_SHARED_DIR: &str = "/media/fat/games/Amiga/shared";
const AMIGAVISION_AGS_BOOT: &str = "/media/fat/games/Amiga/shared/ags_boot";
const ARCADE_NORMAL_PX_PER_FRAME: i32 = 6;
const ARCADE_TURBO_PX_PER_FRAME: i32 = 12;
const ARCADE_QUICK_TAP_MAX: Duration = Duration::from_millis(220);
const ARCADE_TURBO_REPRESS_WINDOW: Duration = Duration::from_millis(350);
const FIFO_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const FIFO_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const MISTER_START_TIMEOUT: Duration = Duration::from_secs(15);

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

    pub fn is_scroll_active(&self) -> bool {
        !self.is_settled() || self.scroll.held_dir != 0 || self.scroll.intent_queue != 0
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
    repeat: RepeatNav,
    prev: PadState,
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
        if launch_ref.starts_with("magik-plan:") {
            materialize_virtual_launch_ref(launch_ref)
        } else if launch_ref.starts_with(AMIGAVISION_GAME_LAUNCH_PREFIX) {
            materialize_amigavision_game_launch_ref(launch_ref)
        } else if launch_ref == AMIGAVISION_LAUNCHER_REF {
            materialize_amigavision_launcher_ref()
        } else {
            Ok(launch_ref.to_string())
        }
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

fn materialize_virtual_launch_ref(launch_ref: &str) -> Result<String, String> {
    let plan = library_db::load_virtual_launch_plan(launch_ref)?
        .ok_or_else(|| format!("virtual launch plan not found: {launch_ref}"))?;
    if plan.payload_path.trim().is_empty() {
        return Err(format!("virtual launch plan has no payload: {launch_ref}"));
    }
    let dir = Path::new(VIRTUAL_LAUNCH_CACHE_DIR);
    fs::create_dir_all(dir).map_err(|e| format!("create virtual launch cache: {e}"))?;
    let path = dir.join(format!("{}.mgl", sanitize_launch_ref(launch_ref)));
    let content = virtual_mgl_content(&plan);
    let should_write = fs::read_to_string(&path)
        .map(|existing| existing != content)
        .unwrap_or(true);
    if should_write {
        fs::write(&path, content).map_err(|e| format!("write virtual launch mgl: {e}"))?;
    }
    Ok(path.display().to_string())
}

fn materialize_amigavision_launcher_ref() -> Result<String, String> {
    materialize_amigavision_launcher_ref_at(
        Path::new(AMIGAVISION_MGL_PATH),
        Path::new(AMIGAVISION_HDF_PATH),
        Path::new(AMIGAVISION_SHARED_DIR),
        Path::new(AMIGAVISION_AGS_BOOT),
    )
}

fn materialize_amigavision_game_launch_ref(launch_ref: &str) -> Result<String, String> {
    let encoded = launch_ref
        .strip_prefix(AMIGAVISION_GAME_LAUNCH_PREFIX)
        .ok_or_else(|| format!("invalid AmigaVision launch ref: {launch_ref}"))?;
    let title = decode_launch_component(encoded)?;
    materialize_amigavision_game_launch_ref_at(
        &title,
        Path::new(AMIGAVISION_MGL_PATH),
        Path::new(AMIGAVISION_HDF_PATH),
        Path::new(AMIGAVISION_SHARED_DIR),
        Path::new(AMIGAVISION_AGS_BOOT),
    )
}

fn materialize_amigavision_launcher_ref_at(
    mgl_path: &Path,
    hdf_path: &Path,
    shared_dir: &Path,
    ags_boot_path: &Path,
) -> Result<String, String> {
    validate_amigavision_install(mgl_path, hdf_path)?;
    fs::create_dir_all(shared_dir).map_err(|e| format!("create AmigaVision shared dir: {e}"))?;
    match fs::remove_file(ags_boot_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale AmigaVision ags_boot: {e}")),
    }
    Ok(mgl_path.display().to_string())
}

fn materialize_amigavision_game_launch_ref_at(
    title: &str,
    mgl_path: &Path,
    hdf_path: &Path,
    shared_dir: &Path,
    ags_boot_path: &Path,
) -> Result<String, String> {
    validate_amigavision_install(mgl_path, hdf_path)?;
    fs::create_dir_all(shared_dir).map_err(|e| format!("create AmigaVision shared dir: {e}"))?;
    fs::write(ags_boot_path, format!("{title}\n"))
        .map_err(|e| format!("write AmigaVision ags_boot: {e}"))?;
    Ok(mgl_path.display().to_string())
}

fn validate_amigavision_install(mgl_path: &Path, hdf_path: &Path) -> Result<(), String> {
    if !mgl_path.is_file() {
        return Err(format!(
            "AmigaVision launcher is not installed: {}",
            mgl_path.display()
        ));
    }
    if !hdf_path.is_file() {
        return Err(format!(
            "AmigaVision HDF is not installed: {}. Extract the AmigaVision MiSTer archive first.",
            hdf_path.display()
        ));
    }
    Ok(())
}

fn virtual_mgl_content(plan: &library_db::VirtualLaunchPlan) -> String {
    let file_type = match plan.mount_kind.as_str() {
        "load-file" => "f",
        "mount-image" => "s",
        _ => "s",
    };
    format!(
        concat!(
            "<mistergamedescription>\n",
            "  <name>{}</name>\n",
            "  <rbf>{}</rbf>\n",
            "  <file delay=\"{}\" type=\"{}\" index=\"{}\" path=\"{}\"/>\n",
            "</mistergamedescription>\n"
        ),
        xml_escape(&plan.title),
        xml_escape(&plan.core_path),
        plan.mount_delay_secs,
        file_type,
        plan.mount_index,
        xml_escape(&plan.payload_path)
    )
}

fn decode_launch_component(value: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(value.len());
    let input = value.as_bytes();
    let mut idx = 0usize;
    while idx < input.len() {
        if input[idx] == b'%' {
            if idx + 2 >= input.len() {
                return Err("invalid percent escape in launch ref".to_string());
            }
            let hi = hex_value(input[idx + 1])
                .ok_or_else(|| "invalid percent escape in launch ref".to_string())?;
            let lo = hex_value(input[idx + 2])
                .ok_or_else(|| "invalid percent escape in launch ref".to_string())?;
            bytes.push((hi << 4) | lo);
            idx += 3;
        } else {
            bytes.push(input[idx]);
            idx += 1;
        }
    }
    String::from_utf8(bytes).map_err(|e| format!("invalid UTF-8 in launch ref: {e}"))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn sanitize_launch_ref(launch_ref: &str) -> String {
    launch_ref
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
    reboot_mister()
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
                image_path: "".into(),
                has_image: false,
                system_id: "amiga".into(),
            }],
            vec![GameSystemEntry {
                id: "amiga".into(),
                title: "Amiga".into(),
                count: 1,
            }],
        )
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
    fn launcher_launches_image_less_system_games() {
        let catalog = image_less_amiga_catalog();
        let mut nav = LauncherNav::new();
        let t0 = Instant::now();

        let mut press_a = PadState::default();
        press_a.btn_a = true;
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
    fn amigavision_game_launch_ref_writes_ags_boot() {
        let root = unique_temp_dir("amigavision-launch");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::create_dir_all(hdf.parent().unwrap()).expect("create hdf dir");
        std::fs::write(&mgl, "<mistergamedescription/>").expect("write mgl");
        std::fs::write(&hdf, "hdf").expect("write hdf");

        let target = materialize_amigavision_game_launch_ref_at(
            "4th & Inches (OCS)[en]",
            &mgl,
            &hdf,
            &shared,
            &ags_boot,
        )
        .expect("materialize AmigaVision game");

        assert_eq!(target, mgl.display().to_string());
        assert_eq!(
            std::fs::read_to_string(&ags_boot).expect("read ags_boot"),
            "4th & Inches (OCS)[en]\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_launcher_ref_removes_stale_ags_boot() {
        let root = unique_temp_dir("amigavision-launcher");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::create_dir_all(&shared).expect("create shared dir");
        std::fs::write(&mgl, "<mistergamedescription/>").expect("write mgl");
        std::fs::write(&hdf, "hdf").expect("write hdf");
        std::fs::write(&ags_boot, "Agony\n").expect("write stale ags_boot");

        let target = materialize_amigavision_launcher_ref_at(&mgl, &hdf, &shared, &ags_boot)
            .expect("materialize AmigaVision launcher");

        assert_eq!(target, mgl.display().to_string());
        assert!(!ags_boot.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn amigavision_launch_ref_reports_missing_hdf() {
        let root = unique_temp_dir("amigavision-missing-hdf");
        let mgl = root.join("_Computer/Amiga.mgl");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        let shared = root.join("games/Amiga/shared");
        let ags_boot = shared.join("ags_boot");
        std::fs::create_dir_all(mgl.parent().unwrap()).expect("create mgl dir");
        std::fs::write(&mgl, "<mistergamedescription/>").expect("write mgl");

        let err =
            materialize_amigavision_game_launch_ref_at("Agony", &mgl, &hdf, &shared, &ags_boot)
                .expect_err("missing HDF should fail");

        assert!(err.contains("AmigaVision HDF is not installed"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn percent_decodes_amigavision_launch_title() {
        assert_eq!(
            decode_launch_component("4th%20%26%20Inches%20%28OCS%29%5Ben%5D")
                .expect("decode title"),
            "4th & Inches (OCS)[en]"
        );
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

    #[test]
    fn virtual_mgl_content_mounts_payload_with_core_path() {
        let plan = library_db::VirtualLaunchPlan {
            launch_ref: "magik-plan:payload-saturn-test".to_string(),
            title: "NiGHTS & Dreams".to_string(),
            core_path: "_Console/Saturn".to_string(),
            payload_path: "/media/fat/games/Saturn/Nights.chd".to_string(),
            mount_kind: "mount-image".to_string(),
            mount_index: 0,
            mount_delay_secs: 1,
        };

        let content = virtual_mgl_content(&plan);

        assert!(content.contains("<rbf>_Console/Saturn</rbf>"));
        assert!(content.contains("type=\"s\" index=\"0\""));
        assert!(content.contains("path=\"/media/fat/games/Saturn/Nights.chd\""));
        assert!(content.contains("<name>NiGHTS &amp; Dreams</name>"));
    }
}
