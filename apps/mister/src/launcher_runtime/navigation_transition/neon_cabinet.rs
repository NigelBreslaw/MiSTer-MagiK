// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fixed-point, pseudo-3D runway treatment inspired by 1980s arcade attract modes.

use super::{
    NavigationTransitionBuffers, NavigationTransitionDirection, NavigationTransitionFailure,
    NavigationTransitionFrame, NavigationTransitionPhase, NavigationTransitionRect,
    NavigationTransitionRenderStats, NavigationTransitionRequest, PROGRESS_MAX, fill_rect_565,
    render_super_scaler_shell,
};
use slint::platform::software_renderer::Rgb565Pixel;

const FRAME_COUNT: usize = 25;
const ROW_COUNT: usize = 110;
const MAX_QUADS: usize = 12;
const MAX_VECTOR_SEGMENTS: usize = 96;
const MAX_SPANS: usize = 1_500;

#[derive(Clone, Copy, Debug, Default)]
struct ProjectedRow {
    y: u16,
    left: u16,
    right: u16,
}

#[derive(Debug, Default)]
pub(super) struct NeonCabinetRenderer {
    width: usize,
    height: usize,
    rows: Vec<ProjectedRow>,
}

impl NeonCabinetRenderer {
    pub(super) fn prepare(&mut self, width: usize, height: usize) {
        if self.width == width
            && self.height == height
            && self.rows.len() == FRAME_COUNT * ROW_COUNT
        {
            return;
        }
        self.width = width;
        self.height = height;
        self.rows.clear();
        if width == 0 || height == 0 {
            return;
        }
        self.rows.reserve(FRAME_COUNT * ROW_COUNT);
        let horizon = height * 7 / 25;
        let floor = height.saturating_sub(1);
        for frame in 0..FRAME_COUNT {
            let dive_q16 = frame as u32 * PROGRESS_MAX as u32 / (FRAME_COUNT - 1) as u32;
            for row in 0..ROW_COUNT {
                let depth_q16 = (row + 1) as u32 * PROGRESS_MAX as u32 / ROW_COUNT as u32;
                let perspective_q16 = depth_q16.saturating_mul(depth_q16) / PROGRESS_MAX as u32;
                let y =
                    horizon + (floor - horizon) * perspective_q16 as usize / PROGRESS_MAX as usize;
                let near_half = width * 23 / 50;
                let far_half = width / 28;
                let half = far_half
                    + (near_half - far_half) * perspective_q16 as usize / PROGRESS_MAX as usize;
                let dive_shift =
                    (width / 12) * dive_q16 as usize / PROGRESS_MAX as usize * (row % 3) / 2;
                let center = (width as isize / 2 + dive_shift as isize - width as isize / 24)
                    .clamp(0, width.saturating_sub(1) as isize)
                    as usize;
                self.rows.push(ProjectedRow {
                    y: y.min(u16::MAX as usize) as u16,
                    left: center.saturating_sub(half).min(u16::MAX as usize) as u16,
                    right: center
                        .saturating_add(half)
                        .min(width.saturating_sub(1))
                        .min(u16::MAX as usize) as u16,
                });
            }
        }
    }

    fn frame_rows(&self, progress_q16: u16) -> &[ProjectedRow] {
        let frame = progress_q16 as usize * (FRAME_COUNT - 1) / PROGRESS_MAX.max(1) as usize;
        let start = frame * ROW_COUNT;
        self.rows.get(start..start + ROW_COUNT).unwrap_or(&[])
    }
}

