// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Retained planning state for incremental CRT Arcade foreground composition.

use mister_magik_framebuffer_scenes::{Rgb565OutputLayout, Rgb565Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrtArcadeOverlayKey {
    pub backdrop_revision: u64,
    pub layout: Rgb565OutputLayout,
    pub viewport: Rgb565Rect,
    pub style_revision: u64,
    pub catalog_generation: u64,
    pub ring_origin: usize,
    pub selection: Rgb565Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrtArcadeOverlayUpdate {
    Full,
    Scroll { delta_y: isize },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrtArcadeOverlayPlan {
    pub full_rebuild: bool,
    pub exposed_stripes: Vec<Rgb565Rect>,
    pub stale_glyph_spans: Vec<Rgb565Rect>,
    pub selection_union: Option<Rgb565Rect>,
}

#[derive(Clone, Debug, Default)]
pub struct CrtArcadeOverlayState {
    key: Option<CrtArcadeOverlayKey>,
    foreground_spans: Vec<Rgb565Rect>,
    invalidated: bool,
}

impl CrtArcadeOverlayState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn key(&self) -> Option<CrtArcadeOverlayKey> {
        self.key
    }

    pub fn invalidate(&mut self) {
        self.invalidated = true;
    }

    pub fn clear(&mut self) {
        self.key = None;
        self.foreground_spans.clear();
        self.invalidated = false;
    }

    pub fn plan(
        &self,
        next: CrtArcadeOverlayKey,
        update: CrtArcadeOverlayUpdate,
    ) -> CrtArcadeOverlayPlan {
        let Some(current) = self.key else {
            return CrtArcadeOverlayPlan {
                full_rebuild: true,
                ..CrtArcadeOverlayPlan::default()
            };
        };
        let CrtArcadeOverlayUpdate::Scroll { delta_y } = update else {
            return CrtArcadeOverlayPlan {
                full_rebuild: true,
                ..CrtArcadeOverlayPlan::default()
            };
        };
        let viewport_height = next.viewport.y1.saturating_sub(next.viewport.y0);
        if self.invalidated
            || current.backdrop_revision != next.backdrop_revision
            || current.layout != next.layout
            || current.viewport != next.viewport
            || current.style_revision != next.style_revision
            || current.catalog_generation != next.catalog_generation
            || delta_y == 0
            || delta_y.unsigned_abs() >= viewport_height
        {
            return CrtArcadeOverlayPlan {
                full_rebuild: true,
                ..CrtArcadeOverlayPlan::default()
            };
        }

        let exposed_height = delta_y.unsigned_abs();
        let exposed_y = if delta_y < 0 {
            next.viewport.y1.saturating_sub(exposed_height)
        } else {
            next.viewport.y0
        };
        CrtArcadeOverlayPlan {
            full_rebuild: false,
            exposed_stripes: vec![Rgb565Rect {
                x0: next.viewport.x0,
                y0: exposed_y,
                x1: next.viewport.x1,
                y1: exposed_y.saturating_add(exposed_height),
            }],
            stale_glyph_spans: self.foreground_spans.clone(),
            selection_union: Some(union_rect(current.selection, next.selection)),
        }
    }

    pub fn commit(&mut self, key: CrtArcadeOverlayKey, foreground_spans: &[Rgb565Rect]) {
        self.key = Some(key);
        self.foreground_spans.clear();
        self.foreground_spans.extend_from_slice(foreground_spans);
        self.invalidated = false;
    }
}

fn union_rect(a: Rgb565Rect, b: Rgb565Rect) -> Rgb565Rect {
    Rgb565Rect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_framebuffer_scenes::OutputRotation;

    fn key(backdrop_revision: u64, ring_origin: usize, selection_y: usize) -> CrtArcadeOverlayKey {
        CrtArcadeOverlayKey {
            backdrop_revision,
            layout: Rgb565OutputLayout::new(320, 240, 320, OutputRotation::None).unwrap(),
            viewport: Rgb565Rect {
                x0: 8,
                y0: 40,
                x1: 312,
                y1: 220,
            },
            style_revision: 4,
            catalog_generation: 9,
            ring_origin,
            selection: Rgb565Rect {
                x0: 8,
                y0: selection_y,
                x1: 312,
                y1: selection_y + 12,
            },
        }
    }

    #[test]
    fn scroll_plan_tracks_exposed_stale_and_selection_damage() {
        let mut state = CrtArcadeOverlayState::new();
        let first = key(3, 170, 96);
        let stale = [Rgb565Rect {
            x0: 20,
            y0: 60,
            x1: 42,
            y1: 61,
        }];
        state.commit(first, &stale);

        let next = key(3, 2, 108);
        let plan = state.plan(next, CrtArcadeOverlayUpdate::Scroll { delta_y: -12 });

        assert!(!plan.full_rebuild);
        assert_eq!(plan.stale_glyph_spans, stale);
        assert_eq!(
            plan.exposed_stripes,
            [Rgb565Rect {
                x0: 8,
                y0: 208,
                x1: 312,
                y1: 220,
            }]
        );
        assert_eq!(
            plan.selection_union,
            Some(Rgb565Rect {
                x0: 8,
                y0: 96,
                x1: 312,
                y1: 120,
            })
        );
    }

    #[test]
    fn identity_changes_and_invalidation_force_full_rebuilds() {
        let mut state = CrtArcadeOverlayState::new();
        let first = key(3, 0, 96);
        state.commit(first, &[]);

        assert!(
            state
                .plan(
                    key(4, 12, 108),
                    CrtArcadeOverlayUpdate::Scroll { delta_y: -12 }
                )
                .full_rebuild
        );
        state.invalidate();
        assert!(
            state
                .plan(
                    key(3, 12, 108),
                    CrtArcadeOverlayUpdate::Scroll { delta_y: -12 }
                )
                .full_rebuild
        );
        state.clear();
        assert_eq!(state.key(), None);
    }
}
