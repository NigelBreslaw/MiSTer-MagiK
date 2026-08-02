// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint-free RGB565 particle renderer shared by the lab frontends.

pub mod particles;

/// Temporary standalone pixel boundary. The platform extraction will replace
/// this type with the shared transparent RGB565 primitive.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(transparent)]
pub struct Rgb565Pixel(pub u16);
