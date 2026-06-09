//! Optional write-combined backbuffer sidecar support.
//!
//! This is intentionally separate from the production `/dev/fb0` display path.
//! The module is opt-in and must pass exact MiSTer kernel/resource checks before
//! userspace attempts to load it or map the hidden buffer.

use crate::fb::Pixel;
use crate::fpga::{Fpga, Mode, FB_SIZE_PX};
use crate::ui_display::{UI_FB_H, UI_FB_W, UI_HDMI_H, UI_HDMI_W};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

pub const DEVICE_PATH: &str = "/dev/mister-magik-fbwc";
pub const MODULE_NAME: &str = "mister_magik_fbwc";
pub const MODULE_PATH: &str = "/media/fat/mister-magik/mister_magik_fbwc.ko";
pub const SUPPORTED_KERNEL: &str = "5.15.1-MiSTer";
const FBWC_MAP_PIXELS: usize = FB_SIZE_PX as usize;
const FBWC_SLOT_BYTES: usize = FBWC_MAP_PIXELS * std::mem::size_of::<Pixel>();
const EXPECTED_FB_PHYS: &str = "0x22000000";
const EXPECTED_FB_SIZE: &str = "0x800000";

#[derive(Clone, Debug)]
pub struct SupportProbe {
    pub ok: bool,
    pub kernel: String,
    pub details: Vec<String>,
}

impl SupportProbe {
    pub fn log_line(&self) -> String {
        format!(
            "fbwc_probe ok={} kernel={} details={}",
            self.ok,
            self.kernel,
            self.details.join(";")
        )
    }
}

pub struct FbwcBuffer {
    file: File,
    ptr: *mut Pixel,
    pixels: usize,
    map_len: usize,
    index: usize,
}

impl FbwcBuffer {
    pub fn open_pixels(pixels: usize) -> io::Result<Self> {
        Self::open_pixels_at(1, pixels)
    }

    pub fn open_pixels_at(index: usize, pixels: usize) -> io::Result<Self> {
        if !(1..=2).contains(&index) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported fbwc buffer index {index}"),
            ));
        }
        let pixels = pixels.clamp(1, FBWC_MAP_PIXELS);
        let map_len = pixels * std::mem::size_of::<Pixel>();
        let offset = ((index - 1) * FBWC_SLOT_BYTES) as libc::off_t;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(DEVICE_PATH)?;
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                offset,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            file,
            ptr: ptr.cast(),
            pixels,
            map_len,
            index,
        })
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn buffer_mut(&mut self) -> &mut [Pixel] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.pixels) }
    }

    pub fn buffer(&self) -> &[Pixel] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.pixels) }
    }

    pub fn clear(&mut self, color: Pixel) {
        self.buffer_mut().fill(color);
    }

    pub fn copy_rect_from(
        &mut self,
        dst_w: usize,
        dst_h: usize,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Pixel],
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let x1 = (x + w).min(dst_w);
        let y1 = (y + h).min(dst_h);
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

    pub fn status(&mut self) -> io::Result<String> {
        let mut s = String::new();
        self.file.read_to_string(&mut s)?;
        Ok(s)
    }
}

impl Drop for FbwcBuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr.cast(), self.map_len);
        }
    }
}

pub fn requested_direct_mode() -> bool {
    std::env::var("MISTER_UI_RENDER_MODE")
        .ok()
        .map(|s| {
            s.eq_ignore_ascii_case("fbwc-direct")
                || s.eq_ignore_ascii_case("fbwc-double")
                || s.eq_ignore_ascii_case("fbwc-shadow")
        })
        .unwrap_or(false)
}

pub fn requested_double_buffer_mode() -> bool {
    std::env::var("MISTER_UI_RENDER_MODE")
        .ok()
        .map(|s| s.eq_ignore_ascii_case("fbwc-double"))
        .unwrap_or(false)
}

pub fn requested_shadow_buffer_mode() -> bool {
    std::env::var("MISTER_UI_RENDER_MODE")
        .ok()
        .map(|s| s.eq_ignore_ascii_case("fbwc-shadow"))
        .unwrap_or(false)
}

