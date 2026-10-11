// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral RGB565 composition for CRT screenshot backdrops.

use crate::preview_transition::blend_rgb565_bucket;
#[cfg(feature = "ui-preview")]
use crate::ui_display::UiLayoutGeometry;
use crate::ui_display::{ResolvedOutputRoute, UiDisplay};
use crate::visual_composition::{PreviewFrame, PreviewPixels};
use mister_magik_core::display::CRT_COMPOSITION_H;
use mister_magik_framebuffer_scenes::{
    OutputRotation, Rgb565OutputLayout, Rgb565SurfaceMut, blend_rgb565_neon_if_available,
};
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::{Duration, Instant};

/// Slint-owned transparent foreground. Never populated from a composed RGB565
/// frame: screenshots, native list glyphs and transition pixels cannot enter it.
#[derive(Default)]
pub struct CrtForeground {
    pixels: Vec<slint::platform::software_renderer::PremultipliedRgbaColor>,
    spans: Vec<Vec<(usize, usize)>>,
    width: usize,
}

impl CrtForeground {
    pub fn render(
        &mut self,
        renderer: &slint::platform::software_renderer::SoftwareRenderer,
        width: usize,
        height: usize,
    ) -> slint::platform::software_renderer::PhysicalRegion {
        self.width = width;
        self.pixels.resize(width * height, Default::default());
        self.spans.resize_with(height, Vec::new);
        let region = renderer.render(&mut self.pixels, width);
        for (origin, size) in region.iter() {
            for y in origin.y as usize..(origin.y as usize + size.height as usize).min(height) {
                let row = &self.pixels[y * width..(y + 1) * width];
                let spans = &mut self.spans[y];
                spans.clear();
                let mut x = 0;
                while x < width {
                    if row[x].alpha == 0 {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < width && row[x].alpha != 0 {
                        x += 1;
                    }
                    spans.push((start, x));
                }
            }
        }
        region
    }

    pub fn compose(&self, destination: &mut [Rgb565Pixel]) {
        use slint::platform::software_renderer::TargetPixel;
        if destination.len() != self.pixels.len() {
            return;
        }
        for (y, spans) in self.spans.iter().enumerate() {
            for &(start, end) in spans {
                let range = y * self.width + start..y * self.width + end;
                for (pixel, overlay) in destination[range.clone()]
                    .iter_mut()
                    .zip(&self.pixels[range])
                {
                    pixel.blend(*overlay);
                }
            }
        }
    }
}

/// A prepared, dimmed RGB565 target produced away from the launcher render
/// thread.  The pixel and row-repeat buffers are immutable so adopting a
/// target only clones two `Arc`s; the UI never rescales or copies the source
/// image on the selection-change path.
#[cfg(feature = "ui")]
#[derive(Clone)]
pub(crate) struct PreparedCrtBackdrop {
    pub(crate) pixels: std::sync::Arc<[Rgb565Pixel]>,
    pub(crate) row_repeats: std::sync::Arc<[bool]>,
}

/// Immutable decoded screenshot data handed from the preview cache to the
/// CRT preparation worker.  The controller owns all derived buffers.
#[cfg(feature = "ui")]
#[derive(Clone)]
pub(crate) struct BackdropSource {
    pub(crate) key: String,
    pub(crate) epoch: u64,
    pub(crate) words: std::sync::Arc<[u16]>,
    pub(crate) source_width: usize,
    pub(crate) source_height: usize,
    pub(crate) stride_pixels: usize,
}

pub const CRT_BACKDROP_FADE_DURATION: Duration = Duration::from_millis(130);
pub const CRT_BACKDROP_DARK_RETAIN_PERCENT: u8 = 40;
pub const CRT_BACKDROP_BACKGROUND: Rgb565Pixel = rgb565_from_rgb888(0x02, 0x08, 0x17);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CrtBackdropWorkTrace {
    pub prepare_us: u64,
    pub prepare_pixels: u32,
    pub blend_us: u64,
    pub blend_pixels: u32,
    pub alpha_bucket: u8,
    pub active: bool,
}

pub struct CrtBackdropState {
    width: usize,
    height: usize,
    physical_height: usize,
    reference_height: usize,
    source: Vec<Rgb565Pixel>,
    target: std::sync::Arc<[Rgb565Pixel]>,
    retarget: Vec<Rgb565Pixel>,
    logical_retarget: Vec<Rgb565Pixel>,
    x_map: Vec<usize>,
    y_map: Vec<usize>,
    source_row_repeats: Vec<bool>,
    target_row_repeats: std::sync::Arc<[bool]>,
    retarget_row_repeats: Vec<bool>,
    // Shared plain targets: clearing the backdrop reuses these instead of
    // allocating and copying a whole frame on the UI thread.
    plain_target: std::sync::Arc<[Rgb565Pixel]>,
    plain_target_row_repeats: std::sync::Arc<[bool]>,
    retarget_is_plain: bool,
    transition_started: Option<Duration>,
    pending_prepare_us: u64,
    pending_prepare_pixels: u32,
}

impl CrtBackdropState {
    pub fn for_display(display: &UiDisplay) -> Option<Self> {
        let reference_height = match display.output_route() {
            ResolvedOutputRoute::Crt240p60 | ResolvedOutputRoute::Crt288p50 => CRT_COMPOSITION_H,
            ResolvedOutputRoute::Crt480p60 | ResolvedOutputRoute::Crt576p50 => display.render_h(),
            ResolvedOutputRoute::Hdmi => return None,
        };
        Some(Self::new_with_reference_height(
            display.render_w(),
            display.render_h(),
            display.output_h() as usize,
            reference_height,
        ))
    }

    pub fn new(width: usize, height: usize) -> Self {
        Self::new_with_heights(width, height, height)
    }

    fn new_with_heights(width: usize, height: usize, physical_height: usize) -> Self {
        Self::new_with_reference_height(width, height, physical_height, height)
    }

