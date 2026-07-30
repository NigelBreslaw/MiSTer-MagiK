// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic PCB/data-track navigation treatment.

use super::{
    NavigationTransitionBuffers, NavigationTransitionDirection, NavigationTransitionFailure,
    NavigationTransitionFrame, NavigationTransitionPhase, NavigationTransitionRect,
    NavigationTransitionRenderStats, NavigationTransitionRequest, PROGRESS_MAX, fill_rect_565,
    render_super_scaler_shell,
};
use crate::particle_engine::TargetMask;
use crate::particle_renderer::{
    pack_visual_command, raster_packed_visual_commands, unpack_visual_command,
};
use slint::platform::software_renderer::Rgb565Pixel;

const FULL_PARTICLE_COUNT: usize = 2_048;
const FALLBACK_PARTICLE_COUNT: usize = 512;
const GLYPH_PACKET_COUNT: usize = 6;
const GLYPH_PACKET_ROWS: [[u8; 7]; GLYPH_PACKET_COUNT] = [
    [
        0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
    ],
    [
        0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
    ],
    [
        0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
    ],
    [
        0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
    ],
    [
        0b10000, 0b11000, 0b01100, 0b00110, 0b01100, 0b11000, 0b10000,
    ],
    [
        0b10000, 0b11000, 0b01100, 0b00110, 0b01100, 0b11000, 0b10000,
    ],
];

#[derive(Debug)]
pub(super) struct SpriteFoundryRenderer {
    particle_count: usize,
    formation: Vec<u32>,
    commands: Vec<u32>,
    dirty_offsets: Vec<u32>,
    width: usize,
    height: usize,
}

impl SpriteFoundryRenderer {
    pub(super) fn empty(particle_count: usize) -> Self {
        let particle_count = normalize_particle_count(particle_count);
        Self {
            particle_count,
            formation: Vec::new(),
            commands: Vec::new(),
            dirty_offsets: Vec::new(),
            width: 0,
            height: 0,
        }
    }

    pub(super) fn prepare(&mut self, width: usize, height: usize) {
        if self.width == width && self.height == height && !self.formation.is_empty() {
            return;
        }
        self.width = width;
        self.height = height;
        self.formation.clear();
        self.commands.clear();
        self.dirty_offsets.clear();
        if width == 0 || height == 0 {
            return;
        }
        self.commands.reserve(self.particle_count);
        self.dirty_offsets
            .reserve(self.particle_count.saturating_mul(2));
        let Some(mask) = packet_target_mask(width, height) else {
            return;
        };
        let offset_x = (width - mask.width()) / 2;
        let offset_y = (height - mask.height()) / 2;
        self.formation.reserve(self.particle_count);
        for index in 0..self.particle_count {
            let point = mask.points()[index % mask.points().len()];
            let hash = mix32(index as u32 ^ 0x5350_5249);
            let x = (offset_x + point.x as usize).min(width.saturating_sub(1));
            let y = (offset_y + point.y as usize).min(height.saturating_sub(1));
            self.formation.push(pack_visual_command(
                (y * width + x) as u32,
                (hash >> 30) as usize,
                hash & 0x20 != 0 && x + 1 < width,
            ));
        }
    }

    #[cfg(test)]
    pub(super) const fn particle_count(&self) -> usize {
        self.particle_count
    }
}

pub(super) fn configured_particle_count() -> usize {
    if super::env_flag("MISTER_NAV_TRANSITION_REDUCED_EFFECTS")
        || std::env::var("MISTER_NAV_TRANSITION_SPRITE_PARTICLES")
            .ok()
            .is_some_and(|value| value.trim() == "512")
    {
        FALLBACK_PARTICLE_COUNT
    } else {
        FULL_PARTICLE_COUNT
    }
}

