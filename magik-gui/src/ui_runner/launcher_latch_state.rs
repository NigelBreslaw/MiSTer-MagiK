use mister_magik_fb::framebuffer::target::{DirtyRect, DirtyRectList};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LatchSlotHardwareState {
    Unknown,
    Writable,
    Pending(u16),
    Active(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LatchSlotCoherency {
    base_invalid: DirtyRectList,
    direct_residue: DirtyRectList,
    hardware: LatchSlotHardwareState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LatchFrameDamage {
    cached_damage: DirtyRectList,
    direct_damage: DirtyRectList,
}

impl LatchFrameDamage {
    pub(super) fn new(
        cached_damage: DirtyRectList,
        preview_direct_rect: Option<DirtyRect>,
        arcade_list_rect: Option<DirtyRect>,
    ) -> Self {
        let mut direct = DirtyRectList::new();
        direct.push_if_some(preview_direct_rect);
        direct.push_if_some(arcade_list_rect);

        Self {
            cached_damage,
            direct_damage: direct,
        }
    }

    #[cfg(test)]
    fn from_rects(
        cached_damage: Option<DirtyRect>,
        preview_direct_rect: Option<DirtyRect>,
        arcade_list_rect: Option<DirtyRect>,
    ) -> Self {
        let mut cached = DirtyRectList::new();
        cached.push_if_some(cached_damage);
        Self::new(cached, preview_direct_rect, arcade_list_rect)
    }

    fn cached_damage(self) -> DirtyRectList {
        self.cached_damage
    }

    fn direct_damage(self) -> DirtyRectList {
        self.direct_damage
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LatchPresentPlan {
    pub(super) slot_index: u8,
    pub(super) restore_rects: DirtyRectList,
    cached_damage: DirtyRectList,
    direct_residue_after: DirtyRectList,
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
                    direct_residue: DirtyRectList::new(),
                    hardware: LatchSlotHardwareState::Unknown,
                },
                LatchSlotCoherency {
                    base_invalid: full_invalid,
                    direct_residue: DirtyRectList::new(),
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
            slot.direct_residue.clear();
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

    pub(super) fn plan_next(&self, damage: LatchFrameDamage) -> Option<LatchPresentPlan> {
        let slot_index = self.select_writable_slot()?;
        Some(self.plan_for_slot(slot_index, damage))
    }

    pub(super) fn mark_post_success(&mut self, plan: LatchPresentPlan) {
        let slot_index = plan.slot_index;
        let other_index = other_slot(slot_index);

        let selected = self.slot_mut(slot_index);
        selected.base_invalid.clear();
        selected.direct_residue = plan.direct_residue_after;
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
        slot.direct_residue.clear();
        slot.hardware = LatchSlotHardwareState::Unknown;
    }

    pub(super) fn restore_bytes_for_slot(&self, slot_index: u8) -> usize {
        let slot = self.slot(slot_index);
        slot.base_invalid.total_rgb565_bytes() + slot.direct_residue.total_rgb565_bytes()
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

    fn plan_for_slot(&self, slot_index: u8, damage: LatchFrameDamage) -> LatchPresentPlan {
        let slot = self.slot(slot_index);
        let mut restore_rects = DirtyRectList::new();
        extend_without_covered_rects(&mut restore_rects, &slot.base_invalid);
        extend_without_covered_rects(&mut restore_rects, &slot.direct_residue);
        extend_without_covered_rects(&mut restore_rects, &damage.cached_damage());

        LatchPresentPlan {
            slot_index,
            restore_rects,
            cached_damage: damage.cached_damage(),
            direct_residue_after: damage.direct_damage(),
        }
    }

    fn slot(&self, slot_index: u8) -> &LatchSlotCoherency {
        &self.slots[slot_offset(slot_index)]
    }

    fn slot_mut(&mut self, slot_index: u8) -> &mut LatchSlotCoherency {
        &mut self.slots[slot_offset(slot_index)]
    }
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
        if !target.iter().any(|existing| existing.contains(rect)) {
            target.push(rect);
        }
    }
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

    fn all_writable(state: &mut TwoBufferLatchState) {
        state.sync_hardware(None, 0, false, 0);
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
        let full = rect(0, 0, WIDTH, HEIGHT);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);

        let first = state
            .plan_next(LatchFrameDamage::from_rects(Some(full), None, None))
            .expect("first slot");
        assert_eq!(first.slot_index, 1);
        assert_eq!(first.restore_rects, DirtyRectList::from_one(full));
        state.mark_post_success(first);

        all_writable(&mut state);
        let second = state
            .plan_next(LatchFrameDamage::from_rects(None, None, None))
            .expect("second slot");
        assert_eq!(second.slot_index, 2);
        assert_eq!(second.restore_rects, DirtyRectList::from_one(full));
    }

    #[test]
    fn direct_residue_is_restored_when_overlay_disappears_on_reused_slot() {
        let preview = rect(1, 0, 4, 2);
        let arcade = rect(2, 1, 4, 3);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        let cached = vec![BASE; WIDTH * HEIGHT];
        let mut slot1 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];
        let mut slot2 = vec![Rgb565Pixel(0xffff); WIDTH * HEIGHT];

        all_writable(&mut state);
        let first = state
            .plan_next(LatchFrameDamage::new(
                DirtyRectList::from_one(rect(0, 0, WIDTH, HEIGHT)),
                Some(preview),
                Some(arcade),
            ))
            .expect("first plan");
        copy_restore(&mut slot1, &cached, first);
        fill_rect(&mut slot1, preview, PREVIEW);
        fill_rect(&mut slot1, arcade, ARCADE);
        assert_eq!(
            slot1,
            parse_ppm_fixture(include_str!("../../testdata/latch_overlay_order.ppm"))
        );
        state.mark_post_success(first);

        all_writable(&mut state);
        let second = state
            .plan_next(LatchFrameDamage::from_rects(None, None, None))
            .expect("second plan");
        copy_restore(&mut slot2, &cached, second);
        state.mark_post_success(second);

        all_writable(&mut state);
        let third = state
            .plan_next(LatchFrameDamage::from_rects(None, None, None))
            .expect("third plan");
        assert_eq!(third.slot_index, 1);
        copy_restore(&mut slot1, &cached, third);

        assert_eq!(
            slot1,
            parse_ppm_fixture(include_str!("../../testdata/latch_residue_cleared.ppm"))
        );
    }

    #[test]
    fn pending_status_blocks_writes_to_both_slots() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        state.sync_hardware(None, 0, true, 7);

        assert!(state
            .plan_next(LatchFrameDamage::from_rects(None, None, None))
            .is_none());
    }

    #[test]
    fn active_preferred_slot_selects_other_writable_slot() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        state.sync_hardware(Some(1), 3, false, 0);

        let plan = state
            .plan_next(LatchFrameDamage::from_rects(None, None, None))
            .expect("other slot should be writable");

        assert_eq!(plan.slot_index, 2);
    }

    #[test]
    fn failed_attempt_full_invalidates_attempted_slot() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let plan = state
            .plan_next(LatchFrameDamage::from_rects(
                Some(rect(0, 0, WIDTH, HEIGHT)),
                None,
                None,
            ))
            .expect("plan");

        state.mark_attempt_failed(plan.slot_index);

        assert_eq!(
            state.restore_bytes_for_slot(plan.slot_index),
            WIDTH * HEIGHT * std::mem::size_of::<Rgb565Pixel>()
        );
    }
}
