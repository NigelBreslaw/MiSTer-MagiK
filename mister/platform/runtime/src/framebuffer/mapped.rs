// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Display backend: the MiSTer HPS framebuffer via /dev/fb0.
//!
//! Key hardware fact (measured): the /dev/fb0 driver mapping is *write-combining*
//! (~700 MB/s), whereas mapping the same physical buffers through /dev/mem is
//! uncached device memory (~105 MB/s). Only buffer 0 is exposed by /dev/fb0, so
//! that is our single fast buffer. We render into cached RAM and copy only the
//! dirty rows here, right after vsync.
//!
//! /dev/fb0 also provides the FBIO_WAITFORVSYNC ioctl we pace on.

use crate::boot_analytics;
use crate::framebuffer::damage::DirtyRect;
use crate::framebuffer::format::{
    RGB565_BITS_PER_PIXEL, fb_mode_format_from_bits_per_pixel, production_label, restore_mode_line,
    rgb565_mode_line, rgb565_stride_bytes,
};
use crate::framebuffer::sample::Rgb565SampleView;
use crate::framebuffer::vertical_scale::{
    Rgb565FrameView, VerticalCopyStats, VerticalRect, VerticalRgb565Transform,
};
use crate::framebuffer::vsync::{VsyncWaitStatus, wait_vsync_fd};