pub(super) fn render_sprite_foundry(
    renderer: &mut SpriteFoundryRenderer,
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
    if width == 0 || height == 0 {
        return Ok(stats);
    }
    renderer.prepare(width, height);
    let working = buffers.working.as_mut_slice();
    let reveal = frame.reveal_progress_q16;
    let motion = match request.direction {
        NavigationTransitionDirection::Reverse if frame.reveal_progress_q16 > 0 => {
            PROGRESS_MAX.saturating_sub(reveal)
        }
        _ => frame.cover_progress_q16,
    };
    let start = match request.direction {
        NavigationTransitionDirection::Forward => rect_center(request.geometry.source_card),
        NavigationTransitionDirection::Reverse => rect_center(request.geometry.destination_title),
    };
    let collapse = rect_center(request.geometry.source_card);
    let anchor = if request.direction == NavigationTransitionDirection::Reverse
        && frame.reveal_progress_q16 > 0
    {
        collapse
    } else {
        start
    };
    let active_rect = active_overlay_rect(request, frame, width, height);
    draw_data_tracks(working, width, height, active_rect, reveal, &mut stats);
    let packet_count = draw_glyph_packets(working, width, height, active_rect, frame, &mut stats);
    stats.glyph_packets = packet_count as u64;

    renderer.commands.clear();
    for index in 0..renderer.particle_count {
        if reveal != 0 && (mix32(index as u32 ^ 0x6a09_e667) & 0xffff) < reveal as u32 {
            continue;
        }
        let (target_x, target_y, palette, neighbor) = renderer
            .formation
            .get(index)
            .copied()
            .and_then(unpack_visual_command)
            .map(|(offset, palette, neighbor)| (offset % width, offset / width, palette, neighbor))
            .unwrap_or_else(|| {
                let hash = mix32(index as u32 ^ 0xbb67_ae85);
                (
                    hash as usize % width,
                    hash.rotate_left(13) as usize % height,
                    (hash >> 30) as usize,
                    hash & 0x20 != 0,
                )
            });
        let (x, y) = manhattan_position(anchor, (target_x, target_y), motion, index, width, height);
        if !point_in_rect(x, y, active_rect) {
            continue;
        }
        let safe_neighbor = neighbor && x + 1 < width && point_in_rect(x + 1, y, active_rect);
        renderer.commands.push(pack_visual_command(
            (y * width + x) as u32,
            palette,
            safe_neighbor,
        ));
    }
    renderer.dirty_offsets.clear();
    stats.particle_pixels =
        raster_packed_visual_commands(working, &renderer.commands, &mut renderer.dirty_offsets)
            as u64;
    Ok(stats)
}

fn normalize_particle_count(count: usize) -> usize {
    if count <= FALLBACK_PARTICLE_COUNT {
        FALLBACK_PARTICLE_COUNT
    } else {
        FULL_PARTICLE_COUNT
    }
}

fn packet_target_mask(width: usize, height: usize) -> Option<TargetMask> {
    let packet_columns = GLYPH_PACKET_COUNT * 6 - 1;
    if width < packet_columns || height < 7 {
        return None;
    }
    let scale = (width / packet_columns).min(height / 7).min(4).max(1);
    let mask_width = packet_columns * scale;
    let mask_height = 7 * scale;
    let mut alpha = vec![0u8; mask_width.saturating_mul(mask_height)];
    for (packet, rows) in GLYPH_PACKET_ROWS.iter().enumerate() {
        for (row, bits) in rows.iter().copied().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) == 0 {
                    continue;
                }
                let x0 = (packet * 6 + column) * scale;
                let y0 = row * scale;
                for y in y0..y0 + scale {
                    alpha[y * mask_width + x0..y * mask_width + x0 + scale].fill(255);
                }
            }
        }
    }
    TargetMask::from_alpha(mask_width, mask_height, mask_width, &alpha, 128, 1).ok()
}

fn active_overlay_rect(
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

fn draw_data_tracks(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if rect.width < 8 || rect.height < 8 {
        return;
    }
    let track_count = 24usize
        .saturating_mul((PROGRESS_MAX - reveal_q16) as usize)
        .div_ceil(PROGRESS_MAX as usize)
        .max(1);
    let mint = Rgb565Pixel(0x0654);
    let violet = Rgb565Pixel(0x49aa);
    for track in 0..track_count {
        let hash = mix32(track as u32 ^ 0x3c6e_f372);
        let x0 = rect.x as usize;
        let y0 = rect.y as usize;
        let rw = rect.width as usize;
        let rh = rect.height as usize;
        let x = x0 + (hash as usize % rw);
        let y = y0 + (hash.rotate_left(11) as usize % rh);
        let horizontal = track & 1 == 0;
        let length = if horizontal {
            12 + hash.rotate_left(7) as usize % rw.min(180).max(1)
        } else {
            10 + hash.rotate_left(7) as usize % rh.min(120).max(1)
        };
        let trace = if track % 3 == 0 { mint } else { violet };
        let line = if horizontal {
            NavigationTransitionRect {
                x: x.min(width.saturating_sub(1)) as u16,
                y: y.min(height.saturating_sub(1)) as u16,
                width: length.min((rect.right() as usize).saturating_sub(x)).max(1) as u16,
                height: 1,
            }
        } else {
            NavigationTransitionRect {
                x: x.min(width.saturating_sub(1)) as u16,
                y: y.min(height.saturating_sub(1)) as u16,
                width: 1,
                height: length
                    .min((rect.bottom() as usize).saturating_sub(y))
                    .max(1) as u16,
            }
        };
        fill_rect_565(destination, width, height, line, trace, stats);
        let pad = NavigationTransitionRect {
            x: line.right().saturating_sub(2),
            y: line.bottom().saturating_sub(2),
            width: 3,
            height: 3,
        };
        fill_rect_565(destination, width, height, pad, mint, stats);
    }
}

fn draw_glyph_packets(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    frame: NavigationTransitionFrame,
    stats: &mut NavigationTransitionRenderStats,
) -> usize {
    if frame.cover_progress_q16 < 24_000 || frame.reveal_progress_q16 > 54_000 {
        return 0;
    }
    let cell = if rect.width >= 480 { 3usize } else { 2usize };
    let packet_width = 5 * cell;
    let gap = cell * 2;
    let total_width = GLYPH_PACKET_COUNT * packet_width + (GLYPH_PACKET_COUNT - 1) * gap;
    if rect.width as usize <= total_width || rect.height as usize <= 7 * cell {
        return 0;
    }
    let origin_x = rect.x as usize + (rect.width as usize - total_width) / 2;
    let origin_y = rect.y as usize + (rect.height as usize - 7 * cell) / 2;
    for (packet, rows) in GLYPH_PACKET_ROWS.iter().enumerate() {
        for (row, bits) in rows.iter().copied().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) == 0 {
                    continue;
                }
                fill_rect_565(
                    destination,
                    width,
                    height,
                    NavigationTransitionRect {
                        x: (origin_x + packet * (packet_width + gap) + column * cell) as u16,
                        y: (origin_y + row * cell) as u16,
                        width: cell as u16,
                        height: cell as u16,
                    },
                    if packet < 4 {
                        Rgb565Pixel(0x07f0)
                    } else {
                        Rgb565Pixel(0xb35f)
                    },
                    stats,
                );
            }
        }
    }
    GLYPH_PACKET_COUNT
}

