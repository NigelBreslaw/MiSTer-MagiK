// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fixed-point cabinet-portal dive inspired by 1980s arcade attract modes.

use super::{
    NavigationTransitionBuffers, NavigationTransitionDirection, NavigationTransitionEdge,
    NavigationTransitionEndpoint, NavigationTransitionFailure, NavigationTransitionFrame,
    NavigationTransitionGeometry, NavigationTransitionPhase, NavigationTransitionRect,
    NavigationTransitionRenderStats, NavigationTransitionRequest, PROGRESS_MAX, clip_rect_to_frame,
    fill_rect_565, lerp_rect, opaque_content_bounds, smoothstep_q16, window_q16,
};
use slint::platform::software_renderer::Rgb565Pixel;

const FRAME_COUNT: usize = 25;
const ROW_COUNT: usize = 110;
const CABINET_COUNT: usize = 2;
const MAX_QUADS: usize = 12;
const MAX_VECTOR_SEGMENTS: usize = 96;
const MAX_SPANS: usize = 1_500;

const VOID: Rgb565Pixel = Rgb565Pixel(0x0843);
const CABINET_DEEP: Rgb565Pixel = Rgb565Pixel(0x0886);
const CABINET_FACE: Rgb565Pixel = Rgb565Pixel(0x1087);
const CYAN: Rgb565Pixel = Rgb565Pixel(0x073f);
const MINT_WHITE: Rgb565Pixel = Rgb565Pixel(0x07fe);
const VIOLET: Rgb565Pixel = Rgb565Pixel(0x8a5f);
const MAGENTA: Rgb565Pixel = Rgb565Pixel(0xf95a);
const AMBER: Rgb565Pixel = Rgb565Pixel(0xfd80);
const WHITE: Rgb565Pixel = Rgb565Pixel(0xf7bf);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ProjectedRow {
    y: u16,
    left: u16,
    right: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CabinetQuad {
    rect: NavigationTransitionRect,
    side: u8,
    depth: u8,
}

#[derive(Clone, Debug)]
struct FramePlan {
    rows: [ProjectedRow; ROW_COUNT],
    cabinets: [CabinetQuad; CABINET_COUNT],
    vanishing_x: u16,
    horizon_y: u16,
    pulse_offset: u8,
}

impl Default for FramePlan {
    fn default() -> Self {
        Self {
            rows: [ProjectedRow::default(); ROW_COUNT],
            cabinets: [CabinetQuad::default(); CABINET_COUNT],
            vanishing_x: 0,
            horizon_y: 0,
            pulse_offset: 0,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct TitleMask {
    bounds: NavigationTransitionRect,
    width: usize,
    height: usize,
    pixels: Vec<Rgb565Pixel>,
    opaque: Vec<bool>,
    row_runs: Vec<MaskRun>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MaskRun {
    y: u16,
    x: u16,
    width: u16,
}

impl TitleMask {
    fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0 || !self.opaque.iter().any(|value| *value)
    }
}

#[derive(Clone, Debug, Default)]
struct HeroPacket {
    label_signature: u64,
    source_title: TitleMask,
    destination_title: TitleMask,
    canonical_title: TitleMask,
    source_detail: TitleMask,
    destination_detail: TitleMask,
}

#[derive(Debug, Default)]
pub(super) struct NeonCabinetRenderer {
    width: usize,
    height: usize,
    geometry_signature: u64,
    plans: Vec<FramePlan>,
    hero_cache: [Option<HeroPacket>; 3],
    hero: HeroPacket,
}

impl NeonCabinetRenderer {
    pub(super) fn prepare(&mut self, width: usize, height: usize) {
        let geometry = NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: (width / 3).min(u16::MAX as usize) as u16,
                y: (height / 7).min(u16::MAX as usize) as u16,
                width: (width / 4).min(u16::MAX as usize) as u16,
                height: (height * 3 / 4).min(u16::MAX as usize) as u16,
            },
            source_label: NavigationTransitionRect {
                x: (width * 2 / 5).min(u16::MAX as usize) as u16,
                y: (height / 2).min(u16::MAX as usize) as u16,
                width: (width / 5).min(u16::MAX as usize) as u16,
                height: (height / 20).max(1).min(u16::MAX as usize) as u16,
            },
            ..NavigationTransitionGeometry::default()
        };
        self.prepare_plans(width, height, geometry);
    }

    pub(super) fn prepare_transition(
        &mut self,
        width: usize,
        height: usize,
        source: &[Rgb565Pixel],
        geometry: NavigationTransitionGeometry,
        direction: NavigationTransitionDirection,
        edge: NavigationTransitionEdge,
    ) {
        self.prepare_plans(width, height, geometry);
        let cache_index = edge_index(edge);
        match direction {
            NavigationTransitionDirection::Forward => {
                self.cache_forward_source(width, height, source, geometry, edge);
                self.hero = self.hero_cache[cache_index]
                    .clone()
                    .unwrap_or_else(|| HeroPacket {
                        label_signature: geometry_signature(width, height, geometry),
                        ..HeroPacket::default()
                    });
            }
            NavigationTransitionDirection::Reverse => {
                let mut hero = self.hero_cache[cache_index]
                    .as_ref()
                    .filter(|packet| {
                        packet.label_signature == geometry_signature(width, height, geometry)
                            && !packet.source_title.is_empty()
                    })
                    .cloned()
                    .unwrap_or_else(|| HeroPacket {
                        label_signature: geometry_signature(width, height, geometry),
                        ..HeroPacket::default()
                    });
                hero.destination_title =
                    extract_mask(source, width, height, geometry.destination_title);
                hero.destination_detail =
                    extract_mask(source, width, height, geometry.destination_detail);
                if hero.source_title.is_empty() {
                    hero.source_title = hero.destination_title.clone();
                }
                if hero.source_detail.is_empty() {
                    hero.source_detail = hero.destination_detail.clone();
                }
                if hero.canonical_title.is_empty() {
                    hero.canonical_title = canonical_title_mask(geometry);
                }
                self.hero = hero;
            }
        }
    }

    pub(super) fn cache_forward_source(
        &mut self,
        width: usize,
        height: usize,
        source: &[Rgb565Pixel],
        geometry: NavigationTransitionGeometry,
        edge: NavigationTransitionEdge,
    ) {
        self.prepare_plans(width, height, geometry);
        self.hero_cache[edge_index(edge)] = Some(HeroPacket {
            label_signature: geometry_signature(width, height, geometry),
            source_title: extract_mask(source, width, height, geometry.source_label),
            canonical_title: canonical_title_mask(geometry),
            source_detail: extract_mask(source, width, height, geometry.source_detail),
            ..HeroPacket::default()
        });
    }

    pub(super) fn cache_forward_destination(
        &mut self,
        width: usize,
        height: usize,
        destination: &[Rgb565Pixel],
        geometry: NavigationTransitionGeometry,
        edge: NavigationTransitionEdge,
    ) {
        let Some(mut packet) = self.hero_cache[edge_index(edge)]
            .as_ref()
            .filter(|packet| packet.label_signature == geometry_signature(width, height, geometry))
            .cloned()
        else {
            return;
        };
        packet.destination_title =
            extract_mask(destination, width, height, geometry.destination_title);
        packet.destination_detail =
            extract_mask(destination, width, height, geometry.destination_detail);
        self.hero_cache[edge_index(edge)] = Some(packet);
    }

    pub(super) fn prepare_destination(
        &mut self,
        width: usize,
        height: usize,
        destination: &[Rgb565Pixel],
        geometry: NavigationTransitionGeometry,
        direction: NavigationTransitionDirection,
        edge: NavigationTransitionEdge,
    ) {
        match direction {
            NavigationTransitionDirection::Forward => {
                self.hero.destination_title =
                    extract_mask(destination, width, height, geometry.destination_title);
                self.hero.destination_detail =
                    extract_mask(destination, width, height, geometry.destination_detail);
                self.cache_forward_destination(width, height, destination, geometry, edge);
            }
            NavigationTransitionDirection::Reverse => {
                let mut cached = self.hero.clone();
                cached.source_title =
                    extract_mask(destination, width, height, geometry.source_label);
                cached.source_detail =
                    extract_mask(destination, width, height, geometry.source_detail);
                self.hero_cache[edge_index(edge)] = Some(cached);
                return;
            }
        }
        if direction == NavigationTransitionDirection::Forward {
            self.hero_cache[edge_index(edge)] = Some(self.hero.clone());
        }
    }

    fn prepare_plans(
        &mut self,
        width: usize,
        height: usize,
        geometry: NavigationTransitionGeometry,
    ) {
        let signature = geometry_signature(width, height, geometry);
        if self.width == width
            && self.height == height
            && self.geometry_signature == signature
            && self.plans.len() == FRAME_COUNT
        {
            return;
        }
        self.width = width;
        self.height = height;
        self.geometry_signature = signature;
        self.plans.clear();
        if width == 0 || height == 0 {
            return;
        }
        self.plans.reserve(FRAME_COUNT);
        let source_center_x =
            geometry.source_label.x as usize + geometry.source_label.width as usize / 2;
        let source_center_y =
            geometry.source_label.y as usize + geometry.source_label.height as usize / 2;
        let final_horizon = height * 49 / 200;
        for frame_index in 0..FRAME_COUNT {
            let canonical = (frame_index as u32 * u32::from(PROGRESS_MAX)
                / FRAME_COUNT.saturating_sub(1).max(1) as u32) as u16;
            let cover = scale_segment(canonical.min(super::COVER_PROGRESS), super::COVER_PROGRESS);
            let camera = smoothstep_q16(cover);
            let vanishing_x = lerp_usize(source_center_x, width / 2, camera);
            let horizon_y = lerp_usize(
                source_center_y.min(height.saturating_sub(1)),
                final_horizon,
                camera,
            );
            let mut plan = FramePlan {
                vanishing_x: vanishing_x.min(u16::MAX as usize) as u16,
                horizon_y: horizon_y.min(u16::MAX as usize) as u16,
                pulse_offset: ((frame_index * 7) % ROW_COUNT) as u8,
                ..FramePlan::default()
            };
            for row_index in 0..ROW_COUNT {
                let depth_q16 = (row_index + 1) as u32 * PROGRESS_MAX as u32 / ROW_COUNT as u32;
                let perspective = depth_q16.saturating_mul(depth_q16) / PROGRESS_MAX as u32;
                let y = horizon_y
                    + height
                        .saturating_sub(1)
                        .saturating_sub(horizon_y)
                        .saturating_mul(perspective as usize)
                        / PROGRESS_MAX as usize;
                let far_half = 5usize;
                let near_half = width.saturating_sub(64) / 2;
                let half = far_half
                    + near_half.saturating_sub(far_half) * perspective as usize
                        / PROGRESS_MAX as usize;
                plan.rows[row_index] = ProjectedRow {
                    y: y.min(height.saturating_sub(1)).min(u16::MAX as usize) as u16,
                    left: vanishing_x.saturating_sub(half).min(u16::MAX as usize) as u16,
                    right: vanishing_x
                        .saturating_add(half)
                        .min(width.saturating_sub(1))
                        .min(u16::MAX as usize) as u16,
                };
            }
            for cabinet_index in 0..CABINET_COUNT {
                let side = (cabinet_index & 1) as u8;
                let lane_index = cabinet_index / 2;
                let depth_index = (18 + lane_index * 24 + frame_index * 4 + usize::from(side) * 9)
                    .min(ROW_COUNT - 1);
                let row = plan.rows[depth_index];
                let scale = 9 + depth_index * 52 / ROW_COUNT;
                let cabinet_width = (scale * 4 / 5).max(5);
                let cabinet_height = (scale * 3 / 2).max(8);
                let anchor = if side == 0 {
                    row.left as usize
                } else {
                    row.right as usize
                };
                let x = if side == 0 {
                    anchor.saturating_sub(cabinet_width / 3)
                } else {
                    anchor.saturating_sub(cabinet_width * 2 / 3)
                };
                let y = (row.y as usize).saturating_sub(cabinet_height);
                plan.cabinets[cabinet_index] = CabinetQuad {
                    rect: NavigationTransitionRect {
                        x: x.min(u16::MAX as usize) as u16,
                        y: y.min(u16::MAX as usize) as u16,
                        width: cabinet_width.min(u16::MAX as usize) as u16,
                        height: cabinet_height.min(u16::MAX as usize) as u16,
                    },
                    side,
                    depth: depth_index as u8,
                };
            }
            self.plans.push(plan);
        }
    }

    fn plan(&self, canonical_q16: u16) -> Option<FramePlan> {
        let scaled = canonical_q16 as u32 * (FRAME_COUNT - 1) as u32;
        let denominator = u32::from(PROGRESS_MAX.max(1));
        let index = (scaled / denominator) as usize;
        let mix_q16 = (scaled % denominator) as u16;
        let first = self.plans.get(index)?;
        let second = self.plans.get((index + 1).min(FRAME_COUNT - 1))?;
        let mut plan = FramePlan {
            vanishing_x: lerp_usize(
                first.vanishing_x as usize,
                second.vanishing_x as usize,
                mix_q16,
            ) as u16,
            horizon_y: lerp_usize(first.horizon_y as usize, second.horizon_y as usize, mix_q16)
                as u16,
            pulse_offset: interpolate_wrapped_offset(
                first.pulse_offset,
                second.pulse_offset,
                mix_q16,
            ),
            ..FramePlan::default()
        };
        for row_index in 0..ROW_COUNT {
            let from = first.rows[row_index];
            let to = second.rows[row_index];
            plan.rows[row_index] = ProjectedRow {
                y: lerp_usize(from.y as usize, to.y as usize, mix_q16) as u16,
                left: lerp_usize(from.left as usize, to.left as usize, mix_q16) as u16,
                right: lerp_usize(from.right as usize, to.right as usize, mix_q16) as u16,
            };
        }
        for cabinet_index in 0..CABINET_COUNT {
            let from = first.cabinets[cabinet_index];
            let to = second.cabinets[cabinet_index];
            plan.cabinets[cabinet_index] = CabinetQuad {
                rect: lerp_rect(from.rect, to.rect, mix_q16),
                side: if mix_q16 < PROGRESS_MAX / 2 {
                    from.side
                } else {
                    to.side
                },
                depth: lerp_usize(from.depth as usize, to.depth as usize, mix_q16) as u8,
            };
        }
        Some(plan)
    }
}

pub(super) fn render_neon_cabinet(
    renderer: &mut NeonCabinetRenderer,
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
    if canonical <= 256 {
        let endpoint = source.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        buffers.working.copy_from_slice(endpoint);
        stats.copied_pixels = endpoint.len() as u64;
        return Ok(stats);
    }
    if canonical >= PROGRESS_MAX.saturating_sub(256) {
        let endpoint = destination.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        buffers.working.copy_from_slice(endpoint);
        stats.copied_pixels = endpoint.len() as u64;
        return Ok(stats);
    }

    renderer.prepare_plans(width, height, request.geometry);
    let Some(plan) = renderer.plan(canonical) else {
        return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
    };
    let working = buffers.working.as_mut_slice();
    if canonical < super::COVER_PROGRESS {
        let source = source.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        working.copy_from_slice(source);
        stats.copied_pixels = source.len() as u64;
        let cover = scale_segment(canonical, super::COVER_PROGRESS);
        render_source_half(
            working,
            width,
            height,
            request.geometry,
            &plan,
            cover,
            &mut stats,
        );
        render_hero(
            &renderer.hero,
            working,
            width,
            height,
            request.geometry,
            cover,
            0,
            &mut stats,
        );
    } else if canonical == super::COVER_PROGRESS {
        render_covered_frame(working, width, height, request.geometry, &plan, &mut stats);
        render_hero(
            &renderer.hero,
            working,
            width,
            height,
            request.geometry,
            PROGRESS_MAX,
            0,
            &mut stats,
        );
    } else {
        let destination = destination.ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        let reveal = scale_from_segment(canonical, super::COVER_PROGRESS);
        render_destination_half(
            working,
            Some(destination),
            width,
            height,
            request.geometry,
            request.edge,
            &plan,
            reveal,
            &mut stats,
        );
        if reveal < 54_000 {
            render_hero(
                &renderer.hero,
                working,
                width,
                height,
                request.geometry,
                PROGRESS_MAX,
                reveal,
                &mut stats,
            );
        }
    }
    stats.projected_rows = ROW_COUNT as u64;
    debug_assert!(
        stats.quads <= MAX_QUADS as u64,
        "Neon emitted {} quads",
        stats.quads
    );
    debug_assert!(
        stats.vector_segments <= MAX_VECTOR_SEGMENTS as u64,
        "Neon emitted {} vectors",
        stats.vector_segments
    );
    debug_assert!(
        stats.spans <= MAX_SPANS as u64,
        "Neon emitted {} spans",
        stats.spans
    );
    Ok(stats)
}

fn render_source_half(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    plan: &FramePlan,
    cover_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let full = frame_rect(width, height);
    let full_glass = primary_glass_rect(full);
    if cover_q16 < 6_000 {
        let lift = if cover_q16 > 2_500 { 3 } else { 2 };
        let shadow = offset_rect(geometry.source_card, lift, lift, width, height);
        draw_corner_outline(working, width, height, shadow, VIOLET, stats);
        draw_corner_outline(
            working,
            width,
            height,
            geometry.source_card,
            if cover_q16 > 2_500 { WHITE } else { CYAN },
            stats,
        );
        return;
    }
    let camera = smoothstep_q16(window_q16(cover_q16, 8_000, 60_000));
    let cabinet_rect = lerp_rect(geometry.source_card, full, camera);
    let shadow = NavigationTransitionRect {
        x: cabinet_rect.x.saturating_add(4),
        y: cabinet_rect.bottom().saturating_sub(1),
        width: cabinet_rect.width.saturating_sub(4),
        height: 4,
    };
    fill_neon_rect(working, width, height, shadow, CABINET_DEEP, stats);
    fill_neon_rect(working, width, height, cabinet_rect, CABINET_FACE, stats);
    stats.quads = stats.quads.saturating_add(2);

    let source_aperture = inset_rect(geometry.source_card, 12, 9, 76, 58);
    let aperture_motion = smoothstep_q16(window_q16(cover_q16, 7_000, 61_000));
    let aperture = lerp_rect(source_aperture, full_glass, aperture_motion);
    if cover_q16 >= 16_000 {
        fill_neon_rect(working, width, height, aperture, VOID, stats);
    }
    stats.quads = stats.quads.saturating_add(1);
    draw_corner_outline(
        working,
        width,
        height,
        cabinet_rect,
        if cover_q16 < 4_000 { WHITE } else { CYAN },
        stats,
    );
    if cover_q16 > 3_000 {
        let echo = offset_rect(cabinet_rect, 2, 1, width, height);
        draw_corner_outline(working, width, height, echo, VIOLET, stats);
    }
    if cover_q16 >= 16_000 {
        draw_cabinet_face(
            working,
            width,
            height,
            cabinet_rect,
            aperture,
            cover_q16,
            stats,
        );
        render_hall(working, width, height, aperture, plan, cover_q16, 0, stats);
    }
}

fn render_covered_frame(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    plan: &FramePlan,
    stats: &mut NavigationTransitionRenderStats,
) {
    let full = frame_rect(width, height);
    let glass = primary_glass_rect(full);
    fill_primary_cabinet_background(working, width, height, full, glass, stats);
    render_hall(working, width, height, glass, plan, PROGRESS_MAX, 0, stats);
    draw_cabinet_face(working, width, height, full, glass, PROGRESS_MAX, stats);
    let _ = geometry;
}

#[allow(clippy::too_many_arguments)]
fn render_destination_half(
    working: &mut [Rgb565Pixel],
    destination: Option<&[Rgb565Pixel]>,
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    edge: NavigationTransitionEdge,
    plan: &FramePlan,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if reveal_q16 >= 54_000 {
        if let Some(destination) = destination {
            working.copy_from_slice(destination);
            stats.copied_pixels = destination.len() as u64;
        } else {
            fill_neon_rect(
                working,
                width,
                height,
                frame_rect(width, height),
                VOID,
                stats,
            );
        }
        return;
    }
    let full = frame_rect(width, height);
    let glass = primary_glass_rect(full);
    fill_primary_cabinet_background(working, width, height, full, glass, stats);
    let environment_fade =
        PROGRESS_MAX.saturating_sub(smoothstep_q16(window_q16(reveal_q16, 22_000, 57_000)));
    render_hall(
        working,
        width,
        height,
        glass,
        plan,
        PROGRESS_MAX,
        reveal_q16,
        stats,
    );
    if environment_fade > 0 {
        draw_cabinet_face(working, width, height, full, glass, PROGRESS_MAX, stats);
    }
    if let Some(destination) = destination {
        let aperture = lerp_rect(
            glass,
            full,
            smoothstep_q16(window_q16(reveal_q16, 26_000, 59_000)),
        );
        reveal_destination_bands(
            working,
            destination,
            width,
            height,
            geometry,
            edge,
            aperture,
            reveal_q16,
            stats,
        );
        if reveal_q16 > 20_000 {
            super::erase_rect_from_snapshot_background(
                working,
                destination,
                width,
                height,
                geometry.destination_title,
                stats,
            );
            stats.spans = stats
                .spans
                .saturating_add(geometry.destination_title.height as u64);
        }
    }
    if edge != NavigationTransitionEdge::HomeToConsoles {
        draw_destination_carriers(
            working,
            width,
            height,
            geometry,
            plan,
            reveal_q16,
            environment_fade,
            stats,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_hall(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    plan: &FramePlan,
    cover_q16: u16,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(clip) = clip_rect_to_frame(clip, width, height) else {
        return;
    };
    let environment =
        PROGRESS_MAX.saturating_sub(smoothstep_q16(window_q16(reveal_q16, 18_000, 58_000)));
    if environment == 0 || reveal_q16 >= 20_000 {
        return;
    }
    let pulse_count = if reveal_q16 < 8_000 {
        20
    } else if reveal_q16 < 14_000 {
        8
    } else {
        4
    };
    let rail_segments = if reveal_q16 < 8_000 {
        8
    } else if reveal_q16 < 14_000 {
        5
    } else {
        2
    };
    let cabinet_limit = if reveal_q16 < 10_000 {
        CABINET_COUNT
    } else if reveal_q16 < 15_000 {
        1
    } else {
        0
    };
    let horizon_y =
        (plan.horizon_y as usize).clamp(clip.y as usize, clip.bottom().saturating_sub(1) as usize);
    let sun_width = (width / 5).min(clip.width as usize);
    let sun_x = plan.vanishing_x as usize;
    if reveal_q16 == 0 {
        for band in 0..4 {
            let half = sun_width.saturating_mul(5 - band) / 10;
            let y = horizon_y.saturating_sub(30).saturating_add(band * 6);
            fill_neon_rect(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: sun_x.saturating_sub(half).max(clip.x as usize) as u16,
                    y: y.max(clip.y as usize).min(clip.bottom() as usize - 1) as u16,
                    width: (half * 2)
                        .min((clip.right() as usize).saturating_sub(sun_x.saturating_sub(half)))
                        as u16,
                    height: 2,
                },
                if band & 1 == 0 { MAGENTA } else { VIOLET },
                stats,
            );
        }
    }

    let pulse_offset = plan.pulse_offset as usize;
    for pulse in 0..pulse_count {
        let row_index = (pulse * 5 + pulse_offset) % ROW_COUNT;
        let row = plan.rows[row_index];
        let y = row.y as usize;
        if y < clip.y as usize || y >= clip.bottom() as usize {
            continue;
        }
        let left = (row.left as usize).max(clip.x as usize);
        let right = (row.right as usize).min(clip.right() as usize);
        if right <= left {
            continue;
        }
        fill_neon_rect(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: left as u16,
                y: y as u16,
                width: (right - left) as u16,
                height: if pulse % 7 == 0 { 2 } else { 1 },
            },
            if pulse % 6 == 0 { MAGENTA } else { CYAN },
            stats,
        );
    }

    for rail in 0..5 {
        let lane_q16 = ((rail + 1) as u32 * u32::from(PROGRESS_MAX) / 6) as u16;
        for segment in 0..rail_segments {
            if stats.vector_segments >= MAX_VECTOR_SEGMENTS as u64 {
                break;
            }
            let a_index = segment * (ROW_COUNT - 1) / rail_segments.max(1);
            let b_index = (segment + 1) * (ROW_COUNT - 1) / rail_segments.max(1);
            let a = plan.rows[a_index];
            let b = plan.rows[b_index];
            let from = (
                lerp_usize(a.left as usize, a.right as usize, lane_q16),
                a.y as usize,
            );
            let to = (
                lerp_usize(b.left as usize, b.right as usize, lane_q16),
                b.y as usize,
            );
            draw_short_vector(
                working,
                width,
                height,
                from,
                to,
                clip,
                if rail & 1 == 0 { CYAN } else { VIOLET },
                stats,
            );
            stats.vector_segments = stats.vector_segments.saturating_add(1);
        }
    }

    let cabinet_visibility = smoothstep_q16(window_q16(cover_q16, 24_000, 43_000));
    if cabinet_visibility > 0 {
        for cabinet in plan.cabinets.into_iter().take(cabinet_limit) {
            if stats.quads >= 9 {
                break;
            }
            let Some(rect) = intersect_rect(cabinet.rect, clip) else {
                continue;
            };
            let cabinet_color = if cabinet.side == 0 { VIOLET } else { CYAN };
            for y in [rect.y, rect.bottom().saturating_sub(1)] {
                fill_neon_rect(
                    working,
                    width,
                    height,
                    NavigationTransitionRect {
                        x: rect.x,
                        y,
                        width: rect.width,
                        height: 1,
                    },
                    cabinet_color,
                    stats,
                );
            }
            let corner_height = rect.height.min(12).max(2);
            for x in [rect.x, rect.right().saturating_sub(1)] {
                fill_neon_rect(
                    working,
                    width,
                    height,
                    NavigationTransitionRect {
                        x,
                        y: rect.y,
                        width: 1,
                        height: corner_height,
                    },
                    cabinet_color,
                    stats,
                );
            }
            let monitor = inset_rect(rect, 18, 13, 64, 42);
            for y in [monitor.y, monitor.bottom().saturating_sub(1)] {
                fill_neon_rect(
                    working,
                    width,
                    height,
                    NavigationTransitionRect {
                        x: monitor.x,
                        y,
                        width: monitor.width,
                        height: 1,
                    },
                    MINT_WHITE,
                    stats,
                );
            }
            let shelf_y = rect
                .y
                .saturating_add((rect.height as u32 * 68 / 100) as u16);
            fill_neon_rect(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: rect.x.saturating_add(2),
                    y: shelf_y,
                    width: rect.width.saturating_sub(4).max(1),
                    height: 1,
                },
                AMBER,
                stats,
            );
            stats.quads = stats.quads.saturating_add(1);
        }
    }

    if reveal_q16 == 0 {
        for tick in 0..7 {
            let row = plan.rows[(22 + tick * 10).min(ROW_COUNT - 1)];
            let right_side = tick & 1 == 1;
            let x = if right_side {
                row.right as usize
            } else {
                row.left as usize
            };
            let start = if right_side {
                x.saturating_sub(18)
            } else {
                x.saturating_add(2)
            };
            fill_neon_rect(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: start.max(clip.x as usize).min(clip.right() as usize - 1) as u16,
                    y: (row.y as usize)
                        .max(clip.y as usize)
                        .min(clip.bottom() as usize - 1) as u16,
                    width: 16.min((clip.right() as usize).saturating_sub(start)).max(1) as u16,
                    height: 1,
                },
                if tick % 3 == 0 { AMBER } else { MAGENTA },
                stats,
            );
        }
    }
}

fn draw_cabinet_face(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    cabinet: NavigationTransitionRect,
    aperture: NavigationTransitionRect,
    cover_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if cover_q16 < 6_000 {
        return;
    }
    draw_corner_outline(working, width, height, aperture, CYAN, stats);
    let shelf_y = cabinet
        .y
        .saturating_add((cabinet.height as u32 * 78 / 100) as u16);
    fill_neon_rect(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: cabinet.x.saturating_add(cabinet.width / 10),
            y: shelf_y,
            width: cabinet.width.saturating_mul(4) / 5,
            height: 2,
        },
        MAGENTA,
        stats,
    );
    stats.quads = stats.quads.saturating_add(1);

    let hood_y = aperture.y.saturating_sub(10);
    let hood_left = aperture.x.saturating_sub(14);
    let hood_right = aperture.right().saturating_add(14);
    for (from, to) in [
        (
            (hood_left as usize, hood_y as usize),
            (aperture.x as usize, aperture.y as usize),
        ),
        (
            (hood_right as usize, hood_y as usize),
            (aperture.right() as usize, aperture.y as usize),
        ),
        (
            (cabinet.x as usize, shelf_y as usize),
            (
                cabinet.x.saturating_add(cabinet.width / 10) as usize,
                shelf_y.saturating_sub(8) as usize,
            ),
        ),
        (
            (cabinet.right() as usize, shelf_y as usize),
            (
                cabinet.right().saturating_sub(cabinet.width / 10) as usize,
                shelf_y.saturating_sub(8) as usize,
            ),
        ),
    ] {
        if stats.vector_segments >= MAX_VECTOR_SEGMENTS as u64 {
            break;
        }
        draw_short_vector(
            working,
            width,
            height,
            from,
            to,
            frame_rect(width, height),
            VIOLET,
            stats,
        );
        stats.vector_segments = stats.vector_segments.saturating_add(1);
    }
    let slot_y = shelf_y.saturating_add(9);
    for offset in [48u16] {
        fill_neon_rect(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: cabinet
                    .x
                    .saturating_add((cabinet.width as u32 * u32::from(offset) / 100) as u16),
                y: slot_y,
                width: (cabinet.width / 60).max(2),
                height: (cabinet.height / 90).max(2),
            },
            AMBER,
            stats,
        );
    }
}

