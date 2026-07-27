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
use std::time::Instant;

use crate::framebuffer::route::{
    FramebufferPlacement, FramebufferRouteMode, LauncherFramebufferRoute,
};
use crate::latch_readiness::{
    LatchWireAttempt, LatchWireDecision, LatchWireDiagnostics, LatchWireErrorPhase,
    LatchWireResult, LatchWireWord,
};

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

use crate::framebuffer::format::{FB_FMT_565, FB_FMT_RXB, rgb565_stride_bytes};

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
        Self::new_with_placement(
            fb_width,
            FramebufferPlacement::from_mode(mode),
            right_guard_cols,
        )
    }

    pub fn new_for_route(
        fb_width: u16,
        route: LauncherFramebufferRoute,
        right_guard_cols: i32,
    ) -> Self {
        Self::new_with_placement(fb_width, route.placement(), right_guard_cols)
    }

    fn new_with_placement(
        fb_width: u16,
        placement: FramebufferPlacement,
        right_guard_cols: i32,
    ) -> Self {
        let [xoff, right, yoff, bottom] = diagnostic_fbuf_rectangle().unwrap_or_else(|| {
            let xoff = i32::from(placement.left);
            let yoff = i32::from(placement.top);
            let right_guard_cols =
                right_guard_cols.clamp(0, placement.width.saturating_sub(1) as i32);
            [
                xoff,
                xoff + i32::from(placement.width) - 1 - right_guard_cols,
                yoff,
                yoff + i32::from(placement.height) - 1,
            ]
        });
        Self {
            xoff: xoff as u16,
            right: right as u16,
            yoff: yoff as u16,
            bottom: bottom as u16,
            stride_bytes: rgb565_stride_bytes(fb_width as usize) as u16,
        }
    }
}

fn diagnostic_fbuf_rectangle() -> Option<[i32; 4]> {
    // The bounded host workflow provides this only for the standalone CRT trial.
    if std::env::var("MISTER_MAGIK_CRT_TRIAL").ok().as_deref() != Some("1") {
        return None;
    }
    let value = std::env::var("MISTER_FB_DIAGNOSTIC_RECT").ok()?;
    parse_diagnostic_fbuf_rectangle(&value)
}

