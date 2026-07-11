//! Experimental mappings exposed by the stock-kernel plugin probe.
//!
//! The plugin maps framebuffer-owned physical ranges with write-combined
//! attributes. This module validates the reported regions before the launcher
//! uses them as Main-flippable hidden RGB565 buffers.

use crate::framebuffer::format::rgb565_stride_bytes;
use crate::framebuffer::hidden::{HiddenFramebufferError, HiddenRgb565BufferIndex};
use slint::platform::software_renderer::Rgb565Pixel;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

static ATOMIC_SCANOUT_DISABLED: AtomicBool = AtomicBool::new(false);
static ATOMIC_SCANOUT_STATE: AtomicU8 = AtomicU8::new(0);

pub const PLUGIN_PROBE_DEVICE: &str = "/dev/mister-magik-plugin-probe";
pub const PLUGIN_SCANOUT_DEVICE: &str = "/dev/mister-magik-scanout";
pub const PLUGIN_PROBE_MIN_VERSION: u32 = 2;
pub const PLUGIN_PROBE_REGION_OFFSET_BYTES: usize = 1024 * 1024;
pub const PLUGIN_HIDDEN_SLOT_FRAME_BYTES: usize = 960 * 540 * 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanoutMode {
    Auto,
    Legacy,
    Required,
}

impl ScanoutMode {
    pub fn wants_atomic(self) -> bool {
        !matches!(self, Self::Legacy)
    }
}

pub fn configured_scanout_mode() -> ScanoutMode {
    match std::env::var("MISTER_SCANOUT_MODE").ok().as_deref() {
        Some("auto") => ScanoutMode::Auto,
        Some("required") => ScanoutMode::Required,
        Some("legacy") => ScanoutMode::Legacy,
        Some(_) => ScanoutMode::Legacy,
        None if std::env::var("MISTER_TRUE_ZERO_COPY").ok().as_deref() == Some("1") => {
            ScanoutMode::Required
        }
        None => ScanoutMode::Legacy,
    }
}

pub fn atomic_scanout_runtime_enabled() -> bool {
    configured_scanout_mode().wants_atomic() && !ATOMIC_SCANOUT_DISABLED.load(Ordering::Acquire)
}

pub fn disable_atomic_scanout() {
    ATOMIC_SCANOUT_DISABLED.store(true, Ordering::Release);
    ATOMIC_SCANOUT_STATE.store(4, Ordering::Release);
}

pub fn mark_atomic_scanout_target_ready() {
    ATOMIC_SCANOUT_STATE.fetch_max(2, Ordering::AcqRel);
}

pub fn mark_atomic_scanout_active() {
    ATOMIC_SCANOUT_STATE.store(3, Ordering::Release);
}

pub fn scanout_runtime_state_label() -> &'static str {
    match ATOMIC_SCANOUT_STATE.load(Ordering::Acquire) {
        2 => "target-ready",
        3 => "active",
        4 => "fallback",
        _ if configured_scanout_mode().wants_atomic() => "requested",
        _ => "legacy",
    }
}

const SCANOUT_SLOT_COUNT: usize = 2;
const SCANOUT_MAX_RANGES: usize = 64;
const SCANOUT_GET_CAPS: libc::c_ulong = 0x80184d20;
const SCANOUT_ACQUIRE_CPU: libc::c_ulong = 0x40044d21;
const SCANOUT_SYNC_DEVICE: libc::c_ulong = 0x42084d22;
const SCANOUT_GET_CAPS_V2: libc::c_ulong = 0x80244d23;
const SCANOUT_ARM_MAILBOX: libc::c_ulong = 0x40084d24;
const SCANOUT_SYNC_RANGES_AND_POST: libc::c_ulong = 0x42344d25;
const SCANOUT_GET_STATUS: libc::c_ulong = 0x80244d26;

