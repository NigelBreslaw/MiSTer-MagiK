// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

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

#[derive(Default)]
pub(super) struct NativeDeviceBackground {
    source: Option<Option<crate::device_art::DeviceKind>>,
    full_repaint: bool,
    layout_epoch: Option<u64>,
}
impl NativeDeviceBackground {
    pub(super) fn invalidate(&mut self) {
        self.full_repaint = true;
    }
}

struct NativeBackground<'a> {
    cache: &'a mut NativeDeviceBackground,
    kind: Option<crate::device_art::DeviceKind>,
}

pub(super) struct LayerTarget<'a> {
    target: &'a mut UiFrameTarget,
    background: Option<NativeBackground<'a>>,
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
    #[cfg(test)]
    pub(super) fn new(target: &'a mut UiFrameTarget, ui: &'a UiDisplay) -> Self {
        Self {
            target,
            background: None,
            layout: UiLayoutGeometry::for_display(ui, ScreenOrientation::Normal),
            layout_epoch: 1,
            drawing_ui: UiDisplay::for_framebuffer(ui.render_w(), ui.render_h()),
        }
    }

    pub(super) fn new_oriented(target: &'a mut UiFrameTarget, layout: UiLayoutGeometry) -> Self {
        Self {
            target,
            background: None,
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
            background: None,
            layout,
            layout_epoch,
            drawing_ui: UiDisplay::for_framebuffer(layout.logical_w(), layout.logical_h()),
        }
    }

    pub(super) fn attach_device_background(
        &mut self,
        background: &'a mut NativeDeviceBackground,
        kind: Option<crate::device_art::DeviceKind>,
        window: &MisterSoftwareWindow,
    ) {
        if background.layout_epoch != Some(self.layout_epoch) {
            background.invalidate();
            background.layout_epoch = Some(self.layout_epoch);
        }
        if background.full_repaint || background.source != Some(kind) {
            window.request_redraw();
        }
        self.background = Some(NativeBackground {
            cache: background,
            kind,
        });
    }

    fn render_slint_pixels(
        &mut self,
        renderer: &slint::platform::software_renderer::SoftwareRenderer,
    ) -> slint::platform::software_renderer::PhysicalRegion {
        let Some(background) = self.background.as_mut() else {
            return self.target.render(renderer);
        };
        if background.cache.full_repaint || background.cache.source != Some(background.kind) {
            use i_slint_core::renderer::RendererSealed;
            renderer.mark_dirty_region(
                i_slint_core::lengths::LogicalRect::new(
                    if background.cache.full_repaint {
                        i_slint_core::lengths::LogicalPoint::new(0.0, 0.0)
                    } else {
                        i_slint_core::lengths::LogicalPoint::new(490.0, 77.0)
                    },
                    if background.cache.full_repaint {
                        i_slint_core::lengths::LogicalSize::new(960.0, 540.0)
                    } else {
                        i_slint_core::lengths::LogicalSize::new(470.0, 423.0)
                    },
                )
                .into(),
            );
        }
        // Keep the renderer's original RGB565 quantization for every panel
        // and glyph. The fixed device plane has no foreground outside its
        // screen opening while this path is eligible.
        let region = self.target.render(renderer);
        #[cfg(feature = "tooling")]
        let _copy =
            mister_magik_framebuffer_scenes::launcher_profile::span("frame.native-device-copy");
        let source = crate::launcher_presentation::system_device_rgb565(background.kind);
        let pixels = self.target.cached_565_mut();
        for rect in dirty_rects(&region, 960, 540).iter() {
            let left = rect.x0.max(490);
            let right = rect.x1.min(960);
            if left >= right {
                continue;
            }
            for y in rect.y0.max(77)..rect.y1.min(500) {
                let spans = if (96..416).contains(&y) {
                    [(left, right.min(572)), (left.max(892), right)]
                } else {
                    [(left, right), (0, 0)]
                };
                for (left, right) in spans {
                    if left >= right {
                        continue;
                    }
                    let start = (y - 35) * 483 + left - 490;
                    for (dst, src) in pixels[y * 960 + left..y * 960 + right]
                        .iter_mut()
                        .zip(&source[start..start + right - left])
                    {
                        dst.0 = src.0;
                    }
                }
            }
        }
        background.cache.source = Some(background.kind);
        background.cache.full_repaint = false;
        region
    }

    pub(super) fn render_slint_base(
        &mut self,
        window: &MisterSoftwareWindow,
    ) -> (Option<DirtyRect>, DirtyRectList) {
        let mut slint_dirty = None;
        let mut slint_damage = DirtyRectList::new();
        window.draw_if_needed(|renderer| {
            #[cfg(feature = "tooling")]
            let _slint =
                mister_magik_framebuffer_scenes::launcher_profile::span("frame.slint-raster");
            let region = self.render_slint_pixels(renderer);
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
        let rendered = window.draw_full_frame_preserving_cache_if_needed(
            self.layout.logical_w(),
            self.layout.logical_h(),
            |renderer| {
                #[cfg(feature = "tooling")]
                let _slint =
                    mister_magik_framebuffer_scenes::launcher_profile::span("frame.slint-raster");
                #[cfg(feature = "tooling")]
                let _full = mister_magik_framebuffer_scenes::launcher_profile::span(
                    "frame.slint-full-raster",
                );
                let region = self.render_slint_pixels(renderer);
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
            },
        );
        (slint_dirty, slint_damage, rendered)
    }

    pub(super) fn render_custom_home(
        &mut self,
        window: &MisterSoftwareWindow,
        pixels: &[mister_magik_framebuffer_scenes::Rgb565Pixel],
        full_slint_raster: bool,
        copy_damage: Option<DirtyRect>,
    ) -> (Option<DirtyRect>, DirtyRectList, bool, Option<DirtyRect>) {
        let layout = self.layout;
        if pixels.len() != layout.logical_w().saturating_mul(layout.logical_h()) {
            return (None, DirtyRectList::new(), false, None);
        }
        // Retained motion is safe only in a seeded native landscape cache.
        let base_dirty = copy_damage
            .filter(|rect| {
                !full_slint_raster
                    && !layout.is_portrait()
                    && layout.logical_w() == 960
                    && layout.logical_h() == 540
                    && rect.x0 < rect.x1
                    && rect.y0 < rect.y1
                    && rect.x1 <= 960
                    && rect.y1 <= 540
            })
            .unwrap_or(DirtyRect {
                x0: 0,
                y0: 0,
                x1: layout.composition_w(),
                y1: layout.composition_h(),
            });
        // A native base update requires recomposing overlays even if Slint's
        // properties did not change or a previous raster consumed the redraw.
        window.request_redraw();
        let mut slint_dirty = None;
        let mut damage = DirtyRectList::new();
        let rendered = window.draw_if_needed(|renderer| {
            #[cfg(feature = "tooling")]
            let _slint =
                mister_magik_framebuffer_scenes::launcher_profile::span("frame.slint-raster");
            use i_slint_core::renderer::RendererSealed;

            // Include native damage in Slint's raster so unchanged overlays
            // are composed with the updated background in the same pass.
            let logical_damage = layout.composition_rect_to_logical_rect(base_dirty);
            renderer.mark_dirty_region(
                i_slint_core::lengths::LogicalRect::new(
                    i_slint_core::lengths::LogicalPoint::new(
                        logical_damage.x0 as f32,
                        logical_damage.y0 as f32,
                    ),
                    i_slint_core::lengths::LogicalSize::new(
                        logical_damage.width() as f32,
                        logical_damage.rows() as f32,
                    ),
                )
                .into(),
            );
            let region =
                self.target
                    .render_over_background(renderer, pixels, layout.output_layout());
            slint_dirty = dirty_rect(&region, layout.composition_w(), layout.composition_h());
            damage = dirty_rects(&region, layout.composition_w(), layout.composition_h());
        });
        damage.push(base_dirty);
        (
            Some(slint_dirty.map_or(base_dirty, |dirty| dirty.union(base_dirty))),
            damage,
            rendered,
            Some(base_dirty),
        )
    }

    #[cfg(test)]
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

    pub(super) fn restore_presentation_cached(&mut self, snapshot: &[Rgb565Pixel]) -> bool {
        restore_cached_565(self.target, snapshot)
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
                    repair_rect: renderer.persistent_oriented_layer_selection_aperture(),
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

    #[allow(clippy::too_many_arguments)]
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
    use crate::visual_platform::install_isolated_test_platform;

    slint::slint! {
        export component NativeDeviceProbe inherits Window {
            width: 960px; height: 540px;
            in property <bool> native: false;
            in property <image> artwork;
            in property <bool> overlay: false;
            in property <length> overlay-x: 520px;
            background: black;
            Rectangle {
                x: 490px; y: 77px; width: 483px; height: 423px; clip: true;
                if !root.native : Image {
                    x: 0px; y: -42px; width: 483px; height: 519px;
                    source: root.artwork; image-fit: fill; image-rendering: pixelated;
                }
                Rectangle { x: 82px; y: 19px; width: 320px; height: 320px; background: black; }
            }
            Rectangle { x: 26px; y: 104px; width: 462px; height: 394px; background: #0a0e18; }
            if root.overlay : Rectangle {
                x: root.overlay-x; y: 70px; width: 220px; height: 420px; background: #00000080;
                Rectangle { x: 20px; y: 60px; width: 140px; height: 80px; background: #bb773399; }
            }
        }

        export component NativeHomeOverlayProbe inherits Window {
            width: 960px;
            height: 540px;
            background: transparent;
            in property <bool> overlay-visible: false;
            in property <length> overlay-x: 20px;
            in property <length> overlay-y: 20px;
            if root.overlay-visible : Rectangle {
                x: root.overlay-x;
                y: root.overlay-y;
                width: 80px;
                height: 60px;
                background: #00000080;
            }
        }
    }

    #[test]
    fn native_device_repaints_kind_and_layout_epoch_changes() {
        std::thread::spawn(|| {
            use crate::device_art::DeviceKind;
            let window = install_isolated_test_platform();
            let app = NativeDeviceProbe::new().unwrap();
            window.set_size(PhysicalSize::new(960, 540));
            app.show().unwrap();
            let ui = UiDisplay::for_framebuffer(960, 540);
            let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(960, 540));
            let mut cache = NativeDeviceBackground::default();
            app.set_native(true);
            {
                let mut layer = LayerTarget::new(&mut target, &ui);
                layer.attach_device_background(&mut cache, Some(DeviceKind::Tv), &window);
                layer.render_slint_base(&window);
            }
            let previous = target.cached_565().to_vec();
            let packed =
                crate::launcher_presentation::system_device_rgb565(Some(DeviceKind::Monitor));
            let mut image = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(483, 519);
            assert!(mister_magik_framebuffer_scenes::expand_rgb565_rgb8(
                packed,
                image.make_mut_bytes()
            ));
            app.set_artwork(slint::Image::from_rgb8(image));
            app.set_native(false);
            window.request_redraw();
            LayerTarget::new(&mut target, &ui).render_slint_full(&window);
            let expected = target.cached_565().to_vec();
            target.cached_565_mut().copy_from_slice(&previous);
            app.set_native(true);
            {
                let mut layer = LayerTarget::new(&mut target, &ui);
                layer.attach_device_background(&mut cache, Some(DeviceKind::Monitor), &window);
                layer.render_slint_base(&window);
            }
            assert!(target.cached_565() == expected);
            target.cached_565_mut().fill(Rgb565Pixel(0xf81f));
            {
                let layout = UiLayoutGeometry::for_display(&ui, ScreenOrientation::Normal);
                let mut layer = LayerTarget::new_oriented_with_epoch(&mut target, layout, 2);
                layer.attach_device_background(&mut cache, Some(DeviceKind::Monitor), &window);
                layer.render_slint_base(&window);
            }
            assert!(target.cached_565() == expected);
        })
        .join()
        .unwrap();
    }

    // Background activity pills are Slint content composed before the Rust
    // Arcade list and preview layers, so they must stay in the header band:
    // on screen, clear of the title and clock, and outside both layer rects.
    #[test]
    fn header_activity_clears_title_clock_and_arcade_layers() {
        std::thread::spawn(|| {
            use crate::arcade_list_renderer::ArcadeListGeometry;
            use crate::visual_composition::hdmi_preview_rect;
            use slint::{ModelRc, VecModel};
            use slint_ui::launcher::{
                CatalogActivity, CatalogView, Launcher, LauncherScreen, MediaPackRow,
                MediaPackState, MediaView, MisterUi, NavigationView, ScreenOrientation,
            };
            let window = install_isolated_test_platform();
            let app = Launcher::new().unwrap();
            app.show().unwrap();
            let catalog = app.global::<CatalogView>();
            let media = app.global::<MediaView>();
            let pack = |system: &str, state: MediaPackState, percent: i32| MediaPackRow {
                system: system.into(),
                image_size: "320px".into(),
                state,
                phase_label: "Downloading".into(),
                percent,
                bytes_label: "42 MB / 61 MB".into(),
                pack_position: "1 of 2".into(),
            };
            for (width, height, orientation) in [
                (960, 540, ScreenOrientation::Normal),
                (540, 960, ScreenOrientation::MonitorClockwise),
            ] {
                let mister = app.global::<MisterUi>();
                mister.set_window_width(width as i32);
                mister.set_window_height(height as i32);
                mister.set_screen_orientation(orientation);
                window.set_size(PhysicalSize::new(width as u32, height as u32));
                app.global::<NavigationView>()
                    .set_screen(LauncherScreen::Settings);
                let ui = UiDisplay::for_framebuffer(width, height);
                let mut target =
                    UiFrameTarget::cached(FramebufferTargetGeometry::new(width, height));

                catalog.set_activity(CatalogActivity::Idle);
                catalog.set_background_activity_visible(false);
                media.set_rows(ModelRc::default());
                window.request_redraw();
                LayerTarget::new(&mut target, &ui).render_slint_full(&window);
                let baseline = target.cached_565().to_vec();

                // Both pills with long labels: the worst case for width.
                catalog.set_activity(CatalogActivity::Background);
                catalog.set_background_activity_visible(true);
                catalog.set_title("Updating systems 120/300 Nintendo Entertainment System".into());
                media.set_rows(ModelRc::new(VecModel::from(vec![
                    pack("Neo Geo Pocket Color", MediaPackState::Downloading, 68),
                    pack("Arcade", MediaPackState::Queued, 0),
                ])));
                window.request_redraw();
                LayerTarget::new(&mut target, &ui).render_slint_full(&window);

                let mut changed: Option<DirtyRect> = None;
                for (index, (now, before)) in target.cached_565().iter().zip(&baseline).enumerate()
                {
                    if now != before {
                        let (x, y) = (index % width, index / width);
                        let pixel = DirtyRect {
                            x0: x,
                            y0: y,
                            x1: x + 1,
                            y1: y + 1,
                        };
                        changed = Some(changed.map_or(pixel, |rect| rect.union(pixel)));
                    }
                }
                let pills = changed.expect("header activity rendered");
                assert!(
                    pills.x0 >= 26,
                    "{width}x{height} pills leave the screen: {pills:?}"
                );
                for y in pills.y0..pills.y1 {
                    for x in pills.x0..pills.x1 {
                        assert_eq!(
                            baseline[y * width + x].0,
                            0,
                            "{width}x{height} pills {pills:?} cover header content at {x},{y}"
                        );
                    }
                }
                let lists = if height > width {
                    [false, true].map(|search| ArcadeListGeometry::portrait(width, height, search))
                } else {
                    [
                        ArcadeListGeometry::NORMAL,
                        ArcadeListGeometry::search_for_render_w(width),
                    ]
                };
                for layer in lists
                    .map(ArcadeListGeometry::dirty_rect)
                    .into_iter()
                    .chain([hdmi_preview_rect(width, height)])
                {
                    assert_eq!(
                        pills.intersection(layer),
                        None,
                        "{width}x{height} pills {pills:?} overlap Rust layer {layer:?}"
                    );
                }
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn native_device_matches_complete_hub_games_and_status_pages() {
        std::thread::spawn(|| {
            use crate::device_art::DeviceKind;
            use slint_ui::launcher::{
                ArcadeLoadState, ArcadeSearchMode, ArcadeView, Launcher, LauncherScreen, MisterUi,
                NavigationView, SystemPageMode,
            };
            let window = install_isolated_test_platform();
            let app = Launcher::new().unwrap();
            window.set_size(PhysicalSize::new(960, 540));
            app.show().unwrap();
            let nav = app.global::<NavigationView>();
            let arcade = app.global::<ArcadeView>();
            nav.set_screen(LauncherScreen::Arcade);
            nav.set_system_title("SUPER NINTENDO".into());
            nav.set_system_title_wraps(true);
            nav.set_system_subtitle("NINTENDO / 1990".into());
            nav.set_system_hub_games_count(1857);
            arcade.set_collection_title("SNES".into());
            let ui = UiDisplay::for_framebuffer(960, 540);
            let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(960, 540));
            let mut cache = NativeDeviceBackground::default();
            for kind in [
                None,
                Some(DeviceKind::Tv),
                Some(DeviceKind::Monitor),
                Some(DeviceKind::Handheld),
            ] {
                let packed = crate::launcher_presentation::system_device_rgb565(kind);
                let mut image = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(483, 519);
                assert!(mister_magik_framebuffer_scenes::expand_rgb565_rgb8(
                    packed,
                    image.make_mut_bytes()
                ));
                arcade.set_device_backdrop(slint::Image::from_rgb8(image));
                for state in 0..5 {
                    nav.set_system_page_mode(if state == 0 {
                        SystemPageMode::Hub
                    } else {
                        SystemPageMode::List
                    });
                    arcade.set_load_state(if state == 2 {
                        ArcadeLoadState::Loading
                    } else {
                        ArcadeLoadState::Ready
                    });
                    arcade.set_active_count(if state == 3 { 0 } else { 1857 });
                    arcade.set_search_mode(if state == 3 {
                        ArcadeSearchMode::Active
                    } else {
                        ArcadeSearchMode::Inactive
                    });
                    arcade.set_drawer_open(state == 4);
                    app.global::<MisterUi>().set_custom_device_base(false);
                    window.request_redraw();
                    LayerTarget::new(&mut target, &ui).render_slint_full(&window);
                    let expected = target.cached_565().to_vec();
                    target.cached_565_mut().fill(Rgb565Pixel(0xf81f));
                    cache.invalidate();
                    app.global::<MisterUi>().set_custom_device_base(true);
                    window.request_redraw();
                    {
                        let mut layer = LayerTarget::new(&mut target, &ui);
                        layer.attach_device_background(&mut cache, kind, &window);
                        layer.render_slint_base(&window);
                    }
                    assert!(
                        target.cached_565() == expected,
                        "{kind:?} state={state} first={:?}",
                        target
                            .cached_565()
                            .iter()
                            .zip(&expected)
                            .enumerate()
                            .find(|(_, (a, b))| a != b)
                    );
                }
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn native_device_background_matches_slint_pixels_through_overlays_and_reentry() {
        std::thread::spawn(|| {
            use crate::device_art::DeviceKind;
            let window = install_isolated_test_platform();
            let app = NativeDeviceProbe::new().unwrap();
            window.set_size(PhysicalSize::new(960, 540));
            app.show().unwrap();
            let ui = UiDisplay::for_framebuffer(960, 540);
            let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(960, 540));
            let mut cache = NativeDeviceBackground::default();
            for kind in [
                None,
                Some(DeviceKind::Tv),
                Some(DeviceKind::Monitor),
                Some(DeviceKind::Handheld),
            ] {
                let packed = crate::launcher_presentation::system_device_rgb565(kind);
                let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(483, 519);
                assert!(mister_magik_framebuffer_scenes::expand_rgb565_rgb8(
                    packed,
                    pixels.make_mut_bytes()
                ));
                app.set_artwork(slint::Image::from_rgb8(pixels));
                for overlay in [false, true] {
                    app.set_overlay(overlay);
                    app.set_native(false);
                    window.request_redraw();
                    {
                        let mut layer = LayerTarget::new(&mut target, &ui);
                        layer.render_slint_full(&window);
                    }
                    let expected = target.cached_565().to_vec();
                    // Simulate leftover Home pixels, then enter native mode.
                    target.cached_565_mut().fill(Rgb565Pixel(0xf81f));
                    cache.invalidate();
                    app.set_native(!overlay);
                    window.request_redraw();
                    {
                        let mut layer = LayerTarget::new(&mut target, &ui);
                        if !overlay {
                            layer.attach_device_background(&mut cache, kind, &window);
                            layer.render_slint_base(&window);
                        } else {
                            layer.render_slint_full(&window);
                        }
                    }
                    assert!(
                        target.cached_565() == expected,
                        "{kind:?}, overlay={overlay}, first={:?}",
                        target
                            .cached_565()
                            .iter()
                            .zip(&expected)
                            .enumerate()
                            .find(|(_, (a, b))| a != b)
                    );
                    if !overlay {
                        assert_eq!(cache.source, Some(kind));
                        assert!(!cache.full_repaint);
                    }
                    // A partial overlay move must restore the old covered art.
                    if overlay {
                        app.set_overlay_x(600.0);
                        window.request_redraw();
                        {
                            let mut layer = LayerTarget::new(&mut target, &ui);
                            layer.render_slint_base(&window);
                        }
                        let native = target.cached_565().to_vec();
                        app.set_native(false);
                        window.request_redraw();
                        LayerTarget::new(&mut target, &ui).render_slint_full(&window);
                        assert!(
                            target.cached_565() == native,
                            "partial first={:?}",
                            target
                                .cached_565()
                                .iter()
                                .zip(&native)
                                .enumerate()
                                .find(|(_, (a, b))| a != b)
                        );
                        app.set_overlay_x(520.0);
                    }
                }
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn home_snapshot_restores_cards_after_an_intermediate_target_overwrite() {
        std::thread::spawn(|| {
            let window = install_isolated_test_platform();
            let overlay = NativeHomeOverlayProbe::new().unwrap();
            window.set_size(PhysicalSize::new(960, 540));
            overlay.show().unwrap();
            let ui = UiDisplay::for_framebuffer(960, 540);
            let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(960, 540));
            let cards = vec![mister_magik_framebuffer_scenes::Rgb565Pixel(0xffff); 960 * 540];
            let mut layer = LayerTarget::new(&mut target, &ui);
            layer.render_custom_home(&window, &cards, true, None);
            layer.render_black();
            assert!(layer.cached_frame_view().pixels().iter().all(|p| p.0 == 0));
            // The snapshot path must restore even after a full raster in this pass.
            layer.render_custom_home(&window, &cards, true, None);
            assert!(
                layer
                    .cached_frame_view()
                    .pixels()
                    .iter()
                    .all(|p| p.0 == 0xffff)
            );
            assert!(
                layer
                    .presentation_frame_view()
                    .pixels()
                    .iter()
                    .all(|p| p.0 == 0xffff)
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn custom_home_redraw_preserves_native_cards_without_input() {
        std::thread::spawn(|| {
            let window = install_isolated_test_platform();
            let overlay = NativeHomeOverlayProbe::new().expect("overlay probe");
            window.set_size(PhysicalSize::new(960, 540));
            overlay.show().expect("show overlay probe");
            let ui = UiDisplay::for_framebuffer(960, 540);
            let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(960, 540));
            let cards = vec![mister_magik_framebuffer_scenes::Rgb565Pixel(0xffff); 960 * 540];
            let mut layer = LayerTarget::new(&mut target, &ui);
            layer.render_custom_home(&window, &cards, false, None);
            assert!(
                layer
                    .presentation_frame_view()
                    .pixels()
                    .iter()
                    .all(|pixel| pixel.0 == 0xffff),
                "transparent Slint redraw erased the native card frame"
            );

            overlay.set_overlay_visible(true);
            layer.render_custom_home(&window, &cards, false, None);
            assert_ne!(
                layer.presentation_frame_view().pixels()[30 * 960 + 30].0,
                0xffff
            );
            assert_eq!(layer.presentation_frame_view().pixels()[0].0, 0xffff);
            let overlay_pixel = layer.presentation_frame_view().pixels()[30 * 960 + 30].0;
            for full in [false, true, false] {
                window.request_redraw();
                layer.render_custom_home(&window, &cards, full, None);
                assert_eq!(
                    layer.presentation_frame_view().pixels()[30 * 960 + 30].0,
                    overlay_pixel,
                    "unchanged overlay vanished or accumulated alpha after background restoration"
                );
            }
            overlay.set_overlay_visible(false);
            layer.render_custom_home(&window, &cards, false, None);
            assert!(
                layer
                    .presentation_frame_view()
                    .pixels()
                    .iter()
                    .all(|pixel| pixel.0 == 0xffff),
                "removed overlay failed to reveal the native frame"
            );

            overlay.set_overlay_x(320.0);
            overlay.set_overlay_y(160.0);
            overlay.set_overlay_visible(true);
            layer.render_custom_home(&window, &cards, false, None);
            let tile_damage = DirtyRect {
                x0: 296,
                y0: 120,
                x1: 934,
                y1: 495,
            };
            let mut changed_cards = cards;
            for y in tile_damage.y0..tile_damage.y1 {
                changed_cards[y * 960 + tile_damage.x0..y * 960 + tile_damage.x1]
                    .fill(mister_magik_framebuffer_scenes::Rgb565Pixel(0x001f));
            }
            layer.render_custom_home(&window, &changed_cards, false, Some(tile_damage));
            let mut expected_overlay = Rgb565Pixel(0x001f);
            expected_overlay.blend(slint::platform::software_renderer::PremultipliedRgbaColor {
                red: 0,
                green: 0,
                blue: 0,
                alpha: 128,
            });
            assert_eq!(
                layer.presentation_frame_view().pixels()[170 * 960 + 330],
                expected_overlay
            );
            assert_eq!(
                layer.presentation_frame_view().pixels()[150 * 960 + 300].0,
                0x001f
            );
            assert_eq!(layer.presentation_frame_view().pixels()[0].0, 0xffff);
            let before = layer.presentation_frame_view().pixels().to_vec();
            let (dirty, damage, rendered, copied) =
                layer.render_custom_home(&window, &changed_cards[..100], false, Some(tile_damage));
            assert!(dirty.is_none() && damage.is_empty() && !rendered && copied.is_none());
            assert!(
                layer.presentation_frame_view().pixels() == before,
                "rejected native geometry must preserve the displayed frame"
            );
        })
        .join()
        .expect("compositor test thread");
    }

    #[test]
    fn custom_home_redraw_restores_native_pixels_in_all_orientations() {
        std::thread::spawn(|| {
            let window = install_isolated_test_platform();
            let overlay = NativeHomeOverlayProbe::new().expect("overlay probe");
            overlay.show().expect("show overlay probe");
            for (width, height) in [
                (960, 540),
                (540, 960),
                (640, 240),
                (768, 288),
                (640, 480),
                (768, 576),
            ] {
                let ui = UiDisplay::for_framebuffer(width, height);
                for orientation in [
                    ScreenOrientation::Normal,
                    ScreenOrientation::MonitorClockwise,
                    ScreenOrientation::MonitorCounterclockwise,
                ] {
                    let layout = UiLayoutGeometry::for_display(&ui, orientation);
                    configure_window_layout(&layout, &window);
                    let mut target =
                        UiFrameTarget::cached(FramebufferTargetGeometry::new(width, height));
                    let cards = (0..layout.logical_w() * layout.logical_h())
                        .map(|i| {
                            mister_magik_framebuffer_scenes::Rgb565Pixel(
                                (i as u16).wrapping_mul(17) | 1,
                            )
                        })
                        .collect::<Vec<_>>();
                    let mut layer = LayerTarget::new_oriented(&mut target, layout);
                    overlay.set_overlay_visible(false);
                    layer.render_custom_home(&window, &cards, true, None);
                    let expected = layer.presentation_frame_view().pixels().to_vec();
                    for y in 0..layout.logical_h() {
                        for x in 0..layout.logical_w() {
                            let (px, py) = layout.logical_pixel_to_composition(x, y);
                            assert_eq!(
                                expected[py * width + px].0,
                                cards[y * layout.logical_w() + x].0
                            );
                        }
                    }
                    overlay.set_overlay_visible(true);
                    layer.render_custom_home(&window, &cards, false, None);
                    let (px, py) = layout.logical_pixel_to_composition(30, 30);
                    assert_ne!(
                        layer.presentation_frame_view().pixels()[py * width + px],
                        expected[py * width + px]
                    );
                    overlay.set_overlay_visible(false);
                    layer.render_custom_home(
                        &window,
                        &cards,
                        false,
                        Some(DirtyRect {
                            x0: 20,
                            y0: 20,
                            x1: 100,
                            y1: 80,
                        }),
                    );
                    assert_eq!(
                        layer.presentation_frame_view().pixels(),
                        expected,
                        "removed overlay damaged {width}x{height} {orientation:?}"
                    );
                }
            }
        })
        .join()
        .expect("compositor geometry test thread");
    }

    #[test]
    fn cold_intro_snapshot_and_idle_handoff_keep_the_real_launcher_cards() {
        std::thread::spawn(|| {
            let window = install_isolated_test_platform();
            let app = slint_ui::launcher::Launcher::new().expect("production launcher");
            app.global::<slint_ui::launcher::MisterUi>()
                .set_custom_home_base(true);
            let ui = UiDisplay::for_framebuffer(960, 540);
            window.set_size(PhysicalSize::new(960, 540));
            app.show().expect("show production launcher");
            let nav = LauncherNav::new();
            let catalog = empty_arcade_catalog("/media/fat");
            let level = crate::launcher_home::CardLevelSnapshot::from_runtime(&nav, &catalog);
            let mut home = super::super::launcher_card_home::LauncherCardHomeSession::new(
                super::super::launcher_card_home::scene_for_display(
                    &ui,
                    UiLayoutGeometry::for_display(&ui, ScreenOrientation::Normal),
                ),
                level,
                0,
                "12:34",
            )
            .expect("production native cards");
            let cards = home.render().to_vec();
            assert!(
                cards.iter().any(|pixel| pixel.0 != 0),
                "native launcher must not be black"
            );
            let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(960, 540));
            let mut layer = LayerTarget::new(&mut target, &ui);
            layer.render_custom_home(&window, &cards, false, None);
            let expected = cards
                .iter()
                .map(|pixel| Rgb565Pixel(pixel.0))
                .collect::<Vec<_>>();
            assert!(
                layer.presentation_frame_view().pixels() == expected,
                "cold-start morph source must contain the native launcher"
            );
            let mut intro = crate::launcher_runtime::startup_intro::StartupIntroPlayback::new(&ui)
                .expect("production intro");
            intro
                .begin_launcher_snapshot_preparation(layer.presentation_frame_view().pixels())
                .expect("capture live launcher target");
            let deadline = Instant::now() + Duration::from_secs(10);
            while !intro
                .poll_launcher_snapshot_preparation()
                .expect("prepare morph target")
            {
                assert!(
                    Instant::now() < deadline,
                    "morph target preparation timed out"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            // Advance to the last 60 Hz crossfade frame before handoff.
            for _ in 0..1199 {
                assert!(!intro.note_presented(16_667));
            }
            for slot in 0..2 {
                let mut hidden_pixels = vec![Rgb565Pixel(0); expected.len()];
                intro
                    .render_into(&mut hidden_pixels, slot, 960)
                    .expect("render morph endpoint");
                assert!(
                    hidden_pixels == expected,
                    "particle endpoint must be the live card frame in both hidden slots"
                );
            }
            layer.render_black();
            assert!(intro.restore_handoff_snapshot(layer.target.cached_565_mut()));
            assert_eq!(layer.presentation_frame_view().pixels(), expected);
            window.request_redraw();
            layer.render_custom_home(&window, &cards, false, None);
            assert!(
                layer.presentation_frame_view().pixels() == expected,
                "idle handoff redraw must not blank the launcher"
            );
        })
        .join()
        .expect("cold intro compositor test thread");
    }

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

        let snapshot = target.cached_565().to_vec();
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
    fn navigation_snapshot_uses_the_publications_immutable_preview_backing() {
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
        layer_target.target.cached_565_mut().fill(Rgb565Pixel(0));
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
        assert!(layer_target.target.physical_direct_preview_matches(
            physical,
            layout.output_layout(),
            12,
        ));
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
