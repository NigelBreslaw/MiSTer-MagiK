// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production mappings exposed by the stock-kernel scanout-slots module.
//!
//! The scanout-slot module maps framebuffer-owned physical ranges with write-combined
//! attributes. This module validates the reported regions before the launcher
//! uses them as Main-flippable hidden RGB565 buffers.

use crate::framebuffer::hidden_scanout::HiddenScanoutFramebuffer;
pub use crate::framebuffer::hidden_scanout::{
    HiddenRgb565BufferIndex, HiddenScanoutError as ScanoutSlotsError, SCANOUT_SLOT_CAPACITY_BYTES,
    SCANOUT_SLOT_MAP_BYTES, SCANOUT_SLOTS_ABI_VERSION, SCANOUT_SLOTS_DEVICE,
    SCANOUT_SLOTS_LAYOUT_WRITE_COMBINE, SCANOUT_SLOTS_REGION_OFFSET_BYTES,
    SCANOUT_SLOTS_SLOT_COUNT, ScanoutSlotLayout, ScanoutSlotsLayout, read_scanout_slots_layout,
    validate_scanout_slots_layout,
};
#[cfg(test)]
use crate::framebuffer::hidden_scanout::{
    validate_scanout_slots_geometry, validate_scanout_slots_geometry_for_layout,
};
use crate::framebuffer::target::DirtyRect;
use crate::framebuffer::vertical_scale::{
    Rgb565FrameView, VerticalRect, VerticalRgb565Transform, VerticalSampling,
};
use slint::platform::software_renderer::Rgb565Pixel;

pub struct ScanoutSlotsRgb565Framebuffer {
    inner: HiddenScanoutFramebuffer,
}

impl ScanoutSlotsRgb565Framebuffer {
    pub fn open(
        index: HiddenRgb565BufferIndex,
        width: usize,
        height: usize,
        stride_bytes: usize,
    ) -> Result<Self, ScanoutSlotsError> {
        Ok(Self {
            inner: HiddenScanoutFramebuffer::open(index, width, height, stride_bytes)?,
        })
    }

    pub fn copy_full_frame(
        &mut self,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
    ) -> Result<usize, ScanoutSlotsError> {
        if src_stride_pixels < self.width() {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width()
            )));
        }
        let needed = src_stride_pixels.checked_mul(self.height()).ok_or(
            ScanoutSlotsError::InvalidGeometry("source size overflow".to_string()),
        )?;
        if src.len() < needed {
            return Err(ScanoutSlotsError::SourceTooShort {
                needed,
                actual: src.len(),
            });
        }
        let width = self.width();
        let height = self.height();
        let stride_pixels = self.stride_pixels();
        let dst = self.buffer_mut();
        copy_full_frame_pixels(dst, stride_pixels, src, src_stride_pixels, width, height);
        Ok(width * height * std::mem::size_of::<Rgb565Pixel>())
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.inner.width()
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.inner.height()
    }

    pub fn copy_rect(
        &mut self,
        src: &[Rgb565Pixel],
        src_stride_pixels: usize,
        rect: crate::framebuffer::target::DirtyRect,
    ) -> Result<usize, ScanoutSlotsError> {
        if rect.x1 > self.width() || rect.y1 > self.height() {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "rect x0={} y0={} x1={} y1={} exceeds {}x{}",
                rect.x0,
                rect.y0,
                rect.x1,
                rect.y1,
                self.width(),
                self.height()
            )));
        }
        if src_stride_pixels < self.width() {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "source stride {src_stride_pixels} is smaller than width {}",
                self.width()
            )));
        }
        let needed = src_stride_pixels.checked_mul(self.height()).ok_or(
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
        let stride_pixels = self.stride_pixels();
        let dst = self.buffer_mut();
        copy_rect_pixels(dst, stride_pixels, src, src_stride_pixels, rect);
        Ok(rect.width() * (rect.y1 - rect.y0) * std::mem::size_of::<Rgb565Pixel>())
    }

    pub fn copy_vertical_rect(
        &mut self,
        source: Rgb565FrameView<'_, Rgb565Pixel>,
        rect: DirtyRect,
    ) -> Result<usize, ScanoutSlotsError> {
        self.copy_vertical_rect_with_sampling(source, rect, VerticalSampling::CenteredNearest)
    }

    pub fn copy_vertical_rect_with_sampling(
        &mut self,
        source: Rgb565FrameView<'_, Rgb565Pixel>,
        rect: DirtyRect,
        sampling: VerticalSampling,
    ) -> Result<usize, ScanoutSlotsError> {
        let transform = VerticalRgb565Transform::new_with_sampling(
            self.width(),
            source.height,
            self.height(),
            sampling,
        )
        .map_err(|error| ScanoutSlotsError::InvalidGeometry(error.to_string()))?;
        let stride_pixels = self.stride_pixels();
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
        self.inner.publish_writes();
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
        if x1 > self.width() || y1 > self.height() {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "target x={x} y={y} w={w} h={h} exceeds {}x{}",
                self.width(),
                self.height()
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
        let dst_stride_pixels = self.stride_pixels();
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

    pub fn shift_rect(
        &mut self,
        rect: DirtyRect,
        delta_x: isize,
        delta_y: isize,
    ) -> Result<usize, ScanoutSlotsError> {
        if rect.x1 > self.width() || rect.y1 > self.height() {
            return Err(ScanoutSlotsError::InvalidGeometry(format!(
                "shift rect x0={} y0={} x1={} y1={} exceeds {}x{}",
                rect.x0,
                rect.y0,
                rect.x1,
                rect.y1,
                self.width(),
                self.height()
            )));
        }
        let moved_width = rect.width().saturating_sub(delta_x.unsigned_abs());
        let moved_height = (rect.rows() as usize).saturating_sub(delta_y.unsigned_abs());
        if moved_width == 0 || moved_height == 0 {
            return Ok(0);
        }
        let stride = self.stride_pixels();
        shift_rect_pixels(self.buffer_mut(), stride, rect, delta_x, delta_y);
        Ok(moved_width
            .saturating_mul(moved_height)
            .saturating_mul(std::mem::size_of::<Rgb565Pixel>()))
    }

    pub fn slot(&self) -> &ScanoutSlotLayout {
        self.inner.slot()
    }

    pub fn physical_addr(&self) -> Result<u32, ScanoutSlotsError> {
        Ok(self.inner.physical_addr())
    }

    pub fn pixels(&self) -> &[Rgb565Pixel] {
        let pixels = self.inner.pixels();
        debug_assert_eq!(
            std::mem::size_of::<Rgb565Pixel>(),
            std::mem::size_of::<crate::framebuffer::rgb565::Rgb565>()
        );
        debug_assert_eq!(
            std::mem::align_of::<Rgb565Pixel>(),
            std::mem::align_of::<crate::framebuffer::rgb565::Rgb565>()
        );
        // SAFETY: both pixel types are transparent RGB565 u16 words with the
        // same size/alignment; the returned slice cannot outlive the mapping.
        unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<Rgb565Pixel>(), pixels.len()) }
    }

    pub fn pixels_mut(&mut self) -> &mut [Rgb565Pixel] {
        self.buffer_mut()
    }

    pub fn stride_pixels(&self) -> usize {
        self.inner.stride_pixels()
    }

    fn buffer_mut(&mut self) -> &mut [Rgb565Pixel] {
        let pixels = self.inner.pixels_mut();
        debug_assert_eq!(
            std::mem::size_of::<Rgb565Pixel>(),
            std::mem::size_of::<crate::framebuffer::rgb565::Rgb565>()
        );
        debug_assert_eq!(
            std::mem::align_of::<Rgb565Pixel>(),
            std::mem::align_of::<crate::framebuffer::rgb565::Rgb565>()
        );
        // SAFETY: both pixel types are transparent RGB565 u16 words with the
        // same size/alignment, and self exclusively owns the mutable mapping.
        unsafe {
            std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<Rgb565Pixel>(), pixels.len())
        }
    }
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

