// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_screensaver::ScreensaverRenderTrace;
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
    Frozen,
}

impl LauncherPresentStatus {
    pub(super) const fn trace_label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ok => "ok",
            Self::Unsupported => "unsupported",
            Self::Frozen => "frozen",
        }
    }
}

pub(super) struct LayerTarget<'a> {
    target: &'a mut UiFrameTarget,
    logical_target: Option<&'a mut UiFrameTarget>,
    ui: &'a UiDisplay,
    layout: UiLayoutGeometry,
    drawing_ui: UiDisplay,
}

impl<'a> LayerTarget<'a> {
    pub(super) fn new(target: &'a mut UiFrameTarget, ui: &'a UiDisplay) -> Self {
        Self {
            target,
            logical_target: None,
            ui,
            layout: UiLayoutGeometry::for_display(ui, ScreenOrientation::Normal),
            drawing_ui: UiDisplay::for_framebuffer(ui.render_w(), ui.render_h()),
        }
    }

    pub(super) fn new_oriented(
        target: &'a mut UiFrameTarget,
        logical_target: Option<&'a mut UiFrameTarget>,
        ui: &'a UiDisplay,
        layout: UiLayoutGeometry,
    ) -> Self {
        debug_assert_eq!(layout.is_portrait(), logical_target.is_some());
        Self {
            target,
            logical_target,
            ui,
            layout,
            drawing_ui: UiDisplay::for_framebuffer(layout.logical_w(), layout.logical_h()),
        }
    }

    fn drawing_target(&self) -> &UiFrameTarget {
        self.logical_target.as_deref().unwrap_or(self.target)
    }

    fn drawing_target_mut(&mut self) -> &mut UiFrameTarget {
        self.logical_target.as_deref_mut().unwrap_or(self.target)
    }

    pub(super) fn render_slint_base(
        &mut self,
        window: &MisterSoftwareWindow,
    ) -> (Option<DirtyRect>, DirtyRectList) {
        let mut slint_dirty = None;
        let mut slint_damage = DirtyRectList::new();
        window.draw_if_needed(|renderer| {
            let region = self.target.render(renderer);
            slint_dirty = dirty_rect(
                &region,
                self.layout.composition_w(),
                self.layout.composition_h(),
            );
            slint_damage = dirty_rects(
                &region,
                self.layout.composition_w(),
                self.layout.composition_h(),
            );
        });
        (slint_dirty, slint_damage)
    }

    pub(super) fn render_slint_full(
        &mut self,
        window: &MisterSoftwareWindow,
    ) -> (Option<DirtyRect>, DirtyRectList, bool) {
        let mut slint_dirty = None;
        let mut slint_damage = DirtyRectList::new();
        let rendered = window.draw_full_frame_if_needed(|renderer| {
            let region = self.target.render(renderer);
            slint_dirty = dirty_rect(
                &region,
                self.layout.composition_w(),
                self.layout.composition_h(),
            );
            slint_damage = dirty_rects(
                &region,
                self.layout.composition_w(),
                self.layout.composition_h(),
            );
        });
        (slint_dirty, slint_damage, rendered)
    }

    pub(super) fn render_black(&mut self) -> DirtyRect {
        self.drawing_target_mut()
            .cached_565_mut()
            .fill(Rgb565Pixel(0));
        DirtyRect {
            x0: 0,
            y0: 0,
            x1: self.layout.logical_w(),
            y1: self.layout.logical_h(),
        }
    }

    pub(super) fn clear_cached_preview(&mut self) -> DirtyRect {
        let rect = preview_screen_rect(&self.drawing_ui);
        let stride = self.layout.logical_w();
        let cached = self.drawing_target_mut().cached_565_mut();
        for y in rect.y0..rect.y1 {
            let row = y * stride;
            cached[row + rect.x0..row + rect.x1].fill(Rgb565Pixel(0));
        }
        rect
    }

