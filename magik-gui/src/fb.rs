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
use mister_magic_fb::framebuffer_copy;
use slint::platform::software_renderer::{PremultipliedRgbaColor, TargetPixel};
use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

const FBIO_WAITFORVSYNC: libc::c_ulong = 0x4004_4620;

/// Cutoff for glyph/text edges: coverage below this is transparent, at/above is
/// fully opaque (crisp pixel font after 2× upscale). Tune via `MISTER_GLYPH_ALPHA_THRESHOLD`.
fn glyph_alpha_threshold() -> u8 {
    static THRESHOLD: OnceLock<u8> = OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        std::env::var("MISTER_GLYPH_ALPHA_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(128)
    })
}

/// One framebuffer pixel in MiSTer's xRGB-8888 layout, stored as 0x00RRGGBB.
/// (Colours verified correct on HDMI, so no R/B swap needed despite FB_FMT_RxB.)
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Pixel(pub u32);

impl TargetPixel for Pixel {
    #[inline]
    fn blend(&mut self, c: PremultipliedRgbaColor) {
        if c.alpha < glyph_alpha_threshold() {
            return;
        }
        self.0 = ((c.red as u32) << 16) | ((c.green as u32) << 8) | (c.blue as u32);
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
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

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

/// Linux fb_fix_screeninfo subset. Keep the C layout because ioctl writes it.
#[repr(C)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 3],
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
            std::thread::sleep(std::time::Duration::from_millis(if attempt == 0 {
                0
            } else {
                200
            }));
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
            red: FbBitfield {
                offset: 0,
                length: 0,
                msb_right: 0,
            },
            green: FbBitfield {
                offset: 0,
                length: 0,
                msb_right: 0,
            },
            blue: FbBitfield {
                offset: 0,
                length: 0,
                msb_right: 0,
            },
            transp: FbBitfield {
                offset: 0,
                length: 0,
                msb_right: 0,
            },
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
        let mut fix = FbFixScreeninfo {
            id: [0; 16],
            smem_start: 0,
            smem_len: 0,
            type_: 0,
            type_aux: 0,
            visual: 0,
            xpanstep: 0,
            ypanstep: 0,
            ywrapstep: 0,
            line_length: 0,
            mmio_start: 0,
            mmio_len: 0,
            accel: 0,
            capabilities: 0,
            reserved: [0; 3],
        };
        if unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut var as *mut _) } == 0 {
            let virt = (var.yres_virtual as usize).max(var.yres as usize);
            if var.xres > 0 && var.xres as usize != w {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("fb0 width is {}, need {w}", var.xres),
                ));
            }
            if virt > 0 && virt != h {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("fb0 is {}px tall, need {h}", virt),
                ));
            }
            if var.bits_per_pixel != 0 && var.bits_per_pixel != 32 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("fb0 is {}bpp, need 32bpp", var.bits_per_pixel),
                ));
            }
            if var.red.length != 0
                && (var.red.offset, var.green.offset, var.blue.offset) != (16, 8, 0)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "fb0 channel offsets are r{} g{} b{}, expected r16 g8 b0",
                        var.red.offset, var.green.offset, var.blue.offset
                    ),
                ));
            }
        }
        if unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut fix as *mut _) } == 0 {
            let expected = w * 4;
            if fix.line_length != 0 && fix.line_length as usize != expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("fb0 stride is {} bytes, need {expected}", fix.line_length),
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
        if scale == 2 {
            let dst_w = self.w;
            let dst_h = self.h;
            let dst = self.buffer_mut();
            framebuffer_copy::copy_rect_2x_to(
                dst,
                dst_w,
                dst_h,
                0,
                src_y0 * 2,
                src,
                src_w,
                0,
                src_y0,
                src_w,
                src_y1,
            );
            return;
        }
        let dst_w = self.w;
        let dst_h = self.h;
        let dst = self.buffer_mut();
        framebuffer_copy::copy_rect_scaled_to(
            dst,
            dst_w,
            dst_h,
            0,
            src_y0 * scale,
            scale,
            src,
            src_w,
            0,
            src_y0,
            src_w,
            src_y1,
        );
    }

    /// Copy logical rect [src_x0,src_x1) × [src_y0,src_y1) from `src` into the fb.
    /// This avoids copying full-width dirty rows when Slint reports a narrow
    /// bounding box.
    pub fn copy_rect_scaled(
        &mut self,
        scale: usize,
        src: &[Pixel],
        src_w: usize,
        src_x0: usize,
        src_y0: usize,
        src_x1: usize,
        src_y1: usize,
    ) {
        if src_x1 <= src_x0 || src_y1 <= src_y0 {
            return;
        }
        let rect_w = src_x1 - src_x0;
        if rect_w * scale >= self.w * 3 / 4 {
            self.copy_rows_scaled(scale, src, src_w, src_y0, src_y1);
            return;
        }
        if scale <= 1 {
            debug_assert_eq!(src_w, self.w);
            let dst_w = self.w;
            let dst = self.buffer_mut();
            for sy in src_y0..src_y1 {
                let a = sy * dst_w + src_x0;
                let b = sy * dst_w + src_x1;
                dst[a..b].copy_from_slice(&src[a..b]);
            }
            return;
        }
        if scale == 2 {
            let dst_w = self.w;
            let dst_h = self.h;
            let dst = self.buffer_mut();
            framebuffer_copy::copy_rect_2x_to(
                dst,
                dst_w,
                dst_h,
                src_x0 * 2,
                src_y0 * 2,
                src,
                src_w,
                src_x0,
                src_y0,
                src_x1,
                src_y1,
            );
            return;
        }

        let dst_w = self.w;
        let dst_h = self.h;
        let dst = self.buffer_mut();
        framebuffer_copy::copy_rect_scaled_to(
            dst,
            dst_w,
            dst_h,
            src_x0 * scale,
            src_y0 * scale,
            scale,
            src,
            src_w,
            src_x0,
            src_y0,
            src_x1,
            src_y1,
        );
    }

    pub fn wait_vsync(&self) {
        let arg: u32 = 0;
        if unsafe { libc::ioctl(self.fb0.as_raw_fd(), FBIO_WAITFORVSYNC, &arg as *const u32) } < 0 {
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "warning: FBIO_WAITFORVSYNC failed: {}",
                    io::Error::last_os_error()
                );
            }
        }
    }

    /// Copy a dense source rectangle into the framebuffer at (x,y).
    #[cfg_attr(mister_ui_scope_launcher, allow(dead_code))]
    pub fn copy_rect_from(&mut self, x: usize, y: usize, w: usize, h: usize, src: &[Pixel]) {
        if w == 0 || h == 0 {
            return;
        }
        let dst_w = self.w;
        let x1 = (x + w).min(self.w);
        let y1 = (y + h).min(self.h);
        if x >= x1 || y >= y1 {
            return;
        }
        let copy_w = x1 - x;
        let copy_h = y1 - y;
        let dst = self.buffer_mut();
        for row in 0..copy_h {
            let src_a = row * w;
            let dst_a = (y + row) * dst_w + x;
            dst[dst_a..dst_a + copy_w].copy_from_slice(&src[src_a..src_a + copy_w]);
        }
    }

    /// Copy a logical source rectangle into an arbitrary framebuffer location,
    /// nearest-neighbour scaled by `scale`.
    #[cfg_attr(mister_ui_scope_launcher, allow(dead_code))]
    pub fn copy_rect_scaled_at(
        &mut self,
        dst_x: usize,
        dst_y: usize,
        scale: usize,
        src: &[Pixel],
        src_w: usize,
        src_h: usize,
    ) {
        if scale <= 1 {
            self.copy_rect_from(dst_x, dst_y, src_w, src_h, src);
            return;
        }
        if scale == 2 {
            let dst_w = self.w;
            let dst_h = self.h;
            let dst = self.buffer_mut();
            framebuffer_copy::copy_rect_2x_to(
                dst, dst_w, dst_h, dst_x, dst_y, src, src_w, 0, 0, src_w, src_h,
            );
            return;
        }
        let dst_w = self.w;
        let dst_h = self.h;
        let dst = self.buffer_mut();
        framebuffer_copy::copy_rect_scaled_to(
            dst, dst_w, dst_h, dst_x, dst_y, scale, src, src_w, 0, 0, src_w, src_h,
        );
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.mem as *mut libc::c_void, self.map_len);
        }
    }
}
