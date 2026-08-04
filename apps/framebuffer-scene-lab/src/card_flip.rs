// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Small, app-local RGB565 door-hinge card flip.
//!
//! The two faces are built procedurally once. Rendering has no allocations and
//! deliberately provides a readable floating-point reference rasterizer and a
//! row-major fixed-point device rasterizer.

use mister_magik_particles::cabinet::Rgb565Pixel;
use std::time::Duration;

pub const WIDTH: usize = 960;
pub const HEIGHT: usize = 540;
pub const CARD_WIDTH: usize = 258;
pub const CARD_HEIGHT: usize = 378;
pub const CARD_X: usize = (WIDTH - CARD_WIDTH) / 2;
pub const CARD_Y: usize = (HEIGHT - CARD_HEIGHT) / 2;
pub const DEFAULT_DURATION: Duration = Duration::from_millis(440);

const CAMERA: i32 = 460;
const OUTLINE_WIDTH: usize = 2;
const MINIMUM_SPINE_WIDTH: usize = 12;
const FIXED_SHIFT: u32 = 16;
const FIXED_ONE: i64 = 1_i64 << FIXED_SHIFT;
const PROGRESS_MAX: u32 = u16::MAX as u32;

const BACKGROUND: Rgb565Pixel = rgb565(0x10, 0x14, 0x20);
const PANEL: Rgb565Pixel = rgb565(0x18, 0x18, 0x29);
const PANEL_ALT: Rgb565Pixel = rgb565(0x21, 0x20, 0x35);
const CYAN: Rgb565Pixel = rgb565(0x06, 0xd6, 0xa0);
const CYAN_BRIGHT: Rgb565Pixel = rgb565(0x40, 0xe5, 0xe7);
const PURPLE: Rgb565Pixel = rgb565(0x79, 0x70, 0xa8);
const PURPLE_DARK: Rgb565Pixel = rgb565(0x5a, 0x49, 0x73);
const TEXT: Rgb565Pixel = rgb565(0xff, 0xf6, 0xff);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Forward,
    Reverse,
}

