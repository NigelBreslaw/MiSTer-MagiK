// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded nonblocking reader for Main's already-mapped virtual EV_KEY device.
//! No raw joystick fallback, grab, configuration or repeat generation.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;

const EVENT_SIZE: usize = if cfg!(target_pointer_width = "64") {
    24
} else {
    16
};
pub const INPUT_BATCH_CAPACITY: usize = 64;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
// Linux EVIOCGKEY(64), matching the buffer passed below exactly.
const EVIOCGKEY: libc::c_ulong = 0x8040_4518;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MainInputDirection {
    Left,
    Right,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MainInputPhase {
    Pressed,
    Released,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MainInputEvent {
    pub direction: MainInputDirection,
    pub phase: MainInputPhase,
}
struct KeyState {
    held: [bool; 2],
    await_neutral: bool,
}
impl KeyState {
    fn new(held: [bool; 2]) -> Self {
        Self {
            held,
            await_neutral: held[0] || held[1],
        }
    }
    fn event(&mut self, kind: u16, code: u16, value: i32) -> io::Result<Option<MainInputEvent>> {
        if kind == 0 && code == 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Main input SYN_DROPPED; reopen required",
            ));
        }
        if kind != 1 || !matches!(value, 0 | 1) {
            return Ok(None);
        }
        let (index, direction) = match code {
            KEY_LEFT => (0, MainInputDirection::Left),
            KEY_RIGHT => (1, MainInputDirection::Right),
            _ => return Ok(None),
        };
        let pressed = value == 1;
        let changed = self.held[index] != pressed;
        self.held[index] = pressed;
        if self.await_neutral {
            self.await_neutral = self.held[0] || self.held[1];
            return Ok(None);
        }
        Ok(changed.then_some(MainInputEvent {
            direction,
            phase: if pressed {
                MainInputPhase::Pressed
            } else {
                MainInputPhase::Released
            },
        }))
    }
}
pub struct MainProxyInput {
    file: File,
    keys: KeyState,
    partial: [u8; EVENT_SIZE],
    filled: usize,
}
impl MainProxyInput {
    pub fn open() -> io::Result<Self> {
        if !cfg!(target_os = "linux") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Main mapped input proxy v2/v3 unavailable",
            ));
        }
        verify_capability()?;
        let path = discover_proxy().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Main mapped input device not found",
            )
        })?;
        let file = OpenOptions::new().read(true).open(path)?;
        set_nonblocking(&file)?;
        let mut bits = [0_u8; 64];
        // SAFETY: owned evdev descriptor; kernel writes at most the encoded 64 bytes.
        if unsafe { libc::ioctl(file.as_raw_fd(), EVIOCGKEY, bits.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let held = [KEY_LEFT, KEY_RIGHT].map(|key| bits[key as usize / 8] & (1 << (key % 8)) != 0);
        Ok(Self {
            file,
            keys: KeyState::new(held),
            partial: [0; EVENT_SIZE],
            filled: 0,
        })
    }
    pub fn ready(&self) -> bool {
        !self.keys.await_neutral
    }
    /// At most 64 kernel events per call. Caller reuses a capacity-64 vector.
    /// Any transport/desync error discards the entire untrusted batch.
    pub fn poll_into(&mut self, events: &mut Vec<MainInputEvent>) -> io::Result<()> {
        events.clear();
        let result = self.drain(events);
        if result.is_err() {
            events.clear();
        }
        result
    }
    fn drain(&mut self, events: &mut Vec<MainInputEvent>) -> io::Result<()> {
        for _ in 0..INPUT_BATCH_CAPACITY {
            match self.file.read(&mut self.partial[self.filled..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Main input disconnected",
                    ));
                }
                Ok(count) => self.filled += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
            if self.filled == EVENT_SIZE {
                self.filled = 0;
                let (kind, code, value) = parse_event(&self.partial);
                if let Some(event) = self.keys.event(kind, code, value)? {
                    events.push(event);
                }
            }
        }
        Ok(())
    }
}

fn verify_capability() -> io::Result<()> {
    let enabled = std::env::var("MISTER_MAGIK_INPUT_PROXY").ok();
    let protocol = std::env::var("MISTER_MAGIK_INPUT_PROXY_PROTOCOL").ok();
    if enabled.is_some() || protocol.is_some() {
        return if enabled.as_deref() == Some("1") && matches!(protocol.as_deref(), Some("2" | "3"))
        {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "unsupported Main input environment capability",
            ))
        };
    }
    // Mini is launched by the isolated service, which currently forwards these
    // variables only for the real app. Read the same existing Main authority
    // used by that service; never infer mapping from raw joystick capabilities.
    let text = std::fs::read_to_string("/tmp/mister-magik/main-status.json")?;
    verify_status_capability(&text)
}

