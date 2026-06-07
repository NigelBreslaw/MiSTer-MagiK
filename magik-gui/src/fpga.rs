//! Native port of MiSTer's HPS↔FPGA "SPI" layer (the GPO/GPI bit-bang in
//! `fpga_io.cpp` / `spi.cpp`). Proven from Python in the spike (AGENTS.md §9.5);
//! this is the real implementation at native speed, where multi-word reads work.
//!
//! The "SPI" is just two memory-mapped registers in the FPGA manager:
//!   GPO (write, 0xFF706000+0x10) and GPI (read, +0x14).
//! GPO is write-only, so we keep a software shadow (`gpo`), exactly like MiSTer's
//! `gpo_copy`. Bit31 must stay set (it means "configured"); bit20 is the IO chip
//! select (EnableIO/DisableIO); bit17 is the strobe; the low 16 bits are data.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::ptr::{read_volatile, write_volatile};

const MGR_BASE: i64 = 0xFF70_6000; // SOCFPGA FPGA-manager, page aligned
const MGR_LEN: usize = 0x1000;
const GPO_OFF: usize = 0x10;
const GPI_OFF: usize = 0x14;

const STROBE: u32 = 1 << 17; // SSPI_STROBE
const ACK: u32 = STROBE; // SSPI_ACK (same bit, read on GPI)
const IO_EN: u32 = 1 << 20; // SSPI_IO_EN
const BIT31: u32 = 0x8000_0000;

// UIO commands (user_io.h).
pub const UIO_GET_VRES: u16 = 0x23;
pub const UIO_GET_FB_PAR: u16 = 0x40;
pub const UIO_SET_FBUF: u16 = 0x2F;
pub const UIO_BUT_SW: u16 = 0x01;
pub const UIO_AUDVOL: u16 = 0x26;

// user_io.h CONF_* flags for UIO_BUT_SW (direct_video + HPS framebuffer path).
pub const CONF_VGA_SCALER: u16 = 0x0004;
pub const CONF_DIRECT_VIDEO: u16 = 0x0400;
pub const CONF_VGA_FB: u16 = 0x1000;

// HPS framebuffer constants (video.cpp).
pub const FB_ADDR: u32 = 0x2000_0000 + (32 * 1024 * 1024); // 0x22000000
pub const FB_SIZE_PX: u32 = 1920 * 1080;
pub const FB_FMT_8888: u16 = 0b00110;
pub const FB_FMT_RXB: u16 = 0b10000;
pub const FB_EN: u16 = 0x8000;
pub const FB_DV_LBRD: i32 = 3;
pub const FB_DV_UBRD: i32 = 2;

#[derive(Clone, Copy, Debug)]
pub struct VideoInfo {
    pub raw_res: u16,
    pub width: u32,
    pub height: u32,
    pub htime: u32,
    pub vtime: u32,
    pub ptime: u32,
    pub vtimeh: u32,
    pub ctime: u32,
    pub pixrep: u16,
    pub de_h: u16,
    pub de_v: u16,
    pub interlaced: bool,
    pub rotated: bool,
}

impl VideoInfo {
    pub fn log_line(self) -> String {
        format!(
            "uio_vres={}x{} raw=0x{:04x} pixrep={} de={}x{} htime={} vtime={} ptime={} vtimeh={} ctime={} interlaced={} rotated={}",
            self.width,
            self.height,
            self.raw_res,
            self.pixrep,
            self.de_h,
            self.de_v,
            self.htime,
            self.vtime,
            self.ptime,
            self.vtimeh,
            self.ctime,
            self.interlaced,
            self.rotated
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FbParams {
    pub crc: u8,
    pub arx: u16,
    pub ary: u16,
    pub arxy: bool,
    pub fb_fmt: u16,
    pub fb_width: u16,
    pub fb_height: u16,
    pub fb_enabled: bool,
}

impl FbParams {
    pub fn log_line(self) -> String {
        format!(
            "uio_fb_par={}x{} fmt=0x{:04x} enabled={} ar={}x{} arxy={} crc=0x{:02x}",
            self.fb_width,
            self.fb_height,
            self.fb_fmt,
            self.fb_enabled,
            self.arx,
            self.ary,
            self.arxy,
            self.crc
        )
    }
}

/// Timing of one MiSTer video mode (vmode_t.item[1..8]): hact, hfp, hs, hbp,
/// vact, vfp, vs, vbp. We only need hact/hbp/vact/vbp for fb positioning.
#[derive(Clone, Copy)]
pub struct Mode {
    pub hact: u16,
    pub hbp: u16,
    pub vact: u16,
    pub vbp: u16,
}

impl Mode {
    pub fn framebuffer_sized(width: u16, height: u16) -> Self {
        Self {
            hact: width,
            hbp: FB_DV_LBRD as u16,
            vact: height,
            vbp: FB_DV_UBRD as u16,
        }
    }
}

/// Bounded spin so a wedged FPGA (GPI bit31 set / ACK never toggles) can't hang
/// us forever, unlike MiSTer which reboots in that case.
const SPIN_LIMIT: u32 = 2_000_000;

pub struct Fpga {
    base: *mut u8,
    _file: std::fs::File,
    gpo: u32,
}

impl Fpga {
    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open("/dev/mem")?;
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                MGR_LEN,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                MGR_BASE as libc::off_t,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            base: base as *mut u8,
            _file: file,
            // GPO is write-only; we can't read its current value, so start from a
            // known-safe shadow (configured bit set, everything else clear).
            gpo: BIT31,
        })
    }

