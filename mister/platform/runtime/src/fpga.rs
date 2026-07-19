// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Native port of MiSTer's HPS↔FPGA "SPI" layer (the GPO/GPI bit-bang in
//! `fpga_io.cpp` / `spi.cpp`). Proven from Python in the framebuffer spike
//! documented in `history/2026-5-2/framebuffer-experiments.md`; this is the real
//! implementation at native speed, where multi-word reads work.
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

use crate::framebuffer::route::{FramebufferRouteMode, LauncherFramebufferRoute};

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
// Private Menu_MiSTer experiment commands. 0x43 is UIO_GET_F12_MOD in
// hps_io.sv, and 0x53..0x56 are file-I/O commands, so keep this pair above
// the stock command ranges used by the Menu core.
pub const MAGIK_UIO_SET_FBUF_LATCH: u16 = mister_magik_latch_contract::SET_FBUF_LATCH;
pub const MAGIK_UIO_GET_FBUF_LATCH: u16 = mister_magik_latch_contract::GET_FBUF_LATCH;
pub const MAGIK_UIO_GET_FBUF_LATCH_CAPS: u16 = mister_magik_latch_contract::GET_FBUF_LATCH_CAPS;
pub const MAGIK_FBUF_LATCH_MAGIC: u16 = mister_magik_latch_contract::LATCH_MAGIC;
pub const MAGIK_FBUF_STATUS_MAGIC: u16 = mister_magik_latch_contract::STATUS_MAGIC;
pub const MAGIK_FBUF_CAPS_MAGIC: u16 = mister_magik_latch_contract::CAPS_MAGIC;

// user_io.h CONF_* flags for UIO_BUT_SW (direct_video + HPS framebuffer path).
pub const CONF_VGA_SCALER: u16 = 0x0004;
pub const CONF_DIRECT_VIDEO: u16 = 0x0400;
pub const CONF_VGA_FB: u16 = 0x1000;

use crate::framebuffer::format::{rgb565_stride_bytes, FB_FMT_565, FB_FMT_RXB};

// HPS framebuffer constants (video.cpp).
pub const FB_ADDR: u32 = 0x2000_0000 + (32 * 1024 * 1024); // 0x22000000
pub const FB_SIZE_PX: u32 = 1920 * 1080;
pub const FB_EN: u16 = 0x8000;
pub const FB_DV_LBRD: i32 = 3;
pub const FB_DV_UBRD: i32 = 2;

#[derive(Clone, Copy, Debug)]
pub struct LatchedFbufGeometry {
    pub xoff: u16,
    pub right: u16,
    pub yoff: u16,
    pub bottom: u16,
    pub stride_bytes: u16,
}

impl LatchedFbufGeometry {
    pub fn new(fb_width: u16, mode: FramebufferRouteMode, right_guard_cols: i32) -> Self {
        let xoff = mode.hbp as i32 - FB_DV_LBRD;
        let yoff = mode.vbp as i32 - FB_DV_UBRD;
        let right_guard_cols = right_guard_cols.clamp(0, mode.hact.saturating_sub(1) as i32);
        let right = xoff + mode.hact as i32 - 1 - right_guard_cols;
        let bottom = yoff + mode.vact as i32 - 1;
        Self {
            xoff: xoff as u16,
            right: right as u16,
            yoff: yoff as u16,
            bottom: bottom as u16,
            stride_bytes: rgb565_stride_bytes(fb_width as usize) as u16,
        }
    }
}

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

/// Bounded spin so a wedged FPGA (GPI bit31 set / ACK never toggles) can't hang
/// us forever, unlike MiSTer which reboots in that case.
const SPIN_LIMIT: u32 = 2_000_000;

pub struct Fpga {
    base: *mut u8,
    _file: std::fs::File,
    gpo: u32,
}

