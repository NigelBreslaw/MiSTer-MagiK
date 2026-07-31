// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic card-to-PCB-to-destination navigation treatment.

use super::{
    NavigationTransitionBuffers, NavigationTransitionDirection, NavigationTransitionEdge,
    NavigationTransitionEndpoint, NavigationTransitionFailure, NavigationTransitionFrame,
    NavigationTransitionGeometry, NavigationTransitionPhase, NavigationTransitionRect,
    NavigationTransitionRenderStats, NavigationTransitionRequest, PROGRESS_MAX,
    blit_scaled_masked_565, clip_rect_to_frame, copy_rect_565, draw_outline_565,
    ease_out_cubic_q16, fill_rect_565, lerp_rect, opaque_content_bounds, smoothstep_q16,
    window_q16,
};
use crate::particle_engine::TargetMask;
use crate::particle_renderer::{
    pack_visual_command, raster_packed_visual_commands_with_palette, unpack_visual_command,
};
use slint::platform::software_renderer::Rgb565Pixel;

const FULL_PARTICLE_COUNT: usize = 2_048;
const FALLBACK_PARTICLE_COUNT: usize = 512;
const GLYPH_PACKET_COUNT: usize = 6;
const TILE_WIDTH: usize = 16;
const TILE_HEIGHT: usize = 16;

const VOID: Rgb565Pixel = Rgb565Pixel(0x00a3);
const BOARD: Rgb565Pixel = Rgb565Pixel(0x0944);
const BOARD_LIFT: Rgb565Pixel = Rgb565Pixel(0x1185);
const COPPER_SHADOW: Rgb565Pixel = Rgb565Pixel(0x5982);
const COPPER: Rgb565Pixel = Rgb565Pixel(0xc343);
const POWER: Rgb565Pixel = Rgb565Pixel(0xfe8c);
const LOGIC_MINT: Rgb565Pixel = Rgb565Pixel(0x3799);
const SILKSCREEN: Rgb565Pixel = Rgb565Pixel(0xefdf);
const STATUS_MAGENTA: Rgb565Pixel = Rgb565Pixel(0xf2d9);
const PARTICLE_PALETTE: [Rgb565Pixel; 4] = [COPPER_SHADOW, COPPER, LOGIC_MINT, SILKSCREEN];