fn verify_status_capability(text: &str) -> io::Result<()> {
    let status: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if status["schema"] == "mister-magik-main-status-v2"
        && status["command_channel"] == "ready"
        && matches!(status["input_proxy_protocol"].as_u64(), Some(2 | 3))
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Main status does not advertise mapped input v2/v3",
        ))
    }
}
fn parse_event(bytes: &[u8]) -> (u16, u16, i32) {
    let offset = bytes.len() - 8;
    (
        u16::from_ne_bytes(bytes[offset..offset + 2].try_into().expect("event type")),
        u16::from_ne_bytes(
            bytes[offset + 2..offset + 4]
                .try_into()
                .expect("event code"),
        ),
        i32::from_ne_bytes(bytes[offset + 4..].try_into().expect("event value")),
    )
}
fn discover_proxy() -> Option<String> {
    let mut paths: Vec<_> = std::fs::read_dir("/sys/class/input")
        .ok()?
        .flatten()
        .collect();
    paths.sort_by_key(|entry| entry.file_name());
    paths.into_iter().find_map(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let index = name.strip_prefix("event")?;
        if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let device_name = std::fs::read_to_string(entry.path().join("device/name")).ok()?;
        (device_name.trim() == "MiSTer virtual input").then(|| format!("/dev/input/{name}"))
    })
}
fn set_nonblocking(file: &File) -> io::Result<()> {
    // SAFETY: both fcntl operations use an owned, live descriptor.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mini_reads_existing_main_capability_without_environment_forwarding() {
        for protocol in [2, 3] {
            assert!(verify_status_capability(&format!(r#"{{"schema":"mister-magik-main-status-v2","command_channel":"ready","input_proxy_protocol":{protocol}}}"#)).is_ok());
        }
        for status in [
            r#"{}"#,
            r#"{"schema":"mister-magik-main-status-v2","command_channel":"ready","input_proxy_protocol":1}"#,
            r#"{"schema":"mister-magik-main-status-v2","command_channel":"starting","input_proxy_protocol":2}"#,
        ] {
            assert!(verify_status_capability(status).is_err());
        }
    }
    #[test]
    fn neutral_start_delivers_first_tap_and_ignores_repeats() {
        let mut keys = KeyState::new([false; 2]);
        assert_eq!(
            keys.event(1, KEY_RIGHT, 1).unwrap().unwrap().phase,
            MainInputPhase::Pressed
        );
        assert!(keys.event(1, KEY_RIGHT, 1).unwrap().is_none());
        assert!(keys.event(1, KEY_RIGHT, 2).unwrap().is_none());
        assert!(keys.event(1, KEY_RIGHT, -1).unwrap().is_none());
        assert_eq!(
            keys.event(1, KEY_RIGHT, 0).unwrap().unwrap().phase,
            MainInputPhase::Released
        );
    }
    #[test]
    fn inherited_hold_requires_all_directions_neutral() {
        let mut keys = KeyState::new([true, true]);
        assert!(keys.event(1, KEY_LEFT, 0).unwrap().is_none());
        assert!(keys.await_neutral);
        assert!(keys.event(1, KEY_RIGHT, 0).unwrap().is_none());
        assert!(!keys.await_neutral);
        assert!(keys.event(1, KEY_LEFT, 1).unwrap().is_some());
    }
    #[test]
    fn desync_is_a_reset_not_a_silent_lost_release() {
        let mut keys = KeyState::new([false; 2]);
        keys.event(1, KEY_RIGHT, 1).unwrap();
        assert!(keys.event(0, 3, 0).is_err());
        assert!(keys.event(4, 0, 12).unwrap().is_none());
    }
    #[test]
    fn parses_both_linux_event_abis() {
        for size in [16, 24] {
            let mut bytes = vec![0; size];
            bytes[size - 8..size - 6].copy_from_slice(&1_u16.to_ne_bytes());
            bytes[size - 6..size - 4].copy_from_slice(&KEY_LEFT.to_ne_bytes());
            bytes[size - 4..].copy_from_slice(&1_i32.to_ne_bytes());
            assert_eq!(parse_event(&bytes), (1, KEY_LEFT, 1));
        }
    }

    #[test]
    fn partial_reads_are_retained_and_desync_discards_batch() {
        use std::io::Write;
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;
        let (reader, mut writer) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let mut input = MainProxyInput {
            file: File::from(OwnedFd::from(reader)),
            keys: KeyState::new([false; 2]),
            partial: [0; EVENT_SIZE],
            filled: 0,
        };
        let mut bytes = [0_u8; EVENT_SIZE];
        bytes[EVENT_SIZE - 8..EVENT_SIZE - 6].copy_from_slice(&1_u16.to_ne_bytes());
        bytes[EVENT_SIZE - 6..EVENT_SIZE - 4].copy_from_slice(&KEY_RIGHT.to_ne_bytes());
        bytes[EVENT_SIZE - 4..].copy_from_slice(&1_i32.to_ne_bytes());
        let mut events = Vec::with_capacity(INPUT_BATCH_CAPACITY);
        writer.write_all(&bytes[..7]).unwrap();
        input.poll_into(&mut events).unwrap();
        assert!(events.is_empty());
        writer.write_all(&bytes[7..]).unwrap();
        input.poll_into(&mut events).unwrap();
        assert_eq!(events.len(), 1);
        bytes[EVENT_SIZE - 4..].copy_from_slice(&0_i32.to_ne_bytes());
        writer.write_all(&bytes).unwrap();
        bytes[EVENT_SIZE - 8..EVENT_SIZE - 6].copy_from_slice(&0_u16.to_ne_bytes());
        bytes[EVENT_SIZE - 6..EVENT_SIZE - 4].copy_from_slice(&3_u16.to_ne_bytes());
        writer.write_all(&bytes).unwrap();
        assert!(input.poll_into(&mut events).is_err());
        assert!(events.is_empty());
    }
}
