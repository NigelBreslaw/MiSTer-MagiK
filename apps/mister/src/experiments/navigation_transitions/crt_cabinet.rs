// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! A selected launcher card springs open into a CRT cabinet, then boots the
//! destination outward from a phosphor hot line. The renderer is expressed as
//! one canonical A -> B timeline so standalone Back navigation is its exact
//! visual inverse.

use super::{
    NavigationTransitionBuffers, NavigationTransitionDirection, NavigationTransitionEdge,
    NavigationTransitionEndpoint, NavigationTransitionFailure, NavigationTransitionFrame,
    NavigationTransitionGeometry, NavigationTransitionPhase, NavigationTransitionRect,
    NavigationTransitionRenderStats, NavigationTransitionRequest, PROGRESS_MAX, clip_rect_to_frame,
    draw_outline_565, ease_out_cubic_q16, fill_rect_565, lerp_rect, smoothstep_q16, window_q16,
};
use slint::platform::software_renderer::Rgb565Pixel;

const SPARK_POOL_SIZE: usize = 192;
const MAX_VISIBLE_SPARKS: usize = 64;
const MAX_SPANS: u64 = 1_500;

const VOID: Rgb565Pixel = Rgb565Pixel(0x0023);
const SHELL_SHADOW: Rgb565Pixel = Rgb565Pixel(0x0824);
const SHELL_OUTER: Rgb565Pixel = Rgb565Pixel(0x1067);
const SHELL_VIOLET: Rgb565Pixel = Rgb565Pixel(0x294a);
const SCREEN_DARK: Rgb565Pixel = Rgb565Pixel(0x0824);
const SCREEN_SCANLINE: Rgb565Pixel = Rgb565Pixel(0x0825);
const HEADER_DARK: Rgb565Pixel = Rgb565Pixel(0x0848);
const CYAN_DEEP: Rgb565Pixel = Rgb565Pixel(0x0370);
const CYAN: Rgb565Pixel = Rgb565Pixel(0x05d2);
const CYAN_WHITE: Rgb565Pixel = Rgb565Pixel(0x9ff7);
const CREAM: Rgb565Pixel = Rgb565Pixel(0xfed7);
const WHITE: Rgb565Pixel = Rgb565Pixel(0xffff);
const AMBER: Rgb565Pixel = Rgb565Pixel(0xfc80);

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
    let raw_source = buffers
        .source
        .get(..)
        .filter(|_| buffers.source_ready)
        .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
    let raw_destination = buffers
        .destination
        .get(..)
        .filter(|_| buffers.destination_ready);
    let width = buffers.width;
    let height = buffers.height;
    if raw_source.len() != width.saturating_mul(height) || buffers.working.len() != raw_source.len()
    {
        return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
    }

    let mut stats = NavigationTransitionRenderStats::default();
    if frame.phase == NavigationTransitionPhase::Settled {
        let endpoint = match frame.endpoint {
            Some(NavigationTransitionEndpoint::Destination) => {
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
    let (source, destination) = match request.direction {
        NavigationTransitionDirection::Forward => (Some(raw_source), raw_destination),
        NavigationTransitionDirection::Reverse => (raw_destination, Some(raw_source)),
    };
    if canonical <= 96 {
        let source = source.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        buffers.working.copy_from_slice(source);
        stats.copied_pixels = source.len() as u64;
        return Ok(stats);
    }
    if canonical >= PROGRESS_MAX.saturating_sub(96) {
        let destination = destination.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        buffers.working.copy_from_slice(destination);
        stats.copied_pixels = destination.len() as u64;
        return Ok(stats);
    }

    let working = buffers.working.as_mut_slice();
    if canonical < super::COVER_PROGRESS {
        let source = source.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        render_expand(
            renderer,
            working,
            source,
            width,
            height,
            request.geometry,
            canonical,
            &mut stats,
        );
    } else if canonical == super::COVER_PROGRESS {
        render_covered(
            renderer,
            working,
            width,
            height,
            request.geometry,
            &mut stats,
        );
    } else {
        let destination = destination.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        render_reveal(
            renderer,
            working,
            destination,
            width,
            height,
            request.edge,
            request.geometry,
            canonical,
            &mut stats,
        );
    }

    debug_assert!(
        stats.sparks <= SPARK_POOL_SIZE as u64,
        "CRT emitted {} sparks",
        stats.sparks
    );
    debug_assert!(
        stats.spans <= MAX_SPANS,
        "CRT emitted {} spans",
        stats.spans
    );
    Ok(stats)
}

fn canonical_progress(direction: NavigationTransitionDirection, progress_q16: u16) -> u16 {
    match direction {
        NavigationTransitionDirection::Forward => progress_q16,
        NavigationTransitionDirection::Reverse => PROGRESS_MAX.saturating_sub(progress_q16),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_expand(
    renderer: &CrtCabinetRenderer,
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    canonical: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    working.copy_from_slice(source);
    stats.copied_pixels = source.len() as u64;
    let cover = scale_segment(canonical, super::COVER_PROGRESS);

    if cover <= 2_400 {
        draw_focus_ignition(working, width, height, geometry.source_card, cover, stats);
        return;
    }

    if cover >= 55_300 {
        fill_rect_565(
            working,
            width,
            height,
            frame_rect(width, height),
            VOID,
            stats,
        );
        draw_procedural_header(working, width, height, stats);
    }

    for (index, delay) in [9_000u16, 6_000, 3_000].into_iter().enumerate() {
        let echo_cover = cover.saturating_sub(delay);
        if echo_cover <= 5_000 || cover >= 55_300u16.saturating_add(index as u16 * 2_400) {
            continue;
        }
        let echo = quantize_rect(
            spring_rect(geometry.source_card, echo_cover, width, height),
            4,
        );
        draw_stepped_contour(
            working,
            width,
            height,
            echo,
            if index == 2 {
                SHELL_VIOLET
            } else {
                SHELL_OUTER
            },
            stats,
        );
    }

    let cabinet = spring_rect(geometry.source_card, cover, width, height);
    draw_cabinet(
        working,
        width,
        height,
        cabinet,
        renderer.reduced_effects,
        stats,
    );

    let marquee = marquee_title_rect(cabinet, geometry);
    let title_handoff = smoothstep_q16(window_q16(cover, 38_300, 55_300));
    let title_carrier = lerp_rect(marquee, geometry.destination_title, title_handoff);
    let source_to_rom = smoothstep_q16(window_q16(cover, 7_000, 18_000));
    let title_lift = smoothstep_q16(window_q16(cover, 4_300, 34_000));
    if source_to_rom < PROGRESS_MAX {
        move_source_text_faded(
            working,
            source,
            width,
            height,
            geometry.source_label,
            title_carrier,
            title_lift,
            PROGRESS_MAX.saturating_sub(source_to_rom),
            true,
            stats,
        );
    }
    if source_to_rom > 0 {
        draw_canonical_title_faded(
            working,
            width,
            height,
            title_carrier,
            geometry,
            if cover < 48_000 { CREAM } else { CYAN_WHITE },
            source_to_rom,
            stats,
        );
    }

    if cover < 48_000 && geometry.source_detail.fits(width, height) {
        let detail_target = detail_rect_below(title_carrier, cabinet);
        let detail_lift = smoothstep_q16(window_q16(cover, 5_500, 38_000));
        move_source_text_faded(
            working,
            source,
            width,
            height,
            geometry.source_detail,
            detail_target,
            detail_lift,
            fade_out(cover, 34_000, 46_000),
            false,
            stats,
        );
    }

    if cover >= 51_100 {
        let line_strength = window_q16(cover, 51_100, PROGRESS_MAX);
        let screen = cabinet_screen_rect(cabinet);
        draw_hot_line_pair(
            working,
            width,
            height,
            screen,
            screen.y as usize + screen.height as usize / 2,
            screen.y as usize + screen.height as usize / 2,
            line_strength,
            renderer.reduced_effects,
            stats,
        );
    }
}

fn render_covered(
    renderer: &CrtCabinetRenderer,
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    stats: &mut NavigationTransitionRenderStats,
) {
    render_cabinet_base(renderer, working, width, height, stats);
    draw_canonical_title(
        working,
        width,
        height,
        geometry.destination_title,
        geometry,
        CYAN_WHITE,
        stats,
    );
    let screen = cabinet_screen_rect(cabinet_stage_rect(width, height));
    let center = screen.y as usize + screen.height as usize / 2;
    draw_hot_line_pair(
        working,
        width,
        height,
        screen,
        center,
        center,
        PROGRESS_MAX,
        renderer.reduced_effects,
        stats,
    );
    if !renderer.reduced_effects {
        stats.sparks = draw_sparks(
            working,
            width,
            height,
            screen,
            center,
            center,
            geometry.destination_title,
            0,
            12,
            stats,
        ) as u64;
    }
}

fn render_cabinet_base(
    renderer: &CrtCabinetRenderer,
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    stats: &mut NavigationTransitionRenderStats,
) {
    fill_rect_565(
        working,
        width,
        height,
        frame_rect(width, height),
        VOID,
        stats,
    );
    draw_procedural_header(working, width, height, stats);
    let cabinet = cabinet_stage_rect(width, height);
    draw_cabinet(
        working,
        width,
        height,
        cabinet,
        renderer.reduced_effects,
        stats,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_reveal(
    renderer: &CrtCabinetRenderer,
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    edge: NavigationTransitionEdge,
    geometry: NavigationTransitionGeometry,
    canonical: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let reveal = scale_from_segment(canonical, super::COVER_PROGRESS);
    if reveal >= 63_800 {
        working.copy_from_slice(destination);
        stats.copied_pixels = destination.len() as u64;
        return;
    }

    // The reveal owns its moving phosphor fronts and sparks. Retaining the
    // covered-phase centre line would leave an unrelated third beam behind
    // them and spend row-span budget on a visual artifact.
    render_cabinet_base(renderer, working, width, height, stats);
    let header = NavigationTransitionRect {
        x: 0,
        y: 0,
        width: width.min(u16::MAX as usize) as u16,
        height: height.min(56).min(u16::MAX as usize) as u16,
    };
    copy_rect(working, destination, width, height, header, stats);
    let carrier_opacity = fade_out(reveal, 0, 10_000);
    if carrier_opacity > 0 {
        draw_canonical_title_faded(
            working,
            width,
            height,
            geometry.destination_title,
            geometry,
            CYAN_WHITE,
            carrier_opacity,
            stats,
        );
    }

    if edge == NavigationTransitionEdge::HomeToConsoles {
        reveal_category(
            renderer,
            working,
            destination,
            width,
            height,
            geometry,
            reveal,
            stats,
        );
    } else {
        reveal_system(
            renderer,
            working,
            destination,
            width,
            height,
            geometry,
            reveal,
            stats,
        );
    }

    if reveal >= 59_500 {
        working.copy_from_slice(destination);
        stats.copied_pixels = stats.copied_pixels.saturating_add(destination.len() as u64);
        if reveal < 62_300 {
            draw_focus_afterglow(working, width, height, edge, geometry, stats);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn reveal_category(
    renderer: &CrtCabinetRenderer,
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    reveal: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let screen = cabinet_screen_rect(cabinet_stage_rect(width, height));
    let card_count = 5usize;
    // The category destination opens with Atari selected. The source card's
    // horizontal slot describes the Home layout and is not destination
    // selection geometry.
    let selected = 0usize;
    let progress = smoothstep_q16(window_q16(reveal, 5_000, 48_000));
    let (beam_top, beam_bottom) = if progress >= 34_000 {
        copy_rect(working, destination, width, height, screen, stats);
        (
            screen.y as usize,
            screen.bottom().saturating_sub(1) as usize,
        )
    } else {
        reveal_region_from_center(
            working,
            destination,
            width,
            height,
            screen,
            progress,
            0,
            stats,
        )
    };
    for column in 0..card_count {
        let distance = column.abs_diff(selected);
        let order = if column == selected {
            0
        } else {
            distance.saturating_mul(2).saturating_sub(1) + usize::from(column > selected)
        };
        let delay = (order * 2_600).min(10_400) as u16;
        let x0 = screen.x as usize + screen.width as usize * column / card_count;
        let x1 = screen.x as usize + screen.width as usize * (column + 1) / card_count;
        let column_rect = NavigationTransitionRect {
            x: x0 as u16,
            y: screen.y,
            width: x1.saturating_sub(x0) as u16,
            height: screen.height,
        };
        if !renderer.reduced_effects && reveal >= 24_000 + delay {
            draw_marquee_glint(
                working,
                width,
                height,
                column_rect,
                reveal.saturating_sub(24_000 + delay),
                column == selected,
                stats,
            );
        }
    }
    draw_hot_line_pair(
        working,
        width,
        height,
        screen,
        beam_top,
        beam_bottom,
        fade_out(reveal, 48_000, 58_000),
        renderer.reduced_effects,
        stats,
    );
    if !renderer.reduced_effects && reveal < 52_000 {
        stats.sparks = draw_sparks(
            working,
            width,
            height,
            screen,
            beam_top,
            beam_bottom,
            geometry.destination_title,
            reveal,
            24,
            stats,
        ) as u64;
    }
}

#[allow(clippy::too_many_arguments)]
fn reveal_system(
    renderer: &CrtCabinetRenderer,
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    reveal: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let list = clip_rect_to_frame(geometry.destination_list, width, height)
        .unwrap_or_else(|| cabinet_screen_rect(cabinet_stage_rect(width, height)));
    let destination_stage = NavigationTransitionRect {
        x: list.x,
        y: list.y,
        width: width
            .saturating_sub(list.x as usize)
            .saturating_sub(8)
            .min(u16::MAX as usize) as u16,
        height: list.height,
    };
    // Assemble the real two-panel destination first. This is intentionally
    // much faster than the screenshot boot: the list must feel locked before
    // the preview aperture comes alive.
    let stage_progress = smoothstep_q16(window_q16(reveal, 1_000, 17_000));
    reveal_region_from_center(
        working,
        destination,
        width,
        height,
        destination_stage,
        stage_progress,
        0,
        stats,
    );
    if reveal >= 5_000
        && let Some(selected) = clip_rect_to_frame(geometry.destination_selected_row, width, height)
    {
        copy_rect(working, destination, width, height, selected, stats);
    }

    if let Some(preview) = clip_rect_to_frame(geometry.destination_preview, width, height) {
        // The black glass and bezel are structural, so they appear with the
        // panel. Only the screenshot pixels are delayed.
        fill_rect_565(working, width, height, preview, SCREEN_DARK, stats);
        draw_preview_bezel(working, width, height, preview, stats);
        let inner = inset_rect(preview, 4, 4);
        let preview_progress = smoothstep_q16(window_q16(reveal, 20_000, 56_000));
        let (top, bottom) = if preview_progress > 0 {
            reveal_region_from_center(
                working,
                destination,
                width,
                height,
                inner,
                preview_progress,
                1,
                stats,
            )
        } else {
            let center = inner.y as usize + inner.height as usize / 2;
            (center, center)
        };
        if reveal >= 16_000 {
            let ignition = window_q16(reveal, 16_000, 21_000);
            let decay = fade_out(reveal, 52_000, 59_000);
            draw_hot_line_pair(
                working,
                width,
                height,
                inner,
                top,
                bottom,
                ignition.min(decay),
                renderer.reduced_effects,
                stats,
            );
            if !renderer.reduced_effects && reveal < 56_000 {
                stats.sparks = draw_sparks(
                    working,
                    width,
                    height,
                    inner,
                    top,
                    bottom,
                    geometry.destination_title,
                    reveal,
                    12,
                    stats,
                ) as u64;
            }
        }
        if !renderer.reduced_effects && (14_000..50_000).contains(&reveal) {
            draw_coin_star(
                working,
                width,
                height,
                preview,
                geometry.destination_selected_row,
                reveal,
                stats,
            );
        }
    }
    if reveal >= 10_000
        && let Some(footer) = clip_rect_to_frame(geometry.destination_footer, width, height)
    {
        copy_rect(working, destination, width, height, footer, stats);
    }
}

fn spring_rect(
    source: NavigationTransitionRect,
    cover: u16,
    width: usize,
    height: usize,
) -> NavigationTransitionRect {
    let full = cabinet_stage_rect(width, height);
    let ninety_two = lerp_rect(source, full, 60_300);
    let overshoot = expand_rect(full, 8, 6, width, height);
    let recoil = inset_rect(full, 5, 3);
    match cover {
        0..=8_500 => {
            let progress = smoothstep_q16(window_q16(cover, 0, 8_500));
            let three_percent = lerp_rect(source, full, 1_966);
            let mut rect = lerp_rect(source, three_percent, progress);
            let pinch = triangle_q16(progress) as u32;
            let dx = (source.width as u32 * pinch / PROGRESS_MAX as u32 / 50) as u16;
            rect.x = rect.x.saturating_add(dx / 2);
            rect.width = rect.width.saturating_sub(dx);
            rect
        }
        8_501..=34_000 => lerp_rect(
            source,
            ninety_two,
            ease_out_cubic_q16(window_q16(cover, 8_500, 34_000)),
        ),
        34_001..=38_300 => lerp_rect(
            ninety_two,
            overshoot,
            smoothstep_q16(window_q16(cover, 34_000, 38_300)),
        ),
        38_301..=42_600 => lerp_rect(
            overshoot,
            recoil,
            smoothstep_q16(window_q16(cover, 38_300, 42_600)),
        ),
        42_601..=51_100 => lerp_rect(
            recoil,
            full,
            smoothstep_q16(window_q16(cover, 42_600, 51_100)),
        ),
        _ => full,
    }
}

fn triangle_q16(progress: u16) -> u16 {
    if progress <= PROGRESS_MAX / 2 {
        progress.saturating_mul(2)
    } else {
        PROGRESS_MAX.saturating_sub(progress).saturating_mul(2)
    }
}

fn cabinet_stage_rect(width: usize, height: usize) -> NavigationTransitionRect {
    let x = width.min(10);
    let y = height.min(58);
    NavigationTransitionRect {
        x: x as u16,
        y: y as u16,
        width: width
            .saturating_sub(x.saturating_mul(2))
            .min(u16::MAX as usize) as u16,
        height: height
            .saturating_sub(y.saturating_add(10))
            .min(u16::MAX as usize) as u16,
    }
}

fn cabinet_screen_rect(cabinet: NavigationTransitionRect) -> NavigationTransitionRect {
    inset_rect(cabinet, 11, 11)
}

fn marquee_title_rect(
    cabinet: NavigationTransitionRect,
    geometry: NavigationTransitionGeometry,
) -> NavigationTransitionRect {
    let length = usize::from(geometry.label_len).max(1);
    let width = (length * 30)
        .clamp(96, 280)
        .min(cabinet.width.saturating_sub(24) as usize);
    let height = 38usize.min(cabinet.height.saturating_sub(12) as usize);
    NavigationTransitionRect {
        x: cabinet
            .x
            .saturating_add(cabinet.width.saturating_sub(width as u16) / 2),
        y: cabinet
            .y
            .saturating_add(cabinet.height.saturating_sub(height as u16) / 2),
        width: width as u16,
        height: height as u16,
    }
}

fn detail_rect_below(
    title: NavigationTransitionRect,
    cabinet: NavigationTransitionRect,
) -> NavigationTransitionRect {
    NavigationTransitionRect {
        x: title.x,
        y: title
            .bottom()
            .saturating_add(5)
            .min(cabinet.bottom().saturating_sub(10)),
        width: title.width,
        height: 10,
    }
}

#[allow(clippy::too_many_arguments)]
fn move_source_text_faded(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    from: NavigationTransitionRect,
    to: NavigationTransitionRect,
    progress_q16: u16,
    opacity_q16: u16,
    enlarge: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some((content, background)) = super::opaque_content_bounds(source, width, height, from)
    else {
        return;
    };
    let target_height = if enlarge {
        (content.height as u32 * 3 / 2)
            .min(to.height.max(1) as u32)
            .max(1) as u16
    } else {
        content.height.min(to.height.max(1))
    };
    let target_width = ((content.width as u32 * target_height as u32)
        / content.height.max(1) as u32)
        .min(to.width.max(1) as u32)
        .max(1) as u16;
    let target = NavigationTransitionRect {
        x: to
            .x
            .saturating_add(to.width.saturating_sub(target_width) / 2),
        y: to
            .y
            .saturating_add(to.height.saturating_sub(target_height) / 2),
        width: target_width,
        height: target_height,
    };
    let moving = lerp_rect(content, target, progress_q16);
    super::blit_scaled_masked_dithered_565(
        working,
        source,
        width,
        height,
        content,
        moving,
        background,
        opacity_q16,
        stats,
    );
}

fn draw_focus_ignition(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    card: NavigationTransitionRect,
    cover: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    draw_outline_565(
        working,
        width,
        height,
        card,
        if cover < 1_200 { WHITE } else { CYAN },
        stats,
    );
    if cover >= 1_200 {
        draw_outline_565(working, width, height, inset_rect(card, 2, 2), CREAM, stats);
    }
}

fn draw_cabinet(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    cabinet: NavigationTransitionRect,
    reduced: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let shadow = offset_rect(cabinet, 4, 4, width, height);
    fill_rect_565(working, width, height, shadow, SHELL_SHADOW, stats);
    fill_rect_565(working, width, height, cabinet, SHELL_OUTER, stats);
    draw_thick_outline(working, width, height, cabinet, SHELL_VIOLET, 1, stats);
    let cyan_band = inset_rect(cabinet, 3, 3);
    draw_thick_outline(working, width, height, cyan_band, CYAN_DEEP, 1, stats);
    let cream_rim = inset_rect(cabinet, 7, 7);
    draw_outline_565(working, width, height, cream_rim, CREAM, stats);
    let screen = cabinet_screen_rect(cabinet);
    fill_scanline_glass(working, width, height, screen, stats);
    draw_outline_565(
        working,
        width,
        height,
        screen,
        if reduced { SHELL_VIOLET } else { CYAN_DEEP },
        stats,
    );
}

fn draw_preview_bezel(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    preview: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    draw_thick_outline(working, width, height, preview, SHELL_VIOLET, 2, stats);
    draw_outline_565(
        working,
        width,
        height,
        inset_rect(preview, 2, 2),
        CYAN,
        stats,
    );
    draw_outline_565(
        working,
        width,
        height,
        inset_rect(preview, 3, 3),
        CREAM,
        stats,
    );
}

fn draw_procedural_header(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    stats: &mut NavigationTransitionRenderStats,
) {
    let header = NavigationTransitionRect {
        x: 10.min(width) as u16,
        y: 8.min(height) as u16,
        width: width.saturating_sub(20.min(width)).min(u16::MAX as usize) as u16,
        height: height.saturating_sub(8).min(42) as u16,
    };
    fill_rect_565(working, width, height, header, HEADER_DARK, stats);
    draw_outline_565(working, width, height, header, SHELL_VIOLET, stats);
    draw_outline_565(
        working,
        width,
        height,
        inset_rect(header, 1, 1),
        CYAN_DEEP,
        stats,
    );
}

fn fill_scanline_glass(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    fill_rect_565(working, width, height, rect, SCREEN_DARK, stats);
    // Keep the CRT cadence but make the luminance delta tiny; the hot line,
    // bezel and real destination content must remain the visual hierarchy.
    for y in (rect.y as usize..rect.bottom() as usize).step_by(4) {
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: rect.x,
                y: y as u16,
                width: rect.width,
                height: 1,
            },
            SCREEN_SCANLINE,
            stats,
        );
        stats.spans = stats.spans.saturating_add(1);
    }
}

fn draw_stepped_contour(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let step = 12usize;
    for x in (rect.x as usize..rect.right() as usize).step_by(step) {
        let segment = (step - 2).min(rect.right() as usize - x);
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: x as u16,
                y: rect.y,
                width: segment as u16,
                height: 1,
            },
            color,
            stats,
        );
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: x as u16,
                y: rect.bottom().saturating_sub(1),
                width: segment as u16,
                height: 1,
            },
            color,
            stats,
        );
        stats.spans = stats.spans.saturating_add(2);
    }
    for y in (rect.y as usize..rect.bottom() as usize).step_by(step) {
        let segment = (step - 2).min(rect.bottom() as usize - y);
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: rect.x,
                y: y as u16,
                width: 1,
                height: segment as u16,
            },
            color,
            stats,
        );
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: rect.right().saturating_sub(1),
                y: y as u16,
                width: 1,
                height: segment as u16,
            },
            color,
            stats,
        );
        stats.spans = stats.spans.saturating_add(2);
    }
}

