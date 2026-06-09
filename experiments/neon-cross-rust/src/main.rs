use std::array;

extern "C" {
    fn neon_add_u8(a: *const u8, b: *const u8, out: *mut u8, len: u32);
    fn neon_dot_i16(a: *const i16, b: *const i16, len: u32) -> i32;
}

fn main() {
    let a: [u8; 37] = array::from_fn(|i| (i * 3) as u8);
    let b: [u8; 37] = array::from_fn(|i| (200usize - i * 2) as u8);
    let mut out = [0u8; 37];

    unsafe {
        neon_add_u8(a.as_ptr(), b.as_ptr(), out.as_mut_ptr(), out.len() as u32);
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
    let dot = unsafe { neon_dot_i16(lhs.as_ptr(), rhs.as_ptr(), lhs.len() as u32) };
    assert_eq!(dot, expected_dot);

    println!(
        "NEON C helper linked and passed: first_add={} dot={dot}",
        out[0]
    );
}