    fn new_with_reference_height(
        width: usize,
        height: usize,
        physical_height: usize,
        reference_height: usize,
    ) -> Self {
        let len = width.saturating_mul(physical_height);
        let logical_len = width.saturating_mul(height);
        let plain_target: std::sync::Arc<[Rgb565Pixel]> =
            std::sync::Arc::from(vec![CRT_BACKDROP_BACKGROUND; len]);
        let plain_target_row_repeats: std::sync::Arc<[bool]> =
            std::sync::Arc::from(plain_row_repeats(physical_height));
        Self {
            width,
            height,
            physical_height,
            reference_height,
            source: vec![CRT_BACKDROP_BACKGROUND; len],
            target: std::sync::Arc::clone(&plain_target),
            retarget: vec![CRT_BACKDROP_BACKGROUND; len],
            logical_retarget: vec![CRT_BACKDROP_BACKGROUND; logical_len],
            x_map: vec![0; width],
            y_map: vec![0; physical_height],
            source_row_repeats: plain_row_repeats(physical_height),
            target_row_repeats: std::sync::Arc::clone(&plain_target_row_repeats),
            retarget_row_repeats: plain_row_repeats(physical_height),
            plain_target,
            plain_target_row_repeats,
            retarget_is_plain: true,
            transition_started: None,
            pending_prepare_us: 0,
            pending_prepare_pixels: 0,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn physical_height(&self) -> usize {
        self.physical_height
    }

    pub fn reference_height(&self) -> usize {
        self.reference_height
    }

    pub fn pixels(&self) -> &[Rgb565Pixel] {
        if self.height == self.physical_height {
            &self.retarget
        } else {
            &self.logical_retarget
        }
    }

    fn target_is_plain(&self) -> bool {
        std::sync::Arc::ptr_eq(&self.target, &self.plain_target)
    }

    pub fn is_transitioning(&self) -> bool {
        self.transition_started.is_some()
    }

    pub fn retarget_plain(&mut self, now: Duration) {
        self.retarget(None, now);
    }

    pub fn clear_plain(&mut self) {
        if self.transition_started.is_none() && self.target_is_plain() && self.retarget_is_plain {
            self.pending_prepare_us = 0;
            self.pending_prepare_pixels = 0;
            return;
        }
        // `source` is only read during a fade, and every fade first copies
        // `retarget` into it, so a cut to plain leaves it untouched.
        let prepare_start = Instant::now();
        self.target = std::sync::Arc::clone(&self.plain_target);
        self.target_row_repeats = std::sync::Arc::clone(&self.plain_target_row_repeats);
        self.retarget.fill(CRT_BACKDROP_BACKGROUND);
        self.retarget_row_repeats.fill(true);
        self.retarget_is_plain = true;
        self.expand_to_logical();
        self.transition_started = None;
        self.pending_prepare_us = duration_us(prepare_start.elapsed());
        self.pending_prepare_pixels = self.retarget.len().min(u32::MAX as usize) as u32;
    }

    pub fn retarget(&mut self, frame: Option<PreviewFrame<'_>>, now: Duration) {
        self.resolve_current(now);
        self.source.copy_from_slice(&self.retarget);
        self.source_row_repeats
            .copy_from_slice(&self.retarget_row_repeats);

        let prepare_start = Instant::now();
        let mut prepared = vec![CRT_BACKDROP_BACKGROUND; self.target.len()];
        let mut prepared_rows = vec![true; self.physical_height];
        let scaled = frame.is_some_and(|frame| {
            scale_dimmed_center_crop_mapped_with_logical_height(
                &mut prepared,
                self.width,
                self.physical_height,
                self.reference_height,
                frame,
                &mut self.x_map,
                &mut self.y_map,
                &mut prepared_rows,
            )
        });
        if scaled {
            self.target = std::sync::Arc::from(prepared);
            self.target_row_repeats = std::sync::Arc::from(prepared_rows);
        } else {
            self.target = std::sync::Arc::clone(&self.plain_target);
            self.target_row_repeats = std::sync::Arc::clone(&self.plain_target_row_repeats);
        }
        self.pending_prepare_us = duration_us(prepare_start.elapsed());
        self.pending_prepare_pixels = self.target.len().min(u32::MAX as usize) as u32;
        self.transition_started = (self.source.as_slice() != self.target.as_ref()).then_some(now);
        if self.transition_started.is_none() {
            self.retarget.copy_from_slice(&self.target);
            self.retarget_row_repeats
                .copy_from_slice(&self.target_row_repeats);
            self.retarget_is_plain = self.target_is_plain();
            self.expand_to_logical();
        }
    }

    /// Adopt an immutable target prepared by the background lane.  This is
    /// intentionally separate from `retarget`, whose compatibility path still
    /// performs scaling synchronously for host tests and non-Arcade callers.
    #[cfg(feature = "ui")]
    pub(crate) fn retarget_prepared(
        &mut self,
        prepared: PreparedCrtBackdrop,
        now: Duration,
        instant: bool,
    ) {
        if !instant {
            self.resolve_current(now);
            self.source.copy_from_slice(&self.retarget);
            self.source_row_repeats
                .copy_from_slice(&self.retarget_row_repeats);
        }
        self.target = prepared.pixels;
        self.target_row_repeats = prepared.row_repeats;
        self.pending_prepare_us = 0;
        self.pending_prepare_pixels = 0;
        if instant {
            self.snap_to_target();
            return;
        }
        self.transition_started = (self.source != self.target.as_ref()).then_some(now);
        if self.transition_started.is_none() {
            self.retarget.copy_from_slice(&self.target);
            self.retarget_row_repeats
                .copy_from_slice(&self.target_row_repeats);
            self.retarget_is_plain = self.target_is_plain();
            self.expand_to_logical();
        }
    }

    #[cfg(feature = "ui-preview")]
    pub fn retarget_for_layout(
        &mut self,
        frame: Option<PreviewFrame<'_>>,
        now: Duration,
        layout: UiLayoutGeometry,
    ) {
        if layout.output_layout().rotation() == OutputRotation::None {
            self.retarget(frame, now);
            return;
        }
        self.resolve_current(now);
        self.source.copy_from_slice(&self.retarget);
        self.source_row_repeats
            .copy_from_slice(&self.retarget_row_repeats);
        let prepare_start = Instant::now();
        let mut x_map = Vec::new();
        let mut y_map = Vec::new();
        let prepared = frame.and_then(|frame| {
            let PreviewPixels::Rgb565 {
                pixels,
                stride_pixels,
            } = frame.pixels
            else {
                return None;
            };
            prepare_dimmed_rgb565_target_for_output_with_maps(
                pixels,
                frame.source_width,
                frame.source_height,
                stride_pixels,
                layout.output_layout(),
                self.reference_height,
                &mut x_map,
                &mut y_map,
            )
        });
        (self.target, self.target_row_repeats) = prepared.map_or_else(
            || {
                (
                    std::sync::Arc::clone(&self.plain_target),
                    std::sync::Arc::clone(&self.plain_target_row_repeats),
                )
            },
            |(pixels, row_repeats)| {
                (
                    std::sync::Arc::from(pixels),
                    std::sync::Arc::from(row_repeats),
                )
            },
        );
        self.pending_prepare_us = duration_us(prepare_start.elapsed());
        self.pending_prepare_pixels = self.target.len().min(u32::MAX as usize) as u32;
        self.transition_started = (self.source != self.target.as_ref()).then_some(now);
        if self.transition_started.is_none() {
            self.retarget.copy_from_slice(&self.target);
            self.retarget_row_repeats
                .copy_from_slice(&self.target_row_repeats);
            self.retarget_is_plain = self.target_is_plain();
            self.expand_to_logical();
        }
    }

    pub fn compose(&mut self, now: Duration) -> CrtBackdropWorkTrace {
        let mut logical_retarget = std::mem::take(&mut self.logical_retarget);
        let trace = self.compose_to(now, &mut logical_retarget);
        self.logical_retarget = logical_retarget;
        if !trace.active {
            self.expand_to_logical();
        }
        trace
    }

    /// Compose directly into an external RGB565 frame. The launcher uses this
    /// path so the logical backdrop does not incur a second full-frame copy.
    pub fn compose_into(
        &mut self,
        now: Duration,
        destination: &mut [Rgb565Pixel],
    ) -> CrtBackdropWorkTrace {
        let trace = self.compose_to(now, destination);
        if !trace.active {
            self.expand_to_logical();
        }
        trace
    }

    fn compose_to(
        &mut self,
        now: Duration,
        destination: &mut [Rgb565Pixel],
    ) -> CrtBackdropWorkTrace {
        let mut trace = CrtBackdropWorkTrace {
            prepare_us: std::mem::take(&mut self.pending_prepare_us),
            prepare_pixels: std::mem::take(&mut self.pending_prepare_pixels),
            ..CrtBackdropWorkTrace::default()
        };
        let Some(started) = self.transition_started else {
            trace.alpha_bucket = 32;
            self.expand_into(destination);
            return trace;
        };
        let elapsed = now.checked_sub(started).unwrap_or_default();
        let numerator = elapsed
            .as_micros()
            .min(CRT_BACKDROP_FADE_DURATION.as_micros());
        let denominator = CRT_BACKDROP_FADE_DURATION.as_micros().max(1);
        let alpha_bucket = ((numerator * 32 + denominator / 2) / denominator) as u16;
        if alpha_bucket == 0 {
            // Retargeting snapshots the currently displayed blend into
            // `retarget`. Rewrite the destination even at the alpha-zero
            // endpoint so a frame marked as fresh cannot retain the prior
            // list overlay underneath the new foreground.
            self.expand_into(destination);
            trace.alpha_bucket = 0;
            trace.active = true;
            return trace;
        }
        let blend_start = Instant::now();
        let row_width = self.width.max(1);
        for row in 0..self.physical_height {
            let start = row.saturating_mul(row_width);
            let end = start.saturating_add(row_width).min(self.retarget.len());
            if row > 0 && self.source_row_repeats[row] && self.target_row_repeats[row] {
                let (before, current) = self.retarget.split_at_mut(start);
                current[..end - start].copy_from_slice(&before[start - row_width..start]);
            } else {
                let destination = &mut self.retarget[start..end];
                let previous = &self.source[start..end];
                let current = &self.target[start..end];
                blend_rgb565_range(destination, previous, current, alpha_bucket);
            }
        }
        for row in 0..self.physical_height {
            self.retarget_row_repeats[row] =
                row > 0 && self.source_row_repeats[row] && self.target_row_repeats[row];
        }
        trace.blend_us = duration_us(blend_start.elapsed());
        trace.blend_pixels = self.retarget.len().min(u32::MAX as usize) as u32;
        trace.alpha_bucket = alpha_bucket.min(32) as u8;
        trace.active = alpha_bucket < 32;
        if !trace.active {
            self.transition_started = None;
            self.retarget_is_plain = self.target_is_plain();
        } else {
            self.retarget_is_plain = false;
        }
        self.expand_into(destination);
        trace
    }

    fn expand_into(&self, destination: &mut [Rgb565Pixel]) {
        let required = self.width.saturating_mul(self.height);
        if destination.len() < required {
            return;
        }
        if self.height == self.physical_height {
            destination[..required].copy_from_slice(&self.retarget);
            return;
        }
        for logical_y in 0..self.height {
            let physical_y = logical_y
                .saturating_mul(self.physical_height)
                .checked_div(self.height)
                .unwrap_or(0)
                .min(self.physical_height.saturating_sub(1));
            let logical_start = logical_y * self.width;
            let physical_start = physical_y * self.width;
            destination[logical_start..logical_start + self.width]
                .copy_from_slice(&self.retarget[physical_start..physical_start + self.width]);
        }
    }

    fn expand_to_logical(&mut self) {
        if self.height == self.physical_height {
            return;
        }
        for logical_y in 0..self.height {
            let physical_y = logical_y
                .saturating_mul(self.physical_height)
                .checked_div(self.height)
                .unwrap_or(0)
                .min(self.physical_height.saturating_sub(1));
            let logical_start = logical_y * self.width;
            let physical_start = physical_y * self.width;
            self.logical_retarget[logical_start..logical_start + self.width]
                .copy_from_slice(&self.retarget[physical_start..physical_start + self.width]);
        }
    }

    fn resolve_current(&mut self, now: Duration) {
        if self.transition_started.is_some() {
            let _ = self.compose(now);
        }
    }

    #[cfg(feature = "ui")]
    fn snap_to_target(&mut self) {
        self.retarget.copy_from_slice(&self.target);
        self.retarget_row_repeats
            .copy_from_slice(&self.target_row_repeats);
        self.retarget_is_plain = self.target_is_plain();
        self.transition_started = None;
        self.expand_to_logical();
    }
}

#[cfg(test)]
fn scale_dimmed_center_crop(
    destination: &mut [Rgb565Pixel],
    destination_width: usize,
    destination_height: usize,
    frame: PreviewFrame<'_>,
) -> bool {
    let mut x_map = vec![0; destination_width];
    let mut y_map = vec![0; destination_height];
    let mut row_repeats = vec![false; destination_height];
    scale_dimmed_center_crop_mapped(
        destination,
        destination_width,
        destination_height,
        frame,
        &mut x_map,
        &mut y_map,
        &mut row_repeats,
    )
}

#[cfg(test)]
fn scale_dimmed_center_crop_mapped(
    destination: &mut [Rgb565Pixel],
    destination_width: usize,
    destination_height: usize,
    frame: PreviewFrame<'_>,
    x_map: &mut [usize],
    y_map: &mut [usize],
    row_repeats: &mut [bool],
) -> bool {
    scale_dimmed_center_crop_mapped_with_logical_height(
        destination,
        destination_width,
        destination_height,
        destination_height,
        frame,
        x_map,
        y_map,
        row_repeats,
    )
}

#[allow(clippy::too_many_arguments)]
fn scale_dimmed_center_crop_mapped_with_logical_height(
    destination: &mut [Rgb565Pixel],
    destination_width: usize,
    destination_height: usize,
    reference_destination_height: usize,
    frame: PreviewFrame<'_>,
    x_map: &mut [usize],
    y_map: &mut [usize],
    row_repeats: &mut [bool],
) -> bool {
    scale_dimmed_center_crop_mapped_with_logical_size(
        destination,
        destination_width,
        destination_height,
        destination_width,
        reference_destination_height,
        frame,
        x_map,
        y_map,
        row_repeats,
    )
}

#[allow(clippy::too_many_arguments)]
fn scale_dimmed_center_crop_mapped_with_logical_size(
    destination: &mut [Rgb565Pixel],
    destination_width: usize,
    destination_height: usize,
    reference_destination_width: usize,
    reference_destination_height: usize,
    frame: PreviewFrame<'_>,
    x_map: &mut [usize],
    y_map: &mut [usize],
    row_repeats: &mut [bool],
) -> bool {
    if destination_width == 0
        || destination_height == 0
        || reference_destination_width == 0
        || reference_destination_height == 0
        || destination.len() < destination_width.saturating_mul(destination_height)
        || x_map.len() < destination_width
        || y_map.len() < destination_height
        || row_repeats.len() < destination_height
        || frame.source_width == 0
        || frame.source_height == 0
    {
        return false;
    }
    let valid = match frame.pixels {
        PreviewPixels::Empty => return false,
        PreviewPixels::Rgb565 {
            pixels,
            stride_pixels,
        } => {
            stride_pixels >= frame.source_width
                && frame
                    .source_height
                    .saturating_sub(1)
                    .saturating_mul(stride_pixels)
                    .saturating_add(frame.source_width)
                    <= pixels.len()
        }
        PreviewPixels::Rgb8(pixels) => {
            frame
                .source_width
                .saturating_mul(frame.source_height)
                .saturating_mul(3)
                <= pixels.len()
        }
    };
    if !valid {
        return false;
    }

    // The low-resolution CRT routes share a 640x480 visual reference while
    // preparing their native 240- or 288-row backdrops. Every screenshot is
    // doubled with nearest-neighbour sampling, centred, and clipped to that
    // reference instead of changing scale according to its orientation.
    let integer_scale_route = reference_destination_width > destination_width
        || reference_destination_height > destination_height;
    if integer_scale_route {
        let integer_scale = 2;
        let scaled_width = frame.source_width.saturating_mul(integer_scale);
        let image_x = (reference_destination_width as isize - scaled_width as isize) / 2;
        for (destination_x, source_x) in x_map[..destination_width].iter_mut().enumerate() {
            let logical_x = destination_x
                .saturating_mul(2)
                .saturating_add(1)
                .saturating_mul(reference_destination_width)
                / destination_width.saturating_mul(2).max(1);
            let local_x = logical_x as isize - image_x;
            *source_x = if local_x >= 0 && local_x < scaled_width as isize {
                local_x as usize / integer_scale
            } else {
                usize::MAX
            };
        }

        let scaled_logical_height = frame.source_height.saturating_mul(integer_scale);
        let image_y = (reference_destination_height as isize - scaled_logical_height as isize) / 2;
        for (destination_y, source_y) in y_map[..destination_height].iter_mut().enumerate() {
            let logical_y = destination_y
                .saturating_mul(2)
                .saturating_add(1)
                .saturating_mul(reference_destination_height)
                / destination_height.saturating_mul(2).max(1);
            let local_y = logical_y as isize - image_y;
            *source_y = if local_y >= 0 && local_y < scaled_logical_height as isize {
                local_y as usize / integer_scale
            } else {
                usize::MAX
            };
        }
    } else {
        let (crop_x, crop_y, crop_width, crop_height) = center_crop_to_aspect(
            frame.source_width,
            frame.source_height,
            reference_destination_width,
            reference_destination_height,
        );
        for (destination_x, source_x) in x_map[..destination_width].iter_mut().enumerate() {
            *source_x = crop_x
                + (destination_x.saturating_mul(crop_width) / destination_width)
                    .min(crop_width - 1);
        }
        for (destination_y, source_y) in y_map[..destination_height].iter_mut().enumerate() {
            *source_y = crop_y
                + (destination_y.saturating_mul(crop_height) / destination_height)
                    .min(crop_height - 1);
        }
    }
    for destination_y in 0..destination_height {
        row_repeats[destination_y] =
            destination_y > 0 && y_map[destination_y - 1] == y_map[destination_y];
        let destination_start = destination_y * destination_width;
        if row_repeats[destination_y] {
            let (before, current) = destination.split_at_mut(destination_start);
            current[..destination_width]
                .copy_from_slice(&before[destination_start - destination_width..destination_start]);
            continue;
        }
        let source_y = y_map[destination_y];
        if source_y == usize::MAX {
            destination[destination_start..destination_start + destination_width]
                .fill(CRT_BACKDROP_BACKGROUND);
            continue;
        }
        let mut previous_source_x = usize::MAX;
        let mut previous_pixel = CRT_BACKDROP_BACKGROUND;
        for (destination_x, source_x) in x_map[..destination_width].iter().copied().enumerate() {
            if source_x == previous_source_x {
                destination[destination_start + destination_x] = previous_pixel;
                continue;
            }
            let pixel = if source_x == usize::MAX {
                CRT_BACKDROP_BACKGROUND
            } else {
                let source = match frame.pixels {
                    PreviewPixels::Empty => CRT_BACKDROP_BACKGROUND,
                    PreviewPixels::Rgb565 {
                        pixels,
                        stride_pixels,
                    } => pixels[source_y * stride_pixels + source_x],
                    PreviewPixels::Rgb8(pixels) => {
                        let index = (source_y * frame.source_width + source_x) * 3;
                        rgb565_from_rgb888(pixels[index], pixels[index + 1], pixels[index + 2])
                    }
                };
                darken_rgb565(source)
            };
            previous_pixel = pixel;
            previous_source_x = source_x;
            destination[destination_start + destination_x] = previous_pixel;
        }
    }
    true
}

/// Prepare one RGB565 screenshot for a CRT backdrop. This
/// helper is deliberately allocation-owned by its caller so it can run on a
/// worker lane and hand the resulting buffers to `CrtBackdropState` by `Arc`.
/// Worker-friendly variant that reuses the nearest-neighbour maps between
/// requests. The destination remains request-owned because it is handed to
/// the backdrop cache, while the maps are pure scratch state.
#[cfg(any(feature = "ui", feature = "ui-preview"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_dimmed_rgb565_target_with_maps(
    source: &[Rgb565Pixel],
    source_width: usize,
    source_height: usize,
    source_stride_pixels: usize,
    destination_width: usize,
    destination_physical_height: usize,
    reference_destination_height: usize,
    x_map: &mut Vec<usize>,
    y_map: &mut Vec<usize>,
) -> Option<(Vec<Rgb565Pixel>, Vec<bool>)> {
    if source_width == 0
        || source_height == 0
        || source_stride_pixels < source_width
        || source.len() < source_stride_pixels.saturating_mul(source_height)
    {
        return None;
    }
    let mut pixels = vec![
        CRT_BACKDROP_BACKGROUND;
        destination_width.saturating_mul(destination_physical_height)
    ];
    let mut row_repeats = vec![false; destination_physical_height];
    let frame = PreviewFrame {
        pixels: PreviewPixels::Rgb565 {
            pixels: source,
            stride_pixels: source_stride_pixels,
        },
        source_width,
        source_height,
        display_width: source_width,
        display_height: source_height,
    };
    x_map.resize(destination_width, 0);
    y_map.resize(destination_physical_height, 0);
    scale_dimmed_center_crop_mapped_with_logical_height(
        &mut pixels,
        destination_width,
        destination_physical_height,
        reference_destination_height,
        frame,
        x_map,
        y_map,
        &mut row_repeats,
    )
    .then_some((pixels, row_repeats))
}

#[cfg(any(feature = "ui", feature = "ui-preview"))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_dimmed_rgb565_target_for_output_with_maps(
    source: &[Rgb565Pixel],
    source_width: usize,
    source_height: usize,
    source_stride_pixels: usize,
    output: Rgb565OutputLayout,
    reference_destination_height: usize,
    x_map: &mut Vec<usize>,
    y_map: &mut Vec<usize>,
) -> Option<(Vec<Rgb565Pixel>, Vec<bool>)> {
    if output.rotation() == OutputRotation::None {
        return prepare_dimmed_rgb565_target_with_maps(
            source,
            source_width,
            source_height,
            source_stride_pixels,
            output.logical_width(),
            output.logical_height(),
            reference_destination_height,
            x_map,
            y_map,
        );
    }
    let mut logical = vec![
        CRT_BACKDROP_BACKGROUND;
        output
            .logical_width()
            .saturating_mul(output.logical_height())
    ];
    let mut logical_row_repeats = vec![false; output.logical_height()];
    let frame = PreviewFrame {
        pixels: PreviewPixels::Rgb565 {
            pixels: source,
            stride_pixels: source_stride_pixels,
        },
        source_width,
        source_height,
        display_width: source_width,
        display_height: source_height,
    };
    x_map.resize(output.logical_width(), 0);
    y_map.resize(output.logical_height(), 0);
    // A rotated low-resolution CRT buffer maps its narrow logical width through
    // the shared 640x480 visual reference for strict 2x nearest-neighbour
    // pixels. Progressive and identity-sized outputs use their native logical
    // aspect when choosing the undistorted centre crop.
    let reference_destination_width = reference_destination_height.max(output.logical_width());
    if !scale_dimmed_center_crop_mapped_with_logical_size(
        &mut logical,
        output.logical_width(),
        output.logical_height(),
        reference_destination_width,
        output.logical_height(),
        frame,
        x_map,
        y_map,
        &mut logical_row_repeats,
    ) {
        return None;
    }
    let mut physical = vec![CRT_BACKDROP_BACKGROUND; output.len()];
    let copied = Rgb565SurfaceMut::new(&mut physical, output)
        .expect("prepared CRT output length matches its layout")
        .copy_rect_strided(
            0,
            0,
            output.logical_width(),
            output.logical_height(),
            &logical,
            output.logical_width(),
            0,
            0,
        );
    debug_assert!(copied, "complete CRT backdrop rotation is in bounds");
    if !copied {
        return None;
    }
    let mut row_repeats = vec![false; output.physical_height()];
    for (row, repeats) in row_repeats.iter_mut().enumerate().skip(1) {
        let previous = (row - 1) * output.physical_stride();
        let current = row * output.physical_stride();
        *repeats = physical[previous..previous + output.physical_width()]
            == physical[current..current + output.physical_width()];
    }
    Some((physical, row_repeats))
}

fn center_crop_to_aspect(
    width: usize,
    height: usize,
    target_width: usize,
    target_height: usize,
) -> (usize, usize, usize, usize) {
    if width.saturating_mul(target_height) > height.saturating_mul(target_width) {
        let crop_width = (height.saturating_mul(target_width) / target_height)
            .max(1)
            .min(width);
        ((width - crop_width) / 2, 0, crop_width, height)
    } else {
        let crop_height = (width.saturating_mul(target_height) / target_width)
            .max(1)
            .min(height);
        (0, (height - crop_height) / 2, width, crop_height)
    }
}

fn darken_rgb565(pixel: Rgb565Pixel) -> Rgb565Pixel {
    let red = DARKEN_5[((pixel.0 >> 11) & 0x1f) as usize];
    let green = DARKEN_6[((pixel.0 >> 5) & 0x3f) as usize];
    let blue = DARKEN_5[(pixel.0 & 0x1f) as usize];
    Rgb565Pixel((red << 11) | (green << 5) | blue)
}

const DARKEN_5: [u16; 32] = darken_table_5();
const DARKEN_6: [u16; 64] = darken_table_6();

const fn darken_table_5() -> [u16; 32] {
    let mut table = [0; 32];
    let mut value = 0;
    while value < table.len() {
        table[value] = value as u16 * CRT_BACKDROP_DARK_RETAIN_PERCENT as u16 / 100;
        value += 1;
    }
    table
}

const fn darken_table_6() -> [u16; 64] {
    let mut table = [0; 64];
    let mut value = 0;
    while value < table.len() {
        table[value] = value as u16 * CRT_BACKDROP_DARK_RETAIN_PERCENT as u16 / 100;
        value += 1;
    }
    table
}

fn plain_row_repeats(height: usize) -> Vec<bool> {
    (0..height).map(|row| row > 0).collect()
}

const fn rgb565_from_rgb888(r: u8, g: u8, b: u8) -> Rgb565Pixel {
    Rgb565Pixel(((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3))
}

fn blend_rgb565_range(
    destination: &mut [Rgb565Pixel],
    previous: &[Rgb565Pixel],
    current: &[Rgb565Pixel],
    alpha_bucket: u16,
) {
    let end = destination.len().min(previous.len()).min(current.len());
    if blend_rgb565_neon_if_available(destination, previous, current, 0, end, alpha_bucket) {
        return;
    }
    let mut previous_source = Rgb565Pixel(u16::MAX);
    let mut previous_current = Rgb565Pixel(u16::MAX);
    for index in 0..end {
        if index > 0 && previous[index] == previous_source && current[index] == previous_current {
            destination[index] = destination[index - 1];
        } else {
            destination[index] = blend_rgb565_bucket(previous[index], current[index], alpha_bucket);
        }
        previous_source = previous[index];
        previous_current = current[index];
    }
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_display::{UiDisplayPlan, UiFramebufferSizePolicy};
    use mister_magik_core::display::{Crt240Composition, DisplayGeometry};

    fn frame<'a>(pixels: &'a [Rgb565Pixel], width: usize, height: usize) -> PreviewFrame<'a> {
        PreviewFrame {
            pixels: PreviewPixels::Rgb565 {
                pixels,
                stride_pixels: width,
            },
            source_width: width,
            source_height: height,
            display_width: width,
            display_height: height,
        }
    }

