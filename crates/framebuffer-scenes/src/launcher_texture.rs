// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Prepared horizontal minification levels. Colour is premultiplied by
//! coverage before filtering, so transparent rounded corners cannot halo.
use crate::Rgb565Pixel;

pub(super) struct Texture {
    levels: Vec<Level>,
}
struct Level {
    pixels: Vec<u32>,
    width: usize,
    height: usize,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Filter {
    level: usize,
    mix: u32,
    x: i32,
}

fn rgba(pixel: Rgb565Pixel, alpha: u32) -> u32 {
    let bits = u32::from(pixel.0);
    let r = ((bits >> 11) * 255 / 31) * alpha / 255;
    let g = (((bits >> 5) & 63) * 255 / 63) * alpha / 255;
    let b = ((bits & 31) * 255 / 31) * alpha / 255;
    r | (g << 8) | (b << 16) | (alpha << 24)
}

#[inline]
pub(super) fn mix(a: u32, b: u32, weight: u32) -> u32 {
    let inv = 256 - weight;
    let rb = (((a & 0x00ff00ff) * inv + (b & 0x00ff00ff) * weight) >> 8) & 0x00ff00ff;
    let ga = ((((a >> 8) & 0x00ff00ff) * inv + ((b >> 8) & 0x00ff00ff) * weight) >> 8) & 0x00ff00ff;
    rb | (ga << 8)
}

pub(super) fn reflect_column(
    destination: &mut [Rgb565Pixel],
    pitch: usize,
    source: &[u16],
    rows: usize,
    sample: (i32, i32),
) {
    if rows == 0 {
        return;
    }
    assert!(pitch > 0 && (rows - 1) * pitch < destination.len());
    assert!(
        sample.1 > 0
            && i32::try_from(i64::from(sample.0) + rows as i64 * i64::from(sample.1)).is_ok()
    );
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_reflect_column(
                out: *mut u16,
                pitch: usize,
                src: *const u16,
                height: usize,
                rows: usize,
                q: i32,
                step: i32,
            );
        }
        // SAFETY: strided output and q progression checked above; the kernel
        // bounds-checks every source load, including vector lookahead/tails.
        unsafe {
            magik_launcher_reflect_column(
                destination.as_mut_ptr().cast(),
                pitch,
                source.as_ptr(),
                source.len(),
                rows,
                sample.0,
                sample.1,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    {
        let mut q = sample.0;
        let get = |r: i32| {
            if r < 0 {
                0
            } else {
                source.get(r as usize).copied().unwrap_or(0)
            }
        };
        for y in 0..rows {
            let row = q.div_euclid(65536);
            destination[y * pitch] = Rgb565Pixel(crate::launcher::mix_colour(
                get(row),
                get(row + 1),
                (q.rem_euclid(65536) / 256) as usize,
            ));
            q += sample.1;
        }
    }
}

pub(super) fn project_column(
    source: &[u32],
    destination: &mut [u32],
    pitch: usize,
    x: usize,
    rows: std::ops::Range<usize>,
    sample: (i32, i32),
) {
    assert!(x < pitch && rows.start <= rows.end && rows.end <= destination.len() / pitch);
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_project_column(
                out: *mut u32,
                pitch: usize,
                x: usize,
                top: usize,
                bottom: usize,
                src: *const u32,
                height: usize,
                q: i32,
                step: i32,
            );
        }
        // SAFETY: destination geometry is bounded above, source is live and
        // the kernel checks every source index, including transparent edges.
        unsafe {
            magik_launcher_project_column(
                destination.as_mut_ptr(),
                pitch,
                x,
                rows.start,
                rows.end,
                source.as_ptr(),
                source.len(),
                sample.0,
                sample.1,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    {
        let mut q = sample.0;
        for y in rows {
            let row = q >> 16;
            let get = |i: i32| {
                if i < 0 {
                    0
                } else {
                    source.get(i as usize).copied().unwrap_or(0)
                }
            };
            destination[y * pitch + x] = mix(get(row), get(row + 1), ((q & 65535) >> 8) as u32);
            q += sample.1;
        }
    }
}

#[cfg(test)]
pub(super) fn project_over_column(
    source: &[u32],
    destination: &mut [Rgb565Pixel],
    pitch: usize,
    rows: usize,
    sample: (i32, i32),
) {
    project_over_column_with_opaque(source, destination, pitch, rows, sample, 0..0);
}

pub(super) fn project_card_over_column(
    source: &[u32],
    destination: &mut [Rgb565Pixel],
    pitch: usize,
    rows: usize,
    sample: (i32, i32),
) {
    assert!(source.len() >= 16);
    project_over_column_with_opaque(
        source,
        destination,
        pitch,
        rows,
        sample,
        8..source.len() - 8,
    );
}

fn project_over_column_with_opaque(
    source: &[u32],
    destination: &mut [Rgb565Pixel],
    pitch: usize,
    rows: usize,
    sample: (i32, i32),
    _opaque: std::ops::Range<usize>,
) {
    if rows == 0 {
        return;
    }
    assert!(
        pitch > 0
            && (rows - 1)
                .checked_mul(pitch)
                .is_some_and(|last| last < destination.len())
    );
    assert!(
        sample.1 > 0
            && i32::try_from(
                i64::from(sample.0) + i64::try_from(rows).unwrap() * i64::from(sample.1)
            )
            .is_ok()
    );
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_project_over_column(
                out: *mut u16,
                pitch: usize,
                src: *const u32,
                height: usize,
                rows: usize,
                q: i32,
                step: i32,
                opaque_top: usize,
                opaque_bottom: usize,
            );
        }
        // SAFETY: output span and coordinate progression checked above;
        // the kernel checks source bounds for both vector pairs and tails.
        unsafe {
            magik_launcher_project_over_column(
                destination.as_mut_ptr().cast(),
                pitch,
                source.as_ptr(),
                source.len(),
                rows,
                sample.0,
                sample.1,
                _opaque.start,
                _opaque.end,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for y in 0..rows {
        let q = sample.0 + y as i32 * sample.1;
        let row = q >> 16;
        let get = |r: i32| {
            if r < 0 {
                0
            } else {
                source.get(r as usize).copied().unwrap_or(0)
            }
        };
        destination[y * pitch] = over(
            mix(get(row), get(row + 1), ((q & 65535) >> 8) as u32),
            destination[y * pitch],
        );
    }
}

pub(super) fn over_row(destination: &mut [Rgb565Pixel], source: &[u32]) {
    assert_eq!(destination.len(), source.len());
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_over_row(out: *mut u16, src: *const u32, n: usize);
        }
        // SAFETY: matching live slices; transparent u16 destination wrapper.
        unsafe {
            magik_launcher_over_row(
                destination.as_mut_ptr().cast(),
                source.as_ptr(),
                source.len(),
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for (d, &s) in destination.iter_mut().zip(source) {
        *d = over(s, *d);
    }
}

/// Uniform diffuse light on premultiplied RGB, preserving silhouette coverage.
/// Runs on the prepared column, shared by body and reflection, before RGB565.
pub(super) fn shade_rgba(pixels: &mut [u32], light: u32) {
    assert!(light <= 256);
    if light == 256 {
        return;
    }
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_shade_rgba(pixels: *mut u32, n: usize, light: u32);
        }
        // SAFETY: exclusive live slice; kernel bounds every vector and tail.
        unsafe { magik_launcher_shade_rgba(pixels.as_mut_ptr(), pixels.len(), light) };
    }
    #[cfg(not(target_arch = "arm"))]
    for p in pixels {
        let rb = (((*p & 0x00ff00ff) * light) >> 8) & 0x00ff00ff;
        let g = ((((*p >> 8) & 255) * light) >> 8) << 8;
        *p = (*p & 0xff000000) | rb | g;
    }
}

pub(super) fn mix_rgba(a: &mut [u32], b: &[u32], weight: u32) {
    assert_eq!(a.len(), b.len());
    assert!(weight <= 256);
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_mix_rgba(a: *mut u32, b: *const u32, n: usize, weight: u32);
        }
        // SAFETY: equal-length live slices, disjoint safe Rust borrows.
        unsafe {
            magik_launcher_mix_rgba(a.as_mut_ptr(), b.as_ptr(), a.len(), weight);
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for (a, &b) in a.iter_mut().zip(b) {
        *a = mix(*a, b, weight);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn project_flat_rgba(
    destination: &mut [u32],
    pitch: usize,
    source: &[u32],
    stride: usize,
    height: usize,
    width: usize,
    rows: usize,
    sample: (i32, i32),
) {
    assert!(width <= pitch && rows * pitch <= destination.len());
    assert!(height <= stride && width * stride <= source.len());
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_flat_rgba(
                out: *mut u32,
                pitch: usize,
                src: *const u32,
                stride: usize,
                height: usize,
                width: usize,
                rows: usize,
                q: i32,
                step: i32,
            );
        }
        // SAFETY: complete destination and source spans checked above; C
        // checks transparent rows and processes incomplete SIMD tails safely.
        unsafe {
            magik_launcher_flat_rgba(
                destination.as_mut_ptr(),
                pitch,
                source.as_ptr(),
                stride,
                height,
                width,
                rows,
                sample.0,
                sample.1,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for x in 0..width {
        project_column(
            &source[x * stride..x * stride + height],
            destination,
            pitch,
            x,
            0..rows,
            sample,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn project_flat(
    destination: &mut [Rgb565Pixel],
    pitch: usize,
    source: &[u32],
    stride: usize,
    height: usize,
    width: usize,
    rows: usize,
    sample: (i32, i32),
) {
    assert!(width <= pitch && rows * pitch <= destination.len());
    assert!(height <= stride && width * stride <= source.len());
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_flat(
                out: *mut u16,
                pitch: usize,
                src: *const u32,
                stride: usize,
                height: usize,
                width: usize,
                rows: usize,
                q: i32,
                step: i32,
            );
        }
        // SAFETY: complete row/column spans checked above; C bounds-checks
        // transparent source rows and handles incomplete SIMD groups.
        unsafe {
            magik_launcher_flat(
                destination.as_mut_ptr().cast(),
                pitch,
                source.as_ptr(),
                stride,
                height,
                width,
                rows,
                sample.0,
                sample.1,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for y in 0..rows {
        let q = sample.0 + y as i32 * sample.1;
        let row = q >> 16;
        for x in 0..width {
            let get = |r: i32| {
                if r >= 0 && (r as usize) < height {
                    source[x * stride + r as usize]
                } else {
                    0
                }
            };
            let p = mix(get(row), get(row + 1), ((q & 65535) >> 8) as u32);
            destination[y * pitch + x] = over(p, destination[y * pitch + x]);
        }
    }
}

#[cfg(test)]
pub(super) fn blend_row(
    output: &mut [Rgb565Pixel],
    a: &[Rgb565Pixel],
    b: &[Rgb565Pixel],
    weight: usize,
) {
    assert_eq!(output.len(), a.len());
    assert_eq!(output.len(), b.len());
    assert!(weight <= 256);
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_blend_row(
                out: *mut u16,
                a: *const u16,
                b: *const u16,
                n: usize,
                weight: u16,
            );
        }
        // SAFETY: equal, nonoverlapping live slices; Rgb565Pixel is transparent
        // over u16 and the kernel supports unaligned rows and scalar tails.
        unsafe {
            magik_launcher_blend_row(
                output.as_mut_ptr().cast(),
                a.as_ptr().cast(),
                b.as_ptr().cast(),
                output.len(),
                weight as u16,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for ((out, a), b) in output.iter_mut().zip(a).zip(b) {
        *out = Rgb565Pixel(crate::launcher::mix_colour(a.0, b.0, weight));
    }
}

#[cfg(test)]
pub(super) fn raster_row(
    output: &mut [Rgb565Pixel],
    alpha: &mut [u8],
    a: &[u32],
    b: &[u32],
    weight: u32,
) {
    assert_eq!(output.len(), alpha.len());
    assert_eq!(output.len(), a.len());
    assert_eq!(output.len(), b.len());
    assert!(weight <= 256);
    #[cfg(target_arch = "arm")]
    {
        unsafe extern "C" {
            fn magik_launcher_raster_row(
                out: *mut u16,
                alpha: *mut u8,
                a: *const u32,
                b: *const u32,
                n: usize,
                weight: u32,
            );
        }
        // SAFETY: lengths are equal, pointers are live and nonoverlapping;
        // Rgb565Pixel is repr(transparent) over u16. The kernel handles tails.
        unsafe {
            magik_launcher_raster_row(
                output.as_mut_ptr().cast(),
                alpha.as_mut_ptr(),
                a.as_ptr(),
                b.as_ptr(),
                output.len(),
                weight,
            );
        }
    }
    #[cfg(not(target_arch = "arm"))]
    for (((out, alpha), &a), &b) in output.iter_mut().zip(alpha).zip(a).zip(b) {
        let p = mix(a, b, weight);
        *alpha = (p >> 24) as u8;
        *out = unpremultiply(p);
    }
}

#[cfg(any(test, not(target_arch = "arm")))]
#[cfg(test)]
fn unpremultiply(p: u32) -> Rgb565Pixel {
    let Some(coverage) = std::num::NonZeroU32::new(p >> 24) else {
        return Rgb565Pixel(0);
    };
    let r = ((p & 255) * 255 / coverage).min(255);
    let g = (((p >> 8) & 255) * 255 / coverage).min(255);
    let b = (((p >> 16) & 255) * 255 / coverage).min(255);
    Rgb565Pixel(((r >> 3) << 11 | (g >> 2) << 5 | b >> 3) as u16)
}

impl Texture {
    pub fn storage_bytes(&self) -> usize {
        self.levels.iter().map(|l| l.pixels.capacity() * 4).sum()
    }
    pub fn new(pixels: &[Rgb565Pixel], width: usize, height: usize) -> Self {
        assert_eq!(pixels.len(), width * height);
        let mut base = vec![0; (width + 2) * height];
        for x in 0..width {
            for y in 0..height {
                let alpha = coverage(x, y, width, height);
                // Extend the edge colour before premultiplication. The old
                // RGB565 face stores black outside its rounded silhouette.
                let mut source_x = x;
                while alpha > 0 && !crate::launcher::rounded_contains(source_x, y, width, height) {
                    source_x = if source_x < width / 2 {
                        source_x + 1
                    } else {
                        source_x - 1
                    };
                }
                base[(x + 1) * height + y] = rgba(pixels[y * width + source_x], alpha);
            }
        }
        Self::from_base(base, width, height)
    }

    fn from_base(base: Vec<u32>, width: usize, height: usize) -> Self {
        let mut levels = vec![Level {
            pixels: base,
            width,
            height,
        }];
        while levels.last().unwrap().width > 1 {
            let old = levels.last().unwrap();
            let width = old.width.div_ceil(2);
            let mut pixels = vec![0; (width + 2) * height];
            for x in 0..width {
                for y in 0..height {
                    pixels[(x + 1) * height + y] = mix(
                        old.pixel((x * 2) as i32, y as i32),
                        old.pixel((x * 2 + 1).min(old.width - 1) as i32, y as i32),
                        128,
                    );
                }
            }
            levels.push(Level {
                pixels,
                width,
                height,
            });
        }
        Self { levels }
    }

    #[cfg(test)]
    pub fn reflection(&self, height: usize) -> Self {
        let source = &self.levels[0];
        let mut pixels = vec![0; (source.width + 2) * height];
        for x in 0..source.width {
            for y in 0..height {
                // Natural-scale mirror, not a compressed thumbnail. Padding
                // stays transparent; no sample can escape the lower quarter.
                if y < source.height / 4 {
                    pixels[(x + 1) * height + y] =
                        source.pixel(x as i32, (source.height - 1 - y) as i32);
                }
            }
        }
        Self::from_base(pixels, source.width, height)
    }

    pub fn filter(&self, x: i32, footprint: u32) -> Filter {
        let footprint = footprint.max(65536);
        let level = ((31 - footprint.leading_zeros()).saturating_sub(16) as usize)
            .min(self.levels.len() - 1);
        let mix = if level + 1 < self.levels.len() {
            ((footprint >> level).saturating_sub(65536) >> 8).min(256)
        } else {
            0
        };
        Filter { level, mix, x }
    }

    #[cfg(test)]
    pub fn prepare_column(&self, filter: Filter, output: &mut [u32]) {
        assert_eq!(output.len(), self.levels[filter.level].height);
        self.prepare_column_rows(filter, 0, output);
    }

    pub fn prepare_column_rows(&self, filter: Filter, start: usize, output: &mut [u32]) {
        let x = ((i64::from(filter.x) + 32768) >> filter.level) - 32768;
        let first = &self.levels[filter.level];
        let second = &self.levels[(filter.level + 1).min(self.levels.len() - 1)];
        let x2 = ((x + 32768) / 2 - 32768) as i32;
        let end = start.checked_add(output.len()).expect("bounded row range");
        assert!(end <= first.height && end <= second.height);
        let a0 = &first.column((x >> 16) as i32)[start..end];
        let a1 = &first.column((x >> 16) as i32 + 1)[start..end];
        let b0 = &second.column(x2 >> 16)[start..end];
        let b1 = &second.column((x2 >> 16) + 1)[start..end];
        let wx = ((x & 65535) >> 8) as u32;
        let wx2 = ((x2 & 65535) >> 8) as u32;
        #[cfg(target_arch = "arm")]
        {
            unsafe extern "C" {
                fn magik_launcher_filter_column(
                    out: *mut u32,
                    a0: *const u32,
                    a1: *const u32,
                    b0: *const u32,
                    b1: *const u32,
                    n: usize,
                    wx: u32,
                    wx2: u32,
                    lod: u32,
                );
            }
            // All columns have the asserted output height; the C kernel uses
            // unaligned loads and handles the final zero to three elements.
            unsafe {
                magik_launcher_filter_column(
                    output.as_mut_ptr(),
                    a0.as_ptr(),
                    a1.as_ptr(),
                    b0.as_ptr(),
                    b1.as_ptr(),
                    output.len(),
                    wx,
                    wx2,
                    filter.mix,
                );
            }
        }
        #[cfg(not(target_arch = "arm"))]
        for (y, pixel) in output.iter_mut().enumerate() {
            let a = mix(a0[y], a1[y], wx);
            *pixel = if filter.mix == 0 {
                a
            } else {
                mix(a, mix(b0[y], b1[y], wx2), filter.mix)
            };
        }
    }

    #[cfg(test)]
    #[inline]
    pub fn sample(&self, filter: Filter, y: i32) -> u32 {
        let x = ((i64::from(filter.x) + 32768) >> filter.level) - 32768;
        let first = self.levels[filter.level].sample(x as i32, y);
        if filter.mix == 0 {
            return first;
        }
        let second = self.levels[filter.level + 1].sample(((x + 32768) / 2 - 32768) as i32, y);
        mix(first, second, filter.mix)
    }
}

pub(super) fn coverage(x: usize, y: usize, width: usize, height: usize) -> u32 {
    let dx = x.min(width - 1 - x);
    let dy = y.min(height - 1 - y);
    if dx >= 8 || dy >= 8 {
        return 255;
    }
    let mut inside = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let a = (dx * 8 + sx * 2 + 1) as i32 - 64;
            let b = (dy * 8 + sy * 2 + 1) as i32 - 64;
            inside += u32::from(a * a + b * b <= 64 * 64);
        }
    }
    inside * 255 / 16
}

impl Level {
    #[inline]
    fn column(&self, x: i32) -> &[u32] {
        let index = if x < 0 || x as usize >= self.width {
            0
        } else {
            x as usize + 1
        };
        &self.pixels[index * self.height..(index + 1) * self.height]
    }
    #[inline]
    fn pixel(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            0
        } else {
            self.pixels[(x as usize + 1) * self.height + y as usize]
        }
    }

    #[cfg(test)]
    #[inline]
    fn sample(&self, x: i32, y: i32) -> u32 {
        let ix = x >> 16;
        let iy = y >> 16;
        let wx = ((x & 65535) >> 8) as u32;
        let wy = ((y & 65535) >> 8) as u32;
        mix(
            mix(self.pixel(ix, iy), self.pixel(ix + 1, iy), wx),
            mix(self.pixel(ix, iy + 1), self.pixel(ix + 1, iy + 1), wx),
            wy,
        )
    }
}

#[inline]
#[cfg_attr(all(target_arch = "arm", not(test)), allow(dead_code))]
pub(super) fn over(sample: u32, destination: Rgb565Pixel) -> Rgb565Pixel {
    let alpha = sample >> 24;
    if alpha == 0 {
        return destination;
    }
    if alpha == 255 {
        return Rgb565Pixel(
            (((sample & 0xf8) << 8) | (((sample >> 8) & 0xfc) << 3) | ((sample >> 19) & 31)) as u16,
        );
    }
    let bg = rgba(destination, 255 - alpha);
    let r = ((sample & 255) + (bg & 255)).min(255);
    let g = (((sample >> 8) & 255) + ((bg >> 8) & 255)).min(255);
    let b = (((sample >> 16) & 255) + ((bg >> 16) & 255)).min(255);
    Rgb565Pixel(((r >> 3) << 11 | (g >> 2) << 5 | (b >> 3)) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn light_preserves_alpha_and_matches_each_channel_at_all_tails() {
        for n in 0..=33 {
            for light in 0..=256 {
                let mut pixels: Vec<u32> = (0..n + 2)
                    .map(|i| (i as u32).wrapping_mul(0x792fe357))
                    .collect();
                let original = pixels.clone();
                shade_rgba(&mut pixels[1..n + 1], light);
                assert_eq!(pixels[0], original[0]);
                assert_eq!(pixels[n + 1], original[n + 1]);
                for i in 1..=n {
                    let p = original[i];
                    let mut expected = p & 0xff000000;
                    for shift in [0, 8, 16] {
                        expected |= (((p >> shift) & 255) * light / 256) << shift;
                    }
                    assert_eq!(pixels[i], expected);
                }
            }
        }
    }

    #[test]
    fn fused_perspective_matches_project_then_over_and_preserves_gaps() {
        let source: Vec<_> = (0..65)
            .map(|i| rgba(Rgb565Pixel((i * 997) as u16), (i * 17 % 256) as u32))
            .collect();
        let solid: Vec<_> = (0..65)
            .map(|i| if i < 32 { 0xff224466 } else { 0xff447788 })
            .collect();
        for source in [source, solid] {
            for origin in [-98304, -32768, 0, 12345, 61 * 65536] {
                for step in [32768, 65536, 98305, 131072] {
                    for rows in [0, 1, 2, 3, 63, 67] {
                        let mut actual = vec![Rgb565Pixel(0x5aa5); rows * 7 + 3];
                        let mut expected = actual.clone();
                        let mut projected = vec![0; rows * 7];
                        project_column(&source, &mut projected, 7, 0, 0..rows, (origin, step));
                        for y in 0..rows {
                            expected[y * 7] = over(projected[y * 7], expected[y * 7]);
                        }
                        project_over_column(&source, &mut actual, 7, rows, (origin, step));
                        assert_eq!(actual, expected);
                    }
                }
            }
        }
    }
    #[test]
    fn flat_projection_matches_column_reference_with_edges_and_vector_tails() {
        for width in [1, 3, 4, 7, 16, 179, 180] {
            let height = 25;
            let stride = 32;
            let pitch = width + 7;
            let rows = 24;
            let source: Vec<_> = (0..width * stride)
                .map(|i| {
                    rgba(
                        Rgb565Pixel((i * 3547) as u16),
                        if i % 7 == 0 { 91 } else { 255 },
                    )
                })
                .collect();
            for q in [-65536, -32768, 0, 4096, 32000, 64000] {
                for step in [65536, 90000, 104857] {
                    let mut actual = vec![Rgb565Pixel(0x5678); pitch * rows];
                    let mut expected = actual.clone();
                    let mut rgba = vec![0; pitch * rows];
                    for x in 0..width {
                        project_column(
                            &source[x * stride..x * stride + height],
                            &mut rgba,
                            pitch,
                            x,
                            0..rows,
                            (q, step),
                        );
                    }
                    for y in 0..rows {
                        over_row(
                            &mut expected[y * pitch..y * pitch + width],
                            &rgba[y * pitch..y * pitch + width],
                        );
                    }
                    project_flat(
                        &mut actual,
                        pitch,
                        &source,
                        stride,
                        height,
                        width,
                        rows,
                        (q, step),
                    );
                    assert_eq!(actual, expected);
                }
            }
        }
    }

    #[test]
    fn rgba_column_blend_matches_scalar_at_all_weights_and_tails() {
        for len in [0, 1, 3, 4, 7, 252] {
            let original: Vec<_> = (0..len)
                .map(|i| (i as u32).wrapping_mul(38791213))
                .collect();
            let other: Vec<_> = original.iter().map(|p| p.reverse_bits()).collect();
            for weight in 0..=256 {
                let mut actual = original.clone();
                mix_rgba(&mut actual, &other, weight);
                let expected: Vec<_> = original
                    .iter()
                    .zip(&other)
                    .map(|(&a, &b)| mix(a, b, weight))
                    .collect();
                assert_eq!(actual, expected);
            }
        }
    }
    #[test]
    fn projection_matches_scalar_across_weights_steps_and_vector_tails() {
        let source: Vec<_> = (0..19)
            .map(|i| {
                rgba(
                    Rgb565Pixel((i * 3547) as u16),
                    if i % 2 == 0 { 127 } else { 255 },
                )
            })
            .collect();
        for length in 0..=33 {
            for step in [32768, 57344, 65536, 79813, 131072] {
                for fraction in [0, 1, 255, 256, 32767, 65280, 65535] {
                    let start = -65536 + fraction;
                    let mut output = vec![0xdeadbeef; (length + 2) * 9];
                    project_column(&source, &mut output, 9, 4, 1..length + 1, (start, step));
                    for y in 0..length + 2 {
                        for x in 0..9 {
                            let expected = if x == 4 && (1..length + 1).contains(&y) {
                                let q = start + (y as i32 - 1) * step;
                                let get = |i: i32| {
                                    if i < 0 {
                                        0
                                    } else {
                                        source.get(i as usize).copied().unwrap_or(0)
                                    }
                                };
                                mix(get(q >> 16), get((q >> 16) + 1), ((q & 65535) >> 8) as u32)
                            } else {
                                0xdeadbeef
                            };
                            assert_eq!(output[y * 9 + x], expected);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn projected_columns_and_composite_rows_match_scalar_at_edges() {
        let source: Vec<_> = (0..17)
            .map(|i| {
                rgba(
                    Rgb565Pixel((i * 2417) as u16),
                    if i % 3 == 0 { 128 } else { 255 },
                )
            })
            .collect();
        let mut projected = vec![0; 7 * 23];
        project_column(&source, &mut projected, 7, 3, 1..22, (-32768, 57344));
        for y in 0..23 {
            for x in 0..7 {
                let expected = if x == 3 && (1..22).contains(&y) {
                    let q = -32768 + (y as i32 - 1) * 57344;
                    let get = |row: i32| {
                        if row < 0 {
                            0
                        } else {
                            source.get(row as usize).copied().unwrap_or(0)
                        }
                    };
                    mix(get(q >> 16), get((q >> 16) + 1), ((q & 65535) >> 8) as u32)
                } else {
                    0
                };
                assert_eq!(projected[y * 7 + x], expected);
            }
        }
        for len in [1, 3, 4, 7, 17] {
            let mut destination = vec![Rgb565Pixel(0xa73c); len];
            over_row(&mut destination, &source[..len]);
            for i in 0..len {
                assert_eq!(destination[i], over(source[i], Rgb565Pixel(0xa73c)));
            }
        }
    }
    #[test]
    fn rgb565_rows_match_scalar_with_all_weights_and_odd_tails() {
        let a: Vec<_> = (0..181).map(|i| Rgb565Pixel((i * 1379) as u16)).collect();
        let b: Vec<_> = (0..181).map(|i| Rgb565Pixel((i * 2417) as u16)).collect();
        for len in [1, 7, 8, 9, 180, 181] {
            let mut output = vec![Rgb565Pixel(0); len];
            for weight in 0..=256 {
                blend_row(&mut output, &a[..len], &b[..len], weight);
                for i in 0..len {
                    assert_eq!(
                        output[i].0,
                        crate::launcher::mix_colour(a[i].0, b[i].0, weight)
                    );
                }
            }
        }
    }
    #[test]
    fn raster_rows_match_scalar_for_opaque_edges_and_odd_tails() {
        for len in [1, 3, 4, 7, 16, 179, 180] {
            let a: Vec<_> = (0..len)
                .map(|i| {
                    rgba(
                        Rgb565Pixel((i * 1379) as u16),
                        if i % 9 == 0 { 128 } else { 255 },
                    )
                })
                .collect();
            let b: Vec<_> = (0..len)
                .map(|i| {
                    rgba(
                        Rgb565Pixel((i * 2417) as u16),
                        if i % 11 == 0 { 0 } else { 255 },
                    )
                })
                .collect();
            for weight in [0, 1, 64, 128, 255, 256] {
                let mut output = vec![Rgb565Pixel(0); len];
                let mut coverage = vec![0; len];
                raster_row(&mut output, &mut coverage, &a, &b, weight);
                for i in 0..len {
                    let p = mix(a[i], b[i], weight);
                    assert_eq!(output[i], unpremultiply(p));
                    assert_eq!(coverage[i], (p >> 24) as u8);
                }
            }
        }
    }
    #[test]
    fn reflection_crops_lower_quarter_without_compression() {
        let pixels: Vec<_> = (0..64 * 256)
            .map(|i| Rgb565Pixel(if i / 64 < 128 { 0xf800 } else { 0x001f }))
            .collect();
        let reflection = Texture::new(&pixels, 64, 256).reflection(64);
        let filter = reflection.filter(32 * 65536, 65536);
        assert_eq!(
            over(reflection.sample(filter, 8 * 65536), Rgb565Pixel(0)).0,
            0x001f
        );
        assert_eq!(
            over(reflection.sample(filter, 56 * 65536), Rgb565Pixel(0)).0,
            0x001f
        );
        let striped: Vec<_> = (0..64 * 252)
            .map(|i| Rgb565Pixel(if i / 64 == 251 { 0xf800 } else { 0x001f }))
            .collect();
        let cropped = Texture::new(&striped, 64, 252).reflection(64);
        let filter = cropped.filter(32 * 65536, 65536);
        assert_eq!(over(cropped.sample(filter, 0), Rgb565Pixel(0)).0, 0xf800);
        assert_eq!(
            over(cropped.sample(filter, 65536), Rgb565Pixel(0)).0,
            0x001f
        );
        assert_eq!(cropped.sample(filter, 63 * 65536), 0);
    }
    #[test]
    fn reflected_column_matches_old_rgb565_lerp_and_preserves_stride_gaps() {
        for height in [0, 1, 3, 64, 65] {
            let source: Vec<u16> = (0..height).map(|i| (i * 997 + 17) as u16).collect();
            for origin in [-98304, -32768, 0, 12345, 61 * 65536] {
                for step in [32768, 65536, 98305, 2 * 65536] {
                    for rows in [0, 1, 2, 3, 63, 67] {
                        let mut actual = vec![Rgb565Pixel(0x5aa5); rows * 7 + 3];
                        let mut expected = actual.clone();
                        for y in 0..rows {
                            let q = i64::from(origin) + y as i64 * i64::from(step);
                            let r = q.div_euclid(65536);
                            let get = |r: i64| {
                                if r < 0 {
                                    0
                                } else {
                                    source.get(r as usize).copied().unwrap_or(0)
                                }
                            };
                            expected[y * 7] = Rgb565Pixel(crate::launcher::mix_colour(
                                get(r),
                                get(r + 1),
                                (q.rem_euclid(65536) / 256) as usize,
                            ));
                        }
                        reflect_column(&mut actual, 7, &source, rows, (origin, step));
                        assert_eq!(actual, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn prepared_columns_match_scalar_filter_at_edges_and_mip_boundaries() {
        let pixels: Vec<_> = (0..65 * 67)
            .map(|i| Rgb565Pixel((i * 997) as u16))
            .collect();
        let texture = Texture::new(&pixels, 65, 67);
        let mut column = vec![0; 67];
        for footprint in [65536, 98304, 131071, 131072, 262143, 8 * 65536] {
            for x in [-32768, 0, 235929, 32 * 65536 + 49152, 64 * 65536] {
                let filter = texture.filter(x, footprint);
                texture.prepare_column(filter, &mut column);
                for start in [0, 1, 50, 66, 67] {
                    let mut cropped = vec![0; 67 - start];
                    texture.prepare_column_rows(filter, start, &mut cropped);
                    assert_eq!(cropped, column[start..]);
                }
                for (y, &pixel) in column.iter().enumerate() {
                    assert_eq!(pixel, texture.sample(filter, (y as i32) * 65536));
                }
            }
        }
    }
    #[test]
    fn compressed_stripes_average_instead_of_disappearing() {
        let pixels: Vec<_> = (0..64 * 64)
            .map(|i| Rgb565Pixel(if i % 2 == 0 { 0xffff } else { 0 }))
            .collect();
        let texture = Texture::new(&pixels, 64, 64);
        for x in [24 * 65536, 24 * 65536 + 32768, 25 * 65536] {
            let p = texture.sample(texture.filter(x, 8 * 65536), 32 * 65536);
            assert!((120..=130).contains(&(p & 255)));
            assert_eq!(p >> 24, 255);
        }
    }
    #[test]
    fn transparent_edges_do_not_add_black_halos() {
        assert_eq!(over(0, Rgb565Pixel(0xffff)), Rgb565Pixel(0xffff));
        let p = rgba(Rgb565Pixel(0xffff), 128);
        assert_eq!(over(p, Rgb565Pixel(0xffff)), Rgb565Pixel(0xffff));
    }
}