const SCANOUT_CAPABILITIES_V2: u32 = 0x0f;
const NO_SLOT: u32 = u32::MAX;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScanoutCaps {
    abi_version: u32,
    slot_count: u32,
    slot_bytes: u32,
    mmap_stride: u32,
    dma_addr: [u32; SCANOUT_SLOT_COUNT],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScanoutCapsV2 {
    abi_version: u32,
    capabilities: u32,
    slot_count: u32,
    slot_bytes: u32,
    mmap_stride: u32,
    mailbox_phys: u32,
    mailbox_epoch: u32,
    dma_addr: [u32; SCANOUT_SLOT_COUNT],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScanoutRange {
    offset: u32,
    length: u32,
}

#[repr(C)]
struct ScanoutSync {
    slot: u32,
    range_count: u32,
    ranges: [ScanoutRange; SCANOUT_MAX_RANGES],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScanoutMailboxArm {
    epoch: u32,
    fpga_capabilities: u32,
}

#[repr(C)]
struct ScanoutPost {
    slot: u32,
    range_count: u32,
    sequence: u32,
    enable: u32,
    filter: u32,
    format: u32,
    width: u32,
    height: u32,
    stride: u32,
    hmin: u32,
    hmax: u32,
    vmin: u32,
    vmax: u32,
    ranges: [ScanoutRange; SCANOUT_MAX_RANGES],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScanoutStatusRaw {
    mailbox_armed: u32,
    active_sequence: u32,
    pending_sequence: u32,
    active_slot: u32,
    pending_slot: u32,
    slot_state: [u32; SCANOUT_SLOT_COUNT],
    completion_count: u32,
    error_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanoutSlotState {
    CpuOwned,
    DeviceQueued,
    DeviceActive,
    CpuReleased,
    Unknown(u32),
}

impl From<u32> for ScanoutSlotState {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::CpuOwned,
            1 => Self::DeviceQueued,
            2 => Self::DeviceActive,
            3 => Self::CpuReleased,
            value => Self::Unknown(value),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanoutStatus {
    pub mailbox_armed: bool,
    pub active_sequence: u32,
    pub pending_sequence: u32,
    pub active_slot: Option<usize>,
    pub pending_slot: Option<usize>,
    pub slot_state: [ScanoutSlotState; SCANOUT_SLOT_COUNT],
    pub completion_count: u32,
    pub error_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanoutPostRoute {
    pub enable: bool,
    pub filter: bool,
    pub format: u8,
    pub width: u16,
    pub height: u16,
    pub stride: u16,
    pub hmin: u16,
    pub hmax: u16,
    pub vmin: u16,
    pub vmax: u16,
}

pub trait Rgb565BlitTarget {
    #[allow(clippy::too_many_arguments)]
    fn copy_rect_565_strided(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
        src_x: usize,
        src_y: usize,
    ) -> Result<usize, PluginProbeError>;
}

pub struct ScanoutPixelsTarget<'a> {
    pixels: &'a mut [Rgb565Pixel],
    width: usize,
    height: usize,
    stride_pixels: usize,
}

impl<'a> ScanoutPixelsTarget<'a> {
    pub fn new(
        pixels: &'a mut [Rgb565Pixel],
        width: usize,
        height: usize,
        stride_pixels: usize,
    ) -> Self {
        Self {
            pixels,
            width,
            height,
            stride_pixels,
        }
    }
}

impl Rgb565BlitTarget for ScanoutPixelsTarget<'_> {
    fn copy_rect_565_strided(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
        src_x: usize,
        src_y: usize,
    ) -> Result<usize, PluginProbeError> {
        validate_strided_copy(
            self.width,
            self.height,
            x,
            y,
            w,
            h,
            src,
            src_stride_pixels,
            src_x,
            src_y,
        )?;
        copy_rect_565_strided_pixels(
            self.pixels,
            self.stride_pixels,
            x,
            y,
            w,
            h,
            src,
            src_stride_pixels,
            src_x,
            src_y,
        );
        Ok(w * h * std::mem::size_of::<Rgb565Pixel>())
    }
}

pub struct PluginScanoutRgb565Buffers {
    device: File,
    maps: [*mut Rgb565Pixel; SCANOUT_SLOT_COUNT],
    map_len: usize,
    frame_pixels: usize,
    stride_pixels: usize,
    dma_addr: [u32; SCANOUT_SLOT_COUNT],
    mailbox: Option<(u32, u32)>,
}

impl PluginScanoutRgb565Buffers {
    pub fn open(width: usize, height: usize) -> io::Result<Self> {
        Self::open_inner(width, height, false)
    }

    pub fn open_atomic(width: usize, height: usize) -> io::Result<Self> {
        Self::open_inner(width, height, true)
    }

    fn open_inner(width: usize, height: usize, atomic: bool) -> io::Result<Self> {
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(PLUGIN_SCANOUT_DEVICE)?;
        let frame_pixels = width
            .checked_mul(height)
            .ok_or_else(|| io::Error::other("scanout geometry overflow"))?;
        let frame_bytes = frame_pixels
            .checked_mul(2)
            .ok_or_else(|| io::Error::other("scanout geometry overflow"))?;
        let (slot_bytes, mmap_stride, dma_addr, mailbox) = if atomic {
            let mut caps = ScanoutCapsV2::default();
            if unsafe { libc::ioctl(device.as_raw_fd(), SCANOUT_GET_CAPS_V2, &mut caps) } != 0 {
                return Err(io::Error::last_os_error());
            }
            if caps.abi_version != 2
                || caps.capabilities != SCANOUT_CAPABILITIES_V2
                || caps.slot_count != 2
                || caps.mailbox_phys == 0
                || caps.mailbox_epoch == 0
            {
                return Err(io::Error::other("unsupported atomic scanout capabilities"));
            }
            (
                caps.slot_bytes,
                caps.mmap_stride,
                caps.dma_addr,
                Some((caps.mailbox_phys, caps.mailbox_epoch)),
            )
        } else {
            let mut caps = ScanoutCaps::default();
            if unsafe { libc::ioctl(device.as_raw_fd(), SCANOUT_GET_CAPS, &mut caps) } != 0 {
                return Err(io::Error::last_os_error());
            }
            if caps.abi_version != 1 || caps.slot_count != 2 {
                return Err(io::Error::other("unsupported scanout capabilities"));
            }
            (caps.slot_bytes, caps.mmap_stride, caps.dma_addr, None)
        };
        if frame_bytes > slot_bytes as usize {
            return Err(io::Error::other(
                "scanout slots are smaller than the render target",
            ));
        }
        let mut maps: [*mut Rgb565Pixel; SCANOUT_SLOT_COUNT] =
            [std::ptr::null_mut(); SCANOUT_SLOT_COUNT];
        for (slot, map) in maps.iter_mut().enumerate() {
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    frame_bytes,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    device.as_raw_fd(),
                    (slot * mmap_stride as usize) as libc::off_t,
                )
            };
            if ptr == libc::MAP_FAILED {
                for prior in maps.iter().take(slot) {
                    unsafe {
                        libc::munmap((*prior).cast(), frame_bytes);
                    }
                }
                return Err(io::Error::last_os_error());
            }
            *map = ptr.cast();
        }
        Ok(Self {
            device,
            maps,
            map_len: frame_bytes,
            frame_pixels,
            stride_pixels: width,
            dma_addr,
            mailbox,
        })
    }

    pub fn mailbox(&self) -> io::Result<(u32, u32)> {
        self.mailbox
            .ok_or_else(|| io::Error::other("atomic scanout mailbox unavailable"))
    }

    pub fn arm_mailbox(&self, fpga_capabilities: u16) -> io::Result<()> {
        let (_, epoch) = self.mailbox()?;
        let arm = ScanoutMailboxArm {
            epoch,
            fpga_capabilities: u32::from(fpga_capabilities),
        };
        if unsafe { libc::ioctl(self.device.as_raw_fd(), SCANOUT_ARM_MAILBOX, &arm) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn dma_addr(&self, slot: usize) -> u32 {
        self.dma_addr[slot]
    }
    pub fn stride_pixels(&self) -> usize {
        self.stride_pixels
    }
    pub fn pixels(&self, slot: usize) -> &[Rgb565Pixel] {
        unsafe { std::slice::from_raw_parts(self.maps[slot], self.frame_pixels) }
    }
    pub fn pixels_mut(&mut self, slot: usize) -> &mut [Rgb565Pixel] {
        unsafe { std::slice::from_raw_parts_mut(self.maps[slot], self.frame_pixels) }
    }
    pub fn target_mut(
        &mut self,
        slot: usize,
        width: usize,
        height: usize,
    ) -> ScanoutPixelsTarget<'_> {
        let stride_pixels = self.stride_pixels;
        ScanoutPixelsTarget::new(self.pixels_mut(slot), width, height, stride_pixels)
    }
    pub fn acquire(&self, slot: usize) -> io::Result<()> {
        let value = slot as u32;
        if unsafe { libc::ioctl(self.device.as_raw_fd(), SCANOUT_ACQUIRE_CPU, &value) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn sync_rects(
        &self,
        slot: usize,
        rects: &[crate::framebuffer::target::DirtyRect],
    ) -> io::Result<()> {
        let mut request = ScanoutSync {
            slot: slot as u32,
            range_count: 0,
            ranges: [ScanoutRange::default(); SCANOUT_MAX_RANGES],
        };
        for rect in rects.iter().take(SCANOUT_MAX_RANGES) {
            let start = (rect.y0 * self.stride_pixels + rect.x0) * 2;
            let end = ((rect.y1 - 1) * self.stride_pixels + rect.x1) * 2;
            request.ranges[request.range_count as usize] = ScanoutRange {
                offset: start as u32,
                length: (end - start) as u32,
            };
            request.range_count += 1;
        }
        if unsafe { libc::ioctl(self.device.as_raw_fd(), SCANOUT_SYNC_DEVICE, &request) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn sync_ranges_and_post(
        &self,
        slot: usize,
        sequence: u32,
        rects: &[crate::framebuffer::target::DirtyRect],
        route: ScanoutPostRoute,
    ) -> io::Result<()> {
        let mut request = ScanoutPost {
            slot: slot as u32,
            range_count: 0,
            sequence,
            enable: u32::from(route.enable),
            filter: u32::from(route.filter),
            format: u32::from(route.format),
            width: u32::from(route.width),
            height: u32::from(route.height),
            stride: u32::from(route.stride),
            hmin: u32::from(route.hmin),
            hmax: u32::from(route.hmax),
            vmin: u32::from(route.vmin),
            vmax: u32::from(route.vmax),
            ranges: [ScanoutRange::default(); SCANOUT_MAX_RANGES],
        };
        append_scanout_ranges(
            &mut request.ranges,
            &mut request.range_count,
            rects,
            self.stride_pixels,
        )?;
        if unsafe {
            libc::ioctl(
                self.device.as_raw_fd(),
                SCANOUT_SYNC_RANGES_AND_POST,
                &request,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn status(&self) -> io::Result<ScanoutStatus> {
        let mut raw = ScanoutStatusRaw::default();
        if unsafe { libc::ioctl(self.device.as_raw_fd(), SCANOUT_GET_STATUS, &mut raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(ScanoutStatus {
            mailbox_armed: raw.mailbox_armed != 0,
            active_sequence: raw.active_sequence,
            pending_sequence: raw.pending_sequence,
            active_slot: (raw.active_slot != NO_SLOT).then_some(raw.active_slot as usize),
            pending_slot: (raw.pending_slot != NO_SLOT).then_some(raw.pending_slot as usize),
            slot_state: raw.slot_state.map(ScanoutSlotState::from),
            completion_count: raw.completion_count,
            error_count: raw.error_count,
        })
    }
}

fn append_scanout_ranges(
    output: &mut [ScanoutRange; SCANOUT_MAX_RANGES],
    count: &mut u32,
    rects: &[crate::framebuffer::target::DirtyRect],
    stride_pixels: usize,
) -> io::Result<()> {
    if rects.len() > SCANOUT_MAX_RANGES {
        return Err(io::Error::other(
            "too many dirty ranges for atomic scanout post",
        ));
    }
    for rect in rects {
        if rect.x0 >= rect.x1 || rect.y0 >= rect.y1 || rect.x1 > stride_pixels {
            return Err(io::Error::other(
                "invalid dirty rectangle for atomic scanout post",
            ));
        }
        let start = (rect.y0 * stride_pixels + rect.x0) * 2;
        let end = ((rect.y1 - 1) * stride_pixels + rect.x1) * 2;
        output[*count as usize] = ScanoutRange {
            offset: start as u32,
            length: (end - start) as u32,
        };
        *count += 1;
    }
    Ok(())
}

impl Drop for PluginScanoutRgb565Buffers {
    fn drop(&mut self) {
        for map in self.maps {
            unsafe {
                libc::munmap(map.cast(), self.map_len);
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginProbeHeader {
    pub name: String,
    pub version: u32,
    pub uts_release: String,
    pub region_offset_bytes: usize,
    pub cache_mode: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginProbeRegion {
    pub index: usize,
    pub name: String,
    pub available: bool,
    pub phys: String,
    pub len: usize,
    pub dma_owned: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginProbeMetadata {
    pub header: PluginProbeHeader,
    pub regions: Vec<PluginProbeRegion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginProbeError {
    Io(String),
    MissingHeader,
    UnsupportedVersion {
        version: u32,
        min_version: u32,
    },
    UnsupportedRegionStride {
        region_offset_bytes: usize,
    },
    UnsupportedCacheMode {
        cache_mode: String,
    },
    MissingRegion {
        name: String,
    },
    RegionUnavailable {
        name: String,
    },
    RegionTooSmall {
        name: String,
        len: usize,
        required: usize,
    },
    RegionIsDmaOwned {
        name: String,
    },
    InvalidGeometry(String),
    SourceTooShort {
        needed: usize,
        actual: usize,
    },
    MmapFailed(String),
    MmapReturnedNull,
    InvalidPhysicalAddress {
        name: String,
        phys: String,
    },
}

impl std::fmt::Display for PluginProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "plugin probe I/O failed: {e}"),
            Self::MissingHeader => write!(f, "plugin probe metadata is missing a header"),
            Self::UnsupportedVersion {
                version,
                min_version,
            } => write!(
                f,
                "plugin probe version {version} is older than required version {min_version}"
            ),
            Self::UnsupportedRegionStride {
                region_offset_bytes,
            } => write!(
                f,
                "plugin probe region stride {region_offset_bytes} does not match expected {PLUGIN_PROBE_REGION_OFFSET_BYTES}"
            ),
            Self::UnsupportedCacheMode { cache_mode } => {
                write!(f, "plugin probe cache mode {cache_mode} is not writecombine")
            }
            Self::MissingRegion { name } => write!(f, "plugin probe region {name} is missing"),
            Self::RegionUnavailable { name } => {
                write!(f, "plugin probe region {name} is unavailable")
            }
            Self::RegionTooSmall {
                name,
                len,
                required,
            } => write!(
                f,
                "plugin probe region {name} has {len} bytes, need {required}"
            ),
            Self::RegionIsDmaOwned { name } => {
                write!(f, "plugin probe region {name} is plugin-owned DMA memory")
            }
            Self::InvalidGeometry(e) => write!(f, "invalid plugin hidden framebuffer geometry: {e}"),
            Self::SourceTooShort { needed, actual } => {
                write!(f, "plugin hidden source has {actual} pixels, need {needed}")
            }
            Self::MmapFailed(e) => write!(f, "plugin probe mmap failed: {e}"),
            Self::MmapReturnedNull => write!(f, "plugin probe mmap returned a null address"),
            Self::InvalidPhysicalAddress { name, phys } => {
                write!(f, "plugin probe region {name} has invalid physical address {phys}")
            }
        }
    }
}

impl std::error::Error for PluginProbeError {}

impl From<io::Error> for PluginProbeError {
    fn from(value: io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

pub fn read_plugin_probe_metadata() -> Result<PluginProbeMetadata, PluginProbeError> {
    parse_plugin_probe_metadata(&fs::read_to_string(PLUGIN_PROBE_DEVICE)?)
}

pub fn parse_plugin_probe_metadata(text: &str) -> Result<PluginProbeMetadata, PluginProbeError> {
    let header = text
        .lines()
        .find_map(parse_plugin_probe_header)
        .ok_or(PluginProbeError::MissingHeader)?;
    let regions = text.lines().filter_map(parse_plugin_probe_region).collect();
    Ok(PluginProbeMetadata { header, regions })
}

pub fn parse_plugin_probe_header(line: &str) -> Option<PluginProbeHeader> {
    if !line.starts_with("plugin_probe_header_tsv\t") {
        return None;
    }
    let mut name = None;
    let mut version = None;
    let mut uts_release = None;
    let mut region_offset_bytes = None;
    let mut cache_mode = None;
    for field in line.split('\t').skip(1) {
        let (key, value) = field.split_once('=')?;
        match key {
            "name" => name = Some(value.to_string()),
            "version" => version = value.parse::<u32>().ok(),
            "uts_release" => uts_release = Some(value.to_string()),
            "region_offset_bytes" => region_offset_bytes = value.parse::<usize>().ok(),
            "cache_mode" => cache_mode = Some(value.to_string()),
            _ => {}
        }
    }
    Some(PluginProbeHeader {
        name: name?,
        version: version?,
        uts_release: uts_release?,
        region_offset_bytes: region_offset_bytes?,
        cache_mode: cache_mode?,
    })
}

pub fn parse_plugin_probe_region(line: &str) -> Option<PluginProbeRegion> {
    if !line.starts_with("plugin_probe_region_tsv\t") {
        return None;
    }
    let mut index = None;
    let mut name = None;
    let mut available = None;
    let mut phys = None;
    let mut len = None;
    let mut dma_owned = None;
    for field in line.split('\t').skip(1) {
        let (key, value) = field.split_once('=')?;
        match key {
            "index" => index = value.parse::<usize>().ok(),
            "name" => name = Some(value.to_string()),
            "available" => available = Some(value == "1"),
            "phys" => phys = Some(value.to_string()),
            "len" => len = value.parse::<usize>().ok(),
            "dma_owned" => dma_owned = Some(value == "1"),
            _ => {}
        }
    }
    Some(PluginProbeRegion {
        index: index?,
        name: name?,
        available: available?,
        phys: phys?,
        len: len?,
        dma_owned: dma_owned?,
    })
}

pub struct PluginHiddenRgb565Framebuffer {
    mem: *mut u8,
    map_len: usize,
    width: usize,
    height: usize,
    stride_pixels: usize,
    region: PluginProbeRegion,
    _device: File,
}

impl PluginHiddenRgb565Framebuffer {
    pub fn open(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
    ) -> Result<Self, PluginProbeError> {
        let metadata = read_plugin_probe_metadata()?;
        Self::open_with_metadata(index, width, height, stride_bytes, &metadata)
    }

    fn open_with_metadata(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
        metadata: &PluginProbeMetadata,
    ) -> Result<Self, PluginProbeError> {
        validate_plugin_metadata(metadata)?;
        let map_len = validate_plugin_geometry(width, height, stride_bytes)
            .map_err(|e| PluginProbeError::InvalidGeometry(e.to_string()))?;
        let region = plugin_hidden_region(metadata, index, map_len)?;
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(PLUGIN_PROBE_DEVICE)?;
        let offset = region
            .index
            .checked_mul(PLUGIN_PROBE_REGION_OFFSET_BYTES)
            .ok_or(PluginProbeError::UnsupportedRegionStride {
                region_offset_bytes: usize::MAX,
            })?;
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                device.as_raw_fd(),
                offset as libc::off_t,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(PluginProbeError::MmapFailed(
                io::Error::last_os_error().to_string(),
            ));
        }
        if mem.is_null() {
            unsafe {
                libc::munmap(mem, map_len);
            }
            return Err(PluginProbeError::MmapReturnedNull);
        }
        Ok(Self {
            mem: mem.cast::<u8>(),
            map_len,
            width,
            height,
            stride_pixels: stride_bytes / std::mem::size_of::<Rgb565Pixel>(),
            region,
            _device: device,
        })
    }

    pub fn copy_full_frame(
        &mut self,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
    ) -> Result<usize, PluginProbeError> {
        if src_stride_pixels < self.width {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width
            )));
        }
        let needed =
            src_stride_pixels
                .checked_mul(self.height)
                .ok_or(PluginProbeError::InvalidGeometry(
                    "source size overflow".to_string(),
                ))?;
        if src.len() < needed {
            return Err(PluginProbeError::SourceTooShort {
                needed,
                actual: src.len(),
            });
        }
        let width = self.width;
        let height = self.height;
        let stride_pixels = self.stride_pixels;
        let dst = self.buffer_mut();
        copy_full_frame_pixels(dst, stride_pixels, src, src_stride_pixels, width, height);
        Ok(stride_pixels * height * std::mem::size_of::<Rgb565Pixel>())
    }

    pub fn copy_rect(
        &mut self,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
        rect: crate::framebuffer::target::DirtyRect,
    ) -> Result<usize, PluginProbeError> {
        if rect.x1 > self.width || rect.y1 > self.height {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "rect x0={} y0={} x1={} y1={} exceeds {}x{}",
                rect.x0, rect.y0, rect.x1, rect.y1, self.width, self.height
            )));
        }
        if src_stride_pixels < self.width {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width
            )));
        }
        let needed =
            src_stride_pixels
                .checked_mul(self.height)
                .ok_or(PluginProbeError::InvalidGeometry(
                    "source size overflow".to_string(),
                ))?;
        if src.len() < needed {
            return Err(PluginProbeError::SourceTooShort {
                needed,
                actual: src.len(),
            });
        }
        if rect.x0 >= rect.x1 || rect.y0 >= rect.y1 {
            return Ok(0);
        }
        let stride_pixels = self.stride_pixels;
        let dst = self.buffer_mut();
        copy_rect_pixels(dst, stride_pixels, src, src_stride_pixels, rect);
        Ok(rect.width() * (rect.y1 - rect.y0) * std::mem::size_of::<Rgb565Pixel>())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn copy_rect_565_strided(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
        src_x: usize,
        src_y: usize,
    ) -> Result<usize, PluginProbeError> {
        if w == 0 || h == 0 {
            return Ok(0);
        }
        let x1 = x
            .checked_add(w)
            .ok_or_else(|| PluginProbeError::InvalidGeometry("target x overflow".to_string()))?;
        let y1 = y
            .checked_add(h)
            .ok_or_else(|| PluginProbeError::InvalidGeometry("target y overflow".to_string()))?;
        if x1 > self.width || y1 > self.height {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "target x={x} y={y} w={w} h={h} exceeds {}x{}",
                self.width, self.height
            )));
        }
        let src_x1 = src_x
            .checked_add(w)
            .ok_or_else(|| PluginProbeError::InvalidGeometry("source x overflow".to_string()))?;
        let src_y1 = src_y
            .checked_add(h)
            .ok_or_else(|| PluginProbeError::InvalidGeometry("source y overflow".to_string()))?;
        if src_stride_pixels < src_x1 {
            return Err(PluginProbeError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than source x+w {src_x1}"
            )));
        }
        let needed =
            src_stride_pixels
                .checked_mul(src_y1)
                .ok_or(PluginProbeError::InvalidGeometry(
                    "source size overflow".to_string(),
                ))?;
        if src.len() < needed {
            return Err(PluginProbeError::SourceTooShort {
                needed,
                actual: src.len(),
            });
        }
        let dst_stride_pixels = self.stride_pixels;
        let dst = self.buffer_mut();
        copy_rect_565_strided_pixels(
            dst,
            dst_stride_pixels,
            x,
            y,
            w,
            h,
            src,
            src_stride_pixels,
            src_x,
            src_y,
        );
        Ok(w * h * std::mem::size_of::<Rgb565Pixel>())
    }

    pub fn region(&self) -> &PluginProbeRegion {
        &self.region
    }

    pub fn physical_addr(&self) -> Result<u32, PluginProbeError> {
        parse_region_phys_u32(&self.region)
    }

    pub fn pixels(&self) -> &[Rgb565Pixel] {
        unsafe {
            std::slice::from_raw_parts(
                self.mem.cast::<Rgb565Pixel>(),
                self.stride_pixels * self.height,
            )
        }
    }

    pub fn stride_pixels(&self) -> usize {
        self.stride_pixels
    }

    fn buffer_mut(&mut self) -> &mut [Rgb565Pixel] {
        unsafe {
            std::slice::from_raw_parts_mut(
                self.mem.cast::<Rgb565Pixel>(),
                self.stride_pixels * self.height,
            )
        }
    }
}

impl Rgb565BlitTarget for PluginHiddenRgb565Framebuffer {
    fn copy_rect_565_strided(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
        src_x: usize,
        src_y: usize,
    ) -> Result<usize, PluginProbeError> {
        PluginHiddenRgb565Framebuffer::copy_rect_565_strided(
            self,
            x,
            y,
            w,
            h,
            src,
            src_stride_pixels,
            src_x,
            src_y,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_strided_copy(
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    src: &[Rgb565Pixel],
    src_stride_pixels: usize,
    src_x: usize,
    src_y: usize,
) -> Result<(), PluginProbeError> {
    if w == 0 || h == 0 {
        return Ok(());
    }
    let x1 = x
        .checked_add(w)
        .ok_or_else(|| PluginProbeError::InvalidGeometry("target x overflow".to_string()))?;
    let y1 = y
        .checked_add(h)
        .ok_or_else(|| PluginProbeError::InvalidGeometry("target y overflow".to_string()))?;
    if x1 > width || y1 > height {
        return Err(PluginProbeError::InvalidGeometry(format!(
            "target x={x} y={y} w={w} h={h} exceeds {width}x{height}"
        )));
    }
    let src_x1 = src_x
        .checked_add(w)
        .ok_or_else(|| PluginProbeError::InvalidGeometry("source x overflow".to_string()))?;
    let src_y1 = src_y
        .checked_add(h)
        .ok_or_else(|| PluginProbeError::InvalidGeometry("source y overflow".to_string()))?;
    if src_stride_pixels < src_x1 {
        return Err(PluginProbeError::InvalidGeometry(format!(
            "source stride {src_stride_pixels} is smaller than source x+w {src_x1}"
        )));
    }
    let needed = src_stride_pixels
        .checked_mul(src_y1)
        .ok_or_else(|| PluginProbeError::InvalidGeometry("source size overflow".to_string()))?;
    if src.len() < needed {
        return Err(PluginProbeError::SourceTooShort {
            needed,
            actual: src.len(),
        });
    }
    Ok(())
}

fn copy_full_frame_pixels(
    dst: &mut [Rgb565Pixel],
    dst_stride_pixels: usize,
    src: &[Rgb565Pixel],
    src_stride_pixels: usize,
    width: usize,
    height: usize,
) {
    if src_stride_pixels == width && dst_stride_pixels == width {
        let len = width * height;
        dst[..len].copy_from_slice(&src[..len]);
        return;
    }
    for y in 0..height {
        let src_start = y * src_stride_pixels;
        let dst_start = y * dst_stride_pixels;
        dst[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
    }
}

fn copy_rect_pixels(
    dst: &mut [Rgb565Pixel],
    dst_stride_pixels: usize,
    src: &[Rgb565Pixel],
    src_stride_pixels: usize,
    rect: crate::framebuffer::target::DirtyRect,
) {
    for y in rect.y0..rect.y1 {
        let src_start = y * src_stride_pixels + rect.x0;
        let dst_start = y * dst_stride_pixels + rect.x0;
        dst[dst_start..dst_start + rect.width()]
            .copy_from_slice(&src[src_start..src_start + rect.width()]);
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_rect_565_strided_pixels(
    dst: &mut [Rgb565Pixel],
    dst_stride_pixels: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    src: &[Rgb565Pixel],
    src_stride_pixels: usize,
    src_x: usize,
    src_y: usize,
) {
    for row in 0..h {
        let src_start = (src_y + row) * src_stride_pixels + src_x;
        let dst_start = (y + row) * dst_stride_pixels + x;
        dst[dst_start..dst_start + w].copy_from_slice(&src[src_start..src_start + w]);
    }
}

pub fn parse_region_phys_u32(region: &PluginProbeRegion) -> Result<u32, PluginProbeError> {
    let raw = region.phys.trim();
    let hex = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    u32::from_str_radix(hex, 16).map_err(|_| PluginProbeError::InvalidPhysicalAddress {
        name: region.name.clone(),
        phys: region.phys.clone(),
    })
}

impl Drop for PluginHiddenRgb565Framebuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.mem.cast::<libc::c_void>(), self.map_len);
        }
    }
}

fn validate_plugin_metadata(metadata: &PluginProbeMetadata) -> Result<(), PluginProbeError> {
    if metadata.header.version < PLUGIN_PROBE_MIN_VERSION {
        return Err(PluginProbeError::UnsupportedVersion {
            version: metadata.header.version,
            min_version: PLUGIN_PROBE_MIN_VERSION,
        });
    }
    if metadata.header.region_offset_bytes != PLUGIN_PROBE_REGION_OFFSET_BYTES {
        return Err(PluginProbeError::UnsupportedRegionStride {
            region_offset_bytes: metadata.header.region_offset_bytes,
        });
    }
    if metadata.header.cache_mode != "writecombine" {
        return Err(PluginProbeError::UnsupportedCacheMode {
            cache_mode: metadata.header.cache_mode.clone(),
        });
    }
    Ok(())
}

fn validate_plugin_geometry(
    width: usize,
    height: usize,
    stride_bytes: usize,
) -> Result<usize, HiddenFramebufferError> {
    if width == 0 || height == 0 {
        return Err(HiddenFramebufferError::InvalidGeometry { width, height });
    }
    let min_stride_bytes = rgb565_stride_bytes(width);
    if stride_bytes < min_stride_bytes {
        return Err(HiddenFramebufferError::InvalidStride {
            stride_bytes,
            min_stride_bytes,
        });
    }
    stride_bytes
        .checked_mul(height)
        .ok_or(HiddenFramebufferError::AddressOverflow)
}

fn plugin_hidden_region(
    metadata: &PluginProbeMetadata,
    index: HiddenRgb565BufferIndex,
    required_len: usize,
) -> Result<PluginProbeRegion, PluginProbeError> {
    let name = format!("hidden-slot-{}", index.get());
    let region = metadata
        .regions
        .iter()
        .find(|region| region.name == name)
        .cloned()
        .ok_or_else(|| PluginProbeError::MissingRegion { name: name.clone() })?;
    if !region.available {
        return Err(PluginProbeError::RegionUnavailable { name });
    }
    if region.dma_owned {
        return Err(PluginProbeError::RegionIsDmaOwned { name });
    }
    if region.len < required_len {
        return Err(PluginProbeError::RegionTooSmall {
            name,
            len: region.len,
            required: required_len,
        });
    }
    Ok(region)
}

#[cfg(test)]
mod tests {
    use crate::framebuffer::target::DirtyRect;

    use super::*;

    fn metadata() -> PluginProbeMetadata {
        parse_plugin_probe_metadata(
            "\
plugin_probe_header_tsv\tname=mister-magik-plugin-probe\tversion=2\tuts_release=5.15.1-MiSTer\topen_count=1\tmmap_count=0\tpage_size=4096\tregion_offset_pages=256\tregion_offset_bytes=1048576\tcache_mode=writecombine\n\
plugin_probe_region_tsv\tindex=0\tname=adjacent-fb-resource\tavailable=1\tphys=0x220fd200\tlen=1036800\tdma_owned=0\n\
plugin_probe_region_tsv\tindex=1\tname=hidden-slot-1\tavailable=1\tphys=0x22800000\tlen=1036800\tdma_owned=0\n\
plugin_probe_region_tsv\tindex=2\tname=hidden-slot-2\tavailable=1\tphys=0x23000000\tlen=1036800\tdma_owned=0\n\
plugin_probe_region_tsv\tindex=3\tname=plugin-owned-dma\tavailable=0\tphys=0x00000000\tlen=1036800\tdma_owned=1\n",
        )
        .unwrap()
    }

    #[test]
    fn atomic_scanout_uapi_layout_matches_kernel_abi_v2() {
        assert_eq!(std::mem::size_of::<ScanoutCaps>(), 24);
        assert_eq!(std::mem::size_of::<ScanoutCapsV2>(), 36);
        assert_eq!(std::mem::size_of::<ScanoutMailboxArm>(), 8);
        assert_eq!(std::mem::size_of::<ScanoutPost>(), 564);
        assert_eq!(std::mem::size_of::<ScanoutStatusRaw>(), 36);
        assert_eq!(SCANOUT_GET_CAPS_V2, 0x8024_4d23);
        assert_eq!(SCANOUT_SYNC_RANGES_AND_POST, 0x4234_4d25);
        assert_eq!(SCANOUT_GET_STATUS, 0x8024_4d26);
    }

    #[test]
    fn atomic_post_rejects_range_overflow_instead_of_truncating_damage() {
        let rect = DirtyRect {
            x0: 0,
            y0: 0,
            x1: 1,
            y1: 1,
        };
        let rects = vec![rect; SCANOUT_MAX_RANGES + 1];
        let mut output = [ScanoutRange::default(); SCANOUT_MAX_RANGES];
        let mut count = 0;

        assert!(append_scanout_ranges(&mut output, &mut count, &rects, 960).is_err());
        assert_eq!(count, 0);
    }

    #[test]
    fn scanout_pixel_target_updates_only_the_requested_strided_rect() {
        let mut destination = vec![Rgb565Pixel(0); 6 * 4];
        let source = (1..=12).map(Rgb565Pixel).collect::<Vec<_>>();
        let mut target = ScanoutPixelsTarget::new(&mut destination, 6, 4, 6);

        let bytes = target
            .copy_rect_565_strided(2, 1, 3, 2, &source, 4, 1, 1)
            .unwrap();

        assert_eq!(bytes, 12);
        assert_eq!(destination[6 + 2..6 + 5], source[5..8]);
        assert_eq!(destination[12 + 2..12 + 5], source[9..12]);
        assert!(destination[..6 + 2].iter().all(|pixel| pixel.0 == 0));
        assert!(destination[12 + 5..].iter().all(|pixel| pixel.0 == 0));
    }

    #[test]
    fn parser_reads_header_and_regions() {
        let metadata = metadata();

        assert_eq!(metadata.header.version, 2);
        assert_eq!(
            metadata.header.region_offset_bytes,
            PLUGIN_PROBE_REGION_OFFSET_BYTES
        );
        assert_eq!(metadata.header.cache_mode, "writecombine");
        assert_eq!(metadata.regions[1].name, "hidden-slot-1");
    }

    #[test]
    fn validation_rejects_old_or_non_wc_contracts() {
        let mut old_metadata = metadata();
        old_metadata.header.version = 1;
        assert!(matches!(
            validate_plugin_metadata(&old_metadata),
            Err(PluginProbeError::UnsupportedVersion { .. })
        ));

        let mut uncached_metadata = metadata();
        uncached_metadata.header.cache_mode = "uncached".to_string();
        assert!(matches!(
            validate_plugin_metadata(&uncached_metadata),
            Err(PluginProbeError::UnsupportedCacheMode { .. })
        ));
    }

    #[test]
    fn hidden_region_selects_only_available_non_dma_slots() {
        let metadata = metadata();
        let region = plugin_hidden_region(
            &metadata,
            HiddenRgb565BufferIndex::new(2).unwrap(),
            PLUGIN_HIDDEN_SLOT_FRAME_BYTES,
        )
        .unwrap();

        assert_eq!(region.index, 2);
        assert_eq!(region.name, "hidden-slot-2");
    }

    #[test]
    fn hidden_region_rejects_short_or_missing_slots() {
        let mut metadata = metadata();
        metadata.regions[1].len = 16;
        assert!(matches!(
            plugin_hidden_region(
                &metadata,
                HiddenRgb565BufferIndex::new(1).unwrap(),
                PLUGIN_HIDDEN_SLOT_FRAME_BYTES
            ),
            Err(PluginProbeError::RegionTooSmall { .. })
        ));

        metadata
            .regions
            .retain(|region| region.name != "hidden-slot-1");
        assert!(matches!(
            plugin_hidden_region(
                &metadata,
                HiddenRgb565BufferIndex::new(1).unwrap(),
                PLUGIN_HIDDEN_SLOT_FRAME_BYTES
            ),
            Err(PluginProbeError::MissingRegion { .. })
        ));
    }

    #[test]
    fn full_frame_copy_uses_contiguous_geometry() {
        let src: Vec<Rgb565Pixel> = (0..12).map(Rgb565Pixel).collect();
        let mut dst = vec![Rgb565Pixel(0); 12];

        copy_full_frame_pixels(&mut dst, 4, &src, 4, 4, 3);

        assert_eq!(dst, src);
    }

    #[test]
    fn full_frame_copy_preserves_padded_destination_rows() {
        let src = vec![
            Rgb565Pixel(1),
            Rgb565Pixel(2),
            Rgb565Pixel(99),
            Rgb565Pixel(3),
            Rgb565Pixel(4),
            Rgb565Pixel(98),
        ];
        let mut dst = vec![Rgb565Pixel(0); 8];

        copy_full_frame_pixels(&mut dst, 4, &src, 3, 2, 2);

        assert_eq!(
            dst,
            vec![
                Rgb565Pixel(1),
                Rgb565Pixel(2),
                Rgb565Pixel(0),
                Rgb565Pixel(0),
                Rgb565Pixel(3),
                Rgb565Pixel(4),
                Rgb565Pixel(0),
                Rgb565Pixel(0),
            ]
        );
    }

    #[test]
    fn rect_copy_updates_only_requested_region() {
        let src: Vec<Rgb565Pixel> = (0..16).map(Rgb565Pixel).collect();
        let mut dst = vec![Rgb565Pixel(99); 16];

        copy_rect_pixels(
            &mut dst,
            4,
            &src,
            4,
            DirtyRect {
                x0: 1,
                y0: 1,
                x1: 3,
                y1: 3,
            },
        );

        assert_eq!(
            dst,
            vec![
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(5),
                Rgb565Pixel(6),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(9),
                Rgb565Pixel(10),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
            ]
        );
    }

    #[test]
    fn strided_rect_copy_updates_destination_offset() {
        let src: Vec<Rgb565Pixel> = (0..30).map(Rgb565Pixel).collect();
        let mut dst = vec![Rgb565Pixel(99); 24];

        copy_rect_565_strided_pixels(&mut dst, 6, 2, 1, 3, 2, &src, 5, 1, 3);

        assert_eq!(
            dst,
            vec![
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(16),
                Rgb565Pixel(17),
                Rgb565Pixel(18),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(21),
                Rgb565Pixel(22),
                Rgb565Pixel(23),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
                Rgb565Pixel(99),
            ]
        );
    }
}