impl Direction {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Reverse => "reverse",
        }
    }

    const fn endpoint(self) -> u16 {
        match self {
            Self::Forward => u16::MAX,
            Self::Reverse => 0,
        }
    }

    const fn start_endpoint(self) -> u16 {
        match self {
            Self::Forward => 0,
            Self::Reverse => u16::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RasterPath {
    Reference,
    #[default]
    Device,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderStats {
    pub active: bool,
    pub progress_q16: u16,
    pub direction: Direction,
    /// The scene requested this render because its state changed.
    pub dirty: bool,
    /// This frame differs from the most recently rendered frame.
    pub changed: bool,
    pub pixel_writes: usize,
}

#[derive(Clone, Copy, Default)]
struct Column {
    valid: bool,
    source_x: u16,
    source_y_zero_q16: i32,
    source_y_step_q16: i32,
    top_y: u16,
    bottom_y: u16,
}

pub struct CardFlip {
    duration: Duration,
    progress_q16: u16,
    start_progress_q16: u16,
    started_at: Duration,
    direction: Direction,
    active: bool,
    dirty: bool,
    rendered_progress: Option<u16>,
    raster_path: RasterPath,
    front: Vec<Rgb565Pixel>,
    back: Vec<Rgb565Pixel>,
    columns: [Column; WIDTH],
    device_initialized: bool,
}

impl Default for CardFlip {
    fn default() -> Self {
        Self::new(RasterPath::default())
    }
}

impl CardFlip {
    #[must_use]
    pub fn new(raster_path: RasterPath) -> Self {
        Self {
            duration: DEFAULT_DURATION,
            progress_q16: 0,
            start_progress_q16: 0,
            started_at: Duration::ZERO,
            direction: Direction::Forward,
            active: false,
            dirty: true,
            rendered_progress: None,
            raster_path,
            front: build_face(false),
            back: build_face(true),
            columns: [Column::default(); WIDTH],
            device_initialized: false,
        }
    }

    pub fn set_duration(&mut self, duration: Duration) {
        self.duration = duration.max(Duration::from_millis(1));
    }

    #[must_use]
    pub const fn progress_q16(&self) -> u16 {
        self.progress_q16
    }

    #[must_use]
    pub const fn direction(&self) -> Direction {
        self.direction
    }

    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty || self.active
    }

    pub fn play(&mut self, direction: Direction, now: Duration) {
        self.advance(now);
        self.direction = direction;
        self.start_progress_q16 = self.progress_q16;
        self.started_at = now;
        self.active = self.progress_q16 != direction.endpoint();
        self.dirty = true;
    }

    pub fn start_from_endpoint(&mut self, direction: Direction, now: Duration) {
        self.progress_q16 = direction.start_endpoint();
        self.start_progress_q16 = self.progress_q16;
        self.active = false;
        self.play(direction, now);
    }

    pub fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        now: Duration,
    ) -> Result<RenderStats, &'static str> {
        if destination.len() != WIDTH * HEIGHT {
            return Err("card flip requires an exact 960x540 RGB565 target");
        }
        self.advance(now);
        let dirty = self.dirty;
        let changed = self.rendered_progress != Some(self.progress_q16) || dirty;
        let progress = self.progress_q16;
        let writes = match self.raster_path {
            RasterPath::Reference => self.render_reference(destination, progress),
            RasterPath::Device => self.render_device(destination, progress),
        };
        self.rendered_progress = Some(progress);
        self.dirty = false;
        Ok(RenderStats {
            active: self.active,
            progress_q16: progress,
            direction: self.direction,
            dirty,
            changed,
            pixel_writes: writes,
        })
    }

    fn advance(&mut self, now: Duration) {
        if !self.active {
            return;
        }
        let elapsed = now.saturating_sub(self.started_at);
        let delta = elapsed
            .as_nanos()
            .saturating_mul(u128::from(PROGRESS_MAX))
            .checked_div(self.duration.as_nanos())
            .unwrap_or(u128::MAX)
            .min(u128::from(PROGRESS_MAX)) as u16;
        let next = match self.direction {
            Direction::Forward => self.start_progress_q16.saturating_add(delta),
            Direction::Reverse => self.start_progress_q16.saturating_sub(delta),
        };
        if next != self.progress_q16 {
            self.progress_q16 = next;
            self.dirty = true;
        }
        if self.progress_q16 == self.direction.endpoint() {
            self.active = false;
        }
    }

    /// Readable macOS/reference path: invert the projected plane with scalar
    /// floating-point math, once per destination column.
    fn render_reference(&mut self, destination: &mut [Rgb565Pixel], progress: u16) -> usize {
        destination.fill(BACKGROUND);
        self.columns.fill(Column::default());
        let phase = f32::from(progress) / PROGRESS_MAX as f32;
        let eased = smoothstep(phase);
        let angle = eased * std::f32::consts::PI;
        let sine = angle.sin();
        let cosine = angle.cos();
        let anchor = CARD_X as f32 + (CARD_WIDTH - 1) as f32 * eased;
        let center_y = CARD_Y as f32 + (CARD_HEIGHT - 1) as f32 * 0.5;
        let half_height = (CARD_HEIGHT - 1) as f32 * 0.5;

        for screen_x in 0..WIDTH {
            let offset = screen_x as f32 - anchor;
            let denominator = CAMERA as f32 * cosine - offset * sine;
            if denominator.abs() < 0.0001 {
                continue;
            }
            let local_from_hinge = offset * CAMERA as f32 / denominator;
            if !(-0.5..=(CARD_WIDTH as f32 - 0.5)).contains(&local_from_hinge) {
                continue;
            }
            let depth = CAMERA as f32 + local_from_hinge * sine;
            if depth <= 0.0 {
                continue;
            }
            let source_x = local_from_hinge.round().clamp(0.0, (CARD_WIDTH - 1) as f32) as u16;
            let step = depth / CAMERA as f32;
            let source_zero = -center_y * step + half_height;
            self.columns[screen_x] = column(
                source_x,
                (source_zero * FIXED_ONE as f32).round() as i32,
                (step * FIXED_ONE as f32).round() as i32,
            );
        }
        self.paint_rows_reference(destination, progress)
    }

    /// Device path: the pose is reduced to fixed point once; inversion and the
    /// row-major sampling loop contain integer arithmetic only.
    fn render_device(&mut self, destination: &mut [Rgb565Pixel], progress: u16) -> usize {
        if self.device_initialized {
            clear_card_bounds(destination);
        } else {
            destination.fill(BACKGROUND);
            self.device_initialized = true;
        }
        self.columns.fill(Column::default());
        let eased = smoothstep_q16(progress);
        let (sine_q16, cosine_q16) = sin_cos_pi_q16(eased);
        let anchor_q16 = (CARD_X as i64 * FIXED_ONE) + (CARD_WIDTH - 1) as i64 * i64::from(eased);
        let center_y_q16 = (CARD_Y as i64 * FIXED_ONE) + (CARD_HEIGHT - 1) as i64 * FIXED_ONE / 2;
        let half_height_q16 = (CARD_HEIGHT - 1) as i64 * FIXED_ONE / 2;
        let camera_q16 = i64::from(CAMERA) * FIXED_ONE;

        for screen_x in 0..WIDTH {
            let offset_q16 = screen_x as i64 * FIXED_ONE - anchor_q16;
            let denominator_q16 =
                i64::from(CAMERA) * cosine_q16 - ((offset_q16 * sine_q16) >> FIXED_SHIFT);
            if denominator_q16.abs() < 4 {
                continue;
            }
            let local_from_hinge_q16 = offset_q16 * camera_q16 / denominator_q16;
            if local_from_hinge_q16 < -FIXED_ONE / 2
                || local_from_hinge_q16 > CARD_WIDTH as i64 * FIXED_ONE - FIXED_ONE / 2
            {
                continue;
            }
            let depth_q16 = camera_q16 + ((local_from_hinge_q16 * sine_q16) >> FIXED_SHIFT);
            if depth_q16 <= 0 {
                continue;
            }
            let source_x = ((local_from_hinge_q16 + FIXED_ONE / 2) >> FIXED_SHIFT)
                .clamp(0, (CARD_WIDTH - 1) as i64) as u16;
            let source_y_step_q16 = depth_q16 / i64::from(CAMERA);
            let source_y_zero_q16 =
                half_height_q16 - ((center_y_q16 * source_y_step_q16) >> FIXED_SHIFT);
            self.columns[screen_x] =
                column(source_x, source_y_zero_q16 as i32, source_y_step_q16 as i32);
        }
        self.paint_rows_device(destination, progress)
    }

    fn paint_rows_reference(&self, destination: &mut [Rgb565Pixel], progress: u16) -> usize {
        let outline = if progress < u16::MAX / 2 {
            CYAN_BRIGHT
        } else {
            PURPLE
        };
        let valid_width = self.columns.iter().filter(|column| column.valid).count();
        if valid_width < MINIMUM_SPINE_WIDTH {
            return render_spine(destination, outline);
        }
        let face = if progress < u16::MAX / 2 {
            &self.front
        } else {
            &self.back
        };
        let mirror = progress >= u16::MAX / 2;
        let Some(first) = self.columns.iter().position(|column| column.valid) else {
            return 0;
        };
        let last = self
            .columns
            .iter()
            .rposition(|column| column.valid)
            .unwrap_or(first);
        let mut writes = 0;

        // This is intentionally row-major for contiguous RGB565 destination writes.
        for y in CARD_Y..CARD_Y + CARD_HEIGHT {
            let Some(left) = (first..=last).find(|&x| column_contains(self.columns[x], y)) else {
                continue;
            };
            let right = (left..=last)
                .rev()
                .find(|&x| column_contains(self.columns[x], y))
                .unwrap_or(left);
            for x in left..=right {
                let column = self.columns[x];
                let Some(source_y) = source_y_at(column, y) else {
                    continue;
                };
                if !stepped_corner(column.source_x as usize, source_y) {
                    continue;
                }
                let top_edge = y < usize::from(column.top_y) + OUTLINE_WIDTH;
                let bottom_edge = y + OUTLINE_WIDTH > usize::from(column.bottom_y);
                let border = x < left + OUTLINE_WIDTH
                    || x + OUTLINE_WIDTH > right
                    || top_edge
                    || bottom_edge;
                let index = y * WIDTH + x;
                if border {
                    destination[index] = outline;
                } else {
                    let source_x = if mirror {
                        CARD_WIDTH - 1 - column.source_x as usize
                    } else {
                        column.source_x as usize
                    };
                    destination[index] = face[source_y * CARD_WIDTH + source_x];
                }
                writes += 1;
            }
        }
        writes
    }

    /// MiSTer-only hot path. Geometry and buffer lengths are fixed and checked
    /// by `render`, allowing raw contiguous reads/writes without repeated slice
    /// bounds checks in the inner loop.
    #[cfg_attr(target_arch = "arm", inline(never))]
    fn paint_rows_device(&self, destination: &mut [Rgb565Pixel], progress: u16) -> usize {
        let outline = if progress < u16::MAX / 2 {
            CYAN_BRIGHT
        } else {
            PURPLE
        };
        let columns = self.columns.as_ptr();
        let mut first = 0;
        // SAFETY: `columns` points to the fixed WIDTH-element array owned by
        // `self`, and both searches are bounded by WIDTH.
        while first < WIDTH && unsafe { !(*columns.add(first)).valid } {
            first += 1;
        }
        if first == WIDTH {
            return 0;
        }
        let mut last = WIDTH - 1;
        while last > first && unsafe { !(*columns.add(last)).valid } {
            last -= 1;
        }
        if last - first + 1 < MINIMUM_SPINE_WIDTH {
            return render_spine(destination, outline);
        }
        let face = if progress < u16::MAX / 2 {
            self.front.as_ptr()
        } else {
            self.back.as_ptr()
        };
        let mirror = progress >= u16::MAX / 2;
        let destination = destination.as_mut_ptr();
        let mut writes = 0;

        for y in CARD_Y..CARD_Y + CARD_HEIGHT {
            let mut left = first;
            while left <= last && !column_contains_device(unsafe { *columns.add(left) }, y) {
                left += 1;
            }
            if left > last {
                continue;
            }
            let mut right = last;
            while right > left && !column_contains_device(unsafe { *columns.add(right) }, y) {
                right -= 1;
            }

            let mut x = left;
            while x <= right {
                // SAFETY: left/right are derived from first/last, which are
                // bounded indices into the fixed columns array.
                let column = unsafe { *columns.add(x) };
                // Card-space fixed-point values stay well inside i32 for the
                // fixed 960x540 scene, avoiding 64-bit arithmetic on ARMv7.
                let value = column
                    .source_y_step_q16
                    .wrapping_mul(y as i32)
                    .wrapping_add(column.source_y_zero_q16);
                let source_y = ((value + (1 << 15)) >> 16) as usize;
                let border = x < left + OUTLINE_WIDTH
                    || x + OUTLINE_WIDTH > right
                    || y < usize::from(column.top_y) + OUTLINE_WIDTH
                    || y + OUTLINE_WIDTH > usize::from(column.bottom_y);
                let pixel = if border {
                    outline
                } else {
                    let source_x = if mirror {
                        CARD_WIDTH - 1 - column.source_x as usize
                    } else {
                        column.source_x as usize
                    };
                    // SAFETY: source_x/source_y are derived from validated
                    // card columns and rows and remain inside the fixed face.
                    unsafe { *face.add(source_y * CARD_WIDTH + source_x) }
                };
                // SAFETY: render validated the fixed 960x540 target and the
                // loop bounds stay inside that plane.
                unsafe { *destination.add(y * WIDTH + x) = pixel };
                writes += 1;
                x += 1;
            }
        }
        writes
    }
}