impl Fpga {
    fn spi_timeout(phase: &str, word: u16) -> io::Error {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("FPGA SPI timeout waiting for ACK {phase} on word 0x{word:04x}"),
        )
    }

    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open("/dev/mem")?;
        // SAFETY: maps the documented MiSTer FPGA manager MMIO page from
        // /dev/mem. MGR_LEN and MGR_BASE are constants for this device, and the
        // returned mapping is checked before use and unmapped in Drop.
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
        debug_assert!(GPO_OFF + std::mem::size_of::<u32>() <= MGR_LEN);
        // SAFETY: base is a live /dev/mem MMIO mapping for MGR_LEN bytes, and
        // GPO_OFF is within that mapping. Volatile preserves the device write.
        unsafe { write_volatile(self.base.add(GPO_OFF) as *mut u32, v) };
    }

    #[inline]
    fn rd(&self) -> u32 {
        debug_assert!(GPI_OFF + std::mem::size_of::<u32>() <= MGR_LEN);
        // SAFETY: base is a live /dev/mem MMIO mapping for MGR_LEN bytes, and
        // GPI_OFF is within that mapping. Volatile preserves the device read.
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
    pub fn spi(&mut self, word: u16) -> io::Result<u16> {
        Ok(self.spi_capture(word)?.1)
    }

    /// Like `spi` but also returns the value captured while ACK is high. Some
    /// FPGA responses present read data only during the strobe-high window; this
    /// lets the first on-device run tell us which phase is authoritative at
    /// native speed (the Python spike documented in
    /// `history/2026-5-2/framebuffer-experiments.md` was too slow to tell).
    /// Returns `(ack_high_data, ack_low_data)`.
    pub fn spi_capture(&mut self, word: u16) -> io::Result<(u16, u16)> {
        let gpo = (self.gpo & !(0xFFFF | STROBE)) | word as u32;
        self.wr(gpo);
        self.wr(gpo | STROBE);

        let hi: u16;
        let mut n = 0;
        loop {
            let g = self.rd();
            if g & ACK != 0 {
                hi = g as u16;
                break;
            }
            n += 1;
            if n >= SPIN_LIMIT {
                self.wr(gpo);
                return Err(Self::spi_timeout("high", word));
            }
        }

        self.wr(gpo);

        let lo: u16;
        n = 0;
        loop {
            let g = self.rd();
            if g & ACK == 0 {
                lo = g as u16;
                break;
            }
            n += 1;
            if n >= SPIN_LIMIT {
                return Err(Self::spi_timeout("low", word));
            }
        }
        Ok((hi, lo))
    }

    #[inline]
    pub fn spi_w(&mut self, word: u16) -> io::Result<u16> {
        self.spi(word)
    }

    /// `spi_uio_cmd_cont`: EnableIO then send the command, leaving IO enabled so
    /// the caller can stream response/parameter words before `disable_io`.
    #[allow(dead_code)] // kept as a diagnostic primitive
    pub fn cmd_cont(&mut self, cmd: u16) -> io::Result<u16> {
        self.enable_io();
        match self.spi(cmd) {
            Ok(res) => Ok(res),
            Err(e) => {
                self.disable_io();
                Err(e)
            }
        }
    }

    /// Like `cmd_cont` but captures both ACK phases of the command word.
    pub fn cmd_capture(&mut self, cmd: u16) -> io::Result<(u16, u16)> {
        self.enable_io();
        match self.spi_capture(cmd) {
            Ok(res) => Ok(res),
            Err(e) => {
                self.disable_io();
                Err(e)
            }
        }
    }

    /// `spi_uio_cmd16`: one command word + one parameter word.
    pub fn uio_cmd16(&mut self, cmd: u16, parm: u16) -> io::Result<u16> {
        self.enable_io();
        let res = (|| {
            self.spi_w(cmd)?;
            self.spi_w(parm)
        })();
        self.disable_io();
        res
    }

    /// Tail of `video_fb_enable` when `direct_video=1` — muxes HDMI to the HPS fb.
    /// Without this, SET_FBUF writes pixels but HDMI stays on the (blank) core path.
    pub fn set_vga_fb(&mut self, enable: bool) -> io::Result<()> {
        let mut map = CONF_VGA_SCALER | CONF_DIRECT_VIDEO;
        if enable {
            map |= CONF_VGA_FB;
        }
        self.uio_cmd16(UIO_BUT_SW, map)?;
        Ok(())
    }

    /// Set the FPGA digital audio attenuation. This mirrors Main_MiSTer's
    /// `send_volume()` path; `0` is max volume and bit 4 would mute.
    pub fn set_audio_volume(&mut self, attenuation: u8) -> io::Result<()> {
        self.uio_cmd16(UIO_AUDVOL, attenuation as u16)?;
        Ok(())
    }

    pub fn read_video_info(&mut self) -> io::Result<VideoInfo> {
        let res = (|| {
            let _ = self.cmd_capture(UIO_GET_VRES)?;
            let word = |this: &mut Self| -> io::Result<u16> { Ok(this.spi_capture(0)?.1) };
            let raw_res = word(self)?;
            let width = word(self)? as u32 | ((word(self)? as u32) << 16);
            let height = word(self)? as u32 | ((word(self)? as u32) << 16);
            let htime = word(self)? as u32 | ((word(self)? as u32) << 16);
            let vtime = word(self)? as u32 | ((word(self)? as u32) << 16);
            let ptime = word(self)? as u32 | ((word(self)? as u32) << 16);
            let vtimeh = word(self)? as u32 | ((word(self)? as u32) << 16);
            let ctime = word(self)? as u32 | ((word(self)? as u32) << 16);
            let pixrep = word(self)?;
            let de_h = word(self)?;
            let de_v = word(self)?;
            Ok(VideoInfo {
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
            })
        })();
        self.disable_io();
        res
    }

    pub fn read_fb_params(&mut self) -> io::Result<FbParams> {
        let res = (|| {
            let (crc, _) = self.cmd_capture(UIO_GET_FB_PAR)?;
            let arx_raw = self.spi_capture(0)?.1;
            let ary_raw = self.spi_capture(0)?.1;
            let fb_fmt = self.spi_capture(0)?.1;
            let fb_width = self.spi_capture(0)?.1;
            let fb_height = self.spi_capture(0)?.1;
            Ok(FbParams {
                crc: crc as u8,
                arx: arx_raw & 0x0fff,
                ary: ary_raw & 0x0fff,
                arxy: (arx_raw & 0x1000) != 0,
                fb_fmt,
                fb_width,
                fb_height,
                fb_enabled: (fb_fmt & 0x40) != 0,
            })
        })();
        self.disable_io();
        res
    }

    pub fn read_magik_latched_fbuf_status(&mut self) -> io::Result<LatchedFbufStatus> {
        let res = (|| {
            let (magic_hi, magic_lo) = self.cmd_capture(MAGIK_UIO_GET_FBUF_LATCH)?;
            let mut words = [0u16; 11];
            for word in words.iter_mut() {
                *word = self.spi_capture(0)?.1;
            }
            Ok(LatchedFbufStatus {
                magic_hi,
                magic_lo,
                active_sequence: words[0],
                pending_sequence: words[1],
                flags: words[2],
                flip_count: words[3],
                post_count: words[4],
                drop_count: words[5],
                active_base: words[6] as u32 | ((words[7] as u32) << 16),
                active_width: words[8],
                active_height: words[9],
                active_stride: words[10],
            })
        })();
        self.disable_io();
        res
    }

    pub fn probe_magik_latched_fbuf_set(&mut self) -> io::Result<(u16, u16)> {
        let res = self.cmd_capture(MAGIK_UIO_SET_FBUF_LATCH);
        self.disable_io();
        res
    }

    pub fn read_magik_latched_fbuf_capabilities(
        &mut self,
    ) -> io::Result<(u16, u16, mister_magik_latch_contract::LatchCapabilities)> {
        let result = (|| {
            let (magic_hi, magic_lo) = self.cmd_capture(MAGIK_UIO_GET_FBUF_LATCH_CAPS)?;
            let mut words = [0u16; mister_magik_latch_contract::CAPS_WORD_COUNT];
            for word in &mut words {
                *word = self.spi_capture(0)?.1;
            }
            let capabilities = mister_magik_latch_contract::decode_capabilities(&words)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            Ok((magic_hi, magic_lo, capabilities))
        })();
        self.disable_io();
        result
    }

    pub fn post_magik_latched_fbuf_rgb565(
        &mut self,
        sequence: u16,
        base_addr: u32,
        fb_width: u16,
        fb_height: u16,
        geometry: LatchedFbufGeometry,
    ) -> io::Result<(u16, u16)> {
        self.disable_io();
        let support = self.cmd_capture(MAGIK_UIO_SET_FBUF_LATCH)?;
        let stream_res: io::Result<()> = (|| {
            let fpga_format = FB_EN | FB_FMT_565 | FB_FMT_RXB;
            self.spi_w(fpga_format)?;
            self.spi_w(base_addr as u16)?;
            self.spi_w((base_addr >> 16) as u16)?;
            self.spi_w(fb_width)?;
            self.spi_w(fb_height)?;
            self.spi_w(geometry.xoff)?;
            self.spi_w(geometry.right)?;
            self.spi_w(geometry.yoff)?;
            self.spi_w(geometry.bottom)?;
            self.spi_w(geometry.stride_bytes)?;
            self.spi_w(sequence)?;
            Ok(())
        })();
        self.disable_io();
        stream_res?;
        Ok(support)
    }

    /// Port of `video_fb_enable(1, n)`, replicating the SET_FBUF sequence in
    /// video.cpp:3290-3321. Routes HPS buffer `n` to scan-out. `mode` is the
    /// active video mode (for positioning); the fb itself is
    /// `fb_width`x`fb_height`.
    pub fn fb_enable_rgb565(
        &mut self,
        n: u32,
        fb_width: u16,
        fb_height: u16,
        mode: FramebufferRouteMode,
        set_vga_fb: bool,
    ) -> io::Result<u16> {
        let fb_addr = FB_ADDR + (FB_SIZE_PX * 4 * n) + if n == 0 { 4096 } else { 0 };
        // direct_video offsets: xoff = item[4] - FB_DV_LBRD, yoff = item[8] - FB_DV_UBRD.
        let xoff = mode.hbp as i32 - FB_DV_LBRD;
        let yoff = mode.vbp as i32 - FB_DV_UBRD;
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
        let (flag, _) = self.cmd_capture(UIO_SET_FBUF)?;
        crate::boot_analytics::event(
            "rust_fb_enable_direct_route",
            format!(
                "n={n} fb_width={fb_width} fb_height={fb_height} format=565 xoff={xoff} yoff={yoff} right={right} bottom={bottom} right_guard_cols={right_guard_cols} stride={} support_flag={flag}",
                rgb565_stride_bytes(fb_width as usize)
            ),
        );

        let stream_res: io::Result<()> = (|| {
            let fpga_format = FB_EN | FB_FMT_565 | FB_FMT_RXB;
            self.spi_w(fpga_format)?; // format + enable
            self.spi_w(fb_addr as u16)?; // base addr low
            self.spi_w((fb_addr >> 16) as u16)?; // base addr high
            self.spi_w(fb_width)?; // frame width
            self.spi_w(fb_height)?; // frame height
            self.spi_w(xoff as u16)?; // scaled left
            self.spi_w(right as u16)?; // scaled right
            self.spi_w(yoff as u16)?; // scaled top
            self.spi_w(bottom as u16)?; // scaled bottom
            self.spi_w(rgb565_stride_bytes(fb_width as usize) as u16)?; // stride (bytes)
            Ok(())
        })();
        self.disable_io();
        stream_res?;
        // MiSTer only toggles this mux when cfg.direct_video is enabled. In
        // normal HDMI mode, SET_FBUF alone is the Main_MiSTer path.
        if set_vga_fb {
            self.set_vga_fb(true)?;
        }
        Ok(flag)
    }

    pub fn enable_launcher_framebuffer_route(
        &mut self,
        route: LauncherFramebufferRoute,
        fb_width: usize,
        fb_height: usize,
    ) -> io::Result<u16> {
        self.fb_enable_rgb565(
            0,
            fb_width as u16,
            fb_height as u16,
            route.mode(),
            route.set_vga_fb(),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LatchedFbufStatus {
    pub magic_hi: u16,
    pub magic_lo: u16,
    pub active_sequence: u16,
    pub pending_sequence: u16,
    pub flags: u16,
    pub flip_count: u16,
    pub post_count: u16,
    pub drop_count: u16,
    pub active_base: u32,
    pub active_width: u16,
    pub active_height: u16,
    pub active_stride: u16,
}

impl LatchedFbufStatus {
    pub fn supported(self) -> bool {
        self.magic_hi == MAGIK_FBUF_STATUS_MAGIC || self.magic_lo == MAGIK_FBUF_STATUS_MAGIC
    }

    pub fn pending(self) -> bool {
        (self.flags & 0x0004) != 0
    }

    pub fn pending_enabled(self) -> bool {
        (self.flags & 0x0002) != 0
    }

    pub fn active_enabled(self) -> bool {
        (self.flags & 0x0001) != 0
    }
}

impl Drop for Fpga {
    fn drop(&mut self) {
        // SAFETY: base/MGR_LEN come from a successful mmap in Fpga::open and
        // this Drop path runs once for the owning Fpga.
        unsafe {
            libc::munmap(self.base as *mut libc::c_void, MGR_LEN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framebuffer_sized_mode_uses_requested_active_area() {
        let mode = FramebufferRouteMode::framebuffer_sized(960, 540);

        assert_eq!(mode.hact, 960);
        assert_eq!(mode.vact, 540);
        assert_eq!(mode.hbp as i32 - FB_DV_LBRD, 0);
        assert_eq!(mode.vbp as i32 - FB_DV_UBRD, 0);
    }
}
