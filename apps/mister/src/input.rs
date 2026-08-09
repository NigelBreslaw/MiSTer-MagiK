// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Linux navigation input — joystick layouts plus keyboard evdev aliases.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::input_event::InputBatch;
use crate::input_hub::InputHub;
use crate::input_state::{InputProfile, JS_EVENT_AXIS, JS_EVENT_BUTTON, PadRawEvent};
pub use crate::input_state::{PadInfo, PadState};

const JS_EVENT_SIZE: usize = 8;
const JS_EVENT_INIT: u8 = 0x80;

const PAD_RESCAN_INTERVAL: Duration = Duration::from_secs(1);
const INPUT_EVENT_SIZE: usize = if cfg!(target_pointer_width = "64") {
    24
} else {
    16
};

const EV_KEY: u16 = 1;
const KEY_ESC: u16 = 1;
const KEY_ENTER: u16 = 28;
const KEY_A: u16 = 30;
const KEY_B: u16 = 48;
const KEY_SPACE: u16 = 57;
const KEY_F9: u16 = 67;
const KEY_F10: u16 = 68;
const KEY_F12: u16 = 88;
const KEY_UP: u16 = 103;
const KEY_PAGEUP: u16 = 104;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
const KEY_DOWN: u16 = 108;
const KEY_PAGEDOWN: u16 = 109;
const KEY_MENU: u16 = 139;
const KEY_TAB: u16 = 15;
const MAIN_INPUT_PROXY_NAME: &str = "MiSTer virtual input";

pub struct PadReader {
    file: File,
    pub path: String,
    pub info: PadInfo,
    profile: InputProfile,
    state: PadState,
}

/// Poll connected joysticks, keyboards, and mouse activity and merge navigation into [`PadState`].
pub struct PadPool {
    pads: Vec<PadReader>,
    keyboards: Vec<KeyboardReader>,
    mouse: Option<File>,
    user_activity: bool,
    merged: PadState,
    active_idx: usize,
    db: crate::controller_db::ControllerDb,
    last_rescan: Instant,
    input_hub: Option<InputHub>,
}

impl PadPool {
    /// Open joystick and keyboard input nodes and load the controller registry.
    pub fn open_all() -> io::Result<Self> {
        let mut db = crate::controller_db::ControllerDb::load();
        crate::ui_errln!("controller db: {} entries from {}", db.len(), db.path());
        let paths = discover_js_devices();
        let mut pads = Vec::new();
        for path in paths {
            match PadReader::open_path_with_db(&path, &db) {
                Ok(r) => {
                    db.note_sighting(r.info());
                    pads.push(r);
                }
                Err(e) => crate::ui_errln!("pad: skip {path}: {e}"),
            }
        }
        if pads.is_empty() {
            crate::ui_errln!("pad: no joystick device found; waiting for hotplug");
        } else {
            crate::ui_errln!("pad: listening on {} device(s)", pads.len());
        }
        let keyboards: Vec<_> = discover_keyboard_devices()
            .into_iter()
            .filter_map(|path| match KeyboardReader::open(&path) {
                Ok(reader) if reader.is_main_proxy => None,
                Ok(reader) => Some(reader),
                Err(e) => {
                    crate::ui_errln!("keyboard: skip {path}: {e}");
                    None
                }
            })
            .collect();
        crate::ui_errln!("input proxy: navigation owned by input hub protocol v2");
        Ok(Self {
            pads,
            keyboards,
            mouse: open_mouse_activity(),
            user_activity: false,
            merged: PadState::default(),
            active_idx: 0,
            db,
            last_rescan: Instant::now(),
            input_hub: Some(InputHub::start()),
        })
    }

    pub fn db(&self) -> &crate::controller_db::ControllerDb {
        &self.db
    }

    pub fn len(&self) -> usize {
        self.pads.len()
    }

    pub fn state(&self) -> &PadState {
        &self.merged
    }

    /// Info for the pad that most recently sent input.
    pub fn info(&self) -> &PadInfo {
        match self.active_pad() {
            Some(pad) => &pad.info,
            None => no_pad_info(),
        }
    }

    pub fn path(&self) -> &str {
        self.active_pad()
            .map(|pad| pad.path.as_str())
            .unwrap_or("(no controller)")
    }

    /// Index of the pad that most recently sent input.
    pub fn active_idx(&self) -> usize {
        self.clamped_active_idx()
    }

    pub fn info_at(&self, idx: usize) -> &PadInfo {
        match self.pads.get(idx) {
            Some(pad) => &pad.info,
            None => no_pad_info(),
        }
    }

    /// First connected pad that has not completed setup (`setup_complete`).
    pub fn index_needing_setup(&self) -> Option<usize> {
        self.pads.iter().position(|p| self.db.needs_setup(&p.info))
    }

    pub fn path_at(&self, idx: usize) -> &str {
        self.pads
            .get(idx)
            .map(|pad| pad.path.as_str())
            .unwrap_or("(no controller)")
    }

    pub fn state_at(&self, idx: usize) -> &PadState {
        match self.pads.get(idx) {
            Some(pad) => &pad.state,
            None => no_pad_state(),
        }
    }

    pub fn navigation_state_at(&self, idx: usize) -> PadState {
        let mut state = self.state_at(idx).clone();
        for keyboard in self
            .keyboards
            .iter()
            .filter(|keyboard| !keyboard.is_main_proxy)
        {
            keyboard.merge_into(&mut state);
        }
        state.rebuild_pressed_now();
        state
    }

    /// Save a new default registry entry for a pad (does not mark setup complete).
    pub fn register_new_at(&mut self, idx: usize) -> io::Result<()> {
        let info = self
            .pads
            .get(idx)
            .map(|pad| pad.info.clone())
            .ok_or_else(|| pad_index_error(idx))?;
        let entry = crate::controller_db::ControllerDb::default_entry(&info);
        self.db.upsert(&info, entry);
        self.db.save()?;
        if let Some(pad) = self.pads.get_mut(idx) {
            pad.refresh_profile();
        }
        Ok(())
    }

