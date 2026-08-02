// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint-free mappings for the two hidden RGB565 scanout slots.

use crate::framebuffer::format::rgb565_stride_bytes;
use crate::framebuffer::rgb565::Rgb565;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicU8, Ordering};

pub use mister_magik_scanout_contract::{ScanoutSlotLayout, ScanoutSlotsLayout};
pub const SCANOUT_SLOTS_DEVICE: &str = mister_magik_scanout_contract::DEVICE;
pub const SCANOUT_SLOTS_ABI_VERSION: u32 = mister_magik_scanout_contract::ABI_VERSION;
pub const SCANOUT_SLOTS_SLOT_COUNT: usize = mister_magik_scanout_contract::SLOT_COUNT;
pub const SCANOUT_SLOTS_REGION_OFFSET_BYTES: usize =
    mister_magik_scanout_contract::REGION_OFFSET_BYTES;
pub const SCANOUT_SLOT_CAPACITY_BYTES: usize = mister_magik_scanout_contract::SLOT_CAPACITY_BYTES;
pub const SCANOUT_SLOT_MAP_BYTES: usize = mister_magik_scanout_contract::MAP_BYTES;
pub const SCANOUT_SLOTS_LAYOUT_WRITE_COMBINE: u32 =
    mister_magik_scanout_contract::LAYOUT_WRITE_COMBINE;
const SCANOUT_SLOTS_GET_LAYOUT: libc::c_ulong =
    mister_magik_scanout_contract::GET_LAYOUT as libc::c_ulong;
static MAPPED_SLOTS: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HiddenRgb565BufferIndex(u8);

impl HiddenRgb565BufferIndex {
    pub fn new(index: u8) -> Result<Self, HiddenScanoutError> {
        match index {
            1 | 2 => Ok(Self(index)),
            _ => Err(HiddenScanoutError::InvalidBufferIndex { index }),
        }
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HiddenScanoutError {
    Io(String),
    InvalidBufferIndex { index: u8 },
    SlotAlreadyMapped { index: u8 },
    InvalidLayout(String),
    InvalidGeometry(String),
    SourceTooShort { needed: usize, actual: usize },
    MmapFailed(String),
    MmapReturnedNull,
}

impl std::fmt::Display for HiddenScanoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "scanout slots I/O failed: {error}"),
            Self::InvalidBufferIndex { index } => {
                write!(f, "scanout slot index must be 1 or 2, got {index}")
            }
            Self::SlotAlreadyMapped { index } => {
                write!(f, "scanout slot {index} is already mapped in this process")
            }
            Self::InvalidLayout(message) => write!(f, "invalid scanout slots layout: {message}"),
            Self::InvalidGeometry(message) => {
                write!(f, "invalid scanout-slot framebuffer geometry: {message}")
            }
            Self::SourceTooShort { needed, actual } => {
                write!(f, "scanout-slot source has {actual} pixels, need {needed}")
            }
            Self::MmapFailed(error) => write!(f, "scanout slots mmap failed: {error}"),
            Self::MmapReturnedNull => write!(f, "scanout slots mmap returned a null address"),
        }
    }
}

impl std::error::Error for HiddenScanoutError {}