pub(super) fn render_neon_cabinet(
    renderer: &mut NeonCabinetRenderer,
    buffers: &mut NavigationTransitionBuffers,
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
) -> Result<NavigationTransitionRenderStats, NavigationTransitionFailure> {
    let mut stats = render_super_scaler_shell(buffers, request, frame)?;
    if frame.progress_q16 == 0 || frame.phase == NavigationTransitionPhase::Settled {
        return Ok(stats);
    }
    let width = buffers.width;
    let height = buffers.height;
    renderer.prepare(width, height);
    let working = buffers.working.as_mut_slice();
    let clip = overlay_rect(request, frame, width, height);
    if clip.width == 0 || clip.height == 0 {
        return Ok(stats);
    }
    let runway_progress = match request.direction {
        NavigationTransitionDirection::Reverse if frame.reveal_progress_q16 > 0 => {
            PROGRESS_MAX.saturating_sub(frame.reveal_progress_q16)
        }
        _ => frame.cover_progress_q16,
    };
    let fade = PROGRESS_MAX.saturating_sub(frame.reveal_progress_q16);
    let rows = renderer.frame_rows(runway_progress);
    let card_center =
        request.geometry.source_card.x as isize + request.geometry.source_card.width as isize / 2;
    let screen_center = width as isize / 2;
    let vanishing_x = card_center
        + (screen_center - card_center) * frame.cover_progress_q16 as isize / PROGRESS_MAX as isize;
    let mut spans = 0usize;
    for (index, table_row) in rows.iter().copied().enumerate() {
        if index % 2 != 0
            || spans >= MAX_SPANS
            || !overlay_visible(index as u32, frame.reveal_progress_q16)
        {
            continue;
        }
        let row = shift_row(table_row, vanishing_x - screen_center, index, width);
        let y = row.y as usize;
        let left = row.left as usize;
        let right = row.right as usize;
        if y < clip.y as usize
            || y >= clip.bottom() as usize
            || right <= clip.x as usize
            || left >= clip.right() as usize
        {
            continue;
        }
        let inset = (PROGRESS_MAX - fade) as usize * 12 / PROGRESS_MAX as usize;
        let clipped_left = left.max(clip.x as usize).saturating_add(inset);
        let clipped_right = right.min(clip.right() as usize).saturating_sub(inset);
        if clipped_left >= clipped_right {
            continue;
        }
        let line = NavigationTransitionRect {
            x: clipped_left as u16,
            y: y as u16,
            width: clipped_right.saturating_sub(clipped_left) as u16,
            height: 1,
        };
        fill_rect_565(
            working,
            width,
            height,
            line,
            if index % 8 == 0 {
                Rgb565Pixel(0xf81f)
            } else {
                Rgb565Pixel(0x043f)
            },
            &mut stats,
        );
        spans += 1;
    }

    let mut segments = 0usize;
    for lane in 0..8 {
        for step in 0..12 {
            if segments >= MAX_VECTOR_SEGMENTS {
                break;
            }
            if !overlay_visible((lane * 17 + step) as u32, frame.reveal_progress_q16) {
                continue;
            }
            let a_index = step * 9;
            let b_index = (step * 9 + 9).min(ROW_COUNT - 1);
            let a = shift_row(
                rows.get(a_index).copied().unwrap_or_default(),
                vanishing_x - screen_center,
                a_index,
                width,
            );
            let b = shift_row(
                rows.get(b_index).copied().unwrap_or(a),
                vanishing_x - screen_center,
                b_index,
                width,
            );
            let lane_q16 = (lane + 1) * PROGRESS_MAX as usize / 9;
            let ax = a.left as usize
                + (a.right.saturating_sub(a.left) as usize * lane_q16 / PROGRESS_MAX as usize);
            let bx = b.left as usize
                + (b.right.saturating_sub(b.left) as usize * lane_q16 / PROGRESS_MAX as usize);
            draw_short_vector(
                working,
                width,
                height,
                (ax, a.y as usize),
                (bx, b.y as usize),
                clip,
                Rgb565Pixel(if lane & 1 == 0 { 0x07ff } else { 0xb81f }),
                &mut stats,
            );
            segments += 1;
        }
    }

    let mut quads = 0usize;
    for cabinet in 0..6 {
        for side in 0..2 {
            if quads >= MAX_QUADS {
                break;
            }
            if !overlay_visible((cabinet * 31 + side * 7) as u32, frame.reveal_progress_q16) {
                continue;
            }
            let row_index = (18 + cabinet * 14).min(ROW_COUNT - 1);
            let row = shift_row(
                rows[row_index],
                vanishing_x - screen_center,
                row_index,
                width,
            );
            let size = 5 + cabinet * 3;
            let x = if side == 0 {
                row.left as usize
            } else {
                row.right as usize
            }
            .saturating_sub(if side == 0 { size / 2 } else { size });
            let quad_x = x
                .max(clip.x as usize)
                .min(clip.right().saturating_sub(1) as usize);
            let quad_y = (row.y as usize)
                .saturating_sub(size * 2)
                .max(clip.y as usize)
                .min(clip.bottom().saturating_sub(1) as usize);
            let quad = NavigationTransitionRect {
                x: quad_x as u16,
                y: quad_y as u16,
                width: size
                    .min((clip.right() as usize).saturating_sub(quad_x))
                    .max(1) as u16,
                height: (size * 2)
                    .min((clip.bottom() as usize).saturating_sub(quad_y))
                    .max(1) as u16,
            };
            fill_rect_565(
                working,
                width,
                height,
                quad,
                Rgb565Pixel(if side == 0 { 0x501f } else { 0x041f }),
                &mut stats,
            );
            super::draw_outline_565(
                working,
                width,
                height,
                quad,
                Rgb565Pixel(0x07ff),
                &mut stats,
            );
            quads += 1;
        }
    }
    stats.projected_rows = rows.len().min(ROW_COUNT) as u64;
    stats.vector_segments = segments as u64;
    stats.quads = quads as u64;
    stats.spans = spans as u64;
    Ok(stats)
}

