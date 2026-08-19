// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_screensaver::ScreensaverRenderTrace;
use super::*;
use mister_magik_framebuffer_scenes::OutputRotation;

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
    layout: UiLayoutGeometry,
    layout_epoch: u64,
    drawing_ui: UiDisplay,
}

fn oriented_preview_cache_token(
    presentation_generation: u64,
    transition_id: u64,
    trace: PreviewTransitionTrace,
) -> u64 {
    let mut token = presentation_generation
        .rotate_left(17)
        .wrapping_add(transition_id.rotate_right(11));
    for byte in trace.effect.label().bytes() {
        token = token.rotate_left(5) ^ u64::from(byte);
    }
    if trace.active {
        token = token
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(u64::from(trace.fade.alpha_bucket));
    } else {
        token ^= 0xa5a5_5a5a_3c3c_c3c3;
    }
    token
}

impl<'a> LayerTarget<'a> {
    pub(super) fn new(target: &'a mut UiFrameTarget, ui: &'a UiDisplay) -> Self {
        Self {
            target,
            layout: UiLayoutGeometry::for_display(ui, ScreenOrientation::Normal),
            layout_epoch: 1,
            drawing_ui: UiDisplay::for_framebuffer(ui.render_w(), ui.render_h()),
        }
    }

    pub(super) fn new_oriented(target: &'a mut UiFrameTarget, layout: UiLayoutGeometry) -> Self {
        Self {
            target,
            layout,
            layout_epoch: 1,
            drawing_ui: UiDisplay::for_framebuffer(layout.logical_w(), layout.logical_h()),
        }
    }

    pub(super) fn new_oriented_with_epoch(
        target: &'a mut UiFrameTarget,
        layout: UiLayoutGeometry,
        layout_epoch: u64,
    ) -> Self {
        debug_assert_ne!(layout_epoch, 0);
        Self {
            target,
            layout,
            layout_epoch,
            drawing_ui: UiDisplay::for_framebuffer(layout.logical_w(), layout.logical_h()),
        }
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
        self.target.cached_565_mut().fill(Rgb565Pixel(0));
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
        let cached = self.target.cached_565_mut();
        for y in rect.y0..rect.y1 {
            let row = y * stride;
            cached[row + rect.x0..row + rect.x1].fill(Rgb565Pixel(0));
        }
        rect
    }

    pub(super) fn clear_presentation_preview(&mut self) -> DirtyRect {
        if !self.layout.is_portrait() {
            return self.clear_cached_preview();
        }
        let rect = self
            .layout
            .logical_rect_to_composition(preview_screen_rect(&self.drawing_ui));
        let stride = self.layout.composition_w();
        let cached = self.target.cached_565_mut();
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
        let trace = saver.render(self.target.cached_565_mut(), width, height);
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
        let cached = self.target.cached_565_mut();
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
        let trace = saver.render(self.target.cached_565_mut(), width, height);
        let cached = self.target.cached_565_mut();
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
        snapshot_cached_565(self.target)
    }

    pub(super) fn restore_cached(&mut self, snapshot: &[Rgb565Pixel]) -> bool {
        restore_cached_565(self.target, snapshot)
    }

    pub(super) fn restore_presentation_cached(&mut self, snapshot: &[Rgb565Pixel]) -> bool {
        restore_cached_565(self.target, snapshot)
    }