    #[cfg(feature = "ui")]
    #[test]
    fn prepared_four_corner_pattern_follows_both_output_rotations() {
        let red = Rgb565Pixel(0xf800);
        let green = Rgb565Pixel(0x07e0);
        let blue = Rgb565Pixel(0x001f);
        let white = Rgb565Pixel(0xffff);
        let source = [
            red,
            Rgb565Pixel(0),
            Rgb565Pixel(0),
            white,
            Rgb565Pixel(0),
            Rgb565Pixel(0),
            Rgb565Pixel(0),
            Rgb565Pixel(0),
            green,
            Rgb565Pixel(0),
            Rgb565Pixel(0),
            blue,
        ];
        for rotation in [
            OutputRotation::Clockwise90,
            OutputRotation::CounterClockwise90,
        ] {
            let output = Rgb565OutputLayout::new(4, 3, 3, rotation).unwrap();
            let (prepared, _) = prepare_dimmed_rgb565_target_for_output_with_maps(
                &source,
                4,
                3,
                4,
                output,
                3,
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .unwrap();
            for (x, y, expected) in [(0, 0, red), (3, 0, white), (0, 2, green), (3, 2, blue)] {
                assert_eq!(
                    prepared[output.physical_offset(x, y)],
                    darken_rgb565(expected),
                    "logical corner ({x}, {y}) for {rotation:?}"
                );
            }
        }
    }

    #[cfg(feature = "ui")]
    #[test]
    fn rotated_crt_shared_kernel_matches_scalar_scatter() {
        let source_width = 11;
        let source_height = 7;
        let source_stride = 13;
        let mut source = vec![Rgb565Pixel(0); source_stride * source_height];
        for y in 0..source_height {
            for x in 0..source_width {
                source[y * source_stride + x] =
                    Rgb565Pixel(((x * 0x421 + y * 0x1083) & 0xffff) as u16);
            }
        }
        let source_frame = PreviewFrame {
            pixels: PreviewPixels::Rgb565 {
                pixels: &source,
                stride_pixels: source_stride,
            },
            source_width,
            source_height,
            display_width: source_width,
            display_height: source_height,
        };

        for logical_width in [7, 9] {
            for rotation in [
                OutputRotation::Clockwise90,
                OutputRotation::CounterClockwise90,
            ] {
                let output = Rgb565OutputLayout::new(logical_width, 13, 13, rotation).unwrap();
                let mut logical =
                    vec![CRT_BACKDROP_BACKGROUND; output.logical_width() * output.logical_height()];
                let mut logical_row_repeats = vec![false; output.logical_height()];
                assert!(scale_dimmed_center_crop_mapped_with_logical_size(
                    &mut logical,
                    output.logical_width(),
                    output.logical_height(),
                    13,
                    output.logical_height(),
                    source_frame,
                    &mut vec![0; output.logical_width()],
                    &mut vec![0; output.logical_height()],
                    &mut logical_row_repeats,
                ));
                let mut expected = vec![CRT_BACKDROP_BACKGROUND; output.len()];
                for logical_y in 0..output.logical_height() {
                    for logical_x in 0..output.logical_width() {
                        expected[output.physical_offset(logical_x, logical_y)] =
                            logical[logical_y * output.logical_width() + logical_x];
                    }
                }

                let (prepared, row_repeats) = prepare_dimmed_rgb565_target_for_output_with_maps(
                    &source,
                    source_width,
                    source_height,
                    source_stride,
                    output,
                    13,
                    &mut Vec::new(),
                    &mut Vec::new(),
                )
                .unwrap();
                assert_eq!(prepared, expected, "{logical_width} {rotation:?}");
                for (row, &repeats) in row_repeats.iter().enumerate().skip(1) {
                    let previous = (row - 1) * output.physical_stride();
                    let current = row * output.physical_stride();
                    assert_eq!(
                        repeats,
                        expected[previous..previous + output.physical_width()]
                            == expected[current..current + output.physical_width()],
                        "row {row} for {logical_width} {rotation:?}"
                    );
                }
            }
        }
    }

    #[cfg(feature = "ui")]
    #[test]
    fn rotated_crt_sources_use_two_x_reference_without_crop_or_stretch() {
        for logical_width in [240, 288] {
            for rotation in [
                OutputRotation::Clockwise90,
                OutputRotation::CounterClockwise90,
            ] {
                let output = Rgb565OutputLayout::new(logical_width, 640, 640, rotation).unwrap();
                let mut x_map = Vec::new();
                let mut y_map = Vec::new();

                let portrait = vec![Rgb565Pixel(0xffff); 240 * 320];
                prepare_dimmed_rgb565_target_for_output_with_maps(
                    &portrait,
                    240,
                    320,
                    240,
                    output,
                    CRT_COMPOSITION_H,
                    &mut x_map,
                    &mut y_map,
                )
                .unwrap();
                assert_eq!(x_map[0], 0, "portrait left edge for {output:?}");
                assert_eq!(x_map[logical_width - 1], 239, "portrait right edge");
                assert_eq!(&y_map[..2], &[0, 0], "portrait top rows");
                assert_eq!(&y_map[638..], &[319, 319], "portrait bottom rows");

                let landscape = vec![Rgb565Pixel(0xffff); 320 * 224];
                prepare_dimmed_rgb565_target_for_output_with_maps(
                    &landscape,
                    320,
                    224,
                    320,
                    output,
                    CRT_COMPOSITION_H,
                    &mut x_map,
                    &mut y_map,
                )
                .unwrap();
                assert_eq!(x_map[0], 40, "landscape clipped left edge");
                assert_eq!(
                    x_map[logical_width - 1],
                    279,
                    "landscape clipped right edge"
                );
                assert_eq!(y_map[95], usize::MAX, "landscape upper matte");
                assert_eq!(&y_map[96..98], &[0, 0], "landscape first doubled row");
                assert_eq!(&y_map[542..544], &[223, 223], "landscape last doubled row");
                assert_eq!(y_map[544], usize::MAX, "landscape lower matte");
            }
        }
    }

    #[cfg(feature = "ui")]
    #[test]
    fn rotated_progressive_crt_sources_crop_to_portrait_aspect_without_stretch() {
        for logical_width in [480, 576] {
            for rotation in [
                OutputRotation::Clockwise90,
                OutputRotation::CounterClockwise90,
            ] {
                let output = Rgb565OutputLayout::new(logical_width, 640, 640, rotation).unwrap();
                let mut x_map = Vec::new();
                let mut y_map = Vec::new();

                let landscape = vec![Rgb565Pixel(0xffff); 320 * 224];
                prepare_dimmed_rgb565_target_for_output_with_maps(
                    &landscape,
                    320,
                    224,
                    320,
                    output,
                    logical_width,
                    &mut x_map,
                    &mut y_map,
                )
                .unwrap();
                let (crop_x, crop_y, crop_width, crop_height) =
                    center_crop_to_aspect(320, 224, logical_width, 640);
                assert_eq!(x_map[0], crop_x);
                assert_eq!(x_map[logical_width - 1], crop_x + crop_width - 1);
                assert_eq!(y_map[0], crop_y);
                assert_eq!(y_map[639], crop_y + crop_height - 1);

                let portrait = vec![Rgb565Pixel(0xffff); 187 * 320];
                prepare_dimmed_rgb565_target_for_output_with_maps(
                    &portrait,
                    187,
                    320,
                    187,
                    output,
                    logical_width,
                    &mut x_map,
                    &mut y_map,
                )
                .unwrap();
                let (crop_x, crop_y, crop_width, crop_height) =
                    center_crop_to_aspect(187, 320, logical_width, 640);
                assert_eq!(x_map[0], crop_x);
                assert_eq!(x_map[logical_width - 1], crop_x + crop_width - 1);
                assert_eq!(y_map[0], crop_y);
                assert_eq!(y_map[639], crop_y + crop_height - 1);
            }
        }
    }

    #[test]
    fn center_crop_matches_landscape_and_portrait_targets_without_distortion() {
        assert_eq!(center_crop_to_aspect(1600, 900, 4, 3), (200, 0, 1200, 900));
        assert_eq!(center_crop_to_aspect(600, 800, 4, 3), (0, 175, 600, 450));
        assert_eq!(center_crop_to_aspect(640, 480, 4, 3), (0, 0, 640, 480));
        assert_eq!(center_crop_to_aspect(320, 224, 4, 3), (11, 0, 298, 224));
        assert_eq!(center_crop_to_aspect(320, 224, 3, 4), (76, 0, 168, 224));
        assert_eq!(center_crop_to_aspect(187, 320, 3, 4), (0, 35, 187, 249));
    }

    #[test]
    fn scaling_retains_forty_percent_of_each_rgb565_channel() {
        let source = [Rgb565Pixel(0xffff)];
        let mut output = [Rgb565Pixel(0)];
        assert!(scale_dimmed_center_crop(
            &mut output,
            1,
            1,
            frame(&source, 1, 1)
        ));
        assert_eq!(output[0], Rgb565Pixel((12 << 11) | (25 << 5) | 12));
    }

    #[test]
    fn mapped_scaling_reuses_nearest_neighbour_rows_and_columns() {
        let source = [
            Rgb565Pixel(0xf800),
            Rgb565Pixel(0x07e0),
            Rgb565Pixel(0x0000),
            Rgb565Pixel(0x001f),
            Rgb565Pixel(0xffff),
            Rgb565Pixel(0x0000),
        ];
        let mut output = [Rgb565Pixel(0); 16];
        let mut x_map = [0; 4];
        let mut y_map = [0; 4];
        let mut row_repeats = [false; 4];
        assert!(scale_dimmed_center_crop_mapped(
            &mut output,
            4,
            4,
            frame(&source, 3, 2),
            &mut x_map,
            &mut y_map,
            &mut row_repeats,
        ));
        assert_eq!(x_map, [0, 0, 1, 1]);
        assert_eq!(y_map, [0, 0, 1, 1]);
        assert_eq!(row_repeats, [false, true, false, true]);
        assert_eq!(
            output,
            [
                darken_rgb565(source[0]),
                darken_rgb565(source[0]),
                darken_rgb565(source[1]),
                darken_rgb565(source[1]),
                darken_rgb565(source[0]),
                darken_rgb565(source[0]),
                darken_rgb565(source[1]),
                darken_rgb565(source[1]),
                darken_rgb565(source[3]),
                darken_rgb565(source[3]),
                darken_rgb565(source[4]),
                darken_rgb565(source[4]),
                darken_rgb565(source[3]),
                darken_rgb565(source[3]),
                darken_rgb565(source[4]),
                darken_rgb565(source[4]),
            ]
        );
    }

    #[test]
    fn every_crt_route_constructs_a_backdrop_at_its_output_geometry() {
        for (route, geometry, composition, expected, reference_height) in [
            (
                ResolvedOutputRoute::Crt240p60,
                DisplayGeometry::new(640, 240),
                Crt240Composition::Native240,
                (640, 240, 240),
                CRT_COMPOSITION_H,
            ),
            (
                ResolvedOutputRoute::Crt240p60,
                DisplayGeometry::new(640, 240),
                Crt240Composition::Legacy480,
                (640, 480, 240),
                CRT_COMPOSITION_H,
            ),
            (
                ResolvedOutputRoute::Crt288p50,
                DisplayGeometry::new(640, 288),
                Crt240Composition::Native240,
                (640, 288, 288),
                CRT_COMPOSITION_H,
            ),
            (
                ResolvedOutputRoute::Crt480p60,
                DisplayGeometry::new(640, 480),
                Crt240Composition::Native240,
                (640, 480, 480),
                480,
            ),
            (
                ResolvedOutputRoute::Crt576p50,
                DisplayGeometry::new(640, 576),
                Crt240Composition::Native240,
                (640, 576, 576),
                576,
            ),
        ] {
            let plan = UiDisplayPlan::from_geometry_with_route_and_composition(
                geometry,
                route,
                "test-backdrop-route",
                UiFramebufferSizePolicy::Auto,
                composition,
            );
            let display = UiDisplay::for_plan(plan);
            let backdrop = CrtBackdropState::for_display(&display).unwrap();
            assert_eq!(
                (
                    backdrop.width(),
                    backdrop.height(),
                    backdrop.physical_height()
                ),
                expected
            );
            assert_eq!(backdrop.reference_height(), reference_height);
            assert_eq!(backdrop.pixels().len(), expected.0 * expected.1);
        }
    }

    #[test]
    fn low_resolution_targets_double_and_clip_every_source() {
        let white = Rgb565Pixel(0xffff);
        for (
            destination_height,
            source_width,
            source_height,
            integer_scale,
            expected_x0,
            expected_x1,
            expected_physical_y0,
            expected_physical_y1,
            expected_source_x0,
            expected_source_x1,
            expected_source_y0,
            expected_source_y1,
        ) in [
            (240, 320, 224, 2, 0, 640, 8, 232, 0, 319, 0, 223),
            (240, 187, 320, 2, 133, 507, 0, 240, 0, 186, 40, 279),
            (240, 280, 320, 2, 40, 600, 0, 240, 0, 279, 40, 279),
            (288, 320, 224, 2, 0, 640, 10, 278, 0, 319, 0, 223),
            (288, 187, 320, 2, 133, 507, 0, 288, 0, 186, 40, 279),
            (288, 280, 320, 2, 40, 600, 0, 288, 0, 279, 40, 279),
        ] {
            let source = vec![white; source_width * source_height];
            let mut destination = vec![CRT_BACKDROP_BACKGROUND; 640 * destination_height];
            let mut x_map = vec![0; 640];
            let mut y_map = vec![0; destination_height];
            let mut row_repeats = vec![false; destination_height];

            assert!(scale_dimmed_center_crop_mapped_with_logical_height(
                &mut destination,
                640,
                destination_height,
                CRT_COMPOSITION_H,
                frame(&source, source_width, source_height),
                &mut x_map,
                &mut y_map,
                &mut row_repeats,
            ));

            if expected_x0 > 0 {
                assert_eq!(x_map[expected_x0 - 1], usize::MAX);
            }
            assert_eq!(x_map[expected_x0], expected_source_x0);
            assert_eq!(x_map[expected_x0 + integer_scale - 1], expected_source_x0);
            assert_eq!(x_map[expected_x1 - integer_scale], expected_source_x1);
            assert_eq!(x_map[expected_x1 - 1], expected_source_x1);
            if expected_x1 < x_map.len() {
                assert_eq!(x_map[expected_x1], usize::MAX);
            }
            if expected_physical_y0 > 0 {
                assert_eq!(y_map[expected_physical_y0 - 1], usize::MAX);
            }
            assert_eq!(y_map[expected_physical_y0], expected_source_y0);
            assert_eq!(y_map[expected_physical_y1 - 1], expected_source_y1);
            if expected_physical_y1 < y_map.len() {
                assert_eq!(y_map[expected_physical_y1], usize::MAX);
            }

            let first_image_row = expected_physical_y0 * 640;
            if expected_x0 > 0 {
                assert_eq!(
                    destination[first_image_row + expected_x0 - 1],
                    CRT_BACKDROP_BACKGROUND
                );
            }
            assert_eq!(
                destination[first_image_row + expected_x0],
                darken_rgb565(white)
            );
            assert_eq!(
                destination[first_image_row + expected_x1 - 1],
                darken_rgb565(white)
            );
            if expected_x1 < 640 {
                assert_eq!(
                    destination[first_image_row + expected_x1],
                    CRT_BACKDROP_BACKGROUND
                );
            }
        }
    }

    #[test]
    fn shared_product_composition_keeps_adjacent_source_rows_distinct() {
        for (route, geometry, first_row, second_row) in [
            (
                ResolvedOutputRoute::Crt240p60,
                DisplayGeometry::new(640, 240),
                119,
                120,
            ),
            (
                ResolvedOutputRoute::Crt288p50,
                DisplayGeometry::new(640, 288),
                143,
                144,
            ),
        ] {
            let display =
                UiDisplay::for_plan(UiDisplayPlan::from_geometry_with_route_and_composition(
                    geometry,
                    route,
                    "test-product-backdrop",
                    UiFramebufferSizePolicy::Auto,
                    Crt240Composition::Native240,
                ));
            let source = [
                Rgb565Pixel(0xf800),
                Rgb565Pixel(0xf800),
                Rgb565Pixel(0x001f),
                Rgb565Pixel(0x001f),
            ];
            let mut backdrop = CrtBackdropState::for_display(&display).unwrap();
            backdrop.retarget(Some(frame(&source, 2, 2)), Duration::ZERO);
            let mut destination =
                vec![CRT_BACKDROP_BACKGROUND; display.render_w() * display.render_h()];

            let trace = backdrop.compose_into(CRT_BACKDROP_FADE_DURATION, &mut destination);

            assert!(!trace.active);
            assert_eq!(
                backdrop.retarget[first_row * 640 + 318],
                darken_rgb565(source[0])
            );
            assert_eq!(
                backdrop.retarget[second_row * 640 + 318],
                darken_rgb565(source[2])
            );
        }
    }

    #[test]
    fn fade_has_exact_endpoints_and_rapid_retarget_snapshots_current_blend() {
        let black = [Rgb565Pixel(0); 4];
        let white = [Rgb565Pixel(0xffff); 4];
        let red = [Rgb565Pixel(0xf800); 4];
        let mut backdrop = CrtBackdropState::new(2, 2);

        backdrop.retarget(Some(frame(&white, 2, 2)), Duration::ZERO);
        let start = backdrop.compose(Duration::ZERO);
        assert_eq!(start.alpha_bucket, 0);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == CRT_BACKDROP_BACKGROUND)
        );

