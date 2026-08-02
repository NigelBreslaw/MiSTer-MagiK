// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint-free RGB565 pixel shared by direct framebuffer renderers.

/// One native MiSTer RGB565 pixel.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rgb565(pub u16);

impl Rgb565 {
    pub const BLACK: Self = Self(0);

    #[must_use]
    pub const fn from_rgb888(red: u8, green: u8, blue: u8) -> Self {
        Self((red as u16 >> 3) << 11 | (green as u16 >> 2) << 5 | (blue as u16 >> 3))
    }

    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl From<u16> for Rgb565 {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl From<Rgb565> for u16 {
    fn from(value: Rgb565) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb888_conversion_uses_native_rgb565_layout() {
        assert_eq!(Rgb565::from_rgb888(255, 0, 0), Rgb565(0xf800));
        assert_eq!(Rgb565::from_rgb888(0, 255, 0), Rgb565(0x07e0));
        assert_eq!(Rgb565::from_rgb888(0, 0, 255), Rgb565(0x001f));
    }

    #[test]
    fn pixel_is_exactly_one_u16() {
        assert_eq!(std::mem::size_of::<Rgb565>(), std::mem::size_of::<u16>());
        assert_eq!(std::mem::align_of::<Rgb565>(), std::mem::align_of::<u16>());
    }
}
