// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production mappings exposed by the stock-kernel scanout-slots module.
//!
//! The scanout-slot module maps framebuffer-owned physical ranges with write-combined
//! attributes. This module validates the reported regions before the launcher
//! uses them as Main-flippable hidden RGB565 buffers.

use crate::framebuffer::format::rgb565_stride_bytes;
use crate::framebuffer::target::DirtyRect;
use crate::framebuffer::vertical_scale::{Rgb565FrameView, VerticalRect, VerticalRgb565Transform};
use slint::platform::software_renderer::Rgb565Pixel;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HiddenRgb565BufferIndex(u8);

impl HiddenRgb565BufferIndex {
    pub fn new(index: u8) -> Result<Self, ScanoutSlotsError> {
        match index {
            1 | 2 => Ok(Self(index)),
            _ => Err(ScanoutSlotsError::InvalidBufferIndex { index }),
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanoutSlotsError {
    Io(String),
    InvalidBufferIndex { index: u8 },
    InvalidLayout(String),
    InvalidGeometry(String),
    SourceTooShort { needed: usize, actual: usize },
    MmapFailed(String),
    MmapReturnedNull,
}

impl std::fmt::Display for ScanoutSlotsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "scanout slots I/O failed: {e}"),
            Self::InvalidBufferIndex { index } => {
                write!(f, "scanout slot index must be 1 or 2, got {index}")
            }
            Self::InvalidLayout(message) => write!(f, "invalid scanout slots layout: {message}"),
            Self::InvalidGeometry(e) => write!(f, "invalid scanout-slot framebuffer geometry: {e}"),
            Self::SourceTooShort { needed, actual } => {
                write!(f, "scanout-slot source has {actual} pixels, need {needed}")
            }
            Self::MmapFailed(e) => write!(f, "scanout slots mmap failed: {e}"),
            Self::MmapReturnedNull => write!(f, "scanout slots mmap returned a null address"),
        }
    }
}

impl std::error::Error for ScanoutSlotsError {}

