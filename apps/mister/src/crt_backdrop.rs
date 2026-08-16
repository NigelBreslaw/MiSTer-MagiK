// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral RGB565 composition for low-resolution CRT screenshot backdrops.

use crate::preview_transition::blend_rgb565_bucket;
use crate::ui_display::{CrtContentRect, CrtUiMetrics, ResolvedOutputRoute, UiDisplay};
use crate::visual_composition::{PreviewFrame, PreviewPixels};
use mister_magik_core::display::CRT_COMPOSITION_H;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::{Duration, Instant};

/// A prepared, dimmed RGB565 target produced away from the launcher render
/// thread.  The pixel and row-repeat buffers are immutable so adopting a
/// target only clones two `Arc`s; the UI never rescales or copies the source
/// image on the selection-change path.
#[cfg(feature = "ui")]
#[derive(Clone)]
pub(crate) struct PreparedCrtBackdrop {
    pub(crate) pixels: std::sync::Arc<[Rgb565Pixel]>,
    pub(crate) row_repeats: std::sync::Arc<[bool]>,
    pub(crate) is_plain: bool,
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
// Keep these synchronized with the opaque bitmap-font colors in CrtTheme.
pub(crate) const CRT_PRODUCT_HEADER_TEXT: Rgb565Pixel = rgb565_from_rgb888(0xaa, 0xa5, 0xff);
pub(crate) const CRT_PRODUCT_FOOTER_TEXT: Rgb565Pixel = rgb565_from_rgb888(0x8b, 0x94, 0xf8);
const CRT_PRODUCT_WARNING_TEXT: Rgb565Pixel = rgb565_from_rgb888(0xff, 0xd1, 0x66);
const CRT_PRODUCT_TEXT_COLORS: &[Rgb565Pixel] = &[
    CRT_PRODUCT_HEADER_TEXT,
    CRT_PRODUCT_FOOTER_TEXT,
    CRT_PRODUCT_WARNING_TEXT,
];

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
    target_is_plain: bool,
    retarget_is_plain: bool,
    transition_started: Option<Duration>,
    pending_prepare_us: u64,
    pending_prepare_pixels: u32,
}