pub fn support_probe() -> SupportProbe {
    let kernel = command_stdout("uname", &["-r"]).unwrap_or_else(|e| format!("error:{e}"));
    let mut ok = true;
    let mut details = Vec::new();

    if kernel.trim() == SUPPORTED_KERNEL {
        details.push(format!("kernel={SUPPORTED_KERNEL}"));
    } else {
        ok = false;
        details.push(format!("unsupported_kernel={}", kernel.trim()));
    }

    let builtins =
        std::fs::read_to_string(format!("/lib/modules/{}/modules.builtin", kernel.trim()))
            .unwrap_or_default();
    if builtins.contains("drivers/video/fbdev/MiSTer_fb.ko") {
        details.push("mister_fb_builtin=1".into());
    } else {
        ok = false;
        details.push("mister_fb_builtin=0".into());
    }

    let modinfo = std::fs::read_to_string(format!(
        "/lib/modules/{}/modules.builtin.modinfo",
        kernel.trim()
    ))
    .unwrap_or_default();
    if modinfo.contains("MiSTer_fb.description=MiSTer framebuffer driver") {
        details.push("mister_fb_modinfo=1".into());
    } else {
        ok = false;
        details.push("mister_fb_modinfo=0".into());
    }

    match device_tree_fb_resource() {
        Ok((phys, size)) if phys == EXPECTED_FB_PHYS && size == EXPECTED_FB_SIZE => {
            details.push(format!("fb_resource={phys}+{size}"));
        }
        Ok((phys, size)) => {
            ok = false;
            details.push(format!("unexpected_fb_resource={phys}+{size}"));
        }
        Err(e) => {
            ok = false;
            details.push(format!("fb_resource_error={e}"));
        }
    }

    SupportProbe {
        ok,
        kernel: kernel.trim().into(),
        details,
    }
}

pub fn ensure_loaded() -> Result<(), String> {
    if module_loaded() && Path::new(DEVICE_PATH).exists() {
        return Ok(());
    }
    let probe = support_probe();
    println!("{}", probe.log_line());
    if !probe.ok {
        return Err("fbwc unsupported on this kernel/device".into());
    }
    if !Path::new(MODULE_PATH).exists() {
        return Err(format!("module not found: {MODULE_PATH}"));
    }
    let status = Command::new("insmod")
        .arg(MODULE_PATH)
        .status()
        .map_err(|e| format!("insmod {MODULE_PATH}: {e}"))?;
    if !status.success() {
        return Err(format!("insmod {MODULE_PATH} exited with {status}"));
    }
    for _ in 0..20 {
        if module_loaded() && Path::new(DEVICE_PATH).exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!("module loaded but {DEVICE_PATH} did not appear"))
}

pub fn unload() -> Result<(), String> {
    if !module_loaded() {
        return Ok(());
    }
    let status = Command::new("rmmod")
        .arg(MODULE_NAME)
        .status()
        .map_err(|e| format!("rmmod {MODULE_NAME}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rmmod {MODULE_NAME} exited with {status}"))
    }
}

pub fn unload_or_warn() {
    if let Err(e) = unload() {
        eprintln!("warning: failed to unload {MODULE_NAME}: {e}; reboot recommended before update/system work");
    }
}

pub fn print_probe() {
    let probe = support_probe();
    println!("{}", probe.log_line());
    println!("module_loaded={}", module_loaded());
    println!("device_exists={}", Path::new(DEVICE_PATH).exists());
    if let Ok(mut file) = File::open(DEVICE_PATH) {
        let mut status = String::new();
        let _ = file.read_to_string(&mut status);
        print!("{status}");
    }
}

pub fn run_bench() {
    if let Err(e) = ensure_loaded() {
        eprintln!("fbwc-bench: {e}");
        std::process::exit(1);
    }
    let mut buf = match FbwcBuffer::open_pixels(FBWC_MAP_PIXELS) {
        Ok(buf) => buf,
        Err(e) => {
            eprintln!("fbwc-bench: mmap {DEVICE_PATH}: {e}");
            std::process::exit(1);
        }
    };
    let status = buf.status().unwrap_or_default();
    print!("{status}");

    bench_fill("full-slot", buf.buffer_mut(), FBWC_MAP_PIXELS, 8);
    bench_fill("ui-960x540", buf.buffer_mut(), UI_FB_W * UI_FB_H, 120);
    bench_fill("partial-240x135", buf.buffer_mut(), 240 * 135, 600);
}