impl From<io::Error> for HiddenScanoutError {
    fn from(value: io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

pub fn read_scanout_slots_layout(file: &File) -> Result<ScanoutSlotsLayout, HiddenScanoutError> {
    let mut layout = ScanoutSlotsLayout::default();
    // SAFETY: layout is a writable fixed-layout C structure for the duration of ioctl.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), SCANOUT_SLOTS_GET_LAYOUT, &mut layout) };
    if result != 0 {
        return Err(io::Error::last_os_error().into());
    }
    validate_scanout_slots_layout(&layout)?;
    Ok(layout)
}

/// One write-combined hidden scanout mapping owned by the kernel module.
pub struct HiddenScanoutFramebuffer {
    mem: *mut u8,
    map_len: usize,
    width: usize,
    height: usize,
    stride_pixels: usize,
    slot: ScanoutSlotLayout,
    _device: File,
    _lease: HiddenSlotLease,
}

struct HiddenSlotLease {
    bit: u8,
}

impl HiddenSlotLease {
    fn acquire(index: HiddenRgb565BufferIndex) -> Result<Self, HiddenScanoutError> {
        let bit = 1 << (index.get() - 1);
        MAPPED_SLOTS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |mapped| {
                (mapped & bit == 0).then_some(mapped | bit)
            })
            .map_err(|_| HiddenScanoutError::SlotAlreadyMapped { index: index.get() })?;
        Ok(Self { bit })
    }
}

impl Drop for HiddenSlotLease {
    fn drop(&mut self) {
        MAPPED_SLOTS.fetch_and(!self.bit, Ordering::Release);
    }
}

impl HiddenScanoutFramebuffer {
    pub fn open(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
    ) -> Result<Self, HiddenScanoutError> {
        let lease = HiddenSlotLease::acquire(index)?;
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(SCANOUT_SLOTS_DEVICE)?;
        let layout = read_scanout_slots_layout(&device)?;
        Self::open_with_layout(index, width, height, stride_bytes, device, &layout, lease)
    }

    fn open_with_layout(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
        device: File,
        layout: &ScanoutSlotsLayout,
        lease: HiddenSlotLease,
    ) -> Result<Self, HiddenScanoutError> {
        validate_scanout_slots_geometry_for_layout(layout, width, height, stride_bytes)?;
        let slot = layout.slots[index.get() as usize - 1];
        let map_len = layout.map_bytes as usize;
        // SAFETY: the validated kernel layout owns this offset and map length.
        let mem = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                device.as_raw_fd(),
                slot.mmap_offset_bytes as libc::off_t,
            )
        };
        if mem == libc::MAP_FAILED {
            return Err(HiddenScanoutError::MmapFailed(
                io::Error::last_os_error().to_string(),
            ));
        }
        if mem.is_null() {
            // SAFETY: the address and length are the values just returned by mmap.
            unsafe {
                libc::munmap(mem, map_len);
            }
            return Err(HiddenScanoutError::MmapReturnedNull);
        }
        Ok(Self {
            mem: mem.cast::<u8>(),
            map_len,
            width,
            height,
            stride_pixels: stride_bytes / std::mem::size_of::<Rgb565>(),
            slot,
            _device: device,
            _lease: lease,
        })
    }

    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    #[must_use]
    pub const fn stride_pixels(&self) -> usize {
        self.stride_pixels
    }

    #[must_use]
    pub const fn slot(&self) -> &ScanoutSlotLayout {
        &self.slot
    }

    #[must_use]
    pub const fn physical_addr(&self) -> u32 {
        self.slot.physical_address
    }

    #[must_use]
    pub fn pixels(&self) -> &[Rgb565] {
        // SAFETY: the mapping was validated for stride*height RGB565 pixels and
        // remains live for the lifetime of self.
        unsafe {
            std::slice::from_raw_parts(self.mem.cast::<Rgb565>(), self.stride_pixels * self.height)
        }
    }

    pub fn pixels_mut(&mut self) -> &mut [Rgb565] {
        // SAFETY: self exclusively owns this mutable mapping and its validated length.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.mem.cast::<Rgb565>(),
                self.stride_pixels * self.height,
            )
        }
    }

    /// Publishes prior CPU writes before a latch post exposes this slot to the FPGA.
    pub fn publish_writes(&mut self) {
        use std::sync::atomic::{Ordering, compiler_fence};

        compiler_fence(Ordering::Release);
        #[cfg(target_arch = "arm")]
        // SAFETY: drains earlier stores through the ARM system domain before MMIO.
        unsafe {
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }
        compiler_fence(Ordering::SeqCst);
    }
}

impl Drop for HiddenScanoutFramebuffer {
    fn drop(&mut self) {
        // SAFETY: mem/map_len came from a successful mmap and are unmapped once here.
        unsafe {
            libc::munmap(self.mem.cast::<libc::c_void>(), self.map_len);
        }
    }
}

