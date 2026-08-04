// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Minimal two-slot RGB565 presenter for standalone framebuffer tools.

use crate::fpga::{
    Fpga, LatchedFbufGeometry, LatchedFbufStatus, MAGIK_FBUF_CAPS_MAGIC, MAGIK_FBUF_LATCH_MAGIC,
};
use crate::framebuffer::format::rgb565_stride_bytes;
use crate::framebuffer::hidden_scanout::{
    HiddenRgb565BufferIndex, HiddenScanoutError, HiddenScanoutFramebuffer,
};
use crate::framebuffer::rgb565::Rgb565;
use crate::framebuffer::route::FramebufferRouteMode;
use crate::framebuffer::route::LauncherFramebufferRoute;
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
        }
    }
}

impl std::error::Error for HiddenLatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scanout(error) => Some(error),
            Self::Transport(error) => Some(error),
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
        let started = Instant::now();
        let (after, status_reads) = wait_for_posted_status(
            &mut self.fpga,
            self.settle_timeout,
            pending.sequence,
            pending.base,
        )?;
        self.pipeline_stats.status_reads = self
            .pipeline_stats
            .status_reads
            .saturating_add(status_reads);
        self.pipeline_stats.poll_reads = self
            .pipeline_stats
            .poll_reads
            .saturating_add(status_reads.saturating_sub(1));
        self.pipeline_stats.settle_us = self
            .pipeline_stats
            .settle_us
            .saturating_add(started.elapsed().as_micros() as u64);
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
        let _guard = self.fpga.lock_latch_transaction()?;
        let post = self
            .fpga
            .post_magik_latched_fbuf_rgb565_observed(
                sequence,
                base,
                self.width,
                self.height,
                self.geometry,
                None,
            )
            .map_err(|error| HiddenLatchError::Transport(error.into_io()))?;
        if post.ack_high != MAGIK_FBUF_LATCH_MAGIC && post.ack_low != MAGIK_FBUF_LATCH_MAGIC {
            return Err(HiddenLatchError::Unsupported(format!(
                "SET acknowledgement 0x{:04x}/0x{:04x}",
                post.ack_high, post.ack_low
            )));
        }
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

    /// Publishes current slot writes, posts through the existing v4 protocol, and
    /// waits for a bounded verified flip before exposing the other slot.
    pub fn present(&mut self) -> Result<HiddenLatchPresentReceipt, HiddenLatchError> {
        self.post()?;
        self.settle_pending()?.ok_or_else(|| {
            HiddenLatchError::PostNotObserved("posted frame lost pending state".into())
        })
    }
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

fn wait_for_posted_status(
    fpga: &mut Fpga,
    timeout: Duration,
    sequence: u16,
    base: u32,
) -> Result<(LatchedFbufStatus, u64), HiddenLatchError> {
    let started = Instant::now();
    let mut status_reads = 0_u64;
    loop {
        let status = fpga.read_magik_latched_fbuf_status()?;
        status_reads = status_reads.saturating_add(1);
        if status.supported()
            && !status.pending()
            && status.active_sequence == sequence
            && status.active_base == base
        {
            return Ok((status, status_reads));
        }
        if started.elapsed() >= timeout {
            return Err(HiddenLatchError::PostNotObserved(format!(
                "posted sequence={sequence} base=0x{base:08x}; active sequence={} base=0x{:08x} pending={} pending_sequence={}",
                status.active_sequence,
                status.active_base,
                status.pending(),
                status.pending_sequence
            )));
        }
        std::thread::sleep(STATUS_POLL_BACKOFF);
    }
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
    fn scaled_geometry_keeps_source_stride_and_full_destination() {
        let geometry = scaled_geometry(960, 1920, 1080).unwrap();

        assert_eq!(geometry.stride_bytes, 1920);
        assert_eq!((geometry.xoff, geometry.right), (0, 1919));
        assert_eq!((geometry.yoff, geometry.bottom), (0, 1079));
    }
}
