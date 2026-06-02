//! Display backend: the MiSTer HPS framebuffer as a set of page-flippable
//! buffers in reserved FPGA DDR (0x22000000+, confirmed not kernel RAM).
//!
//! We map several full-frame buffers via /dev/mem and let the FPGA scan whichever
//! one we point it at (`fpga::fb_enable_direct(n)`). Rendering into the *back*
//! buffer then flipping at vblank means zero per-frame copy and no tearing —
//! the fix for the ~12ms uncached 8MB blit that the single-buffer path paid.
//!
//! /dev/fb0 is still opened, but only for its FBIO_WAITFORVSYNC ioctl.

use crate::fpga::{FB_ADDR, FB_SIZE_PX};
use slint::platform::software_renderer::{PremultipliedRgbaColor, TargetPixel};
use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;

const FBIO_WAITFORVSYNC: libc::c_ulong = 0x4004_4620;

const SLOT_BYTES: usize = (FB_SIZE_PX as usize) * 4; // one buffer slot
const NUM_BUFFERS: usize = 3; // buffers 0,1,2 fit in the reserved region

/// One framebuffer pixel in MiSTer's xRGB-8888 layout, stored as 0x00RRGGBB.
/// (Colours verified correct on HDMI, so no R/B swap needed despite FB_FMT_RxB.)
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Pixel(pub u32);

impl TargetPixel for Pixel {
    #[inline]
    fn blend(&mut self, c: PremultipliedRgbaColor) {
        let inv = (255 - c.alpha) as u32;
        let r = (self.0 >> 16) & 0xff;
        let g = (self.0 >> 8) & 0xff;
        let b = self.0 & 0xff;
        let r = c.red as u32 + (r * inv) / 255;
        let g = c.green as u32 + (g * inv) / 255;
        let b = c.blue as u32 + (b * inv) / 255;
        self.0 = (r << 16) | (g << 8) | b;
    }

    #[inline]
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Pixel(((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }
}

pub struct Display {
    mem: *mut u8,
    map_len: usize,
    w: usize,
    h: usize,
    fb0: std::fs::File,
}

impl Display {
    pub fn open(w: usize, h: usize) -> io::Result<Self> {
        assert!(w * h <= FB_SIZE_PX as usize);
        // /dev/fb0 is kept solely for the vsync ioctl.
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;

        let mem_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/mem")?;
        let map_len = SLOT_BYTES * NUM_BUFFERS;
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                mem_file.as_raw_fd(),
                FB_ADDR as libc::off_t,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            mem: mem as *mut u8,
            map_len,
            w,
            h,
            fb0,
        })
    }

    /// Mutable pixel slice for buffer `n`. Uses slots 1..=2 (full slots); slot 0
    /// has the +4096 params page and is left for fallback, so callers should
    /// page-flip between 1 and 2.
    pub fn buffer_mut(&mut self, n: u32) -> &mut [Pixel] {
        let off = SLOT_BYTES * (n as usize);
        unsafe {
            let ptr = self.mem.add(off) as *mut Pixel;
            std::slice::from_raw_parts_mut(ptr, self.w * self.h)
        }
    }

    pub fn wait_vsync(&self) {
        let arg: u32 = 0;
        unsafe {
            libc::ioctl(self.fb0.as_raw_fd(), FBIO_WAITFORVSYNC, &arg as *const u32);
        }
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.mem as *mut libc::c_void, self.map_len);
        }
    }
}