        let halfway = Duration::from_millis(65);
        backdrop.compose(halfway);
        let snapshot = backdrop.pixels().to_vec();
        backdrop.retarget(Some(frame(&red, 2, 2)), halfway);
        assert_eq!(backdrop.source, snapshot);

        let end = backdrop.compose(halfway + CRT_BACKDROP_FADE_DURATION);
        assert_eq!(end.alpha_bucket, 32);
        assert!(!end.active);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == darken_rgb565(red[0]))
        );

        backdrop.retarget(Some(frame(&black, 2, 2)), Duration::from_secs(1));
        backdrop.compose(Duration::from_secs(1) + CRT_BACKDROP_FADE_DURATION);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == Rgb565Pixel(0))
        );
        backdrop.retarget_plain(Duration::from_secs(2));
        backdrop.compose(Duration::from_secs(2) + CRT_BACKDROP_FADE_DURATION);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == CRT_BACKDROP_BACKGROUND)
        );
    }

    #[test]
    fn clearing_an_image_reuses_the_shared_plain_target() {
        let white = [Rgb565Pixel(0xffff); 4];
        let mut backdrop = CrtBackdropState::new(2, 2);
        assert!(std::sync::Arc::ptr_eq(
            &backdrop.target,
            &backdrop.plain_target
        ));

        backdrop.retarget(Some(frame(&white, 2, 2)), Duration::ZERO);
        backdrop.compose(CRT_BACKDROP_FADE_DURATION);
        assert!(!std::sync::Arc::ptr_eq(
            &backdrop.target,
            &backdrop.plain_target
        ));

        backdrop.clear_plain();
        assert!(std::sync::Arc::ptr_eq(
            &backdrop.target,
            &backdrop.plain_target
        ));
        assert!(std::sync::Arc::ptr_eq(
            &backdrop.target_row_repeats,
            &backdrop.plain_target_row_repeats
        ));
        assert!(!backdrop.is_transitioning());
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == CRT_BACKDROP_BACKGROUND)
        );
    }

    #[cfg(feature = "ui")]
    #[test]
    fn prepared_targets_can_cut_directly_to_an_image_or_blank() {
        let mut backdrop = CrtBackdropState::new(2, 2);
        let target = Rgb565Pixel(0xf800);
        let unused_source = Rgb565Pixel(0x07e0);
        backdrop.source.fill(unused_source);
        backdrop.retarget.fill(Rgb565Pixel(0x001f));
        backdrop.retarget_prepared(
            PreparedCrtBackdrop {
                pixels: std::sync::Arc::from(vec![target; 4]),
                row_repeats: std::sync::Arc::from(vec![false; 2]),
            },
            Duration::ZERO,
            true,
        );

        assert!(!backdrop.is_transitioning());
        assert!(backdrop.pixels().iter().all(|pixel| *pixel == target));
        assert!(backdrop.source.iter().all(|pixel| *pixel == unused_source));
        assert!(!backdrop.compose(Duration::ZERO).active);

        backdrop.clear_plain();

        assert!(!backdrop.is_transitioning());
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == CRT_BACKDROP_BACKGROUND)
        );
        assert!(!backdrop.compose(Duration::from_millis(1)).active);
    }

    #[test]
    fn physical_rows_are_blended_before_logical_expansion() {
        let target = vec![Rgb565Pixel(0xffff); 8];
        let mut backdrop = CrtBackdropState::new_with_heights(4, 4, 2);
        backdrop.retarget(Some(frame(&target, 4, 2)), Duration::ZERO);
        let marker = Rgb565Pixel(0x1234);
        let mut destination = vec![marker; 16];
        let trace = backdrop.compose_into(Duration::from_millis(65), &mut destination);

        assert!(trace.active);
        assert_ne!(backdrop.retarget[5], CRT_BACKDROP_BACKGROUND);
        for row in 0..4 {
            assert_eq!(
                destination[row * 4 + 1],
                backdrop.retarget[(row / 2) * 4 + 1]
            );
            assert_ne!(destination[row * 4 + 1], marker);
        }
    }

    #[test]
    fn alpha_zero_composition_rewrites_the_backdrop_destination() {
        let target = vec![Rgb565Pixel(0xffff); 8];
        let mut backdrop = CrtBackdropState::new_with_heights(4, 4, 2);
        backdrop.retarget(Some(frame(&target, 4, 2)), Duration::ZERO);
        let marker = Rgb565Pixel(0x1234);
        let mut destination = vec![marker; 16];
        let trace = backdrop.compose_into(Duration::ZERO, &mut destination);

        assert_eq!(trace.alpha_bucket, 0);
        assert!(trace.active);
        assert!(destination.iter().all(|pixel| *pixel != marker));
    }

    #[test]
    fn settled_physical_composition_updates_logical_backdrop_pixels() {
        let target = vec![Rgb565Pixel(0xffff); 8];
        let mut backdrop = CrtBackdropState::new_with_heights(4, 4, 2);
        backdrop.retarget(Some(frame(&target, 4, 2)), Duration::ZERO);
        let mut destination = vec![Rgb565Pixel(0); 16];
        let trace = backdrop.compose_into(CRT_BACKDROP_FADE_DURATION, &mut destination);

        assert_eq!(trace.alpha_bucket, 32);
        assert!(!trace.active);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == darken_rgb565(Rgb565Pixel(0xffff)))
        );
    }

    #[test]
    fn equal_height_backdrop_exposes_physical_pixels_without_duplicate_copy() {
        let mut backdrop = CrtBackdropState::new_with_heights(4, 2, 2);
        backdrop.retarget.fill(Rgb565Pixel(0x1234));
        backdrop.logical_retarget.fill(Rgb565Pixel(0xabcd));

        backdrop.expand_to_logical();

        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == Rgb565Pixel(0x1234))
        );
        assert!(
            backdrop
                .logical_retarget
                .iter()
                .all(|pixel| *pixel == Rgb565Pixel(0xabcd))
        );
    }
}

