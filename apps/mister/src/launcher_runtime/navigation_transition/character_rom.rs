// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Character-ROM snapshot recompiler with deterministic exact-pixel verification.

use super::{
    NavigationTransitionBuffers, NavigationTransitionDirection, NavigationTransitionEdge,
    NavigationTransitionFailure, NavigationTransitionFrame, NavigationTransitionGeometry,
    NavigationTransitionPhase, NavigationTransitionRect, NavigationTransitionRenderStats,
    NavigationTransitionRequest, PROGRESS_MAX, clip_rect_to_frame, draw_outline_565,
    ease_out_cubic_q16, fill_rect_565, lerp_rect, smoothstep_q16,
};
use slint::platform::software_renderer::Rgb565Pixel;

const FULL_COLUMNS: usize = 30;
const FULL_ROWS: usize = 17;
const FALLBACK_COLUMNS: usize = 20;
const FALLBACK_ROWS: usize = 12;
const FRAME_COUNT: usize = 26;
const MAX_NEW_FLIPS_PER_FRAME: usize = 96;
const DIAGONAL_BAND_Q16: i32 = 5_200;
const DIAGONAL_Y_WEIGHT_Q16: i32 = 42_598;
const DIAGONAL_HERO_BIAS_Q16: i32 = 2_950;
const EXACT_DECAY_Q16: i32 = 600;
const ENDPOINT_EPSILON_Q16: u16 = 1;
const PALETTE: [Rgb565Pixel; 16] = [
    Rgb565Pixel(0x0022), // ink
    Rgb565Pixel(0x0846), // navy
    Rgb565Pixel(0x18ca), // blue-black
    Rgb565Pixel(0x294b), // deep violet
    Rgb565Pixel(0x49ef), // muted purple
    Rgb565Pixel(0x6a92), // lavender
    Rgb565Pixel(0x8b55), // light lavender
    Rgb565Pixel(0xbdf7), // pale phosphor
    Rgb565Pixel(0x016b), // deep teal
    Rgb565Pixel(0x02d0), // teal
    Rgb565Pixel(0x0473), // bright teal
    Rgb565Pixel(0x05f6), // cyan
    Rgb565Pixel(0x77fb), // mint
    Rgb565Pixel(0xfca0), // amber
    Rgb565Pixel(0xfef7), // cream
    Rgb565Pixel(0xffff), // verification white
];
const CHARACTER_ROM: [[u8; 8]; 128] = build_character_rom();

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CharacterCell {
    glyph: u8,
    foreground: u8,
    background: u8,
}

#[derive(Debug)]
pub(super) struct CharacterRomRenderer {
    columns: usize,
    rows: usize,
    width: usize,
    height: usize,
    destination_cells: Vec<CharacterCell>,
}

impl CharacterRomRenderer {
    pub(super) fn new(fallback: bool) -> Self {
        let (columns, rows) = if fallback {
            (FALLBACK_COLUMNS, FALLBACK_ROWS)
        } else {
            (FULL_COLUMNS, FULL_ROWS)
        };
        Self {
            columns,
            rows,
            width: 0,
            height: 0,
            destination_cells: Vec::new(),
        }
    }

    pub(super) fn prepare(
        &mut self,
        _source: &[Rgb565Pixel],
        destination: &[Rgb565Pixel],
        width: usize,
        height: usize,
    ) {
        self.width = width;
        self.height = height;
        quantize_snapshot(
            destination,
            width,
            height,
            self.columns,
            self.rows,
            &mut self.destination_cells,
        );
    }

    fn ready(&self, width: usize, height: usize) -> bool {
        let count = self.columns.saturating_mul(self.rows);
        self.width == width && self.height == height && self.destination_cells.len() == count
    }

    #[cfg(test)]
    fn cell_count(&self) -> usize {
        self.columns * self.rows
    }
}

pub(super) fn configured_fallback() -> bool {
    super::env_flag("MISTER_NAV_TRANSITION_REDUCED_EFFECTS")
        || std::env::var("MISTER_NAV_TRANSITION_CHARACTER_GRID")
            .ok()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("20x12"))
}