pub fn run_flip_test(f: &mut Fpga) {
    let secs = std::env::args()
        .nth(2)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(10);
    let unload_after = std::env::args().any(|s| s == "--unload" || s == "--rmmod");
    if let Err(e) = ensure_loaded() {
        eprintln!("fbwc-flip-test: {e}");
        std::process::exit(1);
    }
    let mut buf = match FbwcBuffer::open_pixels(UI_FB_W * UI_FB_H) {
        Ok(buf) => buf,
        Err(e) => {
            eprintln!("fbwc-flip-test: mmap {DEVICE_PATH}: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "fbwc-flip-test secs={secs} buffer=1 size={}x{} unload_after={unload_after}",
        UI_FB_W, UI_FB_H
    );
    let set_vga_fb = std::env::var_os("MISTER_DIRECT_VIDEO").is_some();
    let flag = f.fb_enable(
        1,
        UI_FB_W as u16,
        UI_FB_H as u16,
        fpga_scaled_mode(),
        Some(0),
        Some(0),
        set_vga_fb,
    );
    println!("routed fbwc buffer 1 support_flag={flag}");

    let mut pacer = crate::fb::VsyncPacer::from_env();
    let start = Instant::now();
    let mut frames = 0u64;
    let mut draw_us = 0u128;
    while secs == 0 || start.elapsed() < Duration::from_secs(secs) {
        let t0 = Instant::now();
        draw_flip_pattern(buf.buffer_mut(), UI_FB_W, UI_FB_H, frames as usize);
        draw_us += t0.elapsed().as_micros();
        let _pace = pacer.wait();
        frames += 1;
        if frames % 60 == 0 {
            println!(
                "fbwc-flip-test fps_window=60 avg_draw_us={} vsync_hits={} fallback_frames={}",
                draw_us / 60,
                pacer.hits(),
                pacer.fallback_frames()
            );
            draw_us = 0;
        }
    }
    let flag = f.fb_enable(
        0,
        UI_FB_W as u16,
        UI_FB_H as u16,
        fpga_scaled_mode(),
        Some(0),
        Some(0),
        set_vga_fb,
    );
    println!("restored fb0 route support_flag={flag} frames={frames}");
    drop(buf);
    if unload_after {
        unload_or_warn();
    }
}

fn fpga_scaled_mode() -> Mode {
    Mode {
        hact: UI_HDMI_W,
        hbp: 3,
        vact: UI_HDMI_H,
        vbp: 2,
    }
}

fn bench_fill(label: &str, buf: &mut [Pixel], pixels: usize, iters: usize) {
    let pixels = pixels.min(buf.len());
    let bytes = pixels * std::mem::size_of::<Pixel>() * iters;
    let start = Instant::now();
    for i in 0..iters {
        let color = Pixel(0x0001_0101u32.wrapping_mul((i as u32).wrapping_add(1)) & 0x00ff_ffff);
        buf[..pixels].fill(color);
    }
    let secs = start.elapsed().as_secs_f64().max(0.000_001);
    println!(
        "fbwc_bench label={label} pixels={pixels} iters={iters} bytes={bytes} elapsed_ms={:.3} mb_s={:.1}",
        secs * 1000.0,
        bytes as f64 / secs / (1024.0 * 1024.0)
    );
}

fn draw_flip_pattern(buf: &mut [Pixel], w: usize, h: usize, frame: usize) {
    let bar_w = 48;
    let moving_region_x = w / 2;
    let moving_w = w - moving_region_x;
    let moving = (frame * 7) % (moving_w + bar_w);
    for y in 0..h {
        let row = &mut buf[y * w..(y + 1) * w];
        for (x, px) in row.iter_mut().enumerate() {
            let edge = x < 4 || y < 4 || x >= w - 4 || y >= h - 4;
            let split = x >= moving_region_x.saturating_sub(2) && x < moving_region_x + 2;
            if edge || split {
                *px = Pixel(0x00ff_ffff);
                continue;
            }
            if x < moving_region_x {
                *px = Pixel(0x0000_8040);
                continue;
            }

            let local_x = x - moving_region_x;
            let checker = (((local_x / 24) ^ (y / 24) ^ (frame / 8)) & 1) != 0;
            let in_bar = local_x + bar_w >= moving && local_x < moving;
            *px = if in_bar {
                Pixel(0x00ff_0000)
            } else if checker {
                Pixel(0x0000_3050)
            } else {
                Pixel(0x0000_c080)
            };
        }
    }
}

fn module_loaded() -> bool {
    std::fs::read_to_string("/proc/modules")
        .map(|s| s.lines().any(|line| line.starts_with(MODULE_NAME)))
        .unwrap_or(false)
}

fn command_stdout(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("{program} exited with {}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn device_tree_fb_resource() -> Result<(String, String), String> {
    let reg = std::fs::read("/proc/device-tree/MiSTer_fb/reg")
        .map_err(|e| format!("read /proc/device-tree/MiSTer_fb/reg: {e}"))?;
    if reg.len() < 8 {
        return Err(format!("short reg property: {} bytes", reg.len()));
    }
    let phys = u32::from_be_bytes([reg[0], reg[1], reg[2], reg[3]]);
    let size = u32::from_be_bytes([reg[4], reg[5], reg[6], reg[7]]);
    Ok((format!("0x{phys:08x}"), format!("0x{size:x}")))
}
