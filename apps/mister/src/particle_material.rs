// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded RGB565 material primitives shared by the particle showcase.

use slint::platform::software_renderer::Rgb565Pixel;

const MAX_STAMP_RADIUS: i16 = 5;
const MAX_STROKE_SAMPLES: usize = 48;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum MaterialShape {
    Disc,
    Spark,
    Star,
    Smoke,
    Shard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MaterialStamp {
    pub x: i16,
    pub y: i16,
    pub radius: u8,
    pub intensity: u8,
    pub color: Rgb565Pixel,
    pub shape: MaterialShape,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MaterialStroke {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub start_radius: u8,
    pub end_radius: u8,
    pub intensity: u8,
    pub color: Rgb565Pixel,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MaterialRasterStats {
    pub stamps: usize,
    pub attempted_pixel_writes: usize,
}

pub(crate) fn raster_stamp(
    destination: &mut [Rgb565Pixel],
    dirty_offsets: &mut Vec<u32>,
    width: usize,
    height: usize,
    stamp: MaterialStamp,
) -> MaterialRasterStats {
    let radius = i16::from(stamp.radius).clamp(1, MAX_STAMP_RADIUS);
    let intensity = stamp.intensity.min(15);
    let source_r = (stamp.color.0 >> 11) & 0x1f;
    let source_g = (stamp.color.0 >> 5) & 0x3f;
    let source_b = stamp.color.0 & 0x1f;
    let scaled_r = std::array::from_fn::<_, 16, _>(|alpha| source_r * alpha as u16 / 15);
    let scaled_g = std::array::from_fn::<_, 16, _>(|alpha| source_g * alpha as u16 / 15);
    let scaled_b = std::array::from_fn::<_, 16, _>(|alpha| source_b * alpha as u16 / 15);
    let mut stats = MaterialRasterStats {
        stamps: 1,
        attempted_pixel_writes: 0,
    };
    for dy in -radius..=radius {
        let y = stamp.y + dy;
        if !(0..height as i16).contains(&y) {
            continue;
        }
        for dx in -radius..=radius {
            let x = stamp.x + dx;
            if !(0..width as i16).contains(&x) {
                continue;
            }
            let coverage = material_coverage(stamp.shape, dx, dy, radius);
            if coverage == 0 {
                continue;
            }
            let offset = y as usize * width + x as usize;
            let alpha = ((u16::from(coverage) * u16::from(intensity) + 7) / 15).min(15) as u8;
            destination[offset] = additive_rgb565(
                destination[offset],
                scaled_r[usize::from(alpha)],
                scaled_g[usize::from(alpha)],
                scaled_b[usize::from(alpha)],
            );
            dirty_offsets.push(offset as u32);
            stats.attempted_pixel_writes = stats.attempted_pixel_writes.saturating_add(1);
        }
    }
    stats
}

pub(crate) fn raster_tapered_segment(
    destination: &mut [Rgb565Pixel],
    dirty_offsets: &mut Vec<u32>,
    width: usize,
    height: usize,
    stroke: MaterialStroke,
    sample_stride: usize,
    track_dirty: bool,
) -> MaterialRasterStats {
    let dx = stroke.x1 - stroke.x0;
    let dy = stroke.y1 - stroke.y0;
    let steps = dx
        .abs()
        .max(dy.abs())
        .ceil()
        .clamp(1.0, MAX_STROKE_SAMPLES as f32) as usize;
    let length = (dx * dx + dy * dy).sqrt().max(1.0);
    const FIXED_SHIFT: i32 = 16;
    const FIXED_ONE: i32 = 1 << FIXED_SHIFT;
    const FIXED_HALF: i32 = FIXED_ONE / 2;
    let x0_fixed = (stroke.x0 * FIXED_ONE as f32).round() as i32;
    let y0_fixed = (stroke.y0 * FIXED_ONE as f32).round() as i32;
    let dx_fixed = (dx * FIXED_ONE as f32).round() as i32;
    let dy_fixed = (dy * FIXED_ONE as f32).round() as i32;
    let center_step_x = dx_fixed / steps as i32;
    let center_step_y = dy_fixed / steps as i32;
    let normal_x_fixed = (-dy / length * FIXED_ONE as f32).round() as i32;
    let normal_y_fixed = (dx / length * FIXED_ONE as f32).round() as i32;
    let source_r = (stroke.color.0 >> 11) & 0x1f;
    let source_g = (stroke.color.0 >> 5) & 0x3f;
    let source_b = stroke.color.0 & 0x1f;
    let intensity = u16::from(stroke.intensity.min(15));
    let scaled_r = source_r * intensity / 15;
    let scaled_g = source_g * intensity / 15;
    let scaled_b = source_b * intensity / 15;
    let mut stats = MaterialRasterStats {
        stamps: steps + 1,
        attempted_pixel_writes: 0,
    };
    for step in (0..=steps).step_by(sample_stride.max(1)) {
        let start_radius = i32::from(stroke.start_radius.clamp(1, 3));
        let end_radius = i32::from(stroke.end_radius.clamp(1, 3));
        let radius = if start_radius == end_radius {
            start_radius
        } else {
            (start_radius * (steps - step) as i32 + end_radius * step as i32 + steps as i32 / 2)
                / steps as i32
        };
        let center_x_fixed = x0_fixed + center_step_x * step as i32;
        let center_y_fixed = y0_fixed + center_step_y * step as i32;
        for across in -radius..=radius {
            let x_fixed = center_x_fixed + normal_x_fixed * across;
            let y_fixed = center_y_fixed + normal_y_fixed * across;
            let x = if x_fixed >= 0 {
                (x_fixed + FIXED_HALF) >> FIXED_SHIFT
            } else {
                -((-x_fixed + FIXED_HALF) >> FIXED_SHIFT)
            } as i16;
            let y = if y_fixed >= 0 {
                (y_fixed + FIXED_HALF) >> FIXED_SHIFT
            } else {
                -((-y_fixed + FIXED_HALF) >> FIXED_SHIFT)
            } as i16;
            if !(0..width as i16).contains(&x) || !(0..height as i16).contains(&y) {
                continue;
            }
            let offset = y as usize * width + x as usize;
            destination[offset] =
                additive_rgb565(destination[offset], scaled_r, scaled_g, scaled_b);
            if track_dirty {
                dirty_offsets.push(offset as u32);
            }
            stats.attempted_pixel_writes = stats.attempted_pixel_writes.saturating_add(1);
        }
    }
    stats
}

fn material_coverage(shape: MaterialShape, dx: i16, dy: i16, radius: i16) -> u8 {
    let ax = dx.abs();
    let ay = dy.abs();
    let distance2 = dx * dx + dy * dy;
    let radius2 = radius * radius;
    match shape {
        MaterialShape::Disc => radial_coverage(distance2, radius2),
        MaterialShape::Spark => {
            if ax == 0 || ay == 0 {
                15u8.saturating_sub(((ax + ay) * 11 / radius.max(1)) as u8)
            } else if distance2 <= 1 {
                12
            } else {
                0
            }
        }
        MaterialShape::Star => {
            if ax == 0 || ay == 0 {
                15u8.saturating_sub(((ax + ay) * 9 / radius.max(1)) as u8)
            } else if ax == ay && ax <= (radius + 1) / 2 {
                9u8.saturating_sub((ax * 5 / radius.max(1)) as u8)
            } else {
                0
            }
        }
        MaterialShape::Smoke => {
            let radial = radial_coverage(distance2, radius2);
            if radial == 0 || (dx * 3 + dy * 5 + radius) & 3 == 0 {
                0
            } else {
                radial.saturating_mul(2) / 3
            }
        }
        MaterialShape::Shard => {
            if ax + ay <= radius {
                15u8.saturating_sub(((ax + ay) * 10 / radius.max(1)) as u8)
            } else {
                0
            }
        }
    }
}

fn radial_coverage(distance2: i16, radius2: i16) -> u8 {
    if distance2 > radius2 {
        0
    } else {
        (((radius2 - distance2) * 12 / radius2.max(1)) + 3).clamp(1, 15) as u8
    }
}

fn additive_rgb565(
    destination: Rgb565Pixel,
    source_r: u16,
    source_g: u16,
    source_b: u16,
) -> Rgb565Pixel {
    let dst_r = (destination.0 >> 11) & 0x1f;
    let dst_g = (destination.0 >> 5) & 0x3f;
    let dst_b = destination.0 & 0x1f;
    let red = (dst_r + source_r).min(31);
    let green = (dst_g + source_g).min(63);
    let blue = (dst_b + source_b).min(31);
    Rgb565Pixel((red << 11) | (green << 5) | blue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_shapes_are_distinct_and_bounded() {
        let mut signatures = Vec::new();
        for shape in [
            MaterialShape::Disc,
            MaterialShape::Spark,
            MaterialShape::Star,
            MaterialShape::Smoke,
            MaterialShape::Shard,
        ] {
            let signature = (-3..=3)
                .flat_map(|y| (-3..=3).map(move |x| material_coverage(shape, x, y, 3)))
                .collect::<Vec<_>>();
            assert!(signature.iter().any(|&coverage| coverage != 0));
            assert!(!signatures.contains(&signature));
            signatures.push(signature);
        }
    }

    #[test]
    fn additive_blend_preserves_rgb565_channels() {
        let blue = additive_rgb565(Rgb565Pixel(0), 0, 0, 31);
        assert_eq!(blue, Rgb565Pixel(0x001f));
        let saturated = additive_rgb565(Rgb565Pixel(0xffff), 31, 0, 0);
        assert_eq!(saturated, Rgb565Pixel(0xffff));
    }

    #[test]
    fn clipped_stamp_never_writes_outside_destination() {
        let mut destination = vec![Rgb565Pixel(0); 16];
        let mut dirty = Vec::new();
        let stats = raster_stamp(
            &mut destination,
            &mut dirty,
            4,
            4,
            MaterialStamp {
                x: 0,
                y: 0,
                radius: 5,
                intensity: 15,
                color: Rgb565Pixel(0xffff),
                shape: MaterialShape::Disc,
            },
        );
        assert!(stats.attempted_pixel_writes <= 16);
        assert!(dirty.iter().all(|&offset| offset < 16));
    }

    #[test]
    fn sparse_untracked_strokes_preserve_pixels_without_dirty_bookkeeping() {
        let stroke = MaterialStroke {
            x0: 1.0,
            y0: 4.0,
            x1: 14.0,
            y1: 4.0,
            start_radius: 1,
            end_radius: 2,
            intensity: 12,
            color: Rgb565Pixel(0x07ff),
        };
        let mut destination = vec![Rgb565Pixel(0); 16 * 8];
        let mut dirty = Vec::new();
        let sparse = raster_tapered_segment(&mut destination, &mut dirty, 16, 8, stroke, 2, false);

        assert!(sparse.attempted_pixel_writes > 0);
        assert!(dirty.is_empty());
        assert!(destination.iter().any(|pixel| pixel.0 != 0));
    }
}