fn parse_diagnostic_fbuf_rectangle(value: &str) -> Option<[i32; 4]> {
    let values = value
        .split(',')
        .map(str::parse::<i32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let rectangle: [i32; 4] = values.try_into().ok()?;
    let [left, right, top, bottom] = rectangle;
    (left >= 0 && top >= 0 && right >= left && bottom >= top && right <= 2047 && bottom <= 2047)
        .then_some(rectangle)
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

#[derive(Clone, Copy, Debug)]
struct SpiCapture {
    ack_high: u16,
    ack_low: u16,
}

#[derive(Debug)]
struct SpiCaptureFailure {
    error: io::Error,
    ack_high: Option<u16>,
    phase: LatchWireErrorPhase,
}

trait RegisterIo {
    fn write_gpo(&mut self, value: u32);
    fn read_gpi(&mut self) -> u32;
}

struct MmioRegisters {
    base: *mut u8,
    _file: std::fs::File,
}

impl RegisterIo for MmioRegisters {
    fn write_gpo(&mut self, value: u32) {
        debug_assert!(GPO_OFF + std::mem::size_of::<u32>() <= MGR_LEN);
        // SAFETY: base is a live /dev/mem MMIO mapping for MGR_LEN bytes, and
        // GPO_OFF is within that mapping. Volatile preserves the device write.
        unsafe { write_volatile(self.base.add(GPO_OFF) as *mut u32, value) };
    }

    fn read_gpi(&mut self) -> u32 {
        debug_assert!(GPI_OFF + std::mem::size_of::<u32>() <= MGR_LEN);
        // SAFETY: base is a live /dev/mem MMIO mapping for MGR_LEN bytes, and
        // GPI_OFF is within that mapping. Volatile preserves the device read.
        unsafe { read_volatile(self.base.add(GPI_OFF) as *const u32) }
    }
}

impl Drop for MmioRegisters {
    fn drop(&mut self) {
        // SAFETY: base/MGR_LEN come from a successful mmap and this Drop path
        // runs once for the owning register mapping.
        unsafe {
            libc::munmap(self.base as *mut libc::c_void, MGR_LEN);
        }
    }
}

pub struct Fpga {
    registers: Box<dyn RegisterIo>,
    gpo: u32,
    latch_capabilities: Option<mister_magik_latch_contract::LatchCapabilities>,
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
            registers: Box::new(MmioRegisters {
                base: base as *mut u8,
                _file: file,
            }),
            // GPO is write-only; we can't read its current value, so start from a
            // known-safe shadow (configured bit set, everything else clear).
            gpo: BIT31,
            latch_capabilities: None,
        })
    }

    #[inline]
    fn wr(&mut self, v: u32) {
        self.gpo = v;
        self.registers.write_gpo(v);
    }

    #[inline]
    fn rd(&mut self) -> u32 {
        self.registers.read_gpi()
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

    fn reset_spi_transport(&mut self) {
        self.gpo = (self.gpo | BIT31) & !(IO_EN | STROBE | 0xffff);
        self.wr(self.gpo);
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
        self.spi_capture_observed(word)
            .map(|capture| (capture.ack_high, capture.ack_low))
            .map_err(|failure| failure.error)
    }

    fn spi_capture_observed(&mut self, word: u16) -> Result<SpiCapture, SpiCaptureFailure> {
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
                return Err(SpiCaptureFailure {
                    error: Self::spi_timeout("high", word),
                    ack_high: None,
                    phase: LatchWireErrorPhase::AckHigh,
                });
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
                return Err(SpiCaptureFailure {
                    error: Self::spi_timeout("low", word),
                    ack_high: Some(hi),
                    phase: LatchWireErrorPhase::AckLow,
                });
            }
        }
        Ok(SpiCapture {
            ack_high: hi,
            ack_low: lo,
        })
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
        self.read_magik_latched_fbuf_status_sample()
            .map(|sample| sample.status)
            .map_err(LatchedFbufStatusReadError::into_io)
    }

    pub fn read_magik_latched_fbuf_status_sample(
        &mut self,
    ) -> Result<LatchedFbufStatusSample, LatchedFbufStatusReadError> {
        match self.read_magik_latched_fbuf_status_once() {
            Err(mut first) if first.source.kind() == io::ErrorKind::TimedOut => {
                // GET_FBUF_LATCH is an idempotent status query. Return the bus
                // to a known idle state and retry it once; never apply this to
                // framebuffer posts, whose payload may already be committed.
                self.reset_spi_transport();
                match self.read_magik_latched_fbuf_status_once() {
                    Ok(mut sample) => {
                        first.diagnostics.append(&sample.diagnostics);
                        first.diagnostics.decision = LatchWireDecision::TransportRetryRecovered;
                        sample.diagnostics = first.diagnostics;
                        Ok(sample)
                    }
                    Err(second) => {
                        first.diagnostics.append(&second.diagnostics);
                        first.diagnostics.decision = LatchWireDecision::TransportRetryFailed;
                        Err(LatchedFbufStatusReadError {
                            source: second.source,
                            diagnostics: first.diagnostics,
                        })
                    }
                }
            }
            result => result,
        }
    }

    fn read_magik_latched_fbuf_status_once(
        &mut self,
    ) -> Result<LatchedFbufStatusSample, LatchedFbufStatusReadError> {
        let started = Instant::now();
        let mut attempt = LatchWireAttempt {
            command: MAGIK_UIO_GET_FBUF_LATCH,
            command_word: LatchWireWord {
                index: 0,
                transmitted: MAGIK_UIO_GET_FBUF_LATCH,
                ..LatchWireWord::default()
            },
            ..LatchWireAttempt::default()
        };
        self.enable_io();
        let command = match self.spi_capture_observed(MAGIK_UIO_GET_FBUF_LATCH) {
            Ok(capture) => {
                attempt.command_word.ack_high = Some(capture.ack_high);
                attempt.command_word.ack_low = Some(capture.ack_low);
                capture
            }
            Err(failure) => {
                attempt.command_word.ack_high = failure.ack_high;
                attempt.command_word.error_phase = failure.phase;
                attempt.elapsed_us = started.elapsed().as_micros() as u64;
                attempt.result = LatchWireResult::TransportError;
                self.disable_io();
                return Err(LatchedFbufStatusReadError::new(failure.error, attempt));
            }
        };
        let mut words = [0u16; mister_magik_latch_contract::STATUS_WORD_COUNT];
        for (index, word) in words.iter_mut().enumerate() {
            let mut observed = LatchWireWord {
                index: index as u8,
                transmitted: 0,
                ..LatchWireWord::default()
            };
            match self.spi_capture_observed(0) {
                Ok(capture) => {
                    observed.ack_high = Some(capture.ack_high);
                    observed.ack_low = Some(capture.ack_low);
                    *word = capture.ack_low;
                    attempt.response_words[index] = observed;
                    attempt.response_word_count = (index + 1) as u8;
                }
                Err(failure) => {
                    observed.ack_high = failure.ack_high;
                    observed.error_phase = failure.phase;
                    attempt.response_words[index] = observed;
                    attempt.response_word_count = (index + 1) as u8;
                    attempt.elapsed_us = started.elapsed().as_micros() as u64;
                    attempt.result = LatchWireResult::TransportError;
                    self.disable_io();
                    return Err(LatchedFbufStatusReadError::new(failure.error, attempt));
                }
            }
        }
        self.disable_io();
        let decoded = match mister_magik_latch_contract::decode_status(&words) {
            Ok(decoded) => decoded,
            Err(message) => {
                attempt.elapsed_us = started.elapsed().as_micros() as u64;
                attempt.result = LatchWireResult::DecodeError;
                return Err(LatchedFbufStatusReadError::new(
                    io::Error::new(io::ErrorKind::InvalidData, message),
                    attempt,
                ));
            }
        };
        attempt.elapsed_us = started.elapsed().as_micros() as u64;
        attempt.result = LatchWireResult::Decoded;
        Ok(LatchedFbufStatusSample {
            status: LatchedFbufStatus {
                magic_hi: command.ack_high,
                magic_lo: command.ack_low,
                active_sequence: decoded.active_seq,
                pending_sequence: decoded.pending_seq,
                flags: decoded.flags,
                flip_count: decoded.flip_count,
                post_count: decoded.post_count,
                drop_count: decoded.drop_count,
                active_base: decoded.base,
                active_width: decoded.width,
                active_height: decoded.height,
                active_stride: decoded.stride,
            },
            diagnostics: diagnostics_with_attempt(attempt, LatchWireDecision::Decoded),
        })
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
        if let Ok((_, _, capabilities)) = &result {
            self.latch_capabilities = Some(*capabilities);
        }
        result
    }

    pub fn negotiated_magik_latch_capabilities(
        &self,
    ) -> Option<mister_magik_latch_contract::LatchCapabilities> {
        self.latch_capabilities
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
    ) -> io::Result<u16> {
        let fb_addr = FB_ADDR + (FB_SIZE_PX * 4 * n) + if n == 0 { 4096 } else { 0 };
        // direct_video offsets: xoff = item[4] - FB_DV_LBRD, yoff = item[8] - FB_DV_UBRD.
        let xoff = mode.hbp as i32 - FB_DV_LBRD;
        let yoff = mode.vbp as i32 - FB_DV_UBRD;
        // Keep the full active width by default. Removing a guard column makes
        // exact ratios such as 960->1920 non-integral and produces a visible
        // scaler phase correction near the center. Retain the override for
        // targeted right-edge diagnostics.
        let right_guard_cols = std::env::var("MISTER_FB_RIGHT_GUARD_COLS")
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0)
            .clamp(0, mode.hact.saturating_sub(1) as i32);
        let [xoff, right, yoff, bottom] = diagnostic_fbuf_rectangle().unwrap_or([
            xoff,
            xoff + mode.hact as i32 - 1 - right_guard_cols,
            yoff,
            yoff + mode.vact as i32 - 1,
        ]);

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
        Ok(flag)
    }

    pub fn enable_launcher_framebuffer_route(
        &mut self,
        route: LauncherFramebufferRoute,
        fb_width: usize,
        fb_height: usize,
    ) -> io::Result<u16> {
        self.fb_enable_rgb565_with_placement(
            0,
            fb_width as u16,
            fb_height as u16,
            route.placement(),
        )
    }

    fn fb_enable_rgb565_with_placement(
        &mut self,
        n: u32,
        fb_width: u16,
        fb_height: u16,
        placement: FramebufferPlacement,
    ) -> io::Result<u16> {
        let mode = FramebufferRouteMode {
            hact: placement.width,
            hbp: placement.left.saturating_add(FB_DV_LBRD as u16),
            vact: placement.height,
            vbp: placement.top.saturating_add(FB_DV_UBRD as u16),
        };
        self.fb_enable_rgb565(n, fb_width, fb_height, mode)
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LatchedFbufStatusSample {
    pub status: LatchedFbufStatus,
    pub diagnostics: LatchWireDiagnostics,
}

#[derive(Debug)]
pub struct LatchedFbufStatusReadError {
    source: io::Error,
    pub diagnostics: LatchWireDiagnostics,
}

impl LatchedFbufStatusReadError {
    fn new(source: io::Error, attempt: LatchWireAttempt) -> Self {
        Self {
            source,
            diagnostics: diagnostics_with_attempt(attempt, LatchWireDecision::ReadFailed),
        }
    }

    pub fn kind(&self) -> io::ErrorKind {
        self.source.kind()
    }

    pub fn from_io(source: io::Error) -> Self {
        Self {
            source,
            diagnostics: LatchWireDiagnostics::default(),
        }
    }

    pub fn into_io(self) -> io::Error {
        self.source
    }
}

fn diagnostics_with_attempt(
    attempt: LatchWireAttempt,
    decision: LatchWireDecision,
) -> LatchWireDiagnostics {
    let mut diagnostics = LatchWireDiagnostics {
        decision,
        ..LatchWireDiagnostics::default()
    };
    diagnostics.push_attempt(attempt);
    diagnostics
}

impl std::fmt::Display for LatchedFbufStatusReadError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(output)
    }
}

