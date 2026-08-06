// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::damage::TwoSlotDamageLedger;
use super::target::{DirtyRect, DirtyRectList, subtract_dirty_rects};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectLayerUpdate {
    Full(DirtyRect),
    Scroll { delta_y: isize, rect: DirtyRect },
}

impl DirectLayerUpdate {
    pub const fn dirty_rect(self) -> DirtyRect {
        match self {
            Self::Full(rect) | Self::Scroll { rect, .. } => rect,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatchSlotHardwareState {
    Unknown,
    Writable,
    Pending(u16),
    Active(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectLayerState {
    pub rect: DirtyRect,
    pub version: u64,
    pub content_offset_y: i64,
}

impl DirectLayerState {
    pub fn new(rect: DirtyRect, version: u64) -> Self {
        Self {
            rect,
            version,
            content_offset_y: 0,
        }
    }

    pub fn with_content_offset_y(mut self, content_offset_y: i64) -> Self {
        self.content_offset_y = content_offset_y;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LatchSlotCoherency {
    preview_present: Option<DirectLayerState>,
    arcade_present: Option<DirectLayerState>,
    hardware: LatchSlotHardwareState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchFramePlan {
    cached_damage: DirtyRectList,
    preview_desired: Option<DirectLayerState>,
    preview_dirty: Option<DirtyRect>,
    arcade_desired: Option<DirectLayerState>,
    arcade_dirty: Option<DirectLayerUpdate>,
}

impl LatchFramePlan {
    pub fn new(
        cached_damage: DirtyRectList,
        preview_desired: Option<DirectLayerState>,
        preview_dirty: Option<DirtyRect>,
        arcade_desired: Option<DirectLayerState>,
        arcade_dirty: Option<DirectLayerUpdate>,
    ) -> Self {
        Self {
            cached_damage,
            preview_desired,
            preview_dirty,
            arcade_desired,
            arcade_dirty,
        }
    }

    pub fn cached_damage(self) -> DirtyRectList {
        self.cached_damage
    }

    pub fn preview_dirty(self) -> Option<DirtyRect> {
        self.preview_dirty
    }

    pub fn arcade_dirty(self) -> Option<DirectLayerUpdate> {
        self.arcade_dirty
    }

    pub fn for_fb0_recovery(self, full_rect: DirtyRect) -> Self {
        Self {
            cached_damage: DirtyRectList::from_one(full_rect),
            preview_dirty: self.preview_desired.map(|layer| layer.rect),
            arcade_dirty: self
                .arcade_desired
                .map(|layer| DirectLayerUpdate::Full(layer.rect)),
            ..self
        }
    }

    #[cfg(test)]
    fn from_rects(
        cached_damage: Option<DirtyRect>,
        preview_desired: Option<DirectLayerState>,
        preview_dirty: Option<DirtyRect>,
        arcade_desired: Option<DirectLayerState>,
        arcade_dirty: Option<DirectLayerUpdate>,
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
pub struct LatchPresentPlan {
    pub slot_index: u8,
    pub restore_rects: DirtyRectList,
    pub preview_redraw: Option<DirtyRect>,
    pub arcade_redraw: Option<DirectLayerUpdate>,
    preview_after: Option<DirectLayerState>,
    arcade_after: Option<DirectLayerState>,
}

#[derive(Clone, Debug)]
pub struct TwoBufferLatchState {
    slots: [LatchSlotCoherency; 2],
    base_damage: TwoSlotDamageLedger,
    next_slot_index: u8,
}

impl TwoBufferLatchState {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            slots: [
                LatchSlotCoherency {
                    preview_present: None,
                    arcade_present: None,
                    hardware: LatchSlotHardwareState::Unknown,
                },
                LatchSlotCoherency {
                    preview_present: None,
                    arcade_present: None,
                    hardware: LatchSlotHardwareState::Unknown,
                },
            ],
            base_damage: TwoSlotDamageLedger::new(width, height),
            next_slot_index: 1,
        }
    }

    pub fn invalidate_all(&mut self) {
        self.base_damage.invalidate_all();
        for slot in &mut self.slots {
            slot.preview_present = None;
            slot.arcade_present = None;
            slot.hardware = LatchSlotHardwareState::Unknown;
        }
        self.next_slot_index = 1;
    }

    pub fn sync_hardware(
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

    pub fn plan_next(&mut self, input: LatchFramePlan) -> Option<LatchPresentPlan> {
        self.base_damage.record_damage(&input.cached_damage);
        let slot_index = self.select_writable_slot()?;
        Some(self.plan_for_slot(slot_index, input))
    }

    pub fn mark_post_success(&mut self, plan: LatchPresentPlan) {
        let slot_index = plan.slot_index;
        self.base_damage.mark_presented(slot_index);
        let selected = self.slot_mut(slot_index);
        selected.preview_present = plan.preview_after;
        selected.arcade_present = plan.arcade_after;
        selected.hardware = LatchSlotHardwareState::Unknown;
        self.next_slot_index = other_slot(slot_index);
    }

    pub fn mark_attempt_failed(&mut self, slot_index: u8) {
        self.base_damage.mark_attempt_failed(slot_index);
        let slot = self.slot_mut(slot_index);
        slot.preview_present = None;
        slot.arcade_present = None;
        slot.hardware = LatchSlotHardwareState::Unknown;
    }

    pub fn restore_bytes_for_slot(&self, slot_index: u8) -> usize {
        let slot = self.slot(slot_index);
        let mut bytes = self.base_damage.invalid_bytes(slot_index);
        if let Some(preview) = slot.preview_present {
            bytes = bytes.saturating_add(rect_bytes(preview.rect));
        }
        if let Some(arcade) = slot.arcade_present {
            bytes = bytes.saturating_add(rect_bytes(arcade.rect));
        }
        bytes
    }

    pub fn writable_slot_index(&self) -> Option<u8> {
        self.select_writable_slot()
    }

    pub fn slot_is_writable(&self, slot_index: u8) -> bool {
        self.slot(slot_index).hardware == LatchSlotHardwareState::Writable
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

    fn plan_for_slot(&self, slot_index: u8, input: LatchFramePlan) -> LatchPresentPlan {
        let slot = self.slot(slot_index);
        let mut restore_rects = self.base_damage.plan(slot_index);

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
        direct_redraws.push_if_some(arcade_redraw.as_ref().map(direct_layer_update_rect));
        let restore_rects = subtract_dirty_rects(restore_rects, &direct_redraws);

        LatchPresentPlan {
            slot_index,
            restore_rects,
            preview_redraw,
            arcade_redraw,
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
    dirty: Option<DirectLayerUpdate>,
    intersects_restore: bool,
) -> Option<DirectLayerUpdate> {
    let desired = desired?;
    if let Some(current) = current {
        if current.rect != desired.rect || current.version != desired.version {
            Some(DirectLayerUpdate::Full(desired.rect))
        } else if current.content_offset_y != desired.content_offset_y {
            Some(DirectLayerUpdate::Scroll {
                delta_y: desired
                    .content_offset_y
                    .saturating_sub(current.content_offset_y)
                    .clamp(isize::MIN as i64, isize::MAX as i64) as isize,
                rect: desired.rect,
            })
        } else if intersects_restore {
            Some(DirectLayerUpdate::Full(desired.rect))
        } else {
            dirty
        }
    } else {
        Some(DirectLayerUpdate::Full(desired.rect))
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

fn direct_layer_update_rect(update: &DirectLayerUpdate) -> DirtyRect {
    update.dirty_rect()
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

fn push_without_covered_rect(target: &mut DirtyRectList, rect: DirtyRect) {
    if !target.iter().any(|existing| existing.contains(rect)) {
        target.push(rect);
    }
}

fn rect_bytes(rect: DirtyRect) -> usize {
    rect.width()
        .saturating_mul(rect.rows() as usize)
        .saturating_mul(super::format::RGB565_BYTES_PER_PIXEL)
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
        arcade_dirty: Option<DirectLayerUpdate>,
    ) -> LatchFramePlan {
        LatchFramePlan::from_rects(cached_damage, preview, preview_dirty, arcade, arcade_dirty)
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

    fn arcade_update_rect(update: DirectLayerUpdate) -> DirtyRect {
        match update {
            DirectLayerUpdate::Full(rect) | DirectLayerUpdate::Scroll { rect, .. } => rect,
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
                Some(DirectLayerUpdate::Full(arcade)),
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
        assert_eq!(plan.arcade_redraw, Some(DirectLayerUpdate::Full(arcade)));
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
                Some(DirectLayerUpdate::Full(arcade)),
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
    fn preview_fade_to_empty_commits_black_before_layer_retirement_in_both_slots() {
        let preview = rect(1, 0, 4, 2);
        let mut cached = vec![BASE; WIDTH * HEIGHT];
        fill_rect(&mut cached, preview, PREVIEW);
        let mut slot1 = vec![BASE; WIDTH * HEIGHT];
        let mut slot2 = vec![BASE; WIDTH * HEIGHT];
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);

        for slot in [&mut slot1, &mut slot2] {
            all_writable(&mut state);
            let plan = state
                .plan_next(input(
                    Some(full()),
                    Some(layer(preview, 1)),
                    Some(preview),
                    None,
                    None,
                ))
                .expect("initial preview plan");
            apply_plan(slot, &cached, plan);
            state.mark_post_success(plan);
        }

        fill_rect(&mut cached, preview, BASE);
        all_writable(&mut state);
        let fading = state
            .plan_next(input(
                Some(preview),
                Some(layer(preview, 2)),
                Some(preview),
                None,
                None,
            ))
            .expect("fade-to-empty plan");
        assert_eq!(fading.preview_redraw, Some(preview));
        assert!(
            fading
                .restore_rects
                .iter()
                .all(|restore| restore.intersection(preview).is_none())
        );
        apply_plan(&mut slot1, &cached, fading);
        state.mark_post_success(fading);

        all_writable(&mut state);
        let final_black = state
            .plan_next(input(
                None,
                Some(layer(preview, 3)),
                Some(preview),
                None,
                None,
            ))
            .expect("final black plan");
        copy_restore(&mut slot2, &cached, final_black);
        fill_rect(&mut slot2, preview, BASE);
        state.mark_post_success(final_black);

        all_writable(&mut state);
        let retire_first = state
            .plan_next(input(None, None, None, None, None))
            .expect("first retirement plan");
        copy_restore(&mut slot1, &cached, retire_first);
        state.mark_post_success(retire_first);

        all_writable(&mut state);
        let retire_second = state
            .plan_next(input(None, None, None, None, None))
            .expect("second retirement plan");
        copy_restore(&mut slot2, &cached, retire_second);
        state.mark_post_success(retire_second);

        assert!(slot1.iter().all(|pixel| *pixel == BASE));
        assert!(slot2.iter().all(|pixel| *pixel == BASE));
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
                Some(DirectLayerUpdate::Full(arcade)),
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
                Some(DirectLayerUpdate::Full(arcade)),
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
        let second = state
            .plan_next(input(None, None, None, None, None))
            .expect("second plan");
        state.mark_post_success(second);

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
        let second = state
            .plan_next(input(None, None, None, None, None))
            .expect("second plan");
        state.mark_post_success(second);

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
        let second = state
            .plan_next(input(None, None, None, None, None))
            .expect("second plan");
        state.mark_post_success(second);
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
        let second = state
            .plan_next(input(None, None, None, None, None))
            .expect("second plan");
        state.mark_post_success(second);

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
                Some(DirectLayerUpdate::Full(arcade)),
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
                Some(DirectLayerUpdate::Full(arcade)),
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
                Some(DirectLayerUpdate::Full(arcade)),
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
                Some(DirectLayerUpdate::Full(arcade)),
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
                Some(DirectLayerUpdate::Scroll {
                    delta_y: -1,
                    rect: arcade,
                }),
            ))
            .expect("changed plan");

        assert_eq!(changed.preview_redraw, Some(preview));
        assert_eq!(changed.arcade_redraw, Some(DirectLayerUpdate::Full(arcade)));
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
            Some(DirectLayerUpdate::Scroll {
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
            Some(DirectLayerUpdate::Full(arcade))
        );
    }
}
