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

// HPS framebuffer constants (video.cpp).
pub const FB_ADDR: u32 = 0x2000_0000 + (32 * 1024 * 1024); // 0x22000000
pub const FB_SIZE_PX: u32 = 1920 * 1080;
pub const FB_FMT_8888: u16 = 0b00110;
pub const FB_FMT_RXB: u16 = 0b10000;
pub const FB_EN: u16 = 0x8000;
pub const FB_DV_LBRD: i32 = 3;
pub const FB_DV_UBRD: i32 = 2;

/// Timing of one MiSTer video mode (vmode_t.item[1..8]): hact, hfp, hs, hbp,
/// vact, vfp, vs, vbp. We only need hact/hbp/vact/vbp for fb positioning.
#[derive(Clone, Copy)]
pub struct Mode {
    pub hact: u16,
    pub hbp: u16,
    pub vact: u16,
    pub vbp: u16,
}

/// video_mode=8 → 1920x1080@60 (vmodes[8] = {1920,88,44,148,1080,4,5,36}).
pub const MODE_1080P60: Mode = Mode {
    hact: 1920,
    hbp: 148,
    vact: 1080,
    vbp: 36,
};

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
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/mem")?;
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
    pub fn cmd_cont(&mut self, cmd: u16) -> u16 {
        self.enable_io();
        self.spi(cmd)
    }

    /// Like `cmd_cont` but captures both ACK phases of the command word.
    pub fn cmd_capture(&mut self, cmd: u16) -> (u16, u16) {
        self.enable_io();
        self.spi_capture(cmd)
    }

    /// Port of `video_fb_enable(1, n)` for a `direct_video` system, replicating
    /// the exact SET_FBUF sequence in video.cpp:3290-3321. Routes HPS buffer `n`
    /// to the scan-out. `mode` is the active video mode (for positioning); the
    /// fb itself is `fb_width`x`fb_height` (1920x1080 for us).
    ///
    /// Returns the SET_FBUF support flag (non-zero = core supports the HPS fb).
    pub fn fb_enable_direct(
        &mut self,
        n: u32,
        fb_width: u16,
        fb_height: u16,
        mode: Mode,
        xoff_override: Option<i32>,
        yoff_override: Option<i32>,
    ) -> u16 {
        let fb_addr = FB_ADDR + (FB_SIZE_PX * 4 * n) + if n == 0 { 4096 } else { 0 };
        // direct_video offsets: xoff = item[4] - FB_DV_LBRD, yoff = item[8] - FB_DV_UBRD.
        let xoff = xoff_override.unwrap_or(mode.hbp as i32 - FB_DV_LBRD);
        let yoff = yoff_override.unwrap_or(mode.vbp as i32 - FB_DV_UBRD);

        // Clean chip-select edge first (we may be interrupting a stopped MiSTer
        // mid-transaction), then send the command and read its support flag from
        // the ACK-high phase.
        self.disable_io();
        let (flag, _) = self.cmd_capture(UIO_SET_FBUF);

        self.spi_w(FB_EN | FB_FMT_RXB | FB_FMT_8888); // format + enable
        self.spi_w(fb_addr as u16); // base addr low
        self.spi_w((fb_addr >> 16) as u16); // base addr high
        self.spi_w(fb_width); // frame width
        self.spi_w(fb_height); // frame height
        self.spi_w(xoff as u16); // scaled left
        self.spi_w((xoff + mode.hact as i32 - 1) as u16); // scaled right
        self.spi_w(yoff as u16); // scaled top
        self.spi_w((yoff + mode.vact as i32 - 1) as u16); // scaled bottom
        self.spi_w(fb_width * 4); // stride (bytes)
        self.disable_io();
        flag
    }
}

impl Drop for Fpga {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.base as *mut libc::c_void, MGR_LEN);
        }
    }
}