const CHIP_PACKET_ROWS: [[u8; 7]; GLYPH_PACKET_COUNT] = [
    [
        0b00100, 0b01110, 0b11111, 0b00100, 0b11111, 0b01110, 0b00100,
    ],
    [
        0b10101, 0b01010, 0b10101, 0b01010, 0b10101, 0b01010, 0b10101,
    ],
    [
        0b11111, 0b00001, 0b11101, 0b10101, 0b10111, 0b10000, 0b11111,
    ],
    [
        0b10001, 0b11011, 0b01110, 0b00100, 0b01110, 0b11011, 0b10001,
    ],
    [
        0b00100, 0b00110, 0b11111, 0b00110, 0b00100, 0b01100, 0b00100,
    ],
    [
        0b00100, 0b01100, 0b00100, 0b00110, 0b11111, 0b00110, 0b00100,
    ],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PacketClass {
    Title,
    SelectedRow,
    List,
    Preview,
    Footer,
    Background,
}

#[derive(Clone, Debug)]
struct PreparedTitle {
    label_signature: u64,
    width: usize,
    height: usize,
    formation: Vec<u32>,
    compiled_title: Vec<u32>,
    chip_rows: [[u8; 7]; GLYPH_PACKET_COUNT],
    destination_title: Vec<u32>,
    destination_targets: Vec<u32>,
    destination_bounds: NavigationTransitionRect,
    scale: usize,
    bounds: NavigationTransitionRect,
}

#[derive(Clone, Copy, Debug)]
struct PreparedParticlePath {
    cover_route: PreparedManhattanRoute,
    reveal_route: PreparedManhattanRoute,
    class: PacketClass,
    arrival_q16: u16,
    hash: u32,
}

#[derive(Clone, Copy, Debug)]
struct PreparedManhattanRoute {
    points: [(usize, usize); 4],
    lengths: [usize; 3],
}

#[derive(Debug, Default)]
struct SemanticArrivalPlan {
    selected_strips: Vec<u16>,
    list_rows: Vec<u16>,
    footer_strips: Vec<u16>,
}

#[derive(Debug)]
pub(super) struct SpriteFoundryRenderer {
    particle_count: usize,
    formation: Vec<u32>,
    compiled_title: Vec<u32>,
    chip_rows: [[u8; 7]; GLYPH_PACKET_COUNT],
    destination_title: Vec<u32>,
    destination_targets: Vec<u32>,
    destination_title_bounds: NavigationTransitionRect,
    particle_paths: Vec<PreparedParticlePath>,
    semantic_arrivals: SemanticArrivalPlan,
    commands: Vec<u32>,
    dirty_offsets: Vec<u32>,
    title_scale: usize,
    title_bounds: NavigationTransitionRect,
    title_signature: u64,
    title_cache: [Option<PreparedTitle>; 3],
    width: usize,
    height: usize,
}

impl SpriteFoundryRenderer {
    pub(super) fn empty(particle_count: usize) -> Self {
        let particle_count = normalize_particle_count(particle_count);
        Self {
            particle_count,
            formation: Vec::new(),
            compiled_title: Vec::new(),
            chip_rows: CHIP_PACKET_ROWS,
            destination_title: Vec::new(),
            destination_targets: Vec::new(),
            destination_title_bounds: NavigationTransitionRect::default(),
            particle_paths: Vec::new(),
            semantic_arrivals: SemanticArrivalPlan::default(),
            commands: Vec::new(),
            dirty_offsets: Vec::new(),
            title_scale: 1,
            title_bounds: NavigationTransitionRect::default(),
            title_signature: 0,
            title_cache: [None, None, None],
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
        self.compiled_title.clear();
        self.chip_rows = CHIP_PACKET_ROWS;
        self.destination_title.clear();
        self.destination_targets.clear();
        self.destination_title_bounds = NavigationTransitionRect::default();
        self.particle_paths.clear();
        self.semantic_arrivals = SemanticArrivalPlan::default();
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
        self.title_scale = 1;
        self.title_bounds = NavigationTransitionRect {
            x: offset_x as u16,
            y: offset_y as u16,
            width: mask.width().min(u16::MAX as usize) as u16,
            height: mask.height().min(u16::MAX as usize) as u16,
        };
        self.formation.reserve(self.particle_count);
        for index in 0..self.particle_count {
            let point = mask.points()
                [index.saturating_mul(mask.points().len()) / self.particle_count.max(1)];
            let hash = mix32(index as u32 ^ 0x5350_5249);
            let x = (offset_x + point.x as usize).min(width.saturating_sub(1));
            let y = (offset_y + point.y as usize).min(height.saturating_sub(1));
            self.formation.push(pack_visual_command(
                (y * width + x) as u32,
                particle_palette_index(index, PacketClass::Background),
                hash & 0x20 != 0 && x + 1 < width,
            ));
        }
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
        let cache_index = transition_edge_index(edge);
        if direction == NavigationTransitionDirection::Reverse
            && let Some(cached) = self.title_cache[cache_index].as_ref()
            && cached.label_signature == geometry.label_signature
            && cached.width == width
            && cached.height == height
        {
            self.formation.clone_from(&cached.formation);
            self.compiled_title.clone_from(&cached.compiled_title);
            self.chip_rows = cached.chip_rows;
            self.destination_title.clone_from(&cached.destination_title);
            self.destination_targets
                .clone_from(&cached.destination_targets);
            self.destination_title_bounds = cached.destination_bounds;
            self.title_scale = cached.scale;
            self.title_bounds = cached.bounds;
            self.title_signature = cached.label_signature;
            self.width = width;
            self.height = height;
            self.prepare_particle_paths(geometry);
            return;
        }
        // Force a direction-neutral fallback before inspecting the next label,
        // so a malformed capture can never reuse the previous transition's title.
        self.width = 0;
        self.height = 0;
        self.prepare(width, height);
        self.title_signature = geometry.label_signature;
        self.compiled_title.clear();
        if source.len() != width.saturating_mul(height) || width == 0 || height == 0 {
            self.finish_preparation(cache_index, geometry);
            return;
        }
        let label = match direction {
            NavigationTransitionDirection::Forward => geometry.source_label,
            NavigationTransitionDirection::Reverse => geometry.destination_title,
        };
        let Some(label) = clip_rect_to_frame(label, width, height) else {
            self.finish_preparation(cache_index, geometry);
            return;
        };
        let mut histogram = vec![0u32; u16::MAX as usize + 1];
        for y in label.y as usize..label.bottom() as usize {
            for x in label.x as usize..label.right() as usize {
                let color = source[y * width + x].0 as usize;
                histogram[color] = histogram[color].saturating_add(1);
            }
        }
        let background = Rgb565Pixel(
            histogram
                .iter()
                .enumerate()
                .max_by_key(|(_, count)| **count)
                .map(|(color, _)| color as u16)
                .unwrap_or(0),
        );
        let mut points = Vec::with_capacity(label.width as usize * label.height as usize / 3);
        for y in label.y as usize..label.bottom() as usize {
            for x in label.x as usize..label.right() as usize {
                if color_distance_565(source[y * width + x], background) >= 10 {
                    points.push((x, y));
                }
            }
        }
        if points.len() < 8 {
            self.finish_preparation(cache_index, geometry);
            return;
        }
        let min_x = points
            .iter()
            .map(|point| point.0)
            .min()
            .unwrap_or(label.x as usize);
        let max_x = points.iter().map(|point| point.0).max().unwrap_or(min_x);
        let min_y = points
            .iter()
            .map(|point| point.1)
            .min()
            .unwrap_or(label.y as usize);
        let max_y = points.iter().map(|point| point.1).max().unwrap_or(min_y);
        let source_width = max_x.saturating_sub(min_x).saturating_add(1).max(1);
        let source_height = max_y.saturating_sub(min_y).saturating_add(1).max(1);
        if direction == NavigationTransitionDirection::Reverse {
            self.destination_title.clear();
            self.destination_title.reserve(points.len());
            for &(x, y) in &points {
                self.destination_title
                    .push(pack_visual_command((y * width + x) as u32, 2, false));
            }
            self.destination_title_bounds = NavigationTransitionRect {
                x: min_x as u16,
                y: min_y as u16,
                width: source_width.min(u16::MAX as usize) as u16,
                height: source_height.min(u16::MAX as usize) as u16,
            };
        }
        self.chip_rows = title_chip_rows(&points, min_x, min_y, source_width, source_height);
        let max_target_width = width.saturating_mul(3) / 5;
        let max_target_height = height / 5;
        let scale = (max_target_width / source_width)
            .min(max_target_height / source_height)
            .clamp(2, 8);
        let target_width = source_width.saturating_mul(scale);
        let target_height = source_height.saturating_mul(scale);
        let offset_x = width.saturating_sub(target_width) / 2;
        let offset_y = height.saturating_sub(target_height) / 2;
        self.title_scale = scale;
        self.title_bounds = NavigationTransitionRect {
            x: offset_x as u16,
            y: offset_y as u16,
            width: target_width.min(u16::MAX as usize) as u16,
            height: target_height.min(u16::MAX as usize) as u16,
        };
        self.compiled_title.clear();
        self.compiled_title.reserve(points.len());
        for (index, point) in points.iter().copied().enumerate() {
            let x = offset_x
                .saturating_add(point.0.saturating_sub(min_x).saturating_mul(scale))
                .min(width.saturating_sub(1));
            let y = offset_y
                .saturating_add(point.1.saturating_sub(min_y).saturating_mul(scale))
                .min(height.saturating_sub(1));
            self.compiled_title.push(pack_visual_command(
                (y * width + x) as u32,
                2 + (index & 1),
                false,
            ));
        }
        self.formation.clear();
        self.formation.reserve(self.particle_count);
        for index in 0..self.particle_count {
            let command = self.compiled_title
                [index.saturating_mul(self.compiled_title.len()) / self.particle_count.max(1)];
            let (offset, _, _) = unpack_visual_command(command).unwrap_or((0, 0, false));
            let x = offset % width;
            let y = offset / width;
            self.formation.push(pack_visual_command(
                (y * width + x) as u32,
                2 + (index & 1),
                index & 3 == 0 && x + 1 < width,
            ));
        }
        if direction == NavigationTransitionDirection::Reverse {
            self.prepare_exact_title_targets(width);
        }
        self.finish_preparation(cache_index, geometry);
    }

    fn cache_current_title(&mut self, cache_index: usize) {
        self.title_cache[cache_index] = Some(PreparedTitle {
            label_signature: self.title_signature,
            width: self.width,
            height: self.height,
            formation: self.formation.clone(),
            compiled_title: self.compiled_title.clone(),
            chip_rows: self.chip_rows,
            destination_title: self.destination_title.clone(),
            destination_targets: self.destination_targets.clone(),
            destination_bounds: self.destination_title_bounds,
            scale: self.title_scale,
            bounds: self.title_bounds,
        });
    }

    pub(super) fn prepare_destination_title(
        &mut self,
        width: usize,
        height: usize,
        destination: &[Rgb565Pixel],
        rect: NavigationTransitionRect,
        edge: NavigationTransitionEdge,
    ) {
        self.destination_title.clear();
        self.destination_targets.clear();
        self.destination_title_bounds = NavigationTransitionRect::default();
        if destination.len() != width.saturating_mul(height) {
            return;
        }
        let Some(rect) = clip_rect_to_frame(rect, width, height) else {
            return;
        };
        let mut histogram = vec![0u32; u16::MAX as usize + 1];
        for y in rect.y as usize..rect.bottom() as usize {
            for x in rect.x as usize..rect.right() as usize {
                let color = destination[y * width + x].0 as usize;
                histogram[color] = histogram[color].saturating_add(1);
            }
        }
        let background = Rgb565Pixel(
            histogram
                .iter()
                .enumerate()
                .max_by_key(|(_, count)| **count)
                .map(|(color, _)| color as u16)
                .unwrap_or(0),
        );
        for y in rect.y as usize..rect.bottom() as usize {
            for x in rect.x as usize..rect.right() as usize {
                if color_distance_565(destination[y * width + x], background) >= 10 {
                    self.destination_title.push(pack_visual_command(
                        (y * width + x) as u32,
                        2,
                        false,
                    ));
                }
            }
        }
        if let (Some(min_x), Some(max_x), Some(min_y), Some(max_y)) = (
            self.destination_title
                .iter()
                .filter_map(|command| unpack_visual_command(*command))
                .map(|(offset, _, _)| offset % width)
                .min(),
            self.destination_title
                .iter()
                .filter_map(|command| unpack_visual_command(*command))
                .map(|(offset, _, _)| offset % width)
                .max(),
            self.destination_title
                .iter()
                .filter_map(|command| unpack_visual_command(*command))
                .map(|(offset, _, _)| offset / width)
                .min(),
            self.destination_title
                .iter()
                .filter_map(|command| unpack_visual_command(*command))
                .map(|(offset, _, _)| offset / width)
                .max(),
        ) {
            self.destination_title_bounds = NavigationTransitionRect {
                x: min_x as u16,
                y: min_y as u16,
                width: max_x.saturating_sub(min_x).saturating_add(1) as u16,
                height: max_y.saturating_sub(min_y).saturating_add(1) as u16,
            };
        }
        self.prepare_exact_title_targets(width);
        self.cache_current_title(transition_edge_index(edge));
    }

    fn prepare_exact_title_targets(&mut self, width: usize) {
        self.destination_targets.clear();
        if self.destination_title.is_empty()
            || self.compiled_title.is_empty()
            || self.destination_title_bounds.width == 0
            || self.destination_title_bounds.height == 0
        {
            return;
        }
        self.destination_targets.reserve(self.compiled_title.len());
        for source in self.compiled_title.iter().copied() {
            let Some((source_offset, _, _)) = unpack_visual_command(source) else {
                continue;
            };
            let source_x = source_offset % width;
            let source_y = source_offset / width;
            let relative_x = source_x.saturating_sub(self.title_bounds.x as usize);
            let relative_y = source_y.saturating_sub(self.title_bounds.y as usize);
            let expected_x = self.destination_title_bounds.x as usize
                + relative_x.saturating_mul(self.destination_title_bounds.width as usize)
                    / self.title_bounds.width.max(1) as usize;
            let expected_y = self.destination_title_bounds.y as usize
                + relative_y.saturating_mul(self.destination_title_bounds.height as usize)
                    / self.title_bounds.height.max(1) as usize;
            let target = self
                .destination_title
                .iter()
                .copied()
                .min_by_key(|command| {
                    unpack_visual_command(*command)
                        .map(|(offset, _, _)| {
                            (offset % width)
                                .abs_diff(expected_x)
                                .saturating_add((offset / width).abs_diff(expected_y))
                        })
                        .unwrap_or(usize::MAX)
                })
                .unwrap_or(source);
            self.destination_targets.push(target);
        }
    }

    fn finish_preparation(&mut self, cache_index: usize, geometry: NavigationTransitionGeometry) {
        self.cache_current_title(cache_index);
        self.prepare_particle_paths(geometry);
    }

    fn prepare_particle_paths(&mut self, geometry: NavigationTransitionGeometry) {
        self.particle_paths.clear();
        self.particle_paths.reserve(self.particle_count);
        for index in 0..self.particle_count {
            let (destination, class) = semantic_destination_point(
                index,
                self.particle_count,
                geometry,
                self.width,
                self.height,
            );
            let tile_index = destination.1 / TILE_HEIGHT * self.width.div_ceil(TILE_WIDTH)
                + destination.0 / TILE_WIDTH;
            let source = source_packet_point(index, self.particle_count, geometry.source_card);
            let hub = self
                .formation
                .get(index)
                .copied()
                .and_then(unpack_visual_command)
                .map(|(offset, _, _)| (offset % self.width, offset / self.width))
                .unwrap_or(rect_center(geometry.source_card));
            self.particle_paths.push(PreparedParticlePath {
                cover_route: prepare_manhattan_route(source, hub, index, self.width),
                reveal_route: prepare_manhattan_route(hub, destination, index, self.width),
                class,
                arrival_q16: tile_arrival_q16(tile_index, destination, class, geometry),
                hash: mix32(index as u32 ^ 0xbb67_ae85),
            });
        }
        self.prepare_semantic_arrivals(geometry);
    }

    fn prepare_semantic_arrivals(&mut self, geometry: NavigationTransitionGeometry) {
        self.semantic_arrivals.selected_strips = unit_arrivals_for_horizontal_units(
            &self.particle_paths,
            PacketClass::SelectedRow,
            geometry.destination_selected_row,
            10,
        );
        self.semantic_arrivals.footer_strips = unit_arrivals_for_horizontal_units(
            &self.particle_paths,
            PacketClass::Footer,
            geometry.destination_footer,
            14,
        );
        let row_height = geometry.destination_selected_row.height.max(28) as usize;
        self.semantic_arrivals.list_rows = unit_arrivals_for_vertical_units(
            &self.particle_paths,
            PacketClass::List,
            geometry.destination_list,
            row_height,
        );
    }

    #[cfg(test)]
    pub(super) const fn particle_count(&self) -> usize {
        self.particle_count
    }
}

fn unit_arrivals_for_horizontal_units(
    paths: &[PreparedParticlePath],
    class: PacketClass,
    rect: NavigationTransitionRect,
    unit_width: usize,
) -> Vec<u16> {
    let unit_count = (rect.width as usize).div_ceil(unit_width.max(1)).max(1);
    let class_max = paths
        .iter()
        .filter(|path| path.class == class)
        .map(|path| path.arrival_q16)
        .max()
        .unwrap_or(57_500);
    let mut arrivals = vec![0u16; unit_count];
    for path in paths.iter().filter(|path| path.class == class) {
        let x = path.reveal_route.points[3].0;
        let unit = x
            .saturating_sub(rect.x as usize)
            .saturating_div(unit_width.max(1))
            .min(unit_count - 1);
        arrivals[unit] = arrivals[unit].max(path.arrival_q16);
    }
    for arrival in &mut arrivals {
        if *arrival == 0 {
            *arrival = class_max;
        }
    }
    arrivals
}

fn unit_arrivals_for_vertical_units(
    paths: &[PreparedParticlePath],
    class: PacketClass,
    rect: NavigationTransitionRect,
    unit_height: usize,
) -> Vec<u16> {
    let unit_count = (rect.height as usize).div_ceil(unit_height.max(1)).max(1);
    let class_max = paths
        .iter()
        .filter(|path| path.class == class)
        .map(|path| path.arrival_q16)
        .max()
        .unwrap_or(57_500);
    let mut arrivals = vec![0u16; unit_count];
    for path in paths.iter().filter(|path| path.class == class) {
        let y = path.reveal_route.points[3].1;
        let unit = y
            .saturating_sub(rect.y as usize)
            .saturating_div(unit_height.max(1))
            .min(unit_count - 1);
        arrivals[unit] = arrivals[unit].max(path.arrival_q16);
    }
    for arrival in &mut arrivals {
        if *arrival == 0 {
            *arrival = class_max;
        }
    }
    arrivals
}

fn color_distance_565(a: Rgb565Pixel, b: Rgb565Pixel) -> u16 {
    let ar = (a.0 >> 11) & 0x1f;
    let ag = (a.0 >> 5) & 0x3f;
    let ab = a.0 & 0x1f;
    let br = (b.0 >> 11) & 0x1f;
    let bg = (b.0 >> 5) & 0x3f;
    let bb = b.0 & 0x1f;
    ar.abs_diff(br)
        .saturating_add(ag.abs_diff(bg))
        .saturating_add(ab.abs_diff(bb))
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
    let source = buffers
        .source
        .get(..)
        .filter(|_| buffers.source_ready)
        .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
    let destination = buffers
        .destination
        .get(..)
        .filter(|_| buffers.destination_ready);
    let width = buffers.width;
    let height = buffers.height;
    let working = buffers.working.as_mut_slice();
    if working.len() != source.len() || working.len() != width.saturating_mul(height) {
        return Err(NavigationTransitionFailure::SnapshotSizeMismatch);
    }
    let mut stats = NavigationTransitionRenderStats::default();

    if frame.phase == NavigationTransitionPhase::Settled {
        let endpoint = match frame.endpoint {
            Some(NavigationTransitionEndpoint::Destination) => destination.unwrap_or(source),
            _ => source,
        };
        working.copy_from_slice(endpoint);
        stats.copied_pixels = endpoint.len() as u64;
        return Ok(stats);
    }
    if frame.progress_q16 == 0 {
        working.copy_from_slice(source);
        stats.copied_pixels = source.len() as u64;
        return Ok(stats);
    }

    renderer.prepare(width, height);
    let canonical = canonical_progress_q16(request.direction, frame);
    let (canonical_source, canonical_destination) = match request.direction {
        NavigationTransitionDirection::Forward => (Some(source), destination),
        NavigationTransitionDirection::Reverse => (destination, Some(source)),
    };
    let cover_boundary = request.style.cover_progress_q16();
    if canonical == cover_boundary {
        render_covered_board(
            working,
            width,
            height,
            request.geometry,
            renderer.chip_rows,
            &mut stats,
        );
        draw_foundry_packets(renderer, working, PROGRESS_MAX, 0, &mut stats);
        draw_foundry_rim(
            working,
            width,
            height,
            frame_rect(width, height),
            &mut stats,
        );
        draw_compiled_title(
            renderer,
            working,
            request.geometry.destination_title,
            PROGRESS_MAX,
            0,
            &mut stats,
        );
    } else if canonical < cover_boundary {
        let cover = scale_to_segment(canonical, cover_boundary);
        let fallback = canonical_destination.unwrap_or(source);
        if cover <= 6_500 {
            let exact_source = canonical_source.unwrap_or(fallback);
            working.copy_from_slice(exact_source);
            stats.copied_pixels = exact_source.len() as u64;
            return Ok(stats);
        }
        render_card_to_board(
            working,
            canonical_source.unwrap_or(fallback),
            width,
            height,
            request.geometry,
            renderer.title_bounds,
            renderer.chip_rows,
            cover,
            &mut stats,
        );
        draw_foundry_packets(renderer, working, cover, 0, &mut stats);
        draw_foundry_rim(
            working,
            width,
            height,
            foundry_board_rect(
                request.geometry.source_card,
                frame_rect(width, height),
                cover,
            ),
            &mut stats,
        );
        draw_compiled_title(
            renderer,
            working,
            request.geometry.destination_title,
            smoothstep_q16(window_q16(cover, 42_000, 60_000)),
            0,
            &mut stats,
        );
    } else {
        let reveal = scale_from_segment(canonical, cover_boundary);
        let destination = canonical_destination.unwrap_or(source);
        if reveal >= 62_500 {
            working.copy_from_slice(destination);
            stats.copied_pixels = destination.len() as u64;
            return Ok(stats);
        }
        render_destination_board_base(
            working,
            width,
            height,
            request.geometry,
            renderer.chip_rows,
            reveal,
            &mut stats,
        );
        draw_foundry_packets(renderer, working, PROGRESS_MAX, reveal, &mut stats);
        draw_compiled_title(
            renderer,
            working,
            request.geometry.destination_title,
            PROGRESS_MAX,
            smoothstep_q16(window_q16(reveal, 0, 18_000)),
            &mut stats,
        );
        if reveal < 14_000 {
            draw_foundry_rim(
                working,
                width,
                height,
                frame_rect(width, height),
                &mut stats,
            );
        }
        render_destination_layers(
            working,
            destination,
            width,
            height,
            request.geometry,
            &renderer.semantic_arrivals,
            reveal,
            &mut stats,
        );
    }
    Ok(stats)
}

fn draw_compiled_title(
    renderer: &mut SpriteFoundryRenderer,
    working: &mut [Rgb565Pixel],
    destination_title: NavigationTransitionRect,
    visibility_q16: u16,
    travel_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if visibility_q16 == 0 || renderer.compiled_title.is_empty() {
        return;
    }
    let target_bounds = if renderer.destination_title_bounds.width != 0
        && renderer.destination_title_bounds.height != 0
    {
        renderer.destination_title_bounds
    } else {
        destination_title
    };
    let hero_center = rect_center(renderer.title_bounds);
    let target_center = rect_center(target_bounds);
    let conduit_progress = smoothstep_q16(window_q16(travel_q16, 0, 52_000));
    let conduit_center = manhattan_position(
        hero_center,
        target_center,
        conduit_progress,
        1,
        renderer.width,
        renderer.height,
    );
    let compression = smoothstep_q16(window_q16(travel_q16, 30_000, 58_000));
    let rigid_scale = lerp_usize(renderer.title_scale, 1, compression).max(1);
    let topology_morph = smoothstep_q16(window_q16(travel_q16, 52_000, PROGRESS_MAX));
    if travel_q16 > 0 && travel_q16 < PROGRESS_MAX {
        draw_manhattan_track(
            working,
            renderer.width,
            renderer.height,
            frame_rect(renderer.width, renderer.height),
            hero_center,
            target_center,
            1,
            conduit_progress,
            conduit_progress,
            stats,
        );
    }
    for (index, command) in renderer.compiled_title.iter().copied().enumerate() {
        let Some((offset, palette_index, _)) = unpack_visual_command(command) else {
            continue;
        };
        let source_x = offset % renderer.width;
        let source_y = offset / renderer.width;
        let relative_x = source_x.saturating_sub(renderer.title_bounds.x as usize);
        let relative_y = source_y.saturating_sub(renderer.title_bounds.y as usize);
        let column_order =
            relative_x.saturating_mul(58_000) / renderer.title_bounds.width.max(1) as usize;
        let row_shimmer = (relative_y % renderer.title_scale.max(1)).saturating_mul(800);
        if column_order
            .saturating_add(row_shimmer)
            .min(PROGRESS_MAX as usize) as u16
            > visibility_q16
        {
            continue;
        }
        let exact_target = renderer
            .destination_targets
            .get(index)
            .copied()
            .and_then(unpack_visual_command)
            .map(|(offset, _, _)| (offset % renderer.width, offset / renderer.width));
        let target_x = exact_target.map_or_else(
            || {
                target_bounds.x as usize
                    + relative_x.saturating_mul(target_bounds.width as usize)
                        / renderer.title_bounds.width.max(1) as usize
            },
            |target| target.0,
        );
        let target_y = exact_target.map_or_else(
            || {
                target_bounds.y as usize
                    + relative_y.saturating_mul(target_bounds.height as usize)
                        / renderer.title_bounds.height.max(1) as usize
            },
            |target| target.1,
        );
        let rigid_x = (conduit_center.0 as isize)
            .saturating_add(
                (source_x as isize - hero_center.0 as isize).saturating_mul(rigid_scale as isize)
                    / renderer.title_scale.max(1) as isize,
            )
            .max(0) as usize;
        let rigid_y = (conduit_center.1 as isize)
            .saturating_add(
                (source_y as isize - hero_center.1 as isize).saturating_mul(rigid_scale as isize)
                    / renderer.title_scale.max(1) as isize,
            )
            .max(0) as usize;
        let x = lerp_usize(rigid_x, target_x, topology_morph);
        let y = lerp_usize(rigid_y, target_y, topology_morph);
        fill_rect_565(
            working,
            renderer.width,
            renderer.height,
            NavigationTransitionRect {
                x: x as u16,
                y: y as u16,
                width: rigid_scale as u16,
                height: rigid_scale as u16,
            },
            PARTICLE_PALETTE[palette_index.min(PARTICLE_PALETTE.len() - 1)],
            stats,
        );
    }
}

fn canonical_progress_q16(
    direction: NavigationTransitionDirection,
    frame: NavigationTransitionFrame,
) -> u16 {
    match direction {
        NavigationTransitionDirection::Forward => frame.progress_q16,
        NavigationTransitionDirection::Reverse => PROGRESS_MAX.saturating_sub(frame.progress_q16),
    }
}

fn render_card_to_board(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    compiled_title_rect: NavigationTransitionRect,
    chip_rows: [[u8; 7]; GLYPH_PACKET_COUNT],
    cover_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if cover_q16 <= 6_500 {
        working.copy_from_slice(source);
        stats.copied_pixels = source.len() as u64;
        return;
    }
    let full = frame_rect(width, height);
    let active = foundry_board_rect(geometry.source_card, full, cover_q16);
    copy_source_outside_rect(working, source, width, height, active, stats);
    draw_board_shadow(working, width, height, active, stats);
    fill_board_surface(working, width, height, active, stats);
    draw_ground_planes(working, width, height, active, stats);
    draw_destination_footprints(working, width, height, active, geometry, stats);

    let detach = smoothstep_q16(window_q16(cover_q16, 8_000, 56_000));
    copy_undocked_card_tiles(working, source, width, height, geometry, detach, stats);
    if cover_q16 < 45_000 {
        draw_source_latch(
            working,
            width,
            height,
            geometry.source_card,
            cover_q16,
            stats,
        );
    }
    draw_connected_buses(
        working,
        width,
        height,
        geometry,
        active,
        smoothstep_q16(window_q16(cover_q16, 7_000, 57_000)),
        window_q16(cover_q16, 43_000, PROGRESS_MAX),
        stats,
    );
    draw_chip_packets(
        working,
        width,
        height,
        geometry,
        chip_rows,
        window_q16(cover_q16, 37_000, 60_000),
        0,
        stats,
    );
    move_title_pixels_to_bounds(
        working,
        source,
        width,
        height,
        geometry.source_label,
        compiled_title_rect,
        smoothstep_q16(window_q16(cover_q16, 8_000, 52_000)),
        stats,
    );
}

fn copy_source_outside_rect(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        working.copy_from_slice(source);
        stats.copied_pixels = stats.copied_pixels.saturating_add(source.len() as u64);
        return;
    };
    for y in 0..height {
        if y < rect.y as usize || y >= rect.bottom() as usize {
            let start = y * width;
            working[start..start + width].copy_from_slice(&source[start..start + width]);
            stats.copied_pixels = stats.copied_pixels.saturating_add(width as u64);
            continue;
        }
        let row = y * width;
        let left = rect.x as usize;
        if left != 0 {
            working[row..row + left].copy_from_slice(&source[row..row + left]);
            stats.copied_pixels = stats.copied_pixels.saturating_add(left as u64);
        }
        let right = rect.right() as usize;
        if right < width {
            working[row + right..row + width].copy_from_slice(&source[row + right..row + width]);
            stats.copied_pixels = stats
                .copied_pixels
                .saturating_add(width.saturating_sub(right) as u64);
        }
    }
}

fn move_title_pixels_to_bounds(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    from: NavigationTransitionRect,
    to: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some((content, background)) = opaque_content_bounds(source, width, height, from) else {
        return;
    };
    let moving = lerp_rect(content, to, progress_q16);
    blit_scaled_masked_565(
        working, source, width, height, content, moving, background, stats,
    );
}

fn render_covered_board(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    chip_rows: [[u8; 7]; GLYPH_PACKET_COUNT],
    stats: &mut NavigationTransitionRenderStats,
) {
    let full = frame_rect(width, height);
    fill_board_surface(working, width, height, full, stats);
    draw_ground_planes(working, width, height, full, stats);
    draw_destination_footprints(working, width, height, full, geometry, stats);
    draw_connected_buses(
        working,
        width,
        height,
        geometry,
        full,
        PROGRESS_MAX,
        PROGRESS_MAX,
        stats,
    );
    draw_chip_packets(
        working,
        width,
        height,
        geometry,
        chip_rows,
        PROGRESS_MAX,
        0,
        stats,
    );
}

fn render_destination_board_base(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    chip_rows: [[u8; 7]; GLYPH_PACKET_COUNT],
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let full = frame_rect(width, height);
    fill_board_surface(working, width, height, full, stats);
    draw_ground_planes(working, width, height, full, stats);
    draw_destination_footprints(working, width, height, full, geometry, stats);
    let route_visibility = PROGRESS_MAX.saturating_sub(window_q16(reveal_q16, 46_000, 61_500));
    draw_connected_buses(
        working,
        width,
        height,
        geometry,
        full,
        PROGRESS_MAX,
        route_visibility,
        stats,
    );
    draw_chip_packets(
        working,
        width,
        height,
        geometry,
        chip_rows,
        PROGRESS_MAX,
        window_q16(reveal_q16, 1_500, 31_000),
        stats,
    );
}

fn render_destination_layers(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    semantic_arrivals: &SemanticArrivalPlan,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    assemble_destination_tiles(
        working,
        destination,
        width,
        height,
        geometry,
        semantic_arrivals,
        reveal_q16,
        stats,
    );
    // The selected label is the invariant object. It is docked before the
    // payload fan-out and repainted last so packets never damage it.
    copy_region_without_background_progress(
        working,
        destination,
        width,
        height,
        geometry.destination_title,
        smoothstep_q16(window_q16(reveal_q16, 14_000, 26_000)),
        stats,
    );
    if reveal_q16 >= 26_000 {
        copy_region_without_background(
            working,
            destination,
            width,
            height,
            geometry.destination_detail,
            stats,
        );
    }
    draw_destination_aperture_rims(working, width, height, geometry, reveal_q16, stats);
    draw_verification_beam(working, destination, width, height, reveal_q16, stats);
}

fn copy_region_without_background(
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
    let sample_x = rect.right().saturating_sub(1) as usize;
    let sample_y = rect.bottom().saturating_sub(1) as usize;
    let background = source[sample_y * width + sample_x];
    for y in rect.y as usize..rect.bottom() as usize {
        for x in rect.x as usize..rect.right() as usize {
            let pixel = source[y * width + x];
            if pixel != background {
                working[y * width + x] = pixel;
                stats.copied_pixels = stats.copied_pixels.saturating_add(1);
            }
        }
    }
}

fn copy_region_without_background_progress(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    progress_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if progress_q16 == 0 {
        return;
    }
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let background = source[(rect.bottom().saturating_sub(1) as usize) * width
        + rect.right().saturating_sub(1) as usize];
    let visible_width = (rect.width as u32 * progress_q16 as u32 / PROGRESS_MAX as u32)
        .max(1)
        .min(rect.width as u32) as usize;
    for y in rect.y as usize..rect.bottom() as usize {
        for x in rect.x as usize..rect.x as usize + visible_width {
            let pixel = source[y * width + x];
            if pixel != background {
                working[y * width + x] = pixel;
                stats.copied_pixels = stats.copied_pixels.saturating_add(1);
            }
        }
    }
}

fn foundry_board_rect(
    source_card: NavigationTransitionRect,
    full: NavigationTransitionRect,
    cover_q16: u16,
) -> NavigationTransitionRect {
    let die = NavigationTransitionRect {
        x: (full.width as usize / 14).min(u16::MAX as usize) as u16,
        y: (full.height as usize / 10).min(u16::MAX as usize) as u16,
        width: full.width.saturating_mul(6) / 7,
        height: full.height.saturating_mul(4) / 5,
    };
    if cover_q16 <= 28_000 {
        lerp_rect(
            source_card,
            die,
            ease_out_cubic_q16(window_q16(cover_q16, 6_000, 28_000)),
        )
    } else {
        lerp_rect(
            die,
            full,
            smoothstep_q16(window_q16(cover_q16, 28_000, 61_000)),
        )
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

fn draw_board_shadow(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    fill_rect_565(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: rect.x.saturating_add(6),
            y: rect.y.saturating_add(6),
            width: rect.width,
            height: rect.height,
        },
        VOID,
        stats,
    );
}

fn fill_board_surface(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let cuts = [0usize, 17, 43, 68, 88, 100];
    for y in rect.y as usize..rect.bottom() as usize {
        for zone in 0..cuts.len() - 1 {
            let x0 = rect.x as usize + rect.width as usize * cuts[zone] / 100;
            let x1 = rect.x as usize + rect.width as usize * cuts[zone + 1] / 100;
            working[y * width + x0..y * width + x1].fill(if zone == 1 || zone == 3 {
                BOARD_LIFT
            } else {
                BOARD
            });
            stats.filled_pixels = stats
                .filled_pixels
                .saturating_add(x1.saturating_sub(x0) as u64);
        }
    }
}

fn draw_ground_planes(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    if rect.width < 48 || rect.height < 36 {
        return;
    }
    for seam_pct in [17usize, 43, 68, 88] {
        let x = rect.x as usize + rect.width as usize * seam_pct / 100;
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: x as u16,
                y: rect.y.saturating_add(8),
                width: 1,
                height: rect.height.saturating_sub(16),
            },
            COPPER_SHADOW,
            stats,
        );
    }
    for seam_pct in [29usize, 61, 82] {
        let y = rect.y as usize + rect.height as usize * seam_pct / 100;
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: rect.x.saturating_add(8),
                y: y as u16,
                width: rect.width.saturating_sub(16),
                height: 1,
            },
            COPPER_SHADOW,
            stats,
        );
    }
    for x_pct in [17usize, 43, 68, 88] {
        for y_pct in [29usize, 61, 82] {
            draw_via(
                working,
                width,
                height,
                (
                    rect.x as usize + rect.width as usize * x_pct / 100,
                    rect.y as usize + rect.height as usize * y_pct / 100,
                ),
                stats,
            );
        }
    }
}

