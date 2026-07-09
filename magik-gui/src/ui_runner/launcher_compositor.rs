use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LauncherPresentBackend {
    None,
    Fb0Dirty,
    FpgaVblankLatchHidden,
}

impl LauncherPresentBackend {
    pub(super) const fn trace_label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fb0Dirty => "fb0-dirty",
            Self::FpgaVblankLatchHidden => "fpga-vblank-latch-hidden",
        }
    }

    pub(super) const fn is_latch(self) -> bool {
        matches!(self, Self::FpgaVblankLatchHidden)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LauncherPresentStatus {
    None,
    Ok,
    Unsupported,
}

impl LauncherPresentStatus {
    pub(super) const fn trace_label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ok => "ok",
            Self::Unsupported => "unsupported",
        }
    }
}

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
        full_frame_present: bool,
    ) -> (Option<RawPreviewPresent>, PreviewTransitionTrace) {
        blit_raw_preview_if_needed(
            self.target,
            self.ui,
            preview,
            transition,
            elapsed,
            slint_dirty,
            full_frame_present,
        )
    }

    fn present_cached_rect(&mut self, rect: DirtyRect) -> u32 {
        self.target
            .present_rect(self.disp, frame_target_geometry(self.ui), rect)
    }

    fn present_direct_preview_rect(&mut self, rect: DirtyRect) -> u32 {
        self.target.present_direct_preview_rect(self.disp, rect)
    }

    pub(super) fn compose_direct_preview_rect(&mut self, rect: DirtyRect) -> u32 {
        self.target.compose_direct_preview_rect(rect)
    }

    pub(super) fn copy_direct_preview_rect_to_hidden(
        &self,
        hidden: &mut PluginHiddenRgb565Framebuffer,
        rect: DirtyRect,
    ) -> u32 {
        self.target.copy_direct_preview_rect_to_hidden(hidden, rect)
    }

    fn present_arcade_list_update(
        &mut self,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
    ) -> PresentCopyStats {
        copy_arcade_list_update(self.target, self.disp, renderer, update)
    }

    pub(super) fn compose_arcade_list_update(
        &mut self,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
    ) -> PresentCopyStats {
        compose_arcade_list_update(self.target, renderer, update)
    }

    pub(super) fn copy_arcade_list_update_to_hidden(
        &self,
        hidden: &mut PluginHiddenRgb565Framebuffer,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
    ) -> PresentCopyStats {
        copy_arcade_list_update_to_hidden(hidden, renderer, update)
    }

    pub(super) fn cached_565(&self) -> &[Rgb565Pixel] {
        self.target.cached_565()
    }
}

pub(super) struct LauncherPresentRequest<'a, 'b> {
    pub(super) layer_target: &'a mut LayerTarget<'b>,
    pub(super) frame_plan: LauncherFramePlan,
    pub(super) arcade_list_renderer: &'a mut ArcadeListRenderer,
}

pub(super) struct LauncherPresentResult {
    pub(super) copied_rows: u32,
    pub(super) direct_preview_rows: u32,
    pub(super) present_bytes: usize,
    pub(super) wasted_present_bytes: usize,
    pub(super) fb_present_us_override: Option<u128>,
    pub(super) vsync_us_override: Option<u128>,
    pub(super) cached_present_us: u128,
    pub(super) hidden_compose_us: u128,
    pub(super) hidden_preview_compose_us: u128,
    pub(super) hidden_arcade_compose_us: u128,
    pub(super) direct_preview_present_us: u128,
    pub(super) arcade_list_present_us: u128,
    pub(super) main_present_backend: LauncherPresentBackend,
    pub(super) main_present_status: LauncherPresentStatus,
    pub(super) main_present_buffer: u8,
    pub(super) main_present_hidden_copy_us: u128,
    pub(super) main_present_hidden_invalid_bytes: usize,
    pub(super) main_present_hidden_rect_count: u32,
    pub(super) main_present_hidden_catchup_bytes: usize,
    pub(super) main_present_hidden_full_copy: bool,
    pub(super) main_present_request_us: u128,
    pub(super) main_present_set_vga_fb_us: u128,
    pub(super) main_present_wait_us: u64,
    pub(super) main_present_route_us: u64,
    pub(super) arcade_update_label: ArcadeUpdateTrace,
}

pub(super) struct LauncherCompositor;

