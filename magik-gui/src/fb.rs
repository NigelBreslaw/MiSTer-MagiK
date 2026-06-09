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
use mister_magik_fb::framebuffer_copy;
use mister_magik_fb::vsync_pacer::VsyncPaceSource;

use crate::boot_analytics;
use slint::platform::software_renderer::{PremultipliedRgbaColor, TargetPixel};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const FBIO_WAITFORVSYNC: libc::c_ulong = 0x4004_4620;
const DEFAULT_VSYNC_FALLBACK_US: u64 = 16_667;
const PAL_VSYNC_FALLBACK_US: u64 = 20_000;
const VSYNC_GRACE_US: u64 = 1_500;
const PERIOD_ALPHA_NUM: u64 = 1;
const PERIOD_ALPHA_DEN: u64 = 8;

#[derive(Clone, Debug)]
pub enum VsyncWaitStatus {
    Hit { wait_us: u64, at: Instant },
    Timeout { wait_us: u64 },
    Error { wait_us: u64, message: String },
}

#[derive(Clone, Debug)]
pub struct VsyncPace {
    pub source: VsyncPaceSource,
    pub wait_us: u64,
    pub period_us: u64,
    pub miss_streak: u32,
    pub message: Option<String>,
}

pub struct VsyncPacer {
    rx: Receiver<VsyncWaitStatus>,
    period_us: u64,
    last_hit_at: Option<Instant>,
    last_frame_at: Instant,
    miss_streak: u32,
    max_miss_streak: u32,
    observed_max_miss_streak: u32,
    hits: u64,
    timeouts: u64,
    errors: u64,
    fallback_frames: u64,
}

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
    info: FbInfo,
    #[allow(dead_code)]
    fb0: std::fs::File,
}

fn wait_vsync_fd(fd: std::os::unix::io::RawFd) -> VsyncWaitStatus {
    let arg: u32 = 0;
    let start = Instant::now();
    let rc = unsafe { libc::ioctl(fd, FBIO_WAITFORVSYNC, &arg as *const u32) };
    let wait_us = start.elapsed().as_micros() as u64;
    let at = Instant::now();
    if rc == 0 {
        return VsyncWaitStatus::Hit { wait_us, at };
    }

    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ETIMEDOUT) {
        VsyncWaitStatus::Timeout { wait_us }
    } else {
        VsyncWaitStatus::Error {
            wait_us,
            message: err.to_string(),
        }
    }
}