impl From<io::Error> for ScanoutSlotsError {
    fn from(value: io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

pub fn read_scanout_slots_layout(file: &File) -> Result<ScanoutSlotsLayout, ScanoutSlotsError> {
    let mut layout = ScanoutSlotsLayout::default();
    // SAFETY: layout is a writable fixed-layout C structure for the duration of ioctl.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), SCANOUT_SLOTS_GET_LAYOUT, &mut layout) };
    if result != 0 {
        return Err(io::Error::last_os_error().into());
    }
    validate_scanout_slots_layout(&layout)?;
    Ok(layout)
}

pub struct ScanoutSlotsRgb565Framebuffer {
    mem: *mut u8,
    map_len: usize,
    width: usize,
    height: usize,
    stride_pixels: usize,
    slot: ScanoutSlotLayout,
    _device: File,
}

impl ScanoutSlotsRgb565Framebuffer {
    pub fn open(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
    ) -> Result<Self, ScanoutSlotsError> {
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(SCANOUT_SLOTS_DEVICE)?;
        let layout = read_scanout_slots_layout(&device)?;
        Self::open_with_layout(index, width, height, stride_bytes, device, &layout)
    }

    fn open_with_layout(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
        device: File,
        layout: &ScanoutSlotsLayout,
    ) -> Result<Self, ScanoutSlotsError> {
        validate_scanout_slots_geometry_for_layout(layout, width, height, stride_bytes)?;
        let slot = layout.slots[index.get() as usize - 1];
        let map_len = layout.map_bytes as usize;
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
            return Err(ScanoutSlotsError::MmapFailed(
                io::Error::last_os_error().to_string(),
            ));
        }
        if mem.is_null() {
            unsafe {
                libc::munmap(mem, map_len);
            }
            return Err(ScanoutSlotsError::MmapReturnedNull);
        }
        Ok(Self {
            mem: mem.cast::<u8>(),
            map_len,
            width,
            height,
            stride_pixels: stride_bytes / std::mem::size_of::<Rgb565Pixel>(),
            slot,
            _device: device,
        })
    }

    pub fn copy_full_frame(
        &mut self,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
    ) -> Result<usize, ScanoutSlotsError> {
        if src_stride_pixels < self.width {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width
            )));
        }
        let needed = src_stride_pixels.checked_mul(self.height).ok_or(
            ScanoutSlotsError::InvalidGeometry("source size overflow".to_string()),
        )?;
        if src.len() < needed {
            return Err(ScanoutSlotsError::SourceTooShort {
                needed,
                actual: src.len(),
            });
        }
        let width = self.width;
        let height = self.height;
        let stride_pixels = self.stride_pixels;
        let dst = self.buffer_mut();
        copy_full_frame_pixels(dst, stride_pixels, src, src_stride_pixels, width, height);
        Ok(width * height * std::mem::size_of::<Rgb565Pixel>())
    }

    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    pub fn copy_rect(
        &mut self,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
        rect: crate::framebuffer::target::DirtyRect,
    ) -> Result<usize, ScanoutSlotsError> {
        if rect.x1 > self.width || rect.y1 > self.height {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "rect x0={} y0={} x1={} y1={} exceeds {}x{}",
                rect.x0, rect.y0, rect.x1, rect.y1, self.width, self.height
            )));
        }
        if src_stride_pixels < self.width {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width
            )));
        }
        let needed = src_stride_pixels.checked_mul(self.height).ok_or(
            ScanoutSlotsError::InvalidGeometry("source size overflow".to_string()),
        )?;
        if src.len() < needed {
            return Err(ScanoutSlotsError::SourceTooShort {
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

    pub fn copy_vertical_rect(
        &mut self,
        source: Rgb565FrameView<'_, Rgb565Pixel>,
        rect: DirtyRect,
    ) -> Result<usize, ScanoutSlotsError> {
        let transform = VerticalRgb565Transform::new(self.width, source.height, self.height)
            .map_err(|error| ScanoutSlotsError::InvalidGeometry(error.to_string()))?;
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
                self.buffer_mut(),
                stride_pixels,
            )
            .map_err(|error| ScanoutSlotsError::InvalidGeometry(error.to_string()))
            .map(|stats| stats.map_or(0, |stats| stats.bytes))
    }

    /// Publishes every prior CPU write to the write-combined scanout mapping
    /// before the FPGA is told that this slot is ready to read.
    pub fn publish_writes(&mut self) {
        publish_scanout_writes();
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
    ) -> Result<usize, ScanoutSlotsError> {
        if w == 0 || h == 0 {
            return Ok(0);
        }
        let x1 = x
            .checked_add(w)
            .ok_or_else(|| ScanoutSlotsError::InvalidGeometry("target x overflow".to_string()))?;
        let y1 = y
            .checked_add(h)
            .ok_or_else(|| ScanoutSlotsError::InvalidGeometry("target y overflow".to_string()))?;
        if x1 > self.width || y1 > self.height {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "target x={x} y={y} w={w} h={h} exceeds {}x{}",
                self.width, self.height
            )));
        }
        let src_x1 = src_x
            .checked_add(w)
            .ok_or_else(|| ScanoutSlotsError::InvalidGeometry("source x overflow".to_string()))?;
        let src_y1 = src_y
            .checked_add(h)
            .ok_or_else(|| ScanoutSlotsError::InvalidGeometry("source y overflow".to_string()))?;
        if src_stride_pixels < src_x1 {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than source x+w {src_x1}"
            )));
        }
        let needed =
            src_stride_pixels
                .checked_mul(src_y1)
                .ok_or(ScanoutSlotsError::InvalidGeometry(
                    "source size overflow".to_string(),
                ))?;
        if src.len() < needed {
            return Err(ScanoutSlotsError::SourceTooShort {
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

    pub fn slot(&self) -> &ScanoutSlotLayout {
        &self.slot
    }

    pub fn physical_addr(&self) -> Result<u32, ScanoutSlotsError> {
        Ok(self.slot.physical_address)
    }

    pub fn pixels(&self) -> &[Rgb565Pixel] {
        unsafe {
            std::slice::from_raw_parts(
                self.mem.cast::<Rgb565Pixel>(),
                self.stride_pixels * self.height,
            )
        }
    }

    pub fn pixels_mut(&mut self) -> &mut [Rgb565Pixel] {
        self.buffer_mut()
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

#[inline]
fn publish_scanout_writes() {
    use std::sync::atomic::{Ordering, compiler_fence};

    compiler_fence(Ordering::Release);
    #[cfg(target_arch = "arm")]
    // SAFETY: this instruction has no memory operands of its own. It drains
    // prior stores through the ARM system domain before MMIO publishes the
    // framebuffer address to the FPGA.
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    compiler_fence(Ordering::SeqCst);
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

impl Drop for ScanoutSlotsRgb565Framebuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.mem.cast::<libc::c_void>(), self.map_len);
        }
    }
}

pub fn validate_scanout_slots_layout(layout: &ScanoutSlotsLayout) -> Result<(), ScanoutSlotsError> {
    let expected = mister_magik_scanout_contract::EXPECTED_LAYOUT;
    if *layout != expected {
        return Err(ScanoutSlotsError::InvalidLayout(format!(
            "expected {expected:?}, got {layout:?}"
        )));
    }
    Ok(())
}

fn validate_scanout_slots_geometry(
    width: usize,
    height: usize,
    stride_bytes: usize,
) -> Result<usize, ScanoutSlotsError> {
    if width == 0 || height == 0 {
        return Err(ScanoutSlotsError::InvalidGeometry(format!(
            "invalid dimensions {width}x{height}"
        )));
    }
    let min_stride_bytes = rgb565_stride_bytes(width);
    if stride_bytes < min_stride_bytes {
        return Err(ScanoutSlotsError::InvalidGeometry(format!(
            "stride {stride_bytes} is smaller than {min_stride_bytes}"
        )));
    }
    stride_bytes
        .checked_mul(height)
        .ok_or_else(|| ScanoutSlotsError::InvalidGeometry("frame size overflow".to_string()))
}

fn validate_scanout_slots_geometry_for_layout(
    layout: &ScanoutSlotsLayout,
    width: usize,
    height: usize,
    stride_bytes: usize,
) -> Result<usize, ScanoutSlotsError> {
    let frame_len = validate_scanout_slots_geometry(width, height, stride_bytes)?;
    if width > layout.max_width as usize
        || height > layout.max_height as usize
        || stride_bytes > layout.max_stride_bytes as usize
        || frame_len > layout.slot_capacity_bytes as usize
    {
        return Err(ScanoutSlotsError::InvalidGeometry(format!(
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
    use crate::framebuffer::target::DirtyRect;

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
    fn layout_matches_exact_kernel_contract() {
        let layout = layout();
        assert_eq!(std::mem::size_of::<ScanoutSlotsLayout>(), 64);
        assert_eq!(layout.slots[0].physical_address, 0x227e_9000);
        assert_eq!(layout.slots[1].physical_address, 0x22fd_2000);
        validate_scanout_slots_layout(&layout).unwrap();
    }

    #[test]
    fn validation_rejects_every_contract_mismatch() {
        let mut wrong = layout();
        wrong.abi_version = 0;
        assert!(matches!(
            validate_scanout_slots_layout(&wrong),
            Err(ScanoutSlotsError::InvalidLayout(_))
        ));

        let mut wrong = layout();
        wrong.slots[1].physical_address = 0x2300_0000;
        assert!(matches!(
            validate_scanout_slots_layout(&wrong),
            Err(ScanoutSlotsError::InvalidLayout(_))
        ));

        let mut wrong = layout();
        wrong.reserved[0] = 1;
        assert!(matches!(
            validate_scanout_slots_layout(&wrong),
            Err(ScanoutSlotsError::InvalidLayout(_))
        ));
    }

    #[test]
    fn buffer_index_accepts_only_two_slots() {
        assert_eq!(HiddenRgb565BufferIndex::new(1).unwrap().get(), 1);
        assert_eq!(HiddenRgb565BufferIndex::new(2).unwrap().get(), 2);
        assert!(HiddenRgb565BufferIndex::new(0).is_err());
        assert!(HiddenRgb565BufferIndex::new(3).is_err());
    }

    #[test]
    fn capacity_accepts_qualified_540p_720p_and_max_geometry() {
        assert_eq!(
            validate_scanout_slots_geometry(960, 540, 1920),
            Ok(1_036_800)
        );
        assert_eq!(
            validate_scanout_slots_geometry(1280, 720, 2560),
            Ok(1_843_200)
        );
        assert_eq!(
            validate_scanout_slots_geometry_for_layout(&layout(), 1366, 768, 2736),
            Ok(2_101_248)
        );
    }

    #[test]
    fn layout_capacity_rejects_old_abi_and_every_oversized_dimension() {
        let mut old = layout();
        old.abi_version = 2;
        assert!(matches!(
            validate_scanout_slots_layout(&old),
            Err(ScanoutSlotsError::InvalidLayout(_))
        ));
        for (width, height, stride) in [(1367, 768, 2752), (1366, 769, 2736), (1366, 768, 2752)] {
            assert!(matches!(
                validate_scanout_slots_geometry_for_layout(&layout(), width, height, stride),
                Err(ScanoutSlotsError::InvalidGeometry(_))
            ));
        }
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
