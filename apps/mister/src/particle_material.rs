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
            destination[offset] = screen_rgb565(destination[offset], stamp.color, alpha);
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
) -> MaterialRasterStats {
    let dx = stroke.x1 - stroke.x0;
    let dy = stroke.y1 - stroke.y0;
    let steps = dx
        .abs()
        .max(dy.abs())
        .ceil()
        .clamp(1.0, MAX_STROKE_SAMPLES as f32) as usize;
    let mut stats = MaterialRasterStats::default();
    for step in 0..=steps {
        let amount = step as f32 / steps as f32;
        let radius = (f32::from(stroke.start_radius)
            + (f32::from(stroke.end_radius) - f32::from(stroke.start_radius)) * amount)
            .round() as u8;
        let fade = ((1.0 - amount * 0.35) * f32::from(stroke.intensity)).round() as u8;
        let sample = raster_stamp(
            destination,
            dirty_offsets,
            width,
            height,
            MaterialStamp {
                x: (stroke.x0 + dx * amount).round() as i16,
                y: (stroke.y0 + dy * amount).round() as i16,
                radius,
                intensity: fade,
                color: stroke.color,
                shape: MaterialShape::Disc,
            },
        );
        stats.stamps = stats.stamps.saturating_add(sample.stamps);
        stats.attempted_pixel_writes = stats
            .attempted_pixel_writes
            .saturating_add(sample.attempted_pixel_writes);
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

fn screen_rgb565(destination: Rgb565Pixel, source: Rgb565Pixel, alpha: u8) -> Rgb565Pixel {
    if alpha == 0 {
        return destination;
    }
    let dst_r = (destination.0 >> 11) & 0x1f;
    let dst_g = (destination.0 >> 5) & 0x3f;
    let dst_b = destination.0 & 0x1f;
    let src_r = ((source.0 >> 11) & 0x1f) * u16::from(alpha) / 15;
    let src_g = ((source.0 >> 5) & 0x3f) * u16::from(alpha) / 15;
    let src_b = (source.0 & 0x1f) * u16::from(alpha) / 15;
    let red = dst_r + src_r - dst_r * src_r / 31;
    let green = dst_g + src_g - dst_g * src_g / 63;
    let blue = dst_b + src_b - dst_b * src_b / 31;
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
    fn screen_blend_preserves_rgb565_channels() {
        let blue = screen_rgb565(Rgb565Pixel(0), Rgb565Pixel(0x001f), 15);
        assert_eq!(blue, Rgb565Pixel(0x001f));
        let saturated = screen_rgb565(Rgb565Pixel(0xffff), Rgb565Pixel(0xf800), 15);
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
}
