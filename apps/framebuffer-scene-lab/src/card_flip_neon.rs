// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Contiguous RGB565 transfers for the card-flip staging framebuffer.

use mister_magik_framebuffer_scenes::Rgb565Pixel;

/// Fills a contiguous RGB565 destination with one packed pixel value.
pub fn fill_rgb565(destination: &mut [Rgb565Pixel], value: Rgb565Pixel) {
    if destination.is_empty() {
        return;
    }

    #[cfg(target_arch = "arm")]
    // SAFETY: the destination pointer is valid for `destination.len()` writable
    // RGB565 pixels and the native function does not retain it.
    unsafe {
        mister_magik_card_flip_neon_fill(
            destination.as_mut_ptr().cast::<u16>(),
            value.0,
            destination.len(),
        );
    }

    #[cfg(not(target_arch = "arm"))]
    destination.fill(value);
}

/// Fills a validated rectangle inside a packed RGB565 plane.
pub fn fill_rect_rgb565(
    destination: &mut [Rgb565Pixel],
    stride: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    value: Rgb565Pixel,
) {
    assert!(stride > 0 && destination.len().is_multiple_of(stride));
    assert!(x + width <= stride);
    assert!(y + height <= destination.len() / stride);
    if width == 0 || height == 0 {
        return;
    }

    #[cfg(target_arch = "arm")]
    // SAFETY: the assertions above prove every requested row is inside the
    // packed destination plane; the native function retains no pointer.
    unsafe {
        mister_magik_card_flip_neon_fill_rect(
            destination.as_mut_ptr().cast::<u16>(),
            stride,
            x,
            y,
            width,
            height,
            value.0,
        );
    }

    #[cfg(not(target_arch = "arm"))]
    for row in y..y + height {
        let start = row * stride + x;
        destination[start..start + width].fill(value);
    }
}

/// Copies one complete contiguous RGB565 plane into an equally sized destination.
///
/// # Panics
///
/// Panics when the source and destination lengths differ.
pub fn copy_rgb565(destination: &mut [Rgb565Pixel], source: &[Rgb565Pixel]) {
    assert_eq!(
        destination.len(),
        source.len(),
        "RGB565 transfer planes must have equal lengths"
    );
    if destination.is_empty() {
        return;
    }

    #[cfg(target_arch = "arm")]
    // SAFETY: equal slice lengths guarantee that both pointers are valid for
    // `source.len()` pixels. Safe Rust's borrowing rules prevent overlap while
    // the native function runs, and the native function retains neither pointer.
    unsafe {
        mister_magik_card_flip_neon_copy(
            destination.as_mut_ptr().cast::<u16>(),
            source.as_ptr().cast::<u16>(),
            source.len(),
        );
    }

    #[cfg(not(target_arch = "arm"))]
    destination.copy_from_slice(source);
}

#[cfg(target_arch = "arm")]
unsafe extern "C" {
    fn mister_magik_card_flip_neon_fill(destination: *mut u16, value: u16, count: usize);
    fn mister_magik_card_flip_neon_fill_rect(
        destination: *mut u16,
        stride: usize,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        value: u16,
    );
    fn mister_magik_card_flip_neon_copy(destination: *mut u16, source: *const u16, count: usize);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels(values: impl IntoIterator<Item = u16>) -> Vec<Rgb565Pixel> {
        values.into_iter().map(Rgb565Pixel).collect()
    }

    #[test]
    fn empty_transfers_are_noops() {
        fill_rgb565(&mut [], Rgb565Pixel(0x1234));
        copy_rgb565(&mut [], &[]);
    }

    #[test]
    fn fill_handles_vector_chunks_and_scalar_tails() {
        for length in [1, 7, 8, 9, 31, 32, 33, 40, 41] {
            let mut destination = vec![Rgb565Pixel(0); length];
            fill_rgb565(&mut destination, Rgb565Pixel(0xa55a));
            assert_eq!(destination, vec![Rgb565Pixel(0xa55a); length]);
        }
    }

    #[test]
    fn copy_handles_vector_chunks_and_scalar_tails() {
        for length in [1, 7, 8, 9, 31, 32, 33, 40, 41] {
            let source = pixels((0..length).map(|index| index as u16 ^ 0x5aa5));
            let mut destination = vec![Rgb565Pixel(0); length];
            copy_rgb565(&mut destination, &source);
            assert_eq!(destination, source);
        }
    }

    #[test]
    fn exact_fill_and_copy_preserve_rgb565_bits() {
        let source = pixels([0x0000, 0xffff, 0xf800, 0x07e0, 0x001f]);
        let mut destination = vec![Rgb565Pixel(0x1234); source.len()];
        fill_rgb565(&mut destination, Rgb565Pixel(0xabcd));
        assert_eq!(destination, vec![Rgb565Pixel(0xabcd); source.len()]);
        copy_rgb565(&mut destination, &source);
        assert_eq!(destination, source);
    }

    #[test]
    fn rectangle_fill_respects_stride_and_bounds() {
        let mut destination = vec![Rgb565Pixel(0); 8 * 6];
        fill_rect_rgb565(&mut destination, 8, 2, 1, 4, 3, Rgb565Pixel(0x55aa));
        for y in 0..6 {
            for x in 0..8 {
                let expected = if (1..4).contains(&y) && (2..6).contains(&x) {
                    Rgb565Pixel(0x55aa)
                } else {
                    Rgb565Pixel(0)
                };
                assert_eq!(destination[y * 8 + x], expected);
            }
        }
    }

    #[test]
    #[should_panic(expected = "RGB565 transfer planes must have equal lengths")]
    fn copy_rejects_different_plane_lengths() {
        copy_rgb565(&mut [Rgb565Pixel(0)], &[]);
    }
}