#[cfg(all(test, feature = "ui"))]
mod foreground_tests {
    use super::*;
    use crate::ui_display::{
        ScreenOrientation, UiDisplayPlan, UiFramebufferSizePolicy, UiLayoutGeometry,
    };
    use mister_magik_core::display::{Crt240Composition, DisplayGeometry};
    use slint::ComponentHandle;
    use slint::platform::software_renderer::{RenderingRotation, TargetPixel};

    slint::slint! {
        export component ForegroundProbe inherits Window {
            in property <int> w: 640;
            in property <int> h: 240;
            in property <int> glyph-x: 38;
            in property <int> glyph-y: 213;
            in property <bool> glyph-visible: true;
            in property <bool> settings: false;
            width: root.w * 1px; height: root.h * 1px;
            background: root.settings ? black : transparent;
            Rectangle { x: 10px; y: 10px; width: 40px; height: 24px; background: #5a71e780; }
            if root.glyph-visible : Rectangle {
                x: root.glyph-x * 1px; y: root.glyph-y * 1px;
                width: 6px; height: 12px; background: #eee8d5;
            }
        }
    }

    #[test]
    fn retained_foreground_tracks_rotated_alpha_and_full_screen_handoffs() {
        for (height, route) in [
            (240, ResolvedOutputRoute::Crt240p60),
            (288, ResolvedOutputRoute::Crt288p50),
        ] {
            for orientation in [
                ScreenOrientation::Normal,
                ScreenOrientation::MonitorClockwise,
                ScreenOrientation::MonitorCounterclockwise,
            ] {
                let window = crate::visual_platform::install_isolated_test_platform();
                let app = ForegroundProbe::new().unwrap();
                let display =
                    UiDisplay::for_plan(UiDisplayPlan::from_geometry_with_route_and_composition(
                        DisplayGeometry::new(640, height),
                        route,
                        "foreground-handoff",
                        UiFramebufferSizePolicy::Auto,
                        Crt240Composition::Native240,
                    ));
                let layout = UiLayoutGeometry::for_display(&display, orientation);
                let output = layout.output_layout();
                window.set_rendering_rotation(match output.rotation() {
                    OutputRotation::None => RenderingRotation::NoRotation,
                    OutputRotation::Clockwise90 => RenderingRotation::Rotate90,
                    OutputRotation::CounterClockwise90 => RenderingRotation::Rotate270,
                });
                app.set_w(layout.logical_w() as i32);
                app.set_h(layout.logical_h() as i32);
                app.set_glyph_x(layout.content_rect().x as i32 + 6);
                app.set_glyph_y(layout.crt_footer_rect().y as i32 + 5);
                window.set_size(slint::PhysicalSize::new(
                    layout.logical_w() as u32,
                    layout.logical_h() as u32,
                ));
                app.show().unwrap();
                let mut foreground = CrtForeground::default();
                let mut completed = vec![Rgb565Pixel(0xf81f); output.len()];
                for color in [
                    Rgb565Pixel(0xf800),
                    Rgb565Pixel(0x07e0),
                    Rgb565Pixel(0x001f),
                ] {
                    app.set_settings(false);
                    app.set_glyph_visible(true);
                    window.request_redraw();
                    window.draw_full_frame_preserving_cache_if_needed(
                        layout.logical_w(),
                        layout.logical_h(),
                        |r| {
                            foreground.render(
                                r,
                                output.physical_stride(),
                                output.physical_height(),
                            );
                        },
                    );
                    completed.fill(color);
                    foreground.compose(&mut completed);
                    let glyph = output
                        .physical_offset(app.get_glyph_x() as usize, app.get_glyph_y() as usize);
                    assert_eq!(
                        completed[glyph],
                        rgb565_from_rgb888(0xee, 0xe8, 0xd5),
                        "height={height}, orientation={orientation:?}, glyph={glyph}, first opaque={:?}",
                        foreground.pixels.iter().position(|p| p.alpha == 255)
                    );
                    assert_eq!(completed[output.physical_offset(100, 20)], color);
                    let alpha = output.physical_offset(20, 20);
                    let mut expected = color;
                    expected.blend(foreground.pixels[alpha]);
                    assert_eq!(completed[alpha], expected);

                    // Full-screen Settings owns an RGB565 raster, while the alpha
                    // layer remains retained separately through transition writes.
                    app.set_settings(true);
                    window.request_redraw();
                    window.draw_full_frame_preserving_cache_if_needed(
                        layout.logical_w(),
                        layout.logical_h(),
                        |r| {
                            r.render(&mut completed, output.physical_stride());
                        },
                    );
                    let settings = completed.clone();
                    completed.fill(Rgb565Pixel(0xffff));
                    window.request_redraw();
                    window.draw_full_frame_preserving_cache_if_needed(
                        layout.logical_w(),
                        layout.logical_h(),
                        |r| {
                            r.render(&mut completed, output.physical_stride());
                        },
                    );
                    assert_eq!(
                        completed, settings,
                        "full-screen release must remove every transition pixel"
                    );
                    app.set_settings(false);
                    app.set_glyph_visible(false);
                    window.request_redraw();
                    window.draw_full_frame_preserving_cache_if_needed(
                        layout.logical_w(),
                        layout.logical_h(),
                        |r| {
                            foreground.render(
                                r,
                                output.physical_stride(),
                                output.physical_height(),
                            );
                        },
                    );
                    completed.fill(color);
                    foreground.compose(&mut completed);
                    assert_eq!(
                        completed[glyph], color,
                        "removed footer glyph must not survive re-entry"
                    );
                    assert_eq!(
                        completed[alpha], expected,
                        "alpha must not accumulate across screens"
                    );
                }
            }
        }
    }
}