fn draw_thick_outline(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    color: Rgb565Pixel,
    thickness: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    for inset in 0..thickness {
        let current = inset_rect(rect, inset, inset);
        if current.width == 0 || current.height == 0 {
            break;
        }
        draw_outline_565(working, width, height, current, color, stats);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_hot_line_pair(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    top: usize,
    bottom: usize,
    strength_q16: u16,
    reduced: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    if strength_q16 == 0 || clip.width == 0 || clip.height == 0 {
        return;
    }
    draw_hot_line(
        working,
        width,
        height,
        clip,
        top,
        strength_q16,
        reduced,
        stats,
    );
    if bottom.abs_diff(top) > 8 {
        draw_hot_line(
            working,
            width,
            height,
            clip,
            bottom,
            strength_q16,
            reduced,
            stats,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_hot_line(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    y: usize,
    strength_q16: u16,
    reduced: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let core = if strength_q16 > 38_000 {
        WHITE
    } else {
        CYAN_WHITE
    };
    if !reduced && strength_q16 > 16_000 {
        for (offset, color) in [(-2isize, CYAN_DEEP), (-1, CYAN), (2, CYAN_DEEP), (3, CYAN)] {
            let row = y as isize + offset;
            if row >= clip.y as isize && row < clip.bottom() as isize {
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
                    color,
                    stats,
                );
                stats.spans = stats.spans.saturating_add(1);
            }
        }
    }
    for row in [y, y.saturating_add(1)] {
        if row >= clip.y as usize && row < clip.bottom() as usize {
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
                core,
                stats,
            );
            stats.spans = stats.spans.saturating_add(1);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn reveal_region_from_center(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    parity: usize,
    stats: &mut NavigationTransitionRenderStats,
) -> (usize, usize) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return (0, 0);
    };
    let center = rect.y as usize + rect.height as usize / 2;
    if progress_q16 == 0 {
        return (center, center);
    }
    let half = rect.height as usize / 2 + 1;
    let radius = half.saturating_mul(progress_q16 as usize) / PROGRESS_MAX as usize;
    let front_band = 8usize;
    let top = center.saturating_sub(radius).max(rect.y as usize);
    let bottom = center
        .saturating_add(radius)
        .min(rect.bottom().saturating_sub(1) as usize);
    for y in top..=bottom.min(height.saturating_sub(1)) {
        let distance = y.abs_diff(center);
        let exact = distance.saturating_add(front_band) < radius || progress_q16 >= 62_000;
        if exact || ((y + parity) & 1 == 0) {
            copy_row_segment(
                working,
                destination,
                width,
                height,
                y,
                rect.x as usize,
                rect.right() as usize,
                stats,
            );
        }
    }
    (top, bottom)
}

fn copy_row_segment(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    y: usize,
    x0: usize,
    x1: usize,
    stats: &mut NavigationTransitionRenderStats,
) {
    if working.len() != source.len() || working.len() != width.saturating_mul(height) || y >= height
    {
        return;
    }
    let x0 = x0.min(width);
    let x1 = x1.min(width);
    if x1 <= x0 {
        return;
    }
    let start = y * width + x0;
    let end = y * width + x1;
    working[start..end].copy_from_slice(&source[start..end]);
    stats.copied_pixels = stats
        .copied_pixels
        .saturating_add(end.saturating_sub(start) as u64);
    stats.spans = stats.spans.saturating_add(1);
}

fn copy_rect(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    for y in rect.y as usize..rect.bottom() as usize {
        copy_row_segment(
            working,
            source,
            width,
            height,
            y,
            rect.x as usize,
            rect.right() as usize,
            stats,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_sparks(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    top: usize,
    bottom: usize,
    excluded: NavigationTransitionRect,
    phase: u16,
    max_visible: usize,
    stats: &mut NavigationTransitionRenderStats,
) -> usize {
    let mut written = 0usize;
    for seed in 0..SPARK_POOL_SIZE {
        let hash = mix32(seed as u32 ^ u32::from(phase) ^ 0x4352_5442);
        if hash as usize % 3 != (usize::from(phase) / 2_048) % 3 {
            continue;
        }
        let x = clip.x as usize + hash as usize % clip.width.max(1) as usize;
        let line = if hash & 0x1000 == 0 { top } else { bottom };
        let spread = (hash.rotate_left(9) as usize % 15) as isize - 7;
        let y = line as isize + spread;
        if x >= clip.right() as usize
            || y < clip.y as isize
            || y >= clip.bottom() as isize
            || x >= width
            || y < 0
            || y as usize >= height
            || point_in_rect(x, y as usize, excluded)
        {
            continue;
        }
        working[y as usize * width + x] = if hash & 7 == 0 { WHITE } else { AMBER };
        stats.particle_pixels = stats.particle_pixels.saturating_add(1);
        written += 1;
        if written >= max_visible.min(MAX_VISIBLE_SPARKS) {
            break;
        }
    }
    written
}

fn draw_marquee_glint(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    card: NavigationTransitionRect,
    progress: u16,
    selected: bool,
    stats: &mut NavigationTransitionRenderStats,
) {
    let travel = smoothstep_q16(window_q16(progress, 0, 8_000));
    let x = card.x as usize
        + card.width.saturating_sub(2) as usize * travel as usize / PROGRESS_MAX as usize;
    let y = card.y as usize + 2;
    draw_star(
        working,
        width,
        height,
        x,
        y,
        if selected { 4 } else { 3 },
        if selected { WHITE } else { AMBER },
        stats,
    );
}

fn draw_coin_star(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    preview: NavigationTransitionRect,
    selected: NavigationTransitionRect,
    reveal: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let travel = smoothstep_q16(window_q16(reveal, 18_000, 47_000));
    let start_x = preview.x as usize + preview.width as usize / 2;
    let start_y = preview.y as usize + preview.height as usize / 2;
    let target_x = preview.x.saturating_sub(8) as usize;
    let target_y = preview
        .bottom()
        .saturating_sub(48)
        .max(selected.y)
        .min(preview.bottom().saturating_sub(4)) as usize;
    let x = lerp_usize(start_x, target_x, travel);
    let mut y = lerp_usize(start_y, target_y, travel);
    let arc = triangle_q16(travel) as usize * 42 / PROGRESS_MAX as usize;
    y = y.saturating_sub(arc);
    let size = if reveal < 43_000 { 4 } else { 5 };
    draw_star(working, width, height, x, y, size, AMBER, stats);
    stats.glyph_packets = stats.glyph_packets.saturating_add(1);
}

#[allow(clippy::too_many_arguments)]
fn draw_star(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    radius: usize,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    if width == 0 || height == 0 {
        return;
    }
    for distance in 0..=radius {
        for (px, py) in [
            (x.saturating_sub(distance), y),
            (x.saturating_add(distance), y),
            (x, y.saturating_sub(distance)),
            (x, y.saturating_add(distance)),
        ] {
            if px < width && py < height {
                working[py * width + px] = color;
                stats.particle_pixels = stats.particle_pixels.saturating_add(1);
            }
        }
    }
    if x > 0 && y > 0 && x + 1 < width && y + 1 < height {
        for (px, py) in [
            (x - 1, y - 1),
            (x + 1, y - 1),
            (x - 1, y + 1),
            (x + 1, y + 1),
        ] {
            working[py * width + px] = CREAM;
            stats.particle_pixels = stats.particle_pixels.saturating_add(1);
        }
    }
}

fn draw_focus_afterglow(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    edge: NavigationTransitionEdge,
    geometry: NavigationTransitionGeometry,
    stats: &mut NavigationTransitionRenderStats,
) {
    let rect = if edge.enters_system_browser() {
        geometry.destination_selected_row
    } else {
        category_focus_rect(width, height, 0)
    };
    draw_outline_565(working, width, height, rect, CYAN_DEEP, stats);
}

fn category_focus_rect(width: usize, height: usize, slot: usize) -> NavigationTransitionRect {
    let cards = 5usize;
    let slot = slot.min(cards - 1);
    let stage = cabinet_screen_rect(cabinet_stage_rect(width, height));
    let x0 = stage.x as usize + stage.width as usize * slot / cards;
    let x1 = stage.x as usize + stage.width as usize * (slot + 1) / cards;
    NavigationTransitionRect {
        x: x0 as u16,
        y: stage.y,
        width: x1.saturating_sub(x0) as u16,
        height: stage.height,
    }
}

fn draw_canonical_title(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    geometry: NavigationTransitionGeometry,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    draw_canonical_title_faded(
        working,
        width,
        height,
        rect,
        geometry,
        color,
        PROGRESS_MAX,
        stats,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_canonical_title_faded(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    geometry: NavigationTransitionGeometry,
    color: Rgb565Pixel,
    opacity_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let length = usize::from(geometry.label_len).min(geometry.label_ascii.len());
    if length == 0 || rect.width == 0 || rect.height == 0 || opacity_q16 == 0 {
        return;
    }
    let glyph_width = length.saturating_mul(6).saturating_sub(1).max(1);
    let scale_x = rect.width as usize / glyph_width;
    let scale_y = rect.height as usize / 7;
    let scale = scale_x.min(scale_y).clamp(1, 5);
    let text_width = glyph_width.saturating_mul(scale);
    let text_height = 7usize.saturating_mul(scale);
    let origin_x = rect.x as usize + (rect.width as usize).saturating_sub(text_width) / 2;
    let origin_y = rect.y as usize + (rect.height as usize).saturating_sub(text_height) / 2;
    for (index, byte) in geometry.label_ascii[..length].iter().copied().enumerate() {
        let starts_word = index == 0 || geometry.label_ascii[index.saturating_sub(1)] == b' ';
        for (row, bits) in glyph5x7_title(byte, !starts_word)
            .iter()
            .copied()
            .enumerate()
        {
            let mut run_start = None;
            for column in 0..=5usize {
                let active = column < 5 && bits & (1 << (4 - column)) != 0;
                match (run_start, active) {
                    (None, true) => run_start = Some(column),
                    (Some(start), false) => {
                        let run = NavigationTransitionRect {
                            x: (origin_x + (index * 6 + start) * scale) as u16,
                            y: (origin_y + row * scale) as u16,
                            width: ((column - start) * scale) as u16,
                            height: scale as u16,
                        };
                        if opacity_q16 == PROGRESS_MAX {
                            fill_rect_565(working, width, height, run, color, stats);
                            stats.spans = stats.spans.saturating_add(scale as u64);
                        } else {
                            draw_dithered_rect(
                                working,
                                width,
                                height,
                                run,
                                color,
                                opacity_q16,
                                stats,
                            );
                        }
                        run_start = None;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn draw_dithered_rect(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    color: Rgb565Pixel,
    opacity_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    const DITHER: [[u16; 4]; 4] = [
        [0, 32_768, 8_192, 40_960],
        [49_152, 16_384, 57_344, 24_576],
        [12_288, 45_056, 4_096, 36_864],
        [61_440, 28_672, 53_248, 20_480],
    ];
    for y in rect.y as usize..rect.bottom() as usize {
        if y >= height {
            break;
        }
        for x in rect.x as usize..rect.right() as usize {
            if x < width && DITHER[y & 3][x & 3] < opacity_q16 {
                working[y * width + x] = color;
                stats.filled_pixels = stats.filled_pixels.saturating_add(1);
            }
        }
    }
}

fn glyph5x7_title(character: u8, lowercase: bool) -> [u8; 7] {
    if lowercase {
        match character {
            b'A' => [0x00, 0x00, 0x0e, 0x01, 0x0f, 0x11, 0x0f],
            b'B' => [0x10, 0x10, 0x1e, 0x11, 0x11, 0x11, 0x1e],
            b'C' => [0x00, 0x00, 0x0f, 0x10, 0x10, 0x10, 0x0f],
            b'D' => [0x01, 0x01, 0x0f, 0x11, 0x11, 0x11, 0x0f],
            b'E' => [0x00, 0x00, 0x0e, 0x11, 0x1f, 0x10, 0x0f],
            b'F' => [0x06, 0x08, 0x1e, 0x08, 0x08, 0x08, 0x08],
            b'G' => [0x00, 0x00, 0x0f, 0x11, 0x0f, 0x01, 0x0e],
            b'H' => [0x10, 0x10, 0x1e, 0x11, 0x11, 0x11, 0x11],
            b'I' => [0x04, 0x00, 0x0c, 0x04, 0x04, 0x04, 0x0e],
            b'J' => [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0c],
            b'K' => [0x10, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
            b'L' => [0x0c, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
            b'M' => [0x00, 0x00, 0x1a, 0x15, 0x15, 0x15, 0x15],
            b'N' => [0x00, 0x00, 0x1e, 0x11, 0x11, 0x11, 0x11],
            b'O' => [0x00, 0x00, 0x0e, 0x11, 0x11, 0x11, 0x0e],
            b'P' => [0x00, 0x00, 0x1e, 0x11, 0x1e, 0x10, 0x10],
            b'Q' => [0x00, 0x00, 0x0f, 0x11, 0x0f, 0x01, 0x01],
            b'R' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
            b'S' => [0x00, 0x00, 0x0f, 0x10, 0x0e, 0x01, 0x1e],
            b'T' => [0x08, 0x08, 0x1e, 0x08, 0x08, 0x09, 0x06],
            b'U' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0d],
            b'V' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x0a, 0x04],
            b'W' => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0a],
            b'X' => [0x00, 0x00, 0x11, 0x0a, 0x04, 0x0a, 0x11],
            b'Y' => [0x00, 0x00, 0x11, 0x11, 0x0f, 0x01, 0x0e],
            b'Z' => [0x00, 0x00, 0x1f, 0x02, 0x04, 0x08, 0x1f],
            _ => glyph5x7(character),
        }
    } else {
        glyph5x7(character)
    }
}

fn glyph5x7(character: u8) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        b'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        b'C' => [0x0f, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0f],
        b'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        b'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        b'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        b'G' => [0x0f, 0x10, 0x10, 0x13, 0x11, 0x11, 0x0f],
        b'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'I' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1f],
        b'J' => [0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0e],
        b'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        b'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        b'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        b'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        b'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        b'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        b'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        b'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        b'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        b'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        b'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        b'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        b'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11],
        b'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        b'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        b'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        b'0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        b'1' => [0x04, 0x0c, 0x14, 0x04, 0x04, 0x04, 0x1f],
        b'2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        b'3' => [0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e],
        b'4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        b'5' => [0x1f, 0x10, 0x10, 0x1e, 0x01, 0x01, 0x1e],
        b'6' => [0x0e, 0x10, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        b'7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        b'8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        b'9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x01, 0x0e],
        b'-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        b'/' => [0x01, 0x02, 0x04, 0x08, 0x10, 0x00, 0x00],
        b'.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        b' ' => [0; 7],
        _ => [0x1f, 0x11, 0x02, 0x04, 0x04, 0x00, 0x04],
    }
}

fn fade_out(progress: u16, start: u16, end: u16) -> u16 {
    PROGRESS_MAX.saturating_sub(smoothstep_q16(window_q16(progress, start, end)))
}

fn scale_segment(value: u16, end: u16) -> u16 {
    if end == 0 {
        return PROGRESS_MAX;
    }
    (u32::from(value.min(end)) * u32::from(PROGRESS_MAX) / u32::from(end)) as u16
}

fn scale_from_segment(value: u16, start: u16) -> u16 {
    if value <= start {
        return 0;
    }
    ((value - start) as u32 * PROGRESS_MAX as u32
        / PROGRESS_MAX.saturating_sub(start).max(1) as u32) as u16
}

fn frame_rect(width: usize, height: usize) -> NavigationTransitionRect {
    NavigationTransitionRect {
        x: 0,
        y: 0,
        width: width.min(u16::MAX as usize) as u16,
        height: height.min(u16::MAX as usize) as u16,
    }
}

fn inset_rect(
    rect: NavigationTransitionRect,
    horizontal: u16,
    vertical: u16,
) -> NavigationTransitionRect {
    NavigationTransitionRect {
        x: rect.x.saturating_add(horizontal),
        y: rect.y.saturating_add(vertical),
        width: rect.width.saturating_sub(horizontal.saturating_mul(2)),
        height: rect.height.saturating_sub(vertical.saturating_mul(2)),
    }
}

fn expand_rect(
    rect: NavigationTransitionRect,
    horizontal: u16,
    vertical: u16,
    width: usize,
    height: usize,
) -> NavigationTransitionRect {
    clip_rect_to_frame(
        NavigationTransitionRect {
            x: rect.x.saturating_sub(horizontal),
            y: rect.y.saturating_sub(vertical),
            width: rect.width.saturating_add(horizontal.saturating_mul(2)),
            height: rect.height.saturating_add(vertical.saturating_mul(2)),
        },
        width,
        height,
    )
    .unwrap_or_default()
}

fn offset_rect(
    rect: NavigationTransitionRect,
    x: u16,
    y: u16,
    width: usize,
    height: usize,
) -> NavigationTransitionRect {
    clip_rect_to_frame(
        NavigationTransitionRect {
            x: rect.x.saturating_add(x),
            y: rect.y.saturating_add(y),
            ..rect
        },
        width,
        height,
    )
    .unwrap_or_default()
}

fn quantize_rect(rect: NavigationTransitionRect, step: u16) -> NavigationTransitionRect {
    let step = step.max(1);
    NavigationTransitionRect {
        x: rect.x / step * step,
        y: rect.y / step * step,
        width: rect.width.div_ceil(step) * step,
        height: rect.height.div_ceil(step) * step,
    }
}

fn lerp_usize(from: usize, to: usize, progress_q16: u16) -> usize {
    let from = from as i64;
    let delta = to as i64 - from;
    (from + delta * i64::from(progress_q16) / i64::from(PROGRESS_MAX)).max(0) as usize
}

fn point_in_rect(x: usize, y: usize, rect: NavigationTransitionRect) -> bool {
    x >= rect.x as usize
        && x < rect.right() as usize
        && y >= rect.y as usize
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

    fn geometry() -> NavigationTransitionGeometry {
        let mut ascii = [0u8; 32];
        ascii[..8].copy_from_slice(b"CONSOLES");
        NavigationTransitionGeometry {
            label_ascii: ascii,
            label_len: 8,
            source_card: NavigationTransitionRect {
                x: 32,
                y: 14,
                width: 40,
                height: 68,
            },
            source_label: NavigationTransitionRect {
                x: 38,
                y: 42,
                width: 28,
                height: 8,
            },
            source_detail: NavigationTransitionRect {
                x: 38,
                y: 51,
                width: 28,
                height: 5,
            },
            destination_title: NavigationTransitionRect {
                x: 8,
                y: 6,
                width: 64,
                height: 10,
            },
            destination_list: NavigationTransitionRect {
                x: 2,
                y: 16,
                width: 82,
                height: 66,
            },
            destination_selected_row: NavigationTransitionRect {
                x: 2,
                y: 42,
                width: 82,
                height: 8,
            },
            destination_preview: NavigationTransitionRect {
                x: 94,
                y: 22,
                width: 54,
                height: 52,
            },
            destination_footer: NavigationTransitionRect {
                x: 2,
                y: 82,
                width: 82,
                height: 6,
            },
            ..NavigationTransitionGeometry::default()
        }
    }

    fn frame(progress_q16: u16) -> NavigationTransitionFrame {
        NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Expand,
            progress_q16,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        }
    }

    fn snapshot(width: usize, height: usize, seed: u16) -> Vec<Rgb565Pixel> {
        (0..width.saturating_mul(height))
            .map(|index| Rgb565Pixel(seed.wrapping_add(index as u16)))
            .collect()
    }

    fn render_at(
        direction: NavigationTransitionDirection,
        canonical: u16,
        edge: NavigationTransitionEdge,
    ) -> Result<(Vec<Rgb565Pixel>, NavigationTransitionRenderStats), NavigationTransitionFailure>
    {
        let width = 160;
        let height = 90;
        let source = snapshot(width, height, 0x0841);
        let destination = snapshot(width, height, 0x2104);
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        let (raw_source, raw_destination, progress) = match direction {
            NavigationTransitionDirection::Forward => (&source, &destination, canonical),
            NavigationTransitionDirection::Reverse => (
                &destination,
                &source,
                PROGRESS_MAX.saturating_sub(canonical),
            ),
        };
        buffers.capture_source(raw_source)?;
        buffers.capture_destination(raw_destination)?;
        let request = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::CrtCabinetBoot,
            edge,
            direction,
            geometry(),
        );
        let stats = render_crt_cabinet(
            &CrtCabinetRenderer::new(false),
            &mut buffers,
            request,
            frame(progress),
        )?;
        Ok((buffers.working().to_vec(), stats))
    }

    #[test]
    fn forward_and_reverse_are_exact_canonical_complements() {
        for edge in [
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionEdge::ConsolesToSystem,
        ] {
            for canonical in [
                0,
                1,
                96,
                super::super::COVER_PROGRESS - 1,
                super::super::COVER_PROGRESS,
                super::super::COVER_PROGRESS + 1,
                49_152,
                PROGRESS_MAX - 96,
                PROGRESS_MAX - 1,
                PROGRESS_MAX,
            ] {
                assert_eq!(
                    render_at(NavigationTransitionDirection::Forward, canonical, edge)
                        .unwrap()
                        .0,
                    render_at(NavigationTransitionDirection::Reverse, canonical, edge)
                        .unwrap()
                        .0,
                    "{edge:?} canonical {canonical}"
                );
            }
        }
    }

    #[test]
    fn covered_frame_does_not_depend_on_destination_hydration() {
        let width = 160;
        let height = 90;
        let source = snapshot(width, height, 0x0841);
        let destination = snapshot(width, height, 0x2104);
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        let request = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::CrtCabinetBoot,
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            geometry(),
        );
        render_crt_cabinet(
            &CrtCabinetRenderer::new(false),
            &mut buffers,
            request,
            frame(super::super::COVER_PROGRESS),
        )
        .unwrap();
        let before = buffers.working().to_vec();
        buffers.capture_destination(&destination).unwrap();
        render_crt_cabinet(
            &CrtCabinetRenderer::new(false),
            &mut buffers,
            request,
            frame(super::super::COVER_PROGRESS),
        )
        .unwrap();
        assert_eq!(buffers.working(), before);
    }

    #[test]
    fn missing_snapshot_fails_only_when_that_semantic_half_is_needed() {
        let width = 160;
        let height = 90;
        let source = snapshot(width, height, 0x0841);
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        let forward = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::CrtCabinetBoot,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            geometry(),
        );
        assert!(
            render_crt_cabinet(
                &CrtCabinetRenderer::new(false),
                &mut buffers,
                forward,
                frame(super::super::COVER_PROGRESS),
            )
            .is_ok()
        );
        assert_eq!(
            render_crt_cabinet(
                &CrtCabinetRenderer::new(false),
                &mut buffers,
                forward,
                frame(super::super::COVER_PROGRESS + 1),
            ),
            Err(NavigationTransitionFailure::SnapshotSizeMismatch)
        );

        let reverse = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::CrtCabinetBoot,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Reverse,
            geometry(),
        );
        assert_eq!(
            render_crt_cabinet(
                &CrtCabinetRenderer::new(false),
                &mut buffers,
                reverse,
                frame(PROGRESS_MAX - (super::super::COVER_PROGRESS - 1)),
            ),
            Err(NavigationTransitionFailure::SnapshotSizeMismatch)
        );
    }

    #[test]
    fn list_is_established_before_preview_pixels() {
        let width = 160;
        let height = 90;
        let source = vec![Rgb565Pixel(0x0841); width * height];
        let mut destination = vec![Rgb565Pixel(0x2104); width * height];
        let geometry = geometry();
        for y in geometry.destination_list.y as usize..geometry.destination_list.bottom() as usize {
            for x in
                geometry.destination_list.x as usize..geometry.destination_list.right() as usize
            {
                destination[y * width + x] = Rgb565Pixel(0x07e0);
            }
        }
        for y in
            geometry.destination_preview.y as usize..geometry.destination_preview.bottom() as usize
        {
            for x in geometry.destination_preview.x as usize
                ..geometry.destination_preview.right() as usize
            {
                destination[y * width + x] = Rgb565Pixel(0xf800);
            }
        }
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        buffers.capture_destination(&destination).unwrap();
        let request = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::CrtCabinetBoot,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            geometry,
        );
        let canonical = super::super::COVER_PROGRESS
            + ((PROGRESS_MAX - super::super::COVER_PROGRESS) as u32 * 24_000 / PROGRESS_MAX as u32)
                as u16;
        render_crt_cabinet(
            &CrtCabinetRenderer::new(true),
            &mut buffers,
            request,
            frame(canonical),
        )
        .unwrap();
        assert!(
            buffers
                .working()
                .iter()
                .any(|pixel| *pixel == Rgb565Pixel(0x07e0))
        );
        assert!(
            !buffers
                .working()
                .iter()
                .any(|pixel| *pixel == Rgb565Pixel(0xf800))
        );
    }

    #[test]
    fn reduced_mode_keeps_structure_without_sparks() {
        let (_, full) = render_at(
            NavigationTransitionDirection::Forward,
            49_152,
            NavigationTransitionEdge::HomeToConsoles,
        )
        .unwrap();
        assert!(full.sparks > 0);
        assert!(full.sparks <= SPARK_POOL_SIZE as u64);

        let width = 160;
        let height = 90;
        let source = snapshot(width, height, 0x0841);
        let destination = snapshot(width, height, 0x2104);
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        buffers.capture_destination(&destination).unwrap();
        let request = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::CrtCabinetBoot,
            NavigationTransitionEdge::HomeToConsoles,
            NavigationTransitionDirection::Forward,
            geometry(),
        );
        render_crt_cabinet(
            &CrtCabinetRenderer::new(true),
            &mut buffers,
            request,
            frame(49_152),
        )
        .unwrap();
        assert_eq!(buffers.working().len(), width * height);
    }
}
