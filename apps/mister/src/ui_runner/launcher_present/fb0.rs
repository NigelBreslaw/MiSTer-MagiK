// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::super::*;

pub(in crate::ui_runner) struct Fb0DirtyPresentRequest<'a> {
    pub(in crate::ui_runner) frame_plan: LauncherFramePlan,
    pub(in crate::ui_runner) cached_frame: CachedFrameView<'a>,
    pub(in crate::ui_runner) direct_preview: Option<PhysicalLayerView<'a>>,
    pub(in crate::ui_runner) fb0: &'a mut MappedRgb565Framebuffer,
    pub(in crate::ui_runner) arcade_list_renderer: &'a mut ArcadeListRenderer,
}

pub(in crate::ui_runner) struct Fb0DirtyPresentStats {
    pub(in crate::ui_runner) copied_rows: u32,
    pub(in crate::ui_runner) direct_preview_rows: u32,
    pub(in crate::ui_runner) present_bytes: usize,
    pub(in crate::ui_runner) cached_present_us: u128,
    pub(in crate::ui_runner) direct_preview_present_us: u128,
    pub(in crate::ui_runner) arcade_list_present_us: u128,
    pub(in crate::ui_runner) arcade_update_label: ArcadeUpdateTrace,
}

pub(in crate::ui_runner) struct Fb0DirtyPresenter;

pub(in crate::ui_runner) trait Fb0DirtyCopySink {
    fn copy_cached(&mut self, view: CachedFrameView<'_>, rect: DirtyRect) -> u32;
    fn copy_physical_layer(&mut self, view: PhysicalLayerView<'_>, rect: DirtyRect) -> u32;
    fn copy_arcade_list(&mut self, update: ArcadeListUpdate) -> PresentCopyStats;
}

struct LiveFb0DirtyCopySink<'a> {
    fb0: &'a mut MappedRgb565Framebuffer,
    arcade_list_renderer: &'a mut ArcadeListRenderer,
}

impl Fb0DirtyCopySink for LiveFb0DirtyCopySink<'_> {
    fn copy_cached(&mut self, view: CachedFrameView<'_>, rect: DirtyRect) -> u32 {
        copy_cached_rect_565(self.fb0, view, rect).map_or(0, DirtyRect::rows)
    }

    fn copy_physical_layer(&mut self, view: PhysicalLayerView<'_>, rect: DirtyRect) -> u32 {
        copy_physical_layer_rect_565(self.fb0, view, rect)
    }

    fn copy_arcade_list(&mut self, update: ArcadeListUpdate) -> PresentCopyStats {
        copy_arcade_list_update(self.fb0, self.arcade_list_renderer, update)
    }
}

impl Fb0DirtyPresenter {
    pub(in crate::ui_runner) fn present(
        request: Fb0DirtyPresentRequest<'_>,
    ) -> Fb0DirtyPresentStats {
        let mut sink = LiveFb0DirtyCopySink {
            fb0: request.fb0,
            arcade_list_renderer: request.arcade_list_renderer,
        };
        Self::present_to(
            request.frame_plan,
            request.cached_frame,
            request.direct_preview,
            &mut sink,
        )
    }