    pub(super) fn render_screensaver(
        &mut self,
        saver: &mut LauncherScreensaver,
    ) -> (DirtyRect, ScreensaverRenderTrace) {
        let width = self.layout.logical_w();
        let height = self.layout.logical_h();
        let trace = saver.render(self.drawing_target_mut().cached_565_mut(), width, height);
        (
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: width,
                y1: height,
            },
            trace,
        )
    }

    pub(super) fn render_screensaver_fade(
        &mut self,
        launcher_frame: &[Rgb565Pixel],
        alpha: u8,
    ) -> DirtyRect {
        let cached = self.drawing_target_mut().cached_565_mut();
        if cached.len() == launcher_frame.len() {
            let black = Rgb565Pixel(0);
            for (pixel, source) in cached.iter_mut().zip(launcher_frame) {
                *pixel = blend_565(*source, black, alpha);
            }
        } else {
            cached.fill(Rgb565Pixel(0));
        }
        DirtyRect {
            x0: 0,
            y0: 0,
            x1: self.layout.logical_w(),
            y1: self.layout.logical_h(),
        }
    }

    pub(super) fn render_screensaver_crossfade(
        &mut self,
        saver: &mut LauncherScreensaver,
        launcher_frame: &[Rgb565Pixel],
        alpha: u8,
    ) -> (DirtyRect, ScreensaverRenderTrace) {
        let width = self.layout.logical_w();
        let height = self.layout.logical_h();
        let trace = saver.render(self.drawing_target_mut().cached_565_mut(), width, height);
        let cached = self.drawing_target_mut().cached_565_mut();
        if cached.len() == launcher_frame.len() {
            for (pixel, source) in cached.iter_mut().zip(launcher_frame) {
                *pixel = blend_565(*source, *pixel, alpha);
            }
        }
        (
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: width,
                y1: height,
            },
            trace,
        )
    }

    pub(super) fn snapshot_cached(&self) -> Vec<Rgb565Pixel> {
        snapshot_cached_565(self.drawing_target())
    }

    pub(super) fn restore_cached(&mut self, snapshot: &[Rgb565Pixel]) -> bool {
        restore_cached_565(self.drawing_target_mut(), snapshot)
    }

    pub(super) fn swap_cached(&mut self, replacement: &mut Vec<Rgb565Pixel>) -> bool {
        let width = self.layout.logical_w();
        self.drawing_target_mut()
            .swap_cached_565(replacement, width)
    }

    pub(super) fn swap_presentation_cached(&mut self, replacement: &mut Vec<Rgb565Pixel>) -> bool {
        self.target
            .swap_cached_565(replacement, self.layout.composition_w())
    }

    pub(super) fn blend_screensaver_crossfade(
        &mut self,
        launcher_frame: &[Rgb565Pixel],
        alpha: u8,
    ) -> DirtyRect {
        let cached = self.target.cached_565_mut();
        if cached.len() == launcher_frame.len() {
            for (pixel, source) in cached.iter_mut().zip(launcher_frame) {
                *pixel = blend_565(*source, *pixel, alpha);
            }
        }
        DirtyRect {
            x0: 0,
            y0: 0,
            x1: self.layout.composition_w(),
            y1: self.layout.composition_h(),
        }
    }

    pub(super) fn blit_raw_preview_if_needed(
        &mut self,
        preview: &mut PreviewState,
        transition: &mut PreviewTransitionDemo,
        elapsed: Duration,
        slint_dirty: Option<DirtyRect>,
        full_frame_present: bool,
    ) -> (Option<RawPreviewPresent>, PreviewTransitionTrace) {
        let drawing_ui = &self.drawing_ui;
        let allow_direct = !self.layout.is_portrait();
        let target = self.logical_target.as_deref_mut().unwrap_or(self.target);
        blit_raw_preview_if_needed(
            target,
            drawing_ui,
            preview,
            transition,
            elapsed,
            slint_dirty,
            full_frame_present,
            allow_direct,
        )
    }

    pub(super) fn compose_exact_preview(
        &mut self,
        preview: &PreviewState,
    ) -> Option<RawPreviewPresent> {
        let frame = preview.raw_frame()?;
        if frame.status() != PreviewRawFrameStatus::Ready {
            return None;
        }
        if preview_direct_present_enabled() && !self.layout.is_portrait() {
            self.target
                .blit_raw_preview_direct(self.ui, &frame, true)
                .map(RawPreviewPresent::Direct)
        } else {
            self.logical_target
                .as_deref_mut()
                .unwrap_or(self.target)
                .blit_raw_preview(&self.drawing_ui, &frame, true)
                .map(RawPreviewPresent::Cached)
        }
    }

    pub(super) fn compose_direct_preview_rect(&mut self, rect: DirtyRect) -> u32 {
        self.target.compose_direct_preview_rect(rect)
    }

    pub(super) fn copy_direct_preview_rect_to_hidden(
        &self,
        hidden: &mut ScanoutSlotsRgb565Framebuffer,
        rect: DirtyRect,
    ) -> u32 {
        self.target
            .direct_preview_view()
            .map(|view| copy_direct_preview_rect_to_hidden(hidden, view, rect))
            .unwrap_or(0)
    }

    pub(super) fn compose_arcade_list_update(
        &mut self,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
    ) -> PresentCopyStats {
        compose_arcade_list_update(self.drawing_target_mut(), renderer, update)
    }

    pub(super) fn copy_arcade_list_update_to_hidden(
        &self,
        hidden: &mut ScanoutSlotsRgb565Framebuffer,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
    ) -> PresentCopyStats {
        copy_arcade_list_update_to_hidden(hidden, renderer, update)
    }

    pub(super) fn cached_frame_view(&self) -> CachedFrameView<'_> {
        self.drawing_target().cached_frame_view()
    }

    pub(super) fn presentation_frame_view(&self) -> CachedFrameView<'_> {
        self.target.cached_frame_view()
    }

    pub(super) fn presentation_pixels_mut(&mut self) -> &mut [Rgb565Pixel] {
        self.target.cached_565_mut()
    }

    pub(super) fn direct_preview_view(&self) -> Option<DirectPreviewView<'_>> {
        if self.layout.is_portrait() {
            None
        } else {
            self.target.direct_preview_view()
        }
    }

    pub(super) fn rotate_damage_to_composition(
        &mut self,
        logical_damage: &DirtyRectList,
    ) -> DirtyRectList {
        if !self.layout.is_portrait() {
            return *logical_damage;
        }
        let logical = self
            .logical_target
            .as_deref()
            .expect("portrait logical target")
            .cached_565();
        let composition = self.target.cached_565_mut();
        let mut mapped = DirtyRectList::new();
        for rect in logical_damage.iter() {
            let rect = self.layout.logical_rect_to_composition(rect);
            mapped.push(rect);
            for composition_y in rect.y0..rect.y1 {
                let row = composition_y * self.layout.composition_w();
                for composition_x in rect.x0..rect.x1 {
                    let (logical_x, logical_y) = self
                        .layout
                        .composition_pixel_to_logical(composition_x, composition_y);
                    composition[row + composition_x] =
                        logical[logical_y * self.layout.logical_w() + logical_x];
                }
            }
        }
        mapped
    }
}