fn draw_destination_footprints(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    board: NavigationTransitionRect,
    geometry: NavigationTransitionGeometry,
    stats: &mut NavigationTransitionRenderStats,
) {
    for (rect, color) in [
        (geometry.destination_title, COPPER),
        (geometry.destination_list, COPPER_SHADOW),
        (geometry.destination_selected_row, COPPER),
        (geometry.destination_preview, COPPER_SHADOW),
        (geometry.destination_footer, COPPER_SHADOW),
    ] {
        if let Some(footprint) = intersect_rect(rect, board) {
            draw_outline_565(working, width, height, footprint, color, stats);
        }
    }
    if let Some(preview) = intersect_rect(geometry.destination_preview, board)
        && preview.width > 12
        && preview.height > 12
    {
        draw_outline_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: preview.x.saturating_add(6),
                y: preview.y.saturating_add(6),
                width: preview.width.saturating_sub(12),
                height: preview.height.saturating_sub(12),
            },
            COPPER_SHADOW,
            stats,
        );
        for pin in (10..(preview.width as usize).saturating_sub(8)).step_by(20) {
            for y in [
                preview.y.saturating_add(2),
                preview.bottom().saturating_sub(4),
            ] {
                fill_rect_565(
                    working,
                    width,
                    height,
                    NavigationTransitionRect {
                        x: preview.x.saturating_add(pin as u16),
                        y,
                        width: 5,
                        height: 2,
                    },
                    COPPER_SHADOW,
                    stats,
                );
            }
        }
    }
    let list_bank_x = geometry.destination_list.right().saturating_add(12);
    for resistor in 0..6u16 {
        let y = geometry
            .destination_list
            .y
            .saturating_add(32)
            .saturating_add(resistor.saturating_mul(14));
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: list_bank_x,
                y,
                width: 18,
                height: 4,
            },
            COPPER_SHADOW,
            stats,
        );
        for x in [
            list_bank_x.saturating_sub(3),
            list_bank_x.saturating_add(18),
        ] {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x,
                    y: y.saturating_add(1),
                    width: 3,
                    height: 1,
                },
                COPPER,
                stats,
            );
        }
    }
    for point in [
        (
            geometry.destination_title.right().saturating_add(12) as usize,
            geometry.destination_title.y.saturating_add(5) as usize,
        ),
        (
            geometry.destination_selected_row.right().saturating_add(12) as usize,
            geometry.destination_selected_row.y.saturating_add(8) as usize,
        ),
        (
            geometry.destination_preview.x.saturating_sub(12) as usize,
            geometry.destination_preview.y.saturating_add(18) as usize,
        ),
    ] {
        if point_in_rect_inclusive(point, board) {
            draw_via(working, width, height, point, stats);
        }
    }
}