impl VsyncPacer {
    pub fn from_env() -> Self {
        let period_us = configured_fallback_period_us();
        let max_miss_streak = std::env::var("MISTER_VSYNC_DEGRADED_MISSES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("mister-vsync".into())
            .spawn(move || {
                let fb0 = match OpenOptions::new().read(true).write(true).open("/dev/fb0") {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = tx.send(VsyncWaitStatus::Error {
                            wait_us: 0,
                            message: format!("open /dev/fb0: {e}"),
                        });
                        return;
                    }
                };
                loop {
                    if tx.send(wait_vsync_fd(fb0.as_raw_fd())).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn vsync worker");

        Self {
            rx,
            period_us,
            last_hit_at: None,
            last_frame_at: Instant::now(),
            miss_streak: 0,
            max_miss_streak,
            observed_max_miss_streak: 0,
            hits: 0,
            timeouts: 0,
            errors: 0,
            fallback_frames: 0,
        }
    }

    pub fn period_us(&self) -> u64 {
        self.period_us
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn timeouts(&self) -> u64 {
        self.timeouts
    }

    pub fn errors(&self) -> u64 {
        self.errors
    }

    pub fn fallback_frames(&self) -> u64 {
        self.fallback_frames
    }

    pub fn max_miss_streak(&self) -> u32 {
        self.observed_max_miss_streak
    }

    pub fn wait(&mut self) -> VsyncPace {
        let deadline = Duration::from_micros(self.period_us + VSYNC_GRACE_US);
        let status = self
            .drain_ready()
            .or_else(|| self.rx.recv_timeout(deadline).ok());

        match status {
            Some(VsyncWaitStatus::Hit { wait_us, at }) => {
                self.record_hit(at);
                self.last_frame_at = at;
                VsyncPace {
                    source: VsyncPaceSource::Vsync,
                    wait_us,
                    period_us: self.period_us,
                    miss_streak: self.miss_streak,
                    message: None,
                }
            }
            Some(VsyncWaitStatus::Timeout { wait_us }) => {
                self.timeouts += 1;
                self.fallback_after_miss(VsyncPaceSource::Timeout, wait_us, None)
            }
            Some(VsyncWaitStatus::Error {
                wait_us, message, ..
            }) => {
                self.errors += 1;
                self.fallback_after_miss(VsyncPaceSource::Error, wait_us, Some(message))
            }
            None => self.fallback_after_miss(VsyncPaceSource::Fallback, self.period_us, None),
        }
    }

    fn drain_ready(&mut self) -> Option<VsyncWaitStatus> {
        let mut latest = None;
        while let Ok(status) = self.rx.try_recv() {
            latest = Some(status);
        }
        latest
    }

    fn record_hit(&mut self, at: Instant) {
        self.hits += 1;
        self.miss_streak = 0;
        if let Some(prev) = self.last_hit_at {
            let observed = at.saturating_duration_since(prev).as_micros() as u64;
            if (8_000..=25_000).contains(&observed) {
                self.period_us = ((self.period_us * (PERIOD_ALPHA_DEN - PERIOD_ALPHA_NUM))
                    + observed * PERIOD_ALPHA_NUM)
                    / PERIOD_ALPHA_DEN;
            }
        }
        self.last_hit_at = Some(at);
    }

    fn fallback_after_miss(
        &mut self,
        source: VsyncPaceSource,
        wait_us: u64,
        message: Option<String>,
    ) -> VsyncPace {
        self.miss_streak += 1;
        self.observed_max_miss_streak = self.observed_max_miss_streak.max(self.miss_streak);
        self.fallback_frames += 1;

        let target = self.last_frame_at + Duration::from_micros(self.period_us);
        let now = Instant::now();
        if target > now {
            thread::sleep(target - now);
        }
        self.last_frame_at = Instant::now();

        if self.miss_streak == self.max_miss_streak {
            boot_analytics::event(
                "vsync_degraded",
                format!(
                    "miss_streak={} period_us={} source={}",
                    self.miss_streak,
                    self.period_us,
                    source.label()
                ),
            );
        }

        VsyncPace {
            source,
            wait_us,
            period_us: self.period_us,
            miss_streak: self.miss_streak,
            message,
        }
    }
}

fn configured_fallback_period_us() -> u64 {
    if let Some(period_us) = std::env::var("MISTER_VSYNC_FALLBACK_HZ")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|hz| *hz > 1.0)
        .map(|hz| (1_000_000.0 / hz).round() as u64)
    {
        return period_us;
    }

    if mister_ini_menu_pal_enabled() {
        PAL_VSYNC_FALLBACK_US
    } else {
        DEFAULT_VSYNC_FALLBACK_US
    }
}

fn mister_ini_menu_pal_enabled() -> bool {
    let Ok(ini) = std::fs::read_to_string("/media/fat/MiSTer.ini") else {
        return false;
    };
    ini.lines().any(|line| {
        let line = line.split(';').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        key.trim().eq_ignore_ascii_case("menu_pal") && value.trim() == "1"
    })
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
        let stride = if self.stride_bytes != 0 {
            self.stride_bytes
        } else {
            w * 4
        };
        format!("8888 1 {w} {h} {stride}")
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
        unsafe { std::mem::zeroed() }
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
            fallback_w * 4
        },
        bits_per_pixel: if var_ok && var.bits_per_pixel != 0 {
            var.bits_per_pixel
        } else {
            32
        },
        red_offset: var.red.offset,
        green_offset: var.green.offset,
        blue_offset: var.blue.offset,
        transp_offset: var.transp.offset,
    }
}

fn pixels_as_u32(src: &[Pixel]) -> &[u32] {
    unsafe { std::slice::from_raw_parts(src.as_ptr().cast::<u32>(), src.len()) }
}

fn pixels_as_u32_mut(dst: &mut [Pixel]) -> &mut [u32] {
    unsafe { std::slice::from_raw_parts_mut(dst.as_mut_ptr().cast::<u32>(), dst.len()) }
}

impl Display {
    pub fn write_mister_mode(w: usize, h: usize, stride_bytes: usize) -> io::Result<()> {
        std::fs::write(
            "/sys/module/MiSTer_fb/parameters/mode",
            format!("8888 1 {w} {h} {stride_bytes}\n"),
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
    #[cfg_attr(mister_ui_scope_launcher, allow(dead_code))]
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
            match Self::open_current() {
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
                        println!("display current open ok after {attempt} retries");
                    }
                    return Ok(d);
                }
                Err(e) => {
                    boot_analytics::event(
                        "display_open_current_failed",
                        format!("attempt={attempt} error={e}"),
                    );
                    if attempt == 0 || attempt % 5 == 0 {
                        eprintln!("display current open attempt {attempt}: {e}");
                    }
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    pub fn open_current() -> io::Result<Self> {
        let info = Self::read_fb_info()?;
        let w = info.visible_w;
        let h = info.virtual_h.max(info.visible_h);
        if w == 0 || h == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("fb0 reported invalid current size {w}x{h}"),
            ));
        }
        if w * h > FB_SIZE_PX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("fb0 current size {w}x{h} exceeds MiSTer buffer"),
            ));
        }
        Self::open(w, h)
    }

    fn read_fb_info() -> io::Result<FbInfo> {
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        let fd = fb0.as_raw_fd();
        let mut var = FbVarScreeninfo::zeroed();
        let mut fix = FbFixScreeninfo::zeroed();
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

    pub fn open(w: usize, h: usize) -> io::Result<Self> {
        assert!(w * h <= FB_SIZE_PX as usize);
        let fb0 = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        let mut var = FbVarScreeninfo::zeroed();
        let fd = fb0.as_raw_fd();
        let mut fix = FbFixScreeninfo::zeroed();
        let var_ok = unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut var as *mut _) } == 0;
        if var_ok {
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
        let fix_ok = unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut fix as *mut _) } == 0;
        if fix_ok {
            let expected = w * 4;
            if fix.line_length != 0 && fix.line_length as usize != expected {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("fb0 stride is {} bytes, need {expected}", fix.line_length),
                ));
            }
        }
        let info = fb_info_from(var_ok, &var, fix_ok, &fix, w, h);
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

    /// The (single) on-screen buffer, as a mutable pixel slice.
    pub fn buffer_mut(&mut self) -> &mut [Pixel] {
        unsafe { std::slice::from_raw_parts_mut(self.mem, self.w * self.h) }
    }

    #[cfg_attr(mister_ui_scope_launcher, allow(dead_code))]
    pub fn buffer_u32_mut(&mut self) -> &mut [u32] {
        pixels_as_u32_mut(self.buffer_mut())
    }

    #[cfg_attr(mister_ui_scope_launcher, allow(dead_code))]
    pub fn clear(&mut self, color: Pixel) {
        self.buffer_mut().fill(color);
    }

    pub fn right_edge_signature(&self, cols: usize) -> (u64, u32) {
        self.vertical_edge_signature(self.w.saturating_sub(cols), self.w, cols)
    }

    pub fn left_edge_signature(&self, cols: usize) -> (u64, u32) {
        self.vertical_edge_signature(0, cols.min(self.w), cols)
    }

    pub fn top_edge_signature(&self, rows: usize) -> (u64, u32) {
        self.horizontal_edge_signature(0, rows.min(self.h), rows)
    }

    pub fn bottom_edge_signature(&self, rows: usize) -> (u64, u32) {
        self.horizontal_edge_signature(self.h.saturating_sub(rows), self.h, rows)
    }

    pub fn sampled_signature(&self) -> (u64, u32) {
        let pixels = unsafe { std::slice::from_raw_parts(self.mem, self.w * self.h) };
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut nonzero = 0u32;
        let step = 16usize;
        for y in (0..self.h).step_by(step) {
            let row = y * self.w;
            for x in (0..self.w).step_by(step) {
                let p = pixels[row + x].0 & 0x00ff_ffff;
                if p != 0 {
                    nonzero += 1;
                }
                hash ^= p as u64;
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
        }
        (hash, nonzero)
    }

    #[cfg_attr(mister_ui_scope_launcher, allow(dead_code))]
    pub fn rect_sampled_signature(
        &self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        step: usize,
    ) -> (u64, u32) {
        let pixels = unsafe { std::slice::from_raw_parts(self.mem, self.w * self.h) };
        let x1 = x.saturating_add(w).min(self.w);
        let y1 = y.saturating_add(h).min(self.h);
        let step = step.max(1);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut nonzero = 0u32;

        for yy in (y..y1).step_by(step) {
            let row = yy * self.w;
            for xx in (x..x1).step_by(step) {
                let p = pixels[row + xx].0 & 0x00ff_ffff;
                if p != 0 {
                    nonzero += 1;
                }
                hash ^= p as u64;
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
        }

        (hash, nonzero)
    }

    pub fn record_visual_sample(&self, label: &str) {
        if !boot_analytics::enabled() {
            return;
        }

        let sample = self.visual_sample();
        boot_analytics::event(
            "fb_visual_sample",
            format!(
                "label={} class={} samples={} nonzero={} blackish={} color_min={:06x} color_max={:06x} transitions={} hash={:016x}",
                label,
                sample.classification,
                sample.samples,
                sample.nonzero,
                sample.blackish,
                sample.color_min,
                sample.color_max,
                sample.transitions,
                sample.hash
            ),
        );

        let path = "/tmp/mister-magik-visual-samples.tsv";
        let needs_header = std::fs::metadata(path)
            .map(|m| m.len() == 0)
            .unwrap_or(true);
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            if needs_header {
                let _ = writeln!(
                    f,
                    "boot_ms\tlabel\tclass\tsamples\tnonzero\tblackish\tcolor_min\tcolor_max\ttransitions\thash"
                );
            }
            let _ = writeln!(
                f,
                "{}\t{}\t{}\t{}\t{}\t{}\t{:06x}\t{:06x}\t{}\t{:016x}",
                boot_ms(),
                sanitize_tsv(label),
                sample.classification,
                sample.samples,
                sample.nonzero,
                sample.blackish,
                sample.color_min,
                sample.color_max,
                sample.transitions,
                sample.hash
            );
        }
    }

    fn vertical_edge_signature(&self, x0: usize, x1: usize, min_cols: usize) -> (u64, u32) {
        let pixels = unsafe { std::slice::from_raw_parts(self.mem, self.w * self.h) };
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut nonzero = 0u32;
        let x_end = x1.max(x0 + min_cols.min(self.w - x0)).min(self.w);
        for y in 0..self.h {
            let row = y * self.w;
            for x in x0..x_end {
                let p = pixels[row + x].0;
                if p != 0 {
                    nonzero += 1;
                }
                hash ^= p as u64;
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
        }
        (hash, nonzero)
    }

    fn horizontal_edge_signature(&self, y0: usize, y1: usize, min_rows: usize) -> (u64, u32) {
        let pixels = unsafe { std::slice::from_raw_parts(self.mem, self.w * self.h) };
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut nonzero = 0u32;
        let y_end = y1.max(y0 + min_rows.min(self.h - y0)).min(self.h);
        for y in y0..y_end {
            let row = y * self.w;
            for x in 0..self.w {
                let p = pixels[row + x].0;
                if p != 0 {
                    nonzero += 1;
                }
                hash ^= p as u64;
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
        }
        (hash, nonzero)
    }

    fn visual_sample(&self) -> VisualSample {
        let pixels = unsafe { std::slice::from_raw_parts(self.mem, self.w * self.h) };
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut samples = 0u32;
        let mut nonzero = 0u32;
        let mut blackish = 0u32;
        let mut transitions = 0u32;
        let mut color_min = 0x00ff_ffffu32;
        let mut color_max = 0u32;
        let mut prev: Option<u32> = None;
        let step = 16usize;
        for y in (0..self.h).step_by(step) {
            let row = y * self.w;
            for x in (0..self.w).step_by(step) {
                let p = pixels[row + x].0 & 0x00ff_ffff;
                samples += 1;
                if p != 0 {
                    nonzero += 1;
                }
                let r = (p >> 16) & 0xff;
                let g = (p >> 8) & 0xff;
                let b = p & 0xff;
                if r < 8 && g < 8 && b < 8 {
                    blackish += 1;
                }
                color_min = color_min.min(p);
                color_max = color_max.max(p);
                if let Some(prev) = prev {
                    if color_distance(prev, p) > 96 {
                        transitions += 1;
                    }
                }
                prev = Some(p);
                hash ^= p as u64;
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
        }
        let nonzero_pct = pct(nonzero, samples);
        let blackish_pct = pct(blackish, samples);
        let transition_pct = pct(transitions, samples.saturating_sub(1).max(1));
        let classification = if blackish_pct >= 95 {
            "mostly_black"
        } else if nonzero_pct >= 20 && transition_pct >= 35 {
            "static_like"
        } else if nonzero_pct >= 5 {
            "slint_like"
        } else {
            "unknown"
        };
        VisualSample {
            hash,
            samples,
            nonzero,
            blackish,
            color_min,
            color_max,
            transitions,
            classification,
        }
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

    /// Copy logical rect [src_x0,src_x1) × [src_y0,src_y1) from `src` into the fb.
    /// This avoids copying full-width dirty rows when Slint reports a narrow
    /// bounding box.
    pub fn copy_rect(
        &mut self,
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
        if src_x0 == 0 && src_x1 == self.w && src_w == self.w {
            self.copy_rows(src, src_y0, src_y1);
            return;
        }
        debug_assert_eq!(src_w, self.w);
        let dst_w = self.w;
        let dst = self.buffer_mut();
        for sy in src_y0..src_y1 {
            let a = sy * dst_w + src_x0;
            let b = sy * dst_w + src_x1;
            dst[a..b].copy_from_slice(&src[a..b]);
        }
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
                eprintln!("warning: FBIO_WAITFORVSYNC failed: {message}");
            }
        }
        status
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
            framebuffer_copy::copy_rect_2x_u32_to(
                pixels_as_u32_mut(dst),
                dst_w,
                dst_h,
                dst_x,
                dst_y,
                pixels_as_u32(src),
                src_w,
                0,
                0,
                src_w,
                src_h,
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

    #[cfg_attr(mister_ui_scope_launcher, allow(dead_code))]
    pub fn copy_u32_rect_scaled_at(
        &mut self,
        dst_x: usize,
        dst_y: usize,
        scale: usize,
        src: &[u32],
        src_w: usize,
        src_h: usize,
    ) {
        if scale == 0 || src_w == 0 || src_h == 0 {
            return;
        }
        let dst_w = self.w;
        let dst_h = self.h;
        let dst = pixels_as_u32_mut(self.buffer_mut());
        if scale == 1 {
            for sy in 0..src_h {
                let dy = dst_y + sy;
                if dy >= dst_h {
                    break;
                }
                let copy_w = src_w.min(dst_w.saturating_sub(dst_x));
                if copy_w == 0 {
                    continue;
                }
                let src_a = sy * src_w;
                let dst_a = dy * dst_w + dst_x;
                dst[dst_a..dst_a + copy_w].copy_from_slice(&src[src_a..src_a + copy_w]);
            }
            return;
        }
        if scale == 2 {
            framebuffer_copy::copy_rect_2x_u32_to(
                dst, dst_w, dst_h, dst_x, dst_y, src, src_w, 0, 0, src_w, src_h,
            );
            return;
        }
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

struct VisualSample {
    hash: u64,
    samples: u32,
    nonzero: u32,
    blackish: u32,
    color_min: u32,
    color_max: u32,
    transitions: u32,
    classification: &'static str,
}

fn pct(n: u32, d: u32) -> u32 {
    if d == 0 {
        0
    } else {
        n.saturating_mul(100) / d
    }
}

fn color_distance(a: u32, b: u32) -> u32 {
    let ar = (a >> 16) & 0xff;
    let ag = (a >> 8) & 0xff;
    let ab = a & 0xff;
    let br = (b >> 16) & 0xff;
    let bg = (b >> 8) & 0xff;
    let bb = b & 0xff;
    ar.abs_diff(br) + ag.abs_diff(bg) + ab.abs_diff(bb)
}

fn sanitize_tsv(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' | '\n' | '\r' => ' ',
            _ => c,
        })
        .collect()
}

fn boot_ms() -> u64 {
    let Ok(s) = std::fs::read_to_string("/proc/uptime") else {
        return 0;
    };
    let Some(first) = s.split_whitespace().next() else {
        return 0;
    };
    let Ok(secs) = first.parse::<f64>() else {
        return 0;
    };
    (secs * 1000.0).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pacer(period_us: u64) -> VsyncPacer {
        let (_tx, rx) = mpsc::channel();
        VsyncPacer {
            rx,
            period_us,
            last_hit_at: None,
            last_frame_at: Instant::now() - Duration::from_micros(period_us),
            miss_streak: 0,
            max_miss_streak: 3,
            observed_max_miss_streak: 0,
            hits: 0,
            timeouts: 0,
            errors: 0,
            fallback_frames: 0,
        }
    }

    #[test]
    fn learns_pal_50hz_from_successful_hits() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        let mut at = Instant::now();
        pacer.record_hit(at);
        for _ in 0..48 {
            at += Duration::from_micros(20_000);
            pacer.record_hit(at);
        }

        let inferred_hz = 1_000_000.0 / pacer.period_us() as f64;
        assert!(
            (49.5..=50.5).contains(&inferred_hz),
            "inferred_hz={inferred_hz}"
        );
    }

    #[test]
    fn stays_near_60hz_from_successful_hits() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        let mut at = Instant::now();
        pacer.record_hit(at);
        for _ in 0..24 {
            at += Duration::from_micros(16_667);
            pacer.record_hit(at);
        }

        let inferred_hz = 1_000_000.0 / pacer.period_us() as f64;
        assert!(
            (59.5..=60.5).contains(&inferred_hz),
            "inferred_hz={inferred_hz}"
        );
    }

    #[test]
    fn isolated_misses_create_one_fallback_frame_each_without_degraded_streak() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        let mut at = Instant::now();
        pacer.record_hit(at);

        for _ in 0..10 {
            pacer.last_frame_at = Instant::now() - Duration::from_micros(pacer.period_us());
            let pace = pacer.fallback_after_miss(VsyncPaceSource::Timeout, 16_667, None);
            assert_eq!(pace.source, VsyncPaceSource::Timeout);
            assert_eq!(pace.miss_streak, 1);
            at += Duration::from_micros(16_667);
            pacer.record_hit(at);
        }

        assert_eq!(pacer.timeouts(), 0);
        assert_eq!(pacer.fallback_frames(), 10);
        assert_eq!(pacer.max_miss_streak(), 1);
        assert_eq!(pacer.miss_streak, 0);
    }

    #[test]
    fn three_consecutive_misses_reach_degraded_threshold() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        for expected in 1..=3 {
            pacer.last_frame_at = Instant::now() - Duration::from_micros(pacer.period_us());
            let pace = pacer.fallback_after_miss(VsyncPaceSource::Timeout, 16_667, None);
            assert_eq!(pace.miss_streak, expected);
        }

        assert_eq!(pacer.fallback_frames(), 3);
        assert_eq!(pacer.max_miss_streak(), 3);
    }

    #[test]
    fn successful_hit_recovers_after_degraded_streak() {
        let mut pacer = test_pacer(DEFAULT_VSYNC_FALLBACK_US);
        for _ in 0..3 {
            pacer.last_frame_at = Instant::now() - Duration::from_micros(pacer.period_us());
            pacer.fallback_after_miss(VsyncPaceSource::Timeout, 16_667, None);
        }
        assert_eq!(pacer.miss_streak, 3);

        pacer.record_hit(Instant::now());

        assert_eq!(pacer.miss_streak, 0);
        assert_eq!(pacer.hits(), 1);
        assert_eq!(pacer.max_miss_streak(), 3);
    }
}
