// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::time::Instant;

use super::camera_effects::CameraPixel;

pub(super) fn time(out: &mut u64, f: impl FnOnce()) {
    let t = Instant::now();
    f();
    *out += elapsed_us(t);
}

pub(super) fn elapsed_us(t: Instant) -> u64 {
    t.elapsed().as_micros() as u64
}

pub(super) fn clear(dst: &mut [CameraPixel], color: CameraPixel) {
    dst.fill(color);
}