    pub(super) fn swap_cached(&mut self, replacement: &mut Vec<Rgb565Pixel>) -> bool {
        let width = self.layout.logical_w();
        self.target.swap_cached_565(replacement, width)
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
        worker: Option<&mut PreviewCompositor>,
    ) -> (
        Option<RawPreviewPresent>,
        PreviewTransitionTrace,
        bool,
        Option<PreviewCompositorTelemetry>,
    ) {
        let drawing_ui = &self.drawing_ui;
        let raw_dirty_before = preview.raw_dirty();
        let slint_touched_preview = full_frame_present
            || slint_dirty.is_some_and(|rect| {
                rect.intersection(super::raw565_preview_renderer::preview_screen_rect(
                    drawing_ui,
                ))
                .is_some()
            });
        if let Some(worker) = worker.filter(|worker| worker.available()) {
            let raw_dirty = preview.take_raw_dirty();
            let snapshot = preview.owned_raw_transition_frame();
            let borrowed = snapshot.as_ref().map(|frame| frame.borrowed());
            let mut trace = transition.update(borrowed.as_ref(), elapsed);
            let Some(snapshot) = snapshot else {
                return (
                    None,
                    trace,
                    false,
                    Some(worker.telemetry(preview.presentation_generation())),
                );
            };
            let token = oriented_preview_cache_token(
                preview.presentation_generation(),
                snapshot.transition_id,
                trace,
            );
            let key = PreviewCompositionWorkKey {
                layout: self.layout.output_layout(),
                generation: preview.presentation_generation(),
                token,
            };
            if let Some(mut result) = worker.take_current(key) {
                trace.fade = result.fade;
                let adopted = match key.layout.rotation() {
                    OutputRotation::None => self
                        .target
                        .adopt_direct_preview(&mut result.pixels, result.rect),
                    OutputRotation::Clockwise90 | OutputRotation::CounterClockwise90 => {
                        self.target.adopt_physical_direct_preview(
                            &mut result.pixels,
                            result.rect,
                            key.layout,
                            key.token,
                        )
                    }
                };
                worker.recycle(result.pixels);
                if adopted {
                    return (
                        Some(RawPreviewPresent::Direct(result.rect)),
                        trace,
                        false,
                        Some(worker.telemetry(key.generation)),
                    );
                }
                worker.note_adoption_failed(key);
                let queued = worker.queue(PreviewCompositionRequest::new(
                    key,
                    snapshot,
                    trace.effect,
                    trace.progress,
                    trace.active,
                ));
                return (None, trace, queued, Some(worker.telemetry(key.generation)));
            }
            let needs_work = raw_dirty
                || slint_touched_preview
                || trace.active
                || preview.presentation_requires_present()
                || worker.needs_retry(key);
            if needs_work {
                worker.queue(PreviewCompositionRequest::new(
                    key,
                    snapshot,
                    trace.effect,
                    trace.progress,
                    trace.active,
                ));
            }
            return (
                None,
                trace,
                needs_work,
                Some(worker.telemetry(key.generation)),
            );
        }
        let (present, trace) = blit_raw_preview_if_needed(
            self.target,
            drawing_ui,
            preview,
            transition,
            elapsed,
            slint_dirty,
            full_frame_present,
            self.layout.is_portrait() || preview_direct_present_enabled(),
        );
        if self.layout.is_portrait()
            && let Some(RawPreviewPresent::Direct(rect)) = present
        {
            let transition_id = preview
                .raw_transition_frame()
                .map(|frame| frame.transition_id)
                .unwrap_or(0);
            let token = oriented_preview_cache_token(
                preview.presentation_generation(),
                transition_id,
                trace,
            );
            let rotation_pmu =
                mister_magik_perf_events::sampled_span("gui.custom.preview-rotation");
            let physical_rect = self.target.compose_direct_preview_to_physical(
                rect,
                self.layout.output_layout(),
                token,
                raw_dirty_before || slint_touched_preview,
            );
            drop(rotation_pmu);
            return (
                physical_rect.map(RawPreviewPresent::Direct),
                trace,
                false,
                None,
            );
        }
        (present, trace, false, None)
    }