    /// Bind a pad to an existing registry entry (USB port change).
    pub fn claim_existing_at(&mut self, idx: usize, list_index: usize) -> io::Result<()> {
        let info = self
            .pads
            .get(idx)
            .map(|pad| pad.info.clone())
            .ok_or_else(|| pad_index_error(idx))?;
        let items = self.db.list_entries();
        let item = items.get(list_index).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "list index out of range")
        })?;
        self.db.claim_existing(&info, &item.id)?;
        self.db.save()?;
        if let Some(pad) = self.pads.get_mut(idx) {
            pad.refresh_profile();
        }
        Ok(())
    }

    /// Save name + type and mark this pad's setup complete.
    pub fn finish_setup_at(
        &mut self,
        idx: usize,
        label: String,
        kind: crate::controller_db::ControllerKind,
    ) -> io::Result<()> {
        let info = self
            .pads
            .get(idx)
            .map(|pad| pad.info.clone())
            .ok_or_else(|| pad_index_error(idx))?;
        self.db.finish_setup(&info, label, kind);
        self.db.save()?;
        Ok(())
    }

    /// Drain all pads; returns true if merged state changed.
    pub fn poll(&mut self) -> bool {
        self.poll_with_debug_labels(false)
    }

    /// Drain all pads; returns true if merged state changed.
    pub fn poll_with_debug_labels(&mut self, debug_labels: bool) -> bool {
        let mut changed = false;
        self.user_activity = false;

        if self.last_rescan.elapsed() >= PAD_RESCAN_INTERVAL {
            changed |= self.rescan();
            self.last_rescan = Instant::now();
        }

        let mut i = 0;
        while i < self.pads.len() {
            match self.pads[i].poll_with_debug_labels(debug_labels) {
                Ok(true) => {
                    self.active_idx = i;
                    changed = true;
                    self.user_activity = true;
                    i += 1;
                }
                Ok(false) => {
                    i += 1;
                }
                Err(e) => {
                    let path = self.pads[i].path.clone();
                    crate::ui_errln!("pad: disconnected {path}: {e}");
                    self.pads.remove(i);
                    changed = true;
                    if self.active_idx >= self.pads.len() {
                        self.active_idx = self.pads.len().saturating_sub(1);
                    }
                }
            }
        }
        let mut i = 0;
        while i < self.keyboards.len() {
            match self.keyboards[i].poll() {
                Ok(keyboard_changed) => {
                    changed |= keyboard_changed;
                    self.user_activity |= keyboard_changed;
                    i += 1;
                }
                Err(e) => {
                    let path = self.keyboards[i].path.clone();
                    crate::ui_errln!("keyboard: disconnected {path}: {e}");
                    self.keyboards.remove(i);
                    changed = true;
                }
            }
        }
        if let Some(mouse) = self.mouse.as_mut() {
            let mut bytes = [0_u8; 64];
            loop {
                match mouse.read(&mut bytes) {
                    Ok(0) => {
                        self.mouse = None;
                        break;
                    }
                    Ok(_) => {
                        changed = true;
                        self.user_activity = true;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        self.mouse = None;
                        break;
                    }
                }
            }
        }
        if changed {
            self.rebuild_merged_state();
        }
        changed
    }

    pub fn user_activity(&self) -> bool {
        self.user_activity
    }

    pub fn wait_for_input(&self, timeout: Duration) {
        if let Some(hub) = &self.input_hub {
            hub.wait_for_input(timeout);
        } else {
            std::thread::sleep(timeout);
        }
    }

    /// Drain the sole production navigation source. Raw devices remain available
    /// through the other accessors for setup and diagnostics only.
    pub fn drain_input_batch(&self) -> InputBatch {
        self.input_hub
            .as_ref()
            .map_or_else(InputBatch::default, InputHub::drain)
    }

    fn active_pad(&self) -> Option<&PadReader> {
        self.pads.get(self.clamped_active_idx())
    }

    fn clamped_active_idx(&self) -> usize {
        if self.pads.is_empty() {
            0
        } else {
            self.active_idx.min(self.pads.len() - 1)
        }
    }

    fn rescan(&mut self) -> bool {
        let mut changed = false;
        for path in discover_js_devices() {
            if self.pads.iter().any(|pad| pad.path == path) {
                continue;
            }
            match PadReader::open_path_with_db(&path, &self.db) {
                Ok(reader) => {
                    self.db.note_sighting(reader.info());
                    self.pads.push(reader);
                    crate::ui_errln!("pad: hotplug added {path} ({} device(s))", self.pads.len());
                    changed = true;
                }
                Err(e) => crate::ui_errln!("pad: hotplug skip {path}: {e}"),
            }
        }
        for path in discover_keyboard_devices() {
            if self.keyboards.iter().any(|keyboard| keyboard.path == path) {
                continue;
            }
            match KeyboardReader::open(&path) {
                Ok(reader) if reader.is_main_proxy => continue,
                Ok(reader) => {
                    crate::ui_errln!("keyboard: hotplug added {path}");
                    self.keyboards.push(reader);
                    changed = true;
                }
                Err(e) => crate::ui_errln!("keyboard: hotplug skip {path}: {e}"),
            }
        }
        if self.mouse.is_none() {
            self.mouse = open_mouse_activity();
        }
        changed
    }

    fn rebuild_merged_state(&mut self) {
        let states: Vec<&PadState> = self.pads.iter().map(|p| p.state()).collect();
        let active_idx = self.clamped_active_idx();
        let active_raw = self
            .pads
            .get(active_idx)
            .and_then(|pad| pad.state.last_raw_event);
        let active_raw_label = self
            .pads
            .get(active_idx)
            .map(|pad| pad.state.last_raw.clone());
        let active_label = self
            .pads
            .get(active_idx)
            .map(|pad| pad.state.last_event_label.clone());
        self.merged = merge_pad_states(&states);
        for keyboard in &self.keyboards {
            keyboard.merge_into(&mut self.merged);
        }
        self.merged.rebuild_pressed_now();
        self.merged.last_raw_event = active_raw;
        if let Some(last_raw) = active_raw_label {
            self.merged.last_raw = last_raw;
        }
        if let Some(last_event_label) = active_label {
            self.merged.last_event_label = last_event_label;
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_states(states: Vec<PadState>) -> Self {
        let mut pool = Self {
            pads: states
                .into_iter()
                .enumerate()
                .map(|(idx, state)| PadReader {
                    file: File::open("/dev/null").expect("open /dev/null"),
                    path: format!("/dev/input/js{idx}"),
                    info: PadInfo {
                        name: format!("Test pad {idx}"),
                        ..PadInfo::default()
                    },
                    profile: InputProfile::generic(),
                    state,
                })
                .collect(),
            keyboards: Vec::new(),
            mouse: None,
            user_activity: false,
            merged: PadState::default(),
            active_idx: 0,
            db: crate::controller_db::ControllerDb::load(),
            last_rescan: Instant::now(),
            input_hub: None,
        };
        pool.rebuild_merged_state();
        pool
    }

    #[cfg(test)]
    pub(crate) fn set_test_keyboard_state(&mut self, state: PadState) {
        self.keyboards = vec![KeyboardReader {
            file: File::open("/dev/null").expect("open /dev/null"),
            path: "test-keyboard".into(),
            is_main_proxy: false,
            state: KeyboardState {
                up: state.dpad_up,
                down: state.dpad_down,
                left: state.dpad_left,
                right: state.dpad_right,
                a: state.btn_a,
                b: state.btn_b,
                ..KeyboardState::default()
            },
        }];
        self.rebuild_merged_state();
    }
}

fn open_mouse_activity() -> Option<File> {
    let file = OpenOptions::new().read(true).open("/dev/input/mice").ok()?;
    set_nonblocking(&file).ok()?;
    Some(file)
}

impl crate::setup_nav::SetupPadSource for PadPool {
    fn index_needing_setup(&self) -> Option<usize> {
        self.index_needing_setup()
    }

    fn db(&self) -> &crate::controller_db::ControllerDb {
        self.db()
    }

    fn info_at(&self, idx: usize) -> &PadInfo {
        self.info_at(idx)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct KeyboardState {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    a: bool,
    enter: bool,
    b: bool,
    escape: bool,
    x: bool,
    y: bool,
    l: bool,
    r: bool,
    select: bool,
    start: bool,
    home: bool,
}

struct KeyboardReader {
    file: File,
    path: String,
    is_main_proxy: bool,
    state: KeyboardState,
}

impl KeyboardReader {
    fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        set_nonblocking(&file)?;
        let is_main_proxy = input_device_name(path).as_deref() == Some(MAIN_INPUT_PROXY_NAME);
        crate::ui_errln!(
            "keyboard: opened {path}{}",
            if is_main_proxy {
                " (Main menu input proxy)"
            } else {
                ""
            }
        );
        Ok(Self {
            file,
            path: path.to_string(),
            is_main_proxy,
            state: KeyboardState::default(),
        })
    }

    fn poll(&mut self) -> io::Result<bool> {
        drain_keyboard_events(&mut self.file, &mut self.state, self.is_main_proxy)
    }

    fn merge_into(&self, state: &mut PadState) {
        state.dpad_up |= self.state.up;
        state.dpad_down |= self.state.down;
        state.dpad_left |= self.state.left;
        state.dpad_right |= self.state.right;
        state.btn_a |= self.state.a || self.state.enter;
        state.btn_b |= self.state.b || self.state.escape;
        state.btn_x |= self.state.x;
        state.btn_y |= self.state.y;
        state.btn_l |= self.state.l;
        state.btn_r |= self.state.r;
        state.btn_select |= self.state.select;
        state.btn_start |= self.state.start;
        state.btn_home |= self.state.home;
    }
}

fn drain_keyboard_events<R: Read>(
    reader: &mut R,
    state: &mut KeyboardState,
    main_proxy: bool,
) -> io::Result<bool> {
    let mut buf = [0u8; INPUT_EVENT_SIZE];
    let mut changed = false;
    loop {
        match reader.read_exact(&mut buf) {
            Ok(()) => {
                let (event_type, code, value) = parse_input_event(&buf);
                if event_type != EV_KEY {
                    continue;
                }
                // Every keyboard key is activity, even when it has no launcher
                // navigation binding (for example Shift or a letter key).
                changed = true;
                let pressed = value != 0;
                let field = match code {
                    KEY_UP => Some(&mut state.up),
                    KEY_DOWN => Some(&mut state.down),
                    KEY_LEFT => Some(&mut state.left),
                    KEY_RIGHT => Some(&mut state.right),
                    KEY_A => Some(&mut state.a),
                    KEY_ENTER => Some(&mut state.enter),
                    KEY_B => Some(&mut state.b),
                    KEY_ESC => Some(&mut state.escape),
                    KEY_F12 => Some(&mut state.home),
                    KEY_TAB if main_proxy => Some(&mut state.x),
                    KEY_SPACE if main_proxy => Some(&mut state.y),
                    KEY_PAGEUP if main_proxy => Some(&mut state.l),
                    KEY_PAGEDOWN if main_proxy => Some(&mut state.r),
                    KEY_F10 if main_proxy => Some(&mut state.select),
                    KEY_F9 if main_proxy => Some(&mut state.start),
                    KEY_MENU if main_proxy => Some(&mut state.home),
                    _ => None,
                };
                if let Some(field) = field {
                    changed |= *field != pressed;
                    *field = pressed;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(e),
            Err(e) => return Err(e),
        }
    }
    Ok(changed)
}

impl PadReader {
    /// Open the first available js device (for debug subcommands).
    pub fn open() -> io::Result<Self> {
        for path in discover_js_devices() {
            if let Ok(reader) = Self::open_path(&path) {
                return Ok(reader);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no joystick device found",
        ))
    }

    pub fn open_path(path: &str) -> io::Result<Self> {
        let db = crate::controller_db::ControllerDb::load();
        Self::open_path_with_db(path, &db)
    }

    fn open_path_with_db(path: &str, db: &crate::controller_db::ControllerDb) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        set_nonblocking(&file)?;
        let info = read_pad_info(path, &file)?;
        let profile = InputProfile::guess(&info);
        db.log_pad_status(&info, path);
        crate::ui_errln!(
            "pad: opened {path} ({} usb={} {} btn={} axes={}) input_profile={profile:?}",
            info.name,
            info.usb_port,
            info.vendor_id,
            info.js_buttons,
            info.js_axes
        );
        Ok(Self {
            path: path.to_string(),
            file,
            info,
            profile,
            state: PadState::default(),
        })
    }

    pub fn info(&self) -> &PadInfo {
        &self.info
    }

    fn refresh_profile(&mut self) {
        self.profile = InputProfile::guess(&self.info);
    }

    pub fn state(&self) -> &PadState {
        &self.state
    }

    /// Drain pending events; returns true if state changed.
    #[allow(dead_code)]
    pub fn poll(&mut self) -> io::Result<bool> {
        self.poll_with_debug_labels(false)
    }

    /// Drain pending events; returns true if state changed.
    pub fn poll_with_debug_labels(&mut self, debug_labels: bool) -> io::Result<bool> {
        drain_js_events(&mut self.file, self.profile, &mut self.state, debug_labels)
    }
}

fn drain_js_events<R: Read>(
    reader: &mut R,
    profile: InputProfile,
    state: &mut PadState,
    debug_labels: bool,
) -> io::Result<bool> {
    let mut buf = [0u8; JS_EVENT_SIZE];
    let mut changed = false;
    loop {
        match reader.read_exact(&mut buf) {
            Ok(()) => {
                let event_type = buf[6] & !JS_EVENT_INIT;
                let number = buf[7];
                let value = i16::from_le_bytes([buf[4], buf[5]]);
                let event = PadRawEvent {
                    event_type,
                    number,
                    value,
                };
                if profile.apply_js_event(state, event, debug_labels) {
                    changed = true;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(e),
            Err(e) => return Err(e),
        }
    }
    Ok(changed)
}

fn no_pad_info() -> &'static PadInfo {
    static INFO: OnceLock<PadInfo> = OnceLock::new();
    INFO.get_or_init(|| PadInfo {
        name: "No controller".to_string(),
        ..PadInfo::default()
    })
}

fn no_pad_state() -> &'static PadState {
    static STATE: OnceLock<PadState> = OnceLock::new();
    STATE.get_or_init(PadState::default)
}

fn pad_index_error(idx: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("pad index {idx} out of range"),
    )
}

fn discover_js_devices() -> Vec<String> {
    let mut paths = Vec::new();
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return paths;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() > 2 && name.starts_with("js") && name[2..].chars().all(|c| c.is_ascii_digit())
        {
            paths.push(format!("/dev/input/{name}"));
        }
    }
    paths.sort_by_key(|p| {
        p.strip_prefix("/dev/input/js")
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0)
    });
    paths
}

