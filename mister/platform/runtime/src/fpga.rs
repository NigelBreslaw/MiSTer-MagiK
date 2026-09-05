// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Native port of MiSTer's HPS↔FPGA "SPI" layer (the GPO/GPI bit-bang in
//! `fpga_io.cpp` / `spi.cpp`). Native register access supports multi-word reads
//! and observation of the handshake phases.
//!
//! The "SPI" is just two memory-mapped registers in the FPGA manager:
//!   GPO (write, 0xFF706000+0x10) and GPI (read, +0x14).
//! GPO is write-only, so we keep a software shadow (`gpo`), exactly like MiSTer's
//! `gpo_copy`. Bit31 must stay set (it means "configured"); bit20 is the IO chip
//! select (EnableIO/DisableIO); bit17 is the strobe; the low 16 bits are data.

use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::ptr::{read_volatile, write_volatile};
use std::rc::Rc;
use std::time::Instant;

use crate::framebuffer::route::{
    FramebufferPlacement, FramebufferRouteMode, LauncherFramebufferRoute,
};
use crate::latch_readiness::{
    LatchPostDiagnostics, LatchPostWord, LatchRejectionObservation, LatchStatusObservation,
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
pub const MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS: u16 =
    mister_magik_latch_contract::GET_FBUF_LATCH_DIAGNOSTICS;
pub const MAGIK_UIO_GET_FBUF_LATCH_RECEIPT: u16 =
    mister_magik_latch_contract::GET_FBUF_LATCH_RECEIPT;
pub const MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY: u16 =
    mister_magik_latch_contract::GET_FBUF_PRESENTATION_TELEMETRY;
pub const MAGIK_FBUF_LATCH_MAGIC: u16 = mister_magik_latch_contract::LATCH_MAGIC;
pub const MAGIK_FBUF_STATUS_MAGIC: u16 = mister_magik_latch_contract::STATUS_MAGIC;
pub const MAGIK_FBUF_CAPS_MAGIC: u16 = mister_magik_latch_contract::CAPS_MAGIC;
pub const MAGIK_FBUF_DIAGNOSTICS_MAGIC: u16 = mister_magik_latch_contract::DIAGNOSTICS_MAGIC;
pub const MAGIK_FBUF_RECEIPT_MAGIC: u16 = mister_magik_latch_contract::RECEIPT_MAGIC;
pub const MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC: u16 =
    mister_magik_latch_contract::PRESENTATION_TELEMETRY_MAGIC;

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
    uio_lock: Option<File>,
    uio_lock_depth: Rc<Cell<u32>>,
}

pub struct FpgaUioGuard {
    fd: std::os::fd::RawFd,
    depth: Rc<Cell<u32>>,
}

impl Drop for FpgaUioGuard {
    fn drop(&mut self) {
        let depth = self.depth.get();
        debug_assert!(depth > 0);
        let remaining = depth.saturating_sub(1);
        self.depth.set(remaining);
        if remaining != 0 {
            return;
        }
        // SAFETY: fd belongs to the live lock file held by Fpga for longer than
        // this guard; LOCK_UN only releases this process's advisory flock.
        unsafe {
            libc::flock(self.fd, libc::LOCK_UN);
        }
    }
}