    pub(super) fn compose_exact_preview(
        &mut self,
        preview: &PreviewState,
    ) -> Option<RawPreviewPresent> {
        let frame = preview.raw_frame()?;
        if frame.status() != PreviewRawFrameStatus::Ready {
            return None;
        }
        if self.layout.is_portrait() || preview_direct_present_enabled() {
            let rect = self
                .target
                .blit_raw_preview_direct(&self.drawing_ui, &frame, true)?;
            if self.layout.is_portrait() {
                let transition_id = preview
                    .raw_transition_frame()
                    .map(|frame| frame.transition_id)
                    .unwrap_or(0);
                let token = oriented_preview_cache_token(
                    preview.presentation_generation(),
                    transition_id,
                    PreviewTransitionTrace::default(),
                );
                let rotation_pmu =
                    mister_magik_perf_events::sampled_span("gui.custom.preview-rotation");
                let physical_rect = self.target.compose_direct_preview_to_physical(
                    rect,
                    self.layout.output_layout(),
                    token,
                    true,
                );
                drop(rotation_pmu);
                physical_rect.map(RawPreviewPresent::Direct)
            } else {
                Some(RawPreviewPresent::Direct(rect))
            }
        } else {
            self.target
                .blit_raw_preview(&self.drawing_ui, &frame, true)
                .map(RawPreviewPresent::Cached)
        }
    }

    pub(super) fn compose_exact_preview_physical(
        &mut self,
        preview: &PreviewState,
        current: Option<&PhysicalLayerPublication>,
        version: &mut u64,
    ) -> (bool, Option<PhysicalLayerPublication>) {
        if !self.layout.is_portrait() {
            return (false, None);
        }
        let Some(frame) = preview.raw_frame() else {
            return (false, None);
        };
        if frame.status() != PreviewRawFrameStatus::Ready {
            return (false, None);
        }
        let Some(rect) = self
            .target
            .blit_raw_preview_direct(&self.drawing_ui, &frame, true)
        else {
            return (false, None);
        };
        let transition_id = preview
            .raw_transition_frame()
            .map(|frame| frame.transition_id)
            .unwrap_or(0);
        let token = oriented_preview_cache_token(
            preview.presentation_generation(),
            transition_id,
            PreviewTransitionTrace::default(),
        );
        let output = self.layout.output_layout();
        let physical_rect = self.layout.logical_rect_to_composition(rect);
        let rotation_pmu = mister_magik_perf_events::sampled_span("gui.custom.preview-rotation");
        let changed = self
            .target
            .compose_direct_preview_to_physical(rect, output, token, false)
            .is_some();
        drop(rotation_pmu);
        if !changed
            && !self
                .target
                .physical_direct_preview_matches(physical_rect, output, token)
        {
            return (false, None);
        }
        let layout_generation = self.output_layout_generation();
        let current = current.filter(|publication| {
            publication.role() == PhysicalLayerRole::Preview
                && publication.layout_generation() == layout_generation
                && publication.layout_epoch() == self.output_layout_epoch()
        });
        let publication = if changed || current.is_none() {
            *version = version.wrapping_add(1).max(1);
            let state = PhysicalLayerState::new(physical_rect, *version);
            self.capture_preview_publication(
                state,
                Some(PhysicalLayerUpdate::Full(physical_rect)),
                *version,
            )
        } else {
            None
        };
        let effective = publication.as_ref().or(current);
        let ready = effective
            .is_some_and(|publication| self.copy_preview_publication_to_cached(publication));
        (ready, publication)
    }

    fn copy_preview_publication_to_cached(
        &mut self,
        publication: &PhysicalLayerPublication,
    ) -> bool {
        if publication.role() != PhysicalLayerRole::Preview
            || publication.layout_generation() != self.output_layout_generation()
            || publication.layout_epoch() != self.output_layout_epoch()
        {
            return false;
        }
        self.copy_physical_layer_snapshot_to_cached(publication)
    }

    pub(super) fn compose_direct_preview_rect(&mut self, rect: DirtyRect) -> u32 {
        self.target.compose_direct_preview_rect(rect)
    }

    pub(super) fn copy_physical_layer_rect_to_hidden(
        &self,
        hidden: &mut ScanoutSlotsRgb565Framebuffer,
        rect: DirtyRect,
    ) -> u32 {
        self.direct_preview_view()
            .map(|view| copy_physical_layer_rect_to_hidden(hidden, view, rect))
            .unwrap_or(0)
    }