    pub(in crate::ui_runner) fn present_to(
        frame_plan: LauncherFramePlan,
        cached_frame: CachedFrameView<'_>,
        direct_preview: Option<PhysicalLayerView<'_>>,
        sink: &mut impl Fb0DirtyCopySink,
    ) -> Fb0DirtyPresentStats {
        let cached_damage = frame_plan.cached_damage();
        let direct_preview_rect = frame_plan.preview_dirty();
        let arcade_list_update = frame_plan.arcade_dirty();
        let arcade_update_label = ArcadeUpdateTrace::from_update(arcade_list_update.as_ref());
        let arcade_overlay_rect = arcade_list_update.as_ref().map(arcade_update_dirty_rect);
        let cached_present_rects =
            Self::cached_present_plan(cached_damage, direct_preview_rect, arcade_overlay_rect);

        let mut copied_rows = 0u32;
        let mut present_bytes = 0usize;
        let mut cached_present_us = 0u128;
        for rect in cached_present_rects.iter() {
            let copy_start = Instant::now();
            let rows = sink.copy_cached(cached_frame, rect);
            copied_rows += rows;
            if rows != 0 {
                present_bytes += present_bytes_for_rows(rect.width(), rows);
            }
            cached_present_us += copy_start.elapsed().as_micros();
        }

        let mut direct_preview_rows = 0u32;
        let mut direct_preview_present_us = 0u128;
        if let (Some(view), Some(rect)) = (direct_preview, direct_preview_rect) {
            let copy_start = Instant::now();
            direct_preview_rows = sink.copy_physical_layer(view, rect);
            direct_preview_present_us = copy_start.elapsed().as_micros();
            copied_rows += direct_preview_rows;
            if direct_preview_rows != 0 {
                present_bytes += present_bytes_for_rows(rect.width(), direct_preview_rows);
            }
        }

        let mut arcade_list_present_us = 0u128;
        if let Some(update) = arcade_list_update {
            let copy_start = Instant::now();
            let stats = sink.copy_arcade_list(update);
            copied_rows += stats.rows;
            present_bytes += stats.bytes;
            arcade_list_present_us = copy_start.elapsed().as_micros();
        }

        Fb0DirtyPresentStats {
            copied_rows,
            direct_preview_rows,
            present_bytes,
            cached_present_us,
            direct_preview_present_us,
            arcade_list_present_us,
            arcade_update_label,
        }
    }

    fn cached_present_plan(
        cached_damage: DirtyRectList,
        direct_preview_rect: Option<DirtyRect>,
        arcade_overlay_rect: Option<DirtyRect>,
    ) -> DirtyRectList {
        let mut direct_overlays = DirtyRectList::new();
        direct_overlays.push_if_some(direct_preview_rect);
        direct_overlays.push_if_some(arcade_overlay_rect);
        build_launcher_present_plan_from_layers(&cached_damage, &direct_overlays)
    }
}