pub fn validate_scanout_slots_layout(
    layout: &ScanoutSlotsLayout,
) -> Result<(), HiddenScanoutError> {
    let expected = mister_magik_scanout_contract::EXPECTED_LAYOUT;
    if *layout != expected {
        return Err(HiddenScanoutError::InvalidLayout(format!(
            "expected {expected:?}, got {layout:?}"
        )));
    }
    Ok(())
}

pub(crate) fn validate_scanout_slots_geometry(
    width: usize,
    height: usize,
    stride_bytes: usize,
) -> Result<usize, HiddenScanoutError> {
    if width == 0 || height == 0 {
        return Err(HiddenScanoutError::InvalidGeometry(format!(
            "invalid dimensions {width}x{height}"
        )));
    }
    let min_stride_bytes = rgb565_stride_bytes(width);
    if stride_bytes < min_stride_bytes
        || !stride_bytes.is_multiple_of(std::mem::size_of::<Rgb565>())
    {
        return Err(HiddenScanoutError::InvalidGeometry(format!(
            "stride {stride_bytes} is not a whole-pixel stride of at least {min_stride_bytes}"
        )));
    }
    stride_bytes
        .checked_mul(height)
        .ok_or_else(|| HiddenScanoutError::InvalidGeometry("frame size overflow".to_string()))
}

pub(crate) fn validate_scanout_slots_geometry_for_layout(
    layout: &ScanoutSlotsLayout,
    width: usize,
    height: usize,
    stride_bytes: usize,
) -> Result<usize, HiddenScanoutError> {
    let frame_len = validate_scanout_slots_geometry(width, height, stride_bytes)?;
    if width > layout.max_width as usize
        || height > layout.max_height as usize
        || stride_bytes > layout.max_stride_bytes as usize
        || frame_len > layout.slot_capacity_bytes as usize
    {
        return Err(HiddenScanoutError::InvalidGeometry(format!(
            "requested frame {width}x{height} stride={stride_bytes} bytes={frame_len} exceeds ABI maximum {}x{} stride={} capacity={}",
            layout.max_width,
            layout.max_height,
            layout.max_stride_bytes,
            layout.slot_capacity_bytes
        )));
    }
    Ok(frame_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> ScanoutSlotsLayout {
        ScanoutSlotsLayout {
            abi_version: SCANOUT_SLOTS_ABI_VERSION,
            slot_count: 2,
            max_width: 1366,
            max_height: 768,
            max_stride_bytes: 2736,
            slot_capacity_bytes: 2_101_248,
            map_bytes: 2_101_248,
            flags: SCANOUT_SLOTS_LAYOUT_WRITE_COMBINE,
            slots: [
                ScanoutSlotLayout {
                    physical_address: 0x227e_9000,
                    mmap_offset_bytes: 0,
                },
                ScanoutSlotLayout {
                    physical_address: 0x22fd_2000,
                    mmap_offset_bytes: 8_294_400,
                },
            ],
            reserved: [0; 4],
        }
    }

    #[test]
    fn exact_kernel_contract_and_indices_are_enforced() {
        validate_scanout_slots_layout(&layout()).unwrap();
        assert_eq!(HiddenRgb565BufferIndex::new(1).unwrap().get(), 1);
        assert_eq!(HiddenRgb565BufferIndex::new(2).unwrap().get(), 2);
        assert!(HiddenRgb565BufferIndex::new(0).is_err());
        assert!(HiddenRgb565BufferIndex::new(3).is_err());
    }

    #[test]
    fn capacity_accepts_qualified_geometry_and_rejects_oversize() {
        assert_eq!(
            validate_scanout_slots_geometry_for_layout(&layout(), 960, 540, 1920),
            Ok(1_036_800)
        );
        assert!(validate_scanout_slots_geometry_for_layout(&layout(), 1367, 768, 2752).is_err());
    }

    #[test]
    fn one_process_cannot_map_the_same_slot_twice() {
        let index = HiddenRgb565BufferIndex::new(1).unwrap();
        let first = HiddenSlotLease::acquire(index).unwrap();
        assert!(matches!(
            HiddenSlotLease::acquire(index),
            Err(HiddenScanoutError::SlotAlreadyMapped { index: 1 })
        ));
        drop(first);
        assert!(HiddenSlotLease::acquire(index).is_ok());
    }
}
