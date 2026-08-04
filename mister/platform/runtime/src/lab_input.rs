// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Narrow nonblocking joystick adapter for standalone framebuffer labs.

use mister_magik_core::input_state::{
    DirectionalState, InputProfile, JS_EVENT_AXIS, JS_EVENT_BUTTON, PadInfo, PadRawEvent, PadState,
};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

const EVENT_BYTES: usize = 8;
const JS_EVENT_INIT: u8 = 0x80;
const RESCAN_INTERVAL: Duration = Duration::from_secs(1);

struct Pad {
    path: String,
    file: File,
    profile: InputProfile,
    state: PadState,
}

pub struct FramebufferLabInput {
    pads: Vec<Pad>,
    last_rescan: Instant,
    await_neutral: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FramebufferLabState {
    pub directions: DirectionalState,
    pub button_a: bool,
    pub button_b: bool,
}

impl FramebufferLabState {
    fn is_neutral(self) -> bool {
        self.directions.is_neutral() && !self.button_a && !self.button_b
    }
}

impl Default for FramebufferLabInput {
    fn default() -> Self {
        Self::open()
    }
}

impl FramebufferLabInput {
    #[must_use]
    pub fn open() -> Self {
        let mut input = Self {
            pads: Vec::new(),
            last_rescan: Instant::now(),
            await_neutral: true,
        };
        input.rescan();
        input
    }

    pub fn poll(&mut self) -> DirectionalState {
        self.poll_state().directions
    }

    pub fn poll_state(&mut self) -> FramebufferLabState {
        if self.last_rescan.elapsed() >= RESCAN_INTERVAL {
            self.rescan();
            self.last_rescan = Instant::now();
        }
        let mut index = 0;
        while index < self.pads.len() {
            if drain(&mut self.pads[index]).is_err() {
                self.pads.remove(index);
                self.await_neutral = true;
            } else {
                index += 1;
            }
        }
        let state = merge_state(&self.pads);
        if self.await_neutral {
            if state.is_neutral() {
                self.await_neutral = false;
            }
            FramebufferLabState::default()
        } else {
            state
        }
    }

    fn rescan(&mut self) {
        for path in discover() {
            if self.pads.iter().any(|pad| pad.path == path) {
                continue;
            }
            if let Ok(pad) = open_pad(&path) {
                self.pads.push(pad);
                self.await_neutral = true;
            }
        }
    }
}

fn open_pad(path: &str) -> io::Result<Pad> {
    let file = OpenOptions::new().read(true).open(path)?;
    set_nonblocking(&file)?;
    let node = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let sys = Path::new("/sys/class/input").join(node).join("device");
    let read = |name: &str| {
        std::fs::read_to_string(sys.join(name))
            .map(|value| value.trim().to_owned())
            .unwrap_or_default()
    };
    let info = PadInfo {
        name: read("name"),
        vendor_id: format_id(&read("id/vendor")),
        product_id: format_id(&read("id/product")),
        ..PadInfo::default()
    };
    Ok(Pad {
        path: path.to_owned(),
        file,
        profile: InputProfile::guess(&info),
        state: PadState::default(),
    })
}

fn drain(pad: &mut Pad) -> io::Result<()> {
    let mut bytes = [0_u8; EVENT_BYTES];
    loop {
        match pad.file.read_exact(&mut bytes) {
            Ok(()) => {
                let event_type = bytes[6] & !JS_EVENT_INIT;
                if matches!(event_type, JS_EVENT_AXIS | JS_EVENT_BUTTON) {
                    pad.profile.apply_js_event(
                        &mut pad.state,
                        PadRawEvent {
                            event_type,
                            number: bytes[7],
                            value: i16::from_le_bytes([bytes[4], bytes[5]]),
                        },
                        false,
                    );
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) => return Err(error),
        }
    }
}

fn merge_state(pads: &[Pad]) -> FramebufferLabState {
    let mut merged = PadState::default();
    for pad in pads {
        merged.dpad_up |= pad.state.dpad_up;
        merged.dpad_down |= pad.state.dpad_down;
        merged.dpad_left |= pad.state.dpad_left;
        merged.dpad_right |= pad.state.dpad_right;
        merged.btn_a |= pad.state.btn_a;
        merged.btn_b |= pad.state.btn_b;
    }
    FramebufferLabState {
        directions: DirectionalState::from_pad(&merged),
        button_a: merged.btn_a,
        button_b: merged.btn_b,
    }
}

fn discover() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return Vec::new();
    };
    let mut paths: Vec<(u32, String)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(index) = name
            .strip_prefix("js")
            .and_then(|value| value.parse::<u32>().ok())
        {
            paths.push((index, format!("/dev/input/{name}")));
        }
    }
    paths.sort_by_key(|(index, _)| *index);
    paths.into_iter().map(|(_, path)| path).collect()
}

fn format_id(raw: &str) -> String {
    u16::from_str_radix(raw, 16)
        .map(|value| format!("0x{value:04x}"))
        .unwrap_or_default()
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let fd = file.as_raw_fd();
    // SAFETY: fd belongs to file for both fcntl calls.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: only the file status flags for the owned descriptor are changed.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