fn point_in_rect_inclusive(point: (usize, usize), rect: NavigationTransitionRect) -> bool {
    point.0 >= rect.x as usize
        && point.1 >= rect.y as usize
        && point.0 < rect.right() as usize
        && point.1 < rect.bottom() as usize
}

fn copy_undocked_card_tiles(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    detach_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(card) = clip_rect_to_frame(geometry.source_card, width, height) else {
        return;
    };
    if detach_q16 >= 61_000 {
        return;
    }
    let remaining_q16 = PROGRESS_MAX.saturating_sub(window_q16(detach_q16, 0, 61_000));
    let remaining_width =
        (card.width as usize * remaining_q16 as usize / PROGRESS_MAX as usize).max(1);
    let remaining_height =
        (card.height as usize * remaining_q16 as usize / PROGRESS_MAX as usize).max(1);
    let label_center = rect_center(geometry.source_label);
    let x = label_center.0.saturating_sub(remaining_width / 2).clamp(
        card.x as usize,
        card.right() as usize - remaining_width.min(card.width as usize),
    );
    let y = label_center.1.saturating_sub(remaining_height / 2).clamp(
        card.y as usize,
        card.bottom() as usize - remaining_height.min(card.height as usize),
    );
    let remaining = NavigationTransitionRect {
        x: x as u16,
        y: y as u16,
        width: remaining_width.min(card.width as usize) as u16,
        height: remaining_height.min(card.height as usize) as u16,
    };
    let text_group = if geometry.source_detail.width == 0 || geometry.source_detail.height == 0 {
        geometry.source_label
    } else {
        let x = geometry.source_label.x.min(geometry.source_detail.x);
        let y = geometry.source_label.y.min(geometry.source_detail.y);
        NavigationTransitionRect {
            x,
            y,
            width: geometry
                .source_label
                .right()
                .max(geometry.source_detail.right())
                .saturating_sub(x),
            height: geometry
                .source_label
                .bottom()
                .max(geometry.source_detail.bottom())
                .saturating_sub(y),
        }
    };
    copy_rect_excluding(working, source, width, height, remaining, text_group, stats);
}

fn copy_rect_excluding(
    working: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    exclusion: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(hole) = intersect_rect(rect, exclusion) else {
        copy_rect_565(working, source, width, height, rect, stats);
        return;
    };
    let bands = [
        NavigationTransitionRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: hole.y.saturating_sub(rect.y),
        },
        NavigationTransitionRect {
            x: rect.x,
            y: hole.bottom(),
            width: rect.width,
            height: rect.bottom().saturating_sub(hole.bottom()),
        },
        NavigationTransitionRect {
            x: rect.x,
            y: hole.y,
            width: hole.x.saturating_sub(rect.x),
            height: hole.height,
        },
        NavigationTransitionRect {
            x: hole.right(),
            y: hole.y,
            width: rect.right().saturating_sub(hole.right()),
            height: hole.height,
        },
    ];
    for band in bands {
        if band.width != 0 && band.height != 0 {
            copy_rect_565(working, source, width, height, band, stats);
        }
    }
}

fn draw_source_latch(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    card: NavigationTransitionRect,
    cover_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let latch = window_q16(cover_q16, 0, 13_000);
    draw_outline_565(working, width, height, card, LOGIC_MINT, stats);
    if (2_500..20_000).contains(&cover_q16) {
        for x in (card.x as usize + 8..card.right() as usize).step_by(8) {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: x as u16,
                    y: card.y,
                    width: 1,
                    height: card.height,
                },
                COPPER_SHADOW,
                stats,
            );
        }
        for y in (card.y as usize + 8..card.bottom() as usize).step_by(8) {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: card.x,
                    y: y as u16,
                    width: card.width,
                    height: 1,
                },
                COPPER_SHADOW,
                stats,
            );
        }
    }
    for (index, (x, y)) in [
        (card.x.saturating_add(4), card.y.saturating_add(4)),
        (card.right().saturating_sub(7), card.y.saturating_add(4)),
        (card.x.saturating_add(4), card.bottom().saturating_sub(7)),
        (
            card.right().saturating_sub(7),
            card.bottom().saturating_sub(7),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if latch >= (index as u16 + 1) * 9_000 {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x,
                    y,
                    width: 3,
                    height: 3,
                },
                POWER,
                stats,
            );
        }
    }
    if cover_q16 < 18_000 {
        for spark in 0..24 {
            let hash = mix32(spark ^ 0x243f_6a88);
            let side_x = if spark & 1 == 0 { card.x } else { card.right() };
            let x = side_x.saturating_add_signed(((hash >> 28) as i8).saturating_sub(8) as i16);
            let y = card
                .y
                .saturating_add((hash as u16 % card.height.max(1)).min(card.height));
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x,
                    y,
                    width: 2,
                    height: 1,
                },
                if spark % 3 == 0 { POWER } else { COPPER },
                stats,
            );
        }
    }
}

