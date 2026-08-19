// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable RGB565 framebuffer-scene contracts.

use std::error::Error;
use std::fmt;
use std::mem::{MaybeUninit, align_of, size_of};
use std::sync::OnceLock;
use std::time::Duration;

pub mod navigation;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rgb565Pixel(pub u16);

/// Rotation applied while logical scene pixels are written into the physical
/// output buffer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputRotation {
    #[default]
    None,
    Clockwise90,
    CounterClockwise90,
}

impl OutputRotation {
    #[must_use]
    pub const fn transposes_axes(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb565Rect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Rgb565Rect {
    #[must_use]
    pub const fn width(self) -> usize {
        self.x1.saturating_sub(self.x0)
    }

    #[must_use]
    pub const fn height(self) -> usize {
        self.y1.saturating_sub(self.y0)
    }
}

/// Defines how an upright logical RGB565 scene is laid out in a physical
/// framebuffer. The logical dimensions remain the coordinate system used by
/// UI and scene code; the physical dimensions and stride describe memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb565OutputLayout {
    logical_width: usize,
    logical_height: usize,
    physical_width: usize,
    physical_height: usize,
    physical_stride: usize,
    len: usize,
    rotation: OutputRotation,
}

impl Rgb565OutputLayout {
    pub fn new(
        logical_width: usize,
        logical_height: usize,
        physical_stride: usize,
        rotation: OutputRotation,
    ) -> Result<Self, SceneError> {
        if logical_width == 0 || logical_height == 0 {
            return Err(SceneError::InvalidGeometry(
                "RGB565 output dimensions must be nonzero",
            ));
        }
        let (physical_width, physical_height) = if rotation.transposes_axes() {
            (logical_height, logical_width)
        } else {
            (logical_width, logical_height)
        };
        if physical_stride < physical_width {
            return Err(SceneError::InvalidGeometry(
                "RGB565 output stride must cover its physical width",
            ));
        }
        let len =
            physical_stride
                .checked_mul(physical_height)
                .ok_or(SceneError::InvalidGeometry(
                    "RGB565 output buffer length overflows",
                ))?;
        Ok(Self {
            logical_width,
            logical_height,
            physical_width,
            physical_height,
            physical_stride,
            len,
            rotation,
        })
    }

    pub fn identity(geometry: SceneGeometry) -> Self {
        Self::new(
            geometry.width(),
            geometry.height(),
            geometry.stride_pixels(),
            OutputRotation::None,
        )
        .expect("valid scene geometry is a valid identity output layout")
    }

    #[must_use]
    pub const fn logical_width(self) -> usize {
        self.logical_width
    }

    #[must_use]
    pub const fn logical_height(self) -> usize {
        self.logical_height
    }

    #[must_use]
    pub const fn physical_width(self) -> usize {
        self.physical_width
    }

    #[must_use]
    pub const fn physical_height(self) -> usize {
        self.physical_height
    }

    #[must_use]
    pub const fn physical_stride(self) -> usize {
        self.physical_stride
    }

    #[must_use]
    pub const fn len(self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }

    #[must_use]
    pub const fn rotation(self) -> OutputRotation {
        self.rotation
    }

    #[must_use]
    pub const fn logical_to_physical(self, x: usize, y: usize) -> (usize, usize) {
        debug_assert!(x < self.logical_width && y < self.logical_height);
        match self.rotation {
            OutputRotation::None => (x, y),
            OutputRotation::Clockwise90 => (self.logical_height - 1 - y, x),
            OutputRotation::CounterClockwise90 => (y, self.logical_width - 1 - x),
        }
    }

    #[must_use]
    pub const fn physical_to_logical(self, x: usize, y: usize) -> (usize, usize) {
        debug_assert!(x < self.physical_width && y < self.physical_height);
        match self.rotation {
            OutputRotation::None => (x, y),
            OutputRotation::Clockwise90 => (y, self.logical_height - 1 - x),
            OutputRotation::CounterClockwise90 => (self.logical_width - 1 - y, x),
        }
    }

    #[must_use]
    pub const fn logical_delta_to_physical(self, dx: isize, dy: isize) -> (isize, isize) {
        match self.rotation {
            OutputRotation::None => (dx, dy),
            OutputRotation::Clockwise90 => (-dy, dx),
            OutputRotation::CounterClockwise90 => (dy, -dx),
        }
    }

    #[must_use]
    pub fn logical_rect_to_physical(self, rect: Rgb565Rect) -> Rgb565Rect {
        let rect = Rgb565Rect {
            x0: rect.x0.min(self.logical_width),
            y0: rect.y0.min(self.logical_height),
            x1: rect.x1.min(self.logical_width),
            y1: rect.y1.min(self.logical_height),
        };
        match self.rotation {
            OutputRotation::None => rect,
            OutputRotation::Clockwise90 => Rgb565Rect {
                x0: self.logical_height - rect.y1,
                y0: rect.x0,
                x1: self.logical_height - rect.y0,
                y1: rect.x1,
            },
            OutputRotation::CounterClockwise90 => Rgb565Rect {
                x0: rect.y0,
                y0: self.logical_width - rect.x1,
                x1: rect.y1,
                y1: self.logical_width - rect.x0,
            },
        }
    }

    #[must_use]
    pub const fn physical_offset(self, logical_x: usize, logical_y: usize) -> usize {
        let (physical_x, physical_y) = self.logical_to_physical(logical_x, logical_y);
        physical_y * self.physical_stride + physical_x
    }
}

/// A logical sub-rectangle stored as one dense physical RGB565 rectangle.
///
/// Local coordinates preserve the parent output's rotation, so the existing
/// tiled and NEON copy kernels can write a layer without retaining pixels for
/// the rest of the physical framebuffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb565RegionLayout {
    output: Rgb565OutputLayout,
    logical_rect: Rgb565Rect,
    physical_rect: Rgb565Rect,
    dense_output: Rgb565OutputLayout,
}

