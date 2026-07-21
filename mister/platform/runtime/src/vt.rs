// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Hide the framebuffer console cursor (fbcon) while we own `/dev/fb0`.
//!
//! MiSTer often runs with `fb_terminal=1`; without graphics mode the kernel VT
//! keeps drawing a blinking block cursor on top of our pixels (visible in
//! framebuffer PNG captures above the title text). `KD_GRAPHICS` stops that.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;

use crate::boot_analytics;

// linux/kd.h — not in libc bindings on all targets.
const KDSETMODE: libc::c_ulong = 0x4B3A;
const KD_GRAPHICS: libc::c_ulong = 0x01;
const KD_TEXT: libc::c_ulong = 0x00;

/// Active VT in graphics mode; restores text mode on drop.
pub struct VtGraphicsGuard {
    tty: File,
    io: Box<dyn VtIo>,
}

trait VtIo {
    fn open(&self, path: &'static str) -> io::Result<File>;
    fn set_mode(&self, tty: &File, mode: libc::c_ulong) -> io::Result<()>;
    fn record_events(&self) -> bool {
        true
    }
}

struct SystemVtIo;

impl VtIo for SystemVtIo {
    fn open(&self, path: &'static str) -> io::Result<File> {
        OpenOptions::new().read(true).write(true).open(path)
    }

    fn set_mode(&self, tty: &File, mode: libc::c_ulong) -> io::Result<()> {
        // SAFETY: tty is an open tty file descriptor; KDSETMODE takes an
        // integer mode value and does not dereference Rust memory.
        if unsafe { libc::ioctl(tty.as_raw_fd(), KDSETMODE, mode) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl VtGraphicsGuard {
    /// Prefer the virtual-console device that owns the HDMI framebuffer.
    pub fn enter() -> io::Result<Self> {
        Self::enter_with(Box::new(SystemVtIo))
    }

    fn enter_with(io: Box<dyn VtIo>) -> io::Result<Self> {
        if io.record_events() {
            boot_analytics::event("vt_graphics_attempt", "open_tty");
        }
        let (tty, path) = open_vt_tty_with(io.as_ref())?;
        if let Err(e) = io.set_mode(&tty, KD_GRAPHICS) {
            if io.record_events() {
                boot_analytics::event("vt_graphics_result", format!("ok=0 path={path} error={e}"));
            }
            return Err(e);
        }
        if io.record_events() {
            boot_analytics::event("vt_graphics_result", format!("ok=1 path={path}"));
        }
        crate::ui_errln!("vt: KD_GRAPHICS (fbcon cursor hidden)");
        Ok(Self { tty, io })
    }

    /// Best-effort; log and continue if the ioctl fails (e.g. no tty access).
    pub fn enter_or_warn() -> Option<Self> {
        match Self::enter() {
            Ok(g) => Some(g),
            Err(e) => {
                crate::ui_errln!("vt: KD_GRAPHICS failed ({e}) — fbcon cursor may still blink");
                None
            }
        }
    }
}

impl Drop for VtGraphicsGuard {
    fn drop(&mut self) {
        if let Err(error) = self.io.set_mode(&self.tty, KD_TEXT) {
            crate::ui_errln!("vt: KD_TEXT restore failed: {error}");
        }
    }
}

fn open_vt_tty_with(io: &dyn VtIo) -> io::Result<(File, &'static str)> {
    for path in ["/dev/tty0", "/dev/console", "/dev/tty"] {
        match io.open(path) {
            Ok(f) => return Ok((f, path)),
            Err(e) => {
                if io.record_events() {
                    boot_analytics::event("vt_open_failed", format!("path={path} error={e}"));
                }
                crate::ui_errln!("vt: open {path}: {e}");
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no /dev/tty0, /dev/console, or /dev/tty",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct FakeState {
        opens: Vec<&'static str>,
        open_results: VecDeque<io::Result<()>>,
        modes: Vec<libc::c_ulong>,
        mode_results: VecDeque<io::Result<()>>,
    }

    struct FakeVtIo {
        state: Rc<RefCell<FakeState>>,
        file_path: std::path::PathBuf,
    }

    impl VtIo for FakeVtIo {
        fn open(&self, path: &'static str) -> io::Result<File> {
            let mut state = self.state.borrow_mut();
            state.opens.push(path);
            state.open_results.pop_front().unwrap_or(Ok(()))?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.file_path)
        }

        fn set_mode(&self, _tty: &File, mode: libc::c_ulong) -> io::Result<()> {
            let mut state = self.state.borrow_mut();
            state.modes.push(mode);
            state.mode_results.pop_front().unwrap_or(Ok(()))
        }

        fn record_events(&self) -> bool {
            false
        }
    }

    fn fake(state: Rc<RefCell<FakeState>>) -> (FakeVtIo, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-vt-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"tty fixture").unwrap();
        (
            FakeVtIo {
                state,
                file_path: path.clone(),
            },
            path,
        )
    }

    #[test]
    fn tty_open_uses_bounded_fallback_order() {
        let state = Rc::new(RefCell::new(FakeState {
            open_results: VecDeque::from([
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "tty0")),
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "console")),
                Ok(()),
            ]),
            ..FakeState::default()
        }));
        let (fake, path) = fake(Rc::clone(&state));

        let (_, selected) = open_vt_tty_with(&fake).unwrap();

        assert_eq!(selected, "/dev/tty");
        assert_eq!(
            state.borrow().opens,
            ["/dev/tty0", "/dev/console", "/dev/tty"]
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn tty_open_reports_not_found_after_every_candidate_fails() {
        let state = Rc::new(RefCell::new(FakeState {
            open_results: VecDeque::from_iter(
                (0..3).map(|_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))),
            ),
            ..FakeState::default()
        }));
        let (fake, path) = fake(Rc::clone(&state));

        let error = open_vt_tty_with(&fake).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(state.borrow().opens.len(), 3);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn guard_enters_graphics_and_restores_text_on_drop() {
        let state = Rc::new(RefCell::new(FakeState::default()));
        let (fake, path) = fake(Rc::clone(&state));

        let guard = VtGraphicsGuard::enter_with(Box::new(fake)).unwrap();
        assert_eq!(state.borrow().modes, [KD_GRAPHICS]);
        drop(guard);
        assert_eq!(state.borrow().modes, [KD_GRAPHICS, KD_TEXT]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_graphics_mode_does_not_attempt_text_restore() {
        let state = Rc::new(RefCell::new(FakeState {
            mode_results: VecDeque::from([Err(io::Error::other("ioctl failed"))]),
            ..FakeState::default()
        }));
        let (fake, path) = fake(Rc::clone(&state));

        let error = match VtGraphicsGuard::enter_with(Box::new(fake)) {
            Ok(_) => panic!("graphics mode should fail"),
            Err(error) => error,
        };

        assert_eq!(error.to_string(), "ioctl failed");
        assert_eq!(state.borrow().modes, [KD_GRAPHICS]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn text_restore_failure_is_best_effort() {
        let state = Rc::new(RefCell::new(FakeState {
            mode_results: VecDeque::from([Ok(()), Err(io::Error::other("restore failed"))]),
            ..FakeState::default()
        }));
        let (fake, path) = fake(Rc::clone(&state));

        drop(VtGraphicsGuard::enter_with(Box::new(fake)).unwrap());

        assert_eq!(state.borrow().modes, [KD_GRAPHICS, KD_TEXT]);
        std::fs::remove_file(path).unwrap();
    }
}
