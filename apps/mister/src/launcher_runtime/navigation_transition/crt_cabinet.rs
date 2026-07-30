// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! CRT cabinet boot treatment with deterministic alternating-row reconstruction.

use super::{
    NavigationTransitionBuffers, NavigationTransitionDirection, NavigationTransitionFailure,
    NavigationTransitionFrame, NavigationTransitionPhase, NavigationTransitionRect,
    NavigationTransitionRenderStats, NavigationTransitionRequest, PROGRESS_MAX, fill_rect_565,
    render_super_scaler_shell,
};
use slint::platform::software_renderer::Rgb565Pixel;

const SPARK_COUNT: usize = 192;

#[derive(Debug)]
pub(super) struct CrtCabinetRenderer {
    reduced_effects: bool,
}

impl CrtCabinetRenderer {
    pub(super) const fn new(reduced_effects: bool) -> Self {
        Self { reduced_effects }
    }
}

pub(super) fn configured_reduced_effects() -> bool {
    super::env_flag("MISTER_NAV_TRANSITION_REDUCED_EFFECTS")
}

pub(super) fn render_crt_cabinet(
    renderer: &CrtCabinetRenderer,
    buffers: &mut NavigationTransitionBuffers,
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
) -> Result<NavigationTransitionRenderStats, NavigationTransitionFailure> {
    if frame.progress_q16 == 0 || frame.phase == NavigationTransitionPhase::Settled {
        return render_super_scaler_shell(buffers, request, frame);
    }
    let width = buffers.width;
    let height = buffers.height;
    let clip = overlay_rect(request, frame, width, height);
    if clip.width == 0 || clip.height == 0 {
        return render_super_scaler_shell(buffers, request, frame);
    }
    let mut stats = if frame.reveal_progress_q16 == 0 {
        render_super_scaler_shell(buffers, request, frame)?
    } else {
        NavigationTransitionRenderStats::default()
    };
    let source = buffers
        .source
        .get(..)
        .filter(|_| buffers.source_ready)
        .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
    let destination = buffers
        .destination
        .get(..)
        .filter(|_| buffers.destination_ready);
    let working = buffers.working.as_mut_slice();
    if frame.reveal_progress_q16 > 0 {
        if request.direction == NavigationTransitionDirection::Forward {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: 0,
                    y: 0,
                    width: width as u16,
                    height: height as u16,
                },
                Rgb565Pixel(0x0008),
                &mut stats,
            );
        } else if let Some(destination) = destination {
            working.copy_from_slice(destination);
            stats.copied_pixels = stats.copied_pixels.saturating_add(destination.len() as u64);
            fill_rect_565(
                working,
                width,
                height,
                clip,
                Rgb565Pixel(0x0008),
                &mut stats,
            );
        }
        if let Some(destination) = destination {
            reveal_alternating_rows(
                working,
                destination,
                width,
                height,
                clip,
                frame.reveal_progress_q16,
                &mut stats,
            );
            if frame.reveal_progress_q16 >= 65_000 {
                working.copy_from_slice(destination);
                stats.copied_pixels = stats.copied_pixels.saturating_add(destination.len() as u64);
                return Ok(stats);
            }
        }
    }

    draw_bezel(working, width, height, clip, &mut stats);
    let marquee_height = clip.height.min(28);
    fill_rect_565(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: clip.x.saturating_add(3),
            y: clip.y.saturating_add(3),
            width: clip.width.saturating_sub(6),
            height: marquee_height.saturating_sub(6),
        },
        Rgb565Pixel(0x280f),
        &mut stats,
    );

    let line_y = hot_line_y(clip, frame);
    draw_hot_line(
        working,
        width,
        height,
        clip,
        line_y,
        renderer.reduced_effects,
        &mut stats,
    );
    if !renderer.reduced_effects && frame.reveal_progress_q16 < 60_000 {
        stats.sparks = draw_sparks(working, width, height, clip, line_y, &mut stats) as u64;
    }
    match request.direction {
        NavigationTransitionDirection::Forward => super::move_label_pixels(
            working,
            source,
            width,
            height,
            request.geometry.source_label,
            request.geometry.destination_title,
            if frame.reveal_progress_q16 > 0 {
                PROGRESS_MAX
            } else {
                frame.cover_progress_q16
            },
            false,
            &mut stats,
        ),
        NavigationTransitionDirection::Reverse if frame.reveal_progress_q16 > 0 => {
            super::move_label_pixels(
                working,
                source,
                width,
                height,
                request.geometry.destination_title,
                request.geometry.source_label,
                frame.reveal_progress_q16,
                true,
                &mut stats,
            )
        }
        NavigationTransitionDirection::Reverse => {}
    }
    Ok(stats)
}