    pub(super) fn compose_arcade_list_update(
        &mut self,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
    ) -> PresentCopyStats {
        if self.layout.is_portrait() {
            let rotation_pmu =
                mister_magik_perf_events::sampled_span("gui.custom.arcade-list-rotation");
            let stats = compose_arcade_list_update_oriented(
                self.target,
                self.layout.output_layout(),
                renderer,
                update,
            );
            drop(rotation_pmu);
            stats
        } else {
            compose_arcade_list_update(self.target, renderer, update)
        }
    }

    pub(super) fn compose_arcade_list_direct_layer(
        &mut self,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
        catalog_generation: u64,
    ) -> (PresentCopyStats, ArcadeListUpdate) {
        let effective = renderer.compose_persistent_oriented_layer(
            self.layout.output_layout(),
            update,
            catalog_generation,
        );
        let physical_rect = renderer
            .persistent_oriented_layer_view()
            .expect("composed physical Arcade layer has a view")
            .rect();
        let physical_update = match effective {
            ArcadeListUpdate::Full(_) => ArcadeListUpdate::Full(physical_rect),
            ArcadeListUpdate::Scroll {
                delta_x, delta_y, ..
            } => {
                let (delta_x, delta_y) = self
                    .layout
                    .output_layout()
                    .logical_delta_to_physical(delta_x, delta_y);
                ArcadeListUpdate::Scroll {
                    delta_x,
                    delta_y,
                    rect: physical_rect,
                }
            }
        };
        (
            PresentCopyStats {
                rows: physical_update.dirty_rect().rows(),
                bytes: renderer
                    .present_pixels(&effective, matches!(effective, ArcadeListUpdate::Full(_)))
                    * 2,
            },
            physical_update,
        )
    }

    pub(super) fn compose_arcade_list_direct_layer_snapshot(
        &mut self,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
        catalog_generation: u64,
        version: u64,
        content_offset: LayerOffset,
        content_generation: u64,
    ) -> (PresentCopyStats, Option<PhysicalLayerPublication>) {
        let (stats, physical_update) =
            self.compose_arcade_list_direct_layer(renderer, update, catalog_generation);
        let publication = renderer
            .persistent_oriented_layer_view()
            .map(PhysicalLayerView::rect)
            .zip(renderer.take_persistent_oriented_layer_backing())
            .and_then(|(rect, backing)| {
                let state =
                    PhysicalLayerState::new(rect, version).with_content_offset(content_offset);
                PhysicalLayerPublication::capture_owned(
                    PhysicalLayerRole::Arcade,
                    self.output_layout_generation(),
                    self.output_layout_epoch(),
                    content_generation,
                    state,
                    Some(physical_update),
                    backing,
                )
            });
        if let Some(publication) = publication.as_ref() {
            assert!(
                self.copy_physical_layer_snapshot_to_cached(publication),
                "physical Arcade publication does not match the presentation cache"
            );
        }
        (stats, publication)
    }

    fn copy_physical_layer_snapshot_to_cached(
        &mut self,
        publication: &PhysicalLayerPublication,
    ) -> bool {
        let output = self.layout.output_layout();
        if publication.layout_generation() != self.output_layout_generation()
            || publication.layout_epoch() != self.output_layout_epoch()
        {
            return false;
        }
        let view = publication.view();
        let rect = view.rect();
        let stride = output.physical_stride();
        if rect.x0 >= rect.x1
            || rect.y0 >= rect.y1
            || rect.x1 > stride
            || rect.y1 > output.physical_height()
            || self.target.cached_565().len() < output.len()
        {
            return false;
        }
        let destination = self.target.cached_565_mut();
        for row in 0..rect.rows() as usize {
            let destination_start = (rect.y0 + row) * stride + rect.x0;
            destination[destination_start..destination_start + rect.width()].copy_from_slice(
                match view.row(rect, row) {
                    Some(source) => source,
                    None => return false,
                },
            );
        }
        true
    }