impl Fpga {
    fn spi_timeout(phase: &str, word: u16) -> io::Error {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("FPGA SPI timeout waiting for ACK {phase} on word 0x{word:04x}"),
        )
    }

    pub fn open() -> io::Result<Self> {
        fs::create_dir_all("/tmp/mister-magik")?;
        let uio_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(mister_magik_latch_contract::FPGA_UIO_LOCK_PATH)?;
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
            uio_lock: Some(uio_lock),
            uio_lock_depth: Rc::new(Cell::new(0)),
        })
    }

    fn lock_uio_transaction(&self) -> io::Result<Option<FpgaUioGuard>> {
        let Some(lock) = self.uio_lock.as_ref() else {
            return Ok(None);
        };
        let fd = lock.as_raw_fd();
        let depth = self.uio_lock_depth.get();
        if depth == 0 {
            // SAFETY: fd is a valid open lock file descriptor owned by self.
            if unsafe { libc::flock(fd, libc::LOCK_EX) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        self.uio_lock_depth.set(depth.saturating_add(1));
        Ok(Some(FpgaUioGuard {
            fd,
            depth: Rc::clone(&self.uio_lock_depth),
        }))
    }

    pub fn lock_latch_transaction(&self) -> io::Result<Option<FpgaUioGuard>> {
        self.lock_uio_transaction()
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
    /// permits comparison of the ACK-high and ACK-low phases at native speed.
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
        let _uio_guard = self.lock_uio_transaction()?;
        self.uio_cmd16(UIO_AUDVOL, attenuation as u16)?;
        Ok(())
    }

    pub fn read_video_info(&mut self) -> io::Result<VideoInfo> {
        let _uio_guard = self.lock_uio_transaction()?;
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
        let _uio_guard = self.lock_uio_transaction()?;
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
        let _uio_guard = self
            .lock_uio_transaction()
            .map_err(LatchedFbufStatusReadError::from_io)?;
        let Some(protocol) = self
            .latch_capabilities
            .map(|capabilities| capabilities.protocol)
        else {
            return Err(LatchedFbufStatusReadError::from_io(io::Error::new(
                io::ErrorKind::InvalidData,
                "latch status read requires successful capability negotiation",
            )));
        };
        match self.read_magik_latched_fbuf_status_once(protocol) {
            Err(mut first)
                if first.source.kind() == io::ErrorKind::TimedOut
                    || (protocol == mister_magik_latch_contract::LatchProtocol::V5
                        && first.source.kind() == io::ErrorKind::InvalidData) =>
            {
                // GET_FBUF_LATCH is an idempotent status query. Return the bus
                // to a known idle state and retry it once; never apply this to
                // framebuffer posts, whose payload may already be committed.
                self.reset_spi_transport();
                match self.read_magik_latched_fbuf_status_once(protocol) {
                    Ok(mut sample) => {
                        first.diagnostics.append(&sample.diagnostics);
                        first.diagnostics.decision = LatchWireDecision::TransportRetryRecovered;
                        sample.diagnostics = *first.diagnostics;
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
        protocol: mister_magik_latch_contract::LatchProtocol,
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
                return Err(self.latched_status_read_error(failure.error, attempt, protocol));
            }
        };
        let mut words = [0u16; mister_magik_latch_contract::V5_STATUS_WORDS];
        for (index, word) in words
            .iter_mut()
            .take(protocol.status_word_count())
            .enumerate()
        {
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
                    return Err(self.latched_status_read_error(failure.error, attempt, protocol));
                }
            }
        }
        self.disable_io();
        let decoded = match mister_magik_latch_contract::decode_status(
            protocol,
            &words[..protocol.status_word_count()],
        ) {
            Ok(decoded) => decoded,
            Err(message) => {
                attempt.elapsed_us = started.elapsed().as_micros() as u64;
                attempt.result = LatchWireResult::DecodeError;
                return Err(self.latched_status_read_error(
                    io::Error::new(io::ErrorKind::InvalidData, message),
                    attempt,
                    protocol,
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
                reject_count: decoded.reject_count,
                active_route_epoch: decoded.active_route_epoch,
                accepted_sequence: decoded.accepted_seq,
                active_transaction: decoded.active_transaction,
                pending_transaction: decoded.pending_transaction,
                accepted_transaction: decoded.accepted_transaction,
            },
            diagnostics: {
                let mut diagnostics = diagnostics_with_attempt(attempt, LatchWireDecision::Decoded);
                diagnostics.protocol_version = Some(protocol.version());
                diagnostics.capability_flags = self
                    .latch_capabilities
                    .map(|capabilities| capabilities.flags);
                diagnostics
            },
        })
    }

    fn read_magik_latched_fbuf_receipt_unlocked(
        &mut self,
    ) -> io::Result<mister_magik_latch_contract::LatchReceipt> {
        let command = self.cmd_capture(MAGIK_UIO_GET_FBUF_LATCH_RECEIPT)?;
        if command.0 != MAGIK_FBUF_RECEIPT_MAGIC && command.1 != MAGIK_FBUF_RECEIPT_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported latch receipt magic high=0x{:04x} low=0x{:04x}",
                    command.0, command.1
                ),
            ));
        }
        let mut words = [0u16; mister_magik_latch_contract::V5_RECEIPT_WORDS];
        for word in &mut words {
            *word = self.spi_capture(0)?.1;
        }
        mister_magik_latch_contract::decode_receipt(&words)
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
    }

    fn latched_status_read_error(
        &self,
        source: io::Error,
        attempt: LatchWireAttempt,
        protocol: mister_magik_latch_contract::LatchProtocol,
    ) -> LatchedFbufStatusReadError {
        let mut error = LatchedFbufStatusReadError::new(source, attempt);
        error.diagnostics.protocol_version = Some(protocol.version());
        error.diagnostics.capability_flags = self
            .latch_capabilities
            .map(|capabilities| capabilities.flags);
        error
    }

    pub fn probe_magik_latched_fbuf_set(&mut self) -> io::Result<(u16, u16)> {
        let _uio_guard = self.lock_uio_transaction()?;
        let res = self.cmd_capture(MAGIK_UIO_SET_FBUF_LATCH);
        self.disable_io();
        res
    }

    pub fn read_magik_latched_fbuf_capabilities(
        &mut self,
    ) -> io::Result<(u16, u16, mister_magik_latch_contract::LatchCapabilities)> {
        let _uio_guard = self.lock_uio_transaction()?;
        self.latch_capabilities = None;
        let result = match self.read_magik_latched_fbuf_capabilities_once() {
            Err((first, Some(mister_magik_latch_contract::LatchProtocol::V5)))
                if first.kind() == io::ErrorKind::InvalidData =>
            {
                self.reset_spi_transport();
                self.read_magik_latched_fbuf_capabilities_once()
                    .map_err(|(error, _)| error)
            }
            result => result.map_err(|(error, _)| error),
        };
        if let Ok((magic_hi, magic_lo, capabilities)) = &result
            && (*magic_hi == MAGIK_FBUF_CAPS_MAGIC || *magic_lo == MAGIK_FBUF_CAPS_MAGIC)
            && capabilities.production_ready()
        {
            self.latch_capabilities = Some(*capabilities);
        }
        result
    }

    fn read_magik_latched_fbuf_capabilities_once(
        &mut self,
    ) -> Result<
        (u16, u16, mister_magik_latch_contract::LatchCapabilities),
        (
            io::Error,
            Option<mister_magik_latch_contract::LatchProtocol>,
        ),
    > {
        let mut observed_protocol = None;
        let result = (|| {
            let (magic_hi, magic_lo) = self.cmd_capture(MAGIK_UIO_GET_FBUF_LATCH_CAPS)?;
            let mut words = [0u16; mister_magik_latch_contract::V5_CAPS_WORDS];
            words[0] = self.spi_capture(0)?.1;
            let protocol = mister_magik_latch_contract::LatchProtocol::try_from(words[0])
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            observed_protocol = Some(protocol);
            for word in words.iter_mut().take(protocol.caps_word_count()).skip(1) {
                *word = self.spi_capture(0)?.1;
            }
            let capabilities = mister_magik_latch_contract::decode_capabilities(
                &words[..protocol.caps_word_count()],
            )
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            Ok((magic_hi, magic_lo, capabilities))
        })();
        self.disable_io();
        match result {
            Ok(value) => Ok(value),
            Err(error) => Err((error, observed_protocol)),
        }
    }

    pub fn negotiated_magik_latch_capabilities(
        &self,
    ) -> Option<mister_magik_latch_contract::LatchCapabilities> {
        self.latch_capabilities
    }

    pub fn read_magik_presentation_telemetry(
        &mut self,
    ) -> io::Result<mister_magik_latch_contract::PresentationTelemetry> {
        let _uio_guard = self.lock_uio_transaction()?;
        let Some(capabilities) = self.latch_capabilities else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "presentation telemetry requires successful capability negotiation",
            ));
        };
        if capabilities.protocol != mister_magik_latch_contract::LatchProtocol::V5
            || capabilities.flags
                & mister_magik_latch_contract::CAP_AUTHORITATIVE_PRESENTATION_TELEMETRY
                == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "authoritative presentation telemetry is not supported",
            ));
        }
        self.disable_io();
        let result = (|| {
            let command = self.cmd_capture(MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY)?;
            if command.0 != MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC
                && command.1 != MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "unsupported presentation telemetry magic high=0x{:04x} low=0x{:04x}",
                        command.0, command.1
                    ),
                ));
            }
            let mut words = [0u16; mister_magik_latch_contract::V5_PRESENTATION_TELEMETRY_WORDS];
            for word in &mut words {
                *word = self.spi_capture(0)?.1;
            }
            mister_magik_latch_contract::decode_presentation_telemetry(&words)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))
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
        self.post_magik_latched_fbuf_rgb565_observed(
            sequence, base_addr, fb_width, fb_height, geometry, None,
        )
        .map(|attempt| (attempt.ack_high, attempt.ack_low))
        .map_err(LatchedFbufPostError::into_io)
    }

    pub fn post_magik_latched_fbuf_rgb565_observed(
        &mut self,
        sequence: u16,
        base_addr: u32,
        fb_width: u16,
        fb_height: u16,
        geometry: LatchedFbufGeometry,
        injected_skip_index: Option<usize>,
    ) -> Result<LatchedFbufPostAttempt, LatchedFbufPostError> {
        let _uio_guard = self
            .lock_uio_transaction()
            .map_err(LatchedFbufPostError::from_io)?;
        let started = Instant::now();
        self.disable_io();
        let protocol = match self
            .latch_capabilities
            .map(|capabilities| capabilities.protocol)
        {
            Some(protocol) => protocol,
            None => {
                return Err(LatchedFbufPostError::from_io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "latch post requires successful capability negotiation",
                )));
            }
        };
        let words = mister_magik_latch_contract::encode_set(
            protocol,
            mister_magik_latch_contract::LatchSetPayload {
                mode: FB_EN | FB_FMT_565 | FB_FMT_RXB,
                base: base_addr,
                width: fb_width,
                height: fb_height,
                destination_left: geometry.xoff,
                destination_right: geometry.right,
                destination_top: geometry.yoff,
                destination_bottom: geometry.bottom,
                stride: geometry.stride_bytes,
                sequence,
            },
        );
        let injected_skip_index = injected_skip_index.filter(|index| *index < words.word_count);
        let mut diagnostics = LatchPostDiagnostics {
            protocol_version: protocol.version(),
            sequence,
            command_word: LatchPostWord {
                transmitted: MAGIK_UIO_SET_FBUF_LATCH,
                ..LatchPostWord::default()
            },
            expected_word_count: words.word_count as u8,
            injected_skip_index: injected_skip_index.map(|index| index as u8),
            ..LatchPostDiagnostics::default()
        };
        self.enable_io();
        let command_started = Instant::now();
        let support = match self.spi_capture_observed(MAGIK_UIO_SET_FBUF_LATCH) {
            Ok(capture) => {
                diagnostics.command_word.ack_high = Some(capture.ack_high);
                diagnostics.command_word.ack_low = Some(capture.ack_low);
                diagnostics.command_word.elapsed_us = command_started.elapsed().as_micros() as u64;
                capture
            }
            Err(failure) => {
                diagnostics.command_word.ack_high = failure.ack_high;
                diagnostics.command_word.error_phase = failure.phase;
                diagnostics.command_word.elapsed_us = command_started.elapsed().as_micros() as u64;
                diagnostics.total_elapsed_us = started.elapsed().as_micros() as u64;
                self.disable_io();
                return Err(LatchedFbufPostError {
                    source: failure.error,
                    diagnostics: Box::new(diagnostics),
                });
            }
        };
        for (index, word) in words
            .words
            .iter()
            .take(words.word_count)
            .copied()
            .enumerate()
        {
            let word_started = Instant::now();
            let mut observed = LatchPostWord {
                index: index as u8,
                transmitted: word,
                injected_skip: injected_skip_index == Some(index),
                ..LatchPostWord::default()
            };
            if observed.injected_skip {
                diagnostics.words[index] = observed;
                diagnostics.word_count = (index + 1) as u8;
                continue;
            }
            match self.spi_capture_observed(word) {
                Ok(capture) => {
                    observed.ack_high = Some(capture.ack_high);
                    observed.ack_low = Some(capture.ack_low);
                    observed.elapsed_us = word_started.elapsed().as_micros() as u64;
                    diagnostics.words[index] = observed;
                    diagnostics.word_count = (index + 1) as u8;
                    diagnostics.transmitted_word_count += 1;
                }
                Err(failure) => {
                    observed.ack_high = failure.ack_high;
                    observed.error_phase = failure.phase;
                    observed.elapsed_us = word_started.elapsed().as_micros() as u64;
                    diagnostics.words[index] = observed;
                    diagnostics.word_count = (index + 1) as u8;
                    diagnostics.total_elapsed_us = started.elapsed().as_micros() as u64;
                    self.disable_io();
                    return Err(LatchedFbufPostError {
                        source: failure.error,
                        diagnostics: Box::new(diagnostics),
                    });
                }
            }
        }
        // GET_FBUF_LATCH_RECEIPT is a distinct UIO command. Deassert IO_EN
        // after the SET stream so the sys_top bridge resets its command
        // framing before the receipt opcode is sent.
        self.disable_io();
        let receipt = match self.read_magik_latched_fbuf_receipt_unlocked() {
            Ok(receipt) => receipt,
            Err(source) => {
                self.disable_io();
                diagnostics.total_elapsed_us = started.elapsed().as_micros() as u64;
                return Err(LatchedFbufPostError {
                    source,
                    diagnostics: Box::new(diagnostics),
                });
            }
        };
        diagnostics.attempted_transaction = receipt.attempted_transaction;
        diagnostics.receipt_disposition = receipt.disposition;
        diagnostics.accepted_transaction = receipt.accepted_transaction;
        diagnostics.accepted_sequence = receipt.accepted_sequence;
        diagnostics.pending_transaction = receipt.pending_transaction;
        diagnostics.pending_sequence = receipt.pending_sequence;
        diagnostics.active_transaction = receipt.active_transaction;
        diagnostics.active_sequence = receipt.active_sequence;
        diagnostics.receipt_reject_reason = receipt.reject_reason;
        diagnostics.receipt_crc = receipt.crc;
        if !receipt.accepted()
            || receipt.attempted_sequence != sequence
            || receipt.accepted_sequence != sequence
            || receipt.accepted_transaction != receipt.attempted_transaction
            || !((receipt.pending_transaction == receipt.attempted_transaction
                && receipt.pending_sequence == sequence)
                || (receipt.active_transaction == receipt.attempted_transaction
                    && receipt.active_sequence == sequence))
        {
            self.disable_io();
            diagnostics.total_elapsed_us = started.elapsed().as_micros() as u64;
            return Err(LatchedFbufPostError {
                source: io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "latch SET rejected or ambiguous: attempted_tx={} attempted_seq={} disposition={} accepted_tx={} accepted_seq={} pending_tx={} pending_seq={} active_tx={} active_seq={} reject_reason={}",
                        receipt.attempted_transaction,
                        receipt.attempted_sequence,
                        receipt.disposition,
                        receipt.accepted_transaction,
                        receipt.accepted_sequence,
                        receipt.pending_transaction,
                        receipt.pending_sequence,
                        receipt.active_transaction,
                        receipt.active_sequence,
                        receipt.reject_reason
                    ),
                ),
                diagnostics: Box::new(diagnostics),
            });
        }
        self.disable_io();
        diagnostics.total_elapsed_us = started.elapsed().as_micros() as u64;
        Ok(LatchedFbufPostAttempt {
            ack_high: support.ack_high,
            ack_low: support.ack_low,
            diagnostics,
        })
    }

    pub fn read_magik_latched_fbuf_rejection_diagnostics(
        &mut self,
    ) -> io::Result<Option<LatchRejectionObservation>> {
        let _uio_guard = self.lock_uio_transaction()?;
        let Some(capabilities) = self.latch_capabilities else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "latch rejection diagnostics require successful capability negotiation",
            ));
        };
        if capabilities.protocol != mister_magik_latch_contract::LatchProtocol::V5
            || capabilities.flags & mister_magik_latch_contract::CAP_REJECTION_CONTEXT == 0
        {
            return Ok(None);
        }
        self.disable_io();
        let result = (|| {
            let command = self.cmd_capture(MAGIK_UIO_GET_FBUF_LATCH_DIAGNOSTICS)?;
            if command.0 != MAGIK_FBUF_DIAGNOSTICS_MAGIC
                && command.1 != MAGIK_FBUF_DIAGNOSTICS_MAGIC
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "unsupported latch rejection diagnostics magic high=0x{:04x} low=0x{:04x}",
                        command.0, command.1
                    ),
                ));
            }
            let word_count = capabilities
                .protocol
                .diagnostics_word_count()
                .expect("protocol v5 has rejection diagnostics");
            let mut words = [0u16; mister_magik_latch_contract::V5_DIAGNOSTICS_WORDS];
            for word in words.iter_mut().take(word_count) {
                *word = self.spi_capture(0)?.1;
            }
            let decoded = mister_magik_latch_contract::decode_rejection_diagnostics(
                capabilities.protocol,
                &words[..word_count],
            )
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
            Ok(Some(LatchRejectionObservation {
                reject_count: decoded.reject_count,
                reason: decoded.reason,
                expected_index: decoded.expected_index,
                observed_index: decoded.observed_index,
                observed_command: decoded.observed_command,
                receiver_open: decoded.receiver_open,
                receiver_faulted: decoded.receiver_faulted,
                crc: decoded.crc,
            }))
        })();
        self.disable_io();
        result
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
        let _uio_guard = self.lock_uio_transaction()?;
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
    pub reject_count: u16,
    pub active_route_epoch: u16,
    pub accepted_sequence: u16,
    pub active_transaction: u16,
    pub pending_transaction: u16,
    pub accepted_transaction: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LatchedFbufPostAttempt {
    pub ack_high: u16,
    pub ack_low: u16,
    pub diagnostics: LatchPostDiagnostics,
}