    #[inline]
    fn wr(&mut self, v: u32) {
        self.gpo = v;
        unsafe { write_volatile(self.base.add(GPO_OFF) as *mut u32, v) };
    }

    #[inline]
    fn rd(&self) -> u32 {
        unsafe { read_volatile(self.base.add(GPI_OFF) as *const u32) }
    }

    fn spi_en(&mut self, mask: u32, en: bool) {
        let gpo = self.gpo | BIT31;
        self.wr(if en { gpo | mask } else { gpo & !mask });
    }

    pub fn enable_io(&mut self) {
        self.spi_en(IO_EN, true);
    }

    pub fn disable_io(&mut self) {
        self.spi_en(IO_EN, false);
    }

    /// One SPI word, faithful to `fpga_spi`: returns the GPI value captured as ACK
    /// drops (low 16 bits = response data).
    pub fn spi(&mut self, word: u16) -> u16 {
        self.spi_capture(word).1
    }

    /// Like `spi` but also returns the value captured while ACK is high. Some
    /// FPGA responses present read data only during the strobe-high window; this
    /// lets the first on-device run tell us which phase is authoritative at
    /// native speed (the Python spike was too slow to tell — AGENTS.md §9.5).
    /// Returns `(ack_high_data, ack_low_data)`.
    pub fn spi_capture(&mut self, word: u16) -> (u16, u16) {
        let gpo = (self.gpo & !(0xFFFF | STROBE)) | word as u32;
        self.wr(gpo);
        self.wr(gpo | STROBE);

        let mut hi: u16 = 0;
        let mut n = 0;
        loop {
            let g = self.rd();
            if g & ACK != 0 {
                hi = g as u16;
                break;
            }
            n += 1;
            if n >= SPIN_LIMIT {
                break;
            }
        }

        self.wr(gpo);

        let mut lo: u16 = 0;
        n = 0;
        loop {
            let g = self.rd();
            if g & ACK == 0 {
                lo = g as u16;
                break;
            }
            n += 1;
            if n >= SPIN_LIMIT {
                break;
            }
        }
        (hi, lo)
    }

    #[inline]
    pub fn spi_w(&mut self, word: u16) -> u16 {
        self.spi(word)
    }

    /// `spi_uio_cmd_cont`: EnableIO then send the command, leaving IO enabled so
    /// the caller can stream response/parameter words before `disable_io`.
    #[allow(dead_code)] // kept as a diagnostic primitive
    pub fn cmd_cont(&mut self, cmd: u16) -> u16 {
        self.enable_io();
        self.spi(cmd)
    }

    /// Like `cmd_cont` but captures both ACK phases of the command word.
    pub fn cmd_capture(&mut self, cmd: u16) -> (u16, u16) {
        self.enable_io();
        self.spi_capture(cmd)
    }

    /// `spi_uio_cmd16`: one command word + one parameter word.
    pub fn uio_cmd16(&mut self, cmd: u16, parm: u16) -> u16 {
        self.enable_io();
        self.spi_w(cmd);
        let res = self.spi_w(parm);
        self.disable_io();
        res
    }

    /// Tail of `video_fb_enable` when `direct_video=1` — muxes HDMI to the HPS fb.
    /// Without this, SET_FBUF writes pixels but HDMI stays on the (blank) core path.
    pub fn set_vga_fb(&mut self, enable: bool) {
        let mut map = CONF_VGA_SCALER | CONF_DIRECT_VIDEO;
        if enable {
            map |= CONF_VGA_FB;
        }
        self.uio_cmd16(UIO_BUT_SW, map);
    }

    /// Set the FPGA digital audio attenuation. This mirrors Main_MiSTer's
    /// `send_volume()` path; `0` is max volume and bit 4 would mute.
    pub fn set_audio_volume(&mut self, attenuation: u8) {
        self.uio_cmd16(UIO_AUDVOL, attenuation as u16);
    }

    pub fn read_video_info(&mut self) -> VideoInfo {
        let _ = self.cmd_capture(UIO_GET_VRES);
        let word = |this: &mut Self| this.spi_capture(0).1;
        let raw_res = word(self);
        let width = word(self) as u32 | ((word(self) as u32) << 16);
        let height = word(self) as u32 | ((word(self) as u32) << 16);
        let htime = word(self) as u32 | ((word(self) as u32) << 16);
        let vtime = word(self) as u32 | ((word(self) as u32) << 16);
        let ptime = word(self) as u32 | ((word(self) as u32) << 16);
        let vtimeh = word(self) as u32 | ((word(self) as u32) << 16);
        let ctime = word(self) as u32 | ((word(self) as u32) << 16);
        let pixrep = word(self);
        let de_h = word(self);
        let de_v = word(self);
        self.disable_io();
        VideoInfo {
            raw_res,
            width,
            height,
            htime,
            vtime,
            ptime,
            vtimeh,
            ctime,
            pixrep,
            de_h,
            de_v,
            interlaced: (raw_res & 0x100) != 0,
            rotated: (raw_res & 0x200) != 0,
        }
    }