use slint::platform::software_renderer::{PremultipliedRgbaColor, Rgb565Pixel, TargetPixel};
use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_FRAMEBUFFER_PIXELS: usize = 1920 * 1080;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FramebufferPresentError {
    AddressOverflow { context: &'static str },
    DestinationTooShort { needed: usize, actual: usize },
    InvalidFramebufferStride { stride: usize, width: usize },
    InvalidSourceStride { stride: usize, min_stride: usize },
    SourceTooShort { needed: usize, actual: usize },
    InvalidVerticalTransform(&'static str),
}

impl std::fmt::Display for FramebufferPresentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddressOverflow { context } => write!(f, "{context} address overflow"),
            Self::DestinationTooShort { needed, actual } => {
                write!(f, "framebuffer has {actual} pixels, need {needed}")
            }
            Self::InvalidFramebufferStride { stride, width } => {
                write!(
                    f,
                    "framebuffer stride {stride} is smaller than width {width}"
                )
            }
            Self::InvalidSourceStride { stride, min_stride } => {
                write!(
                    f,
                    "source stride {stride} is smaller than required {min_stride}"
                )
            }
            Self::SourceTooShort { needed, actual } => {
                write!(f, "source has {actual} pixels, need {needed}")
            }
            Self::InvalidVerticalTransform(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for FramebufferPresentError {}

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

pub struct MappedRgb565Framebuffer {
    mem: *mut u8,
    map_len: usize,
    w: usize,
    h: usize,
    stride_pixels: usize,
    info: FbInfo,
    #[allow(dead_code)]
    fb0: std::fs::File,
}

#[derive(Clone, Copy, Debug)]
pub struct FbInfo {
    pub visible_w: usize,
    pub visible_h: usize,
    pub virtual_w: usize,
    pub virtual_h: usize,
    pub stride_bytes: usize,
    pub bits_per_pixel: u32,
    pub red_offset: u32,
    pub green_offset: u32,
    pub blue_offset: u32,
    pub transp_offset: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FbRawDiagnostics {
    pub id: String,
    pub smem_start: usize,
    pub smem_len: usize,
    pub type_: u32,
    pub type_aux: u32,
    pub visual: u32,
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
    pub line_length: usize,
    pub mmio_start: usize,
    pub mmio_len: usize,
    pub accel: u32,
    pub capabilities: u16,
    pub xres: usize,
    pub yres: usize,
    pub xres_virtual: usize,
    pub yres_virtual: usize,
    pub xoffset: usize,
    pub yoffset: usize,
    pub bits_per_pixel: u32,
    pub red_offset: u32,
    pub red_length: u32,
    pub red_msb_right: u32,
    pub green_offset: u32,
    pub green_length: u32,
    pub green_msb_right: u32,
    pub blue_offset: u32,
    pub blue_length: u32,
    pub blue_msb_right: u32,
    pub transp_offset: u32,
    pub transp_length: u32,
    pub transp_msb_right: u32,
    pub vmode: u32,
    pub rotate: u32,
    pub colorspace: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FbMmapProbe {
    pub label: &'static str,
    pub requested_len: usize,
    pub ok: bool,
    pub error: Option<String>,
}

impl FbInfo {
    pub fn log_line(self) -> String {
        format!(
            "fb0={}x{} virtual={}x{} stride={} bpp={} rgba_offsets={}/{}/{}/{}",
            self.visible_w,
            self.visible_h,
            self.virtual_w,
            self.virtual_h,
            self.stride_bytes,
            self.bits_per_pixel,
            self.red_offset,
            self.green_offset,
            self.blue_offset,
            self.transp_offset
        )
    }

    pub fn mode_line(self) -> String {
        let w = self.visible_w.max(1);
        let h = self.visible_h.max(1);
        restore_mode_line(
            fb_mode_format_from_bits_per_pixel(self.bits_per_pixel),
            w,
            h,
            self.stride_bytes,
        )
    }
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

impl FbVarScreeninfo {
    fn zeroed() -> Self {
        // SAFETY: Linux framebuffer ioctls expect this C POD struct to be
        // zero-initialized before the kernel fills it.
        unsafe { std::mem::zeroed() }
    }
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

impl FbFixScreeninfo {
    fn zeroed() -> Self {
        // SAFETY: Linux framebuffer ioctls expect this C POD struct to be
        // zero-initialized before the kernel fills it.
        unsafe { std::mem::zeroed() }
    }
}

const _: () = {
    assert!(std::mem::size_of::<Rgb565Pixel>() == std::mem::size_of::<u16>());
    assert!(std::mem::align_of::<Rgb565Pixel>() == std::mem::align_of::<u16>());
    assert!(std::mem::size_of::<FbVarScreeninfo>() == 160);
    #[cfg(target_pointer_width = "32")]
    assert!(std::mem::size_of::<FbFixScreeninfo>() == 68);
    #[cfg(target_pointer_width = "64")]
    assert!(std::mem::size_of::<FbFixScreeninfo>() == 80);
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum FramebufferVarValidationError {
    InvalidWidth {
        actual: u32,
        expected: usize,
    },
    InvalidHeight {
        actual: usize,
        expected: usize,
    },
    InvalidBitsPerPixel {
        actual: u32,
        expected: u32,
    },
    InvalidChannelOffsets {
        red: u32,
        green: u32,
        blue: u32,
        expected_red: u32,
        expected_green: u32,
        expected_blue: u32,
    },
    InvalidChannelLengths {
        red: u32,
        green: u32,
        blue: u32,
        expected_red: u32,
        expected_green: u32,
        expected_blue: u32,
    },
    InvalidMsbRight {
        red: u32,
        green: u32,
        blue: u32,
    },
}

impl std::fmt::Display for FramebufferVarValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidWidth { actual, expected } => {
                write!(f, "fb0 width is {actual}, need {expected}")
            }
            Self::InvalidHeight { actual, expected } => {
                write!(f, "fb0 is {actual}px tall, need {expected}")
            }
            Self::InvalidBitsPerPixel { actual, expected } => {
                write!(
                    f,
                    "fb0 is {actual}bpp, need {expected}bpp for {}",
                    production_label()
                )
            }
            Self::InvalidChannelOffsets {
                red,
                green,
                blue,
                expected_red,
                expected_green,
                expected_blue,
            } => write!(
                f,
                "fb0 channel offsets are r{red} g{green} b{blue}, expected r{expected_red} g{expected_green} b{expected_blue} for {}",
                production_label()
            ),
            Self::InvalidChannelLengths {
                red,
                green,
                blue,
                expected_red,
                expected_green,
                expected_blue,
            } => write!(
                f,
                "fb0 channel lengths are r{red} g{green} b{blue}, expected r{expected_red} g{expected_green} b{expected_blue} for {}",
                production_label()
            ),
            Self::InvalidMsbRight { red, green, blue } => write!(
                f,
                "fb0 RGB bitfields use msb_right r{red} g{green} b{blue}, expected all 0 for {}",
                production_label()
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FramebufferMapValidationError {
    InvalidStride {
        actual_stride_bytes: usize,
        expected_stride_bytes: usize,
    },
    MapTooShort {
        smem_len: usize,
        map_len: usize,
    },
}

impl std::fmt::Display for FramebufferMapValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidStride {
                actual_stride_bytes,
                expected_stride_bytes,
            } => write!(
                f,
                "fb0 stride is {actual_stride_bytes} bytes, need {expected_stride_bytes}"
            ),
            Self::MapTooShort { smem_len, map_len } => {
                write!(f, "fb0 memory length is {smem_len} bytes, need {map_len}")
            }
        }
    }
}

fn fb_info_from(
    var_ok: bool,
    var: &FbVarScreeninfo,
    fix_ok: bool,
    fix: &FbFixScreeninfo,
    fallback_w: usize,
    fallback_h: usize,
) -> FbInfo {
    FbInfo {
        visible_w: if var_ok && var.xres != 0 {
            var.xres as usize
        } else {
            fallback_w
        },
        visible_h: if var_ok && var.yres != 0 {
            var.yres as usize
        } else {
            fallback_h
        },
        virtual_w: if var_ok && var.xres_virtual != 0 {
            var.xres_virtual as usize
        } else {
            fallback_w
        },
        virtual_h: if var_ok && var.yres_virtual != 0 {
            var.yres_virtual as usize
        } else {
            fallback_h
        },
        stride_bytes: if fix_ok && fix.line_length != 0 {
            fix.line_length as usize
        } else {
            rgb565_stride_bytes(fallback_w)
        },
        bits_per_pixel: if var_ok && var.bits_per_pixel != 0 {
            var.bits_per_pixel
        } else {
            RGB565_BITS_PER_PIXEL
        },
        red_offset: var.red.offset,
        green_offset: var.green.offset,
        blue_offset: var.blue.offset,
        transp_offset: var.transp.offset,
    }
}

fn visible_pixels_exceed_limit(w: usize, h: usize) -> bool {
    match w.checked_mul(h) {
        Some(pixels) => pixels > MAX_FRAMEBUFFER_PIXELS,
        None => true,
    }
}

fn validate_var_screeninfo_for_rgb565(
    var: &FbVarScreeninfo,
    w: usize,
    h: usize,
) -> Result<(), FramebufferVarValidationError> {
    let virt_h = (var.yres_virtual as usize).max(var.yres as usize);
    if var.xres > 0 && var.xres as usize != w {
        return Err(FramebufferVarValidationError::InvalidWidth {
            actual: var.xres,
            expected: w,
        });
    }
    if virt_h > 0 && virt_h != h {
        return Err(FramebufferVarValidationError::InvalidHeight {
            actual: virt_h,
            expected: h,
        });
    }
    if var.bits_per_pixel != 0 && var.bits_per_pixel != RGB565_BITS_PER_PIXEL {
        return Err(FramebufferVarValidationError::InvalidBitsPerPixel {
            actual: var.bits_per_pixel,
            expected: RGB565_BITS_PER_PIXEL,
        });
    }

    let expected_offsets = (11, 5, 0);
    let reports_channel_lengths =
        var.red.length != 0 || var.green.length != 0 || var.blue.length != 0;
    if reports_channel_lengths {
        if (var.red.offset, var.green.offset, var.blue.offset) != expected_offsets {
            return Err(FramebufferVarValidationError::InvalidChannelOffsets {
                red: var.red.offset,
                green: var.green.offset,
                blue: var.blue.offset,
                expected_red: expected_offsets.0,
                expected_green: expected_offsets.1,
                expected_blue: expected_offsets.2,
            });
        }

        let expected_lengths = (5, 6, 5);
        if (var.red.length, var.green.length, var.blue.length) != expected_lengths {
            return Err(FramebufferVarValidationError::InvalidChannelLengths {
                red: var.red.length,
                green: var.green.length,
                blue: var.blue.length,
                expected_red: expected_lengths.0,
                expected_green: expected_lengths.1,
                expected_blue: expected_lengths.2,
            });
        }
    }

    if var.red.msb_right != 0 || var.green.msb_right != 0 || var.blue.msb_right != 0 {
        return Err(FramebufferVarValidationError::InvalidMsbRight {
            red: var.red.msb_right,
            green: var.green.msb_right,
            blue: var.blue.msb_right,
        });
    }

    Ok(())
}

fn validate_fix_screeninfo_for_map(
    fix_ok: bool,
    fix: &FbFixScreeninfo,
    expected_stride_bytes: usize,
    map_len: usize,
) -> Result<(), FramebufferMapValidationError> {
    if !fix_ok {
        return Ok(());
    }
    if fix.line_length != 0 && fix.line_length as usize != expected_stride_bytes {
        return Err(FramebufferMapValidationError::InvalidStride {
            actual_stride_bytes: fix.line_length as usize,
            expected_stride_bytes,
        });
    }
    if fix.smem_len != 0 && (fix.smem_len as usize) < map_len {
        return Err(FramebufferMapValidationError::MapTooShort {
            smem_len: fix.smem_len as usize,
            map_len,
        });
    }
    Ok(())
}

impl MappedRgb565Framebuffer {
    pub fn write_mister_mode_rgb565(w: usize, h: usize, stride_bytes: usize) -> io::Result<()> {
        let expected = rgb565_stride_bytes(w);
        let stride_bytes = if stride_bytes == 0 {
            expected
        } else {
            stride_bytes
        };
        std::fs::write(
            "/sys/module/MiSTer_fb/parameters/mode",
            format!("{}\n", rgb565_mode_line(w, h, stride_bytes)),
        )
    }

    pub fn restore_mister_mode(info: FbInfo) -> io::Result<()> {
        std::fs::write(
            "/sys/module/MiSTer_fb/parameters/mode",
            format!("{}\n", info.mode_line()),
        )
    }

    /// Open the framebuffer at its current kernel-reported size, without writing
    /// `/sys/module/MiSTer_fb/parameters/mode`.
    #[allow(dead_code)]
    pub fn open_current_boot() -> io::Result<Self> {
        const RETRIES: u32 = 30;
        let mut last_err = io::Error::new(io::ErrorKind::Other, "no attempt");
        for attempt in 0..RETRIES {
            boot_analytics::event("display_open_current_attempt", format!("attempt={attempt}"));
            std::thread::sleep(std::time::Duration::from_millis(if attempt == 0 {
                0
            } else {
                200
            }));
            match Self::open_current_rgb565() {
                Ok(d) => {
                    let info = d.info();
                    boot_analytics::event(
                        "display_open_current_ok",
                        format!(
                            "attempt={attempt} w={} h={}",
                            info.visible_w, info.virtual_h
                        ),
                    );
                    if attempt > 0 {
                        crate::ui_logln!("display current open ok after {attempt} retries");
                    }
                    return Ok(d);
                }
                Err(e) => {
                    boot_analytics::event(
                        "display_open_current_failed",
                        format!("attempt={attempt} error={e}"),
                    );
                    if attempt == 0 || attempt % 5 == 0 {
                        crate::ui_errln!("display current open attempt {attempt}: {e}");
                    }
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    pub fn open_current_rgb565() -> io::Result<Self> {
        let info = Self::read_fb_info()?;
        let w = info.visible_w;
        let h = info.virtual_h.max(info.visible_h);
        if w == 0 || h == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("fb0 reported invalid current size {w}x{h}"),
            ));
        }
        if visible_pixels_exceed_limit(w, h) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("fb0 current size {w}x{h} exceeds MiSTer buffer"),
            ));
        }
        Self::open_rgb565(w, h)
    }

    fn read_fb_info() -> io::Result<FbInfo> {
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        let fd = fb0.as_raw_fd();
        let mut var = FbVarScreeninfo::zeroed();
        let mut fix = FbFixScreeninfo::zeroed();
        // SAFETY: fd refers to /dev/fb0 and var/fix are full-size repr(C)
        // framebuffer structs with stable addresses for the duration of ioctl.
        let var_ok = unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut var as *mut _) } == 0;
        let fix_ok = unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut fix as *mut _) } == 0;
        if !var_ok {
            return Err(io::Error::last_os_error());
        }
        Ok(fb_info_from(var_ok, &var, fix_ok, &fix, 0, 0))
    }

    pub fn current_info() -> io::Result<FbInfo> {
        Self::read_fb_info()
    }

    #[cfg(feature = "diagnostics")]
    pub fn raw_diagnostics() -> io::Result<FbRawDiagnostics> {
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        let fd = fb0.as_raw_fd();
        let mut var = FbVarScreeninfo::zeroed();
        let mut fix = FbFixScreeninfo::zeroed();
        // SAFETY: fd refers to /dev/fb0 and var/fix are repr(C) framebuffer structs.
        if unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut var as *mut _) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd refers to /dev/fb0 and fix is a repr(C) framebuffer struct.
        if unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut fix as *mut _) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let nul = fix.id.iter().position(|b| *b == 0).unwrap_or(fix.id.len());
        let id = String::from_utf8_lossy(&fix.id[..nul]).to_string();
        Ok(FbRawDiagnostics {
            id,
            smem_start: fix.smem_start as usize,
            smem_len: fix.smem_len as usize,
            type_: fix.type_,
            type_aux: fix.type_aux,
            visual: fix.visual,
            xpanstep: fix.xpanstep,
            ypanstep: fix.ypanstep,
            ywrapstep: fix.ywrapstep,
            line_length: fix.line_length as usize,
            mmio_start: fix.mmio_start as usize,
            mmio_len: fix.mmio_len as usize,
            accel: fix.accel,
            capabilities: fix.capabilities,
            xres: var.xres as usize,
            yres: var.yres as usize,
            xres_virtual: var.xres_virtual as usize,
            yres_virtual: var.yres_virtual as usize,
            xoffset: var.xoffset as usize,
            yoffset: var.yoffset as usize,
            bits_per_pixel: var.bits_per_pixel,
            red_offset: var.red.offset,
            red_length: var.red.length,
            red_msb_right: var.red.msb_right,
            green_offset: var.green.offset,
            green_length: var.green.length,
            green_msb_right: var.green.msb_right,
            blue_offset: var.blue.offset,
            blue_length: var.blue.length,
            blue_msb_right: var.blue.msb_right,
            transp_offset: var.transp.offset,
            transp_length: var.transp.length,
            transp_msb_right: var.transp.msb_right,
            vmode: var.vmode,
            rotate: var.rotate,
            colorspace: var.colorspace,
        })
    }

    #[cfg(feature = "diagnostics")]
    pub fn probe_mmap_lengths(probes: &[(&'static str, usize)]) -> io::Result<Vec<FbMmapProbe>> {
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        let fd = fb0.as_raw_fd();
        let mut out = Vec::with_capacity(probes.len());
        for (label, requested_len) in probes {
            if *requested_len == 0 {
                out.push(FbMmapProbe {
                    label,
                    requested_len: *requested_len,
                    ok: false,
                    error: Some("zero-length mmap skipped".to_string()),
                });
                continue;
            }
            // SAFETY: fd refers to /dev/fb0; the mapping is read-only and is unmapped immediately.
            let mem = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    *requested_len,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            if mem == libc::MAP_FAILED {
                out.push(FbMmapProbe {
                    label,
                    requested_len: *requested_len,
                    ok: false,
                    error: Some(io::Error::last_os_error().to_string()),
                });
            } else {
                // SAFETY: mem/requested_len came from a successful mmap above.
                unsafe {
                    libc::munmap(mem, *requested_len);
                }
                out.push(FbMmapProbe {
                    label,
                    requested_len: *requested_len,
                    ok: true,
                    error: None,
                });
            }
        }
        Ok(out)
    }

    pub fn open_rgb565(w: usize, h: usize) -> io::Result<Self> {
        if visible_pixels_exceed_limit(w, h) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("fb0 requested size {w}x{h} exceeds MiSTer buffer"),
            ));
        }
        let expected_stride_bytes = rgb565_stride_bytes(w);
        let stride_pixels = expected_stride_bytes / std::mem::size_of::<Rgb565Pixel>();
        let map_len = expected_stride_bytes.checked_mul(h).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("fb0 requested map length overflows for {w}x{h}"),
            )
        })?;
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        let mut var = FbVarScreeninfo::zeroed();
        let fd = fb0.as_raw_fd();
        let mut fix = FbFixScreeninfo::zeroed();
        // SAFETY: fd refers to /dev/fb0 and var is a full-size repr(C)
        // framebuffer struct with a stable address for the duration of ioctl.
        let var_ok = unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut var as *mut _) } == 0;
        if var_ok {
            if let Err(e) = validate_var_screeninfo_for_rgb565(&var, w, h) {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, e.to_string()));
            }
        }
        // SAFETY: fd refers to /dev/fb0 and fix is a full-size repr(C)
        // framebuffer struct with a stable address for the duration of ioctl.
        let fix_ok = unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut fix as *mut _) } == 0;
        if let Err(e) =
            validate_fix_screeninfo_for_map(fix_ok, &fix, expected_stride_bytes, map_len)
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, e.to_string()));
        }
        let info = fb_info_from(var_ok, &var, fix_ok, &fix, w, h);
        // mmap the framebuffer itself (offset 0) — this is the write-combining map.
        // SAFETY: map_len was checked for overflow, /dev/fb0 is open read/write,
        // and fix-screeninfo validation confirms the exposed memory is large
        // enough when the driver reports smem_len.
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
        if mem.is_null() {
            // SAFETY: mem/map_len were just returned by mmap and have not been
            // stored elsewhere. Rust slices cannot be formed from a null base.
            unsafe {
                libc::munmap(mem, map_len);
            }
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "fb0 mmap returned a null address",
            ));
        }
        Ok(Self {
            mem: mem as *mut u8,
            map_len,
            w,
            h,
            stride_pixels,
            info,
            fb0,
        })
    }

    pub fn info(&self) -> FbInfo {
        self.info
    }

    pub fn width(&self) -> usize {
        self.w
    }

    pub fn height(&self) -> usize {
        self.h
    }

    fn buffer_565_mut(&mut self) -> &mut [Rgb565Pixel] {
        debug_assert_eq!(
            self.map_len,
            self.stride_pixels * self.h * std::mem::size_of::<Rgb565Pixel>()
        );
        debug_assert_eq!(
            self.mem.align_offset(std::mem::align_of::<Rgb565Pixel>()),
            0
        );
        // SAFETY: self.mem is a live mmap for map_len bytes, Rgb565Pixel is
        // layout-compatible with u16, and &mut self prevents aliasing through
        // another MappedRgb565Framebuffer slice.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.mem.cast::<Rgb565Pixel>(),
                self.stride_pixels * self.h,
            )
        }
    }

    fn buffer_565(&self) -> &[Rgb565Pixel] {
        debug_assert_eq!(
            self.map_len,
            self.stride_pixels * self.h * std::mem::size_of::<Rgb565Pixel>()
        );
        debug_assert_eq!(
            self.mem.align_offset(std::mem::align_of::<Rgb565Pixel>()),
            0
        );
        // SAFETY: self.mem is a live mmap for map_len bytes and Rgb565Pixel is
        // layout-compatible with u16.
        unsafe {
            std::slice::from_raw_parts(self.mem.cast::<Rgb565Pixel>(), self.stride_pixels * self.h)
        }
    }

    fn sample_view(&self) -> Rgb565SampleView<'_> {
        Rgb565SampleView::new(self.buffer_565(), self.w, self.h, self.stride_pixels)
    }

    pub fn clear_black(&mut self) {
        self.buffer_565_mut().fill(Rgb565Pixel(0));
    }

    pub fn right_edge_signature(&self, cols: usize) -> (u64, u32) {
        self.sample_view().right_edge_signature(cols)
    }

    pub fn left_edge_signature(&self, cols: usize) -> (u64, u32) {
        self.sample_view().left_edge_signature(cols)
    }

    pub fn top_edge_signature(&self, rows: usize) -> (u64, u32) {
        self.sample_view().top_edge_signature(rows)
    }

    pub fn bottom_edge_signature(&self, rows: usize) -> (u64, u32) {
        self.sample_view().bottom_edge_signature(rows)
    }

    pub fn sampled_signature(&self) -> (u64, u32) {
        self.sample_view().sampled_signature()
    }

    #[allow(dead_code)]
    pub fn rect_sampled_signature(
        &self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        step: usize,
    ) -> (u64, u32) {
        self.sample_view().rect_sampled_signature(x, y, w, h, step)
    }

    pub fn record_visual_sample(&self, label: &str) {
        self.sample_view().record_visual_sample(label);
    }

    pub fn present_rows_565(
        &mut self,
        src: &[Rgb565Pixel],
        y0: usize,
        y1: usize,
    ) -> Result<(), FramebufferPresentError> {
        let fb_w = self.w;
        let fb_h = self.h;
        let dst_stride = self.stride_pixels;
        let dst = self.buffer_565_mut();
        present_rows_565_to(dst, fb_w, fb_h, dst_stride, src, y0, y1)
    }

    pub fn present_rect_565(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
    ) -> Result<(), FramebufferPresentError> {
        let fb_w = self.w;
        let fb_h = self.h;
        let dst_stride = self.stride_pixels;
        let dst = self.buffer_565_mut();
        present_rect_565_to(dst, fb_w, fb_h, dst_stride, x, y, w, h, src)
    }

    pub fn present_vertical_rect_565(
        &mut self,
        source: Rgb565FrameView<'_, Rgb565Pixel>,
        rect: DirtyRect,
    ) -> Result<Option<VerticalCopyStats>, FramebufferPresentError> {
        let transform = VerticalRgb565Transform::new(self.w, source.height, self.h)
            .map_err(FramebufferPresentError::InvalidVerticalTransform)?;
        let stride_pixels = self.stride_pixels;
        transform
            .copy_rect(
                source,
                VerticalRect {
                    x0: rect.x0,
                    y0: rect.y0,
                    x1: rect.x1,
                    y1: rect.y1,
                },
                self.buffer_565_mut(),
                stride_pixels,
            )
            .map_err(FramebufferPresentError::InvalidVerticalTransform)
    }

    pub fn frame_view_565(&self) -> Rgb565FrameView<'_, Rgb565Pixel> {
        Rgb565FrameView {
            pixels: self.buffer_565(),
            width: self.w,
            height: self.h,
            stride_pixels: self.stride_pixels,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn present_rect_565_strided(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
        src_stride: usize,
        src_x: usize,
        src_y: usize,
    ) -> Result<(), FramebufferPresentError> {
        let fb_w = self.w;
        let fb_h = self.h;
        let dst_stride = self.stride_pixels;
        let dst = self.buffer_565_mut();
        present_rect_565_strided_to(
            dst, fb_w, fb_h, dst_stride, x, y, w, h, src, src_stride, src_x, src_y,
        )
    }

    #[allow(dead_code)]
    pub fn wait_vsync_status(&self) -> VsyncWaitStatus {
        wait_vsync_fd(self.fb0.as_raw_fd())
    }

    #[allow(dead_code)]
    pub fn wait_vsync(&self) -> VsyncWaitStatus {
        let status = self.wait_vsync_status();
        if let VsyncWaitStatus::Error { message, .. } = &status {
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                crate::ui_errln!("warning: FBIO_WAITFORVSYNC failed: {message}");
            }
        }
        status
    }
}