fn column(source_x: u16, source_y_zero_q16: i32, source_y_step_q16: i32) -> Column {
    let step = i64::from(source_y_step_q16).max(1);
    let zero = i64::from(source_y_zero_q16);
    let first_value = -FIXED_ONE / 2;
    let last_value = CARD_HEIGHT as i64 * FIXED_ONE - FIXED_ONE / 2 - 1;
    let top_y = div_ceil_positive(first_value - zero, step).clamp(0, (HEIGHT - 1) as i64) as u16;
    let bottom_y = (last_value - zero)
        .div_euclid(step)
        .clamp(0, (HEIGHT - 1) as i64) as u16;
    Column {
        valid: top_y <= bottom_y,
        source_x,
        source_y_zero_q16,
        source_y_step_q16,
        top_y,
        bottom_y,
    }
}

fn div_ceil_positive(value: i64, divisor: i64) -> i64 {
    let quotient = value.div_euclid(divisor);
    quotient + i64::from(value.rem_euclid(divisor) != 0)
}

fn column_contains(column: Column, y: usize) -> bool {
    if !column.valid || y < usize::from(column.top_y) || y > usize::from(column.bottom_y) {
        return false;
    }
    source_y_at(column, y)
        .is_some_and(|source_y| stepped_corner(column.source_x as usize, source_y))
}