fn discover_keyboard_devices() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/sys/class/input") else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("event") || !name[5..].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let path = format!("/dev/input/{name}");
        let capabilities = std::fs::read_to_string(entry.path().join("device/capabilities/key"))
            .unwrap_or_default();
        // Letter keys distinguish keyboards from controllers that expose
        // Enter as a Home button on their companion evdev node. Main's named
        // proxy is authoritative even if its synthetic capability bitmap does
        // not resemble an ordinary keyboard on this kernel.
        if is_navigation_keyboard(input_device_name(&path).as_deref(), &capabilities) {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

fn is_navigation_keyboard(name: Option<&str>, capabilities: &str) -> bool {
    name == Some(MAIN_INPUT_PROXY_NAME)
        || (capability_has_key(capabilities, KEY_A) && capability_has_key(capabilities, KEY_B))
}

fn input_device_name(path: &str) -> Option<String> {
    let node = Path::new(path).file_name()?.to_str()?;
    std::fs::read_to_string(Path::new("/sys/class/input").join(node).join("device/name"))
        .ok()
        .map(|name| name.trim().to_string())
}

fn capability_has_key(words: &str, code: u16) -> bool {
    capability_has_key_with_word_bits(words, code, usize::BITS as usize)
}

fn capability_has_key_with_word_bits(words: &str, code: u16, word_bits: usize) -> bool {
    let word_index = code as usize / word_bits;
    let bit_index = code as usize % word_bits;
    words
        .split_whitespace()
        .rev()
        .nth(word_index)
        .and_then(|word| usize::from_str_radix(word, 16).ok())
        .is_some_and(|word| word & (1usize << bit_index) != 0)
}

fn merge_pad_states(states: &[&PadState]) -> PadState {
    let mut out = PadState::default();
    for s in states {
        out.dpad_up |= s.dpad_up;
        out.dpad_down |= s.dpad_down;
        out.dpad_left |= s.dpad_left;
        out.dpad_right |= s.dpad_right;
        out.btn_a |= s.btn_a;
        out.btn_b |= s.btn_b;
        out.btn_x |= s.btn_x;
        out.btn_y |= s.btn_y;
        out.btn_l |= s.btn_l;
        out.btn_r |= s.btn_r;
        out.btn_zl |= s.btn_zl;
        out.btn_zr |= s.btn_zr;
        out.btn_select |= s.btn_select;
        out.btn_start |= s.btn_start;
        out.btn_l3 |= s.btn_l3;
        out.btn_r3 |= s.btn_r3;
        out.btn_home |= s.btn_home;
        out.btn_capture |= s.btn_capture;
        if s.left_x.abs() > out.left_x.abs() {
            out.left_x = s.left_x;
        }
        if s.left_y.abs() > out.left_y.abs() {
            out.left_y = s.left_y;
        }
        if s.right_x.abs() > out.right_x.abs() {
            out.right_x = s.right_x;
        }
        if s.right_y.abs() > out.right_y.abs() {
            out.right_y = s.right_y;
        }
        if s.last_raw_event.is_some() {
            out.last_raw_event = s.last_raw_event;
        }
        if !s.last_event_label.is_empty() {
            out.last_raw = s.last_raw.clone();
            out.last_event_label = s.last_event_label.clone();
        }
    }
    out.rebuild_pressed_now();
    out
}

fn read_pad_info(js_path: &str, file: &File) -> io::Result<PadInfo> {
    use std::path::PathBuf;
    let js_node = resolve_js_node(js_path)?;
    let sys = PathBuf::from("/sys/class/input")
        .join(&js_node)
        .join("device");

    let read = |name: &str| -> String {
        std::fs::read_to_string(sys.join(name))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    let vendor_raw = read("id/vendor");
    let product_raw = read("id/product");
    let vendor_id = format_hex_id(&vendor_raw);
    let product_id = format_hex_id(&product_raw);
    let phys = read("phys");
    let usb_port = usb_port_from_sys(&sys)
        .or_else(|| usb_port_from_phys(&phys))
        .unwrap_or_else(|| "unknown".into());

    let key_words = read("capabilities/key");
    let abs_words = read("capabilities/abs");
    let evdev_key_count = count_capability_bits(&key_words);
    let evdev_abs_count = count_capability_bits(&abs_words);

    let js_buttons = js_ioctl_u8(file, 0x8001_6a12).unwrap_or(0); // JSIOCGBUTTONS
    let js_axes = js_ioctl_u8(file, 0x8001_6a11).unwrap_or(0); // JSIOCGAXES
    let name = read("name");

    // Measured on A2 receiver: JSIOCGBUTTONS=13 (js 0..12); Home=js12; no js slot for Capture.
    let capture_available = js_buttons > 13;

    Ok(PadInfo {
        name,
        vendor_id,
        product_id,
        serial: read("uniq"),
        phys,
        usb_port,
        js_buttons,
        js_axes,
        evdev_key_count,
        evdev_abs_count,
        capture_available,
    })
}

fn resolve_js_node(js_path: &str) -> io::Result<String> {
    let link = std::fs::read_link(js_path).or_else(|_| Path::new(js_path).canonicalize())?;
    let file_name = link
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| s.starts_with("js"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a js device"))?;
    Ok(file_name)
}

fn format_hex_id(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    format!("0x{:04x}", u16::from_str_radix(raw, 16).unwrap_or(0))
}

fn usb_port_from_phys(phys: &str) -> Option<String> {
    // e.g. usb-ffb40000.usb-1.3/input0 — use as-is after the host controller prefix.
    let host = phys.split('/').next()?;
    let idx = host.rfind(".usb-")?;
    Some(host[idx + 5..].to_string())
}

fn usb_port_from_sys(sys: &Path) -> Option<String> {
    let full = std::fs::canonicalize(sys).ok()?;
    for part in full.components() {
        if let std::path::Component::Normal(s) = part {
            let s = s.to_str()?;
            if s.starts_with("1-1.") && s.len() > 4 {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn count_capability_bits(words: &str) -> usize {
    words
        .split_whitespace()
        .filter_map(|w| u32::from_str_radix(w, 16).ok())
        .map(|v| v.count_ones() as usize)
        .sum()
}

fn js_ioctl_u8(file: &File, req: libc::c_ulong) -> io::Result<u8> {
    let mut buf = [0u8];
    // SAFETY: file is open for the duration of the ioctl and buf points to one
    // writable byte, which matches JSIOCGAXES/JSIOCGBUTTONS.
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), req, buf.as_mut_ptr()) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buf[0])
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let fd = file.as_raw_fd();
    // SAFETY: fcntl(F_GETFL) does not dereference Rust memory and fd is owned
    // by the borrowed File for the duration of the call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl(F_SETFL) does not dereference Rust memory and preserves the
    // open file description while only adding O_NONBLOCK.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Listen on js + evdev + hidraw for anything Capture might emit (lower layer than js API).
pub fn sniff(path: Option<&str>, secs: u64) -> io::Result<()> {
    let (js_path, info) = match path {
        Some(p) => {
            let f = OpenOptions::new().read(true).open(p)?;
            (p.to_string(), read_pad_info(p, &f)?)
        }
        None => {
            let r = PadReader::open()?;
            (r.path.clone(), r.info.clone())
        }
    };

    let event_path = resolve_event_path(&js_path)?;
    let hidraw_path = resolve_hidraw_path(&js_path);

    let mut js = OpenOptions::new().read(true).open(&js_path)?;
    let mut evdev = OpenOptions::new().read(true).open(&event_path)?;
    set_nonblocking(&js)?;
    set_nonblocking(&evdev)?;

    // Try to grab evdev so MiSTer can't swallow events (ignore failure).
    let grab: i32 = 1;
    // SAFETY: evdev is open for the duration of the ioctl and the kernel reads
    // one i32 EVIOCGRAB flag from &grab.
    let _ = unsafe {
        libc::ioctl(
            evdev.as_raw_fd(),
            0x4004_4590u32 as libc::c_ulong, // EVIOCGRAB
            &grab,
        )
    };

    let mut hidraw = hidraw_path
        .as_ref()
        .and_then(|p| OpenOptions::new().read(true).open(p).ok());

    if let Some(ref h) = hidraw {
        set_nonblocking(h)?;
    }

    crate::ui_errln!("sniff {secs}s on:");
    crate::ui_errln!("  js     {js_path}");
    crate::ui_errln!("  evdev  {event_path}");
    if let Some(ref h) = hidraw_path {
        crate::ui_errln!("  hidraw {h}");
    } else {
        crate::ui_errln!("  hidraw (not found)");
    }
    crate::ui_errln!(
        "  {} usb={} {} — press Capture (and anything else)…",
        info.name,
        info.usb_port,
        info.vendor_id
    );

    let start = std::time::Instant::now();
    let mut js_buf = [0u8; JS_EVENT_SIZE];
    let mut ev_buf = [0u8; 24];
    let mut hid_buf = [0u8; 64];
    let mut hid_idle: Option<[u8; 64]> = None;

    while start.elapsed().as_secs() < secs {
        let mut activity = false;

        match js.read(&mut js_buf) {
            Ok(n) if n >= JS_EVENT_SIZE => {
                let ty = js_buf[6] & !JS_EVENT_INIT;
                let num = js_buf[7];
                let val = i16::from_le_bytes([js_buf[4], js_buf[5]]);
                crate::ui_logln!("[js]     {} {} = {val}", js_kind(ty), num);
                activity = true;
            }
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => return Err(e),
            _ => {}
        }

        match evdev.read(&mut ev_buf) {
            Ok(n) if n >= 16 => {
                let (ty, code, val) = parse_input_event(&ev_buf[..n]);
                if ty != EV_SYN {
                    crate::ui_logln!(
                        "[evdev]  {} code={} ({}) val={val}",
                        ev_type_name(ty),
                        code,
                        ev_code_name(ty, code)
                    );
                    activity = true;
                }
            }
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => return Err(e),
            _ => {}
        }

        if let Some(ref mut h) = hidraw {
            match h.read(&mut hid_buf) {
                Ok(n) if n > 0 => {
                    let slice = &hid_buf[..n];
                    let changed = hid_idle.map(|prev| prev[..n] != slice[..]).unwrap_or(true);
                    if changed {
                        crate::ui_logln!("[hidraw] {} bytes: {}", n, hex_bytes(slice));
                        hid_idle = Some({
                            let mut a = [0u8; 64];
                            a[..n].copy_from_slice(slice);
                            a
                        });
                        activity = true;
                    }
                }
                Err(e) if e.kind() != io::ErrorKind::WouldBlock => return Err(e),
                _ => {}
            }
        }

        if !activity {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    let grab_off: i32 = 0;
    // SAFETY: evdev is still open here and the kernel reads one i32 EVIOCGRAB
    // flag from &grab_off.
    let _ = unsafe {
        libc::ioctl(
            evdev.as_raw_fd(),
            0x4004_4590u32 as libc::c_ulong,
            &grab_off,
        )
    };
    Ok(())
}

const EV_SYN: u16 = 0;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const EV_MSC: u16 = 4;

fn parse_input_event(buf: &[u8]) -> (u16, u16, i32) {
    // arm32 input_event: 8-byte timeval + type + code + value = 16 bytes
    if buf.len() >= 24 {
        (
            u16::from_le_bytes([buf[16], buf[17]]),
            u16::from_le_bytes([buf[18], buf[19]]),
            i32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
        )
    } else {
        (
            u16::from_le_bytes([buf[8], buf[9]]),
            u16::from_le_bytes([buf[10], buf[11]]),
            i32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
        )
    }
}

fn ev_type_name(ty: u16) -> &'static str {
    match ty {
        EV_SYN => "SYN",
        EV_KEY => "KEY",
        EV_REL => "REL",
        EV_ABS => "ABS",
        EV_MSC => "MSC",
        _ => "type",
    }
}

fn js_kind(ty: u8) -> &'static str {
    match ty {
        JS_EVENT_BUTTON => "btn",
        JS_EVENT_AXIS => "axis",
        _ => "?",
    }
}

fn ev_code_name(ty: u16, code: u16) -> String {
    match ty {
        EV_KEY => {
            const KEY: &[(u16, &str)] = &[
                (0x10, "KEY_Q/btn0"),
                (0x11, "KEY_W/btn1"),
                (0x12, "KEY_E/btn2"),
                (0x13, "KEY_R/btn3"),
                (0x14, "KEY_T/btn4"),
                (0x15, "KEY_Y/btn5"),
                (0x16, "KEY_U/btn6"),
                (0x17, "KEY_I/btn7"),
                (0x18, "KEY_O/btn8"),
                (0x19, "KEY_P/btn9"),
                (0x1a, "KEY_[/btn10"),
                (0x1b, "KEY_]/btn11"),
                (0x1c, "KEY_ENTER/btn12-Home"),
                (0x130, "BTN_0"),
                (0x131, "BTN_A"),
                (0x132, "BTN_B"),
                (0x133, "BTN_X"),
                (0x134, "BTN_Y"),
                (0x135, "BTN_Z"),
                (0x136, "BTN_TL"),
                (0x137, "BTN_TR"),
                (0x138, "BTN_TL2"),
                (0x139, "BTN_TR2"),
                (0x13a, "BTN_SELECT"),
                (0x13b, "BTN_START"),
                (0x13c, "BTN_MODE/Home"),
                (0x13d, "BTN_THUMBL"),
                (0x13e, "BTN_THUMBR"),
            ];
            KEY.iter()
                .find(|(c, _)| *c == code)
                .map(|(_, n)| (*n).to_string())
                .unwrap_or_else(|| format!("KEY_{code}"))
        }
        EV_ABS => {
            const ABS: &[(u16, &str)] = &[
                (0, "ABS_X"),
                (1, "ABS_Y"),
                (2, "ABS_Z"),
                (3, "ABS_RX"),
                (4, "ABS_RY"),
                (5, "ABS_RZ"),
                (16, "ABS_HAT0X"),
                (17, "ABS_HAT0Y"),
            ];
            ABS.iter()
                .find(|(c, _)| *c == code)
                .map(|(_, n)| (*n).to_string())
                .unwrap_or_else(|| format!("ABS_{code}"))
        }
        EV_MSC => format!("MSC_{code}"),
        _ => format!("code_{code}"),
    }
}

fn hex_bytes(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_event_path(js_path: &str) -> io::Result<String> {
    let js_node = resolve_js_node(js_path)?;
    let num = js_node
        .strip_prefix("js")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad js node"))?;
    Ok(format!("/dev/input/event{num}"))
}

fn resolve_hidraw_path(js_path: &str) -> Option<String> {
    let js_node = resolve_js_node(js_path).ok()?;
    let input_dev = std::fs::canonicalize(format!("/sys/class/input/{js_node}/device")).ok()?;
    // .../0003:VID:PID.N/input/inputM -> hid node is two levels up
    let hid_dev = input_dev.parent()?.parent()?;
    let hidraw_dir = hid_dev.join("hidraw");
    let entry = std::fs::read_dir(hidraw_dir).ok()?.next()?.ok()?;
    Some(format!("/dev/{}", entry.file_name().to_string_lossy()))
}

/// Raw joystick event logger for calibration.
pub fn log_js_events(path: Option<&str>, secs: u64) -> io::Result<()> {
    let reader = match path {
        Some(p) => PadReader::open_path(p)?,
        None => PadReader::open()?,
    };
    crate::ui_errln!(
        "logging {} for {secs}s (press buttons / move sticks)...",
        reader.path
    );
    let start = std::time::Instant::now();
    let mut file = reader.file;
    let mut buf = [0u8; JS_EVENT_SIZE];
    while start.elapsed().as_secs() < secs {
        match file.read(&mut buf) {
            Ok(0) => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(n) if n >= JS_EVENT_SIZE => {
                let event_type = buf[6] & !JS_EVENT_INIT;
                let number = buf[7];
                let value = i16::from_le_bytes([buf[4], buf[5]]);
                let kind = match event_type {
                    JS_EVENT_BUTTON => "btn",
                    JS_EVENT_AXIS => "axis",
                    _ => "?",
                };
                crate::ui_logln!("{kind} {number} = {value}");
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Interactive calibrate: print labeled prompts, user presses each control.
pub fn calibrate(path: Option<&str>) -> io::Result<()> {
    let reader = match path {
        Some(p) => PadReader::open_path(p)?,
        None => PadReader::open()?,
    };
    crate::ui_errln!(
        "calibrate on {} — press each control when prompted (10s timeout each)",
        reader.path
    );
    let prompts: &[(&str, fn(&PadState) -> bool)] = &[
        ("A", |s| s.btn_a),
        ("B", |s| s.btn_b),
        ("X", |s| s.btn_x),
        ("Y", |s| s.btn_y),
        ("L", |s| s.btn_l),
        ("R", |s| s.btn_r),
        ("ZL", |s| s.btn_zl),
        ("ZR", |s| s.btn_zr),
        ("Select", |s| s.btn_select),
        ("Start", |s| s.btn_start),
        ("L3", |s| s.btn_l3),
        ("R3", |s| s.btn_r3),
        ("D-pad Up", |s| s.dpad_up),
        ("D-pad Down", |s| s.dpad_down),
        ("D-pad Left", |s| s.dpad_left),
        ("D-pad Right", |s| s.dpad_right),
    ];
    let mut file = reader.file;
    let profile = reader.profile;
    let mut buf = [0u8; JS_EVENT_SIZE];
    let mut state = PadState::default();
    for (label, _) in prompts {
        crate::ui_logln!("\n>>> Press [{label}] ...");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if read_one_event(&mut file, &mut buf, profile, &mut state) {
                crate::ui_logln!("    raw: {}", state.last_raw);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    crate::ui_logln!("\ncalibrate done — move left stick, then right stick (5s each)");
    for label in ["left stick", "right stick"] {
        crate::ui_logln!("\n>>> Move [{label}] ...");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if read_one_event(&mut file, &mut buf, profile, &mut state) {
                crate::ui_logln!(
                    "    raw: {}  L=({:.2},{:.2}) R=({:.2},{:.2})",
                    state.last_raw,
                    state.left_x,
                    state.left_y,
                    state.right_x,
                    state.right_y
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    Ok(())
}

fn read_one_event(
    file: &mut File,
    buf: &mut [u8; JS_EVENT_SIZE],
    profile: InputProfile,
    state: &mut PadState,
) -> bool {
    match file.read(buf) {
        Ok(n) if n >= JS_EVENT_SIZE => {
            let event_type = buf[6] & !JS_EVENT_INIT;
            let number = buf[7];
            let value = i16::from_le_bytes([buf[4], buf[5]]);
            profile.apply_js_event(
                state,
                PadRawEvent {
                    event_type,
                    number,
                    value,
                },
                true,
            )
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_state::{JS_EVENT_AXIS, JS_EVENT_BUTTON};
    use std::io::Cursor;

    fn js_event_bytes(event_type: u8, number: u8, value: i16) -> [u8; JS_EVENT_SIZE] {
        let mut buf = [0u8; JS_EVENT_SIZE];
        let value = value.to_le_bytes();
        buf[4] = value[0];
        buf[5] = value[1];
        buf[6] = event_type;
        buf[7] = number;
        buf
    }

    fn input_event_bytes(event_type: u16, code: u16, value: i32) -> [u8; INPUT_EVENT_SIZE] {
        let mut buf = [0u8; INPUT_EVENT_SIZE];
        let offset = if INPUT_EVENT_SIZE == 24 { 16 } else { 8 };
        buf[offset..offset + 2].copy_from_slice(&event_type.to_le_bytes());
        buf[offset + 2..offset + 4].copy_from_slice(&code.to_le_bytes());
        buf[offset + 4..offset + 8].copy_from_slice(&value.to_le_bytes());
        buf
    }

    struct PendingEventsThenWouldBlock {
        bytes: Vec<u8>,
        pos: usize,
    }

    impl PendingEventsThenWouldBlock {
        fn new(bytes: Vec<u8>) -> Self {
            Self { bytes, pos: 0 }
        }
    }

    impl Read for PendingEventsThenWouldBlock {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pos >= self.bytes.len() {
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            let remaining = self.bytes.len() - self.pos;
            let len = remaining.min(buf.len());
            buf[..len].copy_from_slice(&self.bytes[self.pos..self.pos + len]);
            self.pos += len;
            Ok(len)
        }
    }

    fn empty_pool() -> PadPool {
        PadPool {
            pads: Vec::new(),
            keyboards: Vec::new(),
            mouse: None,
            user_activity: false,
            merged: PadState::default(),
            active_idx: 0,
            db: crate::controller_db::ControllerDb::load(),
            last_rescan: Instant::now(),
            input_hub: None,
        }
    }

    #[test]
    fn empty_pool_accessors_are_safe() {
        let pool = empty_pool();

        assert_eq!(pool.len(), 0);
        assert_eq!(pool.active_idx(), 0);
        assert_eq!(pool.path(), "(no controller)");
        assert_eq!(pool.path_at(3), "(no controller)");
        assert_eq!(pool.info().name, "No controller");
        assert_eq!(pool.info_at(3).name, "No controller");
        assert!(!pool.state_at(3).btn_a);
    }

    #[test]
    fn keyboard_maps_navigation_and_f12_home_aliases() {
        let events = [
            input_event_bytes(EV_KEY, KEY_LEFT, 1),
            input_event_bytes(EV_KEY, KEY_UP, 1),
            input_event_bytes(EV_KEY, KEY_A, 1),
            input_event_bytes(EV_KEY, KEY_ENTER, 1),
            input_event_bytes(EV_KEY, KEY_B, 1),
            input_event_bytes(EV_KEY, KEY_ESC, 1),
            input_event_bytes(EV_KEY, KEY_F12, 1),
        ]
        .concat();
        let mut reader = PendingEventsThenWouldBlock::new(events);
        let mut keyboard = KeyboardState::default();

        assert!(drain_keyboard_events(&mut reader, &mut keyboard, false).expect("drain keyboard"));

        let keyboard = KeyboardReader {
            file: File::open("/dev/null").expect("open /dev/null"),
            path: "test".into(),
            is_main_proxy: false,
            state: keyboard,
        };
        let mut state = PadState::default();
        keyboard.merge_into(&mut state);
        assert!(state.dpad_left);
        assert!(state.dpad_up);
        assert!(state.btn_a);
        assert!(state.btn_b);
        assert!(state.btn_home);
    }

    #[test]
    fn main_proxy_maps_resolved_menu_actions() {
        let events = [
            input_event_bytes(EV_KEY, KEY_TAB, 1),
            input_event_bytes(EV_KEY, KEY_SPACE, 1),
            input_event_bytes(EV_KEY, KEY_PAGEUP, 1),
            input_event_bytes(EV_KEY, KEY_PAGEDOWN, 1),
            input_event_bytes(EV_KEY, KEY_F10, 1),
            input_event_bytes(EV_KEY, KEY_F9, 1),
            input_event_bytes(EV_KEY, KEY_MENU, 1),
        ]
        .concat();
        let mut reader = PendingEventsThenWouldBlock::new(events);
        let mut keyboard = KeyboardState::default();

        assert!(drain_keyboard_events(&mut reader, &mut keyboard, true).expect("drain proxy"));

        let proxy = KeyboardReader {
            file: File::open("/dev/null").expect("open /dev/null"),
            path: "proxy".into(),
            is_main_proxy: true,
            state: keyboard,
        };
        let mut state = PadState::default();
        proxy.merge_into(&mut state);
        assert!(state.btn_x);
        assert!(state.btn_y);
        assert!(state.btn_l);
        assert!(state.btn_r);
        assert!(state.btn_select);
        assert!(state.btn_start);
        assert!(state.btn_home);
    }

    #[test]
    fn unbound_keyboard_key_still_reports_activity() {
        let mut reader =
            PendingEventsThenWouldBlock::new(input_event_bytes(EV_KEY, 42, 1).to_vec());
        let mut keyboard = KeyboardState::default();

        assert!(drain_keyboard_events(&mut reader, &mut keyboard, false).expect("drain keyboard"));
        assert_eq!(keyboard, KeyboardState::default());
    }

    #[test]
    fn keyboard_alias_release_keeps_action_held_by_other_alias() {
        let events = [
            input_event_bytes(EV_KEY, KEY_A, 1),
            input_event_bytes(EV_KEY, KEY_ENTER, 1),
            input_event_bytes(EV_KEY, KEY_A, 0),
        ]
        .concat();
        let mut reader = PendingEventsThenWouldBlock::new(events);
        let mut keyboard = KeyboardState::default();

        drain_keyboard_events(&mut reader, &mut keyboard, false).expect("drain keyboard");

        assert!(!keyboard.a);
        assert!(keyboard.enter);
    }

    #[test]
    fn keyboard_capability_words_are_decoded_low_word_last() {
        let mut words = vec![0usize; KEY_LEFT as usize / usize::BITS as usize + 1];
        words[KEY_LEFT as usize / usize::BITS as usize] |=
            1usize << (KEY_LEFT as usize % usize::BITS as usize);
        let capabilities = words
            .iter()
            .rev()
            .map(|word| format!("{word:x}"))
            .collect::<Vec<_>>()
            .join(" ");

        assert!(capability_has_key(&capabilities, KEY_LEFT));
        assert!(!capability_has_key(&capabilities, KEY_RIGHT));
    }

    #[test]
    fn keyboard_capability_words_decode_arm32_kernel_format() {
        let mut words = vec![0u32; KEY_LEFT as usize / 32 + 1];
        words[KEY_LEFT as usize / 32] |= 1u32 << (KEY_LEFT as usize % 32);
        let capabilities = words
            .iter()
            .rev()
            .map(|word| format!("{word:x}"))
            .collect::<Vec<_>>()
            .join(" ");

        assert!(capability_has_key_with_word_bits(
            &capabilities,
            KEY_LEFT,
            32
        ));
        assert!(!capability_has_key_with_word_bits(
            &capabilities,
            KEY_RIGHT,
            32
        ));
    }

    #[test]
    fn main_proxy_is_discovered_without_keyboard_capabilities() {
        assert!(is_navigation_keyboard(Some(MAIN_INPUT_PROXY_NAME), ""));
        assert!(!is_navigation_keyboard(Some("Gamepad"), ""));
    }

    #[test]
    fn empty_pool_setup_mutations_return_errors() {
        let mut pool = empty_pool();

        assert_eq!(
            pool.register_new_at(0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            pool.claim_existing_at(0, 0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            pool.finish_setup_at(
                0,
                "Pad".to_string(),
                crate::controller_db::ControllerKind::Gamepad,
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn merged_state_keeps_compact_raw_event_from_active_state() {
        let mut state = PadState::default();
        state.record_raw_event(JS_EVENT_BUTTON, 0, 1, false);
        state.btn_a = true;
        state.rebuild_pressed_now();

        let merged = merge_pad_states(&[&state]);

        assert_eq!(merged.last_raw_event, state.last_raw_event);
        assert!(merged.btn_a);
        assert!(merged.last_raw.is_empty());
        assert!(merged.last_event_label.is_empty());
    }

    #[test]
    fn merged_state_uses_active_debug_label_when_present() {
        let mut state = PadState::default();
        state.record_raw_event(JS_EVENT_BUTTON, 0, 1, true);
        state.set_debug_event_label(true, || "A down (js btn 0)".to_string());
        state.btn_a = true;
        state.rebuild_pressed_now();
        let merged = merge_pad_states(&[&state]);

        assert_eq!(state.last_raw, "type=1 num=0 val=1");
        assert_eq!(merged.last_raw, "type=1 num=0 val=1");
        assert_eq!(merged.last_event_label, "A down (js btn 0)");
        assert_eq!(
            merged.last_raw_event,
            Some(crate::input_state::PadRawEvent {
                event_type: JS_EVENT_BUTTON,
                number: 0,
                value: 1,
            })
        );
    }

    #[test]
    fn drain_js_events_masks_init_flag_and_applies_button_release() {
        let mut state = PadState {
            btn_a: true,
            ..PadState::default()
        };
        state.rebuild_pressed_now();
        let mut reader = PendingEventsThenWouldBlock::new(
            js_event_bytes(JS_EVENT_BUTTON | JS_EVENT_INIT, 0, 0).to_vec(),
        );

        assert!(
            drain_js_events(&mut reader, InputProfile::generic(), &mut state, true)
                .expect("drain event")
        );

        assert!(!state.btn_a);
        assert_eq!(state.last_event_label, "A up (js btn 0)");
        assert_eq!(
            state.last_raw_event,
            Some(PadRawEvent {
                event_type: JS_EVENT_BUTTON,
                number: 0,
                value: 0,
            })
        );
    }

    #[test]
    fn drain_js_events_releases_generic_hat_axes() {
        let mut state = PadState::default();
        let mut reader = PendingEventsThenWouldBlock::new(
            [
                js_event_bytes(JS_EVENT_AXIS, 6, -32767),
                js_event_bytes(JS_EVENT_AXIS, 6, 0),
                js_event_bytes(JS_EVENT_AXIS, 7, 32767),
                js_event_bytes(JS_EVENT_AXIS, 7, 0),
            ]
            .concat(),
        );

        assert!(
            drain_js_events(&mut reader, InputProfile::generic(), &mut state, false)
                .expect("drain hat events")
        );

        assert!(!state.dpad_left);
        assert!(!state.dpad_right);
        assert!(!state.dpad_up);
        assert!(!state.dpad_down);
    }

    #[test]
    fn drain_js_events_reports_eof_as_disconnect() {
        let mut state = PadState::default();
        let mut reader = Cursor::new(Vec::<u8>::new());
        let err = drain_js_events(&mut reader, InputProfile::generic(), &mut state, false)
            .expect_err("empty stream should disconnect");

        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn drain_js_events_reports_short_read_as_disconnect() {
        let mut state = PadState::default();
        let mut reader = Cursor::new(vec![0; JS_EVENT_SIZE - 1]);
        let err = drain_js_events(&mut reader, InputProfile::generic(), &mut state, false)
            .expect_err("short event should disconnect");

        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
