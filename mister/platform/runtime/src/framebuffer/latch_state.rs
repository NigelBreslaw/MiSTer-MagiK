// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::damage::TwoSlotDamageLedger;
use super::target::{DirtyRect, DirtyRectList, PhysicalLayerView, subtract_dirty_rects};
use slint::platform::software_renderer::Rgb565Pixel;
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalLayerRole {
    Preview,
    Arcade,
}

impl PhysicalLayerRole {
    pub const COUNT: usize = 2;
    pub const ALL: [Self; Self::COUNT] = [Self::Preview, Self::Arcade];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Arcade => "arcade",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::Preview => 0,
            Self::Arcade => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalLayerBackingKey {
    pub role: PhysicalLayerRole,
    pub layout_generation: u64,
    pub layout_epoch: u64,
    pub content_generation: u64,
    pub rect: DirtyRect,
    pub stride: usize,
    pub pixel_count: usize,
    pub source_address: usize,
}

#[derive(Clone)]
struct PhysicalLayerPublicationBacking {
    pixels: Arc<[Rgb565Pixel]>,
    rect: DirtyRect,
}

impl fmt::Debug for PhysicalLayerPublicationBacking {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhysicalLayerPublicationBacking")
            .field("rect", &self.rect)
            .field("pixel_count", &self.pixels.len())
            .field("source_address", &(self.pixels.as_ptr() as usize))
            .finish()
    }
}

impl PartialEq for PhysicalLayerPublicationBacking {
    fn eq(&self, other: &Self) -> bool {
        self.rect == other.rect && Arc::ptr_eq(&self.pixels, &other.pixels)
    }
}

impl Eq for PhysicalLayerPublicationBacking {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalLayerPublication {
    role: PhysicalLayerRole,
    layout_generation: u64,
    layout_epoch: u64,
    content_generation: u64,
    state: PhysicalLayerState,
    update: Option<PhysicalLayerUpdate>,
    backing_key: PhysicalLayerBackingKey,
    backing: PhysicalLayerPublicationBacking,
}

impl PhysicalLayerPublication {
    pub fn capture(
        role: PhysicalLayerRole,
        layout_generation: u64,
        layout_epoch: u64,
        content_generation: u64,
        state: PhysicalLayerState,
        update: Option<PhysicalLayerUpdate>,
        view: PhysicalLayerView<'_>,
    ) -> Option<Self> {
        if layout_generation == 0
            || layout_epoch == 0
            || content_generation == 0
            || state.rect != view.rect()
            || update.is_some_and(|update| update.dirty_rect() != state.rect)
        {
            return None;
        }
        let mut pixels =
            Vec::with_capacity(state.rect.width().checked_mul(state.rect.rows() as usize)?);
        for row in 0..state.rect.rows() as usize {
            pixels.extend_from_slice(view.row(state.rect, row)?);
        }
        let pixels: Arc<[Rgb565Pixel]> = pixels.into();
        let backing_key = PhysicalLayerBackingKey {
            role,
            layout_generation,
            layout_epoch,
            content_generation,
            rect: state.rect,
            stride: state.rect.width(),
            pixel_count: pixels.len(),
            source_address: pixels.as_ptr() as usize,
        };
        Some(Self {
            role,
            layout_generation,
            layout_epoch,
            content_generation,
            state,
            update,
            backing_key,
            backing: PhysicalLayerPublicationBacking {
                pixels,
                rect: state.rect,
            },
        })
    }

    pub fn for_frame(
        &self,
        state: PhysicalLayerState,
        update: Option<PhysicalLayerUpdate>,
    ) -> Option<Self> {
        if state.rect != self.state.rect
            || update.is_some_and(|update| update.dirty_rect() != state.rect)
        {
            return None;
        }
        Some(Self {
            state,
            update,
            ..self.clone()
        })
    }

    pub const fn role(&self) -> PhysicalLayerRole {
        self.role
    }

    pub const fn layout_generation(&self) -> u64 {
        self.layout_generation
    }

    pub const fn layout_epoch(&self) -> u64 {
        self.layout_epoch
    }

    pub const fn content_generation(&self) -> u64 {
        self.content_generation
    }

    pub const fn state(&self) -> PhysicalLayerState {
        self.state
    }

    pub const fn update(&self) -> Option<PhysicalLayerUpdate> {
        self.update
    }

    pub const fn backing_key(&self) -> PhysicalLayerBackingKey {
        self.backing_key
    }

    pub fn view(&self) -> PhysicalLayerView<'_> {
        PhysicalLayerView::dense(&self.backing.pixels, self.backing.rect)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalLayerUpdate {
    Full(DirtyRect),
    Scroll {
        delta_x: isize,
        delta_y: isize,
        rect: DirtyRect,
    },
}

impl PhysicalLayerUpdate {
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
pub struct LayerOffset {
    pub x: i64,
    pub y: i64,
}

impl LayerOffset {
    pub const ZERO: Self = Self { x: 0, y: 0 };

    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalLayerState {
    pub rect: DirtyRect,
    pub version: u64,
    pub content_offset: LayerOffset,
}

impl PhysicalLayerState {
    pub fn new(rect: DirtyRect, version: u64) -> Self {
        Self {
            rect,
            version,
            content_offset: LayerOffset::ZERO,
        }
    }

