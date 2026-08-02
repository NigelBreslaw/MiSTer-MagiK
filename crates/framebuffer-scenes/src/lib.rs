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
    buffer_id: SceneBufferId,
}

impl<'a> SceneTarget<'a> {
    pub fn new(
        pixels: &'a mut [Rgb565Pixel],
        geometry: SceneGeometry,
        buffer_id: SceneBufferId,
    ) -> Result<Self, SceneError> {
        if pixels.len() != geometry.len() {
            return Err(SceneError::TargetSizeMismatch {
                actual: pixels.len(),
                expected: geometry.len(),
            });
        }
        Ok(Self {
            pixels,
            geometry,
            buffer_id,
        })
    }

    #[must_use]
    pub const fn geometry(&self) -> SceneGeometry {
        self.geometry
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
        assert_eq!(target.buffer_id(), id);
    }
}