    pub(super) fn capture_preview_publication(
        &mut self,
        state: PhysicalLayerState,
        update: Option<PhysicalLayerUpdate>,
        content_generation: u64,
    ) -> Option<PhysicalLayerPublication> {
        let backing = self
            .target
            .take_preview_publication_backing(self.layout.is_portrait())?;
        PhysicalLayerPublication::capture_owned(
            PhysicalLayerRole::Preview,
            self.output_layout_generation(),
            self.output_layout_epoch(),
            content_generation,
            state,
            update,
            backing,
        )
    }

    pub(super) fn reclaim_preview_publication(
        &mut self,
        publication: &mut Option<PhysicalLayerPublication>,
    ) -> Option<(PhysicalLayerState, u64)> {
        let current = publication.take()?;
        if current.role() != PhysicalLayerRole::Preview
            || current.layout_generation() != self.output_layout_generation()
            || current.layout_epoch() != self.output_layout_epoch()
        {
            return None;
        }
        let state = current.state();
        let content_generation = current.content_generation();
        let backing = current.try_into_backing().ok()?;
        self.target
            .restore_preview_publication_backing(self.layout.is_portrait(), backing)
            .then_some((state, content_generation))
    }

    pub(super) fn capture_arcade_publication(
        &self,
        renderer: &mut ArcadeListRenderer,
        state: PhysicalLayerState,
        update: Option<PhysicalLayerUpdate>,
        content_generation: u64,
    ) -> Option<PhysicalLayerPublication> {
        let backing = renderer.take_persistent_oriented_layer_backing()?;
        PhysicalLayerPublication::capture_owned(
            PhysicalLayerRole::Arcade,
            self.output_layout_generation(),
            self.output_layout_epoch(),
            content_generation,
            state,
            update,
            backing,
        )
    }

    pub(super) fn reclaim_arcade_publication(
        &self,
        renderer: &mut ArcadeListRenderer,
        publication: &mut Option<PhysicalLayerPublication>,
    ) -> bool {
        let Some(current) = publication.take() else {
            return renderer.persistent_oriented_layer_view().is_some();
        };
        match current.try_into_backing() {
            Ok(backing) => renderer.restore_persistent_oriented_layer_backing(backing),
            Err(_) => false,
        }
    }

    pub(super) fn compose_arcade_list_over_backdrop(
        &mut self,
        renderer: &mut ArcadeListRenderer,
        backdrop: &[Rgb565Pixel],
        update: ArcadeListUpdate,
        backdrop_revision: u64,
        catalog_generation: u64,
        backdrop_is_fresh: bool,
        backdrop_is_settled: bool,
        force_full: bool,
        retained: &mut CrtArcadeOverlayState,
    ) -> ArcadeListCompositionStats {
        renderer.compose_retained_crt_layer_over_backdrop(
            self.target,
            backdrop,
            self.layout.output_layout(),
            update,
            backdrop_revision,
            catalog_generation,
            backdrop_is_fresh,
            backdrop_is_settled,
            force_full,
            retained,
        )
    }

    pub(super) fn compose_arcade_list_snapshot_update(
        &mut self,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
    ) -> PresentCopyStats {
        compose_arcade_list_update(self.target, renderer, update)
    }

    pub(super) fn copy_cached_arcade_list_update_to_hidden(
        &self,
        hidden: &mut ScanoutSlotsRgb565Framebuffer,
        renderer: &mut ArcadeListRenderer,
        update: ArcadeListUpdate,
    ) -> PresentCopyStats {
        debug_assert!(!self.layout.is_portrait());
        copy_arcade_list_update_to_hidden(hidden, renderer, update)
    }

    pub(super) fn arcade_overlay_requires_publication(&self) -> bool {
        self.layout.is_portrait()
    }