impl std::error::Error for LatchedFbufStatusReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    #[derive(Default)]
    struct RegisterState {
        reads: VecDeque<u32>,
        default_read: u32,
        writes: Vec<u32>,
    }

    struct ScriptedRegisters(Rc<RefCell<RegisterState>>);

    impl RegisterIo for ScriptedRegisters {
        fn write_gpo(&mut self, value: u32) {
            self.0.borrow_mut().writes.push(value);
        }

        fn read_gpi(&mut self) -> u32 {
            let mut state = self.0.borrow_mut();
            state.reads.pop_front().unwrap_or(state.default_read)
        }
    }

    fn scripted(pairs: &[(u16, u16)]) -> (Fpga, Rc<RefCell<RegisterState>>) {
        let state = Rc::new(RefCell::new(RegisterState {
            reads: pairs
                .iter()
                .flat_map(|(hi, lo)| [ACK | u32::from(*hi), u32::from(*lo)])
                .collect(),
            ..RegisterState::default()
        }));
        let fpga = Fpga {
            registers: Box::new(ScriptedRegisters(Rc::clone(&state))),
            gpo: BIT31,
            latch_capabilities: None,
        };
        (fpga, state)
    }

    fn scripted_reads(
        reads: impl IntoIterator<Item = u32>,
        default_read: u32,
    ) -> (Fpga, Rc<RefCell<RegisterState>>) {
        let state = Rc::new(RefCell::new(RegisterState {
            reads: reads.into_iter().collect(),
            default_read,
            ..RegisterState::default()
        }));
        let fpga = Fpga {
            registers: Box::new(ScriptedRegisters(Rc::clone(&state))),
            gpo: BIT31,
            latch_capabilities: None,
        };
        (fpga, state)
    }

    fn status_pairs() -> Vec<(u16, u16)> {
        let status_words = [1, 2, 7, 3, 4, 5, 0x9000, 0x227e, 960, 540, 1920];
        let mut pairs = vec![(MAGIK_FBUF_STATUS_MAGIC, 0)];
        pairs.extend(
            status_words
                .into_iter()
                .enumerate()
                .map(|(index, word)| (0xa000 | index as u16, word)),
        );
        pairs
    }

    fn reads_from_pairs(pairs: &[(u16, u16)]) -> Vec<u32> {
        pairs
            .iter()
            .flat_map(|(hi, lo)| [ACK | u32::from(*hi), u32::from(*lo)])
            .collect()
    }

    fn words_from_writes(writes: &[u32]) -> Vec<u16> {
        writes
            .windows(2)
            .filter(|pair| pair[1] == pair[0] | STROBE)
            .map(|pair| pair[0] as u16)
            .collect()
    }

    #[test]
    fn framebuffer_sized_mode_uses_requested_active_area() {
        let mode = FramebufferRouteMode::framebuffer_sized(960, 540);

        assert_eq!(mode.hact, 960);
        assert_eq!(mode.vact, 540);
        assert_eq!(mode.hbp as i32 - FB_DV_LBRD, 0);
        assert_eq!(mode.vbp as i32 - FB_DV_UBRD, 0);
    }

    #[test]
    fn diagnostic_rectangle_parser_accepts_only_bounded_ordered_coordinates() {
        assert_eq!(
            parse_diagnostic_fbuf_rectangle("45,684,40,615"),
            Some([45, 684, 40, 615])
        );
        assert_eq!(parse_diagnostic_fbuf_rectangle("45,44,40,615"), None);
        assert_eq!(parse_diagnostic_fbuf_rectangle("-1,684,40,615"), None);
        assert_eq!(parse_diagnostic_fbuf_rectangle("45,684,40"), None);
        assert_eq!(parse_diagnostic_fbuf_rectangle("45,684,40,2048"), None);
    }

    #[test]
    fn spi_sequences_strobe_and_returns_both_ack_phases() {
        let (mut fpga, state) = scripted(&[(0x1234, 0x5678)]);

        assert_eq!(fpga.spi_capture(0xabcd).unwrap(), (0x1234, 0x5678));
        assert_eq!(
            state.borrow().writes,
            vec![BIT31 | 0xabcd, BIT31 | STROBE | 0xabcd, BIT31 | 0xabcd]
        );
    }

    #[test]
    fn spi_timeouts_restore_low_strobe_and_report_the_phase() {
        let (mut high_timeout, high_state) = scripted(&[]);
        let error = high_timeout.spi(0x55aa).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("ACK high"));
        assert_eq!(*high_state.borrow().writes.last().unwrap(), BIT31 | 0x55aa);

        let (mut low_timeout, low_state) = scripted(&[]);
        low_state.borrow_mut().default_read = ACK;
        let error = low_timeout.spi(0xaa55).unwrap_err();
        assert!(error.to_string().contains("ACK low"));
        assert_eq!(*low_state.borrow().writes.last().unwrap(), BIT31 | 0xaa55);
    }

    #[test]
    fn latch_status_timeout_resets_bus_and_retries_only_the_read_command() {
        let (mut fpga, state) = scripted(&[]);

        let error = fpga.read_magik_latched_fbuf_status_sample().unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(error.diagnostics.attempt_count, 2);
        assert_eq!(
            error.diagnostics.decision,
            LatchWireDecision::TransportRetryFailed
        );
        assert_eq!(
            error.diagnostics.attempts[0].command_word.error_phase,
            LatchWireErrorPhase::AckHigh
        );
        assert_eq!(
            words_from_writes(&state.borrow().writes)
                .into_iter()
                .filter(|word| *word == MAGIK_UIO_GET_FBUF_LATCH)
                .count(),
            2
        );
        assert!(state.borrow().writes.contains(&BIT31));
        assert_eq!(fpga.gpo & (IO_EN | STROBE), 0);
    }

    #[test]
    fn latch_status_command_ack_low_timeout_retains_ack_high() {
        let (mut fpga, _) = scripted_reads([ACK | u32::from(MAGIK_FBUF_STATUS_MAGIC)], ACK);

        let error = fpga.read_magik_latched_fbuf_status_once().unwrap_err();
        let attempt = error.diagnostics.attempts[0];

        assert_eq!(attempt.command_word.ack_high, Some(MAGIK_FBUF_STATUS_MAGIC));
        assert_eq!(attempt.command_word.ack_low, None);
        assert_eq!(
            attempt.command_word.error_phase,
            LatchWireErrorPhase::AckLow
        );
        assert_eq!(attempt.response_word_count, 0);
    }

    #[test]
    fn latch_status_response_timeouts_retain_preceding_words_and_failure_phase() {
        let pairs = status_pairs();
        let prefix = reads_from_pairs(&pairs[..3]);
        let (mut high_timeout, _) = scripted_reads(prefix, 0);

        let high_error = high_timeout
            .read_magik_latched_fbuf_status_once()
            .unwrap_err();
        let high_attempt = high_error.diagnostics.attempts[0];
        assert_eq!(high_attempt.response_word_count, 3);
        assert_eq!(high_attempt.response_words[1].ack_low, Some(2));
        assert_eq!(
            high_attempt.response_words[2].error_phase,
            LatchWireErrorPhase::AckHigh
        );

        let mut low_reads = reads_from_pairs(&pairs[..3]);
        low_reads.push(ACK | 0xa002);
        let (mut low_timeout, _) = scripted_reads(low_reads, ACK);
        let low_error = low_timeout
            .read_magik_latched_fbuf_status_once()
            .unwrap_err();
        let low_attempt = low_error.diagnostics.attempts[0];
        assert_eq!(low_attempt.response_word_count, 3);
        assert_eq!(low_attempt.response_words[1].ack_low, Some(2));
        assert_eq!(low_attempt.response_words[2].ack_high, Some(0xa002));
        assert_eq!(
            low_attempt.response_words[2].error_phase,
            LatchWireErrorPhase::AckLow
        );
    }

    #[test]
    fn latch_status_timeout_then_success_retains_both_attempts_in_order() {
        let pairs = status_pairs();
        let reads = std::iter::repeat(0)
            .take(SPIN_LIMIT as usize)
            .chain(reads_from_pairs(&pairs))
            .collect::<Vec<_>>();
        let (mut fpga, _) = scripted_reads(reads, 0);

        let sample = fpga.read_magik_latched_fbuf_status_sample().unwrap();

        assert_eq!(sample.diagnostics.attempt_count, 2);
        assert_eq!(
            sample.diagnostics.decision,
            LatchWireDecision::TransportRetryRecovered
        );
        assert_eq!(
            sample.diagnostics.attempts[0].command_word.error_phase,
            LatchWireErrorPhase::AckHigh
        );
        assert_eq!(
            sample.diagnostics.attempts[1].result,
            LatchWireResult::Decoded
        );
    }

    #[test]
    fn latch_status_partial_timeout_then_second_failure_retains_both_partials() {
        let pairs = status_pairs();
        let mut reads = reads_from_pairs(&pairs[..2]);
        reads.extend(std::iter::repeat(0).take(SPIN_LIMIT as usize));
        reads.extend(reads_from_pairs(&pairs[..3]));
        reads.push(ACK | 0xa002);
        let (mut fpga, _) = scripted_reads(reads, ACK);

        let error = fpga.read_magik_latched_fbuf_status_sample().unwrap_err();

        assert_eq!(error.diagnostics.attempt_count, 2);
        assert_eq!(
            error.diagnostics.decision,
            LatchWireDecision::TransportRetryFailed
        );
        assert_eq!(error.diagnostics.attempts[0].response_word_count, 2);
        assert_eq!(
            error.diagnostics.attempts[0].response_words[1].error_phase,
            LatchWireErrorPhase::AckHigh
        );
        assert_eq!(error.diagnostics.attempts[1].response_word_count, 3);
        assert_eq!(
            error.diagnostics.attempts[1].response_words[2].error_phase,
            LatchWireErrorPhase::AckLow
        );
    }

    #[test]
    fn latch_status_sample_retains_raw_high_and_low_phases() {
        let pairs = status_pairs();
        let (mut fpga, _) = scripted(&pairs);

        let sample = fpga.read_magik_latched_fbuf_status_sample().unwrap();

        assert_eq!(sample.status.active_base, 0x227e_9000);
        assert_eq!(sample.diagnostics.attempt_count, 1);
        let attempt = sample.diagnostics.attempts[0];
        assert_eq!(attempt.command_word.ack_high, Some(MAGIK_FBUF_STATUS_MAGIC));
        assert_eq!(attempt.response_word_count, 11);
        assert_eq!(attempt.response_words[8].ack_high, Some(0xa008));
        assert_eq!(attempt.response_words[8].ack_low, Some(960));
        assert_eq!(fpga.gpo & IO_EN, 0);
    }

    #[test]
    fn command_helpers_always_release_io_and_emit_expected_words() {
        let (mut fpga, state) = scripted(&[(1, 2), (3, 4)]);
        assert_eq!(fpga.uio_cmd16(0x10, 0x20).unwrap(), 4);
        assert_eq!(words_from_writes(&state.borrow().writes), vec![0x10, 0x20]);
        assert_eq!(fpga.gpo & IO_EN, 0);

        let (mut failed, failed_state) = scripted(&[]);
        assert!(failed.cmd_capture(0x33).is_err());
        assert_eq!(failed.gpo & IO_EN, 0);
        assert_eq!(*failed_state.borrow().writes.last().unwrap() & IO_EN, 0);
    }

    #[test]
    fn video_and_framebuffer_responses_decode_wire_order() {
        let video_words = [
            0x0301, 0x0040, 0x0001, 0x0020, 0, 10, 0, 20, 0, 30, 0, 40, 0, 50, 0, 2, 640, 480,
        ];
        let mut video_pairs = vec![(0xaaaa, 0xbbbb)];
        video_pairs.extend(video_words.map(|word| (0, word)));
        let (mut fpga, _) = scripted(&video_pairs);
        let info = fpga.read_video_info().unwrap();
        assert_eq!((info.width, info.height), (0x1_0040, 0x20));
        assert!(info.interlaced);
        assert!(info.rotated);
        assert!(info.log_line().contains("de=640x480"));

        let (mut fpga, _) = scripted(&[
            (0x12ab, 0),
            (0, 0x1456),
            (0, 0x0789),
            (0, FB_EN | 0x40 | FB_FMT_565),
            (0, 960),
            (0, 540),
        ]);
        let params = fpga.read_fb_params().unwrap();
        assert_eq!((params.crc, params.arx, params.ary), (0xab, 0x456, 0x789));
        assert!(params.arxy);
        assert!(params.fb_enabled);
        assert!(params.log_line().contains("960x540"));
    }

    #[test]
    fn framebuffer_commands_emit_complete_bounded_payloads() {
        let mode = FramebufferRouteMode::framebuffer_sized(960, 540);
        let (mut fpga, state) = scripted(&[(0x44, 0); 13]);
        let support = fpga.fb_enable_rgb565(0, 960, 540, mode).unwrap();
        assert_eq!(support, 0x44);
        let words = words_from_writes(&state.borrow().writes);
        assert!(words.starts_with(&[
            UIO_SET_FBUF,
            FB_EN | FB_FMT_565 | FB_FMT_RXB,
            (FB_ADDR + 4096) as u16,
            ((FB_ADDR + 4096) >> 16) as u16,
            960,
            540,
        ]));
        assert_eq!(words.len(), 11);
        assert_eq!(*words.last().unwrap(), rgb565_stride_bytes(960) as u16);
        assert_eq!(fpga.gpo & IO_EN, 0);

        let geometry = LatchedFbufGeometry::new(960, mode, 1);
        let (mut fpga, state) = scripted(&[(0x55, 0); 12]);
        assert_eq!(
            fpga.post_magik_latched_fbuf_rgb565(7, FB_ADDR, 960, 540, geometry)
                .unwrap(),
            (0x55, 0)
        );
        let words = words_from_writes(&state.borrow().writes);
        assert_eq!(words[0], MAGIK_UIO_SET_FBUF_LATCH);
        assert_eq!(*words.last().unwrap(), 7);
        assert_eq!(fpga.gpo & IO_EN, 0);
    }

    #[test]
    fn production_crt_routes_emit_exact_destinations_through_both_paths() {
        for (scan_h, expected) in [
            (240, [67, 706, 12, 251]),
            (288, [67, 706, 12, 299]),
            (480, [45, 684, 31, 510]),
            (576, [45, 684, 40, 615]),
        ] {
            let route = LauncherFramebufferRoute::for_scan(640, scan_h, true);
            let geometry = LatchedFbufGeometry::new_for_route(640, route, 0);
            assert_eq!(
                [
                    geometry.xoff,
                    geometry.right,
                    geometry.yoff,
                    geometry.bottom,
                ],
                expected
            );

            let (mut fpga, state) = scripted(&[(0x44, 0); 13]);
            fpga.enable_launcher_framebuffer_route(route, 640, scan_h as usize)
                .unwrap();
            let words = words_from_writes(&state.borrow().writes);
            assert_eq!(&words[6..10], &expected);
        }
    }

    #[test]
    fn latched_status_flags_have_stable_meanings() {
        let status = LatchedFbufStatus {
            magic_hi: MAGIK_FBUF_STATUS_MAGIC,
            magic_lo: 0,
            active_sequence: 1,
            pending_sequence: 2,
            flags: 0x0007,
            flip_count: 3,
            post_count: 4,
            drop_count: 5,
            active_base: FB_ADDR,
            active_width: 960,
            active_height: 540,
            active_stride: 1920,
        };
        assert!(status.supported());
        assert!(status.active_enabled());
        assert!(status.pending_enabled());
        assert!(status.pending());
    }
}
