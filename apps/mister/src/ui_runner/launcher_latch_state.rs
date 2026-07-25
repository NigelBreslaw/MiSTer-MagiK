// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::{ArcadeListUpdate, arcade_update_dirty_rect};
use mister_magik_fb::framebuffer::target::{DirtyRect, DirtyRectList, subtract_dirty_rects};

pub(super) const DAMAGE_TILE_SIZE: usize = 32;
const DAMAGE_MAX_WIDTH: usize = 1280;
const DAMAGE_MAX_HEIGHT: usize = 720;
const DAMAGE_MAX_COLUMNS: usize = DAMAGE_MAX_WIDTH.div_ceil(DAMAGE_TILE_SIZE);
const DAMAGE_MAX_ROWS: usize = DAMAGE_MAX_HEIGHT.div_ceil(DAMAGE_TILE_SIZE);
const DAMAGE_MAX_TILES: usize = DAMAGE_MAX_COLUMNS * DAMAGE_MAX_ROWS;
const DAMAGE_WORDS: usize = DAMAGE_MAX_TILES.div_ceil(u64::BITS as usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DamageTileMap {
    bits: [u64; DAMAGE_WORDS],
    width: u16,
    height: u16,
    columns: u8,
    rows: u8,
    full_fallback: bool,
}

impl Default for DamageTileMap {
    fn default() -> Self {
        Self {
            bits: [0; DAMAGE_WORDS],
            width: 0,
            height: 0,
            columns: 0,
            rows: 0,
            full_fallback: false,
        }
    }
}

impl DamageTileMap {
    pub(super) fn empty(width: usize, height: usize) -> Self {
        let columns = width.div_ceil(DAMAGE_TILE_SIZE);
        let rows = height.div_ceil(DAMAGE_TILE_SIZE);
        if width == 0
            || height == 0
            || width > DAMAGE_MAX_WIDTH
            || height > DAMAGE_MAX_HEIGHT
            || columns > DAMAGE_MAX_COLUMNS
            || rows > DAMAGE_MAX_ROWS
        {
            return Self {
                width: u16::try_from(width).unwrap_or(u16::MAX),
                height: u16::try_from(height).unwrap_or(u16::MAX),
                full_fallback: true,
                ..Self::default()
            };
        }
        Self {
            width: width as u16,
            height: height as u16,
            columns: columns as u8,
            rows: rows as u8,
            ..Self::default()
        }
    }

    pub(super) fn full(width: usize, height: usize) -> Self {
        let mut map = Self::empty(width, height);
        map.mark_rect(DirtyRect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        });
        map
    }

    pub(super) fn clear(&mut self) {
        self.bits.fill(0);
        self.full_fallback = false;
    }

    pub(super) fn is_empty(self) -> bool {
        !self.full_fallback && self.bits.iter().all(|word| *word == 0)
    }

    pub(super) fn is_full_fallback(self) -> bool {
        self.full_fallback
    }

    pub(super) fn mark_pixel(&mut self, x: usize, y: usize) {
        self.mark_rect(DirtyRect {
            x0: x,
            y0: y,
            x1: x.saturating_add(1),
            y1: y.saturating_add(1),
        });
    }

    pub(super) fn mark_rect(&mut self, rect: DirtyRect) {
        if self.full_fallback {
            return;
        }
        let width = self.width as usize;
        let height = self.height as usize;
        let x0 = rect.x0.min(width);
        let y0 = rect.y0.min(height);
        let x1 = rect.x1.min(width);
        let y1 = rect.y1.min(height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let first_column = x0 / DAMAGE_TILE_SIZE;
        let last_column = (x1 - 1) / DAMAGE_TILE_SIZE;
        let first_row = y0 / DAMAGE_TILE_SIZE;
        let last_row = (y1 - 1) / DAMAGE_TILE_SIZE;
        for row in first_row..=last_row {
            for column in first_column..=last_column {
                self.set_tile(column, row);
            }
        }
    }

    pub(super) fn union_with(&mut self, other: Self) {
        if self.geometry() != other.geometry() || other.full_fallback {
            self.full_fallback = true;
            return;
        }
        for (target, source) in self.bits.iter_mut().zip(other.bits) {
            *target |= source;
        }
    }

    pub(super) fn intersects(self, rect: DirtyRect) -> bool {
        if self.full_fallback {
            return true;
        }
        let width = self.width as usize;
        let height = self.height as usize;
        let x0 = rect.x0.min(width);
        let y0 = rect.y0.min(height);
        let x1 = rect.x1.min(width);
        let y1 = rect.y1.min(height);
        if x0 >= x1 || y0 >= y1 {
            return false;
        }
        for row in y0 / DAMAGE_TILE_SIZE..=(y1 - 1) / DAMAGE_TILE_SIZE {
            for column in x0 / DAMAGE_TILE_SIZE..=(x1 - 1) / DAMAGE_TILE_SIZE {
                if self.tile_is_set(column, row) {
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn tile_count(self) -> usize {
        if self.full_fallback {
            return (self.columns as usize).saturating_mul(self.rows as usize);
        }
        self.bits
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    pub(super) fn total_rgb565_bytes(self) -> usize {
        let mut bytes = 0usize;
        self.for_each_span(|rect| {
            bytes = bytes.saturating_add(
                rect.width()
                    .saturating_mul(rect.rows() as usize)
                    .saturating_mul(2),
            );
        });
        bytes
    }

    pub(super) fn for_each_span(self, mut visit: impl FnMut(DirtyRect)) {
        let width = self.width as usize;
        let height = self.height as usize;
        if self.full_fallback {
            if width > 0 && height > 0 {
                visit(DirtyRect {
                    x0: 0,
                    y0: 0,
                    x1: width,
                    y1: height,
                });
            }
            return;
        }
        for row in 0..self.rows as usize {
            let mut column = 0usize;
            while column < self.columns as usize {
                if !self.tile_is_set(column, row) {
                    column += 1;
                    continue;
                }
                let first = column;
                while column < self.columns as usize && self.tile_is_set(column, row) {
                    column += 1;
                }
                visit(DirtyRect {
                    x0: first * DAMAGE_TILE_SIZE,
                    y0: row * DAMAGE_TILE_SIZE,
                    x1: (column * DAMAGE_TILE_SIZE).min(width),
                    y1: ((row + 1) * DAMAGE_TILE_SIZE).min(height),
                });
            }
        }
    }

    fn geometry(self) -> (u16, u16, u8, u8) {
        (self.width, self.height, self.columns, self.rows)
    }

    fn set_tile(&mut self, column: usize, row: usize) {
        let index = row * DAMAGE_MAX_COLUMNS + column;
        self.bits[index / u64::BITS as usize] |= 1 << (index % u64::BITS as usize);
    }

    fn tile_is_set(self, column: usize, row: usize) -> bool {
        let index = row * DAMAGE_MAX_COLUMNS + column;
        self.bits[index / u64::BITS as usize] & (1 << (index % u64::BITS as usize)) != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LatchSlotHardwareState {
    Unknown,
    Writable,
    Pending(u16),
    Active(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DirectLayerState {
    pub(super) rect: DirtyRect,
    pub(super) version: u64,
    pub(super) content_offset_y: i64,
}

impl DirectLayerState {
    pub(super) fn new(rect: DirtyRect, version: u64) -> Self {
        Self {
            rect,
            version,
            content_offset_y: 0,
        }
    }

    pub(super) fn with_content_offset_y(mut self, content_offset_y: i64) -> Self {
        self.content_offset_y = content_offset_y;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LatchSlotCoherency {
    base_invalid: DirtyRectList,
    preview_present: Option<DirectLayerState>,
    arcade_present: Option<DirectLayerState>,
    hardware: LatchSlotHardwareState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LauncherFramePlan {
    cached_damage: DirtyRectList,
    preview_desired: Option<DirectLayerState>,
    preview_dirty: Option<DirtyRect>,
    arcade_desired: Option<DirectLayerState>,
    arcade_dirty: Option<ArcadeListUpdate>,
}

impl LauncherFramePlan {
    pub(super) fn new(
        cached_damage: DirtyRectList,
        preview_desired: Option<DirectLayerState>,
        preview_dirty: Option<DirtyRect>,
        arcade_desired: Option<DirectLayerState>,
        arcade_dirty: Option<ArcadeListUpdate>,
    ) -> Self {
        Self {
            cached_damage,
            preview_desired,
            preview_dirty,
            arcade_desired,
            arcade_dirty,
        }
    }

    pub(super) fn cached_damage(self) -> DirtyRectList {
        self.cached_damage
    }

    pub(super) fn preview_dirty(self) -> Option<DirtyRect> {
        self.preview_dirty
    }

    pub(super) fn arcade_dirty(self) -> Option<ArcadeListUpdate> {
        self.arcade_dirty
    }

    pub(super) fn for_fb0_recovery(self, full_rect: DirtyRect) -> Self {
        Self {
            cached_damage: DirtyRectList::from_one(full_rect),
            preview_dirty: self.preview_desired.map(|layer| layer.rect),
            arcade_dirty: self
                .arcade_desired
                .map(|layer| ArcadeListUpdate::Full(layer.rect)),
            ..self
        }
    }

    #[cfg(test)]
    fn from_rects(
        cached_damage: Option<DirtyRect>,
        preview_desired: Option<DirectLayerState>,
        preview_dirty: Option<DirtyRect>,
        arcade_desired: Option<DirectLayerState>,
        arcade_dirty: Option<ArcadeListUpdate>,
    ) -> Self {
        let mut cached = DirtyRectList::new();
        cached.push_if_some(cached_damage);
        Self::new(
            cached,
            preview_desired,
            preview_dirty,
            arcade_desired,
            arcade_dirty,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LatchPresentPlan {
    pub(super) slot_index: u8,
    pub(super) restore_rects: DirtyRectList,
    pub(super) preview_redraw: Option<DirtyRect>,
    pub(super) arcade_redraw: Option<ArcadeListUpdate>,
    cached_damage: DirtyRectList,
    preview_after: Option<DirectLayerState>,
    arcade_after: Option<DirectLayerState>,
}

#[derive(Clone, Debug)]
pub(super) struct TwoBufferLatchState {
    slots: [LatchSlotCoherency; 2],
    next_slot_index: u8,
    full_rect: DirtyRect,
}

impl TwoBufferLatchState {
    pub(super) fn new(width: usize, height: usize) -> Self {
        let full_rect = DirtyRect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        };
        let full_invalid = DirtyRectList::from_one(full_rect);
        Self {
            slots: [
                LatchSlotCoherency {
                    base_invalid: full_invalid,
                    preview_present: None,
                    arcade_present: None,
                    hardware: LatchSlotHardwareState::Unknown,
                },
                LatchSlotCoherency {
                    base_invalid: full_invalid,
                    preview_present: None,
                    arcade_present: None,
                    hardware: LatchSlotHardwareState::Unknown,
                },
            ],
            next_slot_index: 1,
            full_rect,
        }
    }

    pub(super) fn invalidate_all(&mut self) {
        for slot in &mut self.slots {
            slot.base_invalid = DirtyRectList::from_one(self.full_rect);
            slot.preview_present = None;
            slot.arcade_present = None;
            slot.hardware = LatchSlotHardwareState::Unknown;
        }
        self.next_slot_index = 1;
    }

    pub(super) fn sync_hardware(
        &mut self,
        active_slot_index: Option<u8>,
        active_sequence: u16,
        pending: bool,
        pending_sequence: u16,
    ) {
        if pending {
            for slot in &mut self.slots {
                slot.hardware = LatchSlotHardwareState::Pending(pending_sequence);
            }
            return;
        }

        for slot in &mut self.slots {
            slot.hardware = LatchSlotHardwareState::Writable;
        }
        if let Some(slot_index) = active_slot_index {
            self.slot_mut(slot_index).hardware = LatchSlotHardwareState::Active(active_sequence);
        }
    }

    pub(super) fn plan_next(&self, input: LauncherFramePlan) -> Option<LatchPresentPlan> {
        let slot_index = self.select_writable_slot()?;
        Some(self.plan_for_slot(slot_index, input))
    }

    pub(super) fn mark_post_success(&mut self, plan: LatchPresentPlan) {
        let slot_index = plan.slot_index;
        let other_index = other_slot(slot_index);

        let selected = self.slot_mut(slot_index);
        selected.base_invalid.clear();
        selected.preview_present = plan.preview_after;
        selected.arcade_present = plan.arcade_after;
        selected.hardware = LatchSlotHardwareState::Unknown;

        self.slot_mut(other_index)
            .base_invalid
            .extend_from(&plan.cached_damage);
        self.next_slot_index = other_index;
    }

    pub(super) fn mark_attempt_failed(&mut self, slot_index: u8) {
        let full_rect = self.full_rect;
        let slot = self.slot_mut(slot_index);
        slot.base_invalid = DirtyRectList::from_one(full_rect);
        slot.preview_present = None;
        slot.arcade_present = None;
        slot.hardware = LatchSlotHardwareState::Unknown;
    }

    pub(super) fn restore_bytes_for_slot(&self, slot_index: u8) -> usize {
        let slot = self.slot(slot_index);
        let mut bytes = slot.base_invalid.total_rgb565_bytes();
        if let Some(preview) = slot.preview_present {
            bytes = bytes.saturating_add(rect_bytes(preview.rect));
        }
        if let Some(arcade) = slot.arcade_present {
            bytes = bytes.saturating_add(rect_bytes(arcade.rect));
        }
        bytes
    }

    fn select_writable_slot(&self) -> Option<u8> {
        if self.slot(self.next_slot_index).hardware == LatchSlotHardwareState::Writable {
            return Some(self.next_slot_index);
        }
        let other = other_slot(self.next_slot_index);
        if self.slot(other).hardware == LatchSlotHardwareState::Writable {
            Some(other)
        } else {
            None
        }
    }

    fn plan_for_slot(&self, slot_index: u8, input: LauncherFramePlan) -> LatchPresentPlan {
        let slot = self.slot(slot_index);
        let mut restore_rects = DirtyRectList::new();
        extend_without_covered_rects(&mut restore_rects, &slot.base_invalid);
        extend_without_covered_rects(&mut restore_rects, &input.cached_damage);

        let restore_preview =
            direct_layer_needs_restore(slot.preview_present, input.preview_desired);
        let restore_arcade = direct_layer_needs_restore(slot.arcade_present, input.arcade_desired);
        if restore_preview {
            if let Some(preview) = slot.preview_present {
                push_without_covered_rect(&mut restore_rects, preview.rect);
            }
        }
        if restore_arcade {
            if let Some(arcade) = slot.arcade_present {
                push_without_covered_rect(&mut restore_rects, arcade.rect);
            }
        }

        let preview_intersects_restore =
            layer_intersects_restore(input.preview_desired, &restore_rects);
        let arcade_intersects_restore =
            layer_intersects_restore(input.arcade_desired, &restore_rects);

        let preview_redraw = direct_layer_redraw_rect(
            slot.preview_present,
            input.preview_desired,
            input.preview_dirty,
            preview_intersects_restore,
        );
        let arcade_redraw = direct_layer_redraw_update(
            slot.arcade_present,
            input.arcade_desired,
            input.arcade_dirty,
            arcade_intersects_restore,
        );
        let mut direct_redraws = DirtyRectList::new();
        direct_redraws.push_if_some(preview_redraw);
        direct_redraws.push_if_some(arcade_redraw.as_ref().map(arcade_update_dirty_rect));
        let restore_rects = subtract_dirty_rects(restore_rects, &direct_redraws);

        LatchPresentPlan {
            slot_index,
            restore_rects,
            preview_redraw,
            arcade_redraw,
            cached_damage: input.cached_damage,
            preview_after: input.preview_desired,
            arcade_after: input.arcade_desired,
        }
    }

    fn slot(&self, slot_index: u8) -> &LatchSlotCoherency {
        &self.slots[slot_offset(slot_index)]
    }

    fn slot_mut(&mut self, slot_index: u8) -> &mut LatchSlotCoherency {
        &mut self.slots[slot_offset(slot_index)]
    }
}

fn direct_layer_needs_restore(
    current: Option<DirectLayerState>,
    desired: Option<DirectLayerState>,
) -> bool {
    let Some(current) = current else {
        return false;
    };
    !matches!(desired, Some(desired) if desired == current)
}

fn direct_layer_redraw_rect(
    current: Option<DirectLayerState>,
    desired: Option<DirectLayerState>,
    dirty: Option<DirtyRect>,
    intersects_restore: bool,
) -> Option<DirtyRect> {
    let desired = desired?;
    if current != Some(desired) || intersects_restore {
        Some(desired.rect)
    } else {
        dirty
    }
}

fn direct_layer_redraw_update(
    current: Option<DirectLayerState>,
    desired: Option<DirectLayerState>,
    dirty: Option<ArcadeListUpdate>,
    intersects_restore: bool,
) -> Option<ArcadeListUpdate> {
    let desired = desired?;
    if let Some(current) = current {
        if current.rect != desired.rect || current.version != desired.version {
            Some(ArcadeListUpdate::Full(desired.rect))
        } else if current.content_offset_y != desired.content_offset_y {
            Some(ArcadeListUpdate::Scroll {
                delta_y: desired
                    .content_offset_y
                    .saturating_sub(current.content_offset_y)
                    .clamp(isize::MIN as i64, isize::MAX as i64) as isize,
                rect: desired.rect,
            })
        } else if intersects_restore {
            Some(ArcadeListUpdate::Full(desired.rect))
        } else {
            dirty
        }
    } else {
        Some(ArcadeListUpdate::Full(desired.rect))
    }
}

fn layer_intersects_restore(
    layer: Option<DirectLayerState>,
    restore_rects: &DirtyRectList,
) -> bool {
    layer.is_some_and(|layer| {
        restore_rects
            .iter()
            .any(|restore| restore.intersection(layer.rect).is_some())
    })
}

fn slot_offset(slot_index: u8) -> usize {
    match slot_index {
        1 => 0,
        2 => 1,
        _ => panic!("hidden latch slot index must be 1 or 2, got {slot_index}"),
    }
}

fn other_slot(slot_index: u8) -> u8 {
    match slot_index {
        1 => 2,
        2 => 1,
        _ => panic!("hidden latch slot index must be 1 or 2, got {slot_index}"),
    }
}

fn extend_without_covered_rects(target: &mut DirtyRectList, source: &DirtyRectList) {
    for rect in source.iter() {
        push_without_covered_rect(target, rect);
    }
}

fn push_without_covered_rect(target: &mut DirtyRectList, rect: DirtyRect) {
    if !target.iter().any(|existing| existing.contains(rect)) {
        target.push(rect);
    }
}

fn rect_bytes(rect: DirtyRect) -> usize {
    rect.width()
        .saturating_mul(rect.rows() as usize)
        .saturating_mul(mister_magik_fb::framebuffer::format::RGB565_BYTES_PER_PIXEL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::software_renderer::Rgb565Pixel;

    const WIDTH: usize = 4;
    const HEIGHT: usize = 3;
    const BASE: Rgb565Pixel = Rgb565Pixel(0x0000);
    const PREVIEW: Rgb565Pixel = Rgb565Pixel(0xf800);
    const ARCADE: Rgb565Pixel = Rgb565Pixel(0x07e0);

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    #[test]
    fn damage_tiles_clip_edges_and_merge_horizontal_spans() {
        let mut damage = DamageTileMap::empty(65, 33);
        damage.mark_rect(rect(31, 0, 65, 2));
        damage.mark_pixel(64, 32);
        let mut spans = Vec::new();
        damage.for_each_span(|span| spans.push(span));

        assert_eq!(damage.tile_count(), 4);
        assert_eq!(spans, vec![rect(0, 0, 65, 32), rect(64, 32, 65, 33)]);
        assert!(damage.intersects(rect(32, 1, 33, 2)));
        assert!(!damage.intersects(rect(0, 32, 32, 33)));
    }

    #[test]
    fn damage_tiles_union_and_full_fallback_are_conservative() {
        let mut first = DamageTileMap::empty(1280, 720);
        first.mark_pixel(1, 1);
        let mut second = DamageTileMap::empty(1280, 720);
        second.mark_pixel(1279, 719);
        first.union_with(second);
        assert_eq!(first.tile_count(), 2);

        first.union_with(DamageTileMap::empty(1281, 720));
        assert!(first.is_full_fallback());
        assert!(first.intersects(rect(100, 100, 101, 101)));
        assert_eq!(first.total_rgb565_bytes(), 1280 * 720 * 2);
    }

    fn layer(rect: DirtyRect, version: u64) -> DirectLayerState {
        DirectLayerState::new(rect, version)
    }

    fn full() -> DirtyRect {
        rect(0, 0, WIDTH, HEIGHT)
    }

    fn all_writable(state: &mut TwoBufferLatchState) {
        state.sync_hardware(None, 0, false, 0);
    }

    fn input(
        cached_damage: Option<DirtyRect>,
        preview: Option<DirectLayerState>,
        preview_dirty: Option<DirtyRect>,
        arcade: Option<DirectLayerState>,
        arcade_dirty: Option<ArcadeListUpdate>,
    ) -> LauncherFramePlan {
        LauncherFramePlan::from_rects(cached_damage, preview, preview_dirty, arcade, arcade_dirty)
    }

    fn copy_restore(buffer: &mut [Rgb565Pixel], cached: &[Rgb565Pixel], plan: LatchPresentPlan) {
        for rect in plan.restore_rects.iter() {
            for y in rect.y0..rect.y1 {
                let row = y * WIDTH;
                for x in rect.x0..rect.x1 {
                    buffer[row + x] = cached[row + x];
                }
            }
        }
    }

    fn apply_plan(buffer: &mut [Rgb565Pixel], cached: &[Rgb565Pixel], plan: LatchPresentPlan) {
        copy_restore(buffer, cached, plan);
        if let Some(rect) = plan.preview_redraw {
            fill_rect(buffer, rect, PREVIEW);
        }
        if let Some(update) = plan.arcade_redraw {
            fill_rect(buffer, arcade_update_rect(update), ARCADE);
        }
    }

    fn arcade_update_rect(update: ArcadeListUpdate) -> DirtyRect {
        match update {
            ArcadeListUpdate::Full(rect) | ArcadeListUpdate::Scroll { rect, .. } => rect,
        }
    }

    fn fill_rect(buffer: &mut [Rgb565Pixel], rect: DirtyRect, pixel: Rgb565Pixel) {
        for y in rect.y0..rect.y1 {
            let row = y * WIDTH;
            for x in rect.x0..rect.x1 {
                buffer[row + x] = pixel;
            }
        }
    }

    fn parse_ppm_fixture(text: &str) -> Vec<Rgb565Pixel> {
        let values = text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .flat_map(str::split_whitespace)
            .collect::<Vec<_>>();
        assert_eq!(values[0], "P3");
        let width = values[1].parse::<usize>().unwrap();
        let height = values[2].parse::<usize>().unwrap();
        let max = values[3].parse::<u32>().unwrap();
        assert_eq!((width, height), (WIDTH, HEIGHT));
        assert_eq!(max, 255);
        values[4..]
            .chunks_exact(3)
            .map(|rgb| {
                let r = rgb[0].parse::<u8>().unwrap();
                let g = rgb[1].parse::<u8>().unwrap();
                let b = rgb[2].parse::<u8>().unwrap();
                Rgb565Pixel(
                    (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3),
                )
            })
            .collect()
    }

    #[test]
    fn first_posts_restore_full_frames_before_direct_layers() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);

        let first = state
            .plan_next(input(Some(full()), None, None, None, None))
            .expect("first slot");
        assert_eq!(first.slot_index, 1);
        assert_eq!(first.restore_rects, DirtyRectList::from_one(full()));
        state.mark_post_success(first);

        all_writable(&mut state);
        let second = state
            .plan_next(input(None, None, None, None, None))
            .expect("second slot");
        assert_eq!(second.slot_index, 2);
        assert_eq!(second.restore_rects, DirtyRectList::from_one(full()));
    }

    #[test]
    fn idle_mixed_arcade_reuse_keeps_direct_layers_visible() {
        let preview = rect(1, 0, 4, 2);
        let arcade = rect(2, 1, 4, 3);
        let preview_layer = layer(preview, 1);
        let arcade_layer = layer(arcade, 1);
        let cached = vec![BASE; WIDTH * HEIGHT];
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        let mut slot1 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];
        let mut slot2 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];

        all_writable(&mut state);
        let first = state
            .plan_next(input(
                Some(full()),
                Some(preview_layer),
                Some(preview),
                Some(arcade_layer),
                Some(ArcadeListUpdate::Full(arcade)),
            ))
            .expect("first plan");
        apply_plan(&mut slot1, &cached, first);
        state.mark_post_success(first);

        all_writable(&mut state);
        let second = state
            .plan_next(input(
                None,
                Some(preview_layer),
                None,
                Some(arcade_layer),
                None,
            ))
            .expect("second plan");
        apply_plan(&mut slot2, &cached, second);
        state.mark_post_success(second);

        all_writable(&mut state);
        let third = state
            .plan_next(input(
                None,
                Some(preview_layer),
                None,
                Some(arcade_layer),
                None,
            ))
            .expect("third plan");
        assert_eq!(third.slot_index, 1);
        assert_eq!(third.preview_redraw, None);
        assert_eq!(third.arcade_redraw, None);
        apply_plan(&mut slot1, &cached, third);

        assert_eq!(
            slot1,
            parse_ppm_fixture(include_str!("../../testdata/latch_overlay_order.ppm"))
        );
    }

    #[test]
    fn desired_layer_redraws_when_selected_slot_lacks_it() {
        let preview = rect(1, 0, 4, 2);
        let arcade = rect(2, 1, 4, 3);
        let preview_layer = layer(preview, 1);
        let arcade_layer = layer(arcade, 1);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);

        all_writable(&mut state);
        let plan = state
            .plan_next(input(
                None,
                Some(preview_layer),
                None,
                Some(arcade_layer),
                None,
            ))
            .expect("plan");

        assert_eq!(plan.preview_redraw, Some(preview));
        assert_eq!(plan.arcade_redraw, Some(ArcadeListUpdate::Full(arcade)));
    }

    #[test]
    fn direct_residue_is_restored_when_overlay_disappears_on_reused_slot() {
        let preview = rect(1, 0, 4, 2);
        let arcade = rect(2, 1, 4, 3);
        let preview_layer = layer(preview, 1);
        let arcade_layer = layer(arcade, 1);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        let cached = vec![BASE; WIDTH * HEIGHT];
        let mut slot1 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];
        let mut slot2 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];

        all_writable(&mut state);
        let first = state
            .plan_next(input(
                Some(full()),
                Some(preview_layer),
                Some(preview),
                Some(arcade_layer),
                Some(ArcadeListUpdate::Full(arcade)),
            ))
            .expect("first plan");
        apply_plan(&mut slot1, &cached, first);
        state.mark_post_success(first);

        all_writable(&mut state);
        let second = state
            .plan_next(input(None, None, None, None, None))
            .expect("second plan");
        copy_restore(&mut slot2, &cached, second);
        state.mark_post_success(second);

        all_writable(&mut state);
        let third = state
            .plan_next(input(None, None, None, None, None))
            .expect("third plan");
        assert_eq!(third.slot_index, 1);
        copy_restore(&mut slot1, &cached, third);

        assert_eq!(
            slot1,
            parse_ppm_fixture(include_str!("../../testdata/latch_residue_cleared.ppm"))
        );
    }

    #[test]
    fn screensaver_full_frame_replaces_direct_layers_in_both_slots() {
        let preview = rect(1, 0, 4, 2);
        let arcade = rect(2, 1, 4, 3);
        let preview_layer = layer(preview, 1);
        let arcade_layer = layer(arcade, 1);
        let launcher = vec![BASE; WIDTH * HEIGHT];
        let screensaver = vec![Rgb565Pixel(0x001f); WIDTH * HEIGHT];
        let mut slot1 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];
        let mut slot2 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);

        all_writable(&mut state);
        let launcher_plan = state
            .plan_next(input(
                Some(full()),
                Some(preview_layer),
                Some(preview),
                Some(arcade_layer),
                Some(ArcadeListUpdate::Full(arcade)),
            ))
            .expect("launcher plan");
        apply_plan(&mut slot1, &launcher, launcher_plan);
        state.mark_post_success(launcher_plan);

        all_writable(&mut state);
        let second_launcher_plan = state
            .plan_next(input(
                Some(full()),
                Some(preview_layer),
                Some(preview),
                Some(arcade_layer),
                Some(ArcadeListUpdate::Full(arcade)),
            ))
            .expect("second launcher plan");
        apply_plan(&mut slot2, &launcher, second_launcher_plan);
        state.mark_post_success(second_launcher_plan);

        all_writable(&mut state);
        let first_screensaver_plan = state
            .plan_next(input(Some(full()), None, None, None, None))
            .expect("first screensaver plan");
        copy_restore(&mut slot1, &screensaver, first_screensaver_plan);
        state.mark_post_success(first_screensaver_plan);

        all_writable(&mut state);
        let second_screensaver_plan = state
            .plan_next(input(Some(full()), None, None, None, None))
            .expect("second screensaver plan");
        copy_restore(&mut slot2, &screensaver, second_screensaver_plan);
        state.mark_post_success(second_screensaver_plan);

        assert_eq!(slot1, screensaver);
        assert_eq!(slot2, screensaver);
    }

    #[test]
    fn same_rect_new_content_version_forces_redraw() {
        let preview = rect(1, 0, 4, 2);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let first = state
            .plan_next(input(
                Some(full()),
                Some(layer(preview, 1)),
                Some(preview),
                None,
                None,
            ))
            .expect("first plan");
        state.mark_post_success(first);

        all_writable(&mut state);
        state.mark_post_success(
            state
                .plan_next(input(None, None, None, None, None))
                .expect("second plan"),
        );

        all_writable(&mut state);
        let third = state
            .plan_next(input(None, Some(layer(preview, 2)), None, None, None))
            .expect("third plan");

        assert_eq!(third.preview_redraw, Some(preview));
    }

    #[test]
    fn moved_layer_restores_old_rect_and_redraws_new_rect() {
        let old_preview = rect(0, 0, 2, 2);
        let new_preview = rect(2, 0, 4, 2);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let first = state
            .plan_next(input(
                Some(full()),
                Some(layer(old_preview, 1)),
                Some(old_preview),
                None,
                None,
            ))
            .expect("first plan");
        state.mark_post_success(first);

        all_writable(&mut state);
        state.mark_post_success(
            state
                .plan_next(input(None, None, None, None, None))
                .expect("second plan"),
        );

        all_writable(&mut state);
        let third = state
            .plan_next(input(None, Some(layer(new_preview, 2)), None, None, None))
            .expect("third plan");

        assert!(third.restore_rects.iter().any(|rect| rect == old_preview));
        assert_eq!(third.preview_redraw, Some(new_preview));
    }

    #[test]
    fn overlapping_moved_layer_restores_only_uncovered_old_residue() {
        let old_preview = rect(0, 0, 3, 2);
        let new_preview = rect(1, 0, 4, 2);
        let old_residue = rect(0, 0, 1, 2);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let first = state
            .plan_next(input(
                Some(full()),
                Some(layer(old_preview, 1)),
                Some(old_preview),
                None,
                None,
            ))
            .expect("first plan");
        state.mark_post_success(first);
        all_writable(&mut state);
        state.mark_post_success(
            state
                .plan_next(input(None, None, None, None, None))
                .expect("second plan"),
        );
        all_writable(&mut state);

        let moved = state
            .plan_next(input(None, Some(layer(new_preview, 2)), None, None, None))
            .expect("moved plan");

        assert_eq!(moved.restore_rects, DirtyRectList::from_one(old_residue));
        assert_eq!(moved.preview_redraw, Some(new_preview));
    }

    #[test]
    fn cached_damage_intersecting_desired_layer_forces_redraw() {
        let preview = rect(1, 0, 4, 2);
        let preview_layer = layer(preview, 1);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let first = state
            .plan_next(input(
                Some(full()),
                Some(preview_layer),
                Some(preview),
                None,
                None,
            ))
            .expect("first plan");
        state.mark_post_success(first);

        all_writable(&mut state);
        state.mark_post_success(
            state
                .plan_next(input(None, None, None, None, None))
                .expect("second plan"),
        );

        all_writable(&mut state);
        let third = state
            .plan_next(input(
                Some(rect(0, 0, 2, 1)),
                Some(preview_layer),
                None,
                None,
                None,
            ))
            .expect("third plan");

        assert_eq!(third.preview_redraw, Some(preview));
    }

    #[test]
    fn overlapping_preview_and_arcade_preserve_layer_order() {
        let preview = rect(1, 0, 4, 2);
        let arcade = rect(2, 1, 4, 3);
        let cached = vec![BASE; WIDTH * HEIGHT];
        let mut slot = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);

        let plan = state
            .plan_next(input(
                Some(full()),
                Some(layer(preview, 1)),
                Some(preview),
                Some(layer(arcade, 1)),
                Some(ArcadeListUpdate::Full(arcade)),
            ))
            .expect("plan");
        apply_plan(&mut slot, &cached, plan);

        assert_eq!(
            slot,
            parse_ppm_fixture(include_str!("../../testdata/latch_overlay_order.ppm"))
        );
    }

    #[test]
    fn direct_redraws_are_subtracted_from_cached_restore_without_changing_order() {
        let preview = rect(1, 0, 4, 2);
        let arcade = rect(2, 1, 4, 3);
        let cached = vec![BASE; WIDTH * HEIGHT];
        let mut slot = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);

        let plan = state
            .plan_next(input(
                Some(full()),
                Some(layer(preview, 1)),
                Some(preview),
                Some(layer(arcade, 1)),
                Some(ArcadeListUpdate::Full(arcade)),
            ))
            .expect("plan");

        assert_eq!(plan.restore_rects.total_rgb565_bytes(), 4 * 2);
        assert!(plan.restore_rects.iter().all(|restore| {
            restore.intersection(preview).is_none() && restore.intersection(arcade).is_none()
        }));
        apply_plan(&mut slot, &cached, plan);
        assert_eq!(
            slot,
            parse_ppm_fixture(include_str!("../../testdata/latch_overlay_order.ppm"))
        );
    }

    #[test]
    fn changed_arcade_version_promotes_scroll_to_full_before_subtraction() {
        let preview = rect(1, 0, 4, 2);
        let arcade = rect(2, 1, 4, 3);
        let preview_layer = layer(preview, 1);
        let arcade_v1 = layer(arcade, 1);
        let arcade_v2 = layer(arcade, 2);
        let cached = vec![BASE; WIDTH * HEIGHT];
        let mut slot1 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];
        let mut slot2 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);

        all_writable(&mut state);
        let first = state
            .plan_next(input(
                Some(full()),
                Some(preview_layer),
                Some(preview),
                Some(arcade_v1),
                Some(ArcadeListUpdate::Full(arcade)),
            ))
            .expect("first plan");
        apply_plan(&mut slot1, &cached, first);
        state.mark_post_success(first);

        all_writable(&mut state);
        let second = state
            .plan_next(input(
                Some(full()),
                Some(preview_layer),
                Some(preview),
                Some(arcade_v1),
                Some(ArcadeListUpdate::Full(arcade)),
            ))
            .expect("second plan");
        apply_plan(&mut slot2, &cached, second);
        state.mark_post_success(second);

        all_writable(&mut state);
        let changed = state
            .plan_next(input(
                Some(full()),
                Some(preview_layer),
                None,
                Some(arcade_v2),
                Some(ArcadeListUpdate::Scroll {
                    delta_y: -1,
                    rect: arcade,
                }),
            ))
            .expect("changed plan");

        assert_eq!(changed.preview_redraw, Some(preview));
        assert_eq!(changed.arcade_redraw, Some(ArcadeListUpdate::Full(arcade)));
        assert!(changed.restore_rects.iter().all(|restore| {
            restore.intersection(preview).is_none() && restore.intersection(arcade).is_none()
        }));
        apply_plan(&mut slot1, &cached, changed);
        assert_eq!(
            slot1,
            parse_ppm_fixture(include_str!("../../testdata/latch_overlay_order.ppm"))
        );
    }

    #[test]
    fn matching_arcade_generation_accumulates_scroll_for_older_slot() {
        let arcade = rect(0, 0, 4, 3);
        let current = layer(arcade, 7).with_content_offset_y(-3);
        let desired = layer(arcade, 7).with_content_offset_y(-11);

        assert_eq!(
            direct_layer_redraw_update(current.into(), desired.into(), None, true),
            Some(ArcadeListUpdate::Scroll {
                delta_y: -8,
                rect: arcade,
            })
        );
    }

    #[test]
    fn failed_attempt_does_not_mark_direct_layers_valid() {
        let preview = rect(1, 0, 4, 2);
        let preview_layer = layer(preview, 1);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let plan = state
            .plan_next(input(
                Some(full()),
                Some(preview_layer),
                Some(preview),
                None,
                None,
            ))
            .expect("plan");

        state.mark_attempt_failed(plan.slot_index);

        all_writable(&mut state);
        let retry = state
            .plan_next(input(None, Some(preview_layer), None, None, None))
            .expect("retry");
        assert_eq!(retry.preview_redraw, Some(preview));
    }

    #[test]
    fn pending_status_blocks_writes_to_both_slots() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        state.sync_hardware(None, 0, true, 7);

        assert!(
            state
                .plan_next(input(None, None, None, None, None))
                .is_none()
        );
    }

    #[test]
    fn active_preferred_slot_selects_other_writable_slot() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        state.sync_hardware(Some(1), 3, false, 0);

        let plan = state
            .plan_next(input(None, None, None, None, None))
            .expect("other slot should be writable");

        assert_eq!(plan.slot_index, 2);
    }

    #[test]
    fn failed_attempt_full_invalidates_attempted_slot() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let plan = state
            .plan_next(input(Some(full()), None, None, None, None))
            .expect("plan");

        state.mark_attempt_failed(plan.slot_index);

        assert_eq!(
            state.restore_bytes_for_slot(plan.slot_index),
            WIDTH * HEIGHT * std::mem::size_of::<Rgb565Pixel>()
        );
    }

    #[test]
    fn fb0_recovery_forces_full_cached_restore_and_both_direct_layers() {
        let preview = rect(1, 0, 3, 2);
        let arcade = rect(0, 1, 2, 3);
        let original = input(
            Some(rect(0, 0, 1, 1)),
            Some(layer(preview, 7)),
            None,
            Some(layer(arcade, 9)),
            None,
        );

        let recovered = original.for_fb0_recovery(full());

        assert_eq!(recovered.cached_damage(), DirtyRectList::from_one(full()));
        assert_eq!(recovered.preview_dirty(), Some(preview));
        assert_eq!(
            recovered.arcade_dirty(),
            Some(ArcadeListUpdate::Full(arcade))
        );
    }
}
