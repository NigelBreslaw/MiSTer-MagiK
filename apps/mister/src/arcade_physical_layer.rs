// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::arcade_list_renderer::{ArcadeListGeometry, PersistentArcadeLayerStyle};
use crate::framebuffer::target::{DirtyRect, PhysicalLayerBacking, PhysicalLayerView};
use mister_magik_framebuffer_scenes::{Rgb565OutputLayout, Rgb565Rect, Rgb565RegionLayout};
use slint::platform::software_renderer::Rgb565Pixel;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistentOrientedArcadeLayerKey {
    pub geometry: ArcadeListGeometry,
    pub visible_height: usize,
    pub output: Rgb565OutputLayout,
    pub style: PersistentArcadeLayerStyle,
    pub catalog_generation: u64,
    pub ring_origin: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PersistentArcadeLayerDiagnostic {
    pub key: Option<PersistentOrientedArcadeLayerKey>,
    pub rect: Option<DirtyRect>,
    pub stride: usize,
    pub pixels: usize,
    pub allocated_bytes: usize,
}

/// Dense physical Arcade content owned independently from scanout slots.
///
/// The backing contains only normal, non-inverted list pixels. Selection
/// fill/text/frame remain a separate aperture so shifting never treats
/// inverted pixels as ordinary content.
pub struct PersistentOrientedArcadeLayer {
    backing: Option<PhysicalLayerBacking>,
    region_layout: Option<Rgb565RegionLayout>,
    key: Option<PersistentOrientedArcadeLayerKey>,
    selection_aperture: Option<DirtyRect>,
    full_rebuild: bool,
}

impl Default for PersistentOrientedArcadeLayer {
    fn default() -> Self {
        Self {
            backing: None,
            region_layout: None,
            key: None,
            selection_aperture: None,
            full_rebuild: true,
        }
    }
}

impl PersistentOrientedArcadeLayer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ensure(
        &mut self,
        geometry: ArcadeListGeometry,
        visible_height: usize,
        output: Rgb565OutputLayout,
        style: PersistentArcadeLayerStyle,
        catalog_generation: u64,
        ring_origin: usize,
    ) -> bool {
        let key = PersistentOrientedArcadeLayerKey {
            geometry,
            visible_height,
            output,
            style,
            catalog_generation,
            ring_origin,
        };
        let region_layout = Rgb565RegionLayout::new(
            output,
            Rgb565Rect {
                x0: geometry.x,
                y0: geometry.y,
                x1: geometry.x + geometry.width,
                y1: geometry.y + visible_height,
            },
        )
        .expect("Arcade layer geometry is within the launcher output");
        let physical = region_layout.physical_rect();
        let physical_rect = DirtyRect {
            x0: physical.x0,
            y0: physical.y0,
            x1: physical.x1,
            y1: physical.y1,
        };
        let changed = self.key.map_or(true, |current| {
            current.geometry != key.geometry
                || current.visible_height != key.visible_height
                || current.output != key.output
                || current.style != key.style
                || current.catalog_generation != key.catalog_generation
        }) || self.backing.as_ref().map(PhysicalLayerBacking::rect)
            != Some(physical_rect)
            || self.backing.as_ref().map(|backing| backing.pixels().len())
                != Some(region_layout.len());
        if changed {
            self.backing = PhysicalLayerBacking::new(physical_rect, Rgb565Pixel(0));
            self.selection_aperture = None;
            self.full_rebuild = true;
        }
        self.region_layout = Some(region_layout);
        self.key = Some(key);
        changed
    }

    pub fn invalidate(&mut self) {
        self.full_rebuild = true;
        self.selection_aperture = None;
    }

    pub fn key(&self) -> Option<PersistentOrientedArcadeLayerKey> {
        self.key
    }

    pub fn content(&self) -> &[Rgb565Pixel] {
        self.backing
            .as_ref()
            .map(PhysicalLayerBacking::pixels)
            .unwrap_or_default()
    }

    pub fn content_mut(&mut self) -> &mut [Rgb565Pixel] {
        self.backing_mut().pixels_mut()
    }

    pub(crate) fn backing_mut(&mut self) -> &mut PhysicalLayerBacking {
        self.backing
            .as_mut()
            .expect("physical Arcade layer is initialized")
    }

    pub fn region_layout(&self) -> Option<Rgb565RegionLayout> {
        self.region_layout
    }

    pub fn allocated_bytes(&self) -> usize {
        self.backing
            .as_ref()
            .map(PhysicalLayerBacking::allocated_bytes)
            .unwrap_or(0)
    }

    pub fn physical_rect(&self) -> Option<DirtyRect> {
        self.backing.as_ref().map(PhysicalLayerBacking::rect)
    }

    pub fn view(&self) -> Option<PhysicalLayerView<'_>> {
        self.backing.as_ref().map(PhysicalLayerBacking::view)
    }

    pub fn take_backing(&mut self) -> Option<PhysicalLayerBacking> {
        self.backing.take()
    }

    pub fn restore_backing(&mut self, backing: PhysicalLayerBacking) -> bool {
        if self
            .region_layout
            .is_none_or(|layout| layout.len() != backing.pixels().len())
            || self.region_layout.is_some_and(|layout| {
                let rect = layout.physical_rect();
                backing.rect()
                    != DirtyRect {
                        x0: rect.x0,
                        y0: rect.y0,
                        x1: rect.x1,
                        y1: rect.y1,
                    }
            })
        {
            return false;
        }
        self.backing = Some(backing);
        true
    }

    pub fn needs_full_rebuild(&self) -> bool {
        self.full_rebuild
    }

    pub fn mark_full_rebuild_complete(&mut self) {
        self.full_rebuild = false;
    }

    pub fn selection_aperture(&self) -> Option<DirtyRect> {
        self.selection_aperture
    }

    pub fn set_selection_aperture(
        &mut self,
        selection_y: usize,
        selection_height: usize,
    ) -> Option<DirtyRect> {
        let key = self.key?;
        let physical = key.output.logical_rect_to_physical(Rgb565Rect {
            x0: key.geometry.x,
            y0: key.geometry.y.saturating_add(selection_y),
            x1: key.geometry.x.saturating_add(key.geometry.width),
            y1: key
                .geometry
                .y
                .saturating_add(selection_y.saturating_add(selection_height)),
        });
        let aperture = DirtyRect {
            x0: physical.x0,
            y0: physical.y0,
            x1: physical.x1,
            y1: physical.y1,
        };
        self.selection_aperture = Some(aperture);
        Some(aperture)
    }

    pub fn diagnostic(&self) -> PersistentArcadeLayerDiagnostic {
        PersistentArcadeLayerDiagnostic {
            key: self.key,
            rect: self.physical_rect(),
            stride: self
                .backing
                .as_ref()
                .map(PhysicalLayerBacking::stride)
                .unwrap_or(0),
            pixels: self.content().len(),
            allocated_bytes: self.allocated_bytes(),
        }
    }
}
