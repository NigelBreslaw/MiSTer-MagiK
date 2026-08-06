// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Minimal two-slot RGB565 presenter for standalone framebuffer tools.

use crate::fpga::{Fpga, LatchedFbufGeometry, LatchedFbufStatus, MAGIK_FBUF_CAPS_MAGIC};
use crate::framebuffer::damage::{DirtyRect, DirtyRectList, TwoSlotDamageLedger};
use crate::framebuffer::format::rgb565_stride_bytes;
use crate::framebuffer::full_frame_latch::{
    LatchPostRequest, LogicalStatusReadBudget, latch_status_read_failure,
    post_confirm_prepared_frame, read_status_sample, wait_for_latch_completion,
};
use crate::framebuffer::hidden_scanout::{
    HiddenRgb565BufferIndex, HiddenScanoutError, HiddenScanoutFramebuffer,
};
use crate::framebuffer::rgb565::Rgb565;
use crate::framebuffer::route::FramebufferRouteMode;
use crate::framebuffer::route::LauncherFramebufferRoute;
use crate::framebuffer::vertical_scale::{Rgb565FrameView, VerticalRect, VerticalRgb565Transform};
use crate::latch_readiness::{LatchFailure, LatchFailureStage};
use mister_magik_core::display::ResolvedDisplayPlan;
use std::io;
use std::time::{Duration, Instant};

const DEFAULT_SETTLE_TIMEOUT: Duration = Duration::from_millis(50);
const STATUS_POLL_BACKOFF: Duration = Duration::from_micros(100);

#[derive(Debug)]
pub enum HiddenLatchError {
    Scanout(HiddenScanoutError),
    Transport(io::Error),
    Unsupported(String),
    NoWritableSlot(String),
    PostNotObserved(String),
    Protocol(LatchFailure),
}

impl std::fmt::Display for HiddenLatchError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scanout(error) => error.fmt(output),
            Self::Transport(error) => write!(output, "latch transport failed: {error}"),
            Self::Unsupported(message) => write!(output, "latch is unsupported: {message}"),
            Self::NoWritableSlot(message) => write!(output, "no writable hidden slot: {message}"),
            Self::PostNotObserved(message) => {
                write!(output, "latch post was not observed: {message}")
            }
            Self::Protocol(error) => error.fmt(output),
        }
    }
}

impl std::error::Error for HiddenLatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scanout(error) => Some(error),
            Self::Transport(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::Unsupported(_) | Self::NoWritableSlot(_) | Self::PostNotObserved(_) => None,
        }
    }
}

impl From<HiddenScanoutError> for HiddenLatchError {
    fn from(value: HiddenScanoutError) -> Self {
        Self::Scanout(value)
    }
}

impl From<io::Error> for HiddenLatchError {
    fn from(value: io::Error) -> Self {
        Self::Transport(value)
    }
}