fn snapshot_cached_565(target: &UiFrameTarget) -> Vec<Rgb565Pixel> {
    target.cached_565().to_vec()
}

fn restore_cached_565(target: &mut UiFrameTarget, snapshot: &[Rgb565Pixel]) -> bool {
    let cached = target.cached_565_mut();
    if cached.len() != snapshot.len() {
        return false;
    }
    cached.copy_from_slice(snapshot);
    true
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
    pub(super) main_present_hidden_publish_us: u128,
    pub(super) main_present_hidden_invalid_bytes: usize,
    pub(super) main_present_hidden_rect_count: u32,
    pub(super) main_present_hidden_catchup_bytes: usize,
    pub(super) main_present_hidden_full_copy: bool,
    pub(super) main_present_copy_path: &'static str,
    pub(super) main_present_request_us: u128,
    pub(super) main_present_set_vga_fb_us: u128,
    pub(super) main_present_wait_us: u64,
    pub(super) main_present_sequence: u16,
    pub(super) main_present_post_active_sequence: u16,
    pub(super) main_present_post_pending_sequence: u16,
    pub(super) main_present_post_pending: bool,
    pub(super) main_present_flip_count: u16,
    pub(super) main_present_drop_count: u16,
    pub(super) arcade_update_label: ArcadeUpdateTrace,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_cached_preview_blacks_only_the_dynamic_preview_rect() {
        let ui = UiDisplay::for_framebuffer(960, 540);
        let green = Rgb565Pixel(0x07e0);
        let mut target =
            UiFrameTarget::cached(FramebufferTargetGeometry::new(ui.render_w(), ui.render_h()));
        target.cached_565_mut().fill(green);

        let mut layer_target = LayerTarget::new(&mut target, &ui);
        let rect = layer_target.clear_cached_preview();

        assert_eq!(rect, preview_screen_rect(&ui));
        let inside = rect.y0 * ui.render_w() + rect.x0;
        assert_eq!(target.cached_565()[inside], Rgb565Pixel(0));
        assert_eq!(target.cached_565()[0], green);
    }

    #[test]
    fn screensaver_frame_overwrite_can_restore_launcher_cache_exactly() {
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(4, 3));
        let launcher_frame = (0..12)
            .map(|value| Rgb565Pixel(0x1000 + value))
            .collect::<Vec<_>>();
        target.cached_565_mut().copy_from_slice(&launcher_frame);

        let snapshot = snapshot_cached_565(&target);
        target.cached_565_mut().fill(Rgb565Pixel(0x0001));

        assert!(restore_cached_565(&mut target, &snapshot));
        assert_eq!(target.cached_565(), launcher_frame);
    }

    #[test]
    fn activation_black_overwrites_the_complete_cached_frame() {
        let ui = UiDisplay::for_framebuffer(4, 3);
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(4, 3));
        target.cached_565_mut().fill(Rgb565Pixel(0xffff));

        let dirty = LayerTarget::new(&mut target, &ui).render_black();

        assert_eq!(
            dirty,
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: 4,
                y1: 3,
            }
        );
        assert!(
            target
                .cached_565()
                .iter()
                .all(|pixel| *pixel == Rgb565Pixel(0))
        );
    }

    #[test]
    fn normal_damage_returns_without_transforming_the_cached_frame() {
        let ui = UiDisplay::for_framebuffer(4, 3);
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(4, 3));
        let original = (0..12)
            .map(|value| Rgb565Pixel(0x1000 + value))
            .collect::<Vec<_>>();
        target.cached_565_mut().copy_from_slice(&original);
        let damage = DirtyRectList::from_one(DirtyRect {
            x0: 1,
            y0: 1,
            x1: 3,
            y1: 2,
        });

        let mapped = LayerTarget::new(&mut target, &ui).rotate_damage_to_composition(&damage);

        assert_eq!(mapped, damage);
        assert_eq!(target.cached_565(), original);
    }

    #[test]
    fn portrait_idle_damage_performs_no_rotation_work() {
        let ui = UiDisplay::for_framebuffer(4, 3);
        let layout = UiLayoutGeometry::for_display(&ui, ScreenOrientation::MonitorClockwise);
        let mut composition = UiFrameTarget::cached(FramebufferTargetGeometry::new(4, 3));
        composition.cached_565_mut().fill(Rgb565Pixel(0x1234));
        let mut logical = UiFrameTarget::cached(FramebufferTargetGeometry::new(3, 4));
        logical.cached_565_mut().fill(Rgb565Pixel(0xabcd));

        let mapped = LayerTarget::new_oriented(&mut composition, Some(&mut logical), &ui, layout)
            .rotate_damage_to_composition(&DirtyRectList::new());

        assert!(mapped.is_empty());
        assert!(
            composition
                .cached_565()
                .iter()
                .all(|pixel| *pixel == Rgb565Pixel(0x1234))
        );
    }

    #[test]
    fn portrait_rotation_updates_only_mapped_damage() {
        let ui = UiDisplay::for_framebuffer(4, 3);
        for orientation in [
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
        ] {
            let layout = UiLayoutGeometry::for_display(&ui, orientation);
            let mut composition = UiFrameTarget::cached(FramebufferTargetGeometry::new(4, 3));
            let untouched = Rgb565Pixel(0x1234);
            composition.cached_565_mut().fill(untouched);
            let mut logical = UiFrameTarget::cached(FramebufferTargetGeometry::new(3, 4));
            for (index, pixel) in logical.cached_565_mut().iter_mut().enumerate() {
                *pixel = Rgb565Pixel(0x2000 + index as u16);
            }
            let logical_damage = DirtyRectList::from_one(DirtyRect {
                x0: 1,
                y0: 1,
                x1: 3,
                y1: 3,
            });
            let expected = match orientation {
                ScreenOrientation::MonitorClockwise => DirtyRect {
                    x0: 1,
                    y0: 0,
                    x1: 3,
                    y1: 2,
                },
                ScreenOrientation::MonitorCounterclockwise => DirtyRect {
                    x0: 1,
                    y0: 1,
                    x1: 3,
                    y1: 3,
                },
                ScreenOrientation::Normal => unreachable!(),
            };

            let mapped =
                LayerTarget::new_oriented(&mut composition, Some(&mut logical), &ui, layout)
                    .rotate_damage_to_composition(&logical_damage);

            assert_eq!(mapped, DirtyRectList::from_one(expected));
            for composition_y in 0..3 {
                for composition_x in 0..4 {
                    let actual = composition.cached_565()[composition_y * 4 + composition_x];
                    if composition_x >= expected.x0
                        && composition_x < expected.x1
                        && composition_y >= expected.y0
                        && composition_y < expected.y1
                    {
                        let (logical_x, logical_y) =
                            layout.composition_pixel_to_logical(composition_x, composition_y);
                        assert_eq!(actual, logical.cached_565()[logical_y * 3 + logical_x]);
                    } else {
                        assert_eq!(actual, untouched);
                    }
                }
            }
        }
    }
}
