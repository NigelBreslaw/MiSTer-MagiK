// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Vertical door-hinge adaptation of the retained card_flip lab's Q16 sine and
//! inverse-column rasterizer. Pose/divisions are per column, never per pixel.
use crate::Rgb565Pixel;
const ONE: i64 = 65536;

#[derive(Clone, Copy, Default)]
pub(super) struct Column {
    valid: bool,
    sx: usize,
    source_y: i32,
    step: i32,
    top: usize,
    bottom: usize,
}

pub(super) struct Face {
    pub pixels: Vec<Rgb565Pixel>,
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy)]
pub(super) struct Pose {
    pub x: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    // pi radians = 65536; signed angle permits the preferred reverse mapping.
    pub angle: i64,
}

// Bhaskara approximation, as in the retained lab. Exact at 0, pi/2 and pi.
fn sin_pi(t: i64) -> i64 {
    let p = t * (ONE - t) / ONE;
    16 * p * ONE / (5 * ONE - 4 * p)
}

pub(super) fn sin_cos(angle: i64) -> (i64, i64) {
    let t = angle.rem_euclid(2 * ONE);
    let sin = if t <= ONE {
        sin_pi(t)
    } else {
        -sin_pi(t - ONE)
    };
    let c = (t + ONE / 2).rem_euclid(2 * ONE);
    let cos = if c <= ONE {
        sin_pi(c)
    } else {
        -sin_pi(c - ONE)
    };
    (sin, cos)
}

pub(super) fn draw(
    destination: &mut [Rgb565Pixel],
    face: &Face,
    pose: Pose,
    columns: &mut [Column],
    reflection: fn(u16, usize, usize) -> u16,
) {
    // Original lab geometry: the vertical hinge traverses the footprint while
    // the door swings away. Both cards share this pose, regardless of face.
    let progress = pose.angle.rem_euclid(ONE);
    let (sine, cosine) = sin_cos(progress);
    let hinge_x = i64::from(pose.x) * ONE + i64::from(pose.width - 1) * progress;
    let centre_y = i64::from(pose.top) * ONE + i64::from(pose.height - 1) * ONE / 2;
    let camera = i64::from(pose.width) * 460 / 258;
    columns.fill(Column::default());
    // Bound the scan independently of angle. No card can reach the sidebar.
    let left = pose.x.clamp(296, 934) as usize;
    let right = (pose.x + pose.width).clamp(296, 934) as usize;
    let spine = (pose.width * 12 / 258).max(3);
    if cosine.abs() * i64::from(pose.width) < i64::from(spine) * ONE {
        let start = (pose.x + pose.width / 2 - spine / 2).clamp(296, 933) as usize;
        let end = (start + spine as usize).min(934);
        let top = pose.top.max(120) as usize;
        let bottom = (pose.top + pose.height).min(438) as usize;
        for y in top..bottom {
            for x in start..end {
                destination[y * 960 + x] =
                    if x == start || x + 1 == end || y == top || y + 1 == bottom {
                        Rgb565Pixel(0xe73a)
                    } else {
                        face.pixels[10 * face.width + 10]
                    };
            }
        }
        for row in 0..32 {
            let y = bottom + 5 + row;
            if y >= 477 {
                break;
            }
            for x in start..end {
                destination[y * 960 + x] = Rgb565Pixel(reflection(
                    destination[(bottom - 1 - row) * 960 + x].0,
                    x,
                    row,
                ));
            }
        }
        return;
    }
    for (x, column) in columns.iter_mut().enumerate().take(right).skip(left) {
        let offset = x as i64 * ONE - hinge_x;
        let denominator = camera * cosine - offset * sine / ONE;
        if denominator.abs() < 4 {
            continue;
        }
        let local = offset * camera * ONE / denominator;
        if local < -ONE / 2 || local > i64::from(pose.width - 1) * ONE + ONE / 2 {
            continue;
        }
        let depth = ONE + local * sine / ONE / camera;
        if depth <= 0 {
            continue;
        }
        let sx = (local * (face.width - 1) as i64 / i64::from(pose.width - 1) + ONE / 2) / ONE;
        let sx = sx.clamp(0, (face.width - 1) as i64) as usize;
        let sx = if cosine < 0 { face.width - 1 - sx } else { sx };
        let step = depth * (face.height - 1) as i64 / i64::from(pose.height - 1);
        let zero = (face.height - 1) as i64 * ONE / 2 - centre_y * step / ONE;
        let top = ((-zero + step - 1) / step).clamp(120, 438) as usize;
        let bottom = (((face.height - 1) as i64 * ONE - zero) / step).clamp(120, 437) as usize;
        if top > bottom {
            continue;
        }
        *column = Column {
            valid: true,
            sx,
            source_y: (zero + 120 * step) as i32,
            step: step as i32,
            top,
            bottom,
        };
    }
    for y in 120..438 {
        for x in left..right {
            let c = &mut columns[x];
            if c.valid && y >= c.top && y <= c.bottom {
                let sy = ((c.source_y + (1 << 15)) >> 16) as usize;
                destination[y * 960 + x] = face.pixels[sy * face.width + c.sx];
            }
            c.source_y += c.step;
        }
    }
    for x in left..right {
        let c = columns[x];
        if !c.valid {
            continue;
        }
        for row in 0..32 {
            let y = c.bottom + 6 + row;
            if y >= 477 || c.bottom < c.top + row {
                break;
            }
            let source = destination[(c.bottom - row) * 960 + x].0;
            destination[y * 960 + x] = Rgb565Pixel(reflection(source, x, row));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_axes_and_same_world_rotation_have_opposite_visible_face_skew() {
        assert_eq!(sin_cos(0), (0, ONE));
        assert_eq!(sin_cos(ONE / 2), (ONE, 0));
        assert_eq!(sin_cos(ONE), (0, -ONE));
        assert_eq!(sin_cos(-ONE / 2), (-ONE, 0));
        let (a, b) = sin_cos(-ONE / 4);
        let (c, d) = sin_cos(ONE - ONE / 4);
        assert_eq!((a, b), (-c, -d));
    }
}