fn draw_connected_buses(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    clip: NavigationTransitionRect,
    growth_q16: u16,
    pulse_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if growth_q16 == 0 {
        return;
    }
    let source = rect_center(geometry.source_card);
    let title = rect_center(geometry.destination_title);
    let list = geometry.destination_list;
    let preview = geometry.destination_preview;
    let footer = rect_center(geometry.destination_footer);
    let selected = rect_center(geometry.destination_selected_row);
    let targets = [
        title,
        (list.x as usize + 8, list.y as usize + 18),
        (
            list.x as usize + 8,
            list.y as usize + list.height as usize / 3,
        ),
        (
            list.x as usize + 8,
            list.y as usize + list.height as usize * 2 / 3,
        ),
        selected,
        (preview.x as usize, preview.y as usize + 12),
        (preview.right() as usize, preview.y as usize + 12),
        (
            preview.x as usize,
            preview.bottom().saturating_sub(12) as usize,
        ),
        (
            preview.right() as usize,
            preview.bottom().saturating_sub(12) as usize,
        ),
        footer,
    ];
    for (route, target) in targets.into_iter().enumerate() {
        let delay = (route as u16).saturating_mul(2_300);
        let local = window_q16(growth_q16, delay, 58_000u16.saturating_add(delay / 3));
        draw_manhattan_track(
            working,
            width,
            height,
            clip,
            source,
            target,
            route % GLYPH_PACKET_COUNT,
            local,
            pulse_q16,
            stats,
        );
    }
    if growth_q16 >= 18_000 {
        let spine_x = geometry.destination_list.x as usize + 8;
        let branch_width = (geometry.destination_list.width as usize).saturating_sub(16);
        for branch in 0..10 {
            let y = geometry.destination_list.y as usize
                + geometry.destination_list.height as usize * (branch * 2 + 1) / 20;
            let delay = 14_000u16.saturating_add(branch as u16 * 1_500);
            let local = window_q16(growth_q16, delay, 61_000);
            let visible = branch_width * local as usize / PROGRESS_MAX as usize;
            if visible == 0 {
                continue;
            }
            let start = (spine_x, y);
            let end = (spine_x.saturating_add(branch_width), y);
            draw_axis_segment(
                working,
                width,
                height,
                clip,
                start,
                end,
                visible,
                3,
                COPPER_SHADOW,
                stats,
            );
            draw_axis_segment(
                working, width, height, clip, start, end, visible, 1, COPPER, stats,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_manhattan_track(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    start: (usize, usize),
    end: (usize, usize),
    route: usize,
    progress_q16: u16,
    pulse_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let points = route_polyline(start, end, route, width);
    let lengths = [
        axis_distance(points[0], points[1]),
        axis_distance(points[1], points[2]),
        axis_distance(points[2], points[3]),
    ];
    let total = lengths.iter().sum::<usize>().max(1);
    let visible = total * progress_q16 as usize / PROGRESS_MAX as usize;
    let mut consumed = 0usize;
    for segment in 0..3 {
        let segment_visible = visible.saturating_sub(consumed).min(lengths[segment]);
        if segment_visible != 0 {
            draw_axis_segment(
                working,
                width,
                height,
                clip,
                points[segment],
                points[segment + 1],
                segment_visible,
                3,
                COPPER_SHADOW,
                stats,
            );
            draw_axis_segment(
                working,
                width,
                height,
                clip,
                points[segment],
                points[segment + 1],
                segment_visible,
                1,
                COPPER,
                stats,
            );
        }
        consumed = consumed.saturating_add(lengths[segment]);
    }
    if visible >= lengths[0] {
        draw_via(working, width, height, points[1], stats);
    }
    if visible >= lengths[0].saturating_add(lengths[1]) {
        draw_via(working, width, height, points[2], stats);
    }
    if progress_q16 == PROGRESS_MAX {
        draw_via(working, width, height, end, stats);
    }
    if pulse_q16 != 0 {
        let point = point_on_polyline(points, lengths, pulse_q16);
        let vertical = point.0 == points[1].0 && point.1 != points[0].1 && point.1 != points[3].1;
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: point.0.saturating_sub(if vertical { 1 } else { 4 }) as u16,
                y: point.1.saturating_sub(if vertical { 4 } else { 1 }) as u16,
                width: if vertical { 3 } else { 9 },
                height: if vertical { 9 } else { 3 },
            },
            POWER,
            stats,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_axis_segment(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    start: (usize, usize),
    end: (usize, usize),
    visible: usize,
    thickness: usize,
    color: Rgb565Pixel,
    stats: &mut NavigationTransitionRenderStats,
) {
    let rect = if start.0 == end.0 {
        let direction_down = end.1 >= start.1;
        NavigationTransitionRect {
            x: start.0.saturating_sub(thickness / 2) as u16,
            y: if direction_down {
                start.1
            } else {
                start.1.saturating_sub(visible)
            } as u16,
            width: thickness as u16,
            height: visible.max(1) as u16,
        }
    } else {
        let direction_right = end.0 >= start.0;
        NavigationTransitionRect {
            x: if direction_right {
                start.0
            } else {
                start.0.saturating_sub(visible)
            } as u16,
            y: start.1.saturating_sub(thickness / 2) as u16,
            width: visible.max(1) as u16,
            height: thickness as u16,
        }
    };
    if let Some(rect) = intersect_rect(rect, clip) {
        fill_rect_565(working, width, height, rect, color, stats);
    }
}

fn draw_via(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    point: (usize, usize),
    stats: &mut NavigationTransitionRenderStats,
) {
    fill_rect_565(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: point.0.saturating_sub(2) as u16,
            y: point.1 as u16,
            width: 5,
            height: 1,
        },
        COPPER,
        stats,
    );
    fill_rect_565(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: point.0 as u16,
            y: point.1.saturating_sub(2) as u16,
            width: 1,
            height: 5,
        },
        POWER,
        stats,
    );
}

fn draw_chip_packets(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    chip_rows: [[u8; 7]; GLYPH_PACKET_COUNT],
    form_q16: u16,
    dispatch_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if form_q16 == 0 || dispatch_q16 == PROGRESS_MAX {
        return;
    }
    stats.glyph_packets = GLYPH_PACKET_COUNT as u64;
    let (origin_x, origin_y, cell, packet_stride) = packet_layout(width, height);
    let targets = [
        rect_center(geometry.destination_title),
        rect_center(geometry.destination_selected_row),
        (
            geometry.destination_list.x as usize + 18,
            geometry.destination_list.y as usize + geometry.destination_list.height as usize / 3,
        ),
        rect_center(geometry.destination_preview),
        rect_center(geometry.destination_footer),
        (
            geometry.destination_list.x as usize + 18,
            geometry.destination_list.y as usize
                + geometry.destination_list.height as usize * 2 / 3,
        ),
    ];
    for packet in 0..GLYPH_PACKET_COUNT {
        let local_dispatch = window_q16(
            dispatch_q16,
            (packet as u16).saturating_mul(2_400),
            48_000u16.saturating_add((packet as u16).saturating_mul(2_000)),
        );
        let start = (
            origin_x + packet * packet_stride + (5 * cell) / 2,
            origin_y + (7 * cell) / 2,
        );
        let center = manhattan_position(
            start,
            targets[packet],
            smoothstep_q16(local_dispatch),
            packet,
            width,
            height,
        );
        if local_dispatch > 0 && local_dispatch < PROGRESS_MAX {
            let tail = manhattan_position(
                start,
                targets[packet],
                local_dispatch.saturating_sub(4_500),
                packet,
                width,
                height,
            );
            draw_manhattan_track(
                working,
                width,
                height,
                frame_rect(width, height),
                tail,
                center,
                packet,
                PROGRESS_MAX,
                PROGRESS_MAX,
                stats,
            );
        }
        let moving_cell = if local_dispatch == 0 {
            cell
        } else {
            3 + cell
                .saturating_sub(3)
                .saturating_mul((PROGRESS_MAX - local_dispatch) as usize)
                / PROGRESS_MAX as usize
        };
        draw_chip(
            working,
            width,
            height,
            center,
            chip_rows[packet],
            moving_cell.max(2),
            form_q16,
            local_dispatch,
            stats,
        );
        if local_dispatch >= 54_000 {
            let socket = NavigationTransitionRect {
                x: targets[packet].0.saturating_sub(6) as u16,
                y: targets[packet].1.saturating_sub(6) as u16,
                width: 12,
                height: 12,
            };
            draw_outline_565(working, width, height, socket, POWER, stats);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_chip(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    center: (usize, usize),
    rows: [u8; 7],
    cell: usize,
    form_q16: u16,
    dispatch_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let chip_width = 5 * cell + 8;
    let chip_height = 7 * cell + 6;
    let x = center.0.saturating_sub(chip_width / 2);
    let y = center.1.saturating_sub(chip_height / 2);
    let body = NavigationTransitionRect {
        x: x as u16,
        y: y as u16,
        width: chip_width as u16,
        height: chip_height as u16,
    };
    fill_rect_565(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: x.saturating_sub(4) as u16,
            y: y.saturating_sub(4) as u16,
            width: chip_width.saturating_add(8) as u16,
            height: chip_height.saturating_add(8) as u16,
        },
        BOARD_LIFT,
        stats,
    );
    fill_rect_565(working, width, height, body, VOID, stats);
    draw_outline_565(
        working,
        width,
        height,
        body,
        if dispatch_q16 > 0 && dispatch_q16 < PROGRESS_MAX {
            POWER
        } else {
            COPPER
        },
        stats,
    );
    for pin in 0..3 {
        let py = y + 4 + pin * (chip_height.saturating_sub(8) / 2).max(1);
        for px in [x.saturating_sub(3), x + chip_width] {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: px as u16,
                    y: py as u16,
                    width: 3,
                    height: 2,
                },
                COPPER,
                stats,
            );
        }
    }
    if form_q16 < 22_000 {
        return;
    }
    for (row, bits) in rows.into_iter().enumerate() {
        for column in 0..5 {
            if bits & (1 << (4 - column)) == 0 {
                continue;
            }
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: (x + 4 + column * cell) as u16,
                    y: (y + 3 + row * cell) as u16,
                    width: cell as u16,
                    height: cell as u16,
                },
                if form_q16 > 49_000 {
                    SILKSCREEN
                } else {
                    LOGIC_MINT
                },
                stats,
            );
        }
    }
    fill_rect_565(
        working,
        width,
        height,
        NavigationTransitionRect {
            x: x.saturating_add(chip_width.saturating_sub(4)) as u16,
            y: y.saturating_add(2) as u16,
            width: 2,
            height: 2,
        },
        STATUS_MAGENTA,
        stats,
    );
}

fn assemble_destination_tiles(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    semantic_arrivals: &SemanticArrivalPlan,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    assemble_vertical_strips(
        working,
        destination,
        width,
        height,
        geometry.destination_selected_row,
        &semantic_arrivals.selected_strips,
        reveal_q16,
        10,
        stats,
    );
    assemble_list_rows(
        working,
        destination,
        width,
        height,
        geometry.destination_list,
        geometry.destination_selected_row,
        &semantic_arrivals.list_rows,
        reveal_q16,
        stats,
    );
    assemble_preview_tiles(
        working,
        destination,
        width,
        height,
        geometry.destination_preview,
        reveal_q16,
        stats,
    );
    assemble_vertical_strips(
        working,
        destination,
        width,
        height,
        geometry.destination_footer,
        &semantic_arrivals.footer_strips,
        reveal_q16,
        14,
        stats,
    );
}

#[allow(clippy::too_many_arguments)]
fn assemble_vertical_strips(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    unit_arrivals: &[u16],
    reveal_q16: u16,
    strip_width: usize,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(rect) = clip_rect_to_frame(rect, width, height) else {
        return;
    };
    let strips = (rect.width as usize).div_ceil(strip_width);
    for strip in 0..strips {
        let x = rect.x as usize + strip * strip_width;
        let arrival = unit_arrivals
            .get(strip)
            .copied()
            .or_else(|| unit_arrivals.iter().copied().max())
            .unwrap_or(57_500)
            .saturating_add(1_000);
        if reveal_q16 < arrival {
            continue;
        }
        copy_rect_565(
            working,
            destination,
            width,
            height,
            NavigationTransitionRect {
                x: x as u16,
                y: rect.y,
                width: strip_width.min((rect.right() as usize).saturating_sub(x)) as u16,
                height: rect.height,
            },
            stats,
        );
        if reveal_q16 < arrival.saturating_add(2_500) {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: x.saturating_add(strip_width)
                        .saturating_sub(2)
                        .min(rect.right().saturating_sub(1) as usize) as u16,
                    y: (rect.y as usize + rect.height as usize / 2).saturating_sub(3) as u16,
                    width: 2,
                    height: 6.min(rect.height),
                },
                POWER,
                stats,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn assemble_list_rows(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    list: NavigationTransitionRect,
    selected: NavigationTransitionRect,
    unit_arrivals: &[u16],
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(list) = clip_rect_to_frame(list, width, height) else {
        return;
    };
    let row_height = selected.height.max(28) as usize;
    let rows = (list.height as usize).div_ceil(row_height);
    let selected_center = rect_center(selected).1;
    for row in 0..rows {
        let center = list.y as usize + row * row_height + row_height / 2;
        let distance = center.abs_diff(selected_center);
        let arrival = unit_arrivals
            .get(row)
            .copied()
            .or_else(|| unit_arrivals.iter().copied().max())
            .unwrap_or(57_500)
            .saturating_add(1_000)
            .saturating_add((distance.div_ceil(row_height) as u16).saturating_mul(300));
        let local = smoothstep_q16(window_q16(
            reveal_q16,
            arrival,
            arrival.saturating_add(7_000),
        ));
        if local == 0 {
            continue;
        }
        let y = list.y as usize + row * row_height;
        let visible_width = (list.width as u32 * local as u32 / PROGRESS_MAX as u32).max(1) as u16;
        copy_rect_565(
            working,
            destination,
            width,
            height,
            NavigationTransitionRect {
                x: list.x,
                y: y as u16,
                width: visible_width.min(list.width),
                height: row_height.min((list.bottom() as usize).saturating_sub(y)) as u16,
            },
            stats,
        );
        if local < PROGRESS_MAX {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: list.x.saturating_add(visible_width).saturating_sub(2),
                    y: y as u16,
                    width: 2,
                    height: row_height.min((list.bottom() as usize).saturating_sub(y)) as u16,
                },
                LOGIC_MINT,
                stats,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn assemble_preview_tiles(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    preview: NavigationTransitionRect,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let Some(preview) = clip_rect_to_frame(preview, width, height) else {
        return;
    };
    let columns = (preview.width as usize).div_ceil(TILE_WIDTH);
    let rows = (preview.height as usize).div_ceil(TILE_HEIGHT);
    for row in 0..rows {
        for column in 0..columns {
            let x = preview.x as usize + column * TILE_WIDTH;
            let y = preview.y as usize + row * TILE_HEIGHT;
            let center = (
                x.saturating_add(TILE_WIDTH / 2)
                    .min(preview.right().saturating_sub(1) as usize),
                y.saturating_add(TILE_HEIGHT / 2)
                    .min(preview.bottom().saturating_sub(1) as usize),
            );
            let arrival = semantic_unit_arrival_q16(
                center,
                PacketClass::Preview,
                NavigationTransitionGeometry {
                    destination_preview: preview,
                    ..NavigationTransitionGeometry::default()
                },
                width,
            )
            .saturating_add(1_000);
            if reveal_q16 < arrival {
                continue;
            }
            copy_rect_565(
                working,
                destination,
                width,
                height,
                NavigationTransitionRect {
                    x: x as u16,
                    y: y as u16,
                    width: TILE_WIDTH.min((preview.right() as usize).saturating_sub(x)) as u16,
                    height: TILE_HEIGHT.min((preview.bottom() as usize).saturating_sub(y)) as u16,
                },
                stats,
            );
            if reveal_q16 < arrival.saturating_add(9_000) {
                draw_outline_565(
                    working,
                    width,
                    height,
                    NavigationTransitionRect {
                        x: x as u16,
                        y: y as u16,
                        width: TILE_WIDTH.min((preview.right() as usize).saturating_sub(x)) as u16,
                        height: TILE_HEIGHT.min((preview.bottom() as usize).saturating_sub(y))
                            as u16,
                    },
                    LOGIC_MINT,
                    stats,
                );
            }
        }
    }
}

fn semantic_unit_arrival_q16(
    center: (usize, usize),
    class: PacketClass,
    geometry: NavigationTransitionGeometry,
    width: usize,
) -> u16 {
    let tile = center.1 / TILE_HEIGHT * width.div_ceil(TILE_WIDTH) + center.0 / TILE_WIDTH;
    tile_arrival_q16(tile, center, class, geometry)
}

fn tile_arrival_q16(
    tile: usize,
    center: (usize, usize),
    class: PacketClass,
    geometry: NavigationTransitionGeometry,
) -> u16 {
    let hash = mix32(tile as u32 ^ 0x6a09_e667);
    match class {
        PacketClass::Title => 12_000u16.saturating_add((hash & 0x07ff) as u16),
        PacketClass::SelectedRow => 16_000u16.saturating_add((hash & 0x0fff) as u16),
        PacketClass::List => {
            let row_center = geometry.destination_selected_row.y as usize
                + geometry.destination_selected_row.height as usize / 2;
            let distance = center.1.abs_diff(row_center).min(240);
            20_000u16
                .saturating_add((distance * 70) as u16)
                .saturating_add((hash & 0x0fff) as u16)
        }
        PacketClass::Preview => {
            let preview_center = rect_center(geometry.destination_preview);
            let distance = center
                .0
                .abs_diff(preview_center.0)
                .saturating_add(center.1.abs_diff(preview_center.1))
                .min(400);
            31_000u16
                .saturating_add((distance * 36) as u16)
                .saturating_add((hash & 0x07ff) as u16)
        }
        PacketClass::Footer => 45_000u16.saturating_add((hash & 0x0fff) as u16),
        PacketClass::Background => 45_000u16.saturating_add((hash & 0x1fff) as u16),
    }
    .min(57_500)
}

fn draw_destination_aperture_rims(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    geometry: NavigationTransitionGeometry,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if reveal_q16 < 5_000 || reveal_q16 >= 58_000 {
        return;
    }
    let selected_growth = smoothstep_q16(window_q16(reveal_q16, 16_000, 22_000));
    if selected_growth != 0 {
        let selected = geometry.destination_selected_row;
        let contact = (
            selected.x as usize,
            selected.y as usize + selected.height as usize / 2,
        );
        draw_manhattan_track(
            working,
            width,
            height,
            frame_rect(width, height),
            rect_center(geometry.destination_title),
            contact,
            0,
            selected_growth,
            selected_growth,
            stats,
        );
        let visible_width =
            (selected.width as u32 * selected_growth as u32 / PROGRESS_MAX as u32).max(1) as u16;
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                width: visible_width.min(selected.width),
                height: 1,
                ..selected
            },
            POWER,
            stats,
        );
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                y: selected.bottom().saturating_sub(1),
                width: visible_width.min(selected.width),
                height: 1,
                ..selected
            },
            POWER,
            stats,
        );
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                width: 1,
                ..selected
            },
            POWER,
            stats,
        );
        if selected_growth == PROGRESS_MAX {
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: selected.right().saturating_sub(1),
                    width: 1,
                    ..selected
                },
                POWER,
                stats,
            );
        }
    }
    if reveal_q16 >= 30_000 {
        let preview = geometry.destination_preview;
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: preview.x.saturating_add(5),
                y: preview.bottom().saturating_add(5),
                width: preview.width,
                height: 5,
            },
            VOID,
            stats,
        );
        draw_outline_565(working, width, height, preview, LOGIC_MINT, stats);
        for (pin_index, pin) in (8..preview.width as usize).step_by(16).enumerate() {
            let energized =
                reveal_q16 >= 30_000u16.saturating_add((pin_index as u16).saturating_mul(320));
            for y in [
                preview.y.saturating_sub(4),
                preview.bottom().saturating_add(1),
            ] {
                fill_rect_565(
                    working,
                    width,
                    height,
                    NavigationTransitionRect {
                        x: preview.x.saturating_add(pin as u16),
                        y,
                        width: 2,
                        height: 4,
                    },
                    if energized { POWER } else { COPPER },
                    stats,
                );
            }
        }
    }
}