impl Drop for MappedRgb565Framebuffer {
    fn drop(&mut self) {
        // SAFETY: mem/map_len come from a successful mmap in open_rgb565 and are
        // unmapped exactly once when the owning framebuffer object is dropped.
        unsafe {
            libc::munmap(self.mem as *mut libc::c_void, self.map_len);
        }
    }
}

fn present_rows_565_to(
    dst: &mut [Rgb565Pixel],
    fb_w: usize,
    fb_h: usize,
    dst_stride: usize,
    src: &[Rgb565Pixel],
    y0: usize,
    y1: usize,
) -> Result<(), FramebufferPresentError> {
    let y0 = y0.min(fb_h);
    let y1 = y1.min(fb_h);
    if y1 <= y0 || fb_w == 0 {
        return Ok(());
    }

    ensure_framebuffer_stride(dst_stride, fb_w)?;
    let src_needed = row_extent_len(y0, y1 - y0, fb_w, 0, fb_w, "source")?;
    let dst_needed = row_extent_len(y0, y1 - y0, dst_stride, 0, fb_w, "framebuffer")?;
    ensure_destination_len(dst, dst_needed)?;
    ensure_source_len(src, src_needed)?;
    for y in y0..y1 {
        let src_a = y * fb_w;
        let dst_a = y * dst_stride;
        dst[dst_a..dst_a + fb_w].copy_from_slice(&src[src_a..src_a + fb_w]);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn present_rect_565_to(
    dst: &mut [Rgb565Pixel],
    fb_w: usize,
    fb_h: usize,
    dst_stride: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    src: &[Rgb565Pixel],
) -> Result<(), FramebufferPresentError> {
    if w == 0 || h == 0 || fb_w == 0 || fb_h == 0 {
        return Ok(());
    }

    let x1 = x.saturating_add(w).min(fb_w);
    let y1 = y.saturating_add(h).min(fb_h);
    if x >= x1 || y >= y1 {
        return Ok(());
    }

    let copy_w = x1 - x;
    let copy_h = y1 - y;
    ensure_framebuffer_stride(dst_stride, fb_w)?;
    let src_needed = row_extent_len(0, copy_h, w, 0, copy_w, "source")?;
    let dst_needed = row_extent_len(y, copy_h, dst_stride, x, copy_w, "framebuffer")?;
    ensure_source_len(src, src_needed)?;
    ensure_destination_len(dst, dst_needed)?;

    for row in 0..copy_h {
        let src_a = row * w;
        let dst_a = (y + row) * dst_stride + x;
        dst[dst_a..dst_a + copy_w].copy_from_slice(&src[src_a..src_a + copy_w]);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn present_rect_565_strided_to(
    dst: &mut [Rgb565Pixel],
    fb_w: usize,
    fb_h: usize,
    dst_stride: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    src: &[Rgb565Pixel],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
) -> Result<(), FramebufferPresentError> {
    if w == 0 || h == 0 || fb_w == 0 || fb_h == 0 {
        return Ok(());
    }

    let x1 = x.saturating_add(w).min(fb_w);
    let y1 = y.saturating_add(h).min(fb_h);
    if x >= x1 || y >= y1 {
        return Ok(());
    }

    let copy_w = x1 - x;
    let copy_h = y1 - y;
    ensure_framebuffer_stride(dst_stride, fb_w)?;
    let min_stride = src_x.saturating_add(copy_w);
    if src_stride < min_stride {
        return Err(FramebufferPresentError::InvalidSourceStride {
            stride: src_stride,
            min_stride,
        });
    }

    let src_needed = row_extent_len(src_y, copy_h, src_stride, src_x, copy_w, "source")?;
    let dst_needed = row_extent_len(y, copy_h, dst_stride, x, copy_w, "framebuffer")?;
    ensure_source_len(src, src_needed)?;
    ensure_destination_len(dst, dst_needed)?;

    for row in 0..copy_h {
        let src_a = (src_y + row) * src_stride + src_x;
        let dst_a = (y + row) * dst_stride + x;
        dst[dst_a..dst_a + copy_w].copy_from_slice(&src[src_a..src_a + copy_w]);
    }
    Ok(())
}

fn row_extent_len(
    y: usize,
    rows: usize,
    stride: usize,
    x: usize,
    width: usize,
    context: &'static str,
) -> Result<usize, FramebufferPresentError> {
    if rows == 0 || width == 0 {
        return Ok(0);
    }
    let last_y = y
        .checked_add(rows - 1)
        .ok_or(FramebufferPresentError::AddressOverflow { context })?;
    last_y
        .checked_mul(stride)
        .and_then(|base| base.checked_add(x))
        .and_then(|base| base.checked_add(width))
        .ok_or(FramebufferPresentError::AddressOverflow { context })
}

fn ensure_framebuffer_stride(stride: usize, width: usize) -> Result<(), FramebufferPresentError> {
    if stride < width {
        Err(FramebufferPresentError::InvalidFramebufferStride { stride, width })
    } else {
        Ok(())
    }
}

fn ensure_destination_len(
    dst: &[Rgb565Pixel],
    needed: usize,
) -> Result<(), FramebufferPresentError> {
    if dst.len() < needed {
        Err(FramebufferPresentError::DestinationTooShort {
            needed,
            actual: dst.len(),
        })
    } else {
        Ok(())
    }
}

fn ensure_source_len(src: &[Rgb565Pixel], needed: usize) -> Result<(), FramebufferPresentError> {
    if src.len() < needed {
        Err(FramebufferPresentError::SourceTooShort {
            needed,
            actual: src.len(),
        })
    } else {
        Ok(())
    }
}

pub fn pixel_to_rgb565(pixel: Pixel) -> Rgb565Pixel {
    let p = pixel.0 & 0x00ff_ffff;
    <Rgb565Pixel as TargetPixel>::from_rgb((p >> 16) as u8, (p >> 8) as u8, p as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb_info(bits_per_pixel: u32, stride_bytes: usize) -> FbInfo {
        FbInfo {
            visible_w: 960,
            visible_h: 540,
            virtual_w: 960,
            virtual_h: 540,
            stride_bytes,
            bits_per_pixel,
            red_offset: if bits_per_pixel == 16 { 11 } else { 16 },
            green_offset: if bits_per_pixel == 16 { 5 } else { 8 },
            blue_offset: 0,
            transp_offset: 0,
        }
    }

    fn fix_info(line_length: u32, smem_len: u32) -> FbFixScreeninfo {
        FbFixScreeninfo {
            line_length,
            smem_len,
            ..FbFixScreeninfo::zeroed()
        }
    }

    fn var_info() -> FbVarScreeninfo {
        FbVarScreeninfo {
            xres: 960,
            yres: 540,
            xres_virtual: 960,
            yres_virtual: 540,
            bits_per_pixel: RGB565_BITS_PER_PIXEL,
            red: FbBitfield {
                offset: 11,
                length: 5,
                msb_right: 0,
            },
            green: FbBitfield {
                offset: 5,
                length: 6,
                msb_right: 0,
            },
            blue: FbBitfield {
                offset: 0,
                length: 5,
                msb_right: 0,
            },
            ..FbVarScreeninfo::zeroed()
        }
    }

    #[test]
    fn mode_line_preserves_rgb565_framebuffer_mode() {
        assert_eq!(fb_info(16, 1920).mode_line(), "565 1 960 540 1920");
    }

    #[test]
    fn mode_line_preserves_non_rgb565_framebuffer_mode_numerically() {
        assert_eq!(fb_info(32, 3840).mode_line(), "32 1 960 540 3840");
    }

    #[test]
    fn present_rect_565_rejects_short_source() {
        let mut dst = vec![Rgb565Pixel(0); 4 * 4];
        let src = vec![Rgb565Pixel(1); 5];

        let err =
            present_rect_565_strided_to(&mut dst, 4, 4, 4, 1, 1, 3, 2, &src, 3, 0, 0).unwrap_err();

        assert_eq!(
            err,
            FramebufferPresentError::SourceTooShort {
                needed: 6,
                actual: 5
            }
        );
    }

    #[test]
    fn present_rect_565_rejects_invalid_stride() {
        let mut dst = vec![Rgb565Pixel(0); 4 * 4];
        let src = vec![Rgb565Pixel(1); 4];

        let err =
            present_rect_565_strided_to(&mut dst, 4, 4, 4, 0, 0, 2, 2, &src, 1, 0, 0).unwrap_err();

        assert_eq!(
            err,
            FramebufferPresentError::InvalidSourceStride {
                stride: 1,
                min_stride: 2
            }
        );
    }

    #[test]
    fn present_rect_565_clips_right_and_bottom_edges() {
        let mut dst = vec![Rgb565Pixel(0); 4 * 3];
        let src = vec![
            Rgb565Pixel(1),
            Rgb565Pixel(2),
            Rgb565Pixel(3),
            Rgb565Pixel(4),
            Rgb565Pixel(5),
            Rgb565Pixel(6),
        ];

        present_rect_565_strided_to(&mut dst, 4, 3, 4, 2, 1, 4, 3, &src, 4, 0, 0).unwrap();

        assert_eq!(
            dst,
            vec![
                Rgb565Pixel(0),
                Rgb565Pixel(0),
                Rgb565Pixel(0),
                Rgb565Pixel(0),
                Rgb565Pixel(0),
                Rgb565Pixel(0),
                Rgb565Pixel(1),
                Rgb565Pixel(2),
                Rgb565Pixel(0),
                Rgb565Pixel(0),
                Rgb565Pixel(5),
                Rgb565Pixel(6),
            ]
        );
    }

    #[test]
    fn present_rows_565_uses_padded_framebuffer_stride() {
        let fb_w = 961;
        let fb_h = 2;
        let dst_stride = rgb565_stride_bytes(fb_w) / std::mem::size_of::<Rgb565Pixel>();
        assert_eq!(dst_stride, 968);

        let sentinel = Rgb565Pixel(0xffff);
        let mut dst = vec![sentinel; dst_stride * fb_h];
        let src = (0..fb_w * fb_h)
            .map(|i| Rgb565Pixel(i as u16))
            .collect::<Vec<_>>();

        present_rows_565_to(&mut dst, fb_w, fb_h, dst_stride, &src, 0, fb_h).unwrap();

        assert_eq!(&dst[0..fb_w], &src[0..fb_w]);
        assert!(dst[fb_w..dst_stride].iter().all(|p| *p == sentinel));
        assert_eq!(&dst[dst_stride..dst_stride + fb_w], &src[fb_w..fb_w * fb_h]);
        assert!(
            dst[dst_stride + fb_w..dst_stride * fb_h]
                .iter()
                .all(|p| *p == sentinel)
        );
    }

    #[test]
    fn present_rect_565_uses_padded_framebuffer_stride() {
        let fb_w = 961;
        let fb_h = 2;
        let dst_stride = rgb565_stride_bytes(fb_w) / std::mem::size_of::<Rgb565Pixel>();
        let sentinel = Rgb565Pixel(0xffff);
        let mut dst = vec![sentinel; dst_stride * fb_h];
        let src = vec![
            Rgb565Pixel(1),
            Rgb565Pixel(2),
            Rgb565Pixel(3),
            Rgb565Pixel(4),
            Rgb565Pixel(5),
            Rgb565Pixel(6),
            Rgb565Pixel(7),
            Rgb565Pixel(8),
        ];

        present_rect_565_to(&mut dst, fb_w, fb_h, dst_stride, 959, 0, 4, 2, &src).unwrap();

        assert_eq!(dst[959], Rgb565Pixel(1));
        assert_eq!(dst[960], Rgb565Pixel(2));
        assert!(dst[961..dst_stride].iter().all(|p| *p == sentinel));
        assert_eq!(dst[dst_stride + 959], Rgb565Pixel(5));
        assert_eq!(dst[dst_stride + 960], Rgb565Pixel(6));
        assert!(
            dst[dst_stride + 961..dst_stride * fb_h]
                .iter()
                .all(|p| *p == sentinel)
        );
    }

    #[test]
    fn present_rect_565_rejects_invalid_framebuffer_stride() {
        let mut dst = vec![Rgb565Pixel(0); 4];
        let src = vec![Rgb565Pixel(1); 4];

        let err = present_rect_565_to(&mut dst, 4, 1, 3, 0, 0, 4, 1, &src).unwrap_err();

        assert_eq!(
            err,
            FramebufferPresentError::InvalidFramebufferStride {
                stride: 3,
                width: 4
            }
        );
    }

    #[test]
    fn present_rect_565_rejects_short_destination() {
        let mut dst = vec![Rgb565Pixel(0); 15];
        let src = vec![Rgb565Pixel(1); 1];

        let err = present_rect_565_to(&mut dst, 4, 4, 4, 3, 3, 1, 1, &src).unwrap_err();

        assert_eq!(
            err,
            FramebufferPresentError::DestinationTooShort {
                needed: 16,
                actual: 15
            }
        );
    }

    #[test]
    fn var_info_validation_accepts_rgb565_layout() {
        let var = var_info();

        assert_eq!(validate_var_screeninfo_for_rgb565(&var, 960, 540), Ok(()));
    }

    #[test]
    fn var_info_validation_rejects_wrong_channel_lengths() {
        let mut var = var_info();
        var.green.length = 5;

        assert_eq!(
            validate_var_screeninfo_for_rgb565(&var, 960, 540),
            Err(FramebufferVarValidationError::InvalidChannelLengths {
                red: 5,
                green: 5,
                blue: 5,
                expected_red: 5,
                expected_green: 6,
                expected_blue: 5,
            })
        );
    }

    #[test]
    fn var_info_validation_rejects_nonzero_msb_right() {
        let mut var = var_info();
        var.blue.msb_right = 1;

        assert_eq!(
            validate_var_screeninfo_for_rgb565(&var, 960, 540),
            Err(FramebufferVarValidationError::InvalidMsbRight {
                red: 0,
                green: 0,
                blue: 1,
            })
        );
    }

    #[test]
    fn fix_info_validation_accepts_unreported_memory_length() {
        let fix = fix_info(1920, 0);

        assert_eq!(
            validate_fix_screeninfo_for_map(true, &fix, 1920, 1920 * 540),
            Ok(())
        );
    }

    #[test]
    fn fix_info_validation_accepts_sufficient_memory_length() {
        let fix = fix_info(1920, 1920 * 540);

        assert_eq!(
            validate_fix_screeninfo_for_map(true, &fix, 1920, 1920 * 540),
            Ok(())
        );
    }

    #[test]
    fn fix_info_validation_rejects_short_memory_length() {
        let fix = fix_info(1920, 1024);

        assert_eq!(
            validate_fix_screeninfo_for_map(true, &fix, 1920, 1920 * 540),
            Err(FramebufferMapValidationError::MapTooShort {
                smem_len: 1024,
                map_len: 1920 * 540,
            })
        );
    }

    #[test]
    fn fix_info_validation_rejects_stride_mismatch() {
        let fix = fix_info(3840, 3840 * 540);

        assert_eq!(
            validate_fix_screeninfo_for_map(true, &fix, 1920, 1920 * 540),
            Err(FramebufferMapValidationError::InvalidStride {
                actual_stride_bytes: 3840,
                expected_stride_bytes: 1920,
            })
        );
    }

    #[test]
    fn framebuffer_unsafe_layout_assumptions_are_explicit() {
        assert_eq!(
            std::mem::size_of::<Rgb565Pixel>(),
            std::mem::size_of::<u16>()
        );
        assert_eq!(
            std::mem::align_of::<Rgb565Pixel>(),
            std::mem::align_of::<u16>()
        );
        assert_eq!(std::mem::size_of::<FbVarScreeninfo>(), 160);
        #[cfg(target_pointer_width = "32")]
        assert_eq!(std::mem::size_of::<FbFixScreeninfo>(), 68);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(std::mem::size_of::<FbFixScreeninfo>(), 80);
    }
}
