//! Display backend: the MiSTer HPS framebuffer via /dev/fb0.
//!
//! Key hardware fact (measured): the /dev/fb0 driver mapping is *write-combining*
//! (~700 MB/s), whereas mapping the same physical buffers through /dev/mem is
//! uncached device memory (~105 MB/s). Only buffer 0 is exposed by /dev/fb0, so
//! that is our single fast buffer. We render into cached RAM and copy only the
//! dirty rows here, right after vsync.
//!
//! /dev/fb0 also provides the FBIO_WAITFORVSYNC ioctl we pace on.

use crate::fpga::FB_SIZE_PX;
use slint::platform::software_renderer::{PremultipliedRgbaColor, TargetPixel};
use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;

const FBIO_WAITFORVSYNC: libc::c_ulong = 0x4004_4620;

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
    mem: *mut Pixel,
    map_len: usize,
    w: usize,
    h: usize,
    fb0: std::fs::File,
}

impl Display {
    pub fn open(w: usize, h: usize) -> io::Result<Self> {
        assert!(w * h <= FB_SIZE_PX as usize);
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        let map_len = w * h * 4;
        // mmap the framebuffer itself (offset 0) — this is the write-combining map.
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fb0.as_raw_fd(),
                0,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            mem: mem as *mut Pixel,
            map_len,
            w,
            h,
            fb0,
        })
    }

    /// The (single) on-screen buffer, as a mutable pixel slice.
    pub fn buffer_mut(&mut self) -> &mut [Pixel] {
        unsafe { std::slice::from_raw_parts_mut(self.mem, self.w * self.h) }
    }

    /// Copy rows [y0,y1) from `src` into the framebuffer (write-combined).
    pub fn copy_rows(&mut self, src: &[Pixel], y0: usize, y1: usize) {
        let w = self.w;
        let dst = self.buffer_mut();
        let a = y0 * w;
        let b = (y1 * w).min(dst.len());
        if b > a {
            dst[a..b].copy_from_slice(&src[a..b]);
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