impl Rgb565RegionLayout {
    pub fn new(output: Rgb565OutputLayout, logical_rect: Rgb565Rect) -> Result<Self, SceneError> {
        if logical_rect.x0 >= logical_rect.x1
            || logical_rect.y0 >= logical_rect.y1
            || logical_rect.x1 > output.logical_width()
            || logical_rect.y1 > output.logical_height()
        {
            return Err(SceneError::InvalidGeometry(
                "RGB565 region must be a nonempty logical output sub-rectangle",
            ));
        }
        let physical_rect = output.logical_rect_to_physical(logical_rect);
        let dense_output = Rgb565OutputLayout::new(
            logical_rect.width(),
            logical_rect.height(),
            physical_rect.width(),
            output.rotation(),
        )?;
        debug_assert_eq!(dense_output.physical_width(), physical_rect.width());
        debug_assert_eq!(dense_output.physical_height(), physical_rect.height());
        Ok(Self {
            output,
            logical_rect,
            physical_rect,
            dense_output,
        })
    }

    #[must_use]
    pub const fn output(self) -> Rgb565OutputLayout {
        self.output
    }

    #[must_use]
    pub const fn logical_rect(self) -> Rgb565Rect {
        self.logical_rect
    }

    #[must_use]
    pub const fn physical_rect(self) -> Rgb565Rect {
        self.physical_rect
    }

    #[must_use]
    pub const fn physical_stride(self) -> usize {
        self.dense_output.physical_stride()
    }

    #[must_use]
    pub const fn len(self) -> usize {
        self.dense_output.len()
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub const fn allocated_bytes(self) -> usize {
        self.len().saturating_mul(size_of::<Rgb565Pixel>())
    }

    #[must_use]
    pub const fn dense_output(self) -> Rgb565OutputLayout {
        self.dense_output
    }

    #[must_use]
    pub const fn contains_logical_rect(self, rect: Rgb565Rect) -> bool {
        rect.x0 >= self.logical_rect.x0
            && rect.y0 >= self.logical_rect.y0
            && rect.x1 <= self.logical_rect.x1
            && rect.y1 <= self.logical_rect.y1
    }

    #[must_use]
    pub const fn local_logical(self, x: usize, y: usize) -> Option<(usize, usize)> {
        if x < self.logical_rect.x0
            || y < self.logical_rect.y0
            || x >= self.logical_rect.x1
            || y >= self.logical_rect.y1
        {
            return None;
        }
        Some((x - self.logical_rect.x0, y - self.logical_rect.y0))
    }

    #[must_use]
    pub fn dense_offset(self, logical_x: usize, logical_y: usize) -> Option<usize> {
        let (local_x, local_y) = self.local_logical(logical_x, logical_y)?;
        Some(self.dense_output.physical_offset(local_x, local_y))
    }
}

/// Mutable logical access to a dense physical RGB565 region.
pub struct Rgb565RegionSurfaceMut<'a, P> {
    pixels: &'a mut [P],
    layout: Rgb565RegionLayout,
}

impl<'a, P> Rgb565RegionSurfaceMut<'a, P> {
    pub fn new(pixels: &'a mut [P], layout: Rgb565RegionLayout) -> Result<Self, SceneError> {
        if pixels.len() != layout.len() {
            return Err(SceneError::TargetSizeMismatch {
                actual: pixels.len(),
                expected: layout.len(),
            });
        }
        Ok(Self { pixels, layout })
    }

    #[must_use]
    pub const fn layout(&self) -> Rgb565RegionLayout {
        self.layout
    }
}

impl<P: Copy> Rgb565RegionSurfaceMut<'_, P> {
    #[allow(clippy::too_many_arguments)]
    pub fn copy_rect_strided(
        &mut self,
        destination_x: usize,
        destination_y: usize,
        width: usize,
        height: usize,
        source: &[P],
        source_stride: usize,
        source_x: usize,
        source_y: usize,
    ) -> bool {
        if width == 0 || height == 0 {
            return true;
        }
        let Some(destination_x1) = destination_x.checked_add(width) else {
            return false;
        };
        let Some(destination_y1) = destination_y.checked_add(height) else {
            return false;
        };
        let destination = Rgb565Rect {
            x0: destination_x,
            y0: destination_y,
            x1: destination_x1,
            y1: destination_y1,
        };
        if !self.layout.contains_logical_rect(destination) {
            return false;
        }
        let local_x = destination_x - self.layout.logical_rect.x0;
        let local_y = destination_y - self.layout.logical_rect.y0;
        Rgb565SurfaceMut::new(self.pixels, self.layout.dense_output)
            .expect("region target length was validated")
            .copy_rect_strided(
                local_x,
                local_y,
                width,
                height,
                source,
                source_stride,
                source_x,
                source_y,
            )
    }
}

/// Mutable pixel access in logical coordinates backed by a physically
/// oriented buffer.
pub struct Rgb565SurfaceMut<'a, P> {
    pixels: &'a mut [P],
    layout: Rgb565OutputLayout,
}

impl<'a, P> Rgb565SurfaceMut<'a, P> {
    pub fn new(pixels: &'a mut [P], layout: Rgb565OutputLayout) -> Result<Self, SceneError> {
        if pixels.len() != layout.len() {
            return Err(SceneError::TargetSizeMismatch {
                actual: pixels.len(),
                expected: layout.len(),
            });
        }
        Ok(Self { pixels, layout })
    }

    #[must_use]
    pub const fn layout(&self) -> Rgb565OutputLayout {
        self.layout
    }

