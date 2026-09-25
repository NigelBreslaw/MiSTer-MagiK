// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Centre-pivot Y-axis adaptation of the retained card_flip lab's Q16 sine and
//! inverse-column rasterizer. Pose/divisions are per column, never per pixel.
use crate::Rgb565Pixel;
const ONE: i64 = 65536;
const COLUMN_HEIGHT: usize = 272;
pub(super) const STRIP_WIDTH: usize = 32;

pub(super) struct Scratch {
    columns: Vec<Column>,
    texels: Vec<u32>,
    projected: Vec<u32>,
    key: Option<(usize, usize, u32, Pose)>,
    blend: Vec<u32>,
    reflection_pixels: Vec<u16>,
}
impl Scratch {
    pub fn storage_bytes(&self) -> usize {
        self.texels.capacity() * 4
            + self.columns.capacity() * std::mem::size_of::<Column>()
            + self.projected.capacity() * 4
            + self.blend.capacity() * 4
            + self.reflection_pixels.capacity() * 2
    }
    pub fn new() -> Self {
        Self::with_width(960)
    }
    pub fn strip() -> Self {
        Self::with_width(STRIP_WIDTH)
    }
    fn with_width(width: usize) -> Self {
        Self {
            columns: vec![Column::default(); 960],
            texels: vec![0; width * COLUMN_HEIGHT],
            projected: vec![0; ((width.min(638).div_ceil(8) | 1) * 8) * 320],
            key: None,
            blend: vec![0; COLUMN_HEIGHT],
            reflection_pixels: vec![0; width * 64],
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Column {
    valid: bool,
    filter: crate::launcher_texture::Filter,
    source_y: i32,
    step: i32,
    top: usize,
    bottom: usize,
    reflection_y: i64,
}

pub(super) struct Face {
    #[cfg(test)]
    pub pixels: Vec<Rgb565Pixel>,
    pub width: usize,
    pub height: usize,
    pub(super) texture: crate::launcher_texture::Texture,
}

impl Face {
    pub fn storage_bytes(&self) -> usize {
        self.texture.storage_bytes()
    }
    pub fn new(pixels: Vec<Rgb565Pixel>, width: usize, height: usize) -> Self {
        let texture = crate::launcher_texture::Texture::new(&pixels, width, height);
        Self {
            #[cfg(test)]
            pixels,
            width,
            height,
            texture,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Pose {
    // Q16 pixel-edge coordinates, not rounded raster dimensions.
    pub x: i64,
    pub top: i64,
    pub width: i64,
    pub height: i64,
    // pi radians = 65536; signed angle permits the preferred reverse mapping.
    pub angle: i64,
    pub clip: (usize, usize),
    pub body_clip: (usize, usize),
}

#[derive(Clone, Copy, Default)]
struct OpaqueSpan {
    top: u16,
    bottom: u16,
}

#[derive(Clone, Copy)]
pub(super) struct BodyOcclusion {
    clip: (usize, usize),
    spans: [OpaqueSpan; STRIP_WIDTH],
}

impl BodyOcclusion {
    pub fn new(clip: (usize, usize)) -> Self {
        assert!(clip.0 <= clip.1 && clip.1 - clip.0 <= STRIP_WIDTH);
        Self {
            clip,
            spans: [OpaqueSpan::default(); STRIP_WIDTH],
        }
    }

    fn span(self, x: usize) -> Option<(usize, usize)> {
        let span = self.spans.get(x.checked_sub(self.clip.0)?)?;
        (span.top < span.bottom).then_some((usize::from(span.top), usize::from(span.bottom)))
    }

    fn add(&mut self, x: usize, top: usize, bottom: usize) {
        if top >= bottom || x < self.clip.0 || x >= self.clip.1 {
            return;
        }
        let candidate = OpaqueSpan {
            top: top as u16,
            bottom: bottom as u16,
        };
        let span = &mut self.spans[x - self.clip.0];
        if span.top >= span.bottom {
            *span = candidate;
        } else if top <= usize::from(span.bottom) && bottom >= usize::from(span.top) {
            span.top = span.top.min(candidate.top);
            span.bottom = span.bottom.max(candidate.bottom);
        } else if bottom - top > usize::from(span.bottom - span.top) {
            // A single conservative interval cannot represent a gap. Retain
            // whichever proven-opaque interval saves more work.
            *span = candidate;
        }
    }
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

fn diffuse_light(cosine: i64) -> u32 {
    // Two-sided card under a frontal light: 45% ambient at the edge, full
    // brightness on either face. Absolute normal avoids a face-swap flash.
    // Angle-driven, so direction changes and spring motion need no light timer.
    (116 + (140 * cosine.abs() + ONE / 2) / ONE) as u32
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(super) fn draw<F: Fn(u16, usize, usize) -> u16>(
    destination: &mut [Rgb565Pixel],
    face: &Face,
    pose: Pose,
    scratch: &mut Scratch,
    reflection: F,
    reflections_only: bool,
    blend: Option<(&Face, u32)>,
) {
    draw_target(
        destination,
        960,
        (0, 0),
        face,
        pose,
        scratch,
        reflection,
        reflections_only,
        blend,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_target<F: Fn(u16, usize, usize) -> u16>(
    destination: &mut [Rgb565Pixel],
    destination_pitch: usize,
    destination_origin: (usize, usize),
    face: &Face,
    pose: Pose,
    scratch: &mut Scratch,
    reflection: F,
    reflections_only: bool,
    blend: Option<(&Face, u32)>,
) {
    render(
        destination,
        destination_pitch,
        destination_origin,
        face,
        pose,
        scratch,
        reflection,
        reflections_only,
        blend,
        false,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(super) fn draw_occluded<F: Fn(u16, usize, usize) -> u16>(
    destination: &mut [Rgb565Pixel],
    face: &Face,
    pose: Pose,
    scratch: &mut Scratch,
    reflection: F,
    blend: Option<(&Face, u32)>,
    occlusion: &BodyOcclusion,
) {
    draw_occluded_target(
        destination,
        960,
        (0, 0),
        face,
        pose,
        scratch,
        reflection,
        blend,
        occlusion,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_occluded_target<F: Fn(u16, usize, usize) -> u16>(
    destination: &mut [Rgb565Pixel],
    destination_pitch: usize,
    destination_origin: (usize, usize),
    face: &Face,
    pose: Pose,
    scratch: &mut Scratch,
    reflection: F,
    blend: Option<(&Face, u32)>,
    occlusion: &BodyOcclusion,
) {
    render(
        destination,
        destination_pitch,
        destination_origin,
        face,
        pose,
        scratch,
        reflection,
        false,
        blend,
        false,
        Some(occlusion),
    );
}

#[allow(clippy::too_many_arguments)]
fn render<F: Fn(u16, usize, usize) -> u16>(
    destination: &mut [Rgb565Pixel],
    destination_pitch: usize,
    destination_origin: (usize, usize),
    face: &Face,
    pose: Pose,
    scratch: &mut Scratch,
    _reflection: F,
    reflections_only: bool,
    blend: Option<(&Face, u32)>,
    prepare_only: bool,
    occlusion: Option<&BodyOcclusion>,
) {
    let destination_index = |x: usize, y: usize| {
        debug_assert!(x >= destination_origin.0 && y >= destination_origin.1);
        (y - destination_origin.1) * destination_pitch + x - destination_origin.0
    };
    let left =
        ((pose.x - pose.width / 2) / ONE).clamp(pose.clip.0 as i64, pose.clip.1 as i64) as usize;
    let right = ((pose.x + pose.width * 3 / 2 + ONE - 1) / ONE)
        .clamp(pose.clip.0 as i64, pose.clip.1 as i64) as usize;
    if left == right {
        return;
    }
    // Rotate about the card centre. A longer camera distance keeps the
    // growing face inside the existing carousel clip without a door hinge.
    let (sine, cosine) = sin_cos(pose.angle);
    // A paper-thin plane disappears exactly side-on. Keep a two-pixel
    // projected spine so the card remains visible through the face swap.
    let minimum_cosine = (2 * ONE * ONE / pose.width).max(1);
    let projected_cosine = if cosine.abs() < minimum_cosine {
        if cosine < 0 {
            -minimum_cosine
        } else {
            minimum_cosine
        }
    } else {
        cosine
    };
    let projected_face_width = pose.width * cosine.abs() / ONE;
    let spine_weight = ((20 * ONE - projected_face_width) * 256 / (16 * ONE)).clamp(0, 256) as u32;
    let light = diffuse_light(cosine);
    let half = pose.width / 2;
    let centre_x = pose.x + half;
    let centre_y = pose.top + pose.height / 2;
    let camera = pose.width * 4;
    assert!(face.height <= COLUMN_HEIGHT);
    let key = (
        std::ptr::from_ref(&face.texture).addr(),
        blend.map_or(0, |(f, _)| std::ptr::from_ref(&f.texture).addr()),
        blend.map_or(0, |(_, w)| w),
        pose,
    );
    let rebuild = scratch.key != Some(key);
    scratch.key = Some(key);
    let columns = &mut scratch.columns;
    let texels = &mut scratch.texels;
    // Bound the scan independently of angle. No card can reach the sidebar.
    let flat = pose.angle == 0;
    let flat_step = ONE * face.height as i64 * ONE / pose.height;
    let flat_zero = (face.height - 1) as i64 * ONE / 2 - (centre_y - ONE / 2) * flat_step / ONE;
    let flat_top = ((-ONE - flat_zero + flat_step - 1) / flat_step).clamp(120, 438) as usize;
    let flat_bottom = ((face.height as i64 * ONE - flat_zero) / flat_step).clamp(120, 437) as usize;
    let flat_reflection =
        ((face.height as i64 * ONE - ONE / 2 - flat_zero) * ONE / flat_step + ONE / 2 + 3 * ONE)
            .clamp(120 * ONE, 495 * ONE);
    let flat_footprint = (ONE * face.width as i64 * ONE / pose.width).max(ONE) as u32;
    if rebuild {
        // Only this clipped span is read below; stale columns outside it are
        // irrelevant and need not churn the other core's shared cache.
        columns[left..right].fill(Column::default());
        #[cfg(feature = "launcher-profile")]
        let _profile = crate::launcher_profile::span("flip.geometry-filter");
        for (x, column) in columns.iter_mut().enumerate().take(right).skip(left) {
            let offset = x as i64 * ONE + ONE / 2 - centre_x;
            let denominator = camera * projected_cosine / ONE - offset * sine / ONE;
            if denominator.abs() < 4 {
                continue;
            }
            let local = if flat {
                offset
            } else {
                offset * camera / denominator
            };
            if local < -half - ONE || local > half + ONE {
                continue;
            }
            let depth = if flat {
                ONE
            } else {
                ONE + local * sine / camera
            };
            if depth <= 0 {
                continue;
            }
            let sxq = (local + half) * face.width as i64 * ONE / pose.width - ONE / 2;
            let next_offset = offset + ONE;
            let next_denominator = camera * projected_cosine / ONE - next_offset * sine / ONE;
            let next_local = if flat {
                next_offset
            } else if next_denominator.abs() >= 4 {
                next_offset * camera / next_denominator
            } else {
                local
            };
            let footprint = if flat {
                flat_footprint
            } else {
                ((next_local - local).abs() * face.width as i64 * ONE / pose.width)
                    .clamp(ONE, i64::from(u32::MAX)) as u32
            };
            let sxq = if cosine < 0 {
                (face.width - 1) as i64 * ONE - sxq
            } else {
                sxq
            };
            let step = if flat {
                flat_step
            } else {
                depth * face.height as i64 * ONE / pose.height
            };
            let zero = if flat {
                flat_zero
            } else {
                (face.height - 1) as i64 * ONE / 2 - (centre_y - ONE / 2) * step / ONE
            };
            // Include the fractional outer row; transparent texture samples
            // provide coverage rather than hard-clipping a moving silhouette.
            let top = if flat {
                flat_top
            } else {
                ((-ONE - zero + step - 1) / step).clamp(120, 438) as usize
            };
            let bottom = if flat {
                flat_bottom
            } else {
                ((face.height as i64 * ONE - zero) / step).clamp(120, 437) as usize
            };
            if top > bottom {
                continue;
            }
            *column = Column {
                valid: true,
                filter: face.texture.filter(sxq as i32, footprint),
                source_y: (zero + 120 * step) as i32,
                step: step as i32,
                top,
                bottom,
                // Inverse mapping uses pixel centres; convert the lower edge
                // back to edge coordinates, then retain the fractional gap.
                reflection_y: if flat {
                    flat_reflection
                } else {
                    ((face.height as i64 * ONE - ONE / 2 - zero) * ONE / step + ONE / 2 + 3 * ONE)
                        .clamp(120 * ONE, 495 * ONE)
                },
            };
            let start = if !prepare_only && (x < pose.body_clip.0 || x >= pose.body_clip.1) {
                face.height - face.height / 4
            } else {
                0
            };
            face.texture.prepare_column_rows(
                column.filter,
                start,
                &mut texels
                    [(x - left) * COLUMN_HEIGHT + start..(x - left) * COLUMN_HEIGHT + face.height],
            );
            if let Some((other, weight)) = blend {
                other.texture.prepare_column_rows(
                    column.filter,
                    start,
                    &mut scratch.blend[start..face.height],
                );
                crate::launcher_texture::mix_rgba(
                    &mut texels[(x - left) * COLUMN_HEIGHT + start
                        ..(x - left) * COLUMN_HEIGHT + face.height],
                    &scratch.blend[start..face.height],
                    weight,
                );
            }
            if spine_weight > 0 {
                // The visible two-pixel side uses the existing coloured rim
                // rather than an average of the dark face artwork.
                face.texture.prepare_column_rows(
                    face.texture.filter(4 * ONE as i32, ONE as u32),
                    start,
                    &mut scratch.blend[start..face.height],
                );
                crate::launcher_texture::mix_rgba(
                    &mut texels[(x - left) * COLUMN_HEIGHT + start
                        ..(x - left) * COLUMN_HEIGHT + face.height],
                    &scratch.blend[start..face.height],
                    spine_weight,
                );
            }
            crate::launcher_texture::shade_rgba(
                &mut texels
                    [(x - left) * COLUMN_HEIGHT + start..(x - left) * COLUMN_HEIGHT + face.height],
                light,
            );
        }
    }
    if rebuild || !reflections_only {
        let active_left = (left..right).find(|&x| columns[x].valid).unwrap_or(right);
        let active_right = (left..right)
            .rev()
            .find(|&x| columns[x].valid)
            .map_or(active_left, |x| x + 1);
        let active_top = columns[active_left..active_right]
            .iter()
            .filter(|c| c.valid)
            .map(|c| c.top)
            .min()
            .unwrap_or(120);
        let active_bottom = columns[active_left..active_right]
            .iter()
            .filter(|c| c.valid)
            .map(|c| c.bottom + 1)
            .max()
            .unwrap_or(active_top);
        // A compact odd number of 32-byte cache lines per row avoids the
        // full-screen stride's repeated cache-set collisions during column
        // writes. Geometry and filtering remain exactly the same.
        let active_width = active_right - active_left;
        let pitch = (active_width.div_ceil(8) | 1) * 8;
        assert!(pitch * (active_bottom - active_top) <= scratch.projected.len());
        if rebuild {
            #[cfg(feature = "launcher-profile")]
            let _profile = crate::launcher_profile::span("flip.project");
            if flat && prepare_only {
                crate::launcher_texture::project_flat_rgba(
                    &mut scratch.projected,
                    pitch,
                    &texels[(active_left - left) * COLUMN_HEIGHT
                        ..(active_right - left) * COLUMN_HEIGHT],
                    COLUMN_HEIGHT,
                    face.height,
                    active_width,
                    active_bottom - active_top,
                    (
                        (flat_zero + active_top as i64 * flat_step) as i32,
                        flat_step as i32,
                    ),
                );
            } else if !flat && prepare_only {
                for y in active_top..active_bottom {
                    let row = (y - active_top) * pitch;
                    scratch.projected[row..row + active_width].fill(0);
                }
                for x in active_left..active_right {
                    let c = &columns[x];
                    if !c.valid {
                        continue;
                    }
                    crate::launcher_texture::project_column(
                        &texels
                            [(x - left) * COLUMN_HEIGHT..(x - left) * COLUMN_HEIGHT + face.height],
                        &mut scratch.projected,
                        pitch,
                        x - active_left,
                        c.top - active_top..c.bottom + 1 - active_top,
                        (c.source_y + (c.top as i32 - 120) * c.step, c.step),
                    );
                }
            }
        }
        if !reflections_only {
            #[cfg(feature = "launcher-profile")]
            let _profile = crate::launcher_profile::span("flip.compose");
            if flat {
                crate::launcher_texture::project_flat(
                    &mut destination[destination_index(active_left, active_top)..],
                    destination_pitch,
                    &texels[(active_left - left) * COLUMN_HEIGHT
                        ..(active_right - left) * COLUMN_HEIGHT],
                    COLUMN_HEIGHT,
                    face.height,
                    active_width,
                    active_bottom - active_top,
                    (
                        (flat_zero + active_top as i64 * flat_step) as i32,
                        flat_step as i32,
                    ),
                );
            } else if !prepare_only {
                for (x, c) in columns
                    .iter()
                    .enumerate()
                    .take(active_right)
                    .skip(active_left)
                {
                    if !c.valid || x < pose.body_clip.0 || x >= pose.body_clip.1 {
                        continue;
                    }
                    let source = &texels
                        [(x - left) * COLUMN_HEIGHT..(x - left) * COLUMN_HEIGHT + face.height];
                    let mut project = |top: usize, bottom: usize| {
                        if top < bottom {
                            crate::launcher_texture::project_card_over_column(
                                source,
                                &mut destination[destination_index(x, top)..],
                                destination_pitch,
                                bottom - top,
                                (c.source_y + (top as i32 - 120) * c.step, c.step),
                            );
                        }
                    };
                    if let Some((covered_top, covered_bottom)) =
                        occlusion.and_then(|mask| mask.span(x))
                    {
                        let covered_top = covered_top.clamp(c.top, c.bottom + 1);
                        let covered_bottom = covered_bottom.clamp(c.top, c.bottom + 1);
                        if covered_top < covered_bottom {
                            project(c.top, covered_top);
                            project(covered_bottom, c.bottom + 1);
                        } else {
                            project(c.top, c.bottom + 1);
                        }
                    } else {
                        project(c.top, c.bottom + 1);
                    }
                }
            } else {
                for y in active_top..active_bottom {
                    let row_start = destination_index(active_left, y);
                    let range = row_start..row_start + active_width;
                    let row = (y - active_top) * pitch;
                    crate::launcher_texture::over_row(
                        &mut destination[range],
                        &scratch.projected[row..row + active_width],
                    );
                }
            }
        }
    }
    if rebuild {
        #[cfg(feature = "launcher-profile")]
        let _profile = crate::launcher_profile::span("reflection.prepare");
        for (x, c) in columns.iter().enumerate().take(right).skip(left) {
            if c.valid {
                #[cfg(all(target_arch = "arm", not(test)))]
                {
                    unsafe extern "C" {
                        fn magik_launcher_prepare_reflection(
                            out: *mut u16,
                            body: *const u32,
                            height: usize,
                            x: usize,
                        );
                    }
                    let body = &texels
                        [(x - left) * COLUMN_HEIGHT..(x - left) * COLUMN_HEIGHT + face.height];
                    let output =
                        &mut scratch.reflection_pixels[(x - left) * 64..(x - left + 1) * 64];
                    // SAFETY: the body slice contains `face.height` prepared
                    // texels and output is an independent 64-pixel column.
                    unsafe {
                        magik_launcher_prepare_reflection(
                            output.as_mut_ptr(),
                            body.as_ptr(),
                            body.len(),
                            x,
                        );
                    }
                }
                #[cfg(any(not(target_arch = "arm"), test))]
                for row in 0..64 {
                    scratch.reflection_pixels[(x - left) * 64 + row] = _reflection(
                        crate::launcher_texture::over(
                            reflected_texel(
                                &texels[(x - left) * COLUMN_HEIGHT
                                    ..(x - left) * COLUMN_HEIGHT + face.height],
                                row,
                            ),
                            Rgb565Pixel(0),
                        )
                        .0,
                        x,
                        row,
                    );
                }
            }
        }
    }
    if reflections_only && !prepare_only {
        #[cfg(feature = "launcher-profile")]
        let _profile = crate::launcher_profile::span("flip.reflection");
        for x in left..right {
            let c = columns[x];
            if !c.valid {
                continue;
            }
            let origin = c.reflection_y.div_euclid(ONE);
            for y in (origin - 3).max(120)..(origin + 1).min(495) {
                let q = y * ONE - (c.reflection_y - 3 * ONE);
                let row = q.div_euclid(ONE);
                let weight = q.rem_euclid(ONE) / 256;
                let opacity = |r: i64| match r {
                    0 => 144,
                    1 => 80,
                    2 => 32,
                    _ => 0,
                };
                let alpha = (opacity(row) * (256 - weight) + opacity(row + 1) * weight) / 256;
                let index = destination_index(x, y as usize);
                destination[index] = Rgb565Pixel(crate::launcher::mix_colour(
                    destination[index].0,
                    0,
                    alpha as usize,
                ));
            }
            let first_y = origin.max(120);
            // Reuse the body's source-texels-per-screen-pixel step. The
            // mirrored quarter shrinks and grows with the card, not the strip.
            let step = i64::from(c.step);
            let q = ((first_y * ONE + ONE / 2 - c.reflection_y) * step).div_euclid(ONE) - ONE / 2;
            let end = (c.reflection_y + (face.height / 4).min(64) as i64 * ONE * ONE / step + ONE
                - 1)
                / ONE;
            // Fade remains baked before interpolation. The vector kernel
            // only replaces the exact RGB565 lerp, not sampling or the fade.
            crate::launcher_texture::reflect_column(
                &mut destination[destination_index(x, first_y as usize)..],
                destination_pitch,
                &scratch.reflection_pixels[(x - left) * 64..(x - left + 1) * 64],
                (end.min(495) - first_y).max(0) as usize,
                (i32::try_from(q).expect("bounded reflection origin"), c.step),
            );
        }
    }
}

pub(super) fn add_opaque_coverage(
    face: &Face,
    pose: Pose,
    scratch: &Scratch,
    coverage: &mut BodyOcclusion,
) {
    let left =
        ((pose.x - pose.width / 2) / ONE).clamp(pose.clip.0 as i64, pose.clip.1 as i64) as usize;
    let right = ((pose.x + pose.width * 3 / 2 + ONE - 1) / ONE)
        .clamp(pose.clip.0 as i64, pose.clip.1 as i64) as usize;
    if left == right || face.height <= 16 {
        return;
    }
    let source_top = 8_i64;
    let source_bottom = face.height as i64 - 8;
    for x in left.max(pose.body_clip.0)..right.min(pose.body_clip.1) {
        let column = scratch.columns[x];
        if !column.valid {
            continue;
        }
        let source =
            &scratch.texels[(x - left) * COLUMN_HEIGHT..(x - left) * COLUMN_HEIGHT + face.height];
        if source[source_top as usize] >> 24 != 255
            || source[source_bottom as usize - 1] >> 24 != 255
        {
            continue;
        }
        let step = i64::from(column.step);
        let ceil_div = |value: i64| {
            let quotient = value.div_euclid(step);
            quotient + i64::from(value.rem_euclid(step) != 0)
        };
        // Both vertical bilinear inputs must stay inside the known opaque
        // source interior. This deliberately excludes its two boundary rows.
        let top = 120 + ceil_div(source_top * ONE - i64::from(column.source_y));
        let bottom = 120 + ceil_div((source_bottom - 1) * ONE - i64::from(column.source_y));
        coverage.add(
            x,
            top.clamp(column.top as i64, column.bottom as i64 + 1) as usize,
            bottom.clamp(column.top as i64, column.bottom as i64 + 1) as usize,
        );
    }
}

// Horizontal filtering and face blending commute with reversing rows. Reuse
// the completed body column, preserving the lower-quarter transparent pad.
#[cfg_attr(all(target_arch = "arm", not(test)), allow(dead_code))]
fn reflected_texel(body: &[u32], row: usize) -> u32 {
    if row < body.len() / 4 {
        body[body.len() - 1 - row]
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn light_tracks_angle_symmetrically_without_a_face_swap_flash() {
        assert_eq!(diffuse_light(sin_cos(0).1), 256);
        assert_eq!(diffuse_light(sin_cos(ONE).1), 256);
        assert_eq!(diffuse_light(sin_cos(ONE / 2).1), 116);
        let mut previous = 256;
        for angle in 0..=ONE / 2 {
            let light = diffuse_light(sin_cos(angle).1);
            assert!(light <= previous && previous - light <= 1);
            assert_eq!(light, diffuse_light(sin_cos(-angle).1));
            assert_eq!(light, diffuse_light(sin_cos(ONE - angle).1));
            previous = light;
        }
    }

    #[test]
    fn reused_reflection_matches_separate_filter_and_blend() {
        let a = Face::new(
            (0..65 * 252)
                .map(|i| Rgb565Pixel((i * 997) as u16))
                .collect(),
            65,
            252,
        );
        let b = Face::new(
            (0..65 * 252)
                .map(|i| Rgb565Pixel((i * 313 + 31) as u16))
                .collect(),
            65,
            252,
        );
        let ar = a.texture.reflection(64);
        let br = b.texture.reflection(64);
        for footprint in [65536, 98304, 131071, 131072, 262143, 8 * 65536] {
            for x in [-32768, 0, 235929, 32 * 65536 + 49152, 64 * 65536] {
                let filter = a.texture.filter(x, footprint);
                let mut ac = [0; 252];
                let mut bc = [0; 252];
                let mut arc = [0; 64];
                let mut brc = [0; 64];
                a.texture.prepare_column(filter, &mut ac);
                b.texture.prepare_column(filter, &mut bc);
                ar.prepare_column(filter, &mut arc);
                br.prepare_column(filter, &mut brc);
                for weight in 0..=256 {
                    let mut body = ac;
                    let mut reference = arc;
                    crate::launcher_texture::mix_rgba(&mut body, &bc, weight);
                    crate::launcher_texture::mix_rgba(&mut reference, &brc, weight);
                    for (row, expected) in reference.into_iter().enumerate() {
                        assert_eq!(reflected_texel(&body, row), expected);
                    }
                }
            }
        }
    }
    #[test]
    fn fractional_translation_and_scale_change_pixels_before_integer_bounds_change() {
        let face = Face::new(vec![Rgb565Pixel(0xffff); 180 * 252], 180, 252);
        let mut scratch = Scratch::new();
        let mut previous = vec![Rgb565Pixel(0); 960 * 540];
        let mut frame = previous.clone();
        for phase in 0..16 {
            frame.fill(Rgb565Pixel(0));
            let pose = Pose {
                x: 520 * ONE + phase * ONE / 16,
                top: 158 * ONE + phase * ONE / 16,
                width: 180 * ONE - phase * ONE / 16,
                height: (180 * ONE - phase * ONE / 16) * 7 / 5,
                angle: 0,
                clip: (296, 934),
                body_clip: (296, 934),
            };
            draw(
                &mut frame,
                &face,
                pose,
                &mut scratch,
                |p, _, _| p,
                false,
                None,
            );
            if phase > 0 {
                assert!(frame != previous, "phase {phase} snapped");
            }
            previous.copy_from_slice(&frame);
        }
    }

    #[test]
    fn reflection_attachment_tracks_fractional_bottom_and_starts_with_bottom_colour() {
        let pixels = (0..180 * 252)
            .map(|i| Rgb565Pixel(if i / 180 < 126 { 0xf800 } else { 0x001f }))
            .collect();
        let face = Face::new(pixels, 180, 252);
        let mut scratch = Scratch::new();
        let mut frame = vec![Rgb565Pixel(0); 960 * 540];
        for phase in 0..16 {
            let pose = Pose {
                x: 520 * ONE,
                top: 158 * ONE + phase * ONE / 16,
                width: 180 * ONE,
                height: 252 * ONE,
                angle: 0,
                clip: (296, 934),
                body_clip: (296, 934),
            };
            frame.fill(Rgb565Pixel(0));
            draw(
                &mut frame,
                &face,
                pose,
                &mut scratch,
                |p, _, _| p,
                true,
                None,
            );
            let origin = scratch.columns[610].reflection_y;
            assert_eq!(origin, pose.top + pose.height + 3 * ONE);
            assert_eq!(
                frame[((origin / ONE) as usize + 3) * 960 + 610],
                Rgb565Pixel(0x001f)
            );
            assert_eq!(
                frame[((origin / ONE) as usize + 58) * 960 + 610],
                Rgb565Pixel(0x001f)
            );
        }
    }

    #[test]
    fn reflection_uses_cropped_face_at_body_scale_and_projected_columns() {
        let face = Face::new(
            (0..188 * 268)
                .map(|i| Rgb565Pixel(if i / 188 % 2 == 0 { 0xf800 } else { 0x07e0 }))
                .collect(),
            188,
            268,
        );
        let mut scratch = Scratch::new();
        for angle in [0, 12000, -12000, 46000, -46000, 65536] {
            let pose = Pose {
                x: 514 * ONE,
                top: 150 * ONE,
                width: 188 * ONE,
                height: 268 * ONE,
                angle,
                clip: (296, 934),
                body_clip: (296, 934),
            };
            let mut body = vec![Rgb565Pixel(0); 960 * 540];
            let mut reflected = vec![Rgb565Pixel(0xffff); 960 * 540];
            draw(
                &mut body,
                &face,
                pose,
                &mut scratch,
                |p, _, _| p,
                false,
                None,
            );
            draw(
                &mut reflected,
                &face,
                pose,
                &mut scratch,
                |p, _, _| p,
                true,
                None,
            );
            let reference_texture = face.texture.reflection(64);
            for (x, c) in scratch.columns.iter().enumerate().filter(|(_, c)| c.valid) {
                let mut reference = [0; 64];
                reference_texture.prepare_column(c.filter, &mut reference);
                crate::launcher_texture::shade_rgba(
                    &mut reference,
                    diffuse_light(sin_cos(angle).1),
                );
                let end = (c.reflection_y
                    + (face.height / 4).min(64) as i64 * ONE * ONE / i64::from(c.step)
                    + ONE
                    - 1)
                    / ONE;
                for row in 0..(end - c.reflection_y / ONE) as usize {
                    let y = (c.reflection_y / ONE) as usize + row;
                    if y < 495 {
                        assert_eq!(reflected[y * 960 + x], {
                            let q = ((y as i64 * ONE + ONE / 2 - c.reflection_y)
                                * i64::from(c.step))
                            .div_euclid(ONE)
                                - ONE / 2;
                            let r = q.div_euclid(ONE);
                            let get = |i: i64| {
                                if (0..64).contains(&i) {
                                    crate::launcher_texture::over(
                                        reference[i as usize],
                                        Rgb565Pixel(0),
                                    )
                                    .0
                                } else {
                                    0
                                }
                            };
                            Rgb565Pixel(crate::launcher::mix_colour(
                                get(r),
                                get(r + 1),
                                (q.rem_euclid(ONE) / 256) as usize,
                            ))
                        });
                    }
                }
            }
        }
    }

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

    #[test]
    fn edge_on_card_retains_a_two_pixel_spine() {
        let face = Face::new(
            (0..180 * 252)
                .map(|index| Rgb565Pixel(if index % 180 <= 6 { 0xf800 } else { 0x001f }))
                .collect(),
            180,
            252,
        );
        for angle in [-ONE / 2, ONE / 2] {
            let pose = Pose {
                x: 500 * ONE,
                top: 140 * ONE,
                width: 180 * ONE,
                height: 252 * ONE,
                angle,
                clip: (296, 934),
                body_clip: (296, 934),
            };
            let mut pixels = vec![Rgb565Pixel(0); 960 * 540];
            draw(
                &mut pixels,
                &face,
                pose,
                &mut Scratch::new(),
                |pixel, _, _| pixel,
                false,
                None,
            );
            let visible = (500..680).filter(|&x| pixels[266 * 960 + x].0 != 0).count();
            assert_eq!(visible, 2, "edge-on angle {angle}");
            for pixel in pixels[266 * 960 + 500..266 * 960 + 680]
                .iter()
                .filter(|pixel| pixel.0 != 0)
            {
                assert!(pixel.0 >> 11 > pixel.0 & 31, "spine must use coloured rim");
            }
        }
    }

    #[test]
    fn conservative_occlusion_is_pixel_identical_to_full_overdraw() {
        let face = Face::new(
            (0..188 * 268)
                .map(|i| Rgb565Pixel((i as u16).wrapping_mul(977) | 0x0821))
                .collect(),
            188,
            268,
        );
        for foreground_angle in [-46000, -12000, 0, 12000, 46000] {
            for clip_left in (420..700).step_by(32) {
                let clip = (clip_left, (clip_left + STRIP_WIDTH).min(700));
                let background = Pose {
                    x: 500 * ONE,
                    top: 150 * ONE,
                    width: 188 * ONE,
                    height: 268 * ONE,
                    angle: -9000,
                    clip,
                    body_clip: clip,
                };
                let foreground = Pose {
                    x: 514 * ONE,
                    angle: foreground_angle,
                    ..background
                };
                let mut expected = vec![Rgb565Pixel(0x2104); 960 * 540];
                let mut background_scratch = Scratch::strip();
                let mut foreground_scratch = Scratch::strip();
                draw(
                    &mut expected,
                    &face,
                    background,
                    &mut background_scratch,
                    |p, _, _| p,
                    false,
                    None,
                );
                draw(
                    &mut expected,
                    &face,
                    foreground,
                    &mut foreground_scratch,
                    |p, _, _| p,
                    false,
                    None,
                );

                let mut actual = vec![Rgb565Pixel(0x2104); 960 * 540];
                let mut preparation = vec![Rgb565Pixel(0); 960 * 540];
                let mut background_scratch = Scratch::strip();
                let mut foreground_scratch = Scratch::strip();
                draw(
                    &mut preparation,
                    &face,
                    foreground,
                    &mut foreground_scratch,
                    |p, _, _| p,
                    true,
                    None,
                );
                let mut covered = BodyOcclusion::new(clip);
                add_opaque_coverage(&face, foreground, &foreground_scratch, &mut covered);
                draw_occluded(
                    &mut actual,
                    &face,
                    background,
                    &mut background_scratch,
                    |p, _, _| p,
                    None,
                    &covered,
                );
                draw(
                    &mut actual,
                    &face,
                    foreground,
                    &mut foreground_scratch,
                    |p, _, _| p,
                    false,
                    None,
                );
                assert_eq!(actual, expected, "angle {foreground_angle} clip {clip:?}");
            }
        }
    }
}