#[inline(always)]
fn column_contains_device(column: Column, y: usize) -> bool {
    if !column.valid || y < usize::from(column.top_y) || y > usize::from(column.bottom_y) {
        return false;
    }
    let value = column
        .source_y_step_q16
        .wrapping_mul(y as i32)
        .wrapping_add(column.source_y_zero_q16);
    let source_y = ((value.wrapping_add(1 << 15)) >> 16) as usize;
    source_y < CARD_HEIGHT && stepped_corner(column.source_x as usize, source_y)
}

fn clear_card_bounds(destination: &mut [Rgb565Pixel]) {
    #[cfg(target_arch = "arm")]
    {
        crate::card_flip_neon::fill_rect_rgb565(
            destination,
            WIDTH,
            CARD_X,
            CARD_Y,
            CARD_WIDTH,
            CARD_HEIGHT,
            BACKGROUND,
        );
        return;
    }
    #[cfg(not(target_arch = "arm"))]
    for y in CARD_Y..CARD_Y + CARD_HEIGHT {
        let start = y * WIDTH + CARD_X;
        destination[start..start + CARD_WIDTH].fill(BACKGROUND);
    }
}

#[inline(always)]
fn source_y_at(column: Column, y: usize) -> Option<usize> {
    if !column.valid {
        return None;
    }
    let value =
        i64::from(column.source_y_zero_q16) + y as i64 * i64::from(column.source_y_step_q16);
    let rounded = (value + FIXED_ONE / 2) >> FIXED_SHIFT;
    (0..CARD_HEIGHT as i64)
        .contains(&rounded)
        .then_some(rounded as usize)
}