impl LauncherCompositor {
    pub(super) fn present(request: LauncherPresentRequest<'_, '_>) -> LauncherPresentResult {
        let cached_damage = request.frame_plan.cached_damage();
        let raw_preview_direct_rect = request.frame_plan.preview_dirty();
        let arcade_list_update = request.frame_plan.arcade_dirty();
        let arcade_update_label = ArcadeUpdateTrace::from_update(arcade_list_update.as_ref());
        let arcade_overlay_rect = arcade_list_update.as_ref().map(arcade_update_dirty_rect);
        let cached_present_rects =
            Self::cached_present_plan(cached_damage, raw_preview_direct_rect, arcade_overlay_rect);

        let mut copied_rows = 0u32;
        let mut present_bytes = 0usize;
        let mut cached_present_us = 0u128;
        for rect in cached_present_rects.iter() {
            let copy_start = Instant::now();
            copied_rows += request.layer_target.present_cached_rect(rect);
            let bytes = present_bytes_for_rect(rect);
            present_bytes += bytes;
            cached_present_us += copy_start.elapsed().as_micros();
        }

        let mut direct_preview_rows = 0u32;
        let mut direct_preview_present_us = 0u128;
        if let Some(rect) = raw_preview_direct_rect {
            let copy_start = Instant::now();
            direct_preview_rows = request.layer_target.present_direct_preview_rect(rect);
            direct_preview_present_us = copy_start.elapsed().as_micros();
            copied_rows += direct_preview_rows;
            present_bytes += present_bytes_for_rect(rect);
        }

        let mut arcade_list_present_us = 0u128;
        if let Some(update) = arcade_list_update {
            let copy_start = Instant::now();
            let stats = request
                .layer_target
                .present_arcade_list_update(request.arcade_list_renderer, update);
            copied_rows += stats.rows;
            present_bytes += stats.bytes;
            arcade_list_present_us = copy_start.elapsed().as_micros();
        }

        LauncherPresentResult {
            copied_rows,
            direct_preview_rows,
            present_bytes,
            wasted_present_bytes: 0,
            fb_present_us_override: None,
            vsync_us_override: None,
            cached_present_us,
            hidden_compose_us: 0,
            hidden_preview_compose_us: 0,
            hidden_arcade_compose_us: 0,
            direct_preview_present_us,
            arcade_list_present_us,
            main_present_backend: LauncherPresentBackend::Fb0Dirty,
            main_present_status: LauncherPresentStatus::None,
            main_present_buffer: 0,
            main_present_hidden_copy_us: 0,
            main_present_hidden_invalid_bytes: 0,
            main_present_hidden_rect_count: 0,
            main_present_hidden_catchup_bytes: 0,
            main_present_hidden_full_copy: false,
            main_present_request_us: 0,
            main_present_set_vga_fb_us: 0,
            main_present_wait_us: 0,
            main_present_route_us: 0,
            arcade_update_label,
        }
    }

    fn cached_present_plan(
        cached_damage: DirtyRectList,
        raw_preview_direct_rect: Option<DirtyRect>,
        arcade_overlay_rect: Option<DirtyRect>,
    ) -> DirtyRectList {
        let mut direct_overlays = DirtyRectList::new();
        direct_overlays.push_if_some(raw_preview_direct_rect);
        direct_overlays.push_if_some(arcade_overlay_rect);
        build_launcher_present_plan_from_layers(&cached_damage, &direct_overlays)
    }
}

fn present_bytes_for_rect(rect: DirtyRect) -> usize {
    rect.width()
        .saturating_mul(rect.rows() as usize)
        .saturating_mul(mister_magik_fb::framebuffer::format::RGB565_BYTES_PER_PIXEL)
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
        LauncherCompositor::cached_present_plan(
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
    fn compositor_plan_keeps_cached_overlays_and_excludes_direct_overlay() {
        let plan = present_plan_for_test(
            true,
            None,
            Some(rect(600, 120, 920, 360)),
            None,
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
            None,
        );

        assert_eq!(
            plan,
            vec![rect(100, 100, 200, 200), rect(600, 120, 920, 360)]
        );
    }

    #[test]
    fn compositor_plan_excludes_direct_preview_overlay_from_full_frame_base() {
        let preview = rect(600, 120, 920, 360);
        let plan = present_plan_for_test(true, None, None, Some(preview), None);

        assert!(!plan
            .iter()
            .any(|candidate| candidate.intersection(preview).is_some()));
    }
}