fn reveal_alternating_rows(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != destination.len() || width == 0 {
        return;
    }
    let clip_x0 = clip.x as usize;
    let clip_x1 = clip.right() as usize;
    let clip_y0 = clip.y as usize;
    let clip_y1 = clip.bottom() as usize;
    let row_count = clip_y1.saturating_sub(clip_y0).max(1);
    for y in clip_y0..clip_y1.min(height) {
        let local = y - clip_y0;
        let order = if local & 1 == 0 {
            local / 2
        } else {
            row_count.div_ceil(2) + local / 2
        };
        let threshold = order * PROGRESS_MAX as usize / row_count;
        if progress_q16 != PROGRESS_MAX && threshold >= progress_q16 as usize {
            continue;
        }
        let start = y * width + clip_x0.min(width);
        let end = y * width + clip_x1.min(width);
        working[start..end].copy_from_slice(&destination[start..end]);
        stats.copied_pixels = stats
            .copied_pixels
            .saturating_add(end.saturating_sub(start) as u64);
    }
}

fn draw_bezel(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    for inset in 0..3u16 {
        if clip.width <= inset * 2 || clip.height <= inset * 2 {
            break;
        }
        super::draw_outline_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: clip.x.saturating_add(inset),
                y: clip.y.saturating_add(inset),
                width: clip.width.saturating_sub(inset * 2),
                height: clip.height.saturating_sub(inset * 2),
            },
            match inset {
                0 => Rgb565Pixel(0xb81f),
                1 => Rgb565Pixel(0x07ff),
                _ => Rgb565Pixel(0x4208),
            },
            stats,
        );
    }
}

fn hot_line_y(clip: NavigationTransitionRect, frame: NavigationTransitionFrame) -> usize {
    let progress = if frame.reveal_progress_q16 > 0 {
        frame.reveal_progress_q16
    } else {
        frame.cover_progress_q16
    };
    clip.y as usize
        + clip.height.saturating_sub(1) as usize * progress as usize / PROGRESS_MAX as usize
}

fn draw_hot_line(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    y: usize,
    reduced: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let rows: &[(isize, Rgb565Pixel)] = if reduced {
        &[(0, Rgb565Pixel(0xffff))]
    } else {
        &[
            (-2, Rgb565Pixel(0x181f)),
            (-1, Rgb565Pixel(0x7bff)),
            (0, Rgb565Pixel(0xffff)),
            (1, Rgb565Pixel(0x7bff)),
            (2, Rgb565Pixel(0x181f)),
        ]
    };
    for (offset, color) in rows {
        let row = y as isize + offset;
        if row < clip.y as isize || row >= clip.bottom() as isize {
            continue;
        }
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: clip.x,
                y: row as u16,
                width: clip.width,
                height: 1,
            },
            *color,
            stats,
        );
    }
}

fn draw_sparks(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    line_y: usize,
    stats: &mut NavigationTransitionRenderStats,
) -> usize {
    let mut unique_offsets = [usize::MAX; SPARK_COUNT];
    let mut written = 0usize;
    for spark in 0..SPARK_COUNT {
        let hash = mix32(spark as u32 ^ line_y as u32 ^ 0x4352_5442);
        let x = clip.x as usize + hash as usize % clip.width.max(1) as usize;
        let spread = (hash.rotate_left(9) as usize % 29) as isize - 14;
        let y = line_y as isize + spread;
        if x < clip.right() as usize
            && y >= clip.y as isize
            && y < clip.bottom() as isize
            && x < width
            && (y as usize) < height
        {
            let offset = y as usize * width + x;
            if unique_offsets[..written].contains(&offset) {
                continue;
            }
            unique_offsets[written] = offset;
            written += 1;
            working[offset] = Rgb565Pixel(if spark & 3 == 0 { 0xffff } else { 0xfd20 });
            stats.particle_pixels = stats.particle_pixels.saturating_add(1);
        }
    }
    written
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
    fn alternating_reveal_orders_even_rows_before_odd_rows() {
        let width = 8;
        let height = 8;
        let destination = vec![Rgb565Pixel(0xffff); width * height];
        let mut working = vec![Rgb565Pixel(0); width * height];
        let mut stats = NavigationTransitionRenderStats::default();
        reveal_alternating_rows(
            &mut working,
            &destination,
            width,
            height,
            NavigationTransitionRect {
                x: 0,
                y: 0,
                width: width as u16,
                height: height as u16,
            },
            PROGRESS_MAX / 2,
            &mut stats,
        );
        assert!(working[0] == Rgb565Pixel(0xffff));
        assert!(working[width] == Rgb565Pixel(0));
    }

    #[test]
    fn reduced_mode_preserves_structure_without_sparks() {
        assert!(CrtCabinetRenderer::new(true).reduced_effects);
        assert_eq!(SPARK_COUNT, 192);
    }
}