fn render_spine(destination: &mut [Rgb565Pixel], outline: Rgb565Pixel) -> usize {
    let x0 = WIDTH / 2 - MINIMUM_SPINE_WIDTH / 2;
    let mut writes = 0;
    for y in CARD_Y..CARD_Y + CARD_HEIGHT {
        let inset = corner_inset(y - CARD_Y);
        for x in x0 + inset.min(MINIMUM_SPINE_WIDTH / 2)
            ..x0 + MINIMUM_SPINE_WIDTH - inset.min(MINIMUM_SPINE_WIDTH / 2)
        {
            let edge = x < x0 + OUTLINE_WIDTH
                || x >= x0 + MINIMUM_SPINE_WIDTH - OUTLINE_WIDTH
                || y < CARD_Y + OUTLINE_WIDTH
                || y >= CARD_Y + CARD_HEIGHT - OUTLINE_WIDTH;
            destination[y * WIDTH + x] = if edge { outline } else { PURPLE_DARK };
            writes += 1;
        }
    }
    writes
}

#[inline(always)]
fn stepped_corner(x: usize, y: usize) -> bool {
    let edge_y = y.min(CARD_HEIGHT - 1 - y);
    let inset = corner_inset(edge_y);
    x >= inset && x < CARD_WIDTH - inset
}