pub(super) fn render_character_rom(
    renderer: &mut CharacterRomRenderer,
    buffers: &mut NavigationTransitionBuffers,
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
) -> Result<NavigationTransitionRenderStats, NavigationTransitionFailure> {
    let width = buffers.width;
    let height = buffers.height;
    let raw_source = buffers
        .source
        .get(..)
        .filter(|_| buffers.source_ready)
        .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
    let raw_destination = buffers
        .destination
        .get(..)
        .filter(|_| buffers.destination_ready);
    if raw_source.len() != width.saturating_mul(height) || buffers.working.len() != raw_source.len()
    {
        return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
    }

    let mut stats = NavigationTransitionRenderStats::default();
    if frame.phase == NavigationTransitionPhase::Settled {
        let endpoint = match frame.endpoint {
            Some(super::NavigationTransitionEndpoint::Destination) => {
                raw_destination.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?
            }
            _ => raw_source,
        };
        buffers.working.copy_from_slice(endpoint);
        stats.copied_pixels = endpoint.len() as u64;
        return Ok(stats);
    }
    if frame.progress_q16 == 0 {
        buffers.working.copy_from_slice(raw_source);
        stats.copied_pixels = raw_source.len() as u64;
        return Ok(stats);
    }

    let canonical = canonical_progress(request.direction, frame.progress_q16);
    if canonical == super::COVER_PROGRESS {
        render_procedural_covered(
            buffers.working.as_mut_slice(),
            width,
            height,
            request.geometry,
            request.edge,
            renderer.columns,
            renderer.rows,
            &mut stats,
        );
        return Ok(stats);
    }
    let source = match request.direction {
        NavigationTransitionDirection::Forward => Some(raw_source),
        NavigationTransitionDirection::Reverse => raw_destination,
    };
    if canonical <= ENDPOINT_EPSILON_Q16 {
        let source = source.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        buffers.working.copy_from_slice(source);
        stats.copied_pixels = source.len() as u64;
        return Ok(stats);
    }

    let reveal = diagonal_reveal_progress(canonical);
    if reveal == 0 {
        let source = source.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        let working = buffers.working.as_mut_slice();
        working.copy_from_slice(source);
        stats.copied_pixels = source.len() as u64;
        render_rom_cover(
            working,
            source,
            width,
            height,
            request.geometry,
            request.edge,
            renderer.columns,
            renderer.rows,
            scale_segment(canonical, super::COVER_PROGRESS),
            &mut stats,
        );
        let cover_progress = scale_segment(canonical, super::COVER_PROGRESS);
        if cover_progress < 45_000 && request.geometry.source_detail.fits(width, height) {
            super::move_label_pixels(
                working,
                source,
                width,
                height,
                request.geometry.source_detail,
                request.geometry.destination_detail,
                smoothstep_q16(cover_progress),
                false,
                &mut stats,
            );
        }
        return Ok(stats);
    }

    let destination = match request.direction {
        NavigationTransitionDirection::Forward => {
            raw_destination.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?
        }
        NavigationTransitionDirection::Reverse => raw_source,
    };
    if !renderer.ready(width, height) {
        renderer.prepare(raw_source, destination, width, height);
    }
    if canonical == PROGRESS_MAX {
        buffers.working.copy_from_slice(destination);
        stats.copied_pixels = destination.len() as u64;
        return Ok(stats);
    }

    let front = diagonal_front(reveal);
    let requested_flips = (0..renderer.columns.saturating_mul(renderer.rows))
        .filter(|index| {
            let row = *index / renderer.columns;
            let column = *index % renderer.columns;
            diagonal_metric(column, row, renderer.columns, renderer.rows) as i32 <= front
        })
        .count();
    let frame_index = usize::from(canonical) * (FRAME_COUNT - 1) / usize::from(PROGRESS_MAX.max(1));
    let new_flips = flips_for_frame(frame_index, renderer.columns, renderer.rows);
    debug_assert!(new_flips <= MAX_NEW_FLIPS_PER_FRAME);
    let destination_cells = &renderer.destination_cells;
    let working = buffers.working.as_mut_slice();
    render_diagonal_recompile(
        working,
        destination,
        width,
        height,
        renderer.columns,
        renderer.rows,
        request.geometry,
        request.edge,
        destination_cells,
        front,
        reveal,
        &mut stats,
    );
    stats.cell_flips = requested_flips as u64;
    stats.new_cell_flips = new_flips as u64;
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
fn render_diagonal_recompile(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
    geometry: NavigationTransitionGeometry,
    edge: NavigationTransitionEdge,
    destination_cells: &[CharacterCell],
    front: i32,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if width == 0
        || height == 0
        || working.len() != destination.len()
        || destination_cells.len() != columns.saturating_mul(rows)
    {
        return;
    }

    working.fill(PALETTE[0]);
    stats.filled_pixels = stats.filled_pixels.saturating_add(working.len() as u64);
    let full_stage = NavigationTransitionRect {
        x: 8.min(width) as u16,
        y: 8.min(height) as u16,
        width: width.saturating_sub(16.min(width)).min(u16::MAX as usize) as u16,
        height: height.saturating_sub(16.min(height)).min(u16::MAX as usize) as u16,
    };
    draw_link_field(
        working,
        width,
        height,
        full_stage,
        columns,
        rows,
        PROGRESS_MAX,
        reveal_q16 < 8_000,
        stats,
    );
    draw_rom_landmarks(working, width, height, edge, stats);
    let scaffold_mix = smoothstep_q16(
        ((u32::from(reveal_q16) * u32::from(PROGRESS_MAX) / 8_000).min(u32::from(PROGRESS_MAX)))
            as u16,
    );
    draw_destination_scaffold(
        working,
        width,
        height,
        destination_cells,
        columns,
        rows,
        front,
        reveal_q16,
        scaffold_mix,
        stats,
    );
    let exact_front = front.saturating_sub(EXACT_DECAY_Q16);
    for row in 0..rows {
        for column in 0..columns {
            let metric = diagonal_metric(column, row, columns, rows) as i32;
            if metric.abs_diff(front) > DIAGONAL_BAND_Q16 as u32 {
                continue;
            }
            let index = row * columns + column;
            let x0 = column * width / columns;
            let x1 = (column + 1) * width / columns;
            let y0 = row * height / rows;
            let y1 = (row + 1) * height / rows;
            let local = ((front
                .saturating_sub(metric)
                .saturating_add(DIAGONAL_BAND_Q16) as i64
                * PROGRESS_MAX as i64)
                / (DIAGONAL_BAND_Q16 as i64 * 2))
                .clamp(0, PROGRESS_MAX as i64) as u16;
            draw_recompile_cell(
                working,
                width,
                height,
                (x0, y0, x1, y1),
                destination_cells[index],
                index,
                local,
                reveal_q16,
                stats,
            );
        }
    }

    draw_rom_frame(working, width, height, reveal_q16, front, stats);
    fill_rect_565(
        working,
        width,
        height,
        geometry.destination_title,
        PALETTE[0],
        stats,
    );
    draw_rom_label(
        working,
        width,
        height,
        geometry.destination_title,
        geometry,
        PALETTE[11],
        stats,
    );

    let mut fully_verified_rows = 0usize;
    for y in 0..height {
        let y_q16 = (((y.saturating_mul(2).saturating_add(1)) as u64 * PROGRESS_MAX as u64)
            / (height.saturating_mul(2).max(1) as u64)) as i32;
        let x_q16 = diagonal_x_q16(exact_front, y_q16);
        let verified_x = if x_q16 <= 0 {
            0
        } else if x_q16 >= PROGRESS_MAX as i32 {
            width
        } else {
            width.saturating_mul(x_q16 as usize) / PROGRESS_MAX as usize
        };
        if verified_x > 0 {
            let start = y * width;
            working[start..start + verified_x]
                .copy_from_slice(&destination[start..start + verified_x]);
            stats.copied_pixels = stats.copied_pixels.saturating_add(verified_x as u64);
        }
        if verified_x == width {
            fully_verified_rows += 1;
        }
    }
    stats.verified_rows = fully_verified_rows as u64;

    draw_diagonal_verification_beam(working, width, height, exact_front, reveal_q16, stats);
}

#[allow(clippy::too_many_arguments)]
fn draw_destination_scaffold(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    cells: &[CharacterCell],
    columns: usize,
    rows: usize,
    front: i32,
    reveal_q16: u16,
    mix_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if cells.len() != columns.saturating_mul(rows) || width == 0 || height == 0 {
        return;
    }
    for row in 0..rows {
        for column in 0..columns {
            let index = row * columns + column;
            let cell = cells[index];
            let hash = mix32(index as u32 ^ 0x5343_4146);
            let metric = diagonal_metric(column, row, columns, rows) as i32;
            let distance = metric.abs_diff(front);
            let foreground_weight = usize::from(cell.foreground);
            let structural =
                cell.glyph != 0 || foreground_weight >= 4 || cell.background != 0 || hash & 15 == 0;
            if !structural {
                continue;
            }
            let density_limit = if cell.glyph != 0 {
                u32::from(u16::MAX)
            } else if foreground_weight >= 6 || cell.background != 0 {
                60_000
            } else if distance < 8_000 {
                24_000
            } else {
                8_000
            };
            let density_limit =
                density_limit.saturating_mul(u32::from(mix_q16)) / u32::from(PROGRESS_MAX);
            if hash & 0xffff >= density_limit {
                continue;
            }
            let x0 = column * width / columns;
            let x1 = (column + 1) * width / columns;
            let y0 = row * height / rows;
            let y1 = (row + 1) * height / rows;
            let glyph_index = if cell.glyph == 0 {
                10 + (hash as u8 & 7)
            } else {
                cell.glyph
            };
            let color = if distance < 3_200 && hash & 7 == 0 {
                PALETTE[10]
            } else if foreground_weight >= 9 {
                PALETTE[6]
            } else if foreground_weight >= 5 {
                PALETTE[5]
            } else {
                PALETTE[4]
            };
            let travel = packet_travel(index, reveal_q16);
            draw_glyph_packet(
                working,
                width,
                height,
                (x0, y0, x1, y1),
                glyph_index,
                color,
                PALETTE[3],
                travel,
                stats,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_recompile_cell(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    bounds: (usize, usize, usize, usize),
    destination: CharacterCell,
    index: usize,
    local_q16: u16,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let (x0, y0, x1, y1) = bounds;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let cell_width = x1.saturating_sub(x0);
    let cell_height = y1.saturating_sub(y0);
    let front_distance = local_q16.abs_diff(PROGRESS_MAX / 2);
    let density_hash = mix32(index as u32 ^ 0x434f_4445);
    let visible = if front_distance < 5_000 {
        true
    } else if front_distance < 19_000 {
        density_hash & 3 != 0
    } else {
        density_hash & 1 != 0
    };
    let mut glyph_index = if front_distance < 7_000 {
        2 + (density_hash as u8 % 30)
    } else if local_q16 < PROGRESS_MAX / 2 {
        2 + (density_hash.rotate_left(5) as u8 % 30)
    } else {
        destination.glyph
    };
    if glyph_index == 0 && front_distance < 26_000 {
        glyph_index = 2 + (density_hash as u8 % 30);
    }
    let color = if front_distance < 2_400 {
        PALETTE[15]
    } else if front_distance < 5_200 {
        PALETTE[11]
    } else if local_q16 < PROGRESS_MAX / 2 {
        PALETTE[6]
    } else {
        PALETTE[5]
    };
    if visible {
        draw_glyph_packet(
            working,
            width,
            height,
            bounds,
            glyph_index,
            color,
            PALETTE[3],
            packet_travel(index, reveal_q16),
            stats,
        );
        for fragment in 0..(1 + usize::from(density_hash & 7 == 0)) {
            let fragment_x =
                x0 + density_hash.rotate_left((fragment * 7) as u32) as usize % cell_width.max(1);
            let fragment_y = y0
                + density_hash.rotate_left((fragment * 11 + 3) as u32) as usize
                    % cell_height.max(1);
            fill_packet_pixel(
                working,
                width,
                height,
                fragment_x,
                fragment_y,
                2,
                if front_distance < 3_000 {
                    PALETTE[11]
                } else {
                    PALETTE[6]
                },
                stats,
            );
        }
    }

    if mix32(index as u32 ^ 0x524f_4d21) & 3 == 0 {
        let edge = if front_distance < 3_600 {
            PALETTE[10]
        } else {
            PALETTE[4]
        };
        for x in (x0..x1.min(width)).step_by(4) {
            if y0 < height {
                working[y0 * width + x] = edge;
                stats.outline_pixels = stats.outline_pixels.saturating_add(1);
            }
        }
    }
}

fn canonical_progress(direction: NavigationTransitionDirection, progress_q16: u16) -> u16 {
    match direction {
        NavigationTransitionDirection::Forward => progress_q16,
        NavigationTransitionDirection::Reverse => PROGRESS_MAX.saturating_sub(progress_q16),
    }
}

fn diagonal_reveal_progress(canonical: u16) -> u16 {
    if canonical <= super::COVER_PROGRESS {
        0
    } else {
        (((canonical - super::COVER_PROGRESS) as u32 * PROGRESS_MAX as u32)
            / (PROGRESS_MAX - super::COVER_PROGRESS).max(1) as u32)
            .min(PROGRESS_MAX as u32) as u16
    }
}

fn scale_segment(progress: u16, end: u16) -> u16 {
    ((progress.min(end) as u32 * PROGRESS_MAX as u32) / end.max(1) as u32).min(PROGRESS_MAX as u32)
        as u16
}

fn diagonal_front(reveal_q16: u16) -> i32 {
    let span = PROGRESS_MAX as i64 + i64::from(DIAGONAL_BAND_Q16) * 2;
    let base = i64::from(reveal_q16) * span / PROGRESS_MAX as i64 - i64::from(DIAGONAL_BAND_Q16);
    let doubled = i32::from(reveal_q16).saturating_mul(2);
    let peak = i32::from(PROGRESS_MAX)
        .saturating_sub(doubled.saturating_sub(i32::from(PROGRESS_MAX)).abs());
    let bias = peak.saturating_mul(DIAGONAL_HERO_BIAS_Q16) / i32::from(PROGRESS_MAX);
    base as i32 + bias
}

fn diagonal_metric(column: usize, row: usize, columns: usize, rows: usize) -> u16 {
    let x = ((column.saturating_mul(2).saturating_add(1)) as u64 * PROGRESS_MAX as u64)
        / columns.saturating_mul(2).max(1) as u64;
    let y = ((row.saturating_mul(2).saturating_add(1)) as u64 * PROGRESS_MAX as u64)
        / rows.saturating_mul(2).max(1) as u64;
    let weighted = x
        .saturating_mul(PROGRESS_MAX as u64)
        .saturating_add(y.saturating_mul(DIAGONAL_Y_WEIGHT_Q16 as u64));
    (weighted / (PROGRESS_MAX as u64 + DIAGONAL_Y_WEIGHT_Q16 as u64)).min(PROGRESS_MAX as u64)
        as u16
}

fn diagonal_x_q16(front: i32, y_q16: i32) -> i32 {
    let numerator = i64::from(front)
        .saturating_mul(i64::from(PROGRESS_MAX) + i64::from(DIAGONAL_Y_WEIGHT_Q16))
        .saturating_sub(i64::from(y_q16).saturating_mul(i64::from(DIAGONAL_Y_WEIGHT_Q16)));
    (numerator / i64::from(PROGRESS_MAX)) as i32
}

#[allow(clippy::too_many_arguments)]
fn render_procedural_covered(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    edge: NavigationTransitionEdge,
    columns: usize,
    rows: usize,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != width.saturating_mul(height) {
        return;
    }
    working.fill(PALETTE[0]);
    stats.filled_pixels = stats.filled_pixels.saturating_add(working.len() as u64);
    let full = NavigationTransitionRect {
        x: 8.min(width) as u16,
        y: 8.min(height) as u16,
        width: width.saturating_sub(16.min(width)).min(u16::MAX as usize) as u16,
        height: height.saturating_sub(16.min(height)).min(u16::MAX as usize) as u16,
    };
    draw_link_field(
        working,
        width,
        height,
        full,
        columns,
        rows,
        PROGRESS_MAX,
        true,
        stats,
    );
    draw_rom_landmarks(working, width, height, edge, stats);
    draw_rom_frame(working, width, height, 0, diagonal_front(0), stats);
    fill_rect_565(
        working,
        width,
        height,
        geometry.destination_title,
        PALETTE[0],
        stats,
    );
    draw_rom_label(
        working,
        width,
        height,
        geometry.destination_title,
        geometry,
        PALETTE[11],
        stats,
    );
}

fn sample_rect_background(
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    fallback: Rgb565Pixel,
) -> Rgb565Pixel {
    if snapshot.len() != width.saturating_mul(height) || !rect.fits(width, height) {
        return fallback;
    }
    let x0 = usize::from(rect.x);
    let y0 = usize::from(rect.y);
    let x1 = usize::from(rect.right().saturating_sub(1));
    let y1 = usize::from(rect.bottom().saturating_sub(1));
    let corners = [
        snapshot[y0 * width + x0],
        snapshot[y0 * width + x1],
        snapshot[y1 * width + x0],
        snapshot[y1 * width + x1],
    ];
    let mut best = fallback;
    let mut best_count = 0usize;
    for candidate in corners {
        let count = corners.iter().filter(|pixel| **pixel == candidate).count();
        if count > best_count {
            best = candidate;
            best_count = count;
        }
    }
    best
}

fn render_rom_cover(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    edge: NavigationTransitionEdge,
    columns: usize,
    rows: usize,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let full = NavigationTransitionRect {
        x: 8.min(width) as u16,
        y: 8.min(height) as u16,
        width: width.saturating_sub(16.min(width)).min(u16::MAX as usize) as u16,
        height: height.saturating_sub(16.min(height)).min(u16::MAX as usize) as u16,
    };
    for (index, delay) in [9_000u16, 4_500].into_iter().enumerate() {
        let local = progress_q16.saturating_sub(delay);
        if local == 0 {
            continue;
        }
        let rect = lerp_rect(geometry.source_card, full, ease_out_cubic_q16(local));
        draw_pixel_frame(
            working,
            width,
            height,
            rect,
            if index == 1 { PALETTE[5] } else { PALETTE[4] },
            false,
            stats,
        );
    }
    let primary = lerp_rect(geometry.source_card, full, smoothstep_q16(progress_q16));
    blit_scaled_card_carrier(
        working,
        source,
        width,
        height,
        geometry.source_card,
        geometry.source_label,
        geometry.source_detail,
        primary,
        progress_q16,
        stats,
    );
    draw_link_field(
        working,
        width,
        height,
        primary,
        columns,
        rows,
        progress_q16,
        true,
        stats,
    );
    if progress_q16 >= 45_000 {
        draw_rom_landmarks(working, width, height, edge, stats);
    }
    draw_pixel_frame(working, width, height, primary, PALETTE[5], false, stats);
    let title_progress = smoothstep_q16(progress_q16);
    let title_carrier = lerp_rect(
        geometry.source_label,
        geometry.destination_title,
        title_progress,
    );
    if progress_q16 >= 45_000 {
        fill_rect_565(working, width, height, title_carrier, PALETTE[0], stats);
    }
    draw_rom_label(
        working,
        width,
        height,
        title_carrier,
        geometry,
        PALETTE[11],
        stats,
    );
    if progress_q16 > 48_000 {
        draw_rom_corner_blocks(working, width, height, full, PALETTE[6], PALETTE[11], stats);
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_scaled_card_carrier(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    source_rect: NavigationTransitionRect,
    source_label: NavigationTransitionRect,
    source_detail: NavigationTransitionRect,
    destination_rect: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(source_rect) = clip_rect_to_frame(source_rect, width, height) else {
        return;
    };
    let Some(destination_rect) = clip_rect_to_frame(destination_rect, width, height) else {
        return;
    };
    if source.len() != working.len()
        || source.len() != width.saturating_mul(height)
        || source_rect.width == 0
        || source_rect.height == 0
    {
        return;
    }
    if progress_q16 >= 50_000 {
        fill_rect_565(working, width, height, destination_rect, PALETTE[0], stats);
        return;
    }
    let ink_mix = smoothstep_q16(progress_q16);
    let source_x_step =
        ((u64::from(source_rect.width) << 16) / u64::from(destination_rect.width.max(1))) as usize;
    let source_y_step = ((u64::from(source_rect.height) << 16)
        / u64::from(destination_rect.height.max(1))) as usize;
    let label_background = sample_rect_background(source, width, height, source_label, PALETTE[0]);
    let detail_background =
        sample_rect_background(source, width, height, source_detail, PALETTE[0]);
    for destination_y in
        destination_rect.y as usize..destination_rect.bottom().min(height as u16) as usize
    {
        let source_y = source_rect.y as usize
            + (destination_y.saturating_sub(destination_rect.y as usize) * source_y_step >> 16);
        let mut source_x_q16 = usize::from(source_rect.x) << 16;
        for destination_x in
            destination_rect.x as usize..destination_rect.right().min(width as u16) as usize
        {
            let source_x = (source_x_q16 >> 16).min(width - 1);
            let source_y = source_y.min(height - 1);
            let sampled = if rect_contains(source_label, source_x, source_y) {
                label_background
            } else if rect_contains(source_detail, source_x, source_y) {
                detail_background
            } else {
                source[source_y * width + source_x]
            };
            working[destination_y * width + destination_x] =
                blend_rgb565(sampled, PALETTE[0], ink_mix);
            source_x_q16 = source_x_q16.saturating_add(source_x_step);
        }
    }
    stats.copied_pixels = stats.copied_pixels.saturating_add(
        u64::from(destination_rect.width).saturating_mul(u64::from(destination_rect.height)),
    );
}

fn rect_contains(rect: NavigationTransitionRect, x: usize, y: usize) -> bool {
    x >= usize::from(rect.x)
        && x < usize::from(rect.right())
        && y >= usize::from(rect.y)
        && y < usize::from(rect.bottom())
}

#[allow(clippy::too_many_arguments)]
fn draw_link_field(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    columns: usize,
    rows: usize,
    progress_q16: u16,
    cyan_wave: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let wave = progress_q16 / 2;
    let base_density =
        3_000u32.saturating_add(u32::from(progress_q16) * 10_000 / u32::from(PROGRESS_MAX));
    for row in 0..rows {
        let y0 = rect.y as usize + row * rect.height as usize / rows;
        let y1 = rect.y as usize + (row + 1) * rect.height as usize / rows;
        for column in 0..columns {
            let index = row * columns + column;
            let hash = mix32(index as u32 ^ 0x4c49_4e4b);
            let metric = diagonal_metric(column, row, columns, rows);
            let wave_bonus = if metric.abs_diff(wave) < 18_000 {
                17_000
            } else {
                0
            };
            if hash & 0xffff >= base_density.saturating_add(wave_bonus) {
                continue;
            }
            let x0 = rect.x as usize + column * rect.width as usize / columns;
            let x1 = rect.x as usize + (column + 1) * rect.width as usize / columns;
            let cell_width = x1.saturating_sub(x0);
            let cell_height = y1.saturating_sub(y0);
            if cell_width < 8 || cell_height < 8 {
                continue;
            }
            let scale = (cell_width / 10).min(cell_height / 10).clamp(1, 3);
            let packet_width = 8 * scale;
            let packet_height = 8 * scale;
            let packet_x = x0 + cell_width.saturating_sub(packet_width) / 2;
            let packet_y = y0 + cell_height.saturating_sub(packet_height) / 2;
            let color = if cyan_wave && metric.abs_diff(wave) < 4_500 {
                PALETTE[11]
            } else {
                PALETTE[5]
            };
            let glyph = &CHARACTER_ROM[(2 + hash as usize % 126).min(127)];
            for (glyph_y, bits) in glyph.iter().copied().enumerate() {
                for glyph_x in 0..8usize {
                    if bits & (1 << (7 - glyph_x)) != 0 {
                        fill_packet_pixel(
                            working,
                            width,
                            height,
                            packet_x + glyph_x * scale,
                            packet_y + glyph_y * scale,
                            scale,
                            color,
                            stats,
                        );
                    }
                }
            }
        }
    }
}

fn draw_rom_landmarks(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    edge: NavigationTransitionEdge,
    stats: &mut NavigationTransitionRenderStats,
) {
    if width < 64 || height < 64 {
        return;
    }
    let rect = |x: usize, y: usize, w: usize, h: usize| NavigationTransitionRect {
        x: x.min(u16::MAX as usize) as u16,
        y: y.min(u16::MAX as usize) as u16,
        width: w.min(u16::MAX as usize) as u16,
        height: h.min(u16::MAX as usize) as u16,
    };
    match edge {
        NavigationTransitionEdge::HomeToConsoles => {
            let gutter = width / 80;
            let top = height * 17 / 100;
            let card_height = height * 72 / 100;
            let card_width = (width.saturating_sub(gutter * 6)) / 5;
            for index in 0..5 {
                let card = rect(
                    gutter + index * (card_width + gutter),
                    top,
                    card_width,
                    card_height,
                );
                draw_pixel_frame(
                    working,
                    width,
                    height,
                    card,
                    if index == 0 { PALETTE[7] } else { PALETTE[5] },
                    false,
                    stats,
                );
                let center_y = card.y.saturating_add(card.height / 2);
                fill_rect_565(
                    working,
                    width,
                    height,
                    rect(
                        card.x as usize + card.width as usize / 5,
                        center_y as usize,
                        card.width as usize * 3 / 5,
                        2,
                    ),
                    if index == 0 { PALETTE[10] } else { PALETTE[4] },
                    stats,
                );
            }
        }
        NavigationTransitionEdge::HomeToArcade | NavigationTransitionEdge::ConsolesToSystem => {
            let list = rect(
                width / 50,
                height * 19 / 100,
                width * 49 / 100,
                height * 72 / 100,
            );
            let preview = rect(
                width * 57 / 100,
                height * 19 / 100,
                width * 39 / 100,
                height * 61 / 100,
            );
            draw_pixel_frame(working, width, height, list, PALETTE[6], false, stats);
            draw_pixel_frame(working, width, height, preview, PALETTE[6], false, stats);
            let row_height = list.height as usize / 7;
            for row in 0..6 {
                let y = list.y as usize + row * row_height;
                let color = if row == 0 { PALETTE[7] } else { PALETTE[4] };
                for x in (list.x as usize..list.right() as usize).step_by(8) {
                    fill_rect_565(
                        working,
                        width,
                        height,
                        rect(x, y, 4, if row == 0 { 3 } else { 2 }),
                        color,
                        stats,
                    );
                }
            }
            for row in 0..5 {
                let y = preview.y as usize
                    + preview.height as usize / 6
                    + row * preview.height as usize / 7;
                let inset = preview.width as usize / 8 + (row & 1) * 6;
                fill_rect_565(
                    working,
                    width,
                    height,
                    rect(
                        preview.x as usize + inset,
                        y,
                        preview.width as usize - inset * 2,
                        3,
                    ),
                    if row == 2 { PALETTE[6] } else { PALETTE[4] },
                    stats,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rom_label(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    geometry: NavigationTransitionGeometry,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let length = usize::from(geometry.label_len).min(geometry.label_ascii.len());
    if length == 0 || rect.width == 0 || rect.height == 0 {
        return;
    }
    let glyph_width = length.saturating_mul(6).saturating_sub(1);
    if (rect.width as usize) < glyph_width || rect.height < 7 {
        return;
    }
    let scale = (rect.width as usize / glyph_width.max(1))
        .min(rect.height as usize / 7)
        .clamp(1, 4);
    let text_width = glyph_width * scale;
    let text_height = 7 * scale;
    let origin_x = rect.x as usize + (rect.width as usize).saturating_sub(text_width) / 2;
    let origin_y = rect.y as usize + (rect.height as usize).saturating_sub(text_height) / 2;
    for (index, byte) in geometry.label_ascii[..length].iter().copied().enumerate() {
        for (row, bits) in glyph5x7(byte).iter().copied().enumerate() {
            for column in 0..5usize {
                if bits & (1 << (4 - column)) != 0 {
                    fill_packet_pixel(
                        working,
                        width,
                        height,
                        origin_x + (index * 6 + column) * scale,
                        origin_y + row * scale,
                        scale,
                        color,
                        stats,
                    );
                }
            }
        }
    }
}

fn draw_rom_frame(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    reveal_q16: u16,
    front: i32,
    stats: &mut NavigationTransitionRenderStats,
) {
    if reveal_q16 >= 62_500 {
        return;
    }
    let rect = NavigationTransitionRect {
        x: 8.min(width) as u16,
        y: 8.min(height) as u16,
        width: width.saturating_sub(16.min(width)).min(u16::MAX as usize) as u16,
        height: height.saturating_sub(16.min(height)).min(u16::MAX as usize) as u16,
    };
    draw_pixel_frame(working, width, height, rect, PALETTE[5], false, stats);
    draw_rom_corner_blocks(working, width, height, rect, PALETTE[6], PALETTE[11], stats);

    // The two beam intersections energise opposite corners as the diagonal
    // travels from the upper-left program image to the lower-right one.
    if (0..=PROGRESS_MAX as i32).contains(&front) {
        let top_x = (i64::from(diagonal_x_q16(front, 0)) * width as i64 / PROGRESS_MAX as i64)
            .clamp(0, width.saturating_sub(1) as i64) as usize;
        if top_x < width {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: top_x.saturating_sub(3) as u16,
                    y: rect.y,
                    width: 7,
                    height: 3,
                },
                PALETTE[12],
                stats,
            );
        }
    }
}

fn draw_pixel_frame(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    color: Rgb565Pixel,
    solid: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    if solid {
        draw_outline_565(working, width, height, rect, color, stats);
    }
    for x in (rect.x as usize..rect.right() as usize).step_by(12) {
        let segment = 5.min(rect.right() as usize - x);
        for y in [rect.y, rect.bottom().saturating_sub(2)] {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: x as u16,
                    y,
                    width: segment as u16,
                    height: 2,
                },
                color,
                stats,
            );
        }
    }
    for y in (rect.y as usize..rect.bottom() as usize).step_by(12) {
        let segment = 5.min(rect.bottom() as usize - y);
        for x in [rect.x, rect.right().saturating_sub(2)] {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x,
                    y: y as u16,
                    width: 2,
                    height: segment as u16,
                },
                color,
                stats,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_rom_corner_blocks(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    lavender: Rgb565Pixel,
    _cyan: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    for (right, bottom) in [(false, false), (true, false), (false, true), (true, true)] {
        let anchor_x = if right {
            rect.right().saturating_sub(32)
        } else {
            rect.x
        };
        let anchor_y = if bottom {
            rect.bottom().saturating_sub(32)
        } else {
            rect.y
        };
        for step in 0..3u16 {
            let offset = step * 6;
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: if right {
                        anchor_x.saturating_add(offset)
                    } else {
                        anchor_x
                    },
                    y: if bottom {
                        anchor_y.saturating_add(offset)
                    } else {
                        anchor_y
                    },
                    width: 24u16.saturating_sub(offset),
                    height: 3,
                },
                lavender,
                stats,
            );
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: if right {
                        anchor_x.saturating_add(29)
                    } else {
                        anchor_x.saturating_add(offset)
                    },
                    y: if bottom {
                        anchor_y.saturating_add(offset)
                    } else {
                        anchor_y
                    },
                    width: 3,
                    height: 24u16.saturating_sub(offset),
                },
                lavender,
                stats,
            );
        }
    }
}

fn draw_diagonal_verification_beam(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    front: i32,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if width == 0 || height == 0 || reveal_q16 == 0 || reveal_q16 >= 65_000 {
        return;
    }
    for x in (0..width).step_by(2) {
        let x_q16 = (x as i64 * PROGRESS_MAX as i64 / width.max(1) as i64) as i32;
        let numerator = i64::from(front)
            .saturating_mul(i64::from(PROGRESS_MAX) + i64::from(DIAGONAL_Y_WEIGHT_Q16))
            .saturating_sub(i64::from(x_q16).saturating_mul(i64::from(PROGRESS_MAX)));
        let y_q16 = (numerator / i64::from(DIAGONAL_Y_WEIGHT_Q16)) as i32;
        if !(0..PROGRESS_MAX as i32).contains(&y_q16) {
            continue;
        }
        let y = (y_q16 as usize * height / PROGRESS_MAX as usize).min(height - 1);
        for (offset, color) in [
            (-4isize, PALETTE[8]),
            (-3, PALETTE[9]),
            (-2, PALETTE[10]),
            (-1, PALETTE[11]),
            (0, PALETTE[12]),
            (1, PALETTE[11]),
            (2, PALETTE[10]),
            (3, PALETTE[8]),
        ] {
            if offset.abs() >= 3 && (x / 4 + usize::from(reveal_q16) / 512) & 1 != 0 {
                continue;
            }
            let py = y as isize + offset;
            if py >= 0 && py < height as isize {
                let start = py as usize * width + x;
                let end = (start + 2).min((py as usize + 1) * width);
                working[start..end].fill(color);
                stats.filled_pixels = stats
                    .filled_pixels
                    .saturating_add(end.saturating_sub(start) as u64);
            }
        }
        if x % 28 < 8 {
            let py = y.saturating_sub(1);
            let start = py * width + x;
            let end = (start + 2).min((py + 1) * width);
            working[start..end].fill(PALETTE[15]);
            stats.filled_pixels = stats
                .filled_pixels
                .saturating_add(end.saturating_sub(start) as u64);
        }
        if x % 24 == 0 {
            let hash = mix32(x as u32 ^ u32::from(reveal_q16));
            let (tangent, normal) = spark_offsets(hash);
            let spark_y =
                (y as isize + normal).clamp(0, height.saturating_sub(1) as isize) as usize;
            let spark_x =
                (x as isize + tangent).clamp(0, width.saturating_sub(1) as isize) as usize;
            fill_packet_pixel(
                working,
                width,
                height,
                spark_x.min(width - 1),
                spark_y,
                2,
                if hash & 3 == 0 {
                    PALETTE[14]
                } else {
                    PALETTE[11]
                },
                stats,
            );
            if hash & 1 == 0 {
                let fragment_x = spark_x.saturating_add(4).min(width - 1);
                let fragment_y = spark_y.saturating_sub(4);
                fill_packet_pixel(
                    working, width, height, fragment_x, fragment_y, 1, PALETTE[6], stats,
                );
            }
        }
    }
}

fn spark_offsets(hash: u32) -> (isize, isize) {
    (
        (hash.rotate_left(7) % 35) as isize - 17,
        (hash % 41) as isize - 20,
    )
}

fn packet_travel(index: usize, reveal_q16: u16) -> isize {
    let phase = (u32::from(reveal_q16) * 24 / u32::from(PROGRESS_MAX)) as isize;
    let stagger = (mix32(index as u32 ^ 0x4e45_4d4f) & 7) as isize;
    (phase + stagger) % 13 - 6
}

#[allow(clippy::too_many_arguments)]
fn draw_glyph_packet(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    bounds: (usize, usize, usize, usize),
    glyph_index: u8,
    color: Rgb565Pixel,
    shadow: Rgb565Pixel,
    travel: isize,
    stats: &mut NavigationTransitionRenderStats,
) {
    let (x0, y0, x1, y1) = bounds;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let cell_width = x1.saturating_sub(x0);
    let cell_height = y1.saturating_sub(y0);
    if cell_width < 8 || cell_height < 8 {
        return;
    }
    let scale = (cell_width / 10).min(cell_height / 10).clamp(1, 3);
    let packet_width = 8 * scale;
    let packet_height = 8 * scale;
    let center_x = x0 + cell_width.saturating_sub(packet_width) / 2;
    let center_y = y0 + cell_height.saturating_sub(packet_height) / 2;
    let packet_x = (center_x as isize + travel)
        .clamp(x0 as isize, x1.saturating_sub(packet_width) as isize) as usize;
    let packet_y = (center_y as isize - travel)
        .clamp(y0 as isize, y1.saturating_sub(packet_height) as isize) as usize;
    for (glyph_y, bits) in CHARACTER_ROM[glyph_index as usize]
        .iter()
        .copied()
        .enumerate()
    {
        for glyph_x in 0..8usize {
            if bits & (1 << (7 - glyph_x)) == 0 {
                continue;
            }
            let px = packet_x + glyph_x * scale;
            let py = packet_y + glyph_y * scale;
            fill_packet_pixel(
                working,
                width,
                height,
                px.saturating_add(1),
                py.saturating_add(1),
                scale,
                shadow,
                stats,
            );
            fill_packet_pixel(working, width, height, px, py, scale, color, stats);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_packet_pixel(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    scale: usize,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    if x >= width || y >= height {
        return;
    }
    let x1 = x.saturating_add(scale).min(width);
    let y1 = y.saturating_add(scale).min(height);
    for py in y..y1 {
        working[py * width + x..py * width + x1].fill(color);
        stats.filled_pixels = stats
            .filled_pixels
            .saturating_add(x1.saturating_sub(x) as u64);
    }
}

fn blend_rgb565(from: Rgb565Pixel, to: Rgb565Pixel, amount_q16: u16) -> Rgb565Pixel {
    let amount = u32::from(amount_q16) >> 8;
    let inverse = 256 - amount;
    let from_red = u32::from((from.0 >> 11) & 0x1f);
    let from_green = u32::from((from.0 >> 5) & 0x3f);
    let from_blue = u32::from(from.0 & 0x1f);
    let to_red = u32::from((to.0 >> 11) & 0x1f);
    let to_green = u32::from((to.0 >> 5) & 0x3f);
    let to_blue = u32::from(to.0 & 0x1f);
    let red = (from_red * inverse + to_red * amount) >> 8;
    let green = (from_green * inverse + to_green * amount) >> 8;
    let blue = (from_blue * inverse + to_blue * amount) >> 8;
    Rgb565Pixel(((red as u16) << 11) | ((green as u16) << 5) | blue as u16)
}

fn quantize_snapshot(
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
    output: &mut Vec<CharacterCell>,
) {
    output.clear();
    output.reserve(columns.saturating_mul(rows));
    for row in 0..rows {
        let y0 = row * height / rows;
        let y1 = (row + 1) * height / rows;
        for column in 0..columns {
            let x0 = column * width / columns;
            let x1 = (column + 1) * width / columns;
            let mut luminance_tile = [[0u8; 8]; 8];
            let mut luminance_sum = 0u32;
            let mut red = 0u32;
            let mut green = 0u32;
            let mut blue = 0u32;
            for (glyph_y, tile_row) in luminance_tile.iter_mut().enumerate() {
                let y = y0
                    + ((glyph_y * 2 + 1) * y1.saturating_sub(y0) / 16)
                        .min(y1.saturating_sub(y0).saturating_sub(1));
                for (glyph_x, luminance) in tile_row.iter_mut().enumerate() {
                    let x = x0
                        + ((glyph_x * 2 + 1) * x1.saturating_sub(x0) / 16)
                            .min(x1.saturating_sub(x0).saturating_sub(1));
                    let pixel = snapshot
                        .get(
                            y.min(height.saturating_sub(1)) * width
                                + x.min(width.saturating_sub(1)),
                        )
                        .copied()
                        .unwrap_or_default();
                    *luminance = rgb565_luminance(pixel);
                    luminance_sum = luminance_sum.saturating_add(u32::from(*luminance));
                    red = red.saturating_add(u32::from((pixel.0 >> 11) & 0x1f));
                    green = green.saturating_add(u32::from((pixel.0 >> 5) & 0x3f));
                    blue = blue.saturating_add(u32::from(pixel.0 & 0x1f));
                }
            }
            let average_luminance = (luminance_sum / 64) as u8;
            let variance = luminance_tile
                .iter()
                .flatten()
                .map(|value| u32::from(value.abs_diff(average_luminance)))
                .sum::<u32>();
            let mut pattern = [0u8; 8];
            if variance >= 160 {
                for (glyph_y, tile_row) in luminance_tile.iter().enumerate() {
                    for (glyph_x, luminance) in tile_row.iter().copied().enumerate() {
                        if luminance >= average_luminance.saturating_add(3) {
                            pattern[glyph_y] |= 1 << (7 - glyph_x);
                        }
                    }
                }
            }
            let average = Rgb565Pixel(
                (((red / 64) as u16) << 11) | (((green / 64) as u16) << 5) | (blue / 64) as u16,
            );
            let glyph = nearest_character_glyph(pattern);
            output.push(CharacterCell {
                glyph,
                foreground: nearest_palette(average),
                background: if average_luminance < 72 { 0 } else { 1 },
            });
        }
    }
}

fn nearest_character_glyph(pattern: [u8; 8]) -> u8 {
    CHARACTER_ROM
        .iter()
        .enumerate()
        .min_by_key(|(_, glyph)| {
            glyph
                .iter()
                .zip(pattern)
                .map(|(actual, wanted)| (actual ^ wanted).count_ones())
                .sum::<u32>()
        })
        .map(|(index, _)| index as u8)
        .unwrap_or(0)
}

fn flips_for_frame(frame_index: usize, columns: usize, rows: usize) -> usize {
    let count_at = |index: usize| {
        let canonical =
            (index.min(FRAME_COUNT - 1) * usize::from(PROGRESS_MAX) / (FRAME_COUNT - 1)) as u16;
        let front = diagonal_front(diagonal_reveal_progress(canonical));
        (0..columns.saturating_mul(rows))
            .filter(|cell| {
                let row = *cell / columns.max(1);
                let column = *cell % columns.max(1);
                i32::from(diagonal_metric(column, row, columns, rows)) <= front
            })
            .count()
    };
    count_at(frame_index).abs_diff(count_at(frame_index.saturating_sub(1)))
}

fn nearest_palette(pixel: Rgb565Pixel) -> u8 {
    PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| rgb565_distance(pixel, **candidate))
        .map(|(index, _)| index as u8)
        .unwrap_or(0)
}

fn rgb565_distance(a: Rgb565Pixel, b: Rgb565Pixel) -> u32 {
    let ar = ((a.0 >> 11) & 0x1f) as i32;
    let ag = ((a.0 >> 5) & 0x3f) as i32;
    let ab = (a.0 & 0x1f) as i32;
    let br = ((b.0 >> 11) & 0x1f) as i32;
    let bg = ((b.0 >> 5) & 0x3f) as i32;
    let bb = (b.0 & 0x1f) as i32;
    ((ar - br).pow(2) + (ag - bg).pow(2) + (ab - bb).pow(2)) as u32
}

fn rgb565_luminance(pixel: Rgb565Pixel) -> u8 {
    let red = ((pixel.0 >> 11) & 0x1f) as u32 * 255 / 31;
    let green = ((pixel.0 >> 5) & 0x3f) as u32 * 255 / 63;
    let blue = (pixel.0 & 0x1f) as u32 * 255 / 31;
    ((red * 3 + green * 6 + blue) / 10) as u8
}

fn glyph5x7(character: u8) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'I' => [31, 4, 4, 4, 4, 4, 31],
        b'J' => [7, 2, 2, 2, 18, 18, 12],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 21, 19, 17, 17, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 21, 10],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'Z' => [31, 1, 2, 4, 8, 16, 31],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [14, 16, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 1, 14],
        b'-' => [0, 0, 0, 31, 0, 0, 0],
        b'/' => [1, 2, 2, 4, 8, 8, 16],
        b':' => [0, 4, 4, 0, 4, 4, 0],
        b' ' => [0; 7],
        _ => [31, 1, 2, 4, 4, 0, 4],
    }
}

const fn build_character_rom() -> [[u8; 8]; 128] {
    let mut rom = [[0u8; 8]; 128];
    rom[0] = [0; 8];
    rom[1] = [0xff; 8];
    rom[2] = [0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55, 0xaa, 0x55];
    rom[3] = [0x88, 0x22, 0x88, 0x22, 0x88, 0x22, 0x88, 0x22];
    rom[4] = [0xff, 0x00, 0x00, 0xff, 0x00, 0x00, 0xff, 0x00];
    rom[5] = [0x92; 8];
    rom[6] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80];
    rom[7] = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];
    rom[8] = [0x81, 0x42, 0x24, 0x18, 0x18, 0x24, 0x42, 0x81];
    rom[9] = [0x18, 0x18, 0x18, 0xff, 0xff, 0x18, 0x18, 0x18];
    rom[10] = [0xff, 0x81, 0x81, 0x81, 0x81, 0x81, 0x81, 0xff];
    rom[11] = [0xe7, 0x81, 0x81, 0x00, 0x00, 0x81, 0x81, 0xe7];
    rom[12] = [0x18, 0x3c, 0x7e, 0xff, 0xff, 0x7e, 0x3c, 0x18];
    rom[13] = [0x18, 0x3c, 0x7e, 0xdb, 0x18, 0x18, 0x18, 0x18];
    rom[14] = [0x10, 0x18, 0xfc, 0xfe, 0xfc, 0x18, 0x10, 0x00];
    rom[15] = [0x18, 0x18, 0x18, 0x18, 0xdb, 0x7e, 0x3c, 0x18];
    rom[16] = [0x08, 0x18, 0x3f, 0x7f, 0x3f, 0x18, 0x08, 0x00];
    rom[17] = [0x00, 0x24, 0x00, 0x81, 0x00, 0x24, 0x00, 0x81];
    rom[18] = [0x11, 0x44, 0x11, 0x44, 0x11, 0x44, 0x11, 0x44];
    rom[19] = [0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    rom[20] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff];
    rom[21] = [0x80; 8];
    rom[22] = [0x01; 8];
    rom[23] = [0x81, 0x81, 0x81, 0xff, 0xff, 0x81, 0x81, 0x81];
    rom[24] = [0xff, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0xff];
    rom[25] = [0x00, 0x3c, 0x42, 0x99, 0xa5, 0x81, 0x42, 0x3c];
    rom[26] = [0x18, 0x7e, 0x5a, 0xff, 0x7e, 0x24, 0x66, 0xc3];
    rom[27] = [0x66, 0xff, 0xdb, 0xff, 0x7e, 0x24, 0x5a, 0xa5];
    rom[28] = [0x18, 0x18, 0x7e, 0x18, 0x7e, 0x18, 0x18, 0x00];
    rom[29] = [0x00, 0x66, 0x3c, 0xff, 0x3c, 0x66, 0x00, 0x00];
    rom[30] = [0xc3, 0x66, 0x3c, 0x18, 0x18, 0x3c, 0x66, 0xc3];
    rom[31] = [0x00, 0x18, 0x3c, 0x7e, 0x3c, 0x18, 0x00, 0x00];

    let mut glyph = 32usize;
    while glyph < 128 {
        let family = (glyph - 32) / 16;
        let variant = ((glyph - 32) & 15) as u8;
        let mut row = 0usize;
        while row < 8 {
            rom[glyph][row] = match family {
                0 => {
                    let shift = (row + variant as usize) & 7;
                    (1u8 << shift) | (1u8 << ((shift + 1) & 7))
                }
                1 => {
                    let stripe = 1u8 << (variant & 7);
                    if row & 1 == (variant >> 3) as usize {
                        stripe | stripe.rotate_left(3)
                    } else {
                        0
                    }
                }
                2 => {
                    let top = variant & 1 != 0 && row == 0;
                    let bottom = variant & 2 != 0 && row == 7;
                    let left = variant & 4 != 0;
                    let right = variant & 8 != 0;
                    (if top || bottom { 0xff } else { 0 })
                        | (if left { 0x80 } else { 0 })
                        | (if right { 0x01 } else { 0 })
                }
                3 => {
                    let period = (variant as usize & 3) + 2;
                    if (row + variant as usize / 4) % period == 0 {
                        0xff
                    } else {
                        (0x81u8).rotate_left((variant & 3) as u32)
                    }
                }
                4 => {
                    let nibble = variant | (variant << 4);
                    if row < 4 {
                        nibble.rotate_left(row as u32)
                    } else {
                        nibble.rotate_left((7 - row) as u32)
                    }
                }
                _ => {
                    let seed = variant.wrapping_mul(0x13).rotate_left((row & 3) as u32);
                    let mirrored = (seed & 0x0f) | (seed.reverse_bits() & 0xf0);
                    if row == 0 || row == 7 {
                        mirrored & 0x7e
                    } else {
                        mirrored
                    }
                }
            };
            row += 1;
        }
        glyph += 1;
    }
    rom
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
    fn quantization_is_deterministic_and_uses_fixed_contracts() {
        let pixels = (0..160 * 90)
            .map(|value| Rgb565Pixel(value as u16))
            .collect::<Vec<_>>();
        let mut first = Vec::new();
        let mut second = Vec::new();
        quantize_snapshot(&pixels, 160, 90, 30, 17, &mut first);
        quantize_snapshot(&pixels, 160, 90, 30, 17, &mut second);
        assert_eq!(first, second);
        assert_eq!(first.len(), 30 * 17);
        assert_eq!(CHARACTER_ROM.len(), 128);
        assert_eq!(PALETTE.len(), 16);
    }

    #[test]
    fn frame_flip_limit_is_bounded_in_both_grid_modes() {
        for (columns, rows) in [(FULL_COLUMNS, FULL_ROWS), (FALLBACK_COLUMNS, FALLBACK_ROWS)] {
            for frame in 0..FRAME_COUNT {
                assert!(
                    flips_for_frame(frame, columns, rows) <= MAX_NEW_FLIPS_PER_FRAME,
                    "{columns}x{rows} frame {frame}"
                );
            }
        }
    }

    #[test]
    fn fallback_grid_has_the_requested_shape() {
        let renderer = CharacterRomRenderer::new(true);
        assert_eq!(renderer.cell_count(), 20 * 12);
    }

    #[test]
    fn diagonal_hero_connects_bottom_left_to_top_right() {
        let front = diagonal_front(PROGRESS_MAX / 2);
        assert!(front.abs_diff(i32::from(PROGRESS_MAX) / 2 + DIAGONAL_HERO_BIAS_Q16) <= 4);
        let top_intersection = diagonal_x_q16(front, 0);
        let bottom_intersection = diagonal_x_q16(front, i32::from(PROGRESS_MAX));
        assert!(top_intersection.abs_diff(i32::from(PROGRESS_MAX) * 9 / 10) < 1_500);
        assert!(bottom_intersection.abs_diff(i32::from(PROGRESS_MAX) / 4) < 1_500);
    }

    #[test]
    fn diagonal_metric_advances_from_upper_left_to_lower_right() {
        let upper_left = diagonal_metric(0, 0, FULL_COLUMNS, FULL_ROWS);
        let lower_right = diagonal_metric(FULL_COLUMNS - 1, FULL_ROWS - 1, FULL_COLUMNS, FULL_ROWS);
        assert!(upper_left < PROGRESS_MAX / 16);
        assert!(lower_right > PROGRESS_MAX - PROGRESS_MAX / 16);
    }

    #[test]
    fn packet_travel_is_deterministic_and_moves_northeast() {
        let start = packet_travel(17, 0);
        let later = packet_travel(17, PROGRESS_MAX / 4);
        assert_eq!(start, packet_travel(17, 0));
        assert_ne!(start, later);
        assert!((-6..=6).contains(&start));
        assert!((-6..=6).contains(&later));
    }

    #[test]
    fn spark_offsets_are_bounded_for_high_bit_hashes() {
        for hash in [0, 1, 0x7fff_ffff, 0x8000_0000, u32::MAX] {
            let (tangent, normal) = spark_offsets(hash);
            assert!((-17..=17).contains(&tangent));
            assert!((-20..=20).contains(&normal));
        }
    }
}