    pub fn read_fb_params(&mut self) -> FbParams {
        let (crc, _) = self.cmd_capture(UIO_GET_FB_PAR);
        let arx_raw = self.spi_capture(0).1;
        let ary_raw = self.spi_capture(0).1;
        let fb_fmt = self.spi_capture(0).1;
        let fb_width = self.spi_capture(0).1;
        let fb_height = self.spi_capture(0).1;
        self.disable_io();
        FbParams {
            crc: crc as u8,
            arx: arx_raw & 0x0fff,
            ary: ary_raw & 0x0fff,
            arxy: (arx_raw & 0x1000) != 0,
            fb_fmt,
            fb_width,
            fb_height,
            fb_enabled: (fb_fmt & 0x40) != 0,
        }
    }

    /// Port of `video_fb_enable(1, n)`, replicating the SET_FBUF sequence in
    /// video.cpp:3290-3321. Routes HPS buffer `n` to scan-out. `mode` is the
    /// active video mode (for positioning); the fb itself is
    /// `fb_width`x`fb_height`.
    pub fn fb_enable(
        &mut self,
        n: u32,
        fb_width: u16,
        fb_height: u16,
        mode: Mode,
        xoff_override: Option<i32>,
        yoff_override: Option<i32>,
        set_vga_fb: bool,
    ) -> u16 {
        let fb_addr = FB_ADDR + (FB_SIZE_PX * 4 * n) + if n == 0 { 4096 } else { 0 };
        // direct_video offsets: xoff = item[4] - FB_DV_LBRD, yoff = item[8] - FB_DV_UBRD.
        let xoff = xoff_override.unwrap_or(mode.hbp as i32 - FB_DV_LBRD);
        let yoff = yoff_override.unwrap_or(mode.vbp as i32 - FB_DV_UBRD);
        // The MiSTer HPS framebuffer path exposes an unstable shimmer on the
        // final HDMI column when the scaled rectangle reaches the inclusive
        // active-area right edge. Keep one guard column off the scan rectangle;
        // /dev/fb0 still contains the full frame, but HDMI no longer samples the
        // noisy edge. Override only for hardware diagnostics.
        let right_guard_cols = std::env::var("MISTER_FB_RIGHT_GUARD_COLS")
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(1)
            .clamp(0, mode.hact.saturating_sub(1) as i32);
        let right = xoff + mode.hact as i32 - 1 - right_guard_cols;
        let bottom = yoff + mode.vact as i32 - 1;

        // Clean chip-select edge first (we may be interrupting a stopped MiSTer
        // mid-transaction), then send the command and read its support flag from
        // the ACK-high phase.
        self.disable_io();
        let (flag, _) = self.cmd_capture(UIO_SET_FBUF);
        crate::boot_analytics::event(
            "rust_fb_enable_direct_route",
            format!(
                "n={n} fb_width={fb_width} fb_height={fb_height} xoff={xoff} yoff={yoff} right={right} bottom={bottom} right_guard_cols={right_guard_cols} stride={} support_flag={flag}",
                fb_width * 4
            ),
        );

        self.spi_w(FB_EN | FB_FMT_RXB | FB_FMT_8888); // format + enable
        self.spi_w(fb_addr as u16); // base addr low
        self.spi_w((fb_addr >> 16) as u16); // base addr high
        self.spi_w(fb_width); // frame width
        self.spi_w(fb_height); // frame height
        self.spi_w(xoff as u16); // scaled left
        self.spi_w(right as u16); // scaled right
        self.spi_w(yoff as u16); // scaled top
        self.spi_w(bottom as u16); // scaled bottom
        self.spi_w(fb_width * 4); // stride (bytes)
        self.disable_io();
        // MiSTer only toggles this mux when cfg.direct_video is enabled. In
        // normal HDMI mode, SET_FBUF alone is the Main_MiSTer path.
        if set_vga_fb {
            self.set_vga_fb(true);
        }
        flag
    }

    /// Historical helper for the direct-video path.
    pub fn fb_enable_direct(
        &mut self,
        n: u32,
        fb_width: u16,
        fb_height: u16,
        mode: Mode,
        xoff_override: Option<i32>,
        yoff_override: Option<i32>,
    ) -> u16 {
        self.fb_enable(
            n,
            fb_width,
            fb_height,
            mode,
            xoff_override,
            yoff_override,
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framebuffer_sized_mode_uses_requested_active_area() {
        let mode = Mode::framebuffer_sized(960, 540);

        assert_eq!(mode.hact, 960);
        assert_eq!(mode.vact, 540);
        assert_eq!(mode.hbp as i32 - FB_DV_LBRD, 0);
        assert_eq!(mode.vbp as i32 - FB_DV_UBRD, 0);
    }
}

impl Drop for Fpga {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.base as *mut libc::c_void, MGR_LEN);
        }
    }
}