fn overlay_rect(
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
    width: usize,
    height: usize,
) -> NavigationTransitionRect {
    let full = NavigationTransitionRect {
        x: 0,
        y: 0,
        width: width.min(u16::MAX as usize) as u16,
        height: height.min(u16::MAX as usize) as u16,
    };
    match request.direction {
        NavigationTransitionDirection::Forward if frame.reveal_progress_q16 == 0 => {
            super::lerp_rect(
                request.geometry.source_card,
                full,
                super::ease_out_cubic_q16(frame.cover_progress_q16),
            )
        }
        NavigationTransitionDirection::Reverse if frame.reveal_progress_q16 > 0 => {
            super::lerp_rect(
                full,
                request.geometry.source_card,
                super::ease_out_cubic_q16(frame.reveal_progress_q16),
            )
        }
        NavigationTransitionDirection::Reverse => {
            let cover = super::ease_out_cubic_q16(frame.cover_progress_q16);
            let covered_rows = height.saturating_mul(cover as usize) / PROGRESS_MAX as usize;
            NavigationTransitionRect {
                x: 0,
                y: height.saturating_sub(covered_rows).saturating_div(2) as u16,
                width: full.width,
                height: covered_rows as u16,
            }
        }
        NavigationTransitionDirection::Forward => full,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_short_vector(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    from: (usize, usize),
    to: (usize, usize),
    clip: NavigationTransitionRect,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let mut x = from.0 as isize;
    let mut y = from.1 as isize;
    let target_x = to.0 as isize;
    let target_y = to.1 as isize;
    let dx = (target_x - x).abs();
    let sx = if x < target_x { 1 } else { -1 };
    let dy = -(target_y - y).abs();
    let sy = if y < target_y { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        if x >= clip.x as isize
            && y >= clip.y as isize
            && x < clip.right() as isize
            && y < clip.bottom() as isize
            && x >= 0
            && y >= 0
            && (x as usize) < width
            && (y as usize) < height
        {
            destination[y as usize * width + x as usize] = color;
            stats.outline_pixels = stats.outline_pixels.saturating_add(1);
        }
        if x == target_x && y == target_y {
            break;
        }
        let doubled = error * 2;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
}

fn shift_row(
    row: ProjectedRow,
    vanishing_shift: isize,
    index: usize,
    width: usize,
) -> ProjectedRow {
    let shift = vanishing_shift * ROW_COUNT.saturating_sub(index) as isize / ROW_COUNT as isize;
    ProjectedRow {
        y: row.y,
        left: (row.left as isize + shift).clamp(0, width.saturating_sub(1) as isize) as u16,
        right: (row.right as isize + shift).clamp(0, width.saturating_sub(1) as isize) as u16,
    }
}

fn overlay_visible(seed: u32, reveal_q16: u16) -> bool {
    reveal_q16 < 63_000 && mix32(seed ^ 0x9e37_79b9) & 0xffff >= reveal_q16 as u32
}

fn mix32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_tables_stay_below_the_horizon_and_inside_frame() {
        let mut renderer = NeonCabinetRenderer::default();
        renderer.prepare(960, 540);
        assert_eq!(renderer.rows.len(), FRAME_COUNT * ROW_COUNT);
        for row in renderer.rows {
            assert!(row.y >= 540 * 7 / 25);
            assert!(row.y < 540);
            assert!(row.left < 960);
            assert!(row.right < 960);
            assert!(row.left <= row.right);
        }
    }

    #[test]
    fn declared_geometry_budgets_match_the_poc_contract() {
        assert_eq!(ROW_COUNT, 110);
        assert_eq!(MAX_QUADS, 12);
        assert_eq!(MAX_VECTOR_SEGMENTS, 96);
        assert_eq!(MAX_SPANS, 1_500);
    }
}