#[derive(Debug)]
pub struct LatchedFbufPostError {
    source: io::Error,
    pub diagnostics: Box<LatchPostDiagnostics>,
}

impl LatchedFbufPostError {
    pub fn from_io(source: io::Error) -> Self {
        Self {
            source,
            diagnostics: Box::default(),
        }
    }

    pub fn into_io(self) -> io::Error {
        self.source
    }
}

impl std::fmt::Display for LatchedFbufPostError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(output)
    }
}

impl std::error::Error for LatchedFbufPostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LatchedFbufStatusSample {
    pub status: LatchedFbufStatus,
    pub diagnostics: LatchWireDiagnostics,
}

#[derive(Debug)]
pub struct LatchedFbufStatusReadError {
    source: io::Error,
    pub diagnostics: Box<LatchWireDiagnostics>,
}

impl LatchedFbufStatusReadError {
    fn new(source: io::Error, attempt: LatchWireAttempt) -> Self {
        Self {
            source,
            diagnostics: Box::new(diagnostics_with_attempt(
                attempt,
                LatchWireDecision::ReadFailed,
            )),
        }
    }

    pub fn kind(&self) -> io::ErrorKind {
        self.source.kind()
    }

    pub fn from_io(source: io::Error) -> Self {
        Self {
            source,
            diagnostics: Box::default(),
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

    pub fn magik_owned(self) -> bool {
        (self.flags & (1 << mister_magik_latch_contract::STATUS_MAGIK_OWNERSHIP)) != 0
    }

    pub fn rejection_reason(self) -> u8 {
        ((self.flags >> mister_magik_latch_contract::STATUS_REJECT_REASON_SHIFT)
            & ((1 << mister_magik_latch_contract::STATUS_REJECT_REASON_WIDTH) - 1)) as u8
    }
}

impl From<LatchedFbufStatus> for LatchStatusObservation {
    fn from(status: LatchedFbufStatus) -> Self {
        Self {
            active_sequence: status.active_sequence,
            pending_sequence: status.pending_sequence,
            flags: status.flags,
            active_enabled: status.active_enabled(),
            pending_enabled: status.pending_enabled(),
            pending: status.pending(),
            magik_owned: status.magik_owned(),
            flip_count: status.flip_count,
            post_count: status.post_count,
            drop_count: status.drop_count,
            reject_count: status.reject_count,
            rejection_reason: status.rejection_reason(),
            active_route_epoch: status.active_route_epoch,
            accepted_sequence: status.accepted_sequence,
            active_transaction: status.active_transaction,
            pending_transaction: status.pending_transaction,
            accepted_transaction: status.accepted_transaction,
            active_base: status.active_base,
            active_width: status.active_width,
            active_height: status.active_height,
            active_stride: status.active_stride,
        }
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
            latch_capabilities: Some(v5_capabilities()),
            uio_lock: None,
            uio_lock_depth: Rc::new(Cell::new(0)),
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
            latch_capabilities: Some(v5_capabilities()),
            uio_lock: None,
            uio_lock_depth: Rc::new(Cell::new(0)),
        };
        (fpga, state)
    }

    fn status_pairs() -> Vec<(u16, u16)> {
        let mut pairs = vec![(MAGIK_FBUF_STATUS_MAGIC, 0)];
        pairs.extend(
            mister_magik_latch_contract::GOLDEN_STATUS_V5_PAYLOAD
                .into_iter()
                .enumerate()
                .map(|(index, word)| (0xa000 | index as u16, word)),
        );
        pairs.push((0xa00f, mister_magik_latch_contract::GOLDEN_STATUS_V5_CRC));
        pairs
    }

    fn v5_capabilities() -> mister_magik_latch_contract::LatchCapabilities {
        let mut words = [0; mister_magik_latch_contract::V5_CAPS_WORDS];
        words[..5].copy_from_slice(&mister_magik_latch_contract::GOLDEN_CAPS_V5_PAYLOAD);
        words[5] = mister_magik_latch_contract::GOLDEN_CAPS_V5_CRC;
        mister_magik_latch_contract::decode_capabilities(&words).unwrap()
    }

    fn v5_capability_pairs(crc: u16) -> Vec<(u16, u16)> {
        let mut pairs = vec![(MAGIK_FBUF_CAPS_MAGIC, 0)];
        pairs.extend(mister_magik_latch_contract::GOLDEN_CAPS_V5_PAYLOAD.map(|word| (0, word)));
        pairs.push((0, crc));
        pairs
    }

    fn v5_status_pairs(crc: u16) -> Vec<(u16, u16)> {
        let mut pairs = vec![(MAGIK_FBUF_STATUS_MAGIC, 0)];
        pairs.extend(
            mister_magik_latch_contract::GOLDEN_STATUS_V5_PAYLOAD
                .into_iter()
                .enumerate()
                .map(|(index, word)| (0xa000 | index as u16, word)),
        );
        pairs.push((0xa00f, crc));
        pairs
    }

    fn v5_receipt_pairs(sequence: u16, accepted: bool) -> Vec<(u16, u16)> {
        let disposition = if accepted {
            mister_magik_latch_contract::RECEIPT_ACCEPTED
        } else {
            mister_magik_latch_contract::RECEIPT_REJECTED
        };
        let transaction = 1;
        let payload = if accepted {
            [
                transaction,
                sequence,
                disposition,
                transaction,
                sequence,
                transaction,
                sequence,
                0,
                0,
                0,
            ]
        } else {
            [
                transaction,
                sequence,
                disposition,
                0,
                0,
                0,
                0,
                0,
                0,
                u16::from(mister_magik_latch_contract::REJECT_MISSING_WORD),
            ]
        };
        let crc = mister_magik_latch_contract::message_crc(
            MAGIK_UIO_GET_FBUF_LATCH_RECEIPT,
            mister_magik_latch_contract::LatchProtocol::V5,
            &payload,
        );
        let mut pairs = vec![(MAGIK_FBUF_RECEIPT_MAGIC, 0)];
        pairs.extend(payload.map(|word| (0, word)));
        pairs.push((0, crc));
        pairs
    }

    fn v5_presentation_telemetry_pairs(crc: u16) -> Vec<(u16, u16)> {
        let mut pairs = vec![(MAGIK_FBUF_PRESENTATION_TELEMETRY_MAGIC, 0)];
        pairs.extend(
            mister_magik_latch_contract::GOLDEN_PRESENTATION_TELEMETRY_V5_PAYLOAD
                .map(|word| (0, word)),
        );
        pairs.push((0, crc));
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

        let error = fpga
            .read_magik_latched_fbuf_status_once(mister_magik_latch_contract::LatchProtocol::V5)
            .unwrap_err();
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
            .read_magik_latched_fbuf_status_once(mister_magik_latch_contract::LatchProtocol::V5)
            .unwrap_err();
        let high_attempt = high_error.diagnostics.attempts[0];
        assert_eq!(high_attempt.response_word_count, 3);
        assert_eq!(high_attempt.response_words[1].ack_low, Some(43));
        assert_eq!(
            high_attempt.response_words[2].error_phase,
            LatchWireErrorPhase::AckHigh
        );

        let mut low_reads = reads_from_pairs(&pairs[..3]);
        low_reads.push(ACK | 0xa002);
        let (mut low_timeout, _) = scripted_reads(low_reads, ACK);
        let low_error = low_timeout
            .read_magik_latched_fbuf_status_once(mister_magik_latch_contract::LatchProtocol::V5)
            .unwrap_err();
        let low_attempt = low_error.diagnostics.attempts[0];
        assert_eq!(low_attempt.response_word_count, 3);
        assert_eq!(low_attempt.response_words[1].ack_low, Some(43));
        assert_eq!(low_attempt.response_words[2].ack_high, Some(0xa002));
        assert_eq!(
            low_attempt.response_words[2].error_phase,
            LatchWireErrorPhase::AckLow
        );
    }

    #[test]
    fn latch_status_timeout_then_success_retains_both_attempts_in_order() {
        let pairs = status_pairs();
        let reads = std::iter::repeat_n(0, SPIN_LIMIT as usize)
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
        reads.extend(std::iter::repeat_n(0, SPIN_LIMIT as usize));
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
        assert_eq!(attempt.response_word_count, 16);
        assert_eq!(attempt.response_words[7].ack_high, Some(0xa007));
        assert_eq!(attempt.response_words[7].ack_low, Some(960));
        assert_eq!(fpga.gpo & IO_EN, 0);
    }

    #[test]
    fn v5_capability_crc_retries_once_without_downgrading() {
        let mut pairs = v5_capability_pairs(mister_magik_latch_contract::GOLDEN_CAPS_V5_CRC ^ 1);
        pairs.extend(v5_capability_pairs(
            mister_magik_latch_contract::GOLDEN_CAPS_V5_CRC,
        ));
        let (mut fpga, state) = scripted(&pairs);

        let (_, _, capabilities) = fpga.read_magik_latched_fbuf_capabilities().unwrap();

        assert_eq!(
            capabilities.protocol,
            mister_magik_latch_contract::LatchProtocol::V5
        );
        assert_eq!(
            words_from_writes(&state.borrow().writes)
                .into_iter()
                .filter(|word| *word == MAGIK_UIO_GET_FBUF_LATCH_CAPS)
                .count(),
            2
        );

        let mut pairs = v5_capability_pairs(mister_magik_latch_contract::GOLDEN_CAPS_V5_CRC ^ 1);
        pairs.extend(v5_capability_pairs(
            mister_magik_latch_contract::GOLDEN_CAPS_V5_CRC ^ 2,
        ));
        let (mut failed, _) = scripted(&pairs);
        let error = failed.read_magik_latched_fbuf_capabilities().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(failed.latch_capabilities.is_none());

        let reads = reads_from_pairs(&[(MAGIK_FBUF_CAPS_MAGIC, 0), (0, 4)]);
        let (mut obsolete_protocol, _) = scripted_reads(reads, 0);
        obsolete_protocol.latch_capabilities = Some(v5_capabilities());
        let error = obsolete_protocol
            .read_magik_latched_fbuf_capabilities()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(obsolete_protocol.latch_capabilities.is_none());

        let obsolete_protocol = [(MAGIK_FBUF_CAPS_MAGIC, 0), (0, 4)];
        let (mut malformed, _) = scripted(&obsolete_protocol);
        assert!(malformed.read_magik_latched_fbuf_capabilities().is_err());
        assert!(malformed.latch_capabilities.is_none());
    }

    #[test]
    fn v5_status_crc_retries_once_without_protocol_fallback() {
        let mut pairs = v5_status_pairs(mister_magik_latch_contract::GOLDEN_STATUS_V5_CRC ^ 1);
        pairs.extend(v5_status_pairs(
            mister_magik_latch_contract::GOLDEN_STATUS_V5_CRC,
        ));
        let (mut fpga, _) = scripted(&pairs);
        fpga.latch_capabilities = Some(v5_capabilities());

        let sample = fpga.read_magik_latched_fbuf_status_sample().unwrap();

        assert_eq!(sample.status.reject_count, 7);
        assert_eq!(sample.status.active_route_epoch, 9);
        assert_eq!(sample.diagnostics.protocol_version, Some(5));
        assert_eq!(sample.diagnostics.attempt_count, 2);
        assert_eq!(
            sample.diagnostics.decision,
            LatchWireDecision::TransportRetryRecovered
        );

        let mut pairs = v5_status_pairs(mister_magik_latch_contract::GOLDEN_STATUS_V5_CRC ^ 1);
        pairs.extend(v5_status_pairs(
            mister_magik_latch_contract::GOLDEN_STATUS_V5_CRC ^ 2,
        ));
        let (mut failed, _) = scripted(&pairs);
        failed.latch_capabilities = Some(v5_capabilities());
        let error = failed.read_magik_latched_fbuf_status_sample().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.diagnostics.protocol_version, Some(5));
        assert_eq!(
            error.diagnostics.decision,
            LatchWireDecision::TransportRetryFailed
        );
        assert_eq!(error.diagnostics.attempt_count, 2);
    }