fn draw_verification_beam(
    working: &mut [Rgb565Pixel],
    destination: &[Rgb565Pixel],
    width: usize,
    height: usize,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    if reveal_q16 < 48_000 {
        return;
    }
    const HEAD_COUNT: usize = 6;
    let mut head_progress = [0u16; HEAD_COUNT];
    for (head, progress) in head_progress.iter_mut().enumerate() {
        *progress = smoothstep_q16(window_q16(
            reveal_q16,
            48_000u16.saturating_add((head as u16).saturating_mul(550)),
            60_500u16.saturating_add((head as u16).saturating_mul(300)),
        ));
    }
    let rail_start = head_progress
        .iter()
        .map(|progress| width * *progress as usize / PROGRESS_MAX as usize)
        .min()
        .unwrap_or(width);
    if rail_start < width {
        fill_rect_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: rail_start as u16,
                y: (height / 2) as u16,
                width: width.saturating_sub(rail_start) as u16,
                height: 1,
            },
            COPPER_SHADOW,
            stats,
        );
    }
    for head in 0..HEAD_COUNT {
        let verification = head_progress[head];
        let columns = width * verification as usize / PROGRESS_MAX as usize;
        let y0 = height * head / HEAD_COUNT;
        let y1 = height * (head + 1) / HEAD_COUNT;
        if columns != 0 {
            for y in y0..y1 {
                let start = y * width;
                working[start..start + columns]
                    .copy_from_slice(&destination[start..start + columns]);
            }
            stats.copied_pixels = stats
                .copied_pixels
                .saturating_add(columns.saturating_mul(y1.saturating_sub(y0)) as u64);
        }
        if columns != 0 && columns < width {
            let core_y = (y0 + y1).div_ceil(2);
            let tether_y = core_y.min(height / 2);
            let tether_height = core_y.abs_diff(height / 2).saturating_add(1);
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: columns.saturating_add(3).min(width.saturating_sub(1)) as u16,
                    y: tether_y as u16,
                    width: 1,
                    height: tether_height as u16,
                },
                COPPER_SHADOW,
                stats,
            );
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: columns.saturating_sub(2) as u16,
                    y: core_y.saturating_sub(2) as u16,
                    width: 5,
                    height: 5,
                },
                LOGIC_MINT,
                stats,
            );
            fill_rect_565(
                working,
                width,
                height,
                NavigationTransitionRect {
                    x: columns.saturating_sub(1) as u16,
                    y: core_y.saturating_sub(1) as u16,
                    width: 3,
                    height: 3,
                },
                POWER,
                stats,
            );
        }
    }
}

fn draw_foundry_rim(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    rect: NavigationTransitionRect,
    stats: &mut NavigationTransitionRenderStats,
) {
    draw_outline_565(working, width, height, rect, LOGIC_MINT, stats);
    if rect.width > 4 && rect.height > 4 {
        draw_outline_565(
            working,
            width,
            height,
            NavigationTransitionRect {
                x: rect.x.saturating_add(3),
                y: rect.y.saturating_add(3),
                width: rect.width.saturating_sub(6),
                height: rect.height.saturating_sub(6),
            },
            COPPER,
            stats,
        );
    }
}

fn draw_foundry_packets(
    renderer: &mut SpriteFoundryRenderer,
    working: &mut [Rgb565Pixel],
    cover_q16: u16,
    reveal_q16: u16,
    stats: &mut NavigationTransitionRenderStats,
) {
    let width = renderer.width;
    let height = renderer.height;
    if renderer.formation.is_empty()
        || renderer.particle_paths.len() != renderer.particle_count
        || width == 0
        || height == 0
    {
        return;
    }
    renderer.commands.clear();
    for (index, path) in renderer.particle_paths.iter().copied().enumerate() {
        let (position, visible) = if reveal_q16 == 0 {
            let lane = (index % GLYPH_PACKET_COUNT) as u16;
            let delay = 4_000u16
                .saturating_add(lane.saturating_mul(1_350))
                .saturating_add((path.hash & 0x03ff) as u16);
            let finish = 52_000u16.saturating_add(lane.saturating_mul(1_450));
            let local = smoothstep_q16(window_q16(cover_q16, delay, finish));
            (
                prepared_route_position(path.cover_route, local, width, height),
                local != 0,
            )
        } else {
            let arrival = path.arrival_q16;
            let start = arrival.saturating_sub(6_000);
            let local = smoothstep_q16(window_q16(reveal_q16, start, arrival));
            (
                prepared_route_position(path.reveal_route, local, width, height),
                local < PROGRESS_MAX,
            )
        };
        if !visible || position.0 >= width || position.1 >= height {
            continue;
        }
        renderer.commands.push(pack_visual_command(
            (position.1 * width + position.0) as u32,
            particle_palette_index(index, path.class),
            path.hash & 0x30 == 0x30 && position.0 + 1 < width,
        ));
    }
    renderer.dirty_offsets.clear();
    stats.particle_pixels = raster_packed_visual_commands_with_palette(
        working,
        &renderer.commands,
        &mut renderer.dirty_offsets,
        PARTICLE_PALETTE,
    ) as u64;
}

fn semantic_destination_point(
    index: usize,
    count: usize,
    geometry: NavigationTransitionGeometry,
    width: usize,
    height: usize,
) -> ((usize, usize), PacketClass) {
    let normalized = index.saturating_mul(100) / count.max(1);
    let (rect, class, cohort_start, cohort_end) = if normalized < 18 {
        (geometry.destination_title, PacketClass::Title, 0, 18)
    } else if normalized < 38 {
        (
            geometry.destination_selected_row,
            PacketClass::SelectedRow,
            18,
            38,
        )
    } else if normalized < 68 {
        (geometry.destination_list, PacketClass::List, 38, 68)
    } else if normalized < 94 {
        (geometry.destination_preview, PacketClass::Preview, 68, 94)
    } else {
        (geometry.destination_footer, PacketClass::Footer, 94, 100)
    };
    let cohort_begin = count.saturating_mul(cohort_start) / 100;
    let cohort_count = count
        .saturating_mul(cohort_end)
        .saturating_div(100)
        .saturating_sub(cohort_begin)
        .max(1);
    let local = index.saturating_sub(cohort_begin);
    let columns = match class {
        PacketClass::Title => 24,
        PacketClass::SelectedRow | PacketClass::List | PacketClass::Footer => 32,
        PacketClass::Preview => 16,
        PacketClass::Background => 1,
    };
    let rows = cohort_count.div_ceil(columns).max(1);
    let hash = mix32(index as u32 ^ 0x510e_527f);
    let x = rect.x as usize
        + ((local % columns) * rect.width as usize / columns.max(1))
        + (hash as usize & 3);
    let y = rect.y as usize
        + ((local / columns).min(rows - 1) * rect.height as usize / rows)
        + ((hash >> 3) as usize & 3);
    (
        (
            x.min(rect.right().saturating_sub(1) as usize)
                .min(width.saturating_sub(1)),
            y.min(rect.bottom().saturating_sub(1) as usize)
                .min(height.saturating_sub(1)),
        ),
        class,
    )
}

fn particle_palette_index(index: usize, class: PacketClass) -> usize {
    match class {
        PacketClass::Title => 3,
        PacketClass::SelectedRow => 2,
        PacketClass::Preview => 2 + (index & 1),
        PacketClass::List => 1 + usize::from(index % 5 == 0),
        PacketClass::Footer => 1,
        PacketClass::Background => index & 1,
    }
}

fn source_packet_point(
    index: usize,
    particle_count: usize,
    card: NavigationTransitionRect,
) -> (usize, usize) {
    let columns = (card.width as usize / 8).max(1);
    let rows = (card.height as usize / 8).max(1);
    let cells = columns.saturating_mul(rows).max(1);
    let cell = index.saturating_mul(cells) / particle_count.max(1);
    let hash = mix32(index as u32 ^ 0xa54f_f53a);
    (
        card.x as usize
            + (cell % columns) * 8
            + (hash as usize & 3).min(card.width.saturating_sub(1) as usize),
        card.y as usize
            + (cell / columns).min(rows - 1) * 8
            + ((hash >> 3) as usize & 3).min(card.height.saturating_sub(1) as usize),
    )
}

fn title_chip_rows(
    points: &[(usize, usize)],
    min_x: usize,
    min_y: usize,
    source_width: usize,
    source_height: usize,
) -> [[u8; 7]; GLYPH_PACKET_COUNT] {
    let mut rows = [[0u8; 7]; GLYPH_PACKET_COUNT];
    let source_width = source_width.max(1);
    let source_height = source_height.max(1);
    for &(x, y) in points {
        let relative_x = x.saturating_sub(min_x).min(source_width - 1);
        let relative_y = y.saturating_sub(min_y).min(source_height - 1);
        let packet_numerator = relative_x.saturating_mul(GLYPH_PACKET_COUNT);
        let packet = (packet_numerator / source_width).min(GLYPH_PACKET_COUNT - 1);
        let within_packet = packet_numerator % source_width;
        let column = within_packet.saturating_mul(5) / source_width;
        let row = relative_y.saturating_mul(7) / source_height;
        rows[packet][row.min(6)] |= 1 << (4usize.saturating_sub(column.min(4)));
    }
    for (packet, packet_rows) in rows.iter_mut().enumerate() {
        if packet_rows.iter().all(|row| *row == 0) {
            *packet_rows = CHIP_PACKET_ROWS[packet];
        }
    }
    rows
}

