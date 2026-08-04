// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Slint-independent framebuffer damage and reusable scanout-slot history.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl DirtyRect {
    pub fn rows(self) -> u32 {
        (self.y1 - self.y0) as u32
    }

    pub fn width(self) -> usize {
        self.x1 - self.x0
    }

    pub fn is_empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    pub fn is_full_width(self, render_w: usize) -> bool {
        self.x0 == 0 && self.x1 >= render_w
    }

    pub fn contains(self, other: DirtyRect) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && self.x1 >= other.x1 && self.y1 >= other.y1
    }

    pub fn intersection(self, other: DirtyRect) -> Option<DirtyRect> {
        let x0 = self.x0.max(other.x0);
        let y0 = self.y0.max(other.y0);
        let x1 = self.x1.min(other.x1);
        let y1 = self.y1.min(other.y1);
        if x1 > x0 && y1 > y0 {
            Some(DirtyRect { x0, y0, x1, y1 })
        } else {
            None
        }
    }

    pub fn union(self, other: DirtyRect) -> DirtyRect {
        DirtyRect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    #[cfg(test)]
    pub(crate) fn area(self) -> usize {
        self.width() * (self.y1 - self.y0)
    }
}

const DIRTY_RECT_LIST_CAP: usize = 32;
const EMPTY_DIRTY_RECT: DirtyRect = DirtyRect {
    x0: 0,
    y0: 0,
    x1: 0,
    y1: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRectList {
    rects: [DirtyRect; DIRTY_RECT_LIST_CAP],
    len: usize,
}

impl DirtyRectList {
    pub fn new() -> Self {
        Self {
            rects: [EMPTY_DIRTY_RECT; DIRTY_RECT_LIST_CAP],
            len: 0,
        }
    }

    pub fn from_one(rect: DirtyRect) -> Self {
        let mut list = Self::new();
        list.push(rect);
        list
    }

    pub fn push_if_some(&mut self, rect: Option<DirtyRect>) {
        if let Some(rect) = rect {
            self.push(rect);
        }
    }

    pub fn extend_from(&mut self, other: &Self) {
        for rect in other.iter() {
            self.push(rect);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = DirtyRect> + '_ {
        self.rects[..self.len].iter().copied()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, index: usize) -> Option<DirtyRect> {
        self.rects.get(index).copied().filter(|_| index < self.len)
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, rect: DirtyRect) {
        if rect.is_empty() {
            return;
        }
        if !self.try_push(rect) {
            debug_assert!(false, "dirty rect list capacity exceeded");
            let last = DIRTY_RECT_LIST_CAP - 1;
            self.rects[last] = self.rects[last].union(rect);
        }
    }

    fn try_push(&mut self, rect: DirtyRect) -> bool {
        if self.len == DIRTY_RECT_LIST_CAP {
            return false;
        }
        self.rects[self.len] = rect;
        self.len += 1;
        true
    }

    pub fn total_rgb565_bytes(&self) -> usize {
        self.iter()
            .map(|rect| rect.width() * rect.rows() as usize * size_of::<u16>())
            .sum()
    }

    fn add_canonical(&mut self, rect: DirtyRect, full_rect: DirtyRect) {
        let Some(rect) = rect.intersection(full_rect) else {
            return;
        };
        if self.iter().any(|existing| existing.contains(rect)) {
            return;
        }
        let previous = *self;
        self.clear();
        for existing in previous.iter().filter(|existing| !rect.contains(*existing)) {
            if !self.try_push(existing) {
                *self = Self::from_one(full_rect);
                return;
            }
        }
        if !self.try_push(rect) {
            *self = Self::from_one(full_rect);
        }
    }

    fn extend_canonical(&mut self, damage: &Self, full_rect: DirtyRect) {
        for rect in damage.iter() {
            self.add_canonical(rect, full_rect);
            if *self == Self::from_one(full_rect) {
                break;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn to_vec(self) -> Vec<DirtyRect> {
        self.iter().collect()
    }
}

impl Default for DirtyRectList {
    fn default() -> Self {
        Self::new()
    }
}

fn subtract_rect_into(rect: DirtyRect, cut: DirtyRect, out: &mut DirtyRectList) -> bool {
    let Some(overlap) = rect.intersection(cut) else {
        return out.try_push(rect);
    };
    if rect.y0 < overlap.y0
        && !out.try_push(DirtyRect {
            x0: rect.x0,
            y0: rect.y0,
            x1: rect.x1,
            y1: overlap.y0,
        })
    {
        return false;
    }
    if overlap.y1 < rect.y1
        && !out.try_push(DirtyRect {
            x0: rect.x0,
            y0: overlap.y1,
            x1: rect.x1,
            y1: rect.y1,
        })
    {
        return false;
    }
    if rect.x0 < overlap.x0
        && !out.try_push(DirtyRect {
            x0: rect.x0,
            y0: overlap.y0,
            x1: overlap.x0,
            y1: overlap.y1,
        })
    {
        return false;
    }
    if overlap.x1 < rect.x1
        && !out.try_push(DirtyRect {
            x0: overlap.x1,
            y0: overlap.y0,
            x1: rect.x1,
            y1: overlap.y1,
        })
    {
        return false;
    }
    true
}

pub fn subtract_dirty_rects(rects: DirtyRectList, cuts: &DirtyRectList) -> DirtyRectList {
    let original = rects;
    let mut current = rects;
    let mut next = DirtyRectList::new();
    for cut in cuts.iter() {
        next.clear();
        for rect in current.iter() {
            if !subtract_rect_into(rect, cut, &mut next) {
                return original;
            }
        }
        std::mem::swap(&mut current, &mut next);
        if current.is_empty() {
            break;
        }
    }
    current
}

#[derive(Clone, Debug)]
pub struct TwoSlotDamageLedger {
    debts: [DirtyRectList; 2],
    full_rect: DirtyRect,
}

impl TwoSlotDamageLedger {
    pub fn new(width: usize, height: usize) -> Self {
        let full_rect = DirtyRect {
            x0: 0,
            y0: 0,
            x1: width,
            y1: height,
        };
        let full = DirtyRectList::from_one(full_rect);
        Self {
            debts: [full; 2],
            full_rect,
        }
    }

    pub fn record_damage(&mut self, damage: &DirtyRectList) {
        for debt in &mut self.debts {
            debt.extend_canonical(damage, self.full_rect);
        }
    }

    pub fn plan(&self, slot_index: u8) -> DirtyRectList {
        self.debts[slot_offset(slot_index)]
    }

    pub fn mark_presented(&mut self, slot_index: u8) {
        self.debts[slot_offset(slot_index)].clear();
    }

    pub fn mark_attempt_failed(&mut self, slot_index: u8) {
        self.invalidate_slot(slot_index);
    }

    pub fn invalidate_slot(&mut self, slot_index: u8) {
        self.debts[slot_offset(slot_index)] = DirtyRectList::from_one(self.full_rect);
    }

    pub fn invalidate_all(&mut self) {
        let full = DirtyRectList::from_one(self.full_rect);
        self.debts = [full; 2];
    }

    pub fn invalid_bytes(&self, slot_index: u8) -> usize {
        self.plan(slot_index).total_rgb565_bytes()
    }
}

fn slot_offset(slot_index: u8) -> usize {
    match slot_index {
        1 => 0,
        2 => 1,
        _ => panic!("hidden slot index must be 1 or 2"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    #[test]
    fn first_use_of_each_slot_restores_the_full_frame() {
        let mut ledger = TwoSlotDamageLedger::new(960, 600);
        assert_eq!(
            ledger.plan(1),
            DirtyRectList::from_one(rect(0, 0, 960, 600))
        );
        ledger.mark_presented(1);
        assert!(ledger.plan(1).is_empty());
        assert_eq!(
            ledger.plan(2),
            DirtyRectList::from_one(rect(0, 0, 960, 600))
        );
    }

    #[test]
    fn alternating_slots_retain_every_cached_mutation() {
        let mut ledger = TwoSlotDamageLedger::new(100, 80);
        ledger.mark_presented(1);
        ledger.mark_presented(2);
        let a = DirtyRectList::from_one(rect(10, 10, 20, 20));
        ledger.record_damage(&a);
        assert_eq!(ledger.plan(1), a);
        ledger.mark_presented(1);
        let b = DirtyRectList::from_one(rect(30, 30, 40, 40));
        ledger.record_damage(&b);
        assert_eq!(ledger.plan(1), b);
        assert_eq!(
            ledger.plan(2).to_vec(),
            vec![rect(10, 10, 20, 20), rect(30, 30, 40, 40)]
        );
    }

    #[test]
    fn suppressed_presentation_keeps_damage_on_both_slots() {
        let mut ledger = TwoSlotDamageLedger::new(100, 80);
        ledger.mark_presented(1);
        ledger.mark_presented(2);
        let damage = DirtyRectList::from_one(rect(4, 5, 12, 18));
        ledger.record_damage(&damage);
        assert_eq!(ledger.plan(1), damage);
        assert_eq!(ledger.plan(2), damage);
    }

    #[test]
    fn covered_damage_is_canonical_and_failure_is_full_invalid() {
        let mut ledger = TwoSlotDamageLedger::new(100, 80);
        ledger.mark_presented(1);
        ledger.mark_presented(2);
        let mut damage = DirtyRectList::new();
        damage.push(rect(10, 10, 20, 20));
        damage.push(rect(0, 0, 40, 40));
        ledger.record_damage(&damage);
        assert_eq!(ledger.plan(1), DirtyRectList::from_one(rect(0, 0, 40, 40)));
        ledger.mark_attempt_failed(1);
        assert_eq!(ledger.plan(1), DirtyRectList::from_one(rect(0, 0, 100, 80)));
        assert_eq!(ledger.plan(2), DirtyRectList::from_one(rect(0, 0, 40, 40)));
    }

    #[test]
    fn damage_is_clipped_to_the_render_surface() {
        let mut ledger = TwoSlotDamageLedger::new(100, 80);
        ledger.mark_presented(1);
        ledger.mark_presented(2);
        ledger.record_damage(&DirtyRectList::from_one(rect(90, 70, 120, 100)));
        assert_eq!(
            ledger.plan(1),
            DirtyRectList::from_one(rect(90, 70, 100, 80))
        );
    }
}
