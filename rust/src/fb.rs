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

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;

/// Linux fb_var_screeninfo subset — only fields we need for sizing.
#[repr(C)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

const MODE_1080P: &str = "8888 1 1920 1080 7680";

impl Display {
    /// Request 1080p from the MiSTer_fb driver (no-op if sysfs missing).
    pub fn set_mode_1080p() {
        if let Err(e) = std::fs::write("/sys/module/MiSTer_fb/parameters/mode", MODE_1080P) {
            eprintln!("note: could not set MiSTer_fb mode: {e}");
        }
    }

    /// Open /dev/fb0, retrying while the driver comes up after cold boot / main= handoff.
    pub fn open_boot(w: usize, h: usize) -> io::Result<Self> {
        const RETRIES: u32 = 30;
        let mut last_err = io::Error::new(io::ErrorKind::Other, "no attempt");
        for attempt in 0..RETRIES {
            Self::set_mode_1080p();
            std::thread::sleep(std::time::Duration::from_millis(if attempt == 0 { 0 } else { 200 }));
            match Self::open(w, h) {
                Ok(d) => {
                    if attempt > 0 {
                        println!("display open ok after {attempt} retries");
                    }
                    return Ok(d);
                }
                Err(e) => {
                    if attempt == 0 || attempt % 5 == 0 {
                        eprintln!("display open attempt {attempt}: {e}");
                    }
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    pub fn open(w: usize, h: usize) -> io::Result<Self> {
        assert!(w * h <= FB_SIZE_PX as usize);
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        let mut var = FbVarScreeninfo {
            xres: 0,
            yres: 0,
            xres_virtual: 0,
            yres_virtual: 0,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 0,
            grayscale: 0,
            red: FbBitfield { offset: 0, length: 0, msb_right: 0 },
            green: FbBitfield { offset: 0, length: 0, msb_right: 0 },
            blue: FbBitfield { offset: 0, length: 0, msb_right: 0 },
            transp: FbBitfield { offset: 0, length: 0, msb_right: 0 },
            nonstd: 0,
            activate: 0,
            height: 0,
            width: 0,
            accel_flags: 0,
            pixclock: 0,
            left_margin: 0,
            right_margin: 0,
            upper_margin: 0,
            lower_margin: 0,
            hsync_len: 0,
            vsync_len: 0,
            sync: 0,
            vmode: 0,
            rotate: 0,
            colorspace: 0,
            reserved: [0; 4],
        };
        let fd = fb0.as_raw_fd();
        if unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut var as *mut _) } == 0 {
            let virt = (var.yres_virtual as usize).max(var.yres as usize);
            if virt > 0 && virt < h {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("fb0 is {}px tall, need {h}", virt),
                ));
            }
        }
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
    /// `src` stride is `self.w`; used when render size equals fb size.
    pub fn copy_rows(&mut self, src: &[Pixel], y0: usize, y1: usize) {
        let w = self.w;
        let dst = self.buffer_mut();
        let a = y0 * w;
        let b = (y1 * w).min(dst.len());
        if b > a {
            dst[a..b].copy_from_slice(&src[a..b]);
        }
    }

    /// Copy logical rows [src_y0, src_y1) from `src` (stride `src_w`) into the fb.
    /// When `scale == 1`, `src_w` must equal `self.w` and rows map 1:1.
    /// When `scale > 1`, each source pixel becomes a `scale`×`scale` block (nearest).
    pub fn copy_rows_scaled(
        &mut self,
        scale: usize,
        src: &[Pixel],
        src_w: usize,
        src_y0: usize,
        src_y1: usize,
    ) {
        if scale <= 1 {
            debug_assert_eq!(src_w, self.w);
            self.copy_rows(src, src_y0, src_y1);
            return;
        }
        let dst_w = self.w;
        let dst_h = self.h;
        let dst = self.buffer_mut();
        for sy in src_y0..src_y1 {
            let src_row = &src[sy * src_w..(sy + 1) * src_w];
            let py0 = sy * scale;
            for dy in 0..scale {
                let py = py0 + dy;
                if py >= dst_h {
                    break;
                }
                let dst_row = &mut dst[py * dst_w..(py + 1) * dst_w];
                for (sx, &color) in src_row.iter().enumerate() {
                    let px0 = sx * scale;
                    for dx in 0..scale {
                        let dst_x = px0 + dx;
                        if dst_x < dst_w {
                            dst_row[dst_x] = color;
                        }
                    }
                }
            }
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