fn fill_primary_cabinet_background(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    cabinet: NavigationTransitionRect,
    glass: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    fill_neon_rect(working, width, height, cabinet, VOID, stats);
    fill_neon_rect(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: cabinet.x,
            y: cabinet.y,
            width: cabinet.width,
            height: glass.y.saturating_sub(cabinet.y).max(1),
        },
        CABINET_FACE,
        stats,
    );
    fill_neon_rect(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: cabinet.x,
            y: glass.bottom(),
            width: cabinet.width,
            height: cabinet.bottom().saturating_sub(glass.bottom()).max(1),
        },
        CABINET_FACE,
        stats,
    );
    let corner_height = glass.height.min(22).max(2);
    for x in [cabinet.x, glass.right()] {
        for y in [glass.y, glass.bottom().saturating_sub(corner_height)] {
            fill_neon_rect(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x,
                    y,
                    width: glass.x.saturating_sub(cabinet.x).max(2),
                    height: corner_height,
                },
                CABINET_DEEP,
                stats,
            );
        }
    }
}

fn reveal_destination_bands(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    edge: NavigationTransitionEdge,
    aperture: NavigationTransitionRect,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if destination.len() != working.len() || width == 0 || height == 0 {
        return;
    }
    let aperture_x = smoothstep_q16(window_q16(reveal_q16, 1_500, 14_000));
    let aperture_y = smoothstep_q16(window_q16(reveal_q16, 5_000, 23_000));
    let visible_width =
        (aperture.width as usize * aperture_x as usize / PROGRESS_MAX as usize).max(2);
    let visible_height =
        (aperture.height as usize * aperture_y as usize / PROGRESS_MAX as usize).max(2);
    let visible = NavigationTransitionRect {
        x: aperture
            .x
            .saturating_add((aperture.width as usize - visible_width) as u16 / 2),
        y: aperture
            .y
            .saturating_add((aperture.height as usize - visible_height) as u16 / 2),
        width: visible_width.min(u16::MAX as usize) as u16,
        height: visible_height.min(u16::MAX as usize) as u16,
    };
    let selected_center = geometry.destination_selected_row.y as usize
        + geometry.destination_selected_row.height as usize / 2;
    for band_y in (visible.y as usize..visible.bottom() as usize).step_by(4) {
        let band_bottom = (band_y + 4).min(visible.bottom() as usize);
        let center_y = (band_y + band_bottom) / 2;
        let delay = if edge == NavigationTransitionEdge::HomeToConsoles {
            9_000u16.saturating_add((center_y.min(height) / 8) as u16 * 120)
        } else if center_y >= geometry.destination_selected_row.y as usize
            && center_y < geometry.destination_selected_row.bottom() as usize
        {
            7_000
        } else if center_y >= geometry.destination_list.y as usize
            && center_y < geometry.destination_list.bottom() as usize
        {
            11_000u16.saturating_add(center_y.abs_diff(selected_center).min(160) as u16 * 45)
        } else if center_y >= geometry.destination_preview.y as usize
            && center_y < geometry.destination_preview.bottom() as usize
        {
            26_000u16.saturating_add(((center_y / 8) & 1) as u16 * 2_500)
        } else if center_y >= geometry.destination_footer.y as usize {
            30_000
        } else {
            23_000u16.saturating_add((center_y.abs_diff(height / 2).min(180) * 25) as u16)
        };
        let local = smoothstep_q16(window_q16(
            reveal_q16,
            delay,
            delay.saturating_add(16_000).min(PROGRESS_MAX),
        ));
        if local < 12_000 {
            continue;
        }
        let semantic_list_row = edge != NavigationTransitionEdge::HomeToConsoles
            && center_y >= geometry.destination_list.y as usize
            && center_y < geometry.destination_list.bottom() as usize;
        let (x0, x1) = if semantic_list_row {
            (
                geometry.destination_list.x as usize,
                geometry.destination_list.right() as usize,
            )
        } else {
            (visible.x as usize, visible.right() as usize)
        };
        if x1 <= x0 {
            continue;
        }
        for y in band_y..band_bottom {
            let start = y * width + x0;
            let end = y * width + x1;
            working[start..end].copy_from_slice(&destination[start..end]);
            stats.copied_pixels = stats
                .copied_pixels
                .saturating_add(end.saturating_sub(start) as u64);
            stats.spans = stats.spans.saturating_add(1);
        }
    }
    let hot_line = window_q16(reveal_q16, 1_500, 12_000);
    if hot_line > 0 && hot_line < PROGRESS_MAX {
        let hot_line_bounds =
            if geometry.destination_preview.width > 0 && geometry.destination_preview.height > 0 {
                intersect_rect(visible, geometry.destination_preview).unwrap_or(visible)
            } else {
                visible
            };
        let half = hot_line_bounds.width as usize * hot_line as usize / PROGRESS_MAX as usize / 2;
        fill_neon_rect(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: (hot_line_bounds.x as usize + hot_line_bounds.width as usize / 2)
                    .saturating_sub(half)
                    .min(u16::MAX as usize) as u16,
                y: (hot_line_bounds.y as usize + hot_line_bounds.height as usize / 2)
                    .min(u16::MAX as usize) as u16,
                width: (half * 2)
                    .min(hot_line_bounds.width as usize)
                    .min(u16::MAX as usize) as u16,
                height: 2,
            },
            if hot_line > 40_000 { WHITE } else { AMBER },
            stats,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_destination_carriers(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    plan: &FramePlan,
    reveal_q16: u16,
    environment_fade: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if environment_fade == 0 {
        return;
    }
    let selected_motion = smoothstep_q16(window_q16(reveal_q16, 8_000, 28_000));
    if selected_motion < PROGRESS_MAX {
        let origin = NavigationTransitionRect {
            x: plan.vanishing_x.saturating_sub(4),
            y: plan.horizon_y,
            width: 8,
            height: 3,
        };
        let carrier = lerp_rect(origin, geometry.destination_selected_row, selected_motion);
        draw_corner_outline(working, width, height, carrier, AMBER, stats);
    }
    let preview_motion = smoothstep_q16(window_q16(reveal_q16, 12_000, 34_000));
    if preview_motion < PROGRESS_MAX && geometry.destination_preview.width > 0 {
        let origin = NavigationTransitionRect {
            x: plan.vanishing_x.saturating_sub(5),
            y: plan.horizon_y.saturating_sub(3),
            width: 10,
            height: 6,
        };
        let mut target = geometry.destination_preview;
        if preview_motion > 52_000 {
            target.x = target.x.saturating_sub(4);
            target.width = target.width.saturating_add(8);
        }
        let carrier = lerp_rect(origin, target, preview_motion);
        draw_corner_outline(working, width, height, carrier, CYAN, stats);
        stats.quads = stats.quads.saturating_add(1);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_hero(
    hero: &HeroPacket,
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    cover_q16: u16,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if hero.source_title.is_empty() && hero.canonical_title.is_empty() {
        return;
    }
    let marquee_mask =
        if !hero.canonical_title.is_empty() && (cover_q16 >= 12_000 || reveal_q16 > 0) {
            &hero.canonical_title
        } else {
            &hero.source_title
        };
    let multi_line = marquee_mask.height > 7;
    let marquee_bounds = NavigationTransitionRect {
        x: (width / 4).min(u16::MAX as usize) as u16,
        y: (if multi_line { height / 18 } else { height / 11 }).min(u16::MAX as usize) as u16,
        width: (width / 2).min(u16::MAX as usize) as u16,
        height: (if multi_line { height / 5 } else { height / 9 })
            .max(1)
            .min(u16::MAX as usize) as u16,
    };
    let source_target = fit_mask_rect(marquee_mask, geometry.source_label);
    let marquee_target = integer_fit_mask_rect(marquee_mask, marquee_bounds);
    let destination_target = if hero.destination_title.is_empty() {
        fit_mask_rect(marquee_mask, geometry.destination_title)
    } else {
        hero.destination_title.bounds
    };
    let title_rect = if reveal_q16 == 0 {
        lerp_rect(
            source_target,
            marquee_target,
            smoothstep_q16(window_q16(cover_q16, 1_500, 58_000)),
        )
    } else {
        lerp_rect(
            marquee_target,
            destination_target,
            smoothstep_q16(window_q16(reveal_q16, 4_000, 54_000)),
        )
    };
    let title_rect =
        if (reveal_q16 == 0 && cover_q16 >= 6_000) || (reveal_q16 > 0 && reveal_q16 < 34_000) {
            quantize_mask_rect(marquee_mask, title_rect)
        } else {
            title_rect
        };
    let source_opacity = if reveal_q16 == 0 {
        PROGRESS_MAX
    } else {
        PROGRESS_MAX.saturating_sub(smoothstep_q16(window_q16(reveal_q16, 34_000, 55_000)))
    };
    let destination_opacity = if reveal_q16 == 0 {
        0
    } else {
        smoothstep_q16(window_q16(reveal_q16, 34_000, 55_000))
    };
    if cover_q16 >= 6_000
        && cover_q16 < PROGRESS_MAX
        && reveal_q16 == 0
        && (cover_q16 < 40_000 || cover_q16 > 60_000)
    {
        let shadow = offset_rect(title_rect, 2, 2, width, height);
        render_mask(
            marquee_mask,
            working,
            width,
            height,
            shadow,
            source_opacity,
            Some(VIOLET),
            stats,
        );
    }
    render_mask(
        marquee_mask,
        working,
        width,
        height,
        title_rect,
        source_opacity,
        if cover_q16 < 6_000 {
            None
        } else if (cover_q16 > 62_000 && reveal_q16 == 0) || (reveal_q16 > 0 && reveal_q16 < 5_000)
        {
            Some(WHITE)
        } else {
            Some(CYAN)
        },
        stats,
    );
    if destination_opacity > 0 && !hero.destination_title.is_empty() {
        render_mask(
            &hero.destination_title,
            working,
            width,
            height,
            title_rect,
            destination_opacity,
            None,
            stats,
        );
    }

    if !hero.source_detail.is_empty() && cover_q16 < 12_000 && reveal_q16 == 0 {
        let detail_rect = fit_mask_rect(&hero.source_detail, geometry.source_detail);
        render_mask(
            &hero.source_detail,
            working,
            width,
            height,
            detail_rect,
            source_opacity,
            Some(AMBER),
            stats,
        );
    } else if reveal_q16 < 34_000 {
        let packet_width = (marquee_bounds.width / 14).max(10);
        let gap = (packet_width / 2).max(4);
        let total_width = packet_width
            .saturating_mul(5)
            .saturating_add(gap.saturating_mul(4));
        let start_x = marquee_bounds
            .x
            .saturating_add(marquee_bounds.width.saturating_sub(total_width) / 2);
        for packet in 0..5u16 {
            fill_neon_rect(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: start_x.saturating_add(packet.saturating_mul(packet_width + gap)),
                    y: marquee_bounds.bottom().saturating_add(5),
                    width: packet_width,
                    height: if packet == 2 { 2 } else { 1 },
                },
                if packet == 2 { WHITE } else { AMBER },
                stats,
            );
        }
    }
}

fn extract_mask(
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
) -> TitleMask {
    let Some((bounds, background)) = opaque_content_bounds(snapshot, width, height, rect) else {
        return TitleMask::default();
    };
    let mask_width = bounds.width as usize;
    let mask_height = bounds.height as usize;
    let mut pixels = Vec::with_capacity(mask_width.saturating_mul(mask_height));
    let mut opaque = Vec::with_capacity(mask_width.saturating_mul(mask_height));
    for y in bounds.y as usize..bounds.bottom() as usize {
        for x in bounds.x as usize..bounds.right() as usize {
            let pixel = snapshot[y * width + x];
            pixels.push(pixel);
            opaque.push(pixel != background);
        }
    }
    TitleMask {
        bounds,
        width: mask_width,
        height: mask_height,
        pixels,
        row_runs: build_row_runs(&opaque, mask_width, mask_height),
        opaque,
    }
}

fn canonical_title_mask(geometry: NavigationTransitionGeometry) -> TitleMask {
    let length = usize::from(geometry.label_len).min(geometry.label_ascii.len());
    if length == 0 {
        return TitleMask::default();
    }
    let bytes = &geometry.label_ascii[..length];
    let mut lines = [(0usize, length), (0usize, 0usize)];
    let mut line_count = 1usize;
    if length > 8 {
        let midpoint = length / 2;
        let split = bytes
            .iter()
            .enumerate()
            .filter(|(_, byte)| **byte == b' ')
            .min_by_key(|(index, _)| index.abs_diff(midpoint))
            .map(|(index, _)| index)
            .unwrap_or(midpoint);
        if split > 0 && split + 1 < length {
            lines = [(0, split), (split + 1, length)];
            line_count = 2;
        }
    }
    let columns = lines[..line_count]
        .iter()
        .map(|(start, end)| end.saturating_sub(*start))
        .max()
        .unwrap_or(1)
        .max(1);
    let mask_width = columns.saturating_mul(6).saturating_sub(1);
    let mask_height = line_count * 7 + line_count.saturating_sub(1) * 2;
    let mut opaque = vec![false; mask_width.saturating_mul(mask_height)];
    for (line_index, (start, end)) in lines[..line_count].iter().copied().enumerate() {
        let line_columns = end.saturating_sub(start);
        let line_x = columns.saturating_sub(line_columns) * 3;
        let line_y = line_index * 9;
        for (character_index, byte) in bytes[start..end].iter().copied().enumerate() {
            for (row, bits) in glyph5x7(byte).iter().copied().enumerate() {
                for column in 0..5 {
                    if bits & (1 << (4 - column)) != 0 {
                        let x = line_x + character_index * 6 + column;
                        let y = line_y + row;
                        opaque[y * mask_width + x] = true;
                    }
                }
            }
        }
    }
    TitleMask {
        bounds: geometry.source_label,
        width: mask_width,
        height: mask_height,
        pixels: vec![MINT_WHITE; mask_width.saturating_mul(mask_height)],
        row_runs: build_row_runs(&opaque, mask_width, mask_height),
        opaque,
    }
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

#[allow(clippy::too_many_arguments)]
fn render_mask(
    mask: &TitleMask,
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    opacity_q16: u16,
    colorize: Option<Rgb565Pixel>,
    stats: &mut NavigationTransitionRenderStats,
) {
    let requested_rect = rect;
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    if mask.is_empty() || opacity_q16 == 0 {
        return;
    }
    if rect == requested_rect
        && opacity_q16 == PROGRESS_MAX
        && let Some(color) = colorize
        && rect.width as usize % mask.width.max(1) == 0
        && rect.height as usize % mask.height.max(1) == 0
    {
        let scale_x = rect.width as usize / mask.width.max(1);
        let scale_y = rect.height as usize / mask.height.max(1);
        if scale_x == scale_y && (1..=4).contains(&scale_x) {
            render_integer_mask_runs(mask, working, width, rect, scale_x, color, stats);
            return;
        }
    }
    const DITHER: [[u16; 4]; 4] = [
        [0, 32_768, 8_192, 40_960],
        [49_152, 16_384, 57_344, 24_576],
        [12_288, 45_056, 4_096, 36_864],
        [61_440, 28_672, 53_248, 20_480],
    ];
    for dy in 0..rect.height as usize {
        let source_y = dy.saturating_mul(mask.height) / rect.height.max(1) as usize;
        let mut row_wrote = false;
        for dx in 0..rect.width as usize {
            if DITHER[dy & 3][dx & 3] >= opacity_q16 {
                continue;
            }
            let source_x = dx.saturating_mul(mask.width) / rect.width.max(1) as usize;
            let index = source_y.saturating_mul(mask.width).saturating_add(source_x);
            if !mask.opaque.get(index).copied().unwrap_or(false) {
                continue;
            }
            let destination = (rect.y as usize + dy) * width + rect.x as usize + dx;
            working[destination] =
                colorize.unwrap_or_else(|| mask.pixels.get(index).copied().unwrap_or(WHITE));
            row_wrote = true;
            stats.outline_pixels = stats.outline_pixels.saturating_add(1);
        }
        if row_wrote {
            stats.spans = stats.spans.saturating_add(1);
        }
    }
}

fn fit_mask_rect(mask: &TitleMask, target: NavigationTransitionRect) -> NavigationTransitionRect {
    if mask.is_empty() || target.width == 0 || target.height == 0 {
        return target;
    }
    let height = target
        .height
        .min((target.width as u32 * mask.height as u32 / mask.width.max(1) as u32).max(1) as u16)
        .max(1);
    let width = ((mask.width as u32 * height as u32) / mask.height.max(1) as u32)
        .min(target.width.max(1) as u32)
        .max(1) as u16;
    NavigationTransitionRect {
        x: target
            .x
            .saturating_add(target.width.saturating_sub(width) / 2),
        y: target
            .y
            .saturating_add(target.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

fn integer_fit_mask_rect(
    mask: &TitleMask,
    target: NavigationTransitionRect,
) -> NavigationTransitionRect {
    if mask.is_empty() || target.width == 0 || target.height == 0 {
        return target;
    }
    let scale = (target.width as usize / mask.width.max(1))
        .min(target.height as usize / mask.height.max(1))
        .clamp(1, 4);
    let width = (mask.width * scale).min(u16::MAX as usize) as u16;
    let height = (mask.height * scale).min(u16::MAX as usize) as u16;
    NavigationTransitionRect {
        x: target
            .x
            .saturating_add(target.width.saturating_sub(width) / 2),
        y: target
            .y
            .saturating_add(target.height.saturating_sub(height) / 2),
        width,
        height,
    }
}

fn quantize_mask_rect(
    mask: &TitleMask,
    target: NavigationTransitionRect,
) -> NavigationTransitionRect {
    if mask.is_empty() {
        return target;
    }
    let width_scale = (target.width as usize + mask.width / 2) / mask.width.max(1);
    let height_scale = (target.height as usize + mask.height / 2) / mask.height.max(1);
    let scale = width_scale.min(height_scale).clamp(1, 4);
    let width = (mask.width * scale).min(u16::MAX as usize) as u16;
    let height = (mask.height * scale).min(u16::MAX as usize) as u16;
    let center_x = target.x.saturating_add(target.width / 2);
    let center_y = target.y.saturating_add(target.height / 2);
    NavigationTransitionRect {
        x: center_x.saturating_sub(width / 2),
        y: center_y.saturating_sub(height / 2),
        width,
        height,
    }
}

fn build_row_runs(opaque: &[bool], width: usize, height: usize) -> Vec<MaskRun> {
    let mut runs = Vec::new();
    for y in 0..height {
        let mut x = 0;
        while x < width {
            while x < width && !opaque.get(y * width + x).copied().unwrap_or(false) {
                x += 1;
            }
            let start = x;
            while x < width && opaque.get(y * width + x).copied().unwrap_or(false) {
                x += 1;
            }
            if x > start {
                runs.push(MaskRun {
                    y: y.min(u16::MAX as usize) as u16,
                    x: start.min(u16::MAX as usize) as u16,
                    width: (x - start).min(u16::MAX as usize) as u16,
                });
            }
        }
    }
    runs
}

#[allow(clippy::too_many_arguments)]
fn render_integer_mask_runs(
    mask: &TitleMask,
    working: &mut [Rgb565Pixel],
    frame_width: usize,
    rect: NavigationTransitionRect,
    scale: usize,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let mut run_index = 0;
    for source_y in 0..mask.height {
        let first_run = run_index;
        while run_index < mask.row_runs.len() && mask.row_runs[run_index].y as usize == source_y {
            run_index += 1;
        }
        if first_run == run_index {
            continue;
        }
        for scale_y in 0..scale {
            let destination_y = rect.y as usize + source_y * scale + scale_y;
            for run in &mask.row_runs[first_run..run_index] {
                let destination_x = rect.x as usize + run.x as usize * scale;
                let pixel_count = run.width as usize * scale;
                let start = destination_y * frame_width + destination_x;
                let end = start.saturating_add(pixel_count).min(working.len());
                if end > start {
                    working[start..end].fill(color);
                    stats.outline_pixels = stats
                        .outline_pixels
                        .saturating_add(end.saturating_sub(start) as u64);
                }
            }
            stats.spans = stats.spans.saturating_add(1);
        }
    }
}

fn fill_neon_rect(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(clipped) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    fill_rect_565(working, width, height, clipped, color, stats);
    stats.spans = stats.spans.saturating_add(clipped.height as u64);
}

fn draw_corner_outline(
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
    let horizontal = rect.width.min(28).max(2);
    let vertical = rect.height.min(18).max(2);
    for x in [rect.x, rect.right().saturating_sub(horizontal)] {
        for y in [rect.y, rect.bottom().saturating_sub(1)] {
            fill_neon_rect(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x,
                    y,
                    width: horizontal,
                    height: 1,
                },
                color,
                stats,
            );
        }
    }
    for x in [rect.x, rect.right().saturating_sub(1)] {
        for y in [rect.y, rect.bottom().saturating_sub(vertical)] {
            fill_neon_rect(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x,
                    y,
                    width: 1,
                    height: vertical,
                },
                color,
                stats,
            );
        }
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
    stats.spans = stats.spans.saturating_add(1);
}

fn canonical_progress(direction: NavigationTransitionDirection, progress_q16: u16) -> u16 {
    match direction {
        NavigationTransitionDirection::Forward => progress_q16,
        NavigationTransitionDirection::Reverse => PROGRESS_MAX.saturating_sub(progress_q16),
    }
}

fn scale_segment(value: u16, end: u16) -> u16 {
    (value as u32 * PROGRESS_MAX as u32 / end.max(1) as u32).min(PROGRESS_MAX as u32) as u16
}

fn scale_from_segment(value: u16, start: u16) -> u16 {
    ((value.saturating_sub(start) as u32 * PROGRESS_MAX as u32)
        / PROGRESS_MAX.saturating_sub(start).max(1) as u32)
        .min(PROGRESS_MAX as u32) as u16
}

fn lerp_usize(from: usize, to: usize, progress_q16: u16) -> usize {
    let from = from as i64;
    let delta = to as i64 - from;
    (from + delta * progress_q16 as i64 / PROGRESS_MAX as i64).max(0) as usize
}

fn interpolate_wrapped_offset(from: u8, to: u8, progress_q16: u16) -> u8 {
    let from = usize::from(from);
    let delta = (usize::from(to) + ROW_COUNT - from) % ROW_COUNT;
    ((from + delta * progress_q16 as usize / PROGRESS_MAX as usize) % ROW_COUNT) as u8
}

fn inset_rect(
    rect: NavigationTransitionRect,
    x_pct: usize,
    y_pct: usize,
    width_pct: usize,
    height_pct: usize,
) -> NavigationTransitionRect {
    NavigationTransitionRect {
        x: rect
            .x
            .saturating_add((rect.width as usize * x_pct / 100) as u16),
        y: rect
            .y
            .saturating_add((rect.height as usize * y_pct / 100) as u16),
        width: (rect.width as usize * width_pct / 100)
            .max(1)
            .min(u16::MAX as usize) as u16,
        height: (rect.height as usize * height_pct / 100)
            .max(1)
            .min(u16::MAX as usize) as u16,
    }
}

fn primary_glass_rect(frame: NavigationTransitionRect) -> NavigationTransitionRect {
    inset_rect(frame, 3, 5, 94, 72)
}

fn offset_rect(
    rect: NavigationTransitionRect,
    dx: usize,
    dy: usize,
    width: usize,
    height: usize,
) -> NavigationTransitionRect {
    NavigationTransitionRect {
        x: (rect.x as usize)
            .saturating_add(dx)
            .min(width.saturating_sub(1))
            .min(u16::MAX as usize) as u16,
        y: (rect.y as usize)
            .saturating_add(dy)
            .min(height.saturating_sub(1))
            .min(u16::MAX as usize) as u16,
        width: rect.width,
        height: rect.height,
    }
}

fn frame_rect(width: usize, height: usize) -> NavigationTransitionRect {
    NavigationTransitionRect {
        x: 0,
        y: 0,
        width: width.min(u16::MAX as usize) as u16,
        height: height.min(u16::MAX as usize) as u16,
    }
}

fn intersect_rect(
    first: NavigationTransitionRect,
    second: NavigationTransitionRect,
) -> Option<NavigationTransitionRect> {
    let x0 = first.x.max(second.x);
    let y0 = first.y.max(second.y);
    let x1 = first.right().min(second.right());
    let y1 = first.bottom().min(second.bottom());
    if x1 > x0 && y1 > y0 {
        Some(NavigationTransitionRect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        })
    } else {
        None
    }
}

fn geometry_signature(width: usize, height: usize, geometry: NavigationTransitionGeometry) -> u64 {
    let mut value =
        geometry.label_signature ^ (width as u64).rotate_left(11) ^ (height as u64).rotate_left(27);
    for part in [
        geometry.source_card.x,
        geometry.source_card.y,
        geometry.source_card.width,
        geometry.source_card.height,
        geometry.source_label.x,
        geometry.source_label.y,
    ] {
        value ^= u64::from(part);
        value = value.wrapping_mul(0x1000_0000_01b3);
    }
    value
}

const fn edge_index(edge: NavigationTransitionEdge) -> usize {
    match edge {
        NavigationTransitionEdge::HomeToConsoles => 0,
        NavigationTransitionEdge::HomeToArcade => 1,
        NavigationTransitionEdge::ConsolesToSystem => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry_at(x: u16) -> NavigationTransitionGeometry {
        NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x,
                y: 74,
                width: 219,
                height: 448,
            },
            source_label: NavigationTransitionRect {
                x: x.saturating_add(40),
                y: 260,
                width: 140,
                height: 24,
            },
            destination_title: NavigationTransitionRect {
                x: 18,
                y: 18,
                width: 240,
                height: 28,
            },
            label_signature: u64::from(x).saturating_add(1),
            ..NavigationTransitionGeometry::default()
        }
    }

    fn small_geometry(x: u16) -> NavigationTransitionGeometry {
        NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x,
                y: 14,
                width: 40,
                height: 68,
            },
            source_label: NavigationTransitionRect {
                x: x.saturating_add(4),
                y: 40,
                width: 30,
                height: 10,
            },
            source_detail: NavigationTransitionRect {
                x: x.saturating_add(4),
                y: 52,
                width: 30,
                height: 6,
            },
            destination_title: NavigationTransitionRect {
                x: 8,
                y: 6,
                width: 64,
                height: 10,
            },
            destination_detail: NavigationTransitionRect {
                x: 8,
                y: 18,
                width: 64,
                height: 6,
            },
            destination_list: NavigationTransitionRect {
                x: 2,
                y: 24,
                width: 84,
                height: 62,
            },
            destination_selected_row: NavigationTransitionRect {
                x: 2,
                y: 46,
                width: 84,
                height: 10,
            },
            destination_preview: NavigationTransitionRect {
                x: 94,
                y: 28,
                width: 58,
                height: 48,
            },
            destination_footer: NavigationTransitionRect {
                x: 2,
                y: 82,
                width: 84,
                height: 6,
            },
            label_signature: u64::from(x).saturating_add(1),
            label_ascii: [0; 32],
            label_len: 0,
        }
    }

    #[test]
    fn projection_plans_stay_below_the_horizon_and_inside_frame() {
        for x in [18, 370, 723] {
            let mut renderer = NeonCabinetRenderer::default();
            renderer.prepare_plans(960, 540, geometry_at(x));
            assert_eq!(renderer.plans.len(), FRAME_COUNT);
            for plan in renderer.plans {
                for row in plan.rows {
                    assert!(row.y >= plan.horizon_y);
                    assert!(row.y < 540);
                    assert!(row.left < 960);
                    assert!(row.right < 960);
                    assert!(row.left <= row.right);
                }
                for cabinet in plan.cabinets {
                    assert!(cabinet.rect.x < 960);
                    assert!(cabinet.rect.y < 540);
                    assert!(cabinet.rect.right() as usize <= 960);
                    assert!(cabinet.rect.bottom() as usize <= 540);
                }
            }
        }
    }

    #[test]
    fn canonical_progress_is_an_exact_reverse_complement() {
        for progress in [0, 1, 16_384, super::super::COVER_PROGRESS, 49_152, 65_535] {
            assert_eq!(
                canonical_progress(NavigationTransitionDirection::Forward, progress),
                canonical_progress(
                    NavigationTransitionDirection::Reverse,
                    PROGRESS_MAX - progress
                )
            );
        }
    }

    #[test]
    fn declared_geometry_budgets_match_the_poc_contract() {
        assert_eq!(ROW_COUNT, 110);
        assert_eq!(MAX_QUADS, 12);
        assert_eq!(MAX_VECTOR_SEGMENTS, 96);
        assert_eq!(MAX_SPANS, 1_500);
    }

    #[test]
    fn interpolated_plans_do_not_hold_for_twenty_five_debug_steps() {
        let mut renderer = NeonCabinetRenderer::default();
        renderer.prepare_plans(960, 540, geometry_at(18));
        let mut samples = std::collections::BTreeSet::new();
        for frame_index in 0..=240u32 {
            let progress = (frame_index * u32::from(PROGRESS_MAX) / 240) as u16;
            let plan = renderer.plan(progress).unwrap();
            samples.insert((
                plan.vanishing_x,
                plan.horizon_y,
                plan.pulse_offset,
                plan.rows[40].y,
                plan.rows[40].left,
                plan.rows[80].right,
            ));
        }
        assert!(samples.len() > 80, "only {} unique plans", samples.len());
    }

    fn frame_at(progress_q16: u16) -> NavigationTransitionFrame {
        NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Expand,
            progress_q16,
            cover_progress_q16: progress_q16.min(super::super::COVER_PROGRESS),
            reveal_progress_q16: progress_q16.saturating_sub(super::super::COVER_PROGRESS),
            owns_full_frame: true,
            endpoint: None,
            failure: None,
        }
    }

    fn snapshot(width: usize, height: usize, seed: u16) -> Vec<Rgb565Pixel> {
        (0..width.saturating_mul(height))
            .map(|index| {
                Rgb565Pixel(seed.wrapping_add((index as u16).rotate_left((index & 7) as u32)))
            })
            .collect()
    }

    #[test]
    fn covered_frame_is_unchanged_by_forward_or_cold_reverse_hydration() {
        let width = 160;
        let height = 90;
        let source = snapshot(width, height, 0x0841);
        let destination = snapshot(width, height, 0x2104);
        let geometry = small_geometry(60);
        assert!(geometry.source_label.fits(width, height));
        assert!(geometry.destination_title.fits(width, height));
        for direction in [
            NavigationTransitionDirection::Forward,
            NavigationTransitionDirection::Reverse,
        ] {
            let (active, hydrated) = match direction {
                NavigationTransitionDirection::Forward => (&source, &destination),
                NavigationTransitionDirection::Reverse => (&destination, &source),
            };
            let mut renderer = NeonCabinetRenderer::default();
            renderer.prepare_transition(
                width,
                height,
                active,
                geometry,
                direction,
                NavigationTransitionEdge::HomeToArcade,
            );
            assert!(!renderer.hero.source_title.is_empty());
            let mut buffers = NavigationTransitionBuffers::new(width, height);
            buffers.begin_capture();
            buffers.capture_source(active).unwrap();
            let request = NavigationTransitionRequest::new(
                super::super::NavigationTransitionStyle::NeonCabinetDive,
                NavigationTransitionEdge::HomeToArcade,
                direction,
                geometry,
            );
            let progress = match direction {
                NavigationTransitionDirection::Forward => super::super::COVER_PROGRESS,
                NavigationTransitionDirection::Reverse => {
                    PROGRESS_MAX - super::super::COVER_PROGRESS
                }
            };
            render_neon_cabinet(&mut renderer, &mut buffers, request, frame_at(progress)).unwrap();
            let before = buffers.working().to_vec();
            buffers.capture_destination(hydrated).unwrap();
            renderer.prepare_destination(
                width,
                height,
                hydrated,
                geometry,
                direction,
                NavigationTransitionEdge::HomeToArcade,
            );
            assert!(!renderer.hero.destination_title.is_empty());
            render_neon_cabinet(&mut renderer, &mut buffers, request, frame_at(progress)).unwrap();
            assert_eq!(buffers.working(), before);
        }
    }

    #[test]
    fn separate_forward_and_reverse_renderers_are_exact_complements() {
        let width = 160;
        let height = 90;
        let source = snapshot(width, height, 0x0841);
        let destination = snapshot(width, height, 0x2104);
        let geometry = small_geometry(60);
        for canonical in [
            0,
            1,
            256,
            super::super::COVER_PROGRESS - 1,
            super::super::COVER_PROGRESS,
            super::super::COVER_PROGRESS + 1,
            49_152,
            PROGRESS_MAX - 1,
            PROGRESS_MAX,
        ] {
            let mut forward_renderer = NeonCabinetRenderer::default();
            forward_renderer.prepare_transition(
                width,
                height,
                &source,
                geometry,
                NavigationTransitionDirection::Forward,
                NavigationTransitionEdge::HomeToArcade,
            );
            forward_renderer.prepare_destination(
                width,
                height,
                &destination,
                geometry,
                NavigationTransitionDirection::Forward,
                NavigationTransitionEdge::HomeToArcade,
            );
            let mut forward_buffers = NavigationTransitionBuffers::new(width, height);
            forward_buffers.capture_source(&source).unwrap();
            forward_buffers.capture_destination(&destination).unwrap();
            let forward_request = NavigationTransitionRequest::new(
                super::super::NavigationTransitionStyle::NeonCabinetDive,
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionDirection::Forward,
                geometry,
            );
            render_neon_cabinet(
                &mut forward_renderer,
                &mut forward_buffers,
                forward_request,
                frame_at(canonical),
            )
            .unwrap();

            let mut reverse_renderer = NeonCabinetRenderer::default();
            reverse_renderer.cache_forward_source(
                width,
                height,
                &source,
                geometry,
                NavigationTransitionEdge::HomeToArcade,
            );
            reverse_renderer.cache_forward_destination(
                width,
                height,
                &destination,
                geometry,
                NavigationTransitionEdge::HomeToArcade,
            );
            reverse_renderer.prepare_transition(
                width,
                height,
                &destination,
                geometry,
                NavigationTransitionDirection::Reverse,
                NavigationTransitionEdge::HomeToArcade,
            );
            reverse_renderer.prepare_destination(
                width,
                height,
                &source,
                geometry,
                NavigationTransitionDirection::Reverse,
                NavigationTransitionEdge::HomeToArcade,
            );
            let mut reverse_buffers = NavigationTransitionBuffers::new(width, height);
            reverse_buffers.capture_source(&destination).unwrap();
            reverse_buffers.capture_destination(&source).unwrap();
            let reverse_request = NavigationTransitionRequest::new(
                super::super::NavigationTransitionStyle::NeonCabinetDive,
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionDirection::Reverse,
                geometry,
            );
            render_neon_cabinet(
                &mut reverse_renderer,
                &mut reverse_buffers,
                reverse_request,
                frame_at(PROGRESS_MAX - canonical),
            )
            .unwrap();
            assert_eq!(
                forward_buffers.working(),
                reverse_buffers.working(),
                "canonical progress {canonical}"
            );
        }
    }

    #[test]
    fn destination_endpoint_requires_a_captured_destination() {
        let width = 160;
        let height = 90;
        let source = snapshot(width, height, 0x0841);
        let geometry = small_geometry(60);
        let mut renderer = NeonCabinetRenderer::default();
        renderer.prepare_transition(
            width,
            height,
            &source,
            geometry,
            NavigationTransitionDirection::Forward,
            NavigationTransitionEdge::HomeToArcade,
        );
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        let request = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::NeonCabinetDive,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            geometry,
        );
        assert_eq!(
            render_neon_cabinet(&mut renderer, &mut buffers, request, frame_at(PROGRESS_MAX)),
            Err(NavigationTransitionFailure::SnapshotSizeMismatch)
        );
        assert_eq!(
            render_neon_cabinet(
                &mut renderer,
                &mut buffers,
                request,
                frame_at(super::super::COVER_PROGRESS + 1),
            ),
            Err(NavigationTransitionFailure::SnapshotSizeMismatch)
        );

        let mut reverse_renderer = NeonCabinetRenderer::default();
        reverse_renderer.prepare_transition(
            width,
            height,
            &source,
            geometry,
            NavigationTransitionDirection::Reverse,
            NavigationTransitionEdge::HomeToArcade,
        );
        let mut reverse_buffers = NavigationTransitionBuffers::new(width, height);
        reverse_buffers.capture_source(&source).unwrap();
        let reverse_request = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::NeonCabinetDive,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Reverse,
            geometry,
        );
        assert_eq!(
            render_neon_cabinet(
                &mut reverse_renderer,
                &mut reverse_buffers,
                reverse_request,
                frame_at(PROGRESS_MAX - (super::super::COVER_PROGRESS - 1)),
            ),
            Err(NavigationTransitionFailure::SnapshotSizeMismatch)
        );
        for canonical in [1, 128, 256] {
            assert_eq!(
                render_neon_cabinet(
                    &mut reverse_renderer,
                    &mut reverse_buffers,
                    reverse_request,
                    frame_at(PROGRESS_MAX - canonical),
                ),
                Err(NavigationTransitionFailure::SnapshotSizeMismatch),
                "reverse canonical progress {canonical}"
            );
        }
    }

    #[test]
    fn hdmi_keyframes_stay_inside_raw_render_budgets() {
        let width = 960;
        let height = 540;
        let source = vec![Rgb565Pixel(0x0841); width * height];
        let destination = vec![Rgb565Pixel(0x2104); width * height];
        let opaque_title = TitleMask {
            bounds: NavigationTransitionRect {
                x: 32,
                y: 24,
                width: 80,
                height: 16,
            },
            width: 80,
            height: 16,
            pixels: vec![MINT_WHITE; 80 * 16],
            opaque: vec![true; 80 * 16],
            row_runs: (0..16).map(|y| MaskRun { y, x: 0, width: 80 }).collect(),
        };
        let opaque_detail = TitleMask {
            bounds: NavigationTransitionRect {
                x: 32,
                y: 42,
                width: 48,
                height: 8,
            },
            width: 48,
            height: 8,
            pixels: vec![AMBER; 48 * 8],
            opaque: vec![true; 48 * 8],
            row_runs: (0..8).map(|y| MaskRun { y, x: 0, width: 48 }).collect(),
        };
        for (edge, selected_index, root_menu, label) in [
            (NavigationTransitionEdge::HomeToArcade, 0, true, "Arcade"),
            (
                NavigationTransitionEdge::HomeToConsoles,
                1,
                true,
                "Consoles",
            ),
            (
                NavigationTransitionEdge::ConsolesToSystem,
                3,
                false,
                "Atari 2600",
            ),
        ] {
            let geometry = super::super::hdmi_navigation_geometry(
                width,
                height,
                selected_index,
                0,
                root_menu,
                edge,
                label,
            );
            let mut renderer = NeonCabinetRenderer::default();
            renderer.prepare_transition(
                width,
                height,
                &source,
                geometry,
                NavigationTransitionDirection::Forward,
                edge,
            );
            renderer.hero.source_title = opaque_title.clone();
            renderer.hero.destination_title = opaque_title.clone();
            renderer.hero.canonical_title = opaque_title.clone();
            renderer.hero.source_detail = opaque_detail.clone();
            renderer.hero.destination_detail = opaque_detail.clone();
            let mut buffers = NavigationTransitionBuffers::new(width, height);
            buffers.capture_source(&source).unwrap();
            buffers.capture_destination(&destination).unwrap();
            let request = NavigationTransitionRequest::new(
                super::super::NavigationTransitionStyle::NeonCabinetDive,
                edge,
                NavigationTransitionDirection::Forward,
                geometry,
            );
            for frame_index in 0..=26 {
                let progress = super::super::forward_progress_q16_at_elapsed(
                    request.style,
                    request.duration_us,
                    frame_index * 16_667,
                );
                let stats =
                    render_neon_cabinet(&mut renderer, &mut buffers, request, frame_at(progress))
                        .unwrap();
                assert!(stats.projected_rows <= ROW_COUNT as u64);
                assert!(stats.quads <= MAX_QUADS as u64);
                assert!(stats.vector_segments <= MAX_VECTOR_SEGMENTS as u64);
                assert!(stats.spans <= MAX_SPANS as u64);
            }
        }
    }
}