fn manhattan_position(
    anchor: (usize, usize),
    target: (usize, usize),
    progress_q16: u16,
    index: usize,
    width: usize,
    height: usize,
) -> (usize, usize) {
    let hash = mix32(index as u32 ^ 0xa54f_f53a);
    let bend_x = ((target.0 / 24) * 24 + (hash as usize & 7)).min(width.saturating_sub(1));
    let third = PROGRESS_MAX / 3;
    let (x, y) = if progress_q16 <= third {
        (
            lerp_usize(anchor.0, bend_x, scale_segment(progress_q16, third)),
            anchor.1,
        )
    } else if progress_q16 <= third.saturating_mul(2) {
        (
            bend_x,
            lerp_usize(
                anchor.1,
                target.1,
                scale_segment(progress_q16 - third, third),
            ),
        )
    } else {
        (
            lerp_usize(
                bend_x,
                target.0,
                scale_segment(
                    progress_q16 - third.saturating_mul(2),
                    PROGRESS_MAX - third.saturating_mul(2),
                ),
            ),
            target.1,
        )
    };
    (
        x.min(width.saturating_sub(1)),
        y.min(height.saturating_sub(1)),
    )
}

fn scale_segment(value: u16, span: u16) -> u16 {
    (value as u32 * PROGRESS_MAX as u32 / span.max(1) as u32).min(PROGRESS_MAX as u32) as u16
}

fn lerp_usize(from: usize, to: usize, progress_q16: u16) -> usize {
    let from = from as i64;
    let delta = to as i64 - from;
    (from + delta * progress_q16 as i64 / PROGRESS_MAX as i64).max(0) as usize
}

fn rect_center(rect: NavigationTransitionRect) -> (usize, usize) {
    (
        rect.x as usize + rect.width as usize / 2,
        rect.y as usize + rect.height as usize / 2,
    )
}

fn point_in_rect(x: usize, y: usize, rect: NavigationTransitionRect) -> bool {
    x >= rect.x as usize
        && y >= rect.y as usize
        && x < rect.right() as usize
        && y < rect.bottom() as usize
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
    fn packet_mask_contains_six_bounded_glyphs() {
        let mask = packet_target_mask(960, 540).expect("packet mask");
        assert!(mask.width() <= 960);
        assert!(mask.height() <= 540);
        assert!(!mask.points().is_empty());
        assert_eq!(GLYPH_PACKET_ROWS.len(), 6);
    }

    #[test]
    fn manhattan_trajectories_stay_inside_the_frame() {
        for index in 0..FULL_PARTICLE_COUNT {
            for progress in [0, 8_192, 21_845, 32_768, 43_690, PROGRESS_MAX] {
                let point = manhattan_position(
                    (470, 300),
                    (index % 960, index % 540),
                    progress,
                    index,
                    960,
                    540,
                );
                assert!(point.0 < 960);
                assert!(point.1 < 540);
            }
        }
    }

    #[test]
    fn fallback_keeps_the_same_formation_shape() {
        assert_eq!(normalize_particle_count(1), FALLBACK_PARTICLE_COUNT);
        assert_eq!(
            normalize_particle_count(FULL_PARTICLE_COUNT),
            FULL_PARTICLE_COUNT
        );
    }
}
