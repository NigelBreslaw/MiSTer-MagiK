//! Small reader for Main's mapped virtual EV_KEY input device.
//!
//! This intentionally does not inspect raw joystick nodes or guess a layout.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::Path;

const EVENT_SIZE: usize = if cfg!(target_pointer_width = "64") {
    24
} else {
    16
};
const EV_KEY: u16 = 1;
const EV_SYN: u16 = 0;
const SYN_DROPPED: u16 = 3;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
const INPUT_PROXY_CAPABILITY: &str = "MISTER_MAGIK_INPUT_PROXY";
const INPUT_PROXY_PROTOCOL: &str = "MISTER_MAGIK_INPUT_PROXY_PROTOCOL";
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

pub struct MainProxyInput {
    file: File,
    await_neutral: bool,
    left: bool,
    right: bool,
}

impl MainProxyInput {
    pub fn open() -> io::Result<Self> {
        if std::env::var(INPUT_PROXY_CAPABILITY).as_deref() != Ok("1")
            || !matches!(
                std::env::var(INPUT_PROXY_PROTOCOL).as_deref(),
                Ok("2" | "3")
            )
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Main mapped input proxy is unavailable",
            ));
        }
        let path = discover_proxy().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Main mapped input device not found",
            )
        })?;
        let file = OpenOptions::new().read(true).open(path)?;
        set_nonblocking(&file)?;
        let mut input = Self {
            file,
            await_neutral: false,
            left: false,
            right: false,
        };
        input.refresh_held_snapshot();
        Ok(input)
    }

    pub fn poll(&mut self) -> io::Result<Vec<MainInputEvent>> {
        let mut events = Vec::new();
        self.poll_into(&mut events)?;
        Ok(events)
    }

    pub fn poll_into(&mut self, events: &mut Vec<MainInputEvent>) -> io::Result<()> {
        events.clear();
        let mut bytes = [0_u8; EVENT_SIZE];
        loop {
            match self.file.read_exact(&mut bytes) {
                Ok(()) => {
                    let type_offset = if EVENT_SIZE == 24 { 16 } else { 8 };
                    let code_offset = type_offset + 2;
                    let value_offset = type_offset + 4;
                    let event_type =
                        u16::from_ne_bytes([bytes[type_offset], bytes[type_offset + 1]]) & 0x7fff;
                    let code = u16::from_ne_bytes([bytes[code_offset], bytes[code_offset + 1]]);
                    let value = i32::from_ne_bytes([
                        bytes[value_offset],
                        bytes[value_offset + 1],
                        bytes[value_offset + 2],
                        bytes[value_offset + 3],
                    ]);
                    if event_type == EV_SYN && code == SYN_DROPPED {
                        self.left = false;
                        self.right = false;
                        self.await_neutral = true;
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "Main input resync required",
                        ));
                    }
                    if event_type != EV_KEY || !(value == 0 || value == 1) {
                        continue;
                    }
                    let Some(direction) = (match code {
                        KEY_LEFT => Some(MainInputDirection::Left),
                        KEY_RIGHT => Some(MainInputDirection::Right),
                        _ => None,
                    }) else {
                        continue;
                    };
                    let pressed = value == 1;
                    match direction {
                        MainInputDirection::Left => self.left = pressed,
                        MainInputDirection::Right => self.right = pressed,
                    }
                    if self.await_neutral {
                        if !self.left && !self.right {
                            self.await_neutral = false;
                        }
                        continue;
                    }
                    events.push(MainInputEvent {
                        direction,
                        phase: if pressed {
                            MainInputPhase::Pressed
                        } else {
                            MainInputPhase::Released
                        },
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }

    fn refresh_held_snapshot(&mut self) {
        let mut bits = [0_u8; 96];
        // SAFETY: the file descriptor is owned and the kernel writes only the
        // fixed EVIOCGKEY bitset represented by this buffer.
        let result = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                EVIOCGKEY,
                bits.as_mut_ptr().cast::<libc::c_void>(),
            )
        };
        if result < 0 {
            self.await_neutral = true;
            return;
        }
        self.left = bits[(KEY_LEFT / 8) as usize] & (1 << (KEY_LEFT % 8)) != 0;
        self.right = bits[(KEY_RIGHT / 8) as usize] & (1 << (KEY_RIGHT % 8)) != 0;
        self.await_neutral = self.left || self.right;
    }
}

fn discover_proxy() -> Option<String> {
    let mut paths: Vec<_> = std::fs::read_dir("/dev/input").ok()?.flatten().collect();
    paths.sort_by_key(|entry| entry.file_name());
    paths.into_iter().find_map(|entry| {
        let path = entry.path();
        if !path.file_name()?.to_string_lossy().starts_with("event") {
            return None;
        }
        let name = std::fs::read_to_string(
            Path::new("/sys/class/input")
                .join(path.file_name()?)
                .join("device/name"),
        )
        .ok()?;
        (name.trim() == "MiSTer virtual input").then(|| path.to_string_lossy().into_owned())
    })
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let fd = file.as_raw_fd();
    // SAFETY: fd belongs to file and only its status flags are changed.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd belongs to file and only O_NONBLOCK is added.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