    #[test]
    fn status_and_set_refuse_to_guess_a_protocol_before_negotiation() {
        let (mut status_fpga, status_state) = scripted(&[]);
        status_fpga.latch_capabilities = None;
        let error = status_fpga
            .read_magik_latched_fbuf_status_sample()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(status_state.borrow().writes.is_empty());

        let (mut set_fpga, set_state) = scripted(&[]);
        set_fpga.latch_capabilities = None;
        let error = set_fpga
            .post_magik_latched_fbuf_rgb565(
                1,
                FB_ADDR,
                960,
                540,
                LatchedFbufGeometry {
                    xoff: 0,
                    right: 959,
                    yoff: 0,
                    bottom: 539,
                    stride_bytes: 1920,
                },
            )
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(words_from_writes(&set_state.borrow().writes).is_empty());
    }

    #[test]
    fn v5_set_posts_crc_once_and_is_never_retried() {
        let mut pairs = vec![(MAGIK_FBUF_LATCH_MAGIC, 0); 13];
        pairs.extend(v5_receipt_pairs(42, true));
        let (mut fpga, state) = scripted(&pairs);
        fpga.latch_capabilities = Some(v5_capabilities());
        let geometry = LatchedFbufGeometry {
            xoff: 0,
            right: 959,
            yoff: 0,
            bottom: 539,
            stride_bytes: 1920,
        };

        let attempt = fpga
            .post_magik_latched_fbuf_rgb565_observed(42, 0x227e_9000, 960, 540, geometry, None)
            .unwrap();

        let words = words_from_writes(&state.borrow().writes);
        assert_eq!(words[0], MAGIK_UIO_SET_FBUF_LATCH);
        let mut expected_payload = mister_magik_latch_contract::GOLDEN_SET_V5_PAYLOAD;
        expected_payload[10] = 42;
        assert_eq!(&words[1..12], &expected_payload);
        assert_eq!(
            words[12],
            mister_magik_latch_contract::message_crc(
                MAGIK_UIO_SET_FBUF_LATCH,
                mister_magik_latch_contract::LatchProtocol::V5,
                &expected_payload,
            )
        );
        assert_eq!(
            words
                .iter()
                .filter(|word| **word == MAGIK_UIO_SET_FBUF_LATCH)
                .count(),
            1
        );
        let writes = &state.borrow().writes;
        let strobed = writes
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair[1] == pair[0] | STROBE)
            .map(|(index, pair)| (index, pair[0] as u16))
            .collect::<Vec<_>>();
        let set_crc_write = strobed[12].0;
        let receipt_command_write = strobed
            .iter()
            .find(|(_, word)| *word == MAGIK_UIO_GET_FBUF_LATCH_RECEIPT)
            .unwrap()
            .0;
        assert!(
            writes[set_crc_write + 2..receipt_command_write]
                .iter()
                .any(|write| write & IO_EN == 0),
            "SET and receipt commands must be separated by an IO_EN-low boundary"
        );
        assert_eq!(attempt.diagnostics.protocol_version, 5);
        assert_eq!(attempt.diagnostics.sequence, 42);
        assert_eq!(attempt.diagnostics.word_count, 12);
        assert_eq!(attempt.diagnostics.expected_word_count, 12);
        assert_eq!(attempt.diagnostics.transmitted_word_count, 12);
        assert_eq!(attempt.diagnostics.words[11].transmitted, words[12]);
        assert!(attempt.diagnostics.command_word.ack_high.is_some());
        assert!(attempt.diagnostics.words[11].ack_low.is_some());
    }

    #[test]
    fn v5_set_can_omit_one_word_once_for_bounded_receiver_reproduction() {
        let mut pairs = vec![(MAGIK_FBUF_LATCH_MAGIC, 0); 12];
        pairs.extend(v5_receipt_pairs(42, false));
        let (mut fpga, state) = scripted(&pairs);
        fpga.latch_capabilities = Some(v5_capabilities());
        let geometry = LatchedFbufGeometry {
            xoff: 0,
            right: 959,
            yoff: 0,
            bottom: 539,
            stride_bytes: 1920,
        };

        let error = fpga
            .post_magik_latched_fbuf_rgb565_observed(42, 0x227e_9000, 960, 540, geometry, Some(4))
            .unwrap_err();

        let words = words_from_writes(&state.borrow().writes);
        let set_words = &words[..12];
        assert_eq!(set_words[0], MAGIK_UIO_SET_FBUF_LATCH);
        assert!(!set_words[1..].contains(&mister_magik_latch_contract::GOLDEN_SET_V5_PAYLOAD[4]));
        assert!(words.contains(&MAGIK_UIO_GET_FBUF_LATCH_RECEIPT));
        assert_eq!(error.diagnostics.injected_skip_index, Some(4));
        assert!(error.diagnostics.words[4].injected_skip);
        assert_eq!(error.diagnostics.transmitted_word_count, 11);
        assert_eq!(error.diagnostics.expected_word_count, 12);
        assert_eq!(
            error.diagnostics.receipt_disposition,
            mister_magik_latch_contract::RECEIPT_REJECTED
        );
    }

    #[test]
    fn v5_rejection_diagnostics_decode_receiver_context() {
        let mut pairs = vec![(MAGIK_FBUF_DIAGNOSTICS_MAGIC, 0)];
        pairs.extend(
            mister_magik_latch_contract::GOLDEN_DIAGNOSTICS_V5_PAYLOAD.map(|word| (0, word)),
        );
        pairs.push((0, mister_magik_latch_contract::GOLDEN_DIAGNOSTICS_V5_CRC));
        let (mut fpga, _) = scripted(&pairs);
        fpga.latch_capabilities = Some(v5_capabilities());

        let observation = fpga
            .read_magik_latched_fbuf_rejection_diagnostics()
            .unwrap()
            .unwrap();

        assert_eq!(observation.reject_count, 7);
        assert_eq!(observation.reason, 1);
        assert_eq!(observation.expected_index, 11);
        assert_eq!(observation.observed_index, 0);
        assert_eq!(observation.observed_command, 0x58);
    }

    #[test]
    fn v5_presentation_telemetry_validates_magic_crc_and_word_order() {
        let pairs = v5_presentation_telemetry_pairs(
            mister_magik_latch_contract::GOLDEN_PRESENTATION_TELEMETRY_V5_CRC,
        );
        let (mut fpga, state) = scripted(&pairs);
        let telemetry = fpga.read_magik_presentation_telemetry().unwrap();
        assert_eq!(telemetry.owned_vblank_count, 0x1234_5678);
        assert_eq!(telemetry.presented_vblank_count, 0x1234_5670);
        assert_eq!(telemetry.repeated_vblank_count, 8);
        assert_eq!(telemetry.ownership_loss_count, 3);
        assert!(telemetry.lifetime_invariant_valid());
        assert!(
            words_from_writes(&state.borrow().writes)
                .contains(&MAGIK_UIO_GET_FBUF_PRESENTATION_TELEMETRY)
        );

        let bad_pairs = v5_presentation_telemetry_pairs(
            mister_magik_latch_contract::GOLDEN_PRESENTATION_TELEMETRY_V5_CRC ^ 1,
        );
        let (mut bad_crc, _) = scripted(&bad_pairs);
        assert_eq!(
            bad_crc
                .read_magik_presentation_telemetry()
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let mut no_capabilities = v5_capabilities();
        no_capabilities.flags &=
            !mister_magik_latch_contract::CAP_AUTHORITATIVE_PRESENTATION_TELEMETRY;
        let (mut unsupported, _) = scripted(&[]);
        unsupported.latch_capabilities = Some(no_capabilities);
        assert_eq!(
            unsupported
                .read_magik_presentation_telemetry()
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
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
        let mut pairs = vec![(0x55, 0); 13];
        pairs.extend(v5_receipt_pairs(7, true));
        let (mut fpga, state) = scripted(&pairs);
        assert_eq!(
            fpga.post_magik_latched_fbuf_rgb565(7, FB_ADDR, 960, 540, geometry)
                .unwrap(),
            (0x55, 0)
        );
        let words = words_from_writes(&state.borrow().writes);
        assert_eq!(words[0], MAGIK_UIO_SET_FBUF_LATCH);
        assert!(words.contains(&7));
        assert!(words.contains(&MAGIK_UIO_GET_FBUF_LATCH_RECEIPT));
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
            reject_count: 0,
            active_route_epoch: 0,
            accepted_sequence: 2,
            active_transaction: 1,
            pending_transaction: 2,
            accepted_transaction: 2,
        };
        assert!(status.supported());
        assert!(status.active_enabled());
        assert!(status.pending_enabled());
        assert!(status.pending());
    }
}