fn present_bytes_for_rows(width: usize, rows: u32) -> usize {
    width
        .saturating_mul(rows as usize)
        .saturating_mul(mister_magik_fb::framebuffer::format::RGB565_BYTES_PER_PIXEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeCopySink {
        events: Vec<&'static str>,
        cached_rows: u32,
        preview_rows: u32,
        arcade_stats: PresentCopyStats,
    }

    impl Fb0DirtyCopySink for FakeCopySink {
        fn copy_cached(&mut self, _view: CachedFrameView<'_>, _rect: DirtyRect) -> u32 {
            self.events.push("cached");
            self.cached_rows
        }

        fn copy_physical_layer(&mut self, _view: PhysicalLayerView<'_>, _rect: DirtyRect) -> u32 {
            self.events.push("preview");
            self.preview_rows
        }

        fn copy_arcade_list(&mut self, _update: ArcadeListUpdate) -> PresentCopyStats {
            self.events.push("arcade");
            self.arcade_stats
        }
    }

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    fn present_plan_for_test(
        full_frame_present: bool,
        slint_dirty: Option<DirtyRect>,
        raw_preview_rect: Option<DirtyRect>,
        raw_preview_direct_rect: Option<DirtyRect>,
        arcade_overlay_rect: Option<DirtyRect>,
    ) -> Vec<DirtyRect> {
        let mut cached_damage = DirtyRectList::new();
        cached_damage.push_if_some(if full_frame_present {
            Some(rect(0, 0, 960, 540))
        } else {
            slint_dirty
        });
        cached_damage.push_if_some(raw_preview_rect);
        let frame_plan = LauncherFramePlan::new(
            cached_damage,
            None,
            raw_preview_direct_rect,
            None,
            arcade_overlay_rect.map(ArcadeListUpdate::Full),
        );
        Fb0DirtyPresenter::cached_present_plan(
            frame_plan.cached_damage(),
            frame_plan.preview_dirty(),
            frame_plan
                .arcade_dirty()
                .as_ref()
                .map(arcade_update_dirty_rect),
        )
        .iter()
        .collect()
    }

    #[test]
    fn plan_keeps_cached_overlays_and_excludes_direct_overlay() {
        let plan = present_plan_for_test(
            true,
            None,
            Some(rect(600, 120, 920, 360)),
            None,
            Some(rect(48, 54, 432, 486)),
        );

        assert!(plan.contains(&rect(600, 120, 920, 360)));
        assert!(
            !plan
                .iter()
                .any(|candidate| candidate.intersection(rect(48, 54, 432, 486)).is_some())
        );
    }

    #[test]
    fn plan_uses_slint_dirty_when_not_full_frame() {
        let plan = present_plan_for_test(
            false,
            Some(rect(100, 100, 200, 200)),
            Some(rect(600, 120, 920, 360)),
            None,
            None,
        );

        assert_eq!(
            plan,
            vec![rect(100, 100, 200, 200), rect(600, 120, 920, 360)]
        );
    }

    #[test]
    fn plan_excludes_direct_preview_overlay_from_full_frame_base() {
        let preview = rect(600, 120, 920, 360);
        let plan = present_plan_for_test(true, None, None, Some(preview), None);

        assert!(
            !plan
                .iter()
                .any(|candidate| candidate.intersection(preview).is_some())
        );
    }

    #[test]
    fn presenter_copies_cached_then_preview_then_arcade_and_accounts_results() {
        let base = rect(0, 0, 10, 10);
        let preview_rect = rect(2, 2, 4, 4);
        let arcade_rect = rect(6, 6, 8, 8);
        let cached_pixels = vec![Rgb565Pixel(0); 100];
        let cached_frame = CachedFrameView::new(&cached_pixels, 10, 10);
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(10, 10));
        target.direct_preview_565_rect_mut(preview_rect);
        let direct_preview = target.direct_preview_view();
        let frame_plan = LauncherFramePlan::new(
            DirtyRectList::from_one(base),
            None,
            Some(preview_rect),
            None,
            Some(ArcadeListUpdate::Full(arcade_rect)),
        );
        let cached_plan = Fb0DirtyPresenter::cached_present_plan(
            frame_plan.cached_damage(),
            frame_plan.preview_dirty(),
            Some(arcade_rect),
        );
        let expected_cached_bytes = cached_plan
            .iter()
            .map(|rect| present_bytes_for_rows(rect.width(), 1))
            .sum::<usize>();
        let cached_copy_count = cached_plan.len();
        let mut sink = FakeCopySink {
            events: Vec::new(),
            cached_rows: 1,
            preview_rows: 2,
            arcade_stats: PresentCopyStats { rows: 3, bytes: 99 },
        };

        let stats =
            Fb0DirtyPresenter::present_to(frame_plan, cached_frame, direct_preview, &mut sink);

        let mut expected_events = vec!["cached"; cached_copy_count];
        expected_events.extend(["preview", "arcade"]);
        assert_eq!(sink.events, expected_events);
        assert_eq!(stats.copied_rows, cached_copy_count as u32 + 5);
        assert_eq!(stats.direct_preview_rows, 2);
        assert_eq!(
            stats.present_bytes,
            expected_cached_bytes + present_bytes_for_rows(preview_rect.width(), 2) + 99
        );
        assert_eq!(stats.arcade_update_label.to_string(), "full");
    }

    #[test]
    fn presenter_does_not_account_failed_cached_or_preview_copies() {
        let base = rect(0, 0, 10, 10);
        let preview_rect = rect(2, 2, 4, 4);
        let cached_pixels = vec![Rgb565Pixel(0); 100];
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(10, 10));
        target.direct_preview_565_rect_mut(preview_rect);
        let frame_plan = LauncherFramePlan::new(
            DirtyRectList::from_one(base),
            None,
            Some(preview_rect),
            None,
            None,
        );
        let mut sink = FakeCopySink {
            events: Vec::new(),
            cached_rows: 0,
            preview_rows: 0,
            arcade_stats: PresentCopyStats::default(),
        };

        let stats = Fb0DirtyPresenter::present_to(
            frame_plan,
            CachedFrameView::new(&cached_pixels, 10, 10),
            target.direct_preview_view(),
            &mut sink,
        );

        assert_eq!(sink.events.last(), Some(&"preview"));
        assert_eq!(stats.copied_rows, 0);
        assert_eq!(stats.direct_preview_rows, 0);
        assert_eq!(stats.present_bytes, 0);
    }
}
