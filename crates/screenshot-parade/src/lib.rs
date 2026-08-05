// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint-free screenshot parade rendering shared by production and scene labs.

mod raster;

pub use raster::{
    PARADE_SUBPIXEL_ONE, PreparedScreenshotCard, ScreenshotImage, ScreenshotSamplingProfile,
};
