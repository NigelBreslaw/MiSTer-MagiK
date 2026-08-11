// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable RGB565 framebuffer-scene contracts.

use std::error::Error;
use std::fmt;
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
        for row in 0..height {
            let source_start = (source_y + row) * source_stride + source_x;
            for column in 0..width {
                let _ = self.set(
                    destination_x + column,
                    destination_y + row,
                    source[source_start + column],
                );
            }
        }
        true
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
}
