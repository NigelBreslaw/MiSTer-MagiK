//! Hide the framebuffer console cursor (fbcon) while we own `/dev/fb0`.
//!
//! MiSTer often runs with `fb_terminal=1`; without graphics mode the kernel VT
//! keeps drawing a blinking block cursor on top of our pixels (visible in
//! framebuffer PNG captures above the title text). `KD_GRAPHICS` stops that.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;

// linux/kd.h — not in libc bindings on all targets.
const KDSETMODE: libc::c_ulong = 0x4B3A;
const KD_GRAPHICS: libc::c_ulong = 0x01;
const KD_TEXT: libc::c_ulong = 0x00;

/// Active VT in graphics mode; restores text mode on drop.
pub struct VtGraphicsGuard {
    tty: File,
}

impl VtGraphicsGuard {
    /// Prefer the virtual-console device that owns the HDMI framebuffer.
    pub fn enter() -> io::Result<Self> {
        let tty = open_vt_tty()?;
        let fd = tty.as_raw_fd();
        if unsafe { libc::ioctl(fd, KDSETMODE, KD_GRAPHICS) } < 0 {
            return Err(io::Error::last_os_error());
        }
        eprintln!("vt: KD_GRAPHICS (fbcon cursor hidden)");
        Ok(Self { tty })
    }

    /// Best-effort; log and continue if the ioctl fails (e.g. no tty access).
    pub fn enter_or_warn() -> Option<Self> {
        match Self::enter() {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("vt: KD_GRAPHICS failed ({e}) — fbcon cursor may still blink");
                None
            }
        }
    }
}

impl Drop for VtGraphicsGuard {
    fn drop(&mut self) {
        let fd = self.tty.as_raw_fd();
        if unsafe { libc::ioctl(fd, KDSETMODE, KD_TEXT) } < 0 {
            eprintln!("vt: KD_TEXT restore failed: {}", io::Error::last_os_error());
        }
    }
}

fn open_vt_tty() -> io::Result<File> {
    for path in ["/dev/tty0", "/dev/console", "/dev/tty"] {
        match OpenOptions::new().read(true).write(true).open(path) {
            Ok(f) => return Ok(f),
            Err(e) => eprintln!("vt: open {path}: {e}"),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no /dev/tty0, /dev/console, or /dev/tty",
    ))
}