#[inline(always)]
const fn corner_inset(edge_y: usize) -> usize {
    match edge_y {
        0 => 6,
        1 => 4,
        2 => 3,
        3 => 2,
        4 => 1,
        _ => 0,
    }
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn smoothstep_q16(value: u16) -> u16 {
    let x = u64::from(value);
    let max = u64::from(u16::MAX);
    let x2 = (x * x + max / 2) / max;
    let eased = (x2 * (3 * max - 2 * x) + max / 2) / max;
    eased.min(max) as u16
}

/// Bhaskara's sine approximation in normalized Q16 form. It is exact at the
/// endpoints and midpoint and keeps the device pose free of floating point.
fn sin_pi_q16(value: u16) -> i64 {
    let max = i64::from(u16::MAX);
    let value = i64::from(value);
    let product = value * (max - value) / max;
    16 * product * max / (5 * max - 4 * product)
}

fn sin_cos_pi_q16(value: u16) -> (i64, i64) {
    let max = u16::MAX;
    let half = max / 2;
    let sine = sin_pi_q16(value);
    let distance = value.abs_diff(half);
    let cosine_magnitude = sin_pi_q16(distance);
    let cosine = if value <= half {
        cosine_magnitude
    } else {
        -cosine_magnitude
    };
    (sine, cosine)
}

fn build_face(back: bool) -> Vec<Rgb565Pixel> {
    let mut pixels = vec![PANEL; CARD_WIDTH * CARD_HEIGHT];
    if back {
        fill_rect(
            &mut pixels,
            32,
            35,
            CARD_WIDTH - 64,
            CARD_HEIGHT - 70,
            PURPLE_DARK,
        );
        fill_rect(
            &mut pixels,
            38,
            41,
            CARD_WIDTH - 76,
            CARD_HEIGHT - 82,
            PANEL_ALT,
        );
        draw_text_centered(&mut pixels, "MAGIK", 4, CYAN_BRIGHT, 165);
        draw_text_centered(&mut pixels, "CARD FLIP", 2, PURPLE, 213);
    } else {
        fill_rect(&mut pixels, 34, 40, CARD_WIDTH - 68, 32, CYAN);
        fill_rect(&mut pixels, 48, 92, CARD_WIDTH - 96, 120, PURPLE_DARK);
        fill_rect(&mut pixels, 56, 100, CARD_WIDTH - 112, 74, PANEL_ALT);
        fill_rect(&mut pixels, 72, 188, 16, 30, CYAN);
        fill_rect(&mut pixels, 90, 206, 78, 8, CYAN);
        fill_rect(&mut pixels, 78, 242, 102, 48, PURPLE_DARK);
        fill_rect(&mut pixels, 92, 253, 74, 25, PANEL_ALT);
        draw_text_centered(&mut pixels, "ARCADE", 3, PANEL, 46);
        draw_text_centered(&mut pixels, "ARCADE", 4, CYAN_BRIGHT, 304);
        draw_text_centered(&mut pixels, "1752 GAMES", 2, TEXT, 344);
    }
    pixels
}

fn fill_rect(
    pixels: &mut [Rgb565Pixel],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: Rgb565Pixel,
) {
    for row in y..(y + height).min(CARD_HEIGHT) {
        let start = row * CARD_WIDTH + x.min(CARD_WIDTH);
        let end = row * CARD_WIDTH + (x + width).min(CARD_WIDTH);
        pixels[start..end].fill(color);
    }
}

fn draw_text_centered(
    pixels: &mut [Rgb565Pixel],
    text: &str,
    scale: usize,
    color: Rgb565Pixel,
    y: usize,
) {
    let width = text.chars().count() * 6 * scale - scale;
    let x = CARD_WIDTH.saturating_sub(width) / 2;
    for (index, character) in text.chars().enumerate() {
        let glyph = glyph5x7(character);
        for (gy, bits) in glyph.iter().copied().enumerate() {
            for gx in 0..5 {
                if bits & (1 << (4 - gx)) == 0 {
                    continue;
                }
                fill_rect(
                    pixels,
                    x + (index * 6 + gx) * scale,
                    y + gy * scale,
                    scale,
                    scale,
                    color,
                );
            }
        }
    }
}

#[rustfmt::skip]
const fn glyph5x7(character: char) -> [u8; 7] {
    match character {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'C' => [0x0f, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0f],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0f, 0x10, 0x10, 0x13, 0x11, 0x11, 0x0f],
        'I' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1f],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '5' => [0x1f, 0x10, 0x10, 0x1e, 0x01, 0x01, 0x1e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        ' ' => [0; 7],
        _ => [0x1f, 0x11, 0x02, 0x04, 0x04, 0x00, 0x04],
    }
}

