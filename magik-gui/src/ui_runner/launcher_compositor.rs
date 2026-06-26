use super::*;

pub(super) struct LayerTarget<'a> {
    target: &'a mut UiFrameTarget,
    disp: &'a mut MappedRgb565Framebuffer,
    ui: &'a UiDisplay,
}

impl<'a> LayerTarget<'a> {
    pub(super) fn new(
        target: &'a mut UiFrameTarget,
        disp: &'a mut MappedRgb565Framebuffer,
        ui: &'a UiDisplay,
    ) -> Self {
        Self { target, disp, ui }
    }

    pub(super) fn render_slint_base(
        &mut self,
        window: &MinimalSoftwareWindow,
    ) -> Option<DirtyRect> {
        let mut slint_dirty = None;
        window.draw_if_needed(|renderer| {
            let region = self.target.render(renderer, frame_target_geometry(self.ui));
            slint_dirty = dirty_rect(&region, self.ui.render_w(), self.ui.render_h());
        });
        slint_dirty
    }

    pub(super) fn blit_raw_preview_if_needed(
        &mut self,
        preview: &mut PreviewState,
        transition: &mut PreviewTransitionDemo,
        elapsed: Duration,
        slint_dirty: Option<DirtyRect>,
    ) -> (Option<DirtyRect>, PreviewTransitionTrace) {
        blit_raw_preview_if_needed(
            self.target,
            self.ui,
            preview,
            transition,
            elapsed,
            slint_dirty,
        )
    }

    fn full_rect(&self) -> DirtyRect {
        DirtyRect {
            x0: 0,
            y0: 0,
            x1: self.ui.render_w(),
            y1: self.ui.render_h(),
        }
    }

    fn present_cached_rect(&mut self, rect: DirtyRect) -> u32 {
        self.target
            .present_rect(self.disp, frame_target_geometry(self.ui), rect)
    }

    fn present_arcade_list_update(
        &mut self,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
    ) -> u32 {
        copy_arcade_list_update(self.target, self.disp, renderer, update)
    }
}

pub(super) struct LauncherPresentRequest<'a, 'b> {
    pub(super) layer_target: &'a mut LayerTarget<'b>,
    pub(super) full_frame_present: bool,
    pub(super) slint_dirty: Option<DirtyRect>,
    pub(super) raw_preview_rect: Option<DirtyRect>,
    pub(super) arcade_list_rect: Option<ArcadeListUpdate>,
    pub(super) arcade_list_renderer: &'a mut ArcadeListRenderer,
}

pub(super) struct LauncherPresentResult {
    pub(super) copied_rows: u32,
    pub(super) cached_present_us: u128,
    pub(super) arcade_list_present_us: u128,
    pub(super) arcade_update_label: ArcadeUpdateTrace,
}

pub(super) struct LauncherCompositor;

impl LauncherCompositor {
    pub(super) fn present(request: LauncherPresentRequest<'_, '_>) -> LauncherPresentResult {
        let arcade_update_label = ArcadeUpdateTrace::from_update(request.arcade_list_rect.as_ref());
        let arcade_overlay_rect = request
            .arcade_list_rect
            .as_ref()
            .map(arcade_update_dirty_rect);
        let cached_present_rects = Self::cached_present_plan(
            request.layer_target.full_rect(),
            request.full_frame_present,
            request.slint_dirty,
            request.raw_preview_rect,
            arcade_overlay_rect,
        );

        let mut copied_rows = 0u32;
        let mut cached_present_us = 0u128;
        for rect in cached_present_rects.iter() {
            let copy_start = Instant::now();
            copied_rows += request.layer_target.present_cached_rect(rect);
            cached_present_us += copy_start.elapsed().as_micros();
        }

        let mut arcade_list_present_us = 0u128;
        if let Some(update) = request.arcade_list_rect {
            let copy_start = Instant::now();
            copied_rows += request
                .layer_target
                .present_arcade_list_update(request.arcade_list_renderer, update);
            arcade_list_present_us = copy_start.elapsed().as_micros();
        }

        LauncherPresentResult {
            copied_rows,
            cached_present_us,
            arcade_list_present_us,
            arcade_update_label,
        }
    }

    fn cached_present_plan(
        full_rect: DirtyRect,
        full_frame_present: bool,
        slint_dirty: Option<DirtyRect>,
        raw_preview_rect: Option<DirtyRect>,
        arcade_overlay_rect: Option<DirtyRect>,
    ) -> DirtyRectList {
        let cached_base_rect = if full_frame_present {
            Some(full_rect)
        } else {
            slint_dirty
        };
        let mut cached_overlays = DirtyRectList::new();
        cached_overlays.push_if_some(raw_preview_rect);
        let mut direct_overlays = DirtyRectList::new();
        direct_overlays.push_if_some(arcade_overlay_rect);
        build_launcher_present_plan(cached_base_rect, &cached_overlays, &direct_overlays)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    fn present_plan_for_test(
        full_frame_present: bool,
        slint_dirty: Option<DirtyRect>,
        raw_preview_rect: Option<DirtyRect>,
        arcade_overlay_rect: Option<DirtyRect>,
    ) -> Vec<DirtyRect> {
        let full_rect = if full_frame_present {
            rect(0, 0, 960, 540)
        } else {
            rect(0, 0, 0, 0)
        };
        LauncherCompositor::cached_present_plan(
            full_rect,
            full_frame_present,
            slint_dirty,
            raw_preview_rect,
            arcade_overlay_rect,
        )
        .iter()
        .collect()
    }

    #[test]
    fn compositor_plan_keeps_cached_overlays_and_excludes_direct_overlay() {
        let plan = present_plan_for_test(
            true,
            None,
            Some(rect(600, 120, 920, 360)),
            Some(rect(48, 54, 432, 486)),
        );

        assert!(plan.contains(&rect(600, 120, 920, 360)));
        assert!(!plan
            .iter()
            .any(|candidate| candidate.intersection(rect(48, 54, 432, 486)).is_some()));
    }

    #[test]
    fn compositor_plan_uses_slint_dirty_when_not_full_frame() {
        let plan = present_plan_for_test(
            false,
            Some(rect(100, 100, 200, 200)),
            Some(rect(600, 120, 920, 360)),
            None,
        );

        assert_eq!(
            plan,
            vec![rect(100, 100, 200, 200), rect(600, 120, 920, 360)]
        );
    }
}
