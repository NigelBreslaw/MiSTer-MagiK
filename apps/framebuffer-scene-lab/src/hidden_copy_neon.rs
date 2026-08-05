// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Explicit NEON full-frame copy used only by the hidden-slot device lab.

use mister_magik_mister_runtime::framebuffer::rgb565::Rgb565;

pub fn copy_rgb565(destination: &mut [Rgb565], source: &[Rgb565]) {
    assert_eq!(destination.len(), source.len());
    if destination.is_empty() {
        return;
    }
    // SAFETY: both slices contain exactly the same number of repr-transparent
    // RGB565 pixels, do not overlap, and the native function retains neither
    // pointer.
    unsafe {
        mister_magik_hidden_copy_rgb565_neon(
            destination.as_mut_ptr().cast::<u16>(),
            source.as_ptr().cast::<u16>(),
            source.len(),
        );
    }
}

pub fn crc32_rgb565(pixels: &[Rgb565]) -> u32 {
    // SAFETY: the byte view covers the initialized RGB565 slice exactly and
    // lives no longer than that shared slice.
    let bytes = unsafe {
        std::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), std::mem::size_of_val(pixels))
    };
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

unsafe extern "C" {
    fn mister_magik_hidden_copy_rgb565_neon(
        destination: *mut u16,
        source: *const u16,
        pixels: usize,
    );
}