fn shift_rect_pixels(
    pixels: &mut [Rgb565Pixel],
    stride: usize,
    rect: DirtyRect,
    delta_x: isize,
    delta_y: isize,
) {
    let shift_x = delta_x.unsigned_abs();
    let shift_y = delta_y.unsigned_abs();
    let copy_width = rect.width().saturating_sub(shift_x);
    let copy_height = (rect.rows() as usize).saturating_sub(shift_y);
    if copy_width == 0 || copy_height == 0 {
        return;
    }
    let (source_x, destination_x) = if delta_x >= 0 {
        (rect.x0, rect.x0 + shift_x)
    } else {
        (rect.x0 + shift_x, rect.x0)
    };
    let mut copy_row = |source_y: usize| {
        let destination_y = if delta_y >= 0 {
            source_y + shift_y
        } else {
            source_y - shift_y
        };
        let source_start = source_y * stride + source_x;
        let destination_start = destination_y * stride + destination_x;
        pixels.copy_within(source_start..source_start + copy_width, destination_start);
    };
    if delta_y > 0 {
        for source_y in (rect.y0..rect.y0 + copy_height).rev() {
            copy_row(source_y);
        }
    } else {
        for source_y in rect.y0 + shift_y..rect.y0 + shift_y + copy_height {
            copy_row(source_y);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::framebuffer::target::DirtyRect;

    use super::*;

    #[test]
    fn physical_rect_shift_handles_both_axes_and_overlap_direction() {
        let mut pixels = (0..30).map(Rgb565Pixel).collect::<Vec<_>>();
        let original = pixels.clone();
        let rect = DirtyRect {
            x0: 1,
            y0: 1,
            x1: 5,
            y1: 4,
        };

        shift_rect_pixels(&mut pixels, 6, rect, 1, 1);
        for y in 2..4 {
            for x in 2..5 {
                assert_eq!(pixels[y * 6 + x], original[(y - 1) * 6 + x - 1]);
            }
        }

        let shifted = pixels.clone();
        shift_rect_pixels(&mut pixels, 6, rect, -1, -1);
        for y in 1..3 {
            for x in 1..4 {
                assert_eq!(pixels[y * 6 + x], shifted[(y + 1) * 6 + x + 1]);
            }
        }
    }

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