fn packet_target_mask(width: usize, height: usize) -> Option<TargetMask> {
    let packet_columns = GLYPH_PACKET_COUNT * 6 - 1;
    if width < packet_columns || height < 7 {
        return None;
    }
    let scale = (width / packet_columns).min(height / 7).min(8).max(1);
    let mask_width = packet_columns * scale;
    let mask_height = 7 * scale;
    let mut alpha = vec![0u8; mask_width.saturating_mul(mask_height)];
    for (packet, rows) in CHIP_PACKET_ROWS.iter().enumerate() {
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

fn packet_layout(width: usize, height: usize) -> (usize, usize, usize, usize) {
    let cell = 2;
    let packet_stride = 30;
    let total_width = (GLYPH_PACKET_COUNT - 1) * packet_stride + 5 * cell + 8;
    (
        width.saturating_sub(total_width) / 2,
        height.saturating_add(height / 4).saturating_sub(7 * cell) / 2,
        cell,
        packet_stride,
    )
}

fn normalize_particle_count(count: usize) -> usize {
    if count <= FALLBACK_PARTICLE_COUNT {
        FALLBACK_PARTICLE_COUNT
    } else {
        FULL_PARTICLE_COUNT
    }
}

fn transition_edge_index(edge: NavigationTransitionEdge) -> usize {
    match edge {
        NavigationTransitionEdge::HomeToConsoles => 0,
        NavigationTransitionEdge::HomeToArcade => 1,
        NavigationTransitionEdge::ConsolesToSystem => 2,
    }
}

fn manhattan_position(
    start: (usize, usize),
    target: (usize, usize),
    progress_q16: u16,
    index: usize,
    width: usize,
    height: usize,
) -> (usize, usize) {
    prepared_route_position(
        prepare_manhattan_route(start, target, index, width),
        progress_q16,
        width,
        height,
    )
}

fn prepare_manhattan_route(
    start: (usize, usize),
    target: (usize, usize),
    index: usize,
    width: usize,
) -> PreparedManhattanRoute {
    let route = index % GLYPH_PACKET_COUNT;
    let points = route_polyline(start, target, route, width);
    PreparedManhattanRoute {
        points,
        lengths: [
            axis_distance(points[0], points[1]),
            axis_distance(points[1], points[2]),
            axis_distance(points[2], points[3]),
        ],
    }
}

fn prepared_route_position(
    route: PreparedManhattanRoute,
    progress_q16: u16,
    width: usize,
    height: usize,
) -> (usize, usize) {
    let (x, y) = point_on_polyline(route.points, route.lengths, progress_q16);
    (
        x.min(width.saturating_sub(1)),
        y.min(height.saturating_sub(1)),
    )
}

fn route_polyline(
    start: (usize, usize),
    target: (usize, usize),
    route: usize,
    width: usize,
) -> [(usize, usize); 4] {
    let lane = route % GLYPH_PACKET_COUNT;
    let lane_x = width.saturating_mul(lane + 1) / (GLYPH_PACKET_COUNT + 1);
    let bend_x = ((lane_x / 16) * 16).min(width.saturating_sub(1));
    [start, (bend_x, start.1), (bend_x, target.1), target]
}

fn point_on_polyline(
    points: [(usize, usize); 4],
    lengths: [usize; 3],
    progress_q16: u16,
) -> (usize, usize) {
    let total = lengths.iter().sum::<usize>().max(1);
    let distance = total * progress_q16 as usize / PROGRESS_MAX as usize;
    let mut consumed = 0usize;
    for segment in 0..3 {
        if distance <= consumed.saturating_add(lengths[segment]) {
            let local = distance.saturating_sub(consumed);
            let progress =
                (local as u32 * PROGRESS_MAX as u32 / lengths[segment].max(1) as u32) as u16;
            return (
                lerp_usize(points[segment].0, points[segment + 1].0, progress),
                lerp_usize(points[segment].1, points[segment + 1].1, progress),
            );
        }
        consumed = consumed.saturating_add(lengths[segment]);
    }
    points[3]
}

fn axis_distance(a: (usize, usize), b: (usize, usize)) -> usize {
    a.0.abs_diff(b.0).saturating_add(a.1.abs_diff(b.1))
}

fn scale_to_segment(value: u16, end: u16) -> u16 {
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

fn rect_center(rect: NavigationTransitionRect) -> (usize, usize) {
    (
        rect.x as usize + rect.width as usize / 2,
        rect.y as usize + rect.height as usize / 2,
    )
}

#[cfg(test)]
fn point_in_rect(x: usize, y: usize, rect: NavigationTransitionRect) -> bool {
    x >= rect.x as usize
        && y >= rect.y as usize
        && x < rect.right() as usize
        && y < rect.bottom() as usize
}

fn intersect_rect(
    a: NavigationTransitionRect,
    b: NavigationTransitionRect,
) -> Option<NavigationTransitionRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
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

    fn test_geometry() -> NavigationTransitionGeometry {
        NavigationTransitionGeometry {
            source_card: NavigationTransitionRect {
                x: 2,
                y: 4,
                width: 18,
                height: 28,
            },
            source_label: NavigationTransitionRect {
                x: 5,
                y: 14,
                width: 12,
                height: 5,
            },
            destination_title: NavigationTransitionRect {
                x: 2,
                y: 2,
                width: 24,
                height: 5,
            },
            destination_detail: NavigationTransitionRect {
                x: 2,
                y: 7,
                width: 24,
                height: 3,
            },
            destination_list: NavigationTransitionRect {
                x: 2,
                y: 10,
                width: 30,
                height: 22,
            },
            destination_selected_row: NavigationTransitionRect {
                x: 2,
                y: 16,
                width: 30,
                height: 6,
            },
            destination_preview: NavigationTransitionRect {
                x: 36,
                y: 10,
                width: 24,
                height: 20,
            },
            destination_footer: NavigationTransitionRect {
                x: 2,
                y: 33,
                width: 58,
                height: 2,
            },
            ..NavigationTransitionGeometry::default()
        }
    }

    #[test]
    fn packet_mask_contains_six_large_bounded_glyphs() {
        let mask = packet_target_mask(960, 540).expect("packet mask");
        assert!(mask.width() <= 960);
        assert!(mask.height() <= 540);
        assert!(mask.width() >= 240);
        assert!(!mask.points().is_empty());
        assert_eq!(CHIP_PACKET_ROWS.len(), 6);
    }

    #[test]
    fn selected_title_is_quantized_into_six_visible_chip_packets() {
        let mut points = Vec::new();
        for packet in 0..GLYPH_PACKET_COUNT {
            for y in 0..14 {
                points.push((packet * 10 + 2 + y % 3, y));
            }
        }
        let rows = title_chip_rows(&points, 0, 0, 60, 14);
        assert!(rows.iter().all(|packet| packet.iter().any(|row| *row != 0)));
        assert_ne!(rows, CHIP_PACKET_ROWS);
    }

    #[test]
    fn both_particle_counts_sample_the_full_packet_formation() {
        for count in [FALLBACK_PARTICLE_COUNT, FULL_PARTICLE_COUNT] {
            let mut renderer = SpriteFoundryRenderer::empty(count);
            renderer.prepare(960, 540);
            let points = renderer
                .formation
                .iter()
                .filter_map(|command| unpack_visual_command(*command))
                .map(|(offset, _, _)| (offset % 960, offset / 960))
                .collect::<Vec<_>>();
            let min_y = points.iter().map(|point| point.1).min().unwrap();
            let max_y = points.iter().map(|point| point.1).max().unwrap();
            assert!(max_y.saturating_sub(min_y) >= 40);
            let min_x = points.iter().map(|point| point.0).min().unwrap();
            let max_x = points.iter().map(|point| point.0).max().unwrap();
            for packet in 0..GLYPH_PACKET_COUNT {
                let left = min_x + (max_x - min_x + 1) * packet / GLYPH_PACKET_COUNT;
                let right = min_x + (max_x - min_x + 1) * (packet + 1) / GLYPH_PACKET_COUNT;
                assert!(
                    points
                        .iter()
                        .any(|point| point.0 >= left && point.0 < right.max(left + 1))
                );
            }
        }
    }

    #[test]
    fn manhattan_trajectories_stay_inside_the_frame() {
        for index in 0..FULL_PARTICLE_COUNT {
            for progress in [0, 8_192, 21_845, 32_768, 43_690, PROGRESS_MAX] {
                let point = manhattan_position(
                    (31 + index % 180, 72 + index % 360),
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
    fn particle_sources_span_the_selected_card() {
        let card = NavigationTransitionRect {
            x: 18,
            y: 74,
            width: 219,
            height: 448,
        };
        let points = (0..FULL_PARTICLE_COUNT)
            .map(|index| source_packet_point(index, FULL_PARTICLE_COUNT, card))
            .collect::<Vec<_>>();
        let min_x = points.iter().map(|point| point.0).min().unwrap();
        let max_x = points.iter().map(|point| point.0).max().unwrap();
        let min_y = points.iter().map(|point| point.1).min().unwrap();
        let max_y = points.iter().map(|point| point.1).max().unwrap();
        assert!(max_x - min_x >= card.width as usize * 3 / 4);
        assert!(max_y - min_y >= card.height as usize * 3 / 4);
    }

    #[test]
    fn reverse_canonical_progress_runs_the_same_choreography_backwards() {
        let cover_frame = NavigationTransitionFrame {
            progress_q16: PROGRESS_MAX - super::super::COVER_PROGRESS,
            cover_progress_q16: PROGRESS_MAX,
            reveal_progress_q16: 0,
            ..NavigationTransitionFrame::default()
        };
        let reveal_frame = NavigationTransitionFrame {
            progress_q16: PROGRESS_MAX,
            cover_progress_q16: PROGRESS_MAX,
            reveal_progress_q16: PROGRESS_MAX,
            ..NavigationTransitionFrame::default()
        };
        assert_eq!(
            canonical_progress_q16(NavigationTransitionDirection::Reverse, cover_frame),
            super::super::COVER_PROGRESS
        );
        assert_eq!(
            canonical_progress_q16(NavigationTransitionDirection::Reverse, reveal_frame),
            0
        );
    }

    #[test]
    fn canonical_progress_matches_at_representative_reverse_complements() {
        for progress in [1, 8_192, 21_845, 32_768, 49_152, 65_534] {
            let forward = NavigationTransitionFrame {
                progress_q16: progress,
                ..NavigationTransitionFrame::default()
            };
            let reverse = NavigationTransitionFrame {
                progress_q16: PROGRESS_MAX - progress,
                ..NavigationTransitionFrame::default()
            };
            assert_eq!(
                canonical_progress_q16(NavigationTransitionDirection::Forward, forward),
                canonical_progress_q16(NavigationTransitionDirection::Reverse, reverse)
            );
        }
    }

    #[test]
    fn standalone_reverse_renders_forward_choreography_at_complementary_times() {
        let width = 64;
        let height = 36;
        let geometry = test_geometry();
        let mut source = vec![Rgb565Pixel(0x0841); width * height];
        let mut destination = vec![Rgb565Pixel(0x18c8); width * height];
        for y in geometry.source_label.y as usize..geometry.source_label.bottom() as usize {
            for x in geometry.source_label.x as usize..geometry.source_label.right() as usize {
                if (x + y) % 3 == 0 {
                    source[y * width + x] = Rgb565Pixel(0x07ff);
                }
            }
        }
        for y in geometry.destination_title.y as usize..geometry.destination_title.bottom() as usize
        {
            for x in
                geometry.destination_title.x as usize..geometry.destination_title.right() as usize
            {
                if (x + y) % 3 == 0 {
                    destination[y * width + x] = Rgb565Pixel(0x07ff);
                }
            }
        }

        let render_at = |direction, elapsed_us| {
            let (initial, final_frame) = match direction {
                NavigationTransitionDirection::Forward => (&source, &destination),
                NavigationTransitionDirection::Reverse => (&destination, &source),
            };
            let mut poc = NavigationTransitionPoc::new_with_style(width, height, true, 1);
            poc.begin(
                NavigationTransitionEdge::HomeToArcade,
                direction,
                geometry,
                initial,
                0,
            )
            .unwrap();
            poc.capture_destination(final_frame).unwrap();
            poc.tick(elapsed_us);
            poc.render().unwrap().to_vec()
        };
        let duration_us = NavigationTransitionStyle::SpriteFoundry
            .duration_us(NavigationTransitionEdge::HomeToArcade);
        let covered_us =
            duration_us * super::super::COVER_PROGRESS as u64 / super::super::PROGRESS_MAX as u64;
        for forward_us in [0, covered_us, duration_us / 2, duration_us] {
            assert_eq!(
                render_at(NavigationTransitionDirection::Forward, forward_us),
                render_at(
                    NavigationTransitionDirection::Reverse,
                    duration_us - forward_us
                ),
                "standalone reverse diverged at {forward_us} us"
            );
        }
    }

    #[test]
    fn production_geometry_midpoint_is_byte_exact_at_normal_and_debug_durations() {
        let width = 960;
        let height = 540;
        let geometry = super::super::hdmi_navigation_geometry(
            width,
            height,
            0,
            0,
            true,
            NavigationTransitionEdge::HomeToArcade,
            "Arcade",
        );
        let mut source = vec![Rgb565Pixel(0x0841); width * height];
        let mut destination = vec![Rgb565Pixel(0x18c8); width * height];
        for y in geometry.source_label.y as usize..geometry.source_label.bottom() as usize {
            for x in geometry.source_label.x as usize..geometry.source_label.right() as usize {
                if (x + y) % 3 == 0 {
                    source[y * width + x] = Rgb565Pixel(0x07ff);
                }
            }
        }
        for y in geometry.destination_title.y as usize..geometry.destination_title.bottom() as usize
        {
            for x in
                geometry.destination_title.x as usize..geometry.destination_title.right() as usize
            {
                if (x + y) % 3 == 0 {
                    destination[y * width + x] = Rgb565Pixel(0x07ff);
                }
            }
        }

        for duration_ms in [500, 4_000] {
            let duration_us = duration_ms * 1_000;
            let mut forward = NavigationTransitionPoc::new_with_style(width, height, true, 1);
            forward.configure_preview(
                Some(super::super::NavigationTransitionStyle::SpriteFoundry),
                Some(duration_ms),
            );
            forward
                .begin(
                    NavigationTransitionEdge::HomeToArcade,
                    NavigationTransitionDirection::Forward,
                    geometry,
                    &source,
                    0,
                )
                .unwrap();
            forward.capture_destination(&destination).unwrap();
            forward.tick(duration_us / 2);
            let forward_midpoint = forward.render().unwrap().to_vec();

            let mut reverse = NavigationTransitionPoc::new_with_style(width, height, true, 1);
            reverse.configure_preview(
                Some(super::super::NavigationTransitionStyle::SpriteFoundry),
                Some(duration_ms),
            );
            reverse
                .begin(
                    NavigationTransitionEdge::HomeToArcade,
                    NavigationTransitionDirection::Forward,
                    geometry,
                    &source,
                    0,
                )
                .unwrap();
            reverse.capture_destination(&destination).unwrap();
            reverse.tick(duration_us);
            reverse.render().unwrap();
            assert!(reverse.complete().is_some());
            reverse
                .begin(
                    NavigationTransitionEdge::HomeToArcade,
                    NavigationTransitionDirection::Reverse,
                    geometry,
                    &destination,
                    0,
                )
                .unwrap();
            reverse.capture_destination(&source).unwrap();
            reverse.tick(duration_us / 2);
            assert_eq!(
                forward_midpoint,
                reverse.render().unwrap(),
                "duration_ms={duration_ms}"
            );
        }
    }

    #[test]
    fn reverse_covered_frame_is_stable_across_destination_capture() {
        let width = 64;
        let height = 36;
        let source = vec![Rgb565Pixel(0x1122); width * height];
        let destination = vec![Rgb565Pixel(0x3344); width * height];
        let geometry = test_geometry();
        let mut renderer = SpriteFoundryRenderer::empty(FALLBACK_PARTICLE_COUNT);
        renderer.prepare(width, height);
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        let request = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::SpriteFoundry,
            super::super::NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Reverse,
            geometry,
        );
        let frame = NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Covered,
            progress_q16: super::super::COVER_PROGRESS,
            cover_progress_q16: PROGRESS_MAX,
            reveal_progress_q16: 0,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        };
        render_sprite_foundry(&mut renderer, &mut buffers, request, frame).unwrap();
        let before = buffers.working.clone();
        buffers.capture_destination(&destination).unwrap();
        render_sprite_foundry(&mut renderer, &mut buffers, request, frame).unwrap();
        assert_eq!(buffers.working, before);
    }

    #[test]
    fn semantic_particle_cohorts_stay_inside_their_destination_regions() {
        let geometry = test_geometry();
        for count in [FALLBACK_PARTICLE_COUNT, FULL_PARTICLE_COUNT] {
            let mut seen = [false; 5];
            for index in 0..count {
                let (point, class) = semantic_destination_point(index, count, geometry, 64, 36);
                let (rect, cohort) = match class {
                    PacketClass::Title => (geometry.destination_title, 0),
                    PacketClass::SelectedRow => (geometry.destination_selected_row, 1),
                    PacketClass::List => (geometry.destination_list, 2),
                    PacketClass::Preview => (geometry.destination_preview, 3),
                    PacketClass::Footer => (geometry.destination_footer, 4),
                    PacketClass::Background => unreachable!(),
                };
                assert!(point_in_rect(point.0, point.1, rect));
                seen[cohort] = true;
            }
            assert!(seen.into_iter().all(|value| value));
        }
    }

    #[test]
    fn semantic_unit_writes_are_scheduled_after_their_packet_arrivals() {
        let geometry = test_geometry();
        for count in [FALLBACK_PARTICLE_COUNT, FULL_PARTICLE_COUNT] {
            let mut renderer = SpriteFoundryRenderer::empty(count);
            renderer.prepare(64, 36);
            renderer.prepare_particle_paths(geometry);
            for path in renderer.particle_paths.iter().copied() {
                let destination = path.reveal_route.points[3];
                let planned = match path.class {
                    PacketClass::SelectedRow => renderer.semantic_arrivals.selected_strips.get(
                        destination
                            .0
                            .saturating_sub(geometry.destination_selected_row.x as usize)
                            / 10,
                    ),
                    PacketClass::List => renderer.semantic_arrivals.list_rows.get(
                        destination
                            .1
                            .saturating_sub(geometry.destination_list.y as usize)
                            / geometry.destination_selected_row.height.max(28) as usize,
                    ),
                    PacketClass::Footer => renderer.semantic_arrivals.footer_strips.get(
                        destination
                            .0
                            .saturating_sub(geometry.destination_footer.x as usize)
                            / 14,
                    ),
                    PacketClass::Title | PacketClass::Preview | PacketClass::Background => None,
                };
                if let Some(arrival) = planned {
                    assert!(arrival.saturating_add(1_000) >= path.arrival_q16);
                }
            }
        }
    }

    #[test]
    fn invalid_title_capture_replaces_instead_of_reusing_previous_title() {
        let width = 64;
        let height = 36;
        let geometry = test_geometry();
        let mut valid = vec![Rgb565Pixel(0x18c5); width * height];
        for y in geometry.source_label.y as usize..geometry.source_label.bottom() as usize {
            for x in (geometry.source_label.x as usize..geometry.source_label.right() as usize)
                .step_by(2)
            {
                valid[y * width + x] = LOGIC_MINT;
            }
        }
        let invalid = vec![Rgb565Pixel(0x18c5); width * height];
        let mut renderer = SpriteFoundryRenderer::empty(FALLBACK_PARTICLE_COUNT);
        renderer.prepare_transition(
            width,
            height,
            &valid,
            geometry,
            NavigationTransitionDirection::Forward,
            NavigationTransitionEdge::HomeToArcade,
        );
        assert!(!renderer.compiled_title.is_empty());
        assert_ne!(renderer.chip_rows, CHIP_PACKET_ROWS);
        renderer.prepare_transition(
            width,
            height,
            &invalid,
            geometry,
            NavigationTransitionDirection::Forward,
            NavigationTransitionEdge::HomeToArcade,
        );
        assert!(renderer.compiled_title.is_empty());
        assert_eq!(renderer.chip_rows, CHIP_PACKET_ROWS);
        assert!(!renderer.formation.is_empty());
    }

    #[test]
    fn reverse_reuses_the_forward_title_formation_for_the_same_edge() {
        let width = 64;
        let height = 36;
        let geometry = test_geometry();
        let mut source = vec![Rgb565Pixel(0x18c5); width * height];
        for y in geometry.source_label.y as usize..geometry.source_label.bottom() as usize {
            for x in (geometry.source_label.x as usize..geometry.source_label.right() as usize)
                .step_by(2)
            {
                source[y * width + x] = LOGIC_MINT;
            }
        }
        let mut renderer = SpriteFoundryRenderer::empty(FALLBACK_PARTICLE_COUNT);
        renderer.prepare_transition(
            width,
            height,
            &source,
            geometry,
            NavigationTransitionDirection::Forward,
            NavigationTransitionEdge::HomeToArcade,
        );
        let forward_formation = renderer.formation.clone();
        let forward_title = renderer.compiled_title.clone();
        let forward_chip_rows = renderer.chip_rows;
        let unrelated_reverse_source = vec![Rgb565Pixel(0xffff); width * height];
        renderer.prepare_transition(
            width,
            height,
            &unrelated_reverse_source,
            geometry,
            NavigationTransitionDirection::Reverse,
            NavigationTransitionEdge::HomeToArcade,
        );
        assert_eq!(renderer.formation, forward_formation);
        assert_eq!(renderer.compiled_title, forward_title);
        assert_eq!(renderer.chip_rows, forward_chip_rows);
    }

    #[test]
    fn compiled_title_targets_only_exact_destination_foreground_pixels() {
        let width = 64;
        let height = 36;
        let geometry = test_geometry();
        let mut source = vec![Rgb565Pixel(0x18c5); width * height];
        for y in geometry.source_label.y as usize..geometry.source_label.bottom() as usize {
            for x in (geometry.source_label.x as usize..geometry.source_label.right() as usize)
                .step_by(2)
            {
                source[y * width + x] = LOGIC_MINT;
            }
        }
        let mut destination = vec![Rgb565Pixel(0x0020); width * height];
        for y in geometry.destination_title.y as usize..geometry.destination_title.bottom() as usize
        {
            for x in
                geometry.destination_title.x as usize..geometry.destination_title.right() as usize
            {
                if (x + y) % 2 == 0 {
                    destination[y * width + x] = SILKSCREEN;
                }
            }
        }
        let mut renderer = SpriteFoundryRenderer::empty(FALLBACK_PARTICLE_COUNT);
        renderer.prepare_transition(
            width,
            height,
            &source,
            geometry,
            NavigationTransitionDirection::Forward,
            NavigationTransitionEdge::HomeToArcade,
        );
        renderer.prepare_destination_title(
            width,
            height,
            &destination,
            geometry.destination_title,
            NavigationTransitionEdge::HomeToArcade,
        );
        assert_eq!(
            renderer.destination_targets.len(),
            renderer.compiled_title.len()
        );
        assert!(
            renderer
                .destination_targets
                .iter()
                .all(|target| renderer.destination_title.contains(target))
        );
    }

    #[test]
    fn reverse_rejects_a_cached_title_from_a_different_route() {
        let width = 64;
        let height = 36;
        let mut first_geometry = test_geometry();
        first_geometry.label_signature = 0x1111;
        let mut first = vec![Rgb565Pixel(0x18c5); width * height];
        for y in
            first_geometry.source_label.y as usize..first_geometry.source_label.bottom() as usize
        {
            for x in (first_geometry.source_label.x as usize
                ..first_geometry.source_label.right() as usize)
                .step_by(2)
            {
                first[y * width + x] = LOGIC_MINT;
            }
        }
        let mut renderer = SpriteFoundryRenderer::empty(FALLBACK_PARTICLE_COUNT);
        renderer.prepare_transition(
            width,
            height,
            &first,
            first_geometry,
            NavigationTransitionDirection::Forward,
            NavigationTransitionEdge::ConsolesToSystem,
        );
        let first_title = renderer.compiled_title.clone();

        let mut second_geometry = first_geometry;
        second_geometry.label_signature = 0x2222;
        let mut second = vec![Rgb565Pixel(0x18c5); width * height];
        for y in second_geometry.destination_title.y as usize
            ..second_geometry.destination_title.bottom() as usize
        {
            for x in second_geometry.destination_title.x as usize
                ..second_geometry.destination_title.right() as usize
            {
                if (x + y) % 3 == 0 {
                    second[y * width + x] = SILKSCREEN;
                }
            }
        }
        renderer.prepare_transition(
            width,
            height,
            &second,
            second_geometry,
            NavigationTransitionDirection::Reverse,
            NavigationTransitionEdge::ConsolesToSystem,
        );
        assert_eq!(renderer.title_signature, second_geometry.label_signature);
        assert_ne!(renderer.compiled_title, first_title);
        assert_eq!(
            renderer.destination_targets.len(),
            renderer.compiled_title.len()
        );
        assert!(!renderer.destination_targets.is_empty());
        assert!(
            renderer
                .destination_targets
                .iter()
                .all(|target| renderer.destination_title.contains(target))
        );
    }

    #[test]
    fn cached_title_is_rebuilt_when_framebuffer_dimensions_change() {
        let mut geometry = test_geometry();
        geometry.label_signature = 0x4444;
        let source = vec![Rgb565Pixel(0x18c5); 64 * 36];
        let mut renderer = SpriteFoundryRenderer::empty(FALLBACK_PARTICLE_COUNT);
        renderer.prepare_transition(
            64,
            36,
            &source,
            geometry,
            NavigationTransitionDirection::Forward,
            NavigationTransitionEdge::HomeToArcade,
        );
        renderer.prepare_transition(
            32,
            18,
            &vec![Rgb565Pixel(0x39e7); 32 * 18],
            geometry,
            NavigationTransitionDirection::Reverse,
            NavigationTransitionEdge::HomeToArcade,
        );
        assert_eq!((renderer.width, renderer.height), (32, 18));
        assert!(renderer.formation.iter().all(|command| {
            unpack_visual_command(*command).is_some_and(|(offset, _, _)| offset < (32 * 18) as u32)
        }));
    }

    #[test]
    fn terminal_cover_window_is_the_exact_source_without_foundry_effects() {
        let width = 64;
        let height = 36;
        let source = (0..width * height)
            .map(|index| Rgb565Pixel(index as u16))
            .collect::<Vec<_>>();
        let mut buffers = NavigationTransitionBuffers::new(width, height);
        buffers.capture_source(&source).unwrap();
        let mut renderer = SpriteFoundryRenderer::empty(FALLBACK_PARTICLE_COUNT);
        let request = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::SpriteFoundry,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            test_geometry(),
        );
        let frame = NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Expand,
            progress_q16: 6_000,
            cover_progress_q16: 6_000,
            reveal_progress_q16: 0,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        };
        render_sprite_foundry(&mut renderer, &mut buffers, request, frame).unwrap();
        assert_eq!(buffers.working, source);
    }

    #[test]
    fn saturated_semantic_geometry_is_clipped_without_panicking() {
        let width = 64;
        let height = 36;
        let mut geometry = test_geometry();
        geometry.destination_list = NavigationTransitionRect {
            x: 60,
            y: 30,
            width: u16::MAX,
            height: u16::MAX,
        };
        geometry.destination_selected_row = geometry.destination_list;
        geometry.destination_preview = geometry.destination_list;
        geometry.destination_footer = geometry.destination_list;
        let source = vec![Rgb565Pixel(0x18c5); width * height];
        let destination = vec![Rgb565Pixel(0x3344); width * height];
        let mut renderer = SpriteFoundryRenderer::empty(FALLBACK_PARTICLE_COUNT);
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
        buffers.capture_destination(&destination).unwrap();
        let request = NavigationTransitionRequest::new(
            super::super::NavigationTransitionStyle::SpriteFoundry,
            NavigationTransitionEdge::HomeToArcade,
            NavigationTransitionDirection::Forward,
            geometry,
        );
        let frame = NavigationTransitionFrame {
            phase: NavigationTransitionPhase::Reveal,
            progress_q16: 50_000,
            cover_progress_q16: PROGRESS_MAX,
            reveal_progress_q16: 30_000,
            owns_full_frame: true,
            ..NavigationTransitionFrame::default()
        };
        render_sprite_foundry(&mut renderer, &mut buffers, request, frame).unwrap();
        assert_eq!(buffers.working.len(), width * height);
    }

    #[test]
    fn fallback_keeps_the_same_route_topology() {
        assert_eq!(normalize_particle_count(1), FALLBACK_PARTICLE_COUNT);
        assert_eq!(
            normalize_particle_count(FULL_PARTICLE_COUNT),
            FULL_PARTICLE_COUNT
        );
        for count in [FALLBACK_PARTICLE_COUNT, FULL_PARTICLE_COUNT] {
            let first = source_packet_point(
                0,
                count,
                NavigationTransitionRect {
                    x: 10,
                    y: 20,
                    width: 220,
                    height: 440,
                },
            );
            let last = source_packet_point(
                count - 1,
                count,
                NavigationTransitionRect {
                    x: 10,
                    y: 20,
                    width: 220,
                    height: 440,
                },
            );
            assert_ne!(first, last);
        }
    }
}