    #[must_use]
    pub fn physical_pixels(&self) -> &[P] {
        self.pixels
    }

    #[must_use]
    pub fn physical_pixels_mut(&mut self) -> &mut [P] {
        self.pixels
    }

    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> Option<&P> {
        (x < self.layout.logical_width() && y < self.layout.logical_height())
            .then(|| &self.pixels[self.layout.physical_offset(x, y)])
    }

    pub fn set(&mut self, x: usize, y: usize, pixel: P) -> bool {
        if x >= self.layout.logical_width() || y >= self.layout.logical_height() {
            return false;
        }
        let offset = self.layout.physical_offset(x, y);
        self.pixels[offset] = pixel;
        true
    }
}

impl<P: Copy> Rgb565SurfaceMut<'_, P> {
    pub fn fill(&mut self, pixel: P) {
        self.pixels.fill(pixel);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn copy_rect_strided(
        &mut self,
        destination_x: usize,
        destination_y: usize,
        width: usize,
        height: usize,
        source: &[P],
        source_stride: usize,
        source_x: usize,
        source_y: usize,
    ) -> bool {
        if width == 0 || height == 0 {
            return true;
        }
        if destination_x.saturating_add(width) > self.layout.logical_width()
            || destination_y.saturating_add(height) > self.layout.logical_height()
            || source_stride == 0
            || source_y
                .saturating_add(height - 1)
                .saturating_mul(source_stride)
                .saturating_add(source_x)
                .saturating_add(width)
                > source.len()
        {
            return false;
        }
        let source_copy = slices_overlap(self.pixels, source).then(|| {
            let mut copy = Vec::with_capacity(width * height);
            for row in 0..height {
                let source_start = (source_y + row) * source_stride + source_x;
                copy.extend_from_slice(&source[source_start..source_start + width]);
            }
            copy
        });
        let (source, source_stride, source_x, source_y) = source_copy
            .as_deref()
            .map_or((source, source_stride, source_x, source_y), |source| {
                (source, width, 0, 0)
            });

        if self.layout.rotation() == OutputRotation::None {
            for row in 0..height {
                let source_start = (source_y + row) * source_stride + source_x;
                let destination_start =
                    (destination_y + row) * self.layout.physical_stride() + destination_x;
                self.pixels[destination_start..destination_start + width]
                    .copy_from_slice(&source[source_start..source_start + width]);
            }
            return true;
        }

        copy_rotated_rgb565_tiled(
            self.pixels,
            self.layout,
            destination_x,
            destination_y,
            width,
            height,
            source,
            source_stride,
            source_x,
            source_y,
        );
        true
    }
}

fn slices_overlap<P>(left: &[P], right: &[P]) -> bool {
    let element_size = size_of::<P>();
    if element_size == 0 || left.is_empty() || right.is_empty() {
        return false;
    }
    let left_start = left.as_ptr() as usize;
    let right_start = right.as_ptr() as usize;
    let left_end = left_start.saturating_add(left.len().saturating_mul(element_size));
    let right_end = right_start.saturating_add(right.len().saturating_mul(element_size));
    left_start < right_end && right_start < left_end
}

fn rgb565_neon_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("MISTER_RGB565_NEON").ok().as_deref(),
            Some("0" | "off" | "false" | "no" | "scalar")
        )
    })
}

