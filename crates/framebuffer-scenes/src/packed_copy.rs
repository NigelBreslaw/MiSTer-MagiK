// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact packed RGB565 copy. No sampling, pixel conversion or padding writes.
use crate::Rgb565Pixel;

pub fn copy_row(destination: &mut [u16], source: &[Rgb565Pixel]) {
    assert_eq!(destination.len(), source.len());
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_copy_row(dst: *mut u16, src: *const u16, n: usize);
        }
        // SAFETY: Rgb565Pixel is repr(transparent) over u16, both slices are
        // live, equally sized and disjoint; the kernel never accesses padding.
        unsafe {
            magik_launcher_copy_row(
                destination.as_mut_ptr(),
                source.as_ptr().cast(),
                source.len(),
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for (dst, src) in destination.iter_mut().zip(source) {
        *dst = src.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rows_preserve_bits_and_guards_at_all_vector_tails() {
        for offset in 0..16 {
            for length in 0..=129 {
                let source: Vec<_> = (0..length + offset)
                    .map(|x| Rgb565Pixel((x * 7919) as u16))
                    .collect();
                let mut destination = vec![0xdead; length + offset + 16];
                copy_row(&mut destination[offset..offset + length], &source[offset..]);
                assert!(destination[..offset].iter().all(|&p| p == 0xdead));
                assert!(destination[offset + length..].iter().all(|&p| p == 0xdead));
                assert!(
                    destination[offset..offset + length]
                        .iter()
                        .zip(&source[offset..])
                        .all(|(a, b)| *a == b.0)
                );
            }
        }
    }
    #[test]
    #[should_panic]
    fn mismatched_lengths_are_rejected() {
        copy_row(&mut [0; 2], &[Rgb565Pixel(0)]);
    }
}
