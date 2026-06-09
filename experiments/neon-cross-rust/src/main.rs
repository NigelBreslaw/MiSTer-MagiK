#![feature(stdarch_arm_neon_intrinsics)]

#[cfg(not(all(target_arch = "arm", target_feature = "neon")))]
compile_error!("build with an ARM target that has NEON enabled");

use core::arch::arm::{
    int16x8_t, int32x4_t, uint8x16_t, vaddq_u8, vdupq_n_s32, vget_high_s16, vget_low_s16,
    vld1q_s16, vld1q_u8, vmlal_s16, vst1q_s32, vst1q_u8,
};
use std::array;

unsafe fn neon_add_u8(a: &[u8], b: &[u8], out: &mut [u8]) {
    assert_eq!(a.len(), b.len());
    assert_eq!(a.len(), out.len());

    let mut i = 0;
    while i + 16 <= out.len() {
        let av: uint8x16_t = unsafe { vld1q_u8(a.as_ptr().add(i)) };
        let bv: uint8x16_t = unsafe { vld1q_u8(b.as_ptr().add(i)) };
        unsafe { vst1q_u8(out.as_mut_ptr().add(i), vaddq_u8(av, bv)) };
        i += 16;
    }

    while i < out.len() {
        out[i] = a[i].wrapping_add(b[i]);
        i += 1;
    }
}

unsafe fn neon_dot_i16(a: &[i16], b: &[i16]) -> i32 {
    assert_eq!(a.len(), b.len());

    let mut i = 0;
    let mut acc: int32x4_t = vdupq_n_s32(0);
    while i + 8 <= a.len() {
        let av: int16x8_t = unsafe { vld1q_s16(a.as_ptr().add(i)) };
        let bv: int16x8_t = unsafe { vld1q_s16(b.as_ptr().add(i)) };
        acc = vmlal_s16(acc, vget_low_s16(av), vget_low_s16(bv));
        acc = vmlal_s16(acc, vget_high_s16(av), vget_high_s16(bv));
        i += 8;
    }

    let mut lanes = [0i32; 4];
    unsafe { vst1q_s32(lanes.as_mut_ptr(), acc) };
    let mut total = lanes.into_iter().sum();

    while i < a.len() {
        total += i32::from(a[i]) * i32::from(b[i]);
        i += 1;
    }

    total
}

fn main() {
    let a: [u8; 37] = array::from_fn(|i| (i * 3) as u8);
    let b: [u8; 37] = array::from_fn(|i| (200usize - i * 2) as u8);
    let mut out = [0u8; 37];

    unsafe {
        neon_add_u8(&a, &b, &mut out);
    }

    for i in 0..out.len() {
        assert_eq!(out[i], a[i].wrapping_add(b[i]), "add mismatch at {i}");
    }

    let lhs: [i16; 19] = array::from_fn(|i| i as i16 - 9);
    let rhs: [i16; 19] = array::from_fn(|i| 4 - i as i16);
    let expected_dot: i32 = lhs
        .iter()
        .zip(rhs.iter())
        .map(|(x, y)| i32::from(*x) * i32::from(*y))
        .sum();
    let dot = unsafe { neon_dot_i16(&lhs, &rhs) };
    assert_eq!(dot, expected_dot);

    println!(
        "Pure Rust NEON intrinsics passed: first_add={} dot={dot}",
        out[0]
    );
}