#[derive(Clone, Copy)]
struct RotatedCopySpec {
    destination_x: usize,
    destination_y: usize,
    width: usize,
    height: usize,
    source_stride: usize,
    source_x: usize,
    source_y: usize,
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn copy_rotated_rgb565_neon<P: Copy>(
    destination: &mut [P],
    layout: Rgb565OutputLayout,
    source: &[P],
    spec: RotatedCopySpec,
) -> bool {
    let RotatedCopySpec {
        destination_x,
        destination_y,
        width,
        height,
        source_stride,
        source_x,
        source_y,
    } = spec;
    if !rgb565_neon_enabled()
        || size_of::<P>() != size_of::<u16>()
        || align_of::<P>() < align_of::<u16>()
        || (destination.as_ptr() as usize) & 1 != 0
        || (source.as_ptr() as usize) & 1 != 0
    {
        return false;
    }
    unsafe extern "C" {
        fn mister_magik_rgb565_rotate_clockwise(
            destination: *mut u16,
            destination_stride: usize,
            logical_width: usize,
            logical_height: usize,
            destination_x: usize,
            destination_y: usize,
            width: usize,
            height: usize,
            source: *const u16,
            source_stride: usize,
            source_x: usize,
            source_y: usize,
        );
        fn mister_magik_rgb565_rotate_counter_clockwise(
            destination: *mut u16,
            destination_stride: usize,
            logical_width: usize,
            logical_height: usize,
            destination_x: usize,
            destination_y: usize,
            width: usize,
            height: usize,
            source: *const u16,
            source_stride: usize,
            source_x: usize,
            source_y: usize,
        );
    }
    // SAFETY: the caller validated all source and destination rectangles, and
    // overlapping slices were staged before entering this kernel.
    unsafe {
        match layout.rotation() {
            OutputRotation::Clockwise90 => mister_magik_rgb565_rotate_clockwise(
                destination.as_mut_ptr().cast(),
                layout.physical_stride(),
                layout.logical_width(),
                layout.logical_height(),
                destination_x,
                destination_y,
                width,
                height,
                source.as_ptr().cast(),
                source_stride,
                source_x,
                source_y,
            ),
            OutputRotation::CounterClockwise90 => mister_magik_rgb565_rotate_counter_clockwise(
                destination.as_mut_ptr().cast(),
                layout.physical_stride(),
                layout.logical_width(),
                layout.logical_height(),
                destination_x,
                destination_y,
                width,
                height,
                source.as_ptr().cast(),
                source_stride,
                source_x,
                source_y,
            ),
            OutputRotation::None => return false,
        }
    }
    true
}

#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
fn copy_rotated_rgb565_neon<P: Copy>(
    _destination: &mut [P],
    _layout: Rgb565OutputLayout,
    _source: &[P],
    spec: RotatedCopySpec,
) -> bool {
    let _ = (
        spec.destination_x,
        spec.destination_y,
        spec.width,
        spec.height,
        spec.source_stride,
        spec.source_x,
        spec.source_y,
    );
    false
}

#[allow(clippy::too_many_arguments)]
fn copy_rotated_rgb565_tiled<P: Copy>(
    destination: &mut [P],
    layout: Rgb565OutputLayout,
    destination_x: usize,
    destination_y: usize,
    width: usize,
    height: usize,
    source: &[P],
    source_stride: usize,
    source_x: usize,
    source_y: usize,
) {
    if copy_rotated_rgb565_neon(
        destination,
        layout,
        source,
        RotatedCopySpec {
            destination_x,
            destination_y,
            width,
            height,
            source_stride,
            source_x,
            source_y,
        },
    ) {
        return;
    }
    copy_rotated_rgb565_scalar(
        destination,
        layout,
        destination_x,
        destination_y,
        width,
        height,
        source,
        source_stride,
        source_x,
        source_y,
    );
}

#[allow(clippy::too_many_arguments)]
fn copy_rotated_rgb565_scalar<P: Copy>(
    destination: &mut [P],
    layout: Rgb565OutputLayout,
    destination_x: usize,
    destination_y: usize,
    width: usize,
    height: usize,
    source: &[P],
    source_stride: usize,
    source_x: usize,
    source_y: usize,
) {
    const TILE: usize = 16;
    let mut tile: [MaybeUninit<P>; TILE * TILE] = [const { MaybeUninit::uninit() }; TILE * TILE];

    for tile_y in (0..height).step_by(TILE) {
        let tile_height = (height - tile_y).min(TILE);
        for tile_x in (0..width).step_by(TILE) {
            let tile_width = (width - tile_x).min(TILE);
            for row in 0..tile_height {
                let source_start = (source_y + tile_y + row) * source_stride + source_x + tile_x;
                for column in 0..tile_width {
                    tile[row * TILE + column].write(source[source_start + column]);
                }
            }

            match layout.rotation() {
                OutputRotation::Clockwise90 => {
                    let physical_x_min =
                        layout.logical_height() - (destination_y + tile_y + tile_height);
                    for column in 0..tile_width {
                        let physical_y = destination_x + tile_x + column;
                        let destination_start =
                            physical_y * layout.physical_stride() + physical_x_min;
                        for row in 0..tile_height {
                            let destination_offset = destination_start + tile_height - 1 - row;
                            destination[destination_offset] =
                                unsafe { tile[row * TILE + column].assume_init_read() };
                        }
                    }
                }
                OutputRotation::CounterClockwise90 => {
                    for column in 0..tile_width {
                        let physical_y =
                            layout.logical_width() - 1 - (destination_x + tile_x + column);
                        let destination_start =
                            physical_y * layout.physical_stride() + destination_y + tile_y;
                        for row in 0..tile_height {
                            destination[destination_start + row] =
                                unsafe { tile[row * TILE + column].assume_init_read() };
                        }
                    }
                }
                OutputRotation::None => unreachable!("identity copies bypass rotation kernels"),
            }
        }
    }
}

#[must_use]
pub fn blend_rgb565_neon_if_available<P: Copy>(
    destination: &mut [P],
    previous: &[P],
    current: &[P],
    start: usize,
    end: usize,
    alpha_bucket: u16,
) -> bool {
    if !rgb565_neon_enabled()
        || size_of::<P>() != size_of::<u16>()
        || align_of::<P>() < align_of::<u16>()
        || start >= end
        || end > destination.len()
        || end > previous.len()
        || end > current.len()
        || (destination.as_ptr() as usize) & 1 != 0
        || (previous.as_ptr() as usize) & 1 != 0
        || (current.as_ptr() as usize) & 1 != 0
    {
        return false;
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        unsafe extern "C" {
            fn mister_magik_rgb565_blend(
                destination: *mut u16,
                previous: *const u16,
                current: *const u16,
                start: usize,
                end: usize,
                alpha: u16,
            );
        }
        // SAFETY: the caller validated all slice bounds and alignment.
        unsafe {
            mister_magik_rgb565_blend(
                destination.as_mut_ptr().cast(),
                previous.as_ptr().cast(),
                current.as_ptr().cast(),
                start,
                end,
                alpha_bucket,
            );
        }
        true
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        let _ = (destination, previous, current, start, end, alpha_bucket);
        false
    }
}

#[must_use]
pub fn blend_rgb565_black_neon_if_available<P: Copy>(
    destination: &mut [P],
    pixels: &[P],
    start: usize,
    end: usize,
    alpha_bucket: u16,
    fade_in: bool,
) -> bool {
    if !rgb565_neon_enabled()
        || size_of::<P>() != size_of::<u16>()
        || align_of::<P>() < align_of::<u16>()
        || start >= end
        || end > destination.len()
        || end > pixels.len()
        || (destination.as_ptr() as usize) & 1 != 0
        || (pixels.as_ptr() as usize) & 1 != 0
    {
        return false;
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        unsafe extern "C" {
            fn mister_magik_rgb565_blend_black(
                destination: *mut u16,
                pixels: *const u16,
                start: usize,
                end: usize,
                alpha: u16,
                fade_in: i32,
            );
        }
        // SAFETY: the caller validated all slice bounds and alignment.
        unsafe {
            mister_magik_rgb565_blend_black(
                destination.as_mut_ptr().cast(),
                pixels.as_ptr().cast(),
                start,
                end,
                alpha_bucket,
                i32::from(fade_in),
            );
        }
        true
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        let _ = (destination, pixels, start, end, alpha_bucket, fade_in);
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneGeometry {
    width: usize,
    height: usize,
    stride_pixels: usize,
    len: usize,
}

impl SceneGeometry {
    pub fn new(width: usize, height: usize, stride_pixels: usize) -> Result<Self, SceneError> {
        if width == 0 || height == 0 {
            return Err(SceneError::InvalidGeometry(
                "scene dimensions must be nonzero",
            ));
        }
        if stride_pixels < width {
            return Err(SceneError::InvalidGeometry(
                "scene stride must be at least its width",
            ));
        }
        let len = stride_pixels
            .checked_mul(height)
            .ok_or(SceneError::InvalidGeometry("scene buffer length overflows"))?;
        Ok(Self {
            width,
            height,
            stride_pixels,
            len,
        })
    }

    #[must_use]
    pub const fn width(self) -> usize {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> usize {
        self.height
    }

    #[must_use]
    pub const fn stride_pixels(self) -> usize {
        self.stride_pixels
    }

    #[must_use]
    pub const fn len(self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SceneBufferId(u8);

impl SceneBufferId {
    pub fn new(value: u8, reusable_buffers: u8) -> Result<Self, SceneError> {
        if reusable_buffers == 0 {
            return Err(SceneError::InvalidBufferCount);
        }
        if value >= reusable_buffers {
            return Err(SceneError::InvalidBufferId {
                value,
                reusable_buffers,
            });
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Debug)]
pub struct SceneTarget<'a> {
    pixels: &'a mut [Rgb565Pixel],
    geometry: SceneGeometry,
    output_layout: Rgb565OutputLayout,
    buffer_id: SceneBufferId,
}

impl<'a> SceneTarget<'a> {
    pub fn new(
        pixels: &'a mut [Rgb565Pixel],
        geometry: SceneGeometry,
        buffer_id: SceneBufferId,
    ) -> Result<Self, SceneError> {
        Self::new_oriented(
            pixels,
            geometry,
            Rgb565OutputLayout::identity(geometry),
            buffer_id,
        )
    }

    pub fn new_oriented(
        pixels: &'a mut [Rgb565Pixel],
        geometry: SceneGeometry,
        output_layout: Rgb565OutputLayout,
        buffer_id: SceneBufferId,
    ) -> Result<Self, SceneError> {
        if geometry.width() != output_layout.logical_width()
            || geometry.height() != output_layout.logical_height()
        {
            return Err(SceneError::InvalidGeometry(
                "scene logical geometry does not match its output layout",
            ));
        }
        if pixels.len() != output_layout.len() {
            return Err(SceneError::TargetSizeMismatch {
                actual: pixels.len(),
                expected: output_layout.len(),
            });
        }
        Ok(Self {
            pixels,
            geometry,
            output_layout,
            buffer_id,
        })
    }

    #[must_use]
    pub const fn geometry(&self) -> SceneGeometry {
        self.geometry
    }

    #[must_use]
    pub const fn output_layout(&self) -> Rgb565OutputLayout {
        self.output_layout
    }

    #[must_use]
    pub const fn buffer_id(&self) -> SceneBufferId {
        self.buffer_id
    }

    #[must_use]
    pub fn pixels(&self) -> &[Rgb565Pixel] {
        self.pixels
    }

    #[must_use]
    pub fn pixels_mut(&mut self) -> &mut [Rgb565Pixel] {
        self.pixels
    }

    #[must_use]
    pub fn surface_mut(&mut self) -> Rgb565SurfaceMut<'_, Rgb565Pixel> {
        Rgb565SurfaceMut::new(self.pixels, self.output_layout)
            .expect("scene target validates its output layout at construction")
    }

    #[must_use]
    pub fn into_pixels(self) -> &'a mut [Rgb565Pixel] {
        self.pixels
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SceneClock {
    pub frame: u64,
    pub elapsed: Duration,
    pub next_elapsed: Option<Duration>,
}

pub trait FramebufferScene {
    type Stats;

    fn geometry(&self) -> SceneGeometry;

    fn render(
        &mut self,
        target: SceneTarget<'_>,
        clock: SceneClock,
    ) -> Result<Self::Stats, SceneError>;

    fn invalidate_buffer(&mut self, buffer: SceneBufferId);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SceneError {
    InvalidGeometry(&'static str),
    InvalidBufferCount,
    InvalidBufferId { value: u8, reusable_buffers: u8 },
    TargetSizeMismatch { actual: usize, expected: usize },
    Render(String),
}

impl fmt::Display for SceneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGeometry(message) => formatter.write_str(message),
            Self::InvalidBufferCount => {
                formatter.write_str("scene reusable-buffer count must be nonzero")
            }
            Self::InvalidBufferId {
                value,
                reusable_buffers,
            } => write!(
                formatter,
                "scene buffer ID {value} is outside 0..{reusable_buffers}"
            ),
            Self::TargetSizeMismatch { actual, expected } => write!(
                formatter,
                "scene target has {actual} pixels, expected exactly {expected}"
            ),
            Self::Render(message) => formatter.write_str(message),
        }
    }
}

impl Error for SceneError {}

impl From<String> for SceneError {
    fn from(message: String) -> Self {
        Self::Render(message)
    }
}

impl From<&str> for SceneError {
    fn from(message: &str) -> Self {
        Self::Render(message.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_rejects_invalid_or_overflowing_lengths() {
        assert!(SceneGeometry::new(0, 1, 1).is_err());
        assert!(SceneGeometry::new(1, 0, 1).is_err());
        assert!(SceneGeometry::new(2, 1, 1).is_err());
        assert!(SceneGeometry::new(1, 2, usize::MAX).is_err());
        assert_eq!(SceneGeometry::new(2, 3, 4).unwrap().len(), 12);
    }

    #[test]
    fn targets_require_exact_geometry_and_bounded_buffer_identity() {
        let geometry = SceneGeometry::new(2, 2, 3).unwrap();
        let id = SceneBufferId::new(1, 2).unwrap();
        assert!(SceneBufferId::new(2, 2).is_err());
        assert!(SceneBufferId::new(0, 0).is_err());
        assert!(SceneTarget::new(&mut [Rgb565Pixel(0); 5], geometry, id).is_err());
        let mut pixels = [Rgb565Pixel(0); 6];
        let target = SceneTarget::new(&mut pixels, geometry, id).unwrap();
        assert_eq!(target.geometry(), geometry);
        assert_eq!(
            target.output_layout(),
            Rgb565OutputLayout::identity(geometry)
        );
        assert_eq!(target.buffer_id(), id);
    }

    #[test]
    fn oriented_scene_target_separates_logical_and_physical_geometry() {
        let geometry = SceneGeometry::new(2, 3, 2).unwrap();
        let output = Rgb565OutputLayout::new(2, 3, 4, OutputRotation::Clockwise90).unwrap();
        let id = SceneBufferId::new(0, 2).unwrap();
        let mut pixels = [Rgb565Pixel(0); 8];
        let mut target = SceneTarget::new_oriented(&mut pixels, geometry, output, id).unwrap();
        assert_eq!(target.geometry(), geometry);
        assert_eq!(target.output_layout(), output);
        assert!(target.surface_mut().set(0, 0, Rgb565Pixel(7)));
        assert_eq!(pixels[2], Rgb565Pixel(7));
    }

    #[test]
    fn output_layout_maps_both_quarter_turns() {
        let clockwise = Rgb565OutputLayout::new(2, 3, 3, OutputRotation::Clockwise90).unwrap();
        let counterclockwise =
            Rgb565OutputLayout::new(2, 3, 3, OutputRotation::CounterClockwise90).unwrap();

        assert_eq!(
            (clockwise.physical_width(), clockwise.physical_height()),
            (3, 2)
        );
        assert_eq!(clockwise.logical_to_physical(0, 0), (2, 0));
        assert_eq!(clockwise.logical_to_physical(1, 2), (0, 1));
        assert_eq!(clockwise.physical_to_logical(2, 0), (0, 0));
        assert_eq!(clockwise.logical_delta_to_physical(0, 4), (-4, 0));

        assert_eq!(counterclockwise.logical_to_physical(0, 0), (0, 1));
        assert_eq!(counterclockwise.logical_to_physical(1, 2), (2, 0));
        assert_eq!(counterclockwise.physical_to_logical(2, 0), (1, 2));
        assert_eq!(counterclockwise.logical_delta_to_physical(0, 4), (4, 0));
    }

    #[test]
    fn output_layout_maps_rectangles_without_losing_coverage() {
        let rect = Rgb565Rect {
            x0: 1,
            y0: 1,
            x1: 3,
            y1: 4,
        };
        let clockwise = Rgb565OutputLayout::new(4, 6, 6, OutputRotation::Clockwise90).unwrap();
        let counterclockwise =
            Rgb565OutputLayout::new(4, 6, 6, OutputRotation::CounterClockwise90).unwrap();
        assert_eq!(
            clockwise.logical_rect_to_physical(rect),
            Rgb565Rect {
                x0: 2,
                y0: 1,
                x1: 5,
                y1: 3,
            }
        );
        assert_eq!(
            counterclockwise.logical_rect_to_physical(rect),
            Rgb565Rect {
                x0: 1,
                y0: 1,
                x1: 4,
                y1: 3,
            }
        );
    }

    #[test]
    fn dense_region_layout_matches_parent_physical_coordinates() {
        let logical_rect = Rgb565Rect {
            x0: 2,
            y0: 1,
            x1: 7,
            y1: 5,
        };
        for rotation in [
            OutputRotation::None,
            OutputRotation::Clockwise90,
            OutputRotation::CounterClockwise90,
        ] {
            let output = Rgb565OutputLayout::new(9, 6, 11, rotation).unwrap();
            let region = Rgb565RegionLayout::new(output, logical_rect).unwrap();
            assert_eq!(region.output(), output);
            assert_eq!(region.logical_rect(), logical_rect);
            assert_eq!(
                region.physical_rect(),
                output.logical_rect_to_physical(logical_rect)
            );
            assert_eq!(
                region.len(),
                region.physical_rect().width() * region.physical_rect().height()
            );
            assert_eq!(region.allocated_bytes(), region.len() * 2);

            for y in logical_rect.y0..logical_rect.y1 {
                for x in logical_rect.x0..logical_rect.x1 {
                    let (physical_x, physical_y) = output.logical_to_physical(x, y);
                    let physical_rect = region.physical_rect();
                    let expected = (physical_y - physical_rect.y0) * region.physical_stride()
                        + physical_x
                        - physical_rect.x0;
                    assert_eq!(region.dense_offset(x, y), Some(expected));
                }
            }
        }
    }

    #[test]
    fn dense_region_surface_is_pixel_identical_to_parent_output() {
        let logical_rect = Rgb565Rect {
            x0: 2,
            y0: 1,
            x1: 7,
            y1: 5,
        };
        let source: Vec<_> = (0..20).map(|value| Rgb565Pixel(value + 1)).collect();
        for rotation in [
            OutputRotation::None,
            OutputRotation::Clockwise90,
            OutputRotation::CounterClockwise90,
        ] {
            let output = Rgb565OutputLayout::new(9, 6, 11, rotation).unwrap();
            let region = Rgb565RegionLayout::new(output, logical_rect).unwrap();
            let mut full = vec![Rgb565Pixel(0); output.len()];
            assert!(
                Rgb565SurfaceMut::new(&mut full, output)
                    .unwrap()
                    .copy_rect_strided(2, 1, 5, 4, &source, 5, 0, 0)
            );
            let mut dense = vec![Rgb565Pixel(0); region.len()];
            assert!(
                Rgb565RegionSurfaceMut::new(&mut dense, region)
                    .unwrap()
                    .copy_rect_strided(2, 1, 5, 4, &source, 5, 0, 0)
            );
            let physical = region.physical_rect();
            let mut cropped = Vec::with_capacity(region.len());
            for y in physical.y0..physical.y1 {
                let start = y * output.physical_stride() + physical.x0;
                cropped.extend_from_slice(&full[start..start + physical.width()]);
            }
            assert_eq!(dense, cropped, "rotation={rotation:?}");
        }
    }

    #[test]
    fn dense_region_rejects_out_of_bounds_geometry_and_writes() {
        let output = Rgb565OutputLayout::new(9, 6, 11, OutputRotation::Clockwise90).unwrap();
        assert!(
            Rgb565RegionLayout::new(
                output,
                Rgb565Rect {
                    x0: 2,
                    y0: 1,
                    x1: 2,
                    y1: 5,
                }
            )
            .is_err()
        );
        let region = Rgb565RegionLayout::new(
            output,
            Rgb565Rect {
                x0: 2,
                y0: 1,
                x1: 7,
                y1: 5,
            },
        )
        .unwrap();
        let mut dense = vec![Rgb565Pixel(0); region.len()];
        let source = [Rgb565Pixel(1); 4];
        let mut surface = Rgb565RegionSurfaceMut::new(&mut dense, region).unwrap();
        assert!(!surface.copy_rect_strided(1, 1, 2, 2, &source, 2, 0, 0));
        assert!(!surface.copy_rect_strided(6, 4, 2, 2, &source, 2, 0, 0));
    }

    #[test]
    fn oriented_surface_copies_directly_into_physical_order() {
        let source = [1_u16, 2, 3, 4, 5, 6];
        for (rotation, expected) in [
            (OutputRotation::Clockwise90, [5, 3, 1, 6, 4, 2]),
            (OutputRotation::CounterClockwise90, [2, 4, 6, 1, 3, 5]),
        ] {
            let layout = Rgb565OutputLayout::new(2, 3, 3, rotation).unwrap();
            let mut physical = [0_u16; 6];
            let mut surface = Rgb565SurfaceMut::new(&mut physical, layout).unwrap();
            assert!(surface.copy_rect_strided(0, 0, 2, 3, &source, 2, 0, 0));
            assert_eq!(physical, expected);
        }
    }

    fn reference_copy(
        destination: &mut [Rgb565Pixel],
        layout: Rgb565OutputLayout,
        source: &[Rgb565Pixel],
        spec: RotatedCopySpec,
    ) -> bool {
        let RotatedCopySpec {
            destination_x,
            destination_y,
            width,
            height,
            source_stride,
            source_x,
            source_y,
        } = spec;
        if width == 0 || height == 0 {
            return true;
        }
        if destination_x.saturating_add(width) > layout.logical_width()
            || destination_y.saturating_add(height) > layout.logical_height()
            || source_stride == 0
            || source_y
                .saturating_add(height - 1)
                .saturating_mul(source_stride)
                .saturating_add(source_x)
                .saturating_add(width)
                > source.len()
        {
            return false;
        }
        for row in 0..height {
            let source_start = (source_y + row) * source_stride + source_x;
            for column in 0..width {
                let (physical_x, physical_y) =
                    layout.logical_to_physical(destination_x + column, destination_y + row);
                destination[physical_y * layout.physical_stride() + physical_x] =
                    source[source_start + column];
            }
        }
        true
    }

    #[test]
    fn tiled_rotations_match_reference_for_strided_odd_rectangles() {
        let logical_width = 7;
        let logical_height = 5;
        let source_stride = 11;
        let source = (0..source_stride * 9)
            .map(|index| Rgb565Pixel((index as u16).wrapping_mul(37)))
            .collect::<Vec<_>>();
        let rectangles = [(0, 0, 7, 5), (1, 0, 5, 3), (0, 2, 7, 3), (2, 1, 3, 4)];

        for rotation in [
            OutputRotation::Clockwise90,
            OutputRotation::CounterClockwise90,
        ] {
            let layout =
                Rgb565OutputLayout::new(logical_width, logical_height, 11, rotation).unwrap();
            for &(x, y, width, height) in &rectangles {
                let mut optimized = vec![Rgb565Pixel(0xdead); layout.len()];
                let mut expected = optimized.clone();
                assert!(reference_copy(
                    &mut expected,
                    layout,
                    &source,
                    RotatedCopySpec {
                        destination_x: x,
                        destination_y: y,
                        width,
                        height,
                        source_stride,
                        source_x: 2,
                        source_y: 1,
                    },
                ));
                let mut surface = Rgb565SurfaceMut::new(&mut optimized, layout).unwrap();
                assert!(surface.copy_rect_strided(
                    x,
                    y,
                    width,
                    height,
                    &source,
                    source_stride,
                    2,
                    1,
                ));
                assert_eq!(
                    optimized, expected,
                    "rotation={rotation:?} rect={x},{y},{width},{height}"
                );
            }
        }
    }

    #[test]
    fn rotated_copy_keeps_invalid_bounds_and_clipped_valid_rectangles_stable() {
        let layout = Rgb565OutputLayout::new(5, 3, 7, OutputRotation::Clockwise90).unwrap();
        let source = (0..32)
            .map(|index| Rgb565Pixel(index as u16))
            .collect::<Vec<_>>();
        let mut optimized = vec![Rgb565Pixel(0xbeef); layout.len()];
        let mut expected = optimized.clone();
        {
            let mut surface = Rgb565SurfaceMut::new(&mut optimized, layout).unwrap();
            assert!(surface.copy_rect_strided(3, 1, 2, 2, &source, 8, 4, 2));
        }
        assert!(reference_copy(
            &mut expected,
            layout,
            &source,
            RotatedCopySpec {
                destination_x: 3,
                destination_y: 1,
                width: 2,
                height: 2,
                source_stride: 8,
                source_x: 4,
                source_y: 2,
            },
        ));
        assert_eq!(optimized, expected);

        let before = optimized.clone();
        {
            let mut surface = Rgb565SurfaceMut::new(&mut optimized, layout).unwrap();
            assert!(!surface.copy_rect_strided(4, 2, 2, 2, &source, 8, 0, 0));
        }
        assert_eq!(optimized, before);
    }

    #[test]
    fn rotated_copy_stages_overlapping_ring_segments() {
        let layout = Rgb565OutputLayout::new(5, 4, 7, OutputRotation::CounterClockwise90).unwrap();
        let mut storage = (0..layout.len())
            .map(|index| Rgb565Pixel((index as u16).wrapping_mul(53)))
            .collect::<Vec<_>>();
        let original = storage.clone();
        let mut expected = original.clone();
        assert!(reference_copy(
            &mut expected,
            layout,
            &original,
            RotatedCopySpec {
                destination_x: 1,
                destination_y: 1,
                width: 3,
                height: 2,
                source_stride: 7,
                source_x: 0,
                source_y: 0,
            },
        ));

        let source = unsafe { std::slice::from_raw_parts(storage.as_ptr(), storage.len()) };
        let mut surface = Rgb565SurfaceMut::new(&mut storage, layout).unwrap();
        assert!(surface.copy_rect_strided(1, 1, 3, 2, source, 7, 0, 0));
        assert_eq!(storage, expected);
    }

    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    #[test]
    fn neon_rotation_matches_scalar_for_partial_tiles() {
        let layout = Rgb565OutputLayout::new(13, 9, 16, OutputRotation::Clockwise90).unwrap();
        let source = (0..19 * 13)
            .map(|index| Rgb565Pixel((index as u16).wrapping_mul(29)))
            .collect::<Vec<_>>();
        let mut scalar = vec![Rgb565Pixel(0xaaaa); layout.len()];
        let mut neon = scalar.clone();
        copy_rotated_rgb565_scalar(&mut scalar, layout, 2, 1, 9, 7, &source, 19, 3, 4);
        assert!(copy_rotated_rgb565_neon(
            &mut neon,
            layout,
            &source,
            RotatedCopySpec {
                destination_x: 2,
                destination_y: 1,
                width: 9,
                height: 7,
                source_stride: 19,
                source_x: 3,
                source_y: 4,
            },
        ));
        assert_eq!(neon, scalar);
    }

    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    #[test]
    fn neon_blend_matches_scalar_for_offsets_tails_and_alpha_buckets() {
        fn scalar(from: Rgb565Pixel, to: Rgb565Pixel, alpha: u16) -> Rgb565Pixel {
            let from = u32::from(from.0);
            let to = u32::from(to.0);
            let alpha = u32::from(alpha.min(32));
            let inverse = 32 - alpha;
            let red_blue = (((from & 0xf81f) * inverse + (to & 0xf81f) * alpha) >> 5) & 0xf81f;
            let green = (((from & 0x07e0) * inverse + (to & 0x07e0) * alpha) >> 5) & 0x07e0;
            Rgb565Pixel((red_blue | green) as u16)
        }

        let previous = (0..64)
            .map(|index| Rgb565Pixel((index as u16).wrapping_mul(977).wrapping_add(13)))
            .collect::<Vec<_>>();
        let current = (0..64)
            .map(|index| Rgb565Pixel((index as u16).wrapping_mul(613).wrapping_add(29)))
            .collect::<Vec<_>>();
        for alpha in 0..=32 {
            for offset in 0..8 {
                for length in 1..=24 {
                    let previous = &previous[offset..offset + length];
                    let current = &current[offset..offset + length];
                    let expected = previous
                        .iter()
                        .zip(current)
                        .map(|(&from, &to)| scalar(from, to, alpha))
                        .collect::<Vec<_>>();
                    let mut actual = vec![Rgb565Pixel(0xdead); length];
                    assert!(blend_rgb565_neon_if_available(
                        &mut actual,
                        previous,
                        current,
                        0,
                        length,
                        alpha,
                    ));
                    assert_eq!(
                        actual, expected,
                        "alpha={alpha} offset={offset} length={length}"
                    );

                    for fade_in in [false, true] {
                        let expected = previous
                            .iter()
                            .map(|&pixel| {
                                if fade_in {
                                    scalar(Rgb565Pixel(0), pixel, alpha)
                                } else {
                                    scalar(pixel, Rgb565Pixel(0), alpha)
                                }
                            })
                            .collect::<Vec<_>>();
                        let mut actual = vec![Rgb565Pixel(0xbeef); length];
                        assert!(blend_rgb565_black_neon_if_available(
                            &mut actual,
                            previous,
                            0,
                            length,
                            alpha,
                            fade_in,
                        ));
                        assert_eq!(
                            actual, expected,
                            "black alpha={alpha} offset={offset} length={length} fade_in={fade_in}"
                        );
                    }
                }
            }
        }
    }
}