impl From<LatchFailure> for HiddenLatchError {
    fn from(value: LatchFailure) -> Self {
        Self::Protocol(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HiddenLatchPresentReceipt {
    pub slot_index: u8,
    pub sequence: u16,
    pub flip_count: u16,
    pub post_count: u16,
    pub drop_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HiddenLatchPostReceipt {
    pub slot_index: u8,
    pub sequence: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HiddenLatchPipelineStats {
    pub status_reads: u64,
    pub poll_reads: u64,
    pub settle_us: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CachedHiddenLatchCopyStats {
    pub slot_index: u8,
    pub source_rect_count: u32,
    pub destination_rect_count: u32,
    pub source_bytes: usize,
    pub destination_bytes: usize,
    pub full_restore: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingPresentation {
    slot_index: u8,
    sequence: u16,
    base: u32,
}

/// Owns both hidden scanout slots and publishes one completed RGB565 frame at a time.
pub struct HiddenLatchPresenter {
    fpga: Fpga,
    slots: [HiddenScanoutFramebuffer; 2],
    bases: [u32; 2],
    writable_slot: usize,
    sequence: u16,
    width: u16,
    height: u16,
    geometry: LatchedFbufGeometry,
    settle_timeout: Duration,
    pending: Option<PendingPresentation>,
    pipeline_stats: HiddenLatchPipelineStats,
}

impl HiddenLatchPresenter {
    /// Opens the qualified framebuffer-sized route without changing video mode or Main state.
    pub fn open(width: u16, height: u16) -> Result<Self, HiddenLatchError> {
        Self::open_with_geometry(
            width,
            height,
            LatchedFbufGeometry::new(
                width,
                FramebufferRouteMode::framebuffer_sized(width, height),
                0,
            ),
        )
    }

    pub fn open_for_plan(plan: ResolvedDisplayPlan) -> Result<Self, HiddenLatchError> {
        let width = u16::try_from(plan.fb_w).map_err(|_| {
            HiddenLatchError::Unsupported(format!("framebuffer width {} exceeds u16", plan.fb_w))
        })?;
        let height = u16::try_from(plan.fb_h).map_err(|_| {
            HiddenLatchError::Unsupported(format!("framebuffer height {} exceeds u16", plan.fb_h))
        })?;
        let route = LauncherFramebufferRoute::for_scan(
            plan.scan_w,
            plan.scan_h,
            plan.output_route.is_crt(),
        );
        Self::open_with_geometry(
            width,
            height,
            LatchedFbufGeometry::new_for_route(width, route, 0),
        )
    }

    fn open_with_geometry(
        width: u16,
        height: u16,
        geometry: LatchedFbufGeometry,
    ) -> Result<Self, HiddenLatchError> {
        let stride_bytes = rgb565_stride_bytes(usize::from(width));
        let slot1 = HiddenScanoutFramebuffer::open(
            HiddenRgb565BufferIndex::new(1)?,
            usize::from(width),
            usize::from(height),
            stride_bytes,
        )?;
        let slot2 = HiddenScanoutFramebuffer::open(
            HiddenRgb565BufferIndex::new(2)?,
            usize::from(width),
            usize::from(height),
            stride_bytes,
        )?;
        let bases = [slot1.physical_addr(), slot2.physical_addr()];
        let mut fpga = Fpga::open()?;
        let (magic_hi, magic_lo, capabilities) = fpga.read_magik_latched_fbuf_capabilities()?;
        if (magic_hi != MAGIK_FBUF_CAPS_MAGIC && magic_lo != MAGIK_FBUF_CAPS_MAGIC)
            || !capabilities.production_ready()
        {
            return Err(HiddenLatchError::Unsupported(format!(
                "magic=0x{magic_hi:04x}/0x{magic_lo:04x} protocol={} flags=0x{:04x} max={}x{} stride={}",
                capabilities.protocol_version,
                capabilities.flags,
                capabilities.max_width,
                capabilities.max_height,
                capabilities.max_stride_bytes
            )));
        }
        if width > capabilities.max_width
            || height > capabilities.max_height
            || stride_bytes > usize::from(capabilities.max_stride_bytes)
        {
            return Err(HiddenLatchError::Unsupported(format!(
                "requested {width}x{height} stride={stride_bytes} exceeds negotiated maximum {}x{} stride={}",
                capabilities.max_width, capabilities.max_height, capabilities.max_stride_bytes
            )));
        }
        let initial = wait_for_settled_status(&mut fpga, DEFAULT_SETTLE_TIMEOUT)?;
        let writable_slot = writable_slot_for_status(bases, initial)?;
        Ok(Self {
            fpga,
            slots: [slot1, slot2],
            bases,
            writable_slot,
            sequence: 1,
            width,
            height,
            geometry,
            settle_timeout: DEFAULT_SETTLE_TIMEOUT,
            pending: None,
            pipeline_stats: HiddenLatchPipelineStats::default(),
        })
    }

    /// Opens RGB565 source slots and stretches their destination rectangle to
    /// an already verified progressive scanout, matching the production route.
    pub fn open_scaled(
        width: u16,
        height: u16,
        destination_width: u16,
        destination_height: u16,
    ) -> Result<Self, HiddenLatchError> {
        let geometry = scaled_geometry(width, destination_width, destination_height)?;
        let mut presenter = Self::open(width, height)?;
        presenter.geometry = geometry;
        Ok(presenter)
    }

    #[must_use]
    pub const fn width(&self) -> usize {
        self.width as usize
    }

    #[must_use]
    pub const fn height(&self) -> usize {
        self.height as usize
    }

    #[must_use]
    pub const fn destination_width(&self) -> usize {
        self.geometry.right.saturating_sub(self.geometry.xoff) as usize + 1
    }

    #[must_use]
    pub const fn destination_height(&self) -> usize {
        self.geometry.bottom.saturating_sub(self.geometry.yoff) as usize + 1
    }

    #[must_use]
    pub fn stride_pixels(&self) -> usize {
        self.slots[self.writable_slot].stride_pixels()
    }

    #[must_use]
    pub const fn writable_slot_index(&self) -> u8 {
        assert!(
            self.pending.is_none(),
            "pending latch post must settle before slot access"
        );
        self.writable_slot as u8 + 1
    }

    #[must_use]
    pub const fn pipeline_stats(&self) -> HiddenLatchPipelineStats {
        self.pipeline_stats
    }

    pub fn presentation_telemetry(
        &mut self,
    ) -> Result<mister_magik_latch_contract::PresentationTelemetry, HiddenLatchError> {
        self.fpga
            .read_magik_presentation_telemetry()
            .map_err(HiddenLatchError::Transport)
    }

    /// Returns the inactive slot selected after the previous verified presentation.
    pub fn pixels_mut(&mut self) -> &mut [Rgb565] {
        assert!(
            self.pending.is_none(),
            "pending latch post must settle before pixel access"
        );
        self.slots[self.writable_slot].pixels_mut()
    }

    /// Verifies a previously posted frame and exposes the other slot for writing.
    /// In a paced loop this normally performs one status transaction after the
    /// vblank deadline; bounded polling remains the late-flip recovery path.
    pub fn settle_pending(
        &mut self,
    ) -> Result<Option<HiddenLatchPresentReceipt>, HiddenLatchError> {
        let Some(pending) = self.pending else {
            return Ok(None);
        };
        let completion =
            wait_for_latch_completion(&mut self.fpga, pending.sequence, self.settle_timeout)?;
        let after = completion.status;
        if after.active_base != pending.base {
            return Err(HiddenLatchError::PostNotObserved(format!(
                "posted sequence={} base=0x{:08x}; active base=0x{:08x}",
                pending.sequence, pending.base, after.active_base
            )));
        }
        self.pipeline_stats.status_reads = self
            .pipeline_stats
            .status_reads
            .saturating_add(u64::from(completion.poll_count));
        self.pipeline_stats.poll_reads = self
            .pipeline_stats
            .poll_reads
            .saturating_add(u64::from(completion.poll_count.saturating_sub(1)));
        self.pipeline_stats.settle_us = self
            .pipeline_stats
            .settle_us
            .saturating_add(completion.wall_us);
        self.pending = None;
        self.writable_slot = 1 - self.writable_slot;
        Ok(Some(HiddenLatchPresentReceipt {
            slot_index: pending.slot_index,
            sequence: pending.sequence,
            flip_count: after.flip_count,
            post_count: after.post_count,
            drop_count: after.drop_count,
        }))
    }

    /// Publishes the current writable slot without waiting for its vblank flip.
    pub fn post(&mut self) -> Result<HiddenLatchPostReceipt, HiddenLatchError> {
        if let Some(pending) = self.pending {
            return Err(HiddenLatchError::NoWritableSlot(format!(
                "sequence {} is still pending verification",
                pending.sequence
            )));
        }
        self.slots[self.writable_slot].publish_writes();
        let sequence = self.sequence;
        self.sequence = next_sequence(sequence);
        let slot_index = self.writable_slot as u8 + 1;
        let base = self.bases[self.writable_slot];
        let bases = self.bases;
        let width = self.width;
        let height = self.height;
        let receipt = post_confirm_prepared_frame(
            &mut self.fpga,
            LatchPostRequest {
                sequence,
                slot_index,
                base_addr: base,
                width: self.width,
                height: self.height,
                geometry: self.geometry,
            },
            |hardware, budget| read_lab_status(hardware, budget, bases, width, height),
        )?;
        self.pipeline_stats.status_reads = self
            .pipeline_stats
            .status_reads
            .saturating_add(1 + u64::from(receipt.status_reads));
        self.pipeline_stats.poll_reads = self
            .pipeline_stats
            .poll_reads
            .saturating_add(u64::from(receipt.status_reads.saturating_sub(1)));
        self.pipeline_stats.settle_us = self
            .pipeline_stats
            .settle_us
            .saturating_add(receipt.status_us);
        self.pending = Some(PendingPresentation {
            slot_index,
            sequence,
            base,
        });
        Ok(HiddenLatchPostReceipt {
            slot_index,
            sequence,
        })
    }

    /// Publishes current slot writes, posts through the existing v5 protocol, and
    /// waits for a bounded verified flip before exposing the other slot.
    pub fn present(&mut self) -> Result<HiddenLatchPresentReceipt, HiddenLatchError> {
        self.post()?;
        self.settle_pending()?.ok_or_else(|| {
            HiddenLatchError::PostNotObserved("posted frame lost pending state".into())
        })
    }
}

pub struct CachedHiddenLatchPresenter {
    raw: HiddenLatchPresenter,
    ledger: TwoSlotDamageLedger,
    transform: VerticalRgb565Transform,
    render_width: usize,
    render_height: usize,
    prepared_slot: Option<u8>,
    poisoned: bool,
}

impl CachedHiddenLatchPresenter {
    pub fn open(plan: ResolvedDisplayPlan) -> Result<Self, HiddenLatchError> {
        let raw = HiddenLatchPresenter::open_for_plan(plan)?;
        let transform = VerticalRgb565Transform::new(plan.render_w, plan.render_h, plan.fb_h)
            .map_err(|error| HiddenLatchError::Unsupported(error.into()))?;
        Ok(Self {
            raw,
            ledger: TwoSlotDamageLedger::new(plan.render_w, plan.render_h),
            transform,
            render_width: plan.render_w,
            render_height: plan.render_h,
            prepared_slot: None,
            poisoned: false,
        })
    }

    #[must_use]
    pub const fn render_width(&self) -> usize {
        self.render_width
    }

    #[must_use]
    pub const fn render_height(&self) -> usize {
        self.render_height
    }

    #[must_use]
    pub const fn pipeline_stats(&self) -> HiddenLatchPipelineStats {
        self.raw.pipeline_stats()
    }

    pub fn presentation_telemetry(
        &mut self,
    ) -> Result<mister_magik_latch_contract::PresentationTelemetry, HiddenLatchError> {
        self.ensure_healthy()?;
        self.raw.presentation_telemetry()
    }

    #[must_use]
    pub const fn writable_slot_index(&self) -> u8 {
        self.raw.writable_slot_index()
    }

    pub fn prepare_cached(
        &mut self,
        source: &[Rgb565],
        damage: &DirtyRectList,
    ) -> Result<CachedHiddenLatchCopyStats, HiddenLatchError> {
        self.ensure_healthy()?;
        if self.prepared_slot.is_some() {
            return Err(HiddenLatchError::NoWritableSlot(
                "a cached frame is already prepared".into(),
            ));
        }
        let needed = self.render_width.saturating_mul(self.render_height);
        if source.len() < needed {
            return Err(HiddenLatchError::Unsupported(format!(
                "cached source has {} pixels, need {needed}",
                source.len()
            )));
        }
        self.ledger.record_damage(damage);
        let slot_index = self.raw.writable_slot_index();
        let restore = self.ledger.plan(slot_index);
        let full_restore = is_full_restore(&restore, self.render_width, self.render_height);
        let destination_stride = self.raw.stride_pixels();
        let destination = self.raw.pixels_mut();
        let source_view = Rgb565FrameView {
            pixels: source,
            width: self.render_width,
            height: self.render_height,
            stride_pixels: self.render_width,
        };
        let mut destination_bytes = 0usize;
        let mut destination_rect_count = 0_u32;
        for rect in restore.iter() {
            let copied = match self.transform.copy_rect(
                source_view,
                VerticalRect {
                    x0: rect.x0,
                    y0: rect.y0,
                    x1: rect.x1,
                    y1: rect.y1,
                },
                destination,
                destination_stride,
            ) {
                Ok(copied) => copied,
                Err(error) => {
                    self.ledger.mark_attempt_failed(slot_index);
                    return Err(HiddenLatchError::Unsupported(error.into()));
                }
            };
            if let Some(copied) = copied {
                destination_rect_count = destination_rect_count.saturating_add(1);
                destination_bytes = destination_bytes.saturating_add(copied.bytes);
            }
        }
        self.prepared_slot = Some(slot_index);
        Ok(CachedHiddenLatchCopyStats {
            slot_index,
            source_rect_count: restore.len() as u32,
            destination_rect_count,
            source_bytes: restore.total_rgb565_bytes(),
            destination_bytes,
            full_restore,
        })
    }

    pub fn post_prepared(&mut self) -> Result<HiddenLatchPostReceipt, HiddenLatchError> {
        self.ensure_healthy()?;
        let prepared_slot = self.prepared_slot.ok_or_else(|| {
            HiddenLatchError::NoWritableSlot("no cached frame has been prepared".into())
        })?;
        match self.raw.post() {
            Ok(receipt) if receipt.slot_index == prepared_slot => Ok(receipt),
            Ok(receipt) => {
                self.ledger.invalidate_all();
                self.poisoned = true;
                Err(HiddenLatchError::PostNotObserved(format!(
                    "prepared slot {prepared_slot}, posted slot {}",
                    receipt.slot_index
                )))
            }
            Err(error) => {
                self.ledger.mark_attempt_failed(prepared_slot);
                self.poisoned = true;
                Err(error)
            }
        }
    }

    pub fn settle_pending(
        &mut self,
    ) -> Result<Option<HiddenLatchPresentReceipt>, HiddenLatchError> {
        self.ensure_healthy()?;
        match self.raw.settle_pending() {
            Ok(Some(receipt)) => {
                if self.prepared_slot != Some(receipt.slot_index) {
                    self.ledger.invalidate_all();
                    self.poisoned = true;
                    return Err(HiddenLatchError::PostNotObserved(format!(
                        "prepared slot {:?}, settled slot {}",
                        self.prepared_slot, receipt.slot_index
                    )));
                }
                self.ledger.mark_presented(receipt.slot_index);
                self.prepared_slot = None;
                Ok(Some(receipt))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                if let Some(slot_index) = self.prepared_slot {
                    self.ledger.mark_attempt_failed(slot_index);
                }
                self.poisoned = true;
                Err(error)
            }
        }
    }

    pub fn present_cached(
        &mut self,
        source: &[Rgb565],
        damage: &DirtyRectList,
    ) -> Result<(CachedHiddenLatchCopyStats, HiddenLatchPresentReceipt), HiddenLatchError> {
        let copy = self.prepare_cached(source, damage)?;
        self.post_prepared()?;
        let present = self.settle_pending()?.ok_or_else(|| {
            HiddenLatchError::PostNotObserved("posted cached frame lost pending state".into())
        })?;
        Ok((copy, present))
    }

    fn ensure_healthy(&self) -> Result<(), HiddenLatchError> {
        if self.poisoned {
            Err(HiddenLatchError::PostNotObserved(
                "cached presenter requires reopen after ambiguous latch state".into(),
            ))
        } else {
            Ok(())
        }
    }
}

fn is_full_restore(restore: &DirtyRectList, width: usize, height: usize) -> bool {
    restore.len() == 1
        && restore.get(0)
            == Some(DirtyRect {
                x0: 0,
                y0: 0,
                x1: width,
                y1: height,
            })
}

fn scaled_geometry(
    source_width: u16,
    destination_width: u16,
    destination_height: u16,
) -> Result<LatchedFbufGeometry, HiddenLatchError> {
    if destination_width == 0 || destination_height == 0 {
        return Err(HiddenLatchError::Unsupported(format!(
            "invalid destination geometry {destination_width}x{destination_height}"
        )));
    }
    Ok(LatchedFbufGeometry::new(
        source_width,
        FramebufferRouteMode::framebuffer_sized(destination_width, destination_height),
        0,
    ))
}

fn wait_for_settled_status(
    fpga: &mut Fpga,
    timeout: Duration,
) -> Result<LatchedFbufStatus, HiddenLatchError> {
    let started = Instant::now();
    loop {
        let status = fpga.read_magik_latched_fbuf_status()?;
        if !status.supported() {
            return Err(HiddenLatchError::Unsupported(format!(
                "status magic 0x{:04x}/0x{:04x}",
                status.magic_hi, status.magic_lo
            )));
        }
        if !status.pending() {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            return Err(HiddenLatchError::NoWritableSlot(format!(
                "pending sequence {} did not settle within {} ms",
                status.pending_sequence,
                timeout.as_millis()
            )));
        }
        std::thread::sleep(STATUS_POLL_BACKOFF);
    }
}

fn read_lab_status(
    hardware: &mut Fpga,
    budget: &mut LogicalStatusReadBudget,
    bases: [u32; 2],
    width: u16,
    height: u16,
) -> Result<crate::fpga::LatchedFbufStatusSample, LatchFailure> {
    budget.consume().map_err(|_| {
        LatchFailure::runtime(
            LatchFailureStage::PostVerification,
            crate::latch_readiness::LatchFailureReason::FpgaTransportFailed,
            "latch status exhausted its bounded read budget",
        )
    })?;
    let capabilities = hardware.negotiated_magik_latch_capabilities();
    let sample = read_status_sample(hardware, capabilities)
        .map_err(|error| latch_status_read_failure(LatchFailureStage::FpgaStatus, error))?;
    let status = sample.status;
    if status.active_enabled()
        && bases.contains(&status.active_base)
        && (status.active_width != width
            || status.active_height != height
            || status.active_stride != rgb565_stride_bytes(usize::from(width)) as u16)
    {
        return Err(LatchFailure::runtime(
            LatchFailureStage::PostVerification,
            crate::latch_readiness::LatchFailureReason::ActiveGeometryMismatch,
            format!(
                "latched framebuffer geometry mismatch active={}x{} stride={} expected={}x{} stride={}",
                status.active_width,
                status.active_height,
                status.active_stride,
                width,
                height,
                rgb565_stride_bytes(usize::from(width))
            ),
        ));
    }
    Ok(sample)
}

fn writable_slot_for_status(
    bases: [u32; 2],
    status: LatchedFbufStatus,
) -> Result<usize, HiddenLatchError> {
    if status.pending() {
        return Err(HiddenLatchError::NoWritableSlot(format!(
            "pending sequence {}",
            status.pending_sequence
        )));
    }
    Ok(if status.active_base == bases[0] { 1 } else { 0 })
}

const fn next_sequence(sequence: u16) -> u16 {
    let next = sequence.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(active_base: u32, pending: bool) -> LatchedFbufStatus {
        LatchedFbufStatus {
            magic_hi: crate::fpga::MAGIK_FBUF_STATUS_MAGIC,
            magic_lo: 0,
            active_sequence: 1,
            pending_sequence: u16::from(pending),
            flags: if pending { 0x0004 } else { 0 },
            flip_count: 0,
            post_count: 0,
            drop_count: 0,
            active_base,
            active_width: 960,
            active_height: 540,
            active_stride: 1920,
            reject_count: 0,
            active_route_epoch: 0,
            accepted_sequence: 1,
            active_transaction: 1,
            pending_transaction: 0,
            accepted_transaction: 1,
        }
    }

    #[test]
    fn selects_slot_other_than_active_hidden_base() {
        let bases = [0x227e_9000, 0x22fd_2000];
        assert_eq!(
            writable_slot_for_status(bases, status(bases[0], false)).unwrap(),
            1
        );
        assert_eq!(
            writable_slot_for_status(bases, status(bases[1], false)).unwrap(),
            0
        );
        assert_eq!(
            writable_slot_for_status(bases, status(0x2200_0000, false)).unwrap(),
            0
        );
    }

    #[test]
    fn refuses_selection_while_latch_is_pending() {
        assert!(writable_slot_for_status([1, 2], status(0, true)).is_err());
    }

    #[test]
    fn sequence_never_uses_zero() {
        assert_eq!(next_sequence(1), 2);
        assert_eq!(next_sequence(u16::MAX), 1);
    }

    #[test]
    fn full_restore_requires_one_exact_surface_rectangle() {
        assert!(is_full_restore(
            &DirtyRectList::from_one(DirtyRect {
                x0: 0,
                y0: 0,
                x1: 960,
                y1: 600,
            }),
            960,
            600,
        ));
        assert!(!is_full_restore(
            &DirtyRectList::from_one(DirtyRect {
                x0: 336,
                y0: 90,
                x1: 623,
                y1: 510,
            }),
            960,
            600,
        ));
    }

    #[test]
    fn scaled_geometry_keeps_source_stride_and_full_destination() {
        let geometry = scaled_geometry(960, 1920, 1080).unwrap();

        assert_eq!(geometry.stride_bytes, 1920);
        assert_eq!((geometry.xoff, geometry.right), (0, 1919));
        assert_eq!((geometry.yoff, geometry.bottom), (0, 1079));
    }
}