const fn rgb565(red: u8, green: u8, blue: u8) -> Rgb565Pixel {
    Rgb565Pixel(((red as u16 >> 3) << 11) | ((green as u16 >> 2) << 5) | (blue as u16 >> 3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(scene: &mut CardFlip, at: Duration) -> Vec<Rgb565Pixel> {
        let mut pixels = vec![Rgb565Pixel(0); WIDTH * HEIGHT];
        scene.render(&mut pixels, at).unwrap();
        pixels
    }

    fn non_background_bounds(pixels: &[Rgb565Pixel]) -> (usize, usize, usize, usize) {
        let mut min_x = WIDTH;
        let mut min_y = HEIGHT;
        let mut max_x = 0;
        let mut max_y = 0;
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                if pixels[y * WIDTH + x] != BACKGROUND {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        (min_x, min_y, max_x, max_y)
    }

    #[test]
    fn endpoints_use_exact_centered_geometry() {
        for path in [RasterPath::Reference, RasterPath::Device] {
            let mut scene = CardFlip::new(path);
            assert_eq!(
                non_background_bounds(&frame(&mut scene, Duration::ZERO)),
                (
                    CARD_X,
                    CARD_Y,
                    CARD_X + CARD_WIDTH - 1,
                    CARD_Y + CARD_HEIGHT - 1
                )
            );
            scene.start_from_endpoint(Direction::Reverse, Duration::ZERO);
            assert_eq!(
                non_background_bounds(&frame(&mut scene, Duration::ZERO)),
                (
                    CARD_X,
                    CARD_Y,
                    CARD_X + CARD_WIDTH - 1,
                    CARD_Y + CARD_HEIGHT - 1
                )
            );
        }
    }

    #[test]
    fn endpoint_border_is_two_pixels_with_six_pixel_steps() {
        let mut scene = CardFlip::new(RasterPath::Device);
        let pixels = frame(&mut scene, Duration::ZERO);
        assert_eq!(pixels[CARD_Y * WIDTH + CARD_X], BACKGROUND);
        assert_eq!(pixels[CARD_Y * WIDTH + CARD_X + 6], CYAN_BRIGHT);
        assert_eq!(pixels[(CARD_Y + 2) * WIDTH + CARD_X + 3], CYAN_BRIGHT);
        assert_eq!(pixels[(CARD_Y + 5) * WIDTH + CARD_X], CYAN_BRIGHT);
        assert_ne!(pixels[(CARD_Y + 6) * WIDTH + CARD_X + 2], CYAN_BRIGHT);
    }

    #[test]
    fn midpoint_has_a_twelve_pixel_spine() {
        for path in [RasterPath::Reference, RasterPath::Device] {
            let mut scene = CardFlip::new(path);
            scene.start_from_endpoint(Direction::Forward, Duration::ZERO);
            let pixels = frame(&mut scene, DEFAULT_DURATION / 2);
            let bounds = non_background_bounds(&pixels);
            assert_eq!(bounds.2 - bounds.0 + 1, MINIMUM_SPINE_WIDTH);
            assert_eq!(bounds.3 - bounds.1 + 1, CARD_HEIGHT);
        }
    }

    #[test]
    fn reversal_continues_from_the_current_progress() {
        let mut scene = CardFlip::default();
        scene.play(Direction::Forward, Duration::ZERO);
        let forward = scene
            .render(
                &mut vec![Rgb565Pixel(0); WIDTH * HEIGHT],
                Duration::from_millis(220),
            )
            .unwrap()
            .progress_q16;
        scene.play(Direction::Reverse, Duration::from_millis(220));
        let same = scene
            .render(
                &mut vec![Rgb565Pixel(0); WIDTH * HEIGHT],
                Duration::from_millis(220),
            )
            .unwrap()
            .progress_q16;
        let reverse = scene
            .render(
                &mut vec![Rgb565Pixel(0); WIDTH * HEIGHT],
                Duration::from_millis(330),
            )
            .unwrap()
            .progress_q16;
        assert_eq!(same, forward);
        assert!(reverse < same);
    }

    #[test]
    fn reference_and_device_paths_match_major_checkpoints() {
        for milliseconds in [0, 110, 220, 330, 440] {
            let mut reference = CardFlip::new(RasterPath::Reference);
            let mut device = CardFlip::new(RasterPath::Device);
            reference.start_from_endpoint(Direction::Forward, Duration::ZERO);
            device.start_from_endpoint(Direction::Forward, Duration::ZERO);
            let reference = frame(&mut reference, Duration::from_millis(milliseconds));
            let device = frame(&mut device, Duration::from_millis(milliseconds));
            let mismatches = reference
                .iter()
                .zip(&device)
                .filter(|(a, b)| a != b)
                .count();
            assert!(
                mismatches <= 1_500,
                "checkpoint {milliseconds}ms mismatched {mismatches} pixels"
            );
        }
    }

    #[test]
    fn render_state_reports_dirty_changed_and_active() {
        let mut scene = CardFlip::default();
        let mut pixels = vec![Rgb565Pixel(0); WIDTH * HEIGHT];
        let first = scene.render(&mut pixels, Duration::ZERO).unwrap();
        let stable = scene.render(&mut pixels, Duration::ZERO).unwrap();
        scene.play(Direction::Forward, Duration::ZERO);
        let moving = scene.render(&mut pixels, Duration::from_millis(1)).unwrap();
        assert!(first.dirty && first.changed);
        assert!(!stable.dirty && !stable.changed);
        assert!(moving.dirty && moving.changed && moving.active);
    }
}