    pub(super) fn cached_frame_view(&self) -> CachedFrameView<'_> {
        self.target.cached_frame_view()
    }

    pub(super) fn presentation_frame_view(&self) -> CachedFrameView<'_> {
        self.target.cached_frame_view()
    }

    pub(super) fn presentation_pixels_mut(&mut self) -> &mut [Rgb565Pixel] {
        self.target.cached_565_mut()
    }

    pub(super) fn direct_preview_view(&self) -> Option<PhysicalLayerView<'_>> {
        if self.layout.is_portrait() {
            self.target.physical_direct_preview_view()
        } else {
            self.target.direct_preview_view()
        }
    }

    pub(super) fn direct_preview_rect(&self) -> Option<DirtyRect> {
        self.direct_preview_view().map(PhysicalLayerView::rect)
    }

    pub(super) fn direct_preview_backing_diagnostic(
        &self,
    ) -> mister_magik_fb::framebuffer::target::DirectPreviewBackingDiagnostic {
        self.target
            .direct_preview_backing_diagnostic(self.layout.is_portrait())
    }

    pub(super) fn output_layout_generation(&self) -> u64 {
        let output = self.layout.output_layout();
        let rotation = match output.rotation() {
            OutputRotation::None => 0_u64,
            OutputRotation::Clockwise90 => 1,
            OutputRotation::CounterClockwise90 => 2,
        };
        [
            output.logical_width() as u64,
            output.logical_height() as u64,
            output.physical_stride() as u64,
            output.physical_height() as u64,
            rotation,
        ]
        .into_iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, value| {
            (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    pub(super) const fn output_layout_epoch(&self) -> u64 {
        self.layout_epoch
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
    pub(super) readiness_source_evidence:
        Option<super::launcher_readiness::PostedSourceFrameEvidence>,
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
    pub(super) arcade_copy_trace: crate::arcade_list_renderer::PersistentArcadeCopyTrace,
    pub(super) main_present_backend: LauncherPresentBackend,
    pub(super) main_present_status: LauncherPresentStatus,
    pub(super) main_present_buffer: u8,
    pub(super) main_present_hidden_copy_us: u128,
    pub(super) main_present_hidden_publish_us: u128,
    pub(super) main_present_hidden_copied_bytes: usize,
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
    pub(super) main_present_receipt_crc: u16,
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
    fn portrait_preview_clear_matches_logical_mapping_for_both_rotations() {
        let ui = UiDisplay::for_framebuffer(960, 540);
        for orientation in [
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
        ] {
            let layout = UiLayoutGeometry::for_display(&ui, orientation);
            let logical_rect = preview_screen_rect(&UiDisplay::for_framebuffer(
                layout.logical_w(),
                layout.logical_h(),
            ));
            let original = (0..ui.render_w() * ui.render_h())
                .map(|index| Rgb565Pixel((index as u16).wrapping_mul(17) | 1))
                .collect::<Vec<_>>();
            let mut expected = original.clone();
            let mut surface = mister_magik_framebuffer_scenes::Rgb565SurfaceMut::new(
                &mut expected,
                layout.output_layout(),
            )
            .unwrap();
            for y in logical_rect.y0..logical_rect.y1 {
                for x in logical_rect.x0..logical_rect.x1 {
                    assert!(surface.set(x, y, Rgb565Pixel(0)));
                }
            }

            let mut target =
                UiFrameTarget::cached(FramebufferTargetGeometry::new(ui.render_w(), ui.render_h()));
            target.cached_565_mut().copy_from_slice(&original);
            let cleared =
                LayerTarget::new_oriented(&mut target, layout).clear_presentation_preview();

            assert_eq!(cleared, layout.logical_rect_to_composition(logical_rect));
            assert_eq!(target.cached_565(), expected);
        }
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
    fn portrait_preview_layer_rect_is_the_published_physical_backing_rect() {
        let ui = UiDisplay::for_framebuffer(4, 3);
        let layout = UiLayoutGeometry::for_display(&ui, ScreenOrientation::MonitorClockwise);
        let logical = DirtyRect {
            x0: 0,
            y0: 1,
            x1: 3,
            y1: 3,
        };
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(4, 3));
        target
            .direct_preview_565_rect_mut(logical)
            .0
            .fill(Rgb565Pixel(9));
        let published = target
            .compose_direct_preview_to_physical(logical, layout.output_layout(), 1, true)
            .unwrap();

        let layer_target = LayerTarget::new_oriented(&mut target, layout);
        assert_eq!(layer_target.direct_preview_rect(), Some(published));
        assert_eq!(published, layout.logical_rect_to_composition(logical));
    }

    #[test]
    fn navigation_snapshot_uses_only_the_matching_preview_publication() {
        let ui = UiDisplay::for_framebuffer(4, 3);
        let layout = UiLayoutGeometry::for_display(&ui, ScreenOrientation::MonitorClockwise);
        let logical = DirtyRect {
            x0: 0,
            y0: 1,
            x1: 3,
            y1: 3,
        };
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(4, 3));
        target
            .direct_preview_565_rect_mut(logical)
            .0
            .copy_from_slice(&[
                Rgb565Pixel(1),
                Rgb565Pixel(2),
                Rgb565Pixel(3),
                Rgb565Pixel(4),
                Rgb565Pixel(5),
                Rgb565Pixel(6),
            ]);
        let physical = target
            .compose_direct_preview_to_physical(logical, layout.output_layout(), 11, true)
            .unwrap();
        let expected = target
            .physical_direct_preview_view()
            .unwrap()
            .pixels()
            .to_vec();
        target.cached_565_mut().fill(Rgb565Pixel(0));
        let mut layer_target = LayerTarget::new_oriented(&mut target, layout);
        let publication = layer_target
            .capture_preview_publication(
                PhysicalLayerState::new(physical, 1),
                Some(PhysicalLayerUpdate::Full(physical)),
                1,
            )
            .unwrap();

        assert!(layer_target.copy_preview_publication_to_cached(&publication));
        let copied = layer_target
            .presentation_frame_view()
            .pixels()
            .iter()
            .enumerate()
            .filter_map(|(index, pixel)| {
                let x = index % 4;
                let y = index / 4;
                (x >= physical.x0 && x < physical.x1 && y >= physical.y0 && y < physical.y1)
                    .then_some(*pixel)
            })
            .collect::<Vec<_>>();
        assert_eq!(copied, expected);

        let mut replacement = vec![Rgb565Pixel(9); expected.len()];
        assert!(layer_target.target.adopt_physical_direct_preview(
            &mut replacement,
            physical,
            layout.output_layout(),
            12,
        ));
        assert!(!layer_target.copy_preview_publication_to_cached(&publication));
    }

    #[test]
    fn physical_layer_snapshot_updates_only_its_published_rect() {
        let ui = UiDisplay::for_framebuffer(4, 3);
        let layout = UiLayoutGeometry::for_display(&ui, ScreenOrientation::MonitorClockwise);
        let output = layout.output_layout();
        let rect = DirtyRect {
            x0: 1,
            y0: 1,
            x1: 4,
            y1: 3,
        };
        let source = (0..output.len())
            .map(|index| Rgb565Pixel(index as u16 + 1))
            .collect::<Vec<_>>();
        let view = PhysicalLayerView::from_frame_region(
            &source,
            output.physical_stride(),
            output.physical_height(),
            rect,
        )
        .unwrap();
        let untouched = Rgb565Pixel(0xffff);
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(
            output.physical_stride(),
            output.physical_height(),
        ));
        target.cached_565_mut().fill(untouched);
        let mut layer_target = LayerTarget::new_oriented(&mut target, layout);
        let publication = PhysicalLayerPublication::capture(
            PhysicalLayerRole::Arcade,
            layer_target.output_layout_generation(),
            layer_target.output_layout_epoch(),
            9,
            PhysicalLayerState::new(rect, 3),
            Some(PhysicalLayerUpdate::Full(rect)),
            view,
        )
        .unwrap();

        assert!(layer_target.copy_physical_layer_snapshot_to_cached(&publication));
        for y in 0..output.physical_height() {
            for x in 0..output.physical_stride() {
                let index = y * output.physical_stride() + x;
                let expected = if x >= rect.x0 && x < rect.x1 && y >= rect.y0 && y < rect.y1 {
                    source[index]
                } else {
                    untouched
                };
                assert_eq!(target.cached_565()[index], expected);
            }
        }
    }
}