impl CrtBackdropState {
    pub fn for_display(display: &UiDisplay) -> Option<Self> {
        match display.output_route() {
            ResolvedOutputRoute::Crt240p60 | ResolvedOutputRoute::Crt288p50 => {
                Some(Self::new_with_reference_height(
                    display.render_w(),
                    display.render_h(),
                    display.output_h() as usize,
                    CRT_COMPOSITION_H,
                ))
            }
            _ => None,
        }
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
        Self {
            width,
            height,
            physical_height,
            reference_height,
            source: vec![CRT_BACKDROP_BACKGROUND; len],
            target: std::sync::Arc::from(vec![CRT_BACKDROP_BACKGROUND; len]),
            retarget: vec![CRT_BACKDROP_BACKGROUND; len],
            logical_retarget: vec![CRT_BACKDROP_BACKGROUND; logical_len],
            x_map: vec![0; width],
            y_map: vec![0; physical_height],
            source_row_repeats: plain_row_repeats(physical_height),
            target_row_repeats: std::sync::Arc::from(plain_row_repeats(physical_height)),
            retarget_row_repeats: plain_row_repeats(physical_height),
            target_is_plain: true,
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
        &self.logical_retarget
    }

    pub fn is_transitioning(&self) -> bool {
        self.transition_started.is_some()
    }

    pub fn retarget_plain(&mut self, now: Duration) {
        self.retarget(None, now);
    }

    pub fn clear_plain(&mut self) {
        if self.transition_started.is_none() && self.target_is_plain && self.retarget_is_plain {
            self.pending_prepare_us = 0;
            self.pending_prepare_pixels = 0;
            return;
        }
        let prepare_start = Instant::now();
        self.source.fill(CRT_BACKDROP_BACKGROUND);
        self.target = std::sync::Arc::from(vec![CRT_BACKDROP_BACKGROUND; self.target.len()]);
        self.retarget.fill(CRT_BACKDROP_BACKGROUND);
        self.logical_retarget.fill(CRT_BACKDROP_BACKGROUND);
        self.source_row_repeats.fill(true);
        self.target_row_repeats = std::sync::Arc::from(vec![true; self.physical_height]);
        self.retarget_row_repeats.fill(true);
        self.target_is_plain = true;
        self.retarget_is_plain = true;
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
        self.target_is_plain = match frame {
            Some(frame)
                if scale_dimmed_center_crop_mapped_with_logical_height(
                    &mut prepared,
                    self.width,
                    self.physical_height,
                    self.reference_height,
                    frame,
                    &mut self.x_map,
                    &mut self.y_map,
                    &mut prepared_rows,
                ) =>
            {
                false
            }
            _ => true,
        };
        self.target = std::sync::Arc::from(prepared);
        self.target_row_repeats = std::sync::Arc::from(prepared_rows);
        self.pending_prepare_us = duration_us(prepare_start.elapsed());
        self.pending_prepare_pixels = self.target.len().min(u32::MAX as usize) as u32;
        self.transition_started = (self.source.as_slice() != self.target.as_ref()).then_some(now);
        if self.transition_started.is_none() {
            self.retarget.copy_from_slice(&self.target);
            self.retarget_row_repeats
                .copy_from_slice(&self.target_row_repeats);
            self.retarget_is_plain = self.target_is_plain;
            self.expand_to_logical();
        }
    }

    /// Adopt an immutable target prepared by the background lane.  This is
    /// intentionally separate from `retarget`, whose compatibility path still
    /// performs scaling synchronously for host tests and non-Arcade callers.
    #[cfg(feature = "ui")]
    pub(crate) fn retarget_prepared(
        &mut self,
        prepared: Option<PreparedCrtBackdrop>,
        now: Duration,
    ) {
        self.resolve_current(now);
        self.source.copy_from_slice(&self.retarget);
        self.source_row_repeats
            .copy_from_slice(&self.retarget_row_repeats);
        let Some(prepared) = prepared else {
            self.target = std::sync::Arc::from(vec![CRT_BACKDROP_BACKGROUND; self.target.len()]);
            self.target_row_repeats = std::sync::Arc::from(vec![true; self.physical_height]);
            self.target_is_plain = true;
            self.pending_prepare_us = 0;
            self.pending_prepare_pixels = 0;
            self.transition_started =
                (self.source.as_slice() != self.target.as_ref()).then_some(now);
            return;
        };
        self.target = prepared.pixels;
        self.target_row_repeats = prepared.row_repeats;
        self.target_is_plain = prepared.is_plain;
        self.pending_prepare_us = 0;
        self.pending_prepare_pixels = 0;
        self.transition_started = (self.source != self.target.as_ref()).then_some(now);
        if self.transition_started.is_none() {
            self.retarget.copy_from_slice(&self.target);
            self.retarget_row_repeats
                .copy_from_slice(&self.target_row_repeats);
            self.retarget_is_plain = self.target_is_plain;
            self.expand_to_logical();
        }
    }

    pub fn compose(&mut self, now: Duration) -> CrtBackdropWorkTrace {
        let mut logical_retarget = std::mem::take(&mut self.logical_retarget);
        let trace = self.compose_to(now, &mut logical_retarget, &[], &[], 1);
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
        let trace = self.compose_to(now, destination, &[], &[], 1);
        if !trace.active {
            self.expand_to_logical();
        }
        trace
    }

    /// Compose the backdrop while preserving opaque UI rectangles already in
    /// `destination`. Coordinates are in the logical RGB565 frame space.
    /// Protected pixels are still represented by the normal UI base/chrome;
    /// they are not part of the screenshot fade work.
    pub fn compose_into_excluding(
        &mut self,
        now: Duration,
        destination: &mut [Rgb565Pixel],
        protected_rects: &[(usize, usize, usize, usize)],
    ) -> CrtBackdropWorkTrace {
        let trace = self.compose_to(now, destination, protected_rects, &[], 1);
        if !trace.active {
            self.expand_to_logical();
        }
        trace
    }

    /// Compose a deliberately coarse 2x2 backdrop fade for CRT performance
    /// experiments. The interactive foreground remains full resolution.
    pub fn compose_into_coarse_excluding(
        &mut self,
        now: Duration,
        destination: &mut [Rgb565Pixel],
        protected_rects: &[(usize, usize, usize, usize)],
    ) -> CrtBackdropWorkTrace {
        let trace = self.compose_to(now, destination, protected_rects, &[], 2);
        if !trace.active {
            self.expand_to_logical();
        }
        trace
    }

    /// Compose the fixed production CRT backdrop directly into a frame while
    /// preserving only Arcade header and footer text pixels. Their transparent
    /// containers therefore reveal the screenshot beneath them.
    pub fn compose_product_into(
        &mut self,
        now: Duration,
        destination: &mut [Rgb565Pixel],
        content: CrtContentRect,
        metrics: CrtUiMetrics,
    ) -> CrtBackdropWorkTrace {
        let protected = product_chrome_rects(content, metrics);
        let trace = self.compose_to(now, destination, &protected, CRT_PRODUCT_TEXT_COLORS, 1);
        if !trace.active {
            self.expand_to_logical();
        }
        trace
    }

    fn compose_to(
        &mut self,
        now: Duration,
        destination: &mut [Rgb565Pixel],
        protected_rects: &[(usize, usize, usize, usize)],
        preserved_colors: &[Rgb565Pixel],
        coarse_factor: usize,
    ) -> CrtBackdropWorkTrace {
        let mut trace = CrtBackdropWorkTrace {
            prepare_us: std::mem::take(&mut self.pending_prepare_us),
            prepare_pixels: std::mem::take(&mut self.pending_prepare_pixels),
            ..CrtBackdropWorkTrace::default()
        };
        let Some(started) = self.transition_started else {
            trace.alpha_bucket = 32;
            self.expand_into(destination, protected_rects, preserved_colors);
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
            self.expand_into(destination, protected_rects, preserved_colors);
            trace.alpha_bucket = 0;
            trace.active = true;
            return trace;
        }
        let blend_start = Instant::now();
        let row_width = self.width.max(1);
        let coarse_factor = coarse_factor.max(1);
        let mut row = 0;
        while row < self.physical_height {
            let start = row.saturating_mul(row_width);
            let end = start.saturating_add(row_width).min(self.retarget.len());
            if row > 0 && self.source_row_repeats[row] && self.target_row_repeats[row] {
                let (before, current) = self.retarget.split_at_mut(start);
                current[..end - start].copy_from_slice(&before[start - row_width..start]);
            } else {
                let destination = &mut self.retarget[start..end];
                let previous = &self.source[start..end];
                let current = &self.target[start..end];
                // The retarget buffer is stored at the physical backdrop
                // height. Protected rectangles use logical output-space
                // coordinates and are applied only during expansion below;
                // mixing those coordinate systems here leaves stale rows in
                // the expanded list viewport on 240p.
                blend_rgb565_range(
                    destination,
                    previous,
                    current,
                    0,
                    destination.len(),
                    alpha_bucket,
                    coarse_factor,
                );
            }
            for copy_row in row + 1..(row + coarse_factor).min(self.physical_height) {
                let source_start = row * row_width;
                let source_end = source_start
                    .saturating_add(row_width)
                    .min(self.retarget.len());
                let destination_start = copy_row * row_width;
                let destination_end = destination_start
                    .saturating_add(row_width)
                    .min(self.retarget.len());
                let (before, after) = self.retarget.split_at_mut(destination_start);
                after[..destination_end - destination_start]
                    .copy_from_slice(&before[source_start..source_end]);
            }
            row = row.saturating_add(coarse_factor);
        }
        for row in 0..self.physical_height {
            self.retarget_row_repeats[row] = (coarse_factor > 1 && row % coarse_factor != 0)
                || (row > 0 && self.source_row_repeats[row] && self.target_row_repeats[row]);
        }
        trace.blend_us = duration_us(blend_start.elapsed());
        trace.blend_pixels = self.retarget.len().min(u32::MAX as usize) as u32;
        trace.alpha_bucket = alpha_bucket.min(32) as u8;
        trace.active = alpha_bucket < 32;
        if !trace.active {
            self.transition_started = None;
            self.retarget_is_plain = self.target_is_plain;
        } else {
            self.retarget_is_plain = false;
        }
        self.expand_into(destination, protected_rects, preserved_colors);
        trace
    }

    fn expand_into(
        &self,
        destination: &mut [Rgb565Pixel],
        protected_rects: &[(usize, usize, usize, usize)],
        preserved_colors: &[Rgb565Pixel],
    ) {
        let required = self.width.saturating_mul(self.height);
        if destination.len() < required {
            return;
        }
        if self.height == self.physical_height {
            for row in 0..self.height {
                copy_rgb565_row_preserving(
                    &mut destination[row * self.width..(row + 1) * self.width],
                    &self.retarget[row * self.width..(row + 1) * self.width],
                    row,
                    protected_rects,
                    preserved_colors,
                );
            }
            return;
        }
        if self.height == self.physical_height.saturating_mul(2) {
            for physical_y in 0..self.physical_height {
                let source_start = physical_y * self.width;
                let source_end = source_start + self.width;
                let logical_y = physical_y * 2;
                for row in [logical_y, logical_y + 1] {
                    let destination_start = row * self.width;
                    copy_rgb565_row_preserving(
                        &mut destination[destination_start..destination_start + self.width],
                        &self.retarget[source_start..source_end],
                        row,
                        protected_rects,
                        preserved_colors,
                    );
                }
            }
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
            copy_rgb565_row_preserving(
                &mut destination[logical_start..logical_start + self.width],
                &self.retarget[physical_start..physical_start + self.width],
                logical_y,
                protected_rects,
                preserved_colors,
            );
        }
    }

    fn expand_to_logical(&mut self) {
        if self.height == self.physical_height {
            self.logical_retarget.copy_from_slice(&self.retarget);
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
    if destination_width == 0
        || destination_height == 0
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
    let integer_scale_route = reference_destination_height > destination_height;
    if integer_scale_route {
        let integer_scale = 2;
        let scaled_width = frame.source_width.saturating_mul(integer_scale);
        let image_x = (destination_width as isize - scaled_width as isize) / 2;
        for (destination_x, source_x) in x_map[..destination_width].iter_mut().enumerate() {
            let local_x = destination_x as isize - image_x;
            *source_x = if local_x >= 0 && local_x < scaled_width as isize {
                local_x as usize / integer_scale
            } else {
                usize::MAX
            };
        }

        let scaled_logical_height = frame.source_height.saturating_mul(integer_scale);
        let image_y = (reference_destination_height as isize - scaled_logical_height as isize) / 2;
        for destination_y in 0..destination_height {
            let logical_y = destination_y
                .saturating_mul(2)
                .saturating_add(1)
                .saturating_mul(reference_destination_height)
                / destination_height.saturating_mul(2).max(1);
            let local_y = logical_y as isize - image_y;
            y_map[destination_y] = if local_y >= 0 && local_y < scaled_logical_height as isize {
                local_y as usize / integer_scale
            } else {
                usize::MAX
            };
        }
    } else {
        let (crop_x, crop_y, crop_width, crop_height) =
            center_crop_4_3(frame.source_width, frame.source_height);
        for (destination_x, source_x) in x_map[..destination_width].iter_mut().enumerate() {
            *source_x = crop_x
                + (destination_x.saturating_mul(crop_width) / destination_width)
                    .min(crop_width - 1);
        }
        for destination_y in 0..destination_height {
            y_map[destination_y] = crop_y
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

/// Prepare one RGB565 screenshot for the low-resolution CRT backdrop.  This
/// helper is deliberately allocation-owned by its caller so it can run on a
/// worker lane and hand the resulting buffers to `CrtBackdropState` by `Arc`.
/// Worker-friendly variant that reuses the nearest-neighbour maps between
/// requests. The destination remains request-owned because it is handed to
/// the backdrop cache, while the maps are pure scratch state.
#[cfg(feature = "ui")]
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

fn center_crop_4_3(width: usize, height: usize) -> (usize, usize, usize, usize) {
    if width.saturating_mul(3) > height.saturating_mul(4) {
        let crop_width = (height.saturating_mul(4) / 3).max(1).min(width);
        ((width - crop_width) / 2, 0, crop_width, height)
    } else {
        let crop_height = (width.saturating_mul(3) / 4).max(1).min(height);
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
    start: usize,
    end: usize,
    alpha_bucket: u16,
    coarse_factor: usize,
) {
    let end = end
        .min(destination.len())
        .min(previous.len())
        .min(current.len());
    let start = start.min(end);
    if coarse_factor <= 1 {
        let mut previous_source = Rgb565Pixel(u16::MAX);
        let mut previous_current = Rgb565Pixel(u16::MAX);
        for index in start..end {
            if index > start
                && previous[index] == previous_source
                && current[index] == previous_current
            {
                destination[index] = destination[index - 1];
            } else {
                destination[index] =
                    blend_rgb565_bucket(previous[index], current[index], alpha_bucket);
            }
            previous_source = previous[index];
            previous_current = current[index];
        }
    } else {
        if coarse_factor == 2 {
            match alpha_bucket.min(32) {
                0 => copy_rgb565_coarse_two_source(destination, previous, start, end),
                32 => copy_rgb565_coarse_two_source(destination, current, start, end),
                _ => {
                    #[cfg(all(target_os = "linux", target_arch = "arm"))]
                    {
                        // SAFETY: the slices are disjoint, and the bounds passed to the C
                        // kernel are validated by this function before entering the loop.
                        unsafe {
                            blend_rgb565_coarse_two_neon(
                                destination,
                                previous,
                                current,
                                start,
                                end,
                                alpha_bucket,
                            );
                        }
                        return;
                    }
                    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
                    {
                        match alpha_bucket.min(32) {
                            4 => blend_rgb565_coarse_two_const::<4, 28>(
                                destination,
                                previous,
                                current,
                                start,
                                end,
                            ),
                            8 => blend_rgb565_coarse_two_const::<8, 24>(
                                destination,
                                previous,
                                current,
                                start,
                                end,
                            ),
                            12 => blend_rgb565_coarse_two_const::<12, 20>(
                                destination,
                                previous,
                                current,
                                start,
                                end,
                            ),
                            16 => blend_rgb565_coarse_two_const::<16, 16>(
                                destination,
                                previous,
                                current,
                                start,
                                end,
                            ),
                            20 => blend_rgb565_coarse_two_const::<20, 12>(
                                destination,
                                previous,
                                current,
                                start,
                                end,
                            ),
                            25 => blend_rgb565_coarse_two_const::<25, 7>(
                                destination,
                                previous,
                                current,
                                start,
                                end,
                            ),
                            _ => {}
                        }
                        let mut index = start;
                        while index + 1 < end {
                            let pixel =
                                blend_rgb565_bucket(previous[index], current[index], alpha_bucket);
                            destination[index] = pixel;
                            destination[index + 1] = pixel;
                            index += 2;
                        }
                        if index < end {
                            destination[index] =
                                blend_rgb565_bucket(previous[index], current[index], alpha_bucket);
                        }
                    }
                }
            }
            return;
        }
        let mut index = start;
        let mut previous_source = Rgb565Pixel(u16::MAX);
        let mut previous_current = Rgb565Pixel(u16::MAX);
        let mut previous_pixel = Rgb565Pixel(0);
        while index < end {
            let pixel = if index > start
                && previous[index] == previous_source
                && current[index] == previous_current
            {
                previous_pixel
            } else {
                blend_rgb565_bucket(previous[index], current[index], alpha_bucket)
            };
            let block_end = index.saturating_add(coarse_factor).min(end);
            destination[index..block_end].fill(pixel);
            previous_source = previous[index];
            previous_current = current[index];
            previous_pixel = pixel;
            index = block_end;
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
unsafe fn blend_rgb565_coarse_two_neon(
    destination: &mut [Rgb565Pixel],
    previous: &[Rgb565Pixel],
    current: &[Rgb565Pixel],
    start: usize,
    end: usize,
    alpha_bucket: u16,
) {
    unsafe extern "C" {
        fn mister_magik_crt_backdrop_blend_coarse_two(
            destination: *mut u16,
            previous: *const u16,
            current: *const u16,
            start: usize,
            end: usize,
            alpha: u16,
        );
    }
    // SAFETY: callers clamp all three slices to `end`; the C kernel only accesses those ranges.
    unsafe {
        mister_magik_crt_backdrop_blend_coarse_two(
            destination.as_mut_ptr().cast(),
            previous.as_ptr().cast(),
            current.as_ptr().cast(),
            start,
            end,
            alpha_bucket,
        );
    }
}

#[inline(always)]
fn copy_rgb565_coarse_two_source(
    destination: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    start: usize,
    end: usize,
) {
    let mut index = start;
    while index + 1 < end {
        let pixel = source[index];
        destination[index] = pixel;
        destination[index + 1] = pixel;
        index += 2;
    }
    if index < end {
        destination[index] = source[index];
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
#[inline(always)]
fn blend_rgb565_coarse_two_const<const ALPHA: u32, const INVERSE: u32>(
    destination: &mut [Rgb565Pixel],
    previous: &[Rgb565Pixel],
    current: &[Rgb565Pixel],
    start: usize,
    end: usize,
) {
    let mut index = start;
    while index + 1 < end {
        let pixel = blend_rgb565_const::<ALPHA, INVERSE>(previous[index], current[index]);
        destination[index] = pixel;
        destination[index + 1] = pixel;
        index += 2;
    }
    if index < end {
        destination[index] = blend_rgb565_const::<ALPHA, INVERSE>(previous[index], current[index]);
    }
}

#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
#[inline(always)]
const fn blend_rgb565_const<const ALPHA: u32, const INVERSE: u32>(
    from: Rgb565Pixel,
    to: Rgb565Pixel,
) -> Rgb565Pixel {
    let from = from.0 as u32;
    let to = to.0 as u32;
    let red_blue = (((from & 0xf81f) * INVERSE + (to & 0xf81f) * ALPHA) >> 5) & 0xf81f;
    let green = (((from & 0x07e0) * INVERSE + (to & 0x07e0) * ALPHA) >> 5) & 0x07e0;
    Rgb565Pixel((red_blue | green) as u16)
}

#[inline(always)]
fn copy_rgb565_row_preserving(
    destination: &mut [Rgb565Pixel],
    source: &[Rgb565Pixel],
    row: usize,
    protected_rects: &[(usize, usize, usize, usize)],
    preserved_colors: &[Rgb565Pixel],
) {
    let width = destination.len().min(source.len());
    let mut overlapping = None;
    let mut overlap_count = 0usize;
    for &(x0, y0, x1, y1) in protected_rects {
        if row >= y0 && row < y1 {
            overlap_count += 1;
            if overlap_count == 1 {
                overlapping = Some((x0, x1));
            }
        }
    }
    // Most backdrop rows do not intersect the protected chrome. Keep that
    // common path to one slice copy instead of walking every protected rect.
    match (overlap_count, overlapping) {
        (0, _) => {
            destination[..width].copy_from_slice(&source[..width]);
            return;
        }
        (1, Some((x0, x1))) => {
            let protected_start = x0.min(width);
            let protected_end = x1.min(width).max(protected_start);
            destination[..protected_start].copy_from_slice(&source[..protected_start]);
            if !preserved_colors.is_empty() {
                for index in protected_start..protected_end {
                    if !preserved_colors.contains(&destination[index]) {
                        destination[index] = source[index];
                    }
                }
            }
            destination[protected_end..width].copy_from_slice(&source[protected_end..width]);
            return;
        }
        _ => {}
    }
    if !preserved_colors.is_empty() {
        for index in 0..width {
            let protected = protected_rects
                .iter()
                .any(|&(x0, y0, x1, y1)| row >= y0 && row < y1 && index >= x0 && index < x1);
            if !protected || !preserved_colors.contains(&destination[index]) {
                destination[index] = source[index];
            }
        }
        return;
    }
    let mut cursor = 0;
    for &(x0, y0, x1, y1) in protected_rects {
        if row < y0 || row >= y1 {
            continue;
        }
        let protected_start = x0.min(width);
        let protected_end = x1.min(width).max(protected_start);
        let copy_end = protected_start.max(cursor);
        destination[cursor..copy_end].copy_from_slice(&source[cursor..copy_end]);
        cursor = cursor.max(protected_end);
    }
    destination[cursor..width].copy_from_slice(&source[cursor..width]);
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

pub fn product_chrome_rects(
    content: CrtContentRect,
    metrics: CrtUiMetrics,
) -> [(usize, usize, usize, usize); 2] {
    let grid_x = metrics.grid_x.max(1) as usize;
    let grid_y = metrics.grid_y.max(1) as usize;
    let header = (
        content.x + grid_x * 2,
        content.y + grid_y * 2,
        content.x + content.width - grid_x * 2,
        content.y + grid_y * 2 + metrics.header_height.max(1) as usize,
    );
    let footer = (
        header.0,
        content.y + content.height - metrics.footer_height.max(1) as usize - grid_y * 2,
        header.2,
        content.y + content.height - grid_y * 2,
    );
    [header, footer]
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

    #[test]
    fn center_crop_maps_wide_and_tall_sources_to_four_three() {
        assert_eq!(center_crop_4_3(1600, 900), (200, 0, 1200, 900));
        assert_eq!(center_crop_4_3(600, 800), (0, 175, 600, 450));
        assert_eq!(center_crop_4_3(640, 480), (0, 0, 640, 480));
        assert_eq!(center_crop_4_3(320, 224), (11, 0, 298, 224));
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
    fn low_resolution_routes_share_backdrop_reference_height() {
        for (route, geometry, composition, expected) in [
            (
                ResolvedOutputRoute::Crt240p60,
                DisplayGeometry::new(640, 240),
                Crt240Composition::Native240,
                (640, 240, 240),
            ),
            (
                ResolvedOutputRoute::Crt240p60,
                DisplayGeometry::new(640, 240),
                Crt240Composition::Legacy480,
                (640, 480, 240),
            ),
            (
                ResolvedOutputRoute::Crt288p50,
                DisplayGeometry::new(640, 288),
                Crt240Composition::Native240,
                (640, 288, 288),
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
            assert_eq!(backdrop.reference_height(), CRT_COMPOSITION_H);
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
            let metrics = CrtUiMetrics::for_display(&display);
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

            let trace = backdrop.compose_product_into(
                CRT_BACKDROP_FADE_DURATION,
                &mut destination,
                display.content_rect(),
                metrics,
            );

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
    fn protected_rectangles_are_not_overwritten_during_compose() {
        let target = vec![Rgb565Pixel(0xffff); 8];
        let mut backdrop = CrtBackdropState::new(4, 2);
        backdrop.retarget(Some(frame(&target, 4, 2)), Duration::ZERO);
        let marker = Rgb565Pixel(0x07e0);
        let mut destination = vec![marker; 8];
        let trace = backdrop.compose_into_excluding(
            Duration::from_millis(65),
            &mut destination,
            &[(1, 0, 3, 2)],
        );
        assert!(trace.active);
        assert_eq!(destination[1], marker);
        assert_eq!(destination[2], marker);
        assert_ne!(destination[0], marker);
        assert_ne!(destination[3], marker);
    }

    #[test]
    fn product_chrome_preserves_text_colors_but_repaints_container_pixels() {
        let marker = Rgb565Pixel(0xf81f);
        let source = [Rgb565Pixel(0xffff); 4];
        let mut destination = [
            marker,
            CRT_PRODUCT_HEADER_TEXT,
            CRT_PRODUCT_FOOTER_TEXT,
            marker,
        ];

        copy_rgb565_row_preserving(
            &mut destination,
            &source,
            0,
            &[(1, 0, 4, 1)],
            CRT_PRODUCT_TEXT_COLORS,
        );

        assert_eq!(destination[0], source[0]);
        assert_eq!(destination[1], CRT_PRODUCT_HEADER_TEXT);
        assert_eq!(destination[2], CRT_PRODUCT_FOOTER_TEXT);
        assert_eq!(destination[3], source[3]);
    }

    #[test]
    fn coarse_compose_expands_each_fade_sample_to_a_two_by_two_block() {
        let target = vec![Rgb565Pixel(0xffff); 16];
        let mut backdrop = CrtBackdropState::new(4, 4);
        backdrop.retarget(Some(frame(&target, 4, 4)), Duration::ZERO);
        let mut destination = vec![Rgb565Pixel(0); 16];
        let trace = backdrop.compose_into_coarse_excluding(
            Duration::from_millis(65),
            &mut destination,
            &[],
        );
        assert!(trace.active);
        assert_eq!(destination[0], destination[1]);
        assert_eq!(destination[0], destination[4]);
        assert_eq!(destination[2], destination[3]);
        assert_eq!(destination[2], destination[6]);
    }

    #[test]
    fn physical_rows_are_blended_before_logical_chrome_exclusion() {
        let target = vec![Rgb565Pixel(0xffff); 8];
        let mut backdrop = CrtBackdropState::new_with_heights(4, 4, 2);
        backdrop.retarget(Some(frame(&target, 4, 2)), Duration::ZERO);
        let marker = Rgb565Pixel(0x1234);
        let mut destination = vec![marker; 16];
        let trace = backdrop.compose_into_coarse_excluding(
            Duration::from_millis(65),
            &mut destination,
            &[(1, 1, 3, 3)],
        );

        assert!(trace.active);
        assert_ne!(backdrop.retarget[5], CRT_BACKDROP_BACKGROUND);
        assert_eq!(destination[4 + 1], marker);
        assert_eq!(destination[8 + 1], marker);
        assert_ne!(destination[12 + 1], marker);
    }

    #[test]
    fn alpha_zero_composition_rewrites_the_backdrop_destination() {
        let target = vec![Rgb565Pixel(0xffff); 8];
        let mut backdrop = CrtBackdropState::new_with_heights(4, 4, 2);
        backdrop.retarget(Some(frame(&target, 4, 2)), Duration::ZERO);
        let marker = Rgb565Pixel(0x1234);
        let mut destination = vec![marker; 16];
        let trace = backdrop.compose_into_coarse_excluding(Duration::ZERO, &mut destination, &[]);

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
        let trace = backdrop.compose_into_coarse_excluding(
            CRT_BACKDROP_FADE_DURATION,
            &mut destination,
            &[],
        );

        assert_eq!(trace.alpha_bucket, 32);
        assert!(!trace.active);
        assert!(
            backdrop
                .pixels()
                .iter()
                .all(|pixel| *pixel == darken_rgb565(Rgb565Pixel(0xffff)))
        );
    }
}