    pub fn with_content_offset(mut self, content_offset: LayerOffset) -> Self {
        self.content_offset = content_offset;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LatchSlotCoherency {
    layers: [Option<PhysicalLayerState>; PhysicalLayerRole::COUNT],
    hardware: LatchSlotHardwareState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatchFramePlan {
    cached_damage: DirtyRectList,
    preview_desired: Option<PhysicalLayerState>,
    preview_dirty: Option<DirtyRect>,
    arcade_desired: Option<PhysicalLayerState>,
    arcade_dirty: Option<PhysicalLayerUpdate>,
    preview_publication: Option<PhysicalLayerPublication>,
    arcade_publication: Option<PhysicalLayerPublication>,
    layer_ownership: [PhysicalLayerOwnership; PhysicalLayerRole::COUNT],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhysicalLayerOwnership {
    Cached,
    Published,
}

impl LatchFramePlan {
    pub fn from_cached_layers(
        cached_damage: DirtyRectList,
        preview_desired: Option<PhysicalLayerState>,
        preview_dirty: Option<DirtyRect>,
        arcade_desired: Option<PhysicalLayerState>,
        arcade_dirty: Option<PhysicalLayerUpdate>,
    ) -> Self {
        Self {
            cached_damage,
            preview_desired,
            preview_dirty,
            arcade_desired,
            arcade_dirty,
            preview_publication: None,
            arcade_publication: None,
            layer_ownership: [PhysicalLayerOwnership::Cached; PhysicalLayerRole::COUNT],
        }
    }

    pub fn from_publications(
        cached_damage: DirtyRectList,
        preview: Option<PhysicalLayerPublication>,
        arcade: Option<PhysicalLayerPublication>,
    ) -> Self {
        debug_assert!(
            preview
                .as_ref()
                .is_none_or(|publication| { publication.role() == PhysicalLayerRole::Preview })
        );
        debug_assert!(
            arcade
                .as_ref()
                .is_none_or(|publication| { publication.role() == PhysicalLayerRole::Arcade })
        );
        Self {
            cached_damage,
            preview_desired: preview.as_ref().map(PhysicalLayerPublication::state),
            preview_dirty: preview
                .as_ref()
                .and_then(PhysicalLayerPublication::update)
                .map(PhysicalLayerUpdate::dirty_rect),
            arcade_desired: arcade.as_ref().map(PhysicalLayerPublication::state),
            arcade_dirty: arcade.as_ref().and_then(PhysicalLayerPublication::update),
            preview_publication: preview,
            arcade_publication: arcade,
            layer_ownership: [PhysicalLayerOwnership::Published; PhysicalLayerRole::COUNT],
        }
    }

    pub fn from_preview_publication_and_cached_arcade(
        cached_damage: DirtyRectList,
        preview: Option<PhysicalLayerPublication>,
        arcade_desired: Option<PhysicalLayerState>,
        arcade_dirty: Option<PhysicalLayerUpdate>,
    ) -> Self {
        debug_assert!(
            preview
                .as_ref()
                .is_none_or(|publication| publication.role() == PhysicalLayerRole::Preview)
        );
        Self {
            cached_damage,
            preview_desired: preview.as_ref().map(PhysicalLayerPublication::state),
            preview_dirty: preview
                .as_ref()
                .and_then(PhysicalLayerPublication::update)
                .map(PhysicalLayerUpdate::dirty_rect),
            arcade_desired,
            arcade_dirty,
            preview_publication: preview,
            arcade_publication: None,
            layer_ownership: [
                PhysicalLayerOwnership::Published,
                PhysicalLayerOwnership::Cached,
            ],
        }
    }

    pub fn cached_damage(&self) -> DirtyRectList {
        self.cached_damage
    }

    pub fn preview_dirty(&self) -> Option<DirtyRect> {
        self.preview_dirty
    }

    pub fn arcade_dirty(&self) -> Option<PhysicalLayerUpdate> {
        self.arcade_dirty
    }

    pub fn for_fb0_recovery(self, full_rect: DirtyRect) -> Self {
        Self {
            cached_damage: DirtyRectList::from_one(full_rect),
            preview_dirty: self.preview_desired.map(|layer| layer.rect),
            arcade_dirty: self
                .arcade_desired
                .map(|layer| PhysicalLayerUpdate::Full(layer.rect)),
            preview_publication: self.preview_publication.as_ref().and_then(|publication| {
                publication.for_frame(
                    publication.state(),
                    Some(PhysicalLayerUpdate::Full(publication.state().rect)),
                )
            }),
            arcade_publication: self.arcade_publication.as_ref().and_then(|publication| {
                publication.for_frame(
                    publication.state(),
                    Some(PhysicalLayerUpdate::Full(publication.state().rect)),
                )
            }),
            ..self
        }
    }

    pub fn publication(&self, role: PhysicalLayerRole) -> Option<&PhysicalLayerPublication> {
        match role {
            PhysicalLayerRole::Preview => self.preview_publication.as_ref(),
            PhysicalLayerRole::Arcade => self.arcade_publication.as_ref(),
        }
    }

    #[cfg(test)]
    fn from_rects(
        cached_damage: Option<DirtyRect>,
        preview_desired: Option<PhysicalLayerState>,
        preview_dirty: Option<DirtyRect>,
        arcade_desired: Option<PhysicalLayerState>,
        arcade_dirty: Option<PhysicalLayerUpdate>,
    ) -> Self {
        let mut cached = DirtyRectList::new();
        cached.push_if_some(cached_damage);
        Self::from_cached_layers(
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
    pub arcade_redraw: Option<PhysicalLayerUpdate>,
    pub arcade_redraw_diff_safe: bool,
    preview_after: Option<PhysicalLayerState>,
    arcade_after: Option<PhysicalLayerState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatchPlanError {
    PublicationMismatch {
        role: PhysicalLayerRole,
    },
    StalePublication {
        role: PhysicalLayerRole,
        latest_layout_epoch: u64,
        latest_content_generation: u64,
        offered_layout_epoch: u64,
        offered_content_generation: u64,
    },
    NoWritableSlot,
}

impl LatchPresentPlan {
    pub const fn preview_state_after(self) -> Option<PhysicalLayerState> {
        self.preview_after
    }

    pub const fn arcade_state_after(self) -> Option<PhysicalLayerState> {
        self.arcade_after
    }
}

#[derive(Clone, Debug)]
pub struct TwoBufferLatchState {
    slots: [LatchSlotCoherency; 2],
    base_damage: TwoSlotDamageLedger,
    next_slot_index: u8,
    planned_publications: [Option<PhysicalLayerPublication>; PhysicalLayerRole::COUNT],
    slot_publications: [[Option<PhysicalLayerPublication>; PhysicalLayerRole::COUNT]; 2],
    latest_publication_generation: [Option<(u64, u64)>; PhysicalLayerRole::COUNT],
}

impl TwoBufferLatchState {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            slots: [
                LatchSlotCoherency {
                    layers: [None; PhysicalLayerRole::COUNT],
                    hardware: LatchSlotHardwareState::Unknown,
                },
                LatchSlotCoherency {
                    layers: [None; PhysicalLayerRole::COUNT],
                    hardware: LatchSlotHardwareState::Unknown,
                },
            ],
            base_damage: TwoSlotDamageLedger::new(width, height),
            next_slot_index: 1,
            planned_publications: std::array::from_fn(|_| None),
            slot_publications: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            latest_publication_generation: [None; 2],
        }
    }

    pub fn invalidate_all(&mut self) {
        self.base_damage.invalidate_all();
        for slot in &mut self.slots {
            slot.layers.fill(None);
            slot.hardware = LatchSlotHardwareState::Unknown;
        }
        self.next_slot_index = 1;
        self.planned_publications.fill(None);
        for publications in &mut self.slot_publications {
            publications.fill(None);
        }
        self.latest_publication_generation.fill(None);
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

    pub fn plan_next(&mut self, input: LatchFramePlan) -> Result<LatchPresentPlan, LatchPlanError> {
        if let Some(role) = first_publication_mismatch(&input) {
            return Err(LatchPlanError::PublicationMismatch { role });
        }
        if let Some(error) = self.stale_publication_error(&input) {
            return Err(error);
        }
        self.base_damage.record_damage(&input.cached_damage);
        let slot_index = self
            .select_writable_slot()
            .ok_or(LatchPlanError::NoWritableSlot)?;
        self.planned_publications[0] = input.preview_publication.clone();
        self.planned_publications[1] = input.arcade_publication.clone();
        for publication in self.planned_publications.iter().flatten() {
            self.latest_publication_generation[physical_layer_role_offset(publication.role())] =
                Some((publication.layout_epoch(), publication.content_generation()));
        }
        Ok(self.plan_for_slot(slot_index, input))
    }

    pub fn mark_post_success(&mut self, plan: LatchPresentPlan) {
        let slot_index = plan.slot_index;
        self.base_damage.mark_presented(slot_index);
        let selected = self.slot_mut(slot_index);
        selected.layers[PhysicalLayerRole::Preview.index()] = plan.preview_after;
        selected.layers[PhysicalLayerRole::Arcade.index()] = plan.arcade_after;
        selected.hardware = LatchSlotHardwareState::Unknown;
        let slot_offset = slot_offset(slot_index);
        self.slot_publications[slot_offset] = std::mem::take(&mut self.planned_publications);
        self.next_slot_index = other_slot(slot_index);
    }

    pub fn mark_attempt_failed(&mut self, slot_index: u8) {
        self.base_damage.mark_attempt_failed(slot_index);
        let slot = self.slot_mut(slot_index);
        slot.layers.fill(None);
        slot.hardware = LatchSlotHardwareState::Unknown;
        self.planned_publications.fill(None);
        self.slot_publications[slot_offset(slot_index)].fill(None);
    }

    pub fn planned_publication(
        &self,
        role: PhysicalLayerRole,
    ) -> Option<&PhysicalLayerPublication> {
        self.planned_publications[physical_layer_role_offset(role)].as_ref()
    }

    fn stale_publication_error(&self, input: &LatchFramePlan) -> Option<LatchPlanError> {
        [
            input.preview_publication.as_ref(),
            input.arcade_publication.as_ref(),
        ]
        .into_iter()
        .flatten()
        .find_map(|publication| {
            let latest =
                self.latest_publication_generation[physical_layer_role_offset(publication.role())]?;
            let offered = (publication.layout_epoch(), publication.content_generation());
            (offered < latest).then_some(LatchPlanError::StalePublication {
                role: publication.role(),
                latest_layout_epoch: latest.0,
                latest_content_generation: latest.1,
                offered_layout_epoch: offered.0,
                offered_content_generation: offered.1,
            })
        })
    }

    #[cfg(test)]
    fn retained_publication_generation(
        &self,
        slot_index: u8,
        role: PhysicalLayerRole,
    ) -> Option<u64> {
        self.slot_publications[slot_offset(slot_index)][physical_layer_role_offset(role)]
            .as_ref()
            .map(PhysicalLayerPublication::content_generation)
    }

    pub fn restore_bytes_for_slot(&self, slot_index: u8) -> usize {
        let slot = self.slot(slot_index);
        let mut bytes = self.base_damage.invalid_bytes(slot_index);
        for layer in slot.layers.iter().flatten() {
            bytes = bytes.saturating_add(rect_bytes(layer.rect));
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

        let restore_preview = direct_layer_needs_restore(
            slot.layers[PhysicalLayerRole::Preview.index()],
            input.preview_desired,
        );
        let restore_arcade = direct_layer_needs_restore(
            slot.layers[PhysicalLayerRole::Arcade.index()],
            input.arcade_desired,
        );
        if restore_preview {
            if let Some(preview) = slot.layers[PhysicalLayerRole::Preview.index()] {
                push_without_covered_rect(&mut restore_rects, preview.rect);
            }
        }
        if restore_arcade {
            if let Some(arcade) = slot.layers[PhysicalLayerRole::Arcade.index()] {
                push_without_covered_rect(&mut restore_rects, arcade.rect);
            }
        }

        let preview_intersects_restore =
            layer_intersects_restore(input.preview_desired, &restore_rects);
        let arcade_intersects_restore =
            layer_intersects_restore(input.arcade_desired, &restore_rects);
        let retained_publications = &self.slot_publications[slot_offset(slot_index)];
        let preview_publication_changed = input.layer_ownership[PhysicalLayerRole::Preview.index()]
            == PhysicalLayerOwnership::Published
            && !same_publication_identity(
                retained_publications[PhysicalLayerRole::Preview.index()].as_ref(),
                input.preview_publication.as_ref(),
            );
        let arcade_publication_changed = input.layer_ownership[PhysicalLayerRole::Arcade.index()]
            == PhysicalLayerOwnership::Published
            && !same_publication_identity(
                retained_publications[PhysicalLayerRole::Arcade.index()].as_ref(),
                input.arcade_publication.as_ref(),
            );
        let arcade_publication_requires_full =
            arcade_publication_changed && input.arcade_dirty.is_none();

        let preview_redraw = direct_layer_redraw_rect(
            slot.layers[PhysicalLayerRole::Preview.index()],
            input.preview_desired,
            input.preview_dirty,
            preview_intersects_restore,
            preview_publication_changed,
        );
        let arcade_redraw = direct_layer_redraw_update(
            slot.layers[PhysicalLayerRole::Arcade.index()],
            input.arcade_desired,
            input.arcade_dirty,
            arcade_intersects_restore,
            arcade_publication_requires_full,
        );
        let arcade_redraw_diff_safe = !arcade_intersects_restore
            && !arcade_publication_requires_full
            && slot.layers[PhysicalLayerRole::Arcade.index()].is_some_and(|current| {
                input.arcade_desired.is_some_and(|desired| {
                    current.rect == desired.rect && current.version == desired.version
                })
            });
        let mut direct_redraws = DirtyRectList::new();
        direct_redraws.push_if_some(preview_redraw);
        direct_redraws.push_if_some(arcade_redraw.as_ref().map(direct_layer_update_rect));
        let restore_rects = subtract_dirty_rects(restore_rects, &direct_redraws);

        LatchPresentPlan {
            slot_index,
            restore_rects,
            preview_redraw,
            arcade_redraw,
            arcade_redraw_diff_safe,
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
    current: Option<PhysicalLayerState>,
    desired: Option<PhysicalLayerState>,
) -> bool {
    let Some(current) = current else {
        return false;
    };
    !matches!(desired, Some(desired)
        if desired.rect == current.rect && desired.version == current.version)
}

fn direct_layer_redraw_rect(
    current: Option<PhysicalLayerState>,
    desired: Option<PhysicalLayerState>,
    dirty: Option<DirtyRect>,
    intersects_restore: bool,
    publication_changed: bool,
) -> Option<DirtyRect> {
    let desired = desired?;
    if current != Some(desired) || intersects_restore || publication_changed {
        Some(desired.rect)
    } else {
        dirty
    }
}

fn direct_layer_redraw_update(
    current: Option<PhysicalLayerState>,
    desired: Option<PhysicalLayerState>,
    dirty: Option<PhysicalLayerUpdate>,
    intersects_restore: bool,
    publication_requires_full: bool,
) -> Option<PhysicalLayerUpdate> {
    let desired = desired?;
    if publication_requires_full {
        return Some(PhysicalLayerUpdate::Full(desired.rect));
    }
    if let Some(current) = current {
        if current.rect != desired.rect || current.version != desired.version {
            Some(PhysicalLayerUpdate::Full(desired.rect))
        } else if current.content_offset != desired.content_offset {
            Some(PhysicalLayerUpdate::Scroll {
                delta_x: desired
                    .content_offset
                    .x
                    .saturating_sub(current.content_offset.x)
                    .clamp(isize::MIN as i64, isize::MAX as i64) as isize,
                delta_y: desired
                    .content_offset
                    .y
                    .saturating_sub(current.content_offset.y)
                    .clamp(isize::MIN as i64, isize::MAX as i64) as isize,
                rect: desired.rect,
            })
        } else if intersects_restore {
            Some(PhysicalLayerUpdate::Full(desired.rect))
        } else {
            dirty
        }
    } else {
        Some(PhysicalLayerUpdate::Full(desired.rect))
    }
}

fn layer_intersects_restore(
    layer: Option<PhysicalLayerState>,
    restore_rects: &DirtyRectList,
) -> bool {
    layer.is_some_and(|layer| {
        restore_rects
            .iter()
            .any(|restore| restore.intersection(layer.rect).is_some())
    })
}

fn direct_layer_update_rect(update: &PhysicalLayerUpdate) -> DirtyRect {
    update.dirty_rect()
}

fn same_publication_identity(
    retained: Option<&PhysicalLayerPublication>,
    desired: Option<&PhysicalLayerPublication>,
) -> bool {
    match (retained, desired) {
        (None, None) => true,
        (Some(retained), Some(desired)) => {
            retained.role() == desired.role()
                && retained.layout_generation() == desired.layout_generation()
                && retained.layout_epoch() == desired.layout_epoch()
                && retained.content_generation() == desired.content_generation()
        }
        _ => false,
    }
}

fn first_publication_mismatch(input: &LatchFramePlan) -> Option<PhysicalLayerRole> {
    if !layer_publication_matches(
        input.layer_ownership[PhysicalLayerRole::Preview.index()],
        input.preview_desired,
        input.preview_dirty.map(PhysicalLayerUpdate::Full),
        input.preview_publication.as_ref(),
        PhysicalLayerRole::Preview,
    ) {
        return Some(PhysicalLayerRole::Preview);
    }
    if !layer_publication_matches(
        input.layer_ownership[PhysicalLayerRole::Arcade.index()],
        input.arcade_desired,
        input.arcade_dirty,
        input.arcade_publication.as_ref(),
        PhysicalLayerRole::Arcade,
    ) {
        return Some(PhysicalLayerRole::Arcade);
    }
    None
}

fn layer_publication_matches(
    ownership: PhysicalLayerOwnership,
    desired: Option<PhysicalLayerState>,
    update: Option<PhysicalLayerUpdate>,
    publication: Option<&PhysicalLayerPublication>,
    role: PhysicalLayerRole,
) -> bool {
    match ownership {
        PhysicalLayerOwnership::Cached => publication.is_none(),
        PhysicalLayerOwnership::Published => {
            publication_matches(desired, update, publication, role)
        }
    }
}

fn publication_matches(
    desired: Option<PhysicalLayerState>,
    update: Option<PhysicalLayerUpdate>,
    publication: Option<&PhysicalLayerPublication>,
    role: PhysicalLayerRole,
) -> bool {
    match (desired, publication) {
        (None, None) => true,
        (Some(desired), Some(publication)) => {
            publication.role() == role
                && publication.state() == desired
                && publication.update() == update
        }
        _ => false,
    }
}

const fn physical_layer_role_offset(role: PhysicalLayerRole) -> usize {
    role.index()
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

    fn layer(rect: DirtyRect, version: u64) -> PhysicalLayerState {
        PhysicalLayerState::new(rect, version)
    }

    fn publication(
        role: PhysicalLayerRole,
        rect: DirtyRect,
        content_generation: u64,
    ) -> PhysicalLayerPublication {
        publication_with_layout(role, rect, 7, 1, content_generation)
    }

    fn publication_with_layout(
        role: PhysicalLayerRole,
        rect: DirtyRect,
        layout_generation: u64,
        layout_epoch: u64,
        content_generation: u64,
    ) -> PhysicalLayerPublication {
        let pixels =
            vec![Rgb565Pixel(content_generation as u16); rect.width() * rect.rows() as usize];
        PhysicalLayerPublication::capture(
            role,
            layout_generation,
            layout_epoch,
            content_generation,
            layer(rect, 1),
            Some(PhysicalLayerUpdate::Full(rect)),
            PhysicalLayerView::dense(&pixels, rect),
        )
        .unwrap()
    }

    fn publication_input(
        preview: Option<PhysicalLayerPublication>,
        arcade: Option<PhysicalLayerPublication>,
    ) -> LatchFramePlan {
        LatchFramePlan::from_publications(DirtyRectList::new(), preview, arcade)
    }

    #[test]
    fn cached_layer_plan_accepts_an_unpublished_arcade_redraw() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let arcade = rect(0, 0, 3, 2);
        let update = PhysicalLayerUpdate::Full(arcade);

        let plan = state
            .plan_next(LatchFramePlan::from_cached_layers(
                DirtyRectList::new(),
                None,
                None,
                Some(layer(arcade, 1)),
                Some(update),
            ))
            .expect("cached Arcade layer plan");

        assert_eq!(plan.arcade_redraw, Some(update));
        assert!(
            state
                .planned_publication(PhysicalLayerRole::Arcade)
                .is_none()
        );
    }

    #[test]
    fn published_preview_and_cached_arcade_share_one_frame_plan() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let preview = rect(1, 0, 4, 2);
        let arcade = rect(0, 1, 3, 3);
        let arcade_update = PhysicalLayerUpdate::Full(arcade);

        let plan = state
            .plan_next(LatchFramePlan::from_preview_publication_and_cached_arcade(
                DirtyRectList::new(),
                Some(publication(PhysicalLayerRole::Preview, preview, 1)),
                Some(layer(arcade, 1)),
                Some(arcade_update),
            ))
            .expect("mixed-ownership frame plan");

        assert_eq!(plan.preview_redraw, Some(preview));
        assert_eq!(plan.arcade_redraw, Some(arcade_update));
        assert!(
            state
                .planned_publication(PhysicalLayerRole::Preview)
                .is_some()
        );
        assert!(
            state
                .planned_publication(PhysicalLayerRole::Arcade)
                .is_none()
        );
    }

    #[test]
    fn published_layer_plan_rejects_an_unpublished_arcade_redraw() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let arcade = rect(0, 0, 3, 2);

        let input = LatchFramePlan {
            cached_damage: DirtyRectList::new(),
            preview_desired: None,
            preview_dirty: None,
            arcade_desired: Some(layer(arcade, 1)),
            arcade_dirty: Some(PhysicalLayerUpdate::Full(arcade)),
            preview_publication: None,
            arcade_publication: None,
            layer_ownership: [PhysicalLayerOwnership::Published; PhysicalLayerRole::COUNT],
        };

        assert_eq!(
            state.plan_next(input),
            Err(LatchPlanError::PublicationMismatch {
                role: PhysicalLayerRole::Arcade,
            })
        );
    }

    fn full() -> DirtyRect {
        rect(0, 0, WIDTH, HEIGHT)
    }

    fn all_writable(state: &mut TwoBufferLatchState) {
        state.sync_hardware(None, 0, false, 0);
    }

    fn input(
        cached_damage: Option<DirtyRect>,
        preview: Option<PhysicalLayerState>,
        preview_dirty: Option<DirtyRect>,
        arcade: Option<PhysicalLayerState>,
        arcade_dirty: Option<PhysicalLayerUpdate>,
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

    fn arcade_update_rect(update: PhysicalLayerUpdate) -> DirtyRect {
        match update {
            PhysicalLayerUpdate::Full(rect) | PhysicalLayerUpdate::Scroll { rect, .. } => rect,
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
    fn publications_remain_owned_until_each_slot_replaces_them() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let preview = rect(0, 0, 2, 2);

        let first = state
            .plan_next(publication_input(
                Some(publication(PhysicalLayerRole::Preview, preview, 1)),
                None,
            ))
            .unwrap();
        state.mark_post_success(first);
        let second = state
            .plan_next(publication_input(
                Some(publication(PhysicalLayerRole::Preview, preview, 2)),
                None,
            ))
            .unwrap();
        state.mark_post_success(second);

        assert_eq!(
            state.retained_publication_generation(1, PhysicalLayerRole::Preview),
            Some(1)
        );
        assert_eq!(
            state.retained_publication_generation(2, PhysicalLayerRole::Preview),
            Some(2)
        );

        let third = state
            .plan_next(publication_input(
                Some(publication(PhysicalLayerRole::Preview, preview, 3)),
                None,
            ))
            .unwrap();
        state.mark_post_success(third);
        assert_eq!(
            state.retained_publication_generation(1, PhysicalLayerRole::Preview),
            Some(3)
        );
        assert_eq!(
            state.retained_publication_generation(2, PhysicalLayerRole::Preview),
            Some(2)
        );
    }

    #[test]
    fn publication_pixels_are_immutable_after_source_reuse() {
        let preview = rect(1, 0, 4, 2);
        let expected = vec![Rgb565Pixel(0x1234); preview.width() * preview.rows() as usize];
        let mut source = expected.clone();
        let publication = PhysicalLayerPublication::capture(
            PhysicalLayerRole::Preview,
            7,
            1,
            3,
            layer(preview, 1),
            Some(PhysicalLayerUpdate::Full(preview)),
            PhysicalLayerView::dense(&source, preview),
        )
        .expect("owned publication");

        source.fill(Rgb565Pixel(0xffff));

        assert_eq!(publication.view().pixels(), expected);
        assert_ne!(
            publication.backing_key().source_address,
            source.as_ptr() as usize
        );
    }

    #[test]
    fn stale_publication_is_rejected_before_slot_planning() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let preview = rect(0, 0, 2, 2);
        let current = state
            .plan_next(publication_input(
                Some(publication(PhysicalLayerRole::Preview, preview, 4)),
                None,
            ))
            .unwrap();
        state.mark_post_success(current);

        assert_eq!(
            state.plan_next(publication_input(
                Some(publication(PhysicalLayerRole::Preview, preview, 3)),
                None
            )),
            Err(LatchPlanError::StalePublication {
                role: PhysicalLayerRole::Preview,
                latest_layout_epoch: 1,
                latest_content_generation: 4,
                offered_layout_epoch: 1,
                offered_content_generation: 3,
            })
        );
    }

    #[test]
    fn newer_layout_epoch_is_not_ordered_by_layout_hash() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let preview = rect(0, 0, 2, 2);
        let first = publication_with_layout(
            PhysicalLayerRole::Preview,
            preview,
            0x95ee_f91e_e662_524f,
            1,
            9,
        );
        let first = state
            .plan_next(publication_input(Some(first), None))
            .unwrap();
        state.mark_post_success(first);

        let rotated = publication_with_layout(
            PhysicalLayerRole::Preview,
            preview,
            0x0d0a_857f_79bb_b25c,
            2,
            1,
        );
        let rotated = state
            .plan_next(publication_input(Some(rotated), None))
            .expect("new epoch must supersede a numerically larger layout hash");
        state.mark_post_success(rotated);

        let old_epoch = publication_with_layout(
            PhysicalLayerRole::Preview,
            preview,
            0xffff_ffff_ffff_ffff,
            1,
            99,
        );
        assert!(matches!(
            state.plan_next(publication_input(Some(old_epoch), None)),
            Err(LatchPlanError::StalePublication {
                offered_layout_epoch: 1,
                latest_layout_epoch: 2,
                ..
            })
        ));
    }

    #[test]
    fn failed_post_discards_selected_slot_publication_only() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let arcade = rect(0, 0, 3, 2);
        for generation in [1, 2] {
            let plan = state
                .plan_next(publication_input(
                    None,
                    Some(publication(PhysicalLayerRole::Arcade, arcade, generation)),
                ))
                .unwrap();
            state.mark_post_success(plan);
        }
        let failed = state
            .plan_next(publication_input(
                None,
                Some(publication(PhysicalLayerRole::Arcade, arcade, 3)),
            ))
            .unwrap();
        state.mark_attempt_failed(failed.slot_index);

        assert_eq!(
            state.retained_publication_generation(1, PhysicalLayerRole::Arcade),
            None
        );
        assert_eq!(
            state.retained_publication_generation(2, PhysicalLayerRole::Arcade),
            Some(2)
        );
    }

    #[test]
    fn layer_retirement_releases_slot_publication() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        all_writable(&mut state);
        let preview = rect(0, 0, 2, 2);
        for generation in [1, 1] {
            let plan = state
                .plan_next(publication_input(
                    Some(publication(PhysicalLayerRole::Preview, preview, generation)),
                    None,
                ))
                .unwrap();
            state.mark_post_success(plan);
        }
        let retired = state.plan_next(publication_input(None, None)).unwrap();
        state.mark_post_success(retired);

        assert_eq!(
            state.retained_publication_generation(1, PhysicalLayerRole::Preview),
            None
        );
        assert_eq!(
            state.retained_publication_generation(2, PhysicalLayerRole::Preview),
            Some(1)
        );
    }

    #[test]
    fn one_frame_arcade_publication_change_reaches_both_slots() {
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);
        let arcade = rect(0, 0, 3, 2);
        let generation_one = publication(PhysicalLayerRole::Arcade, arcade, 1);

        for _ in 0..2 {
            all_writable(&mut state);
            let plan = state
                .plan_next(publication_input(None, Some(generation_one.clone())))
                .expect("seed Arcade publication in both slots");
            assert_eq!(plan.arcade_redraw, Some(PhysicalLayerUpdate::Full(arcade)));
            state.mark_post_success(plan);
        }

        let generation_two = publication(PhysicalLayerRole::Arcade, arcade, 2);
        all_writable(&mut state);
        let changed = state
            .plan_next(publication_input(None, Some(generation_two.clone())))
            .expect("publish changed Arcade content");
        assert_eq!(
            changed.arcade_redraw,
            Some(PhysicalLayerUpdate::Full(arcade))
        );
        state.mark_post_success(changed);

        let unchanged = generation_two
            .for_frame(generation_two.state(), None)
            .expect("quiet publication");
        all_writable(&mut state);
        let catch_up = state
            .plan_next(publication_input(None, Some(unchanged.clone())))
            .expect("catch up alternate hidden slot");
        assert_eq!(
            catch_up.arcade_redraw,
            Some(PhysicalLayerUpdate::Full(arcade))
        );
        assert!(!catch_up.arcade_redraw_diff_safe);
        state.mark_post_success(catch_up);

        all_writable(&mut state);
        let settled = state
            .plan_next(publication_input(None, Some(unchanged)))
            .expect("both hidden slots now contain the publication");
        assert_eq!(settled.arcade_redraw, None);
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
                Some(PhysicalLayerUpdate::Full(arcade)),
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
        assert_eq!(plan.arcade_redraw, Some(PhysicalLayerUpdate::Full(arcade)));
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
                Some(PhysicalLayerUpdate::Full(arcade)),
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
                Some(PhysicalLayerUpdate::Full(arcade)),
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
                Some(PhysicalLayerUpdate::Full(arcade)),
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
                Some(PhysicalLayerUpdate::Full(arcade)),
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
                Some(PhysicalLayerUpdate::Full(arcade)),
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
                Some(PhysicalLayerUpdate::Full(arcade)),
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
                Some(PhysicalLayerUpdate::Full(arcade)),
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
                Some(PhysicalLayerUpdate::Scroll {
                    delta_x: 0,
                    delta_y: -1,
                    rect: arcade,
                }),
            ))
            .expect("changed plan");

        assert_eq!(changed.preview_redraw, Some(preview));
        assert_eq!(
            changed.arcade_redraw,
            Some(PhysicalLayerUpdate::Full(arcade))
        );
        assert!(!changed.arcade_redraw_diff_safe);
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
    fn same_arcade_identity_allows_slot_local_content_diff() {
        let arcade = rect(0, 0, 4, 3);
        let arcade_layer = layer(arcade, 7);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);

        all_writable(&mut state);
        let first = state
            .plan_next(input(
                Some(full()),
                None,
                None,
                Some(arcade_layer),
                Some(PhysicalLayerUpdate::Full(arcade)),
            ))
            .expect("first plan");
        state.mark_post_success(first);

        all_writable(&mut state);
        let second = state
            .plan_next(input(
                None,
                None,
                None,
                Some(arcade_layer),
                Some(PhysicalLayerUpdate::Full(arcade)),
            ))
            .expect("second plan");
        state.mark_post_success(second);

        all_writable(&mut state);
        let content_update = state
            .plan_next(input(
                None,
                None,
                None,
                Some(arcade_layer),
                Some(PhysicalLayerUpdate::Full(arcade)),
            ))
            .expect("content update");
        assert_eq!(
            content_update.arcade_redraw,
            Some(PhysicalLayerUpdate::Full(arcade))
        );
        assert!(content_update.arcade_redraw_diff_safe);
    }

    #[test]
    fn matching_arcade_generation_accumulates_scroll_for_older_slot() {
        let arcade = rect(0, 0, 4, 3);
        let current = layer(arcade, 7).with_content_offset(LayerOffset::new(0, -3));
        let desired = layer(arcade, 7).with_content_offset(LayerOffset::new(0, -11));

        assert!(!direct_layer_needs_restore(Some(current), Some(desired)));

        assert_eq!(
            direct_layer_redraw_update(current.into(), desired.into(), None, true, false),
            Some(PhysicalLayerUpdate::Scroll {
                delta_x: 0,
                delta_y: -8,
                rect: arcade,
            })
        );
    }

    #[test]
    fn physical_layer_offset_emits_both_scroll_axes() {
        let arcade = rect(0, 0, 4, 3);
        let current = layer(arcade, 7).with_content_offset(LayerOffset::new(-4, 6));
        let desired = layer(arcade, 7).with_content_offset(LayerOffset::new(9, -2));

        assert_eq!(
            direct_layer_redraw_update(current.into(), desired.into(), None, false, false),
            Some(PhysicalLayerUpdate::Scroll {
                delta_x: 13,
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
    fn published_layer_reversal_restores_both_slots_after_full_damage_and_failure() {
        let preview = rect(1, 0, 3, 2);
        let mut state = TwoBufferLatchState::new(WIDTH, HEIGHT);

        for generation in 1..=2 {
            all_writable(&mut state);
            let plan = state
                .plan_next(publication_input(
                    Some(publication(PhysicalLayerRole::Preview, preview, generation)),
                    None,
                ))
                .expect("populate hidden slot");
            state.mark_post_success(plan);
        }

        state.invalidate_all();
        all_writable(&mut state);
        let failed = state
            .plan_next(LatchFramePlan::from_publications(
                DirtyRectList::from_one(full()),
                Some(publication(PhysicalLayerRole::Preview, preview, 3)),
                None,
            ))
            .expect("full-damage reapply");
        assert_eq!(failed.preview_redraw, Some(preview));
        state.mark_attempt_failed(failed.slot_index);

        all_writable(&mut state);
        let retired_failed_slot = state
            .plan_next(LatchFramePlan::from_publications(
                DirtyRectList::new(),
                None,
                None,
            ))
            .expect("retire failed slot");
        assert!(
            retired_failed_slot
                .restore_rects
                .iter()
                .any(|rect| rect == full())
        );
        assert_eq!(retired_failed_slot.preview_state_after(), None);
        state.mark_post_success(retired_failed_slot);

        all_writable(&mut state);
        let retired_other_slot = state
            .plan_next(LatchFramePlan::from_publications(
                DirtyRectList::new(),
                None,
                None,
            ))
            .expect("retire other slot");
        assert!(
            retired_other_slot
                .restore_rects
                .iter()
                .any(|rect| rect == preview)
        );
        assert_eq!(retired_other_slot.preview_state_after(), None);
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
            Some(PhysicalLayerUpdate::Full(arcade))
        );
    }
}
