// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Latch presentation and bounded development fault injection.
//!
//! `MISTER_MAGIK_DEV_LATCH_STATUS_TIMEOUT_AT=N` fails exactly the Nth
//! `0x0058` status read when the executable is installed under
//! `/media/fat/mister-magik-dev`; public installations ignore it. Keep this
//! process-scoped and never write it to `launcher.env`.
//! `MISTER_MAGIK_DEV_LATCH_POST_SKIP_WORD_INDEX=N` similarly omits exactly one
//! zero-based SET payload word once per process, only from the development
//! installation, so receiver rejection evidence can be validated end to end.
//!
//! `catalog-lab latch-load-scenario OUTPUT` creates a bounded 500K-game
//! reproduction manifest. Resulting support evidence is retained at
//! `diagnostics/latch/latest.json` under the active MagiK installation.

use super::super::*;
use mister_magik_fb::framebuffer::downsample::Rgb565FrameView;
use mister_magik_fb::framebuffer::full_frame_latch::{
    LatchCompletion, LatchCopyResult, LatchPostRequest, LogicalStatusReadBudget,
    latch_status_read_failure, post_confirm_prepared_frame, read_status_sample,
    rejected_wire_diagnostics,
};
#[cfg(test)]
use mister_magik_fb::framebuffer::full_frame_latch::{
    dev_latch_post_skip_for, dev_latch_timeout_for, posted_sequence_observed, read_post_status,
};
use mister_magik_fb::framebuffer::vertical_scale::Rgb565FrameView as VerticalRgb565FrameView;
use mister_magik_fb::latch_readiness::{
    LatchFailure, LatchFailureReason, LatchFailureStage, LatchWireDecision,
};
use std::io;

const TRANSIENT_PENDING_SETTLE_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HiddenSlotRenderGrant {
    pub(crate) slot_index: u8,
    pub(crate) generation: u64,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride_pixels: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CompletedHiddenFrame {
    pub(crate) grant: HiddenSlotRenderGrant,
}

pub(in crate::ui_runner) struct PluginLatchFrameBuffers {
    buffer1: ScanoutSlotsRgb565Framebuffer,
    buffer2: ScanoutSlotsRgb565Framebuffer,
    base1: u32,
    base2: u32,
}

impl PluginLatchFrameBuffers {
    /// Opens both qualified hidden slots for production presentation or bounded diagnostics.
    pub(in crate::ui_runner) fn open(width: usize, height: usize) -> Result<Self, LatchFailure> {
        let stride_bytes = rgb565_stride_bytes(width);
        let buffer1 = open_hidden_buffer(1, width, height, stride_bytes)?;
        let buffer2 = open_hidden_buffer(2, width, height, stride_bytes)?;
        let base1 = hidden_buffer_base(&buffer1, 1)?;
        let base2 = hidden_buffer_base(&buffer2, 2)?;
        Ok(Self {
            buffer1,
            buffer2,
            base1,
            base2,
        })
    }
}

impl LatchFrameBuffers for PluginLatchFrameBuffers {
    type Buffer = ScanoutSlotsRgb565Framebuffer;

    fn base_addr(&self, slot_index: u8) -> u32 {
        if slot_index == 1 {
            self.base1
        } else {
            self.base2
        }
    }

    fn buffer_mut(&mut self, slot_index: u8) -> &mut Self::Buffer {
        if slot_index == 1 {
            &mut self.buffer1
        } else {
            &mut self.buffer2
        }
    }

    fn frame_view(&self, slot_index: u8, width: usize, height: usize) -> Rgb565FrameView<'_> {
        let buffer = if slot_index == 1 {
            &self.buffer1
        } else {
            &self.buffer2
        };
        Rgb565FrameView {
            pixels: buffer.pixels(),
            width,
            height,
            stride_pixels: buffer.stride_pixels(),
        }
    }

    fn copy_rect(
        buffer: &mut Self::Buffer,
        cached: CachedFrameView<'_>,
        rect: DirtyRect,
    ) -> Result<LatchCopyResult, String> {
        let full_source =
            rect.x0 == 0 && rect.y0 == 0 && rect.x1 == cached.width() && rect.y1 == cached.height();
        if full_source && cached.width() == buffer.width() && cached.height() == buffer.height() {
            return buffer
                .copy_full_frame(cached.pixels(), cached.stride())
                .map(|bytes| LatchCopyResult {
                    bytes,
                    path: LatchCopyPath::IdentityFull,
                })
                .map_err(|e| e.to_string());
        }
        buffer
            .copy_vertical_rect(
                VerticalRgb565FrameView {
                    pixels: cached.pixels(),
                    width: cached.width(),
                    height: cached.height(),
                    stride_pixels: cached.stride(),
                },
                rect,
            )
            .map(|bytes| LatchCopyResult {
                bytes,
                path: if full_source {
                    LatchCopyPath::VerticalFull
                } else {
                    LatchCopyPath::VerticalPartial
                },
            })
            .map_err(|e| e.to_string())
    }

    fn publish_writes(buffer: &mut Self::Buffer) {
        buffer.publish_writes();
    }
}

fn open_hidden_buffer(
    slot_index: u8,
    width: usize,
    height: usize,
    stride_bytes: usize,
) -> Result<ScanoutSlotsRgb565Framebuffer, LatchFailure> {
    let index = HiddenRgb565BufferIndex::new(slot_index).map_err(|error| {
        LatchFailure::runtime(
            LatchFailureStage::ModuleLayout,
            LatchFailureReason::ScanoutLayoutMismatch,
            error.to_string(),
        )
    })?;
    match ScanoutSlotsRgb565Framebuffer::open(index, width, height, stride_bytes) {
        Ok(buffer) => Ok(buffer),
        Err(e) => {
            crate::ui_errln!("fpga_vblank_latch_hidden_open_failed buffer={slot_index} error={e}");
            Err(LatchFailure::incompatible(
                LatchFailureStage::BufferMap,
                if matches!(
                    e,
                    mister_magik_fb::framebuffer::scanout_slots::ScanoutSlotsError::InvalidLayout(
                        _
                    )
                ) {
                    LatchFailureReason::ScanoutLayoutMismatch
                } else if matches!(
                    e,
                    mister_magik_fb::framebuffer::scanout_slots::ScanoutSlotsError::InvalidGeometry(
                        _
                    )
                ) {
                    LatchFailureReason::ScanoutGeometryUnsupported
                } else {
                    LatchFailureReason::ScanoutMapFailed
                },
                e.to_string(),
            ))
        }
    }
}

fn hidden_buffer_base(
    buffer: &ScanoutSlotsRgb565Framebuffer,
    slot_index: u8,
) -> Result<u32, LatchFailure> {
    match buffer.physical_addr() {
        Ok(base) => Ok(base),
        Err(e) => {
            crate::ui_errln!(
                "fpga_vblank_latch_hidden_open_failed buffer={slot_index} stage=physical_addr error={e}"
            );
            Err(LatchFailure::runtime(
                LatchFailureStage::ModuleLayout,
                LatchFailureReason::ScanoutLayoutMismatch,
                e.to_string(),
            ))
        }
    }
}

pub(in crate::ui_runner) struct FpgaVblankLatchHiddenPresenter<B = PluginLatchFrameBuffers> {
    buffers: Option<B>,
    base1: u32,
    base2: u32,
    disabled: bool,
    sequence: u16,
    width: usize,
    height: usize,
    render_width: usize,
    render_height: usize,
    latch_geometry: crate::fpga::LatchedFbufGeometry,
    hidden_active_verified: bool,
    capabilities_verified: bool,
    negotiated_capabilities: Option<mister_magik_latch_contract::LatchCapabilities>,
    last_committed_buffer: Option<u8>,
    latch_state: TwoBufferLatchState,
    direct_generation: u64,
    outstanding_direct_grant: Option<HiddenSlotRenderGrant>,
}

#[derive(Debug)]
pub(in crate::ui_runner) struct FpgaVblankLatchHiddenPresentStats {
    pub(in crate::ui_runner) copied_bytes: usize,
    pub(in crate::ui_runner) invalid_bytes: usize,
    pub(in crate::ui_runner) rect_count: u32,
    pub(in crate::ui_runner) catchup_bytes: usize,
    pub(in crate::ui_runner) full_copy: bool,
    pub(in crate::ui_runner) copy_path: LatchCopyPath,
    pub(in crate::ui_runner) buffer_index: u8,
    pub(in crate::ui_runner) copied_rows: u32,
    pub(in crate::ui_runner) copy_us: u128,
    pub(in crate::ui_runner) publish_us: u128,
    pub(in crate::ui_runner) post_us: u128,
    pub(in crate::ui_runner) set_vga_fb_us: u128,
    pub(in crate::ui_runner) status_us: u64,
    pub(in crate::ui_runner) set_supported: bool,
    pub(in crate::ui_runner) status_supported: bool,
    pub(in crate::ui_runner) posted_sequence: u16,
    /// Includes the initial observation; values above one recovered a transient gap.
    pub(in crate::ui_runner) post_status_reads: u8,
    /// Physical GET attempts, including the one permitted transport retry per logical read.
    pub(in crate::ui_runner) post_status_wire_attempts: u8,
    pub(in crate::ui_runner) flip_count: u16,
    pub(in crate::ui_runner) drop_count: u16,
}

impl FpgaVblankLatchHiddenPresenter<PluginLatchFrameBuffers> {
    pub(in crate::ui_runner) fn open(ui: &UiDisplay) -> Result<Self, LatchFailure> {
        let width = ui.fb_w();
        let height = ui.fb_h();
        let buffers = PluginLatchFrameBuffers::open(width, height)?;
        let route = LauncherFramebufferRoute::for_scan(ui.scan_w(), ui.scan_h(), ui.direct_video());
        Ok(Self::new(
            buffers,
            width,
            height,
            ui.render_w(),
            ui.render_h(),
            crate::fpga::LatchedFbufGeometry::new_for_route(
                width as u16,
                route,
                configured_fpga_latch_right_guard_cols(),
            ),
        ))
    }

    pub(in crate::ui_runner) fn take_direct_frame_buffers(
        &mut self,
    ) -> Result<PluginLatchFrameBuffers, LatchFailure> {
        if self.outstanding_direct_grant.is_some() {
            return Err(LatchFailure::runtime(
                LatchFailureStage::BufferMap,
                LatchFailureReason::NoWritableHiddenBuffer,
                "cannot transfer hidden mappings with an outstanding direct grant",
            ));
        }
        self.buffers.take().ok_or_else(|| {
            LatchFailure::runtime(
                LatchFailureStage::BufferMap,
                LatchFailureReason::ScanoutMapFailed,
                "hidden mappings are already owned by a direct renderer",
            )
        })
    }

    pub(in crate::ui_runner) fn restore_direct_frame_buffers(
        &mut self,
        returned: Option<PluginLatchFrameBuffers>,
    ) -> Result<(), LatchFailure> {
        if self.buffers.is_some() {
            return Err(LatchFailure::runtime(
                LatchFailureStage::BufferMap,
                LatchFailureReason::ScanoutMapFailed,
                "hidden mappings returned while presenter mappings are still active",
            ));
        }
        let buffers = match returned {
            Some(buffers) => buffers,
            None => PluginLatchFrameBuffers::open(self.width, self.height)?,
        };
        if buffers.base_addr(1) != self.base1 || buffers.base_addr(2) != self.base2 {
            return Err(LatchFailure::runtime(
                LatchFailureStage::ModuleLayout,
                LatchFailureReason::ScanoutLayoutMismatch,
                "returned hidden mappings do not match the presenter's physical slots",
            ));
        }
        self.buffers = Some(buffers);
        Ok(())
    }
}

impl<B: LatchFrameBuffers> FpgaVblankLatchHiddenPresenter<B> {
    fn new(
        buffers: B,
        width: usize,
        height: usize,
        render_width: usize,
        render_height: usize,
        latch_geometry: crate::fpga::LatchedFbufGeometry,
    ) -> Self {
        let base1 = buffers.base_addr(1);
        let base2 = buffers.base_addr(2);
        Self {
            buffers: Some(buffers),
            base1,
            base2,
            disabled: false,
            sequence: 1,
            width,
            height,
            render_width,
            render_height,
            latch_geometry,
            hidden_active_verified: false,
            capabilities_verified: false,
            negotiated_capabilities: None,
            last_committed_buffer: None,
            latch_state: TwoBufferLatchState::new(render_width, render_height),
            direct_generation: 0,
            outstanding_direct_grant: None,
        }
    }

    fn base_addr(&self, slot_index: u8) -> u32 {
        if slot_index == 1 {
            self.base1
        } else {
            self.base2
        }
    }

    fn buffers(&self) -> &B {
        self.buffers
            .as_ref()
            .expect("hidden mappings must be restored before copied presentation")
    }

    pub(in crate::ui_runner) fn exact_identity_geometry(&self) -> bool {
        self.width == self.render_width && self.height == self.render_height
    }

    pub(in crate::ui_runner) fn try_issue_hidden_slot_render_grant<H: LatchHardware>(
        &mut self,
        hardware: &mut H,
        display_session: &mut LauncherDisplaySession,
    ) -> Result<Option<HiddenSlotRenderGrant>, LatchFailure> {
        self.try_issue_external_hidden_slot_render_grant(hardware, display_session, true)
    }

    pub(in crate::ui_runner) fn try_issue_startup_intro_hidden_slot_render_grant<
        H: LatchHardware,
    >(
        &mut self,
        hardware: &mut H,
        display_session: &mut LauncherDisplaySession,
    ) -> Result<Option<HiddenSlotRenderGrant>, LatchFailure> {
        self.try_issue_external_hidden_slot_render_grant(hardware, display_session, false)
    }

    fn try_issue_external_hidden_slot_render_grant<H: LatchHardware>(
        &mut self,
        hardware: &mut H,
        _display_session: &mut LauncherDisplaySession,
        require_identity_geometry: bool,
    ) -> Result<Option<HiddenSlotRenderGrant>, LatchFailure> {
        if self.disabled
            || (require_identity_geometry && !self.exact_identity_geometry())
            || self.outstanding_direct_grant.is_some()
        {
            return Ok(None);
        }
        self.verify_capabilities(hardware)?;
        let sample = self.read_geometry_safe_status(hardware)?;
        let status = sample.status;
        if status.pending() {
            return Ok(None);
        }
        let Some(slot_index) = self.latch_state.writable_slot_index() else {
            return Ok(None);
        };
        self.direct_generation = self.direct_generation.wrapping_add(1).max(1);
        let grant = HiddenSlotRenderGrant {
            slot_index,
            generation: self.direct_generation,
            width: self.width,
            height: self.height,
            stride_pixels: self.width,
        };
        self.outstanding_direct_grant = Some(grant);
        Ok(Some(grant))
    }

    pub(in crate::ui_runner) fn present_completed_hidden_frame<H: LatchHardware>(
        &mut self,
        completed: CompletedHiddenFrame,
        hardware: &mut H,
        _display_session: &mut LauncherDisplaySession,
    ) -> Result<FpgaVblankLatchHiddenPresentStats, LatchFailure> {
        let grant = completed.grant;
        if self.outstanding_direct_grant != Some(grant) {
            return Err(LatchFailure::runtime(
                LatchFailureStage::PostVerification,
                LatchFailureReason::PostedSequenceUnverified,
                format!(
                    "stale external hidden frame slot={} generation={} expected={:?}",
                    grant.slot_index, grant.generation, self.outstanding_direct_grant
                ),
            ));
        }
        let status_started = Instant::now();
        let before_sample = self.read_geometry_safe_status(hardware)?;
        let before_status = before_sample.status;
        if before_status.pending() || !self.latch_state.slot_is_writable(grant.slot_index) {
            return Err(LatchFailure::runtime(
                LatchFailureStage::PostVerification,
                LatchFailureReason::NoWritableHiddenBuffer,
                format!(
                    "external slot {} became active or pending before post",
                    grant.slot_index
                ),
            )
            .with_wire_diagnostics(rejected_wire_diagnostics(before_sample.diagnostics)));
        }
        let mut status_us = status_started.elapsed().as_micros() as u64;
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let receipt = post_confirm_prepared_frame(
            hardware,
            LatchPostRequest {
                sequence,
                slot_index: grant.slot_index,
                base_addr: self.base_addr(grant.slot_index),
                width: self.width as u16,
                height: self.height as u16,
                geometry: self.latch_geometry,
            },
            |hardware, budget| self.read_geometry_safe_status_with_budget(hardware, budget),
        )?;
        // Retained in the accounting schema for compatibility. Main_MiSTer
        // exclusively owns UIO_BUT_SW and the VGA framebuffer mux.
        let set_vga_fb_us = 0;
        let after_status = receipt.status;
        status_us = status_us.saturating_add(receipt.status_us);
        self.outstanding_direct_grant = None;
        self.last_committed_buffer = Some(grant.slot_index);
        self.latch_state.invalidate_all();
        self.hidden_active_verified = true;
        Ok(FpgaVblankLatchHiddenPresentStats {
            copied_bytes: 0,
            invalid_bytes: 0,
            rect_count: 0,
            catchup_bytes: 0,
            full_copy: false,
            copy_path: LatchCopyPath::ExternalDirect,
            buffer_index: grant.slot_index,
            copied_rows: 0,
            copy_us: 0,
            publish_us: 0,
            post_us: receipt.post_us,
            set_vga_fb_us,
            status_us,
            set_supported: receipt.set_supported,
            status_supported: receipt.status_supported,
            posted_sequence: sequence,
            post_status_reads: receipt.status_reads,
            post_status_wire_attempts: receipt.status_wire_attempts,
            flip_count: after_status.flip_count,
            drop_count: after_status.drop_count,
        })
    }

    pub(in crate::ui_runner) fn invalidate_external_mode(&mut self) {
        self.direct_generation = self.direct_generation.wrapping_add(1).max(1);
        self.outstanding_direct_grant = None;
        self.last_committed_buffer = None;
        self.latch_state.invalidate_all();
    }

    fn verify_capabilities<H: LatchHardware>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), LatchFailure> {
        if self.capabilities_verified {
            return Ok(());
        }
        let (magic_hi, magic_lo, capabilities) =
            hardware.read_latch_capabilities().map_err(|error| {
                LatchFailure::runtime(
                    LatchFailureStage::FpgaCapabilities,
                    LatchFailureReason::FpgaTransportFailed,
                    error.to_string(),
                )
            })?;
        let supported = magic_hi == crate::fpga::MAGIK_FBUF_CAPS_MAGIC
            || magic_lo == crate::fpga::MAGIK_FBUF_CAPS_MAGIC;
        if !supported || !capabilities.production_ready() {
            self.disabled = true;
            return Err(LatchFailure::incompatible(
                LatchFailureStage::FpgaCapabilities,
                if supported {
                    LatchFailureReason::FpgaCapabilitiesInsufficient
                } else {
                    LatchFailureReason::FpgaProtocolUnsupported
                },
                format!(
                    "magic=0x{magic_hi:04x}/0x{magic_lo:04x} protocol={} flags=0x{:04x} max={}x{} stride={}",
                    capabilities.protocol_version,
                    capabilities.flags,
                    capabilities.max_width,
                    capabilities.max_height,
                    capabilities.max_stride_bytes
                ),
            ));
        }
        self.negotiated_capabilities = Some(capabilities);
        self.capabilities_verified = true;
        Ok(())
    }

    pub(in crate::ui_runner) fn present_cached_full_frame<H, F>(
        &mut self,
        cached: CachedFrameView<'_>,
        input: LauncherFramePlan,
        hardware: &mut H,
        _display_session: &mut LauncherDisplaySession,
        apply_overlays: F,
    ) -> Result<FpgaVblankLatchHiddenPresentStats, LatchFailure>
    where
        H: LatchHardware,
        F: FnOnce(&mut B::Buffer, LatchPresentPlan) -> Result<(), String>,
    {
        if self.disabled {
            return Err(LatchFailure::incompatible(
                LatchFailureStage::FpgaStatus,
                LatchFailureReason::FpgaStatusUnsupported,
                "presenter disabled after unsupported command response",
            ));
        }

        self.verify_capabilities(hardware)?;

        let status_start = Instant::now();
        let mut before_sample = self.read_geometry_safe_status(hardware)?;
        let mut before_status = before_sample.status;
        let mut status_us = status_start.elapsed().as_micros() as u64;

        let mut plan = self.latch_state.plan_next(input);
        if plan.is_none() && before_status.pending() {
            let settle_started = Instant::now();
            while settle_started.elapsed() < TRANSIENT_PENDING_SETTLE_TIMEOUT {
                std::thread::sleep(Duration::from_millis(1));
                let retry_started = Instant::now();
                before_sample = self.read_geometry_safe_status(hardware)?;
                before_status = before_sample.status;
                status_us = status_us.saturating_add(retry_started.elapsed().as_micros() as u64);
                plan = self.latch_state.plan_next(input);
                if plan.is_some() || !before_status.pending() {
                    break;
                }
            }
        }
        let plan = plan.ok_or_else(|| {
            LatchFailure::runtime(
                LatchFailureStage::PostVerification,
                LatchFailureReason::NoWritableHiddenBuffer,
                "both hidden buffers are active or pending",
            )
            .with_wire_diagnostics(rejected_wire_diagnostics(before_sample.diagnostics))
        })?;
        let buffer_index = plan.slot_index;
        let invalid_bytes = self.latch_state.restore_bytes_for_slot(buffer_index);
        let rect_count = plan.restore_rects.len() as u32;
        // Damage remains in composition coordinates; analytics report the
        // destination rows that the native scanout copy actually touched.
        let vertical_transform =
            mister_magik_fb::framebuffer::vertical_scale::VerticalRgb565Transform::new(
                self.width,
                self.render_height,
                self.height,
            )
            .map_err(|error| {
                LatchFailure::runtime(
                    LatchFailureStage::FrameCopy,
                    LatchFailureReason::FrameCopyFailed,
                    error,
                )
            })?;
        let copied_rows = plan
            .restore_rects
            .iter()
            .filter_map(|rect| {
                vertical_transform.destination_rect_for_source(
                    mister_magik_fb::framebuffer::vertical_scale::VerticalRect {
                        x0: rect.x0,
                        y0: rect.y0,
                        x1: rect.x1,
                        y1: rect.y1,
                    },
                )
            })
            .map(|rect| rect.rows() as u32)
            .sum::<u32>();
        let full_copy = rect_list_contains(plan.restore_rects, self.full_rect());
        let catchup_bytes = plan.restore_rects.total_rgb565_bytes();
        let base_addr = self.base_addr(buffer_index);
        let buffer = self
            .buffers
            .as_mut()
            .expect("hidden mappings must be restored before copied presentation")
            .buffer_mut(buffer_index);
        let copy_start = Instant::now();
        let mut copied_bytes = 0usize;
        let mut copy_path = LatchCopyPath::IdentityFull;
        for rect in plan.restore_rects.iter() {
            match B::copy_rect(buffer, cached, rect) {
                Ok(result) => {
                    copied_bytes = copied_bytes.saturating_add(result.bytes);
                    if result.path != LatchCopyPath::IdentityFull {
                        copy_path = result.path;
                    }
                }
                Err(e) => {
                    self.latch_state.mark_attempt_failed(buffer_index);
                    return Err(LatchFailure::runtime(
                        LatchFailureStage::FrameCopy,
                        LatchFailureReason::FrameCopyFailed,
                        e,
                    ));
                }
            }
        }
        let copy_us = copy_start.elapsed().as_micros();
        if let Err(e) = apply_overlays(buffer, plan) {
            self.latch_state.mark_attempt_failed(buffer_index);
            return Err(LatchFailure::runtime(
                LatchFailureStage::OverlayCompose,
                LatchFailureReason::OverlayComposeFailed,
                e,
            ));
        }
        let publish_start = Instant::now();
        B::publish_writes(buffer);
        let publish_us = publish_start.elapsed().as_micros();

        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let receipt = match post_confirm_prepared_frame(
            hardware,
            LatchPostRequest {
                sequence,
                slot_index: buffer_index,
                base_addr,
                width: self.width as u16,
                height: self.height as u16,
                geometry: self.latch_geometry,
            },
            |hardware, budget| self.read_geometry_safe_status_with_budget(hardware, budget),
        ) {
            Ok(receipt) => receipt,
            Err(failure) => {
                self.latch_state.mark_attempt_failed(buffer_index);
                if failure.reason == LatchFailureReason::FpgaStatusUnsupported {
                    self.disabled = true;
                }
                return Err(failure);
            }
        };
        // Retained in the accounting schema for compatibility. Main_MiSTer
        // exclusively owns UIO_BUT_SW and the VGA framebuffer mux.
        let set_vga_fb_us = 0;
        let after_status = receipt.status;
        status_us = status_us.saturating_add(receipt.status_us);
        let flip_count = after_status.flip_count;
        let drop_count = after_status.drop_count;

        self.latch_state.mark_post_success(plan);
        self.last_committed_buffer = Some(buffer_index);
        Ok(FpgaVblankLatchHiddenPresentStats {
            copied_bytes,
            invalid_bytes,
            rect_count,
            catchup_bytes,
            full_copy,
            copy_path,
            buffer_index,
            copied_rows,
            copy_us,
            publish_us,
            post_us: receipt.post_us,
            set_vga_fb_us,
            status_us,
            set_supported: receipt.set_supported,
            status_supported: receipt.status_supported,
            posted_sequence: sequence,
            post_status_reads: receipt.status_reads,
            post_status_wire_attempts: receipt.status_wire_attempts,
            flip_count,
            drop_count,
        })
    }

    pub(in crate::ui_runner) fn committed_frame_view(
        &self,
        buffer_index: u8,
    ) -> Rgb565FrameView<'_> {
        self.buffers()
            .frame_view(buffer_index, self.width, self.height)
    }

    pub(in crate::ui_runner) fn committed_frame_view_if_mapped(
        &self,
        buffer_index: u8,
    ) -> Option<Rgb565FrameView<'_>> {
        self.buffers
            .as_ref()
            .map(|buffers| buffers.frame_view(buffer_index, self.width, self.height))
    }

    pub(in crate::ui_runner) fn buffer_base_addr(&self, buffer_index: u8) -> u32 {
        self.base_addr(buffer_index)
    }

    pub(in crate::ui_runner) fn publish_requested_full_snapshot(&self) -> bool {
        if self.buffers.is_none()
            || !mister_magik_fb::framebuffer::stream::adaptive_full_snapshot_due()
        {
            return false;
        }
        let Some(buffer_index) = self.last_committed_buffer else {
            return false;
        };
        mister_magik_fb::framebuffer::stream::publish_latch_snapshot(
            self.committed_frame_view(buffer_index),
            mister_magik_fb::framebuffer::stream::LatchStreamScale::Full,
        )
        .queued
    }

    fn full_rect(&self) -> DirtyRect {
        DirtyRect {
            x0: 0,
            y0: 0,
            x1: self.render_width,
            y1: self.render_height,
        }
    }

    fn classify_latch_status(
        &self,
        status: crate::fpga::LatchedFbufStatus,
    ) -> Result<LatchStatusSync, LatchStatusSyncError> {
        classify_latch_status(
            status,
            self.base_addr(1),
            self.base_addr(2),
            self.width,
            self.height,
            self.hidden_active_verified,
        )
    }

    fn read_geometry_safe_status(
        &mut self,
        hardware: &mut impl LatchHardware,
    ) -> Result<crate::fpga::LatchedFbufStatusSample, LatchFailure> {
        let mut budget = LogicalStatusReadBudget::new();
        self.read_geometry_safe_status_with_budget(hardware, &mut budget)
    }

    fn read_geometry_safe_status_with_budget(
        &mut self,
        hardware: &mut impl LatchHardware,
        budget: &mut LogicalStatusReadBudget,
    ) -> Result<crate::fpga::LatchedFbufStatusSample, LatchFailure> {
        budget.consume().map_err(|_| {
            LatchFailure::runtime(
                LatchFailureStage::PostVerification,
                LatchFailureReason::FpgaTransportFailed,
                "latch status exhausted its bounded read budget",
            )
        })?;
        let mut sample = read_status_sample(hardware, self.negotiated_capabilities)
            .map_err(|error| latch_status_read_failure(LatchFailureStage::FpgaStatus, error))?;
        if !matches!(
            self.classify_latch_status(sample.status),
            Err(LatchStatusSyncError::HiddenGeometryMismatch { .. })
        ) {
            self.sync_latch_state_from_status(&sample)?;
            return Ok(sample);
        }

        let fallback_status = sample.status;
        let mut previous = LatchSafetyProjection::from(sample.status);
        let mut diagnostics = std::mem::take(&mut sample.diagnostics);
        while !budget.exhausted() {
            budget.consume().map_err(|_| {
                LatchFailure::runtime(
                    LatchFailureStage::PostVerification,
                    LatchFailureReason::FpgaTransportFailed,
                    "latch status exhausted its bounded read budget",
                )
            })?;
            let mut next = read_status_sample(hardware, self.negotiated_capabilities)
                .map_err(|error| latch_status_read_failure(LatchFailureStage::FpgaStatus, error))?;
            diagnostics.append(&next.diagnostics);
            let projection = LatchSafetyProjection::from(next.status);
            if projection == previous {
                diagnostics.decision = LatchWireDecision::Corroborated;
                next.diagnostics = diagnostics;
                self.sync_latch_state_from_status(&next)?;
                return Ok(next);
            }
            previous = projection;
        }
        sample.status = fallback_status;
        sample.diagnostics = diagnostics;
        self.sync_latch_state_from_status(&sample)?;
        Ok(sample)
    }

    fn sync_latch_state_from_status(
        &mut self,
        sample: &crate::fpga::LatchedFbufStatusSample,
    ) -> Result<(), LatchFailure> {
        let status = sample.status;
        let sync = match self.classify_latch_status(status) {
            Ok(sync) => sync,
            Err(LatchStatusSyncError::Unsupported { magic_hi, magic_lo }) => {
                self.disabled = true;
                return Err(LatchFailure::incompatible(
                    LatchFailureStage::FpgaStatus,
                    LatchFailureReason::FpgaStatusUnsupported,
                    format!("ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"),
                )
                .with_wire_diagnostics(rejected_wire_diagnostics(sample.diagnostics.clone())));
            }
            Err(LatchStatusSyncError::HiddenGeometryMismatch {
                active_width,
                active_height,
                active_stride,
                expected_width,
                expected_height,
                expected_stride,
            }) => {
                self.latch_state.invalidate_all();
                self.hidden_active_verified = false;
                return Err(LatchFailure::runtime(
                    LatchFailureStage::PostVerification,
                    LatchFailureReason::ActiveGeometryMismatch,
                    format!(
                        "latched framebuffer geometry mismatch active={active_width}x{active_height} stride={active_stride} expected={expected_width}x{expected_height} stride={expected_stride}"
                    ),
                )
                .with_wire_diagnostics(rejected_wire_diagnostics(
                    sample.diagnostics.clone(),
                )));
            }
        };

        self.apply_latch_status_sync(sync);
        Ok(())
    }

    fn apply_latch_status_sync(&mut self, sync: LatchStatusSync) {
        if sync.recovered_non_hidden_active {
            self.latch_state.invalidate_all();
            self.hidden_active_verified = false;
        }
        if sync.hidden_active_verified {
            self.hidden_active_verified = true;
        }
        self.latch_state.sync_hardware(
            sync.active_slot,
            sync.active_sequence,
            sync.pending,
            sync.pending_sequence,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LatchStatusSync {
    active_slot: Option<u8>,
    active_sequence: u16,
    pending: bool,
    pending_sequence: u16,
    hidden_active_verified: bool,
    recovered_non_hidden_active: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LatchStatusSyncError {
    Unsupported {
        magic_hi: u16,
        magic_lo: u16,
    },
    HiddenGeometryMismatch {
        active_width: u16,
        active_height: u16,
        active_stride: u16,
        expected_width: u16,
        expected_height: u16,
        expected_stride: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LatchSafetyProjection {
    active_enabled: bool,
    active_base: u32,
    active_width: u16,
    active_height: u16,
    active_stride: u16,
    active_sequence: u16,
}

impl From<crate::fpga::LatchedFbufStatus> for LatchSafetyProjection {
    fn from(status: crate::fpga::LatchedFbufStatus) -> Self {
        Self {
            active_enabled: status.active_enabled(),
            active_base: status.active_base,
            active_width: status.active_width,
            active_height: status.active_height,
            active_stride: status.active_stride,
            active_sequence: status.active_sequence,
        }
    }
}

fn wait_for_latch_completion_with(
    mut read_status: impl FnMut() -> io::Result<crate::fpga::LatchedFbufStatus>,
    posted_sequence: u16,
    timeout: Duration,
    mut yield_wait: impl FnMut(),
) -> Result<LatchCompletion, LatchFailure> {
    let started = Instant::now();
    let cpu_started = thread_cpu_us();
    let mut poll_count = 0u16;
    let mut post_observed = false;
    loop {
        let status = read_status().map_err(|error| {
            LatchFailure::runtime(
                LatchFailureStage::PostVerification,
                LatchFailureReason::FpgaTransportFailed,
                error.to_string(),
            )
        })?;
        poll_count = poll_count.saturating_add(1);
        if !status.supported() {
            return Err(LatchFailure::incompatible(
                LatchFailureStage::PostVerification,
                LatchFailureReason::FpgaStatusUnsupported,
                "latch completion status is unsupported",
            ));
        }
        if !status.pending() && status.active_sequence == posted_sequence {
            return Ok(LatchCompletion {
                status,
                poll_count,
                wall_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                cpu_us: elapsed_thread_cpu_us(cpu_started),
            });
        }
        post_observed |= status.pending() && status.pending_sequence == posted_sequence;
        if started.elapsed() >= timeout {
            return Err(LatchFailure::runtime(
                LatchFailureStage::PostVerification,
                LatchFailureReason::PostedSequenceUnverified,
                format!(
                    "latch completion timed out posted={posted_sequence} pending_observed={} final_active={} final_pending={} final_pending_sequence={} polls={poll_count}",
                    u8::from(post_observed),
                    status.active_sequence,
                    u8::from(status.pending()),
                    status.pending_sequence,
                ),
            ));
        }
        yield_wait();
    }
}

#[cfg(target_os = "linux")]
fn thread_cpu_us() -> Option<u64> {
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, time.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let time = unsafe { time.assume_init() };
    Some(
        u64::try_from(time.tv_sec)
            .unwrap_or(0)
            .saturating_mul(1_000_000)
            .saturating_add(u64::try_from(time.tv_nsec).unwrap_or(0) / 1_000),
    )
}

#[cfg(not(target_os = "linux"))]
fn thread_cpu_us() -> Option<u64> {
    None
}

fn elapsed_thread_cpu_us(start: Option<u64>) -> u64 {
    start
        .and_then(|start| thread_cpu_us().map(|end| end.saturating_sub(start)))
        .unwrap_or(0)
}

fn classify_latch_status(
    status: crate::fpga::LatchedFbufStatus,
    base1: u32,
    base2: u32,
    width: usize,
    height: usize,
    hidden_active_verified: bool,
) -> Result<LatchStatusSync, LatchStatusSyncError> {
    if !status.supported() {
        return Err(LatchStatusSyncError::Unsupported {
            magic_hi: status.magic_hi,
            magic_lo: status.magic_lo,
        });
    }
    let pending = status.pending();
    let pending_sequence = status.pending_sequence;
    if !status.active_enabled() {
        return Ok(LatchStatusSync {
            active_slot: None,
            active_sequence: status.active_sequence,
            pending,
            pending_sequence,
            hidden_active_verified: false,
            recovered_non_hidden_active: false,
        });
    }
    let active_slot = match status.active_base {
        base if base == base1 => Some(1),
        base if base == base2 => Some(2),
        _ => None,
    };
    if let Some(active_slot) = active_slot {
        let expected_width = width as u16;
        let expected_height = height as u16;
        let expected_stride = rgb565_stride_bytes(width) as u16;
        if status.active_width != expected_width
            || status.active_height != expected_height
            || status.active_stride != expected_stride
        {
            return Err(LatchStatusSyncError::HiddenGeometryMismatch {
                active_width: status.active_width,
                active_height: status.active_height,
                active_stride: status.active_stride,
                expected_width,
                expected_height,
                expected_stride,
            });
        }
        return Ok(LatchStatusSync {
            active_slot: Some(active_slot),
            active_sequence: status.active_sequence,
            pending,
            pending_sequence,
            hidden_active_verified: true,
            recovered_non_hidden_active: false,
        });
    }
    Ok(LatchStatusSync {
        active_slot: None,
        active_sequence: status.active_sequence,
        pending,
        pending_sequence,
        hidden_active_verified: false,
        recovered_non_hidden_active: hidden_active_verified,
    })
}

fn rect_list_contains(list: DirtyRectList, target: DirtyRect) -> bool {
    list.iter().any(|rect| rect == target)
}

fn configured_fpga_latch_right_guard_cols() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("MISTER_FB_RIGHT_GUARD_COLS")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_fb::framebuffer::ownership::FramebufferRouteGuard;
    use std::cell::RefCell;
    use std::rc::Rc;

    const WIDTH: usize = 4;
    const HEIGHT: usize = 3;
    const BASE1: u32 = 0x227e_9000;
    const BASE2: u32 = 0x22fd_2000;
    const FRONT_BASE: u32 = 0x2200_1000;

    #[test]
    fn latch_timeout_injection_is_valid_only_for_development_layout() {
        assert_eq!(
            dev_latch_timeout_for(
                std::path::Path::new("/media/fat/mister-magik-dev/mister-magik-fb"),
                Some("17"),
            ),
            Some(17)
        );
        assert_eq!(
            dev_latch_timeout_for(
                std::path::Path::new("/media/fat/mister-magik/mister-magik-fb"),
                Some("17"),
            ),
            None
        );
        assert_eq!(
            dev_latch_timeout_for(
                std::path::Path::new("/media/fat/mister-magik-dev/mister-magik-fb"),
                Some("0"),
            ),
            None
        );
        assert_eq!(
            dev_latch_timeout_for(
                std::path::Path::new("/media/fat/mister-magik-dev/mister-magik-fb"),
                Some("invalid"),
            ),
            None
        );
    }

    #[test]
    fn latch_post_skip_injection_is_valid_only_for_development_layout() {
        assert_eq!(
            dev_latch_post_skip_for(
                std::path::Path::new("/media/fat/mister-magik-dev/mister-magik-fb"),
                Some("4"),
            ),
            Some(4)
        );
        assert_eq!(
            dev_latch_post_skip_for(
                std::path::Path::new("/media/fat/mister-magik/mister-magik-fb"),
                Some("4"),
            ),
            None
        );
        assert_eq!(
            dev_latch_post_skip_for(
                std::path::Path::new("/media/fat/mister-magik-dev/mister-magik-fb"),
                Some("12"),
            ),
            None
        );
        assert_eq!(
            dev_latch_post_skip_for(
                std::path::Path::new("/media/fat/mister-magik-dev/mister-magik-fb"),
                Some("invalid"),
            ),
            None
        );
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestEvent {
        ReadStatus,
        Copy,
        Overlay,
        Publish,
        Post,
    }

    type EventLog = Rc<RefCell<Vec<TestEvent>>>;

    struct FakeBuffer {
        copy_count: usize,
        events: EventLog,
        pixels: Vec<Rgb565Pixel>,
    }

    impl FakeBuffer {
        fn new(events: EventLog) -> Self {
            Self {
                copy_count: 0,
                events,
                pixels: vec![Rgb565Pixel(0); WIDTH * HEIGHT],
            }
        }
    }

    struct FakeBuffers {
        buffers: [FakeBuffer; 2],
    }

    impl FakeBuffers {
        fn new(events: EventLog) -> Self {
            Self {
                buffers: [FakeBuffer::new(events.clone()), FakeBuffer::new(events)],
            }
        }
    }

    impl LatchFrameBuffers for FakeBuffers {
        type Buffer = FakeBuffer;

        fn base_addr(&self, slot_index: u8) -> u32 {
            if slot_index == 1 { BASE1 } else { BASE2 }
        }

        fn buffer_mut(&mut self, slot_index: u8) -> &mut Self::Buffer {
            &mut self.buffers[usize::from(slot_index - 1)]
        }

        fn frame_view(&self, slot_index: u8, width: usize, height: usize) -> Rgb565FrameView<'_> {
            Rgb565FrameView {
                pixels: &self.buffers[usize::from(slot_index - 1)].pixels,
                width,
                height,
                stride_pixels: width,
            }
        }

        fn copy_rect(
            buffer: &mut Self::Buffer,
            cached: CachedFrameView<'_>,
            rect: DirtyRect,
        ) -> Result<LatchCopyResult, String> {
            buffer.events.borrow_mut().push(TestEvent::Copy);
            buffer.copy_count += 1;
            for y in rect.y0..rect.y1 {
                let start = y * WIDTH + rect.x0;
                let end = y * WIDTH + rect.x1;
                buffer.pixels[start..end].copy_from_slice(&cached.pixels()[start..end]);
            }
            Ok(LatchCopyResult {
                bytes: rect.width() * rect.rows() as usize * 2,
                path: if rect.x0 == 0
                    && rect.y0 == 0
                    && rect.x1 == cached.width()
                    && rect.y1 == cached.height()
                {
                    LatchCopyPath::IdentityFull
                } else {
                    LatchCopyPath::VerticalPartial
                },
            })
        }

        fn publish_writes(buffer: &mut Self::Buffer) {
            buffer.events.borrow_mut().push(TestEvent::Publish);
        }
    }

    #[derive(Default)]
    struct FakeHardware {
        statuses: Vec<io::Result<crate::fpga::LatchedFbufStatus>>,
        status_diagnostics: Vec<mister_magik_fb::latch_readiness::LatchWireDiagnostics>,
        posts: Vec<io::Result<(u16, u16)>>,
        read_count: usize,
        post_bases: Vec<u32>,
        last_posted_sequence: Option<u16>,
        posted_sequence_visibility_delay_reads: usize,
        events: Option<EventLog>,
    }

    impl LauncherDisplayHardware for FakeHardware {
        fn enable_launcher_route(
            &mut self,
            _route: LauncherFramebufferRoute,
            _fb_width: usize,
            _fb_height: usize,
        ) -> io::Result<u16> {
            Ok(1)
        }
    }

    impl LatchHardware for FakeHardware {
        fn read_latch_capabilities(
            &mut self,
        ) -> io::Result<(u16, u16, mister_magik_latch_contract::LatchCapabilities)> {
            Ok((
                crate::fpga::MAGIK_FBUF_CAPS_MAGIC,
                0,
                mister_magik_latch_contract::decode_capabilities(&[
                    mister_magik_latch_contract::PROTOCOL_VERSION,
                    mister_magik_latch_contract::REQUIRED_CAPS,
                    1366,
                    768,
                    2736,
                    mister_magik_latch_contract::GOLDEN_CAPS_V5_CRC,
                ])
                .map_err(io::Error::other)?,
            ))
        }

        fn read_latched_status(
            &mut self,
        ) -> Result<crate::fpga::LatchedFbufStatusSample, crate::fpga::LatchedFbufStatusReadError>
        {
            if let Some(events) = &self.events {
                events.borrow_mut().push(TestEvent::ReadStatus);
            }
            self.read_count += 1;
            let diagnostics = if self.status_diagnostics.is_empty() {
                Default::default()
            } else {
                self.status_diagnostics.remove(0)
            };
            let mut status = match self.statuses.remove(0) {
                Ok(status) => status,
                Err(error) => {
                    let mut error = crate::fpga::LatchedFbufStatusReadError::from_io(error);
                    error.diagnostics = Box::new(diagnostics);
                    return Err(error);
                }
            };
            if self.posted_sequence_visibility_delay_reads > 0 {
                self.posted_sequence_visibility_delay_reads -= 1;
            } else if let Some(sequence) = self.last_posted_sequence.take() {
                status.pending_sequence = sequence;
                status.flags |= 0x0004;
            }
            Ok(crate::fpga::LatchedFbufStatusSample {
                status,
                diagnostics,
            })
        }

        fn negotiated_latch_capabilities(
            &self,
        ) -> Option<mister_magik_latch_contract::LatchCapabilities> {
            Some(
                mister_magik_latch_contract::decode_capabilities(&[
                    mister_magik_latch_contract::PROTOCOL_VERSION,
                    mister_magik_latch_contract::REQUIRED_CAPS,
                    1366,
                    768,
                    2736,
                    mister_magik_latch_contract::GOLDEN_CAPS_V5_CRC,
                ])
                .unwrap(),
            )
        }

        fn post_latched_rgb565(
            &mut self,
            sequence: u16,
            base_addr: u32,
            _fb_width: u16,
            _fb_height: u16,
            _geometry: crate::fpga::LatchedFbufGeometry,
        ) -> Result<crate::fpga::LatchedFbufPostAttempt, crate::fpga::LatchedFbufPostError>
        {
            if let Some(events) = &self.events {
                events.borrow_mut().push(TestEvent::Post);
            }
            self.post_bases.push(base_addr);
            let result = if self.posts.is_empty() {
                Ok((crate::fpga::MAGIK_FBUF_LATCH_MAGIC, 0))
            } else {
                self.posts.remove(0)
            };
            match result {
                Ok((ack_high, ack_low)) => {
                    self.last_posted_sequence = Some(sequence);
                    Ok(crate::fpga::LatchedFbufPostAttempt {
                        ack_high,
                        ack_low,
                        diagnostics: mister_magik_fb::latch_readiness::LatchPostDiagnostics {
                            protocol_version: 5,
                            sequence,
                            expected_word_count: mister_magik_latch_contract::V5_SET_WORDS as u8,
                            transmitted_word_count: mister_magik_latch_contract::V5_SET_WORDS as u8,
                            ..Default::default()
                        },
                    })
                }
                Err(error) => Err(crate::fpga::LatchedFbufPostError::from_io(error)),
            }
        }
    }

    fn status(active_base: u32, flags: u16) -> crate::fpga::LatchedFbufStatus {
        crate::fpga::LatchedFbufStatus {
            magic_hi: crate::fpga::MAGIK_FBUF_STATUS_MAGIC,
            magic_lo: 0,
            active_sequence: 11,
            pending_sequence: 12,
            flags,
            flip_count: 3,
            post_count: 4,
            drop_count: 5,
            active_base,
            active_width: WIDTH as u16,
            active_height: HEIGHT as u16,
            active_stride: rgb565_stride_bytes(WIDTH) as u16,
            reject_count: 0,
            active_route_epoch: 0,
            accepted_sequence: if flags & 0x0004 != 0 { 12 } else { 11 },
            active_transaction: 1,
            pending_transaction: if flags & 0x0004 != 0 { 2 } else { 0 },
            accepted_transaction: if flags & 0x0004 != 0 { 2 } else { 1 },
        }
    }

    fn wire_diagnostics(command: u16) -> mister_magik_fb::latch_readiness::LatchWireDiagnostics {
        let mut diagnostics = mister_magik_fb::latch_readiness::LatchWireDiagnostics::default();
        diagnostics.push_attempt(mister_magik_fb::latch_readiness::LatchWireAttempt {
            command,
            ..Default::default()
        });
        diagnostics
    }

    fn wire_transport_failure(
        commands: &[u16],
    ) -> mister_magik_fb::latch_readiness::LatchWireDiagnostics {
        let mut diagnostics = mister_magik_fb::latch_readiness::LatchWireDiagnostics {
            decision: LatchWireDecision::TransportRetryFailed,
            ..Default::default()
        };
        for command in commands {
            diagnostics.push_attempt(mister_magik_fb::latch_readiness::LatchWireAttempt {
                command: *command,
                ..Default::default()
            });
        }
        diagnostics
    }

    fn presenter_with_events(events: EventLog) -> FpgaVblankLatchHiddenPresenter<FakeBuffers> {
        let plan =
            UiDisplayPlan::from_mister_ini_text("[Menu]\nvideo_mode=8\n[MiSTer]\ndirect_video=1\n")
                .expect("display plan");
        let route = LauncherFramebufferRoute::for_scan(plan.scan_w, plan.scan_h, plan.direct_video);
        FpgaVblankLatchHiddenPresenter::new(
            FakeBuffers::new(events),
            WIDTH,
            HEIGHT,
            WIDTH,
            HEIGHT,
            crate::fpga::LatchedFbufGeometry::new(WIDTH as u16, route.mode(), 1),
        )
    }

    fn presenter() -> FpgaVblankLatchHiddenPresenter<FakeBuffers> {
        presenter_with_events(EventLog::default())
    }

    fn display_session() -> LauncherDisplaySession {
        let plan =
            UiDisplayPlan::from_mister_ini_text("[Menu]\nvideo_mode=8\n[MiSTer]\ndirect_video=1\n")
                .expect("display plan");
        let ui = UiDisplay::for_plan(plan);
        LauncherDisplaySession::with_guard(&ui, FramebufferRouteGuard::disabled())
    }

    fn frame_plan() -> LauncherFramePlan {
        LauncherFramePlan::new(DirtyRectList::new(), None, None, None, None)
    }

    fn cached_pixels() -> Vec<Rgb565Pixel> {
        vec![Rgb565Pixel(0); WIDTH * HEIGHT]
    }

    fn present(
        presenter: &mut FpgaVblankLatchHiddenPresenter<FakeBuffers>,
        hardware: &mut FakeHardware,
        display: &mut LauncherDisplaySession,
    ) -> Result<FpgaVblankLatchHiddenPresentStats, LatchFailure> {
        let pixels = cached_pixels();
        presenter.present_cached_full_frame(
            CachedFrameView::new(&pixels, WIDTH, HEIGHT),
            frame_plan(),
            hardware,
            display,
            |_, _| Ok(()),
        )
    }

    #[test]
    fn successful_present_orders_copy_overlay_post_and_status_reads() {
        let events = EventLog::default();
        let mut presenter = presenter_with_events(events.clone());
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(BASE1, 0x0001)),
            ],
            events: Some(events.clone()),
            ..FakeHardware::default()
        };
        let mut display = display_session();
        let pixels = cached_pixels();

        presenter
            .present_cached_full_frame(
                CachedFrameView::new(&pixels, WIDTH, HEIGHT),
                frame_plan(),
                &mut hardware,
                &mut display,
                |_, _| {
                    events.borrow_mut().push(TestEvent::Overlay);
                    Ok(())
                },
            )
            .unwrap();

        assert_eq!(
            *events.borrow(),
            [
                TestEvent::ReadStatus,
                TestEvent::Copy,
                TestEvent::Overlay,
                TestEvent::Publish,
                TestEvent::ReadStatus,
                TestEvent::Post,
                TestEvent::ReadStatus,
            ]
        );
    }

    #[test]
    fn external_hidden_grant_posts_while_mappings_are_owned_by_the_worker() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE2, 0x0001)),
            ],
            ..FakeHardware::default()
        };
        let mut display = display_session();
        let worker_buffers = presenter.buffers.take().expect("loan mappings to worker");

        let grant = presenter
            .try_issue_hidden_slot_render_grant(&mut hardware, &mut display)
            .unwrap()
            .expect("inactive slot grant");
        assert_eq!(grant.slot_index, 2);
        assert!(
            presenter
                .try_issue_hidden_slot_render_grant(&mut hardware, &mut display)
                .unwrap()
                .is_none()
        );

        let stats = presenter
            .present_completed_hidden_frame(
                CompletedHiddenFrame { grant },
                &mut hardware,
                &mut display,
            )
            .unwrap();
        assert_eq!(stats.copy_path, LatchCopyPath::ExternalDirect);
        assert_eq!(stats.copied_bytes, 0);
        assert_eq!(hardware.post_bases, vec![BASE2]);
        assert!(
            presenter
                .present_completed_hidden_frame(
                    CompletedHiddenFrame { grant },
                    &mut hardware,
                    &mut display,
                )
                .is_err()
        );
        presenter.buffers = Some(worker_buffers);
    }

    #[test]
    fn startup_intro_grant_uses_native_slot_geometry_for_transformed_composition() {
        let mut presenter = presenter();
        presenter.render_height = HEIGHT * 2;
        let mut hardware = FakeHardware {
            statuses: vec![Ok(status(BASE1, 0x0001))],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        assert!(
            presenter
                .try_issue_hidden_slot_render_grant(&mut hardware, &mut display)
                .unwrap()
                .is_none()
        );
        let grant = presenter
            .try_issue_startup_intro_hidden_slot_render_grant(&mut hardware, &mut display)
            .unwrap()
            .expect("startup intro native slot grant");

        assert_eq!((grant.width, grant.height), (WIDTH, HEIGHT));
        assert_eq!(grant.stride_pixels, WIDTH);
    }

    #[test]
    fn external_hidden_grant_rejects_pending_hardware() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(status(BASE1, 0x0001 | 0x0004))],
            ..FakeHardware::default()
        };
        let mut display = display_session();
        assert!(
            presenter
                .try_issue_hidden_slot_render_grant(&mut hardware, &mut display)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn external_hidden_completion_rejects_a_slot_that_became_active_or_pending() {
        for status_before_post in [status(BASE2, 0x0001), status(BASE1, 0x0001 | 0x0004)] {
            let mut presenter = presenter();
            let mut hardware = FakeHardware {
                statuses: vec![Ok(status(BASE1, 0x0001)), Ok(status_before_post)],
                ..FakeHardware::default()
            };
            let mut display = display_session();
            let grant = presenter
                .try_issue_hidden_slot_render_grant(&mut hardware, &mut display)
                .unwrap()
                .expect("inactive slot grant");

            let failure = presenter
                .present_completed_hidden_frame(
                    CompletedHiddenFrame { grant },
                    &mut hardware,
                    &mut display,
                )
                .unwrap_err();

            assert_eq!(failure.reason_code(), "no-writable-hidden-buffer");
            assert!(hardware.post_bases.is_empty());
        }
    }

    #[test]
    fn external_hidden_invalidation_rejects_stale_completion_tokens() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(status(BASE1, 0x0001))],
            ..FakeHardware::default()
        };
        let mut display = display_session();
        let grant = presenter
            .try_issue_hidden_slot_render_grant(&mut hardware, &mut display)
            .unwrap()
            .expect("inactive slot grant");

        presenter.invalidate_external_mode();
        let failure = presenter
            .present_completed_hidden_frame(
                CompletedHiddenFrame { grant },
                &mut hardware,
                &mut display,
            )
            .unwrap_err();

        assert_eq!(failure.reason_code(), "posted-sequence-unverified");
        assert!(hardware.post_bases.is_empty());
    }

    #[test]
    fn external_hidden_completion_becomes_the_committed_capture_view() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE2, 0x0001)),
            ],
            ..FakeHardware::default()
        };
        let mut display = display_session();
        let grant = presenter
            .try_issue_hidden_slot_render_grant(&mut hardware, &mut display)
            .unwrap()
            .expect("inactive slot grant");
        let buffers = presenter.buffers.as_mut().expect("presenter owns buffers");
        buffers.buffer_mut(grant.slot_index).pixels[0] = Rgb565Pixel(0x5aa5);
        FakeBuffers::publish_writes(buffers.buffer_mut(grant.slot_index));

        let stats = presenter
            .present_completed_hidden_frame(
                CompletedHiddenFrame { grant },
                &mut hardware,
                &mut display,
            )
            .unwrap();

        assert_eq!(
            presenter.committed_frame_view(stats.buffer_index).pixels[0],
            Rgb565Pixel(0x5aa5)
        );
        assert_eq!(stats.copy_path, LatchCopyPath::ExternalDirect);
        assert_eq!(stats.copied_bytes, 0);
    }

    #[test]
    fn committed_frame_view_contains_final_overlay_pixels() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(BASE1, 0x0001)),
            ],
            ..FakeHardware::default()
        };
        let mut display = display_session();
        let pixels = cached_pixels();

        let stats = presenter
            .present_cached_full_frame(
                CachedFrameView::new(&pixels, WIDTH, HEIGHT),
                frame_plan(),
                &mut hardware,
                &mut display,
                |hidden, _| {
                    hidden.pixels[0] = Rgb565Pixel(0x5aa5);
                    Ok(())
                },
            )
            .expect("successful latch present");

        let committed = presenter.committed_frame_view(stats.buffer_index);
        assert_eq!(committed.pixels[0], Rgb565Pixel(0x5aa5));
        assert_eq!(committed.width, WIDTH);
        assert_eq!(committed.height, HEIGHT);
        assert_eq!(committed.stride_pixels, WIDTH);
    }

    #[test]
    fn cold_front_buffer_uses_first_hidden_slot_across_locked_status_check() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(BASE1, 0x0001)),
            ],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let stats = present(&mut presenter, &mut hardware, &mut display).unwrap();

        assert_eq!(stats.buffer_index, 1);
        assert_eq!(hardware.post_bases, [BASE1]);
        assert_eq!(hardware.read_count, 3);
    }

    #[test]
    fn hidden_active_slot_is_not_selected_for_next_post() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE2, 0x0001)),
            ],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let stats = present(&mut presenter, &mut hardware, &mut display).unwrap();

        assert_eq!(stats.buffer_index, 2);
        assert_eq!(hardware.post_bases, [BASE2]);
        assert_eq!(hardware.read_count, 3);
    }

    #[test]
    fn front_buffer_after_hidden_verification_invalidates_slots_without_rearming_main_route() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE2, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE2, 0x0001)),
            ],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        present(&mut presenter, &mut hardware, &mut display).unwrap();
        present(&mut presenter, &mut hardware, &mut display).unwrap();
        let recovered = present(&mut presenter, &mut hardware, &mut display).unwrap();
        let recovered_other_slot = present(&mut presenter, &mut hardware, &mut display).unwrap();

        assert!(recovered.full_copy);
        assert_eq!(recovered.invalid_bytes, WIDTH * HEIGHT * 2);
        assert!(recovered_other_slot.full_copy);
        assert_eq!(recovered_other_slot.invalid_bytes, WIDTH * HEIGHT * 2);
        assert_eq!(hardware.read_count, 12);
    }

    #[test]
    fn pending_status_blocks_hidden_writes_before_post() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: (0..128)
                .map(|_| Ok(status(BASE1, 0x0001 | 0x0004)))
                .collect(),
            ..FakeHardware::default()
        };
        let mut display = display_session();

        assert!(present(&mut presenter, &mut hardware, &mut display).is_err());
        assert!(hardware.post_bases.is_empty());
        assert!(hardware.read_count > 1);
    }

    #[test]
    fn transient_pending_status_settles_before_hidden_write() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(BASE1, 0x0001 | 0x0004)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE2, 0x0001)),
            ],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let stats = present(&mut presenter, &mut hardware, &mut display)
            .expect("pending latch should settle before allocation");

        assert_eq!(stats.buffer_index, 2);
        assert_eq!(hardware.post_bases, [BASE2]);
        assert_eq!(hardware.read_count, 4);
    }

    #[test]
    fn transient_post_visibility_gap_is_retried_before_failure() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
            ],
            posted_sequence_visibility_delay_reads: 2,
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let stats = present(&mut presenter, &mut hardware, &mut display)
            .expect("a transient status gap must not reject a successful post");

        assert_eq!(stats.buffer_index, 1);
        assert_eq!(hardware.post_bases, [BASE1]);
        assert_eq!(hardware.read_count, 3);
    }

    #[test]
    fn post_observation_retains_every_wire_sample() {
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001 | 0x0004)),
            ],
            status_diagnostics: vec![
                wire_diagnostics(1),
                wire_diagnostics(2),
                wire_diagnostics(3),
            ],
            posted_sequence_visibility_delay_reads: usize::MAX,
            ..FakeHardware::default()
        };
        hardware.statuses[2].as_mut().unwrap().pending_sequence = 77;

        let mut budget = LogicalStatusReadBudget::new();
        let (sample, _, reads, wire_attempts) = read_post_status(77, &mut budget, |budget| {
            budget.consume().unwrap();
            read_status_sample(&mut hardware, None)
                .map_err(|error| latch_status_read_failure(LatchFailureStage::FpgaStatus, error))
        })
        .unwrap();

        assert_eq!(reads, 3);
        assert_eq!(wire_attempts, 3);
        assert_eq!(sample.diagnostics.attempt_count, 3);
        assert_eq!(sample.diagnostics.attempts[0].command, 1);
        assert_eq!(sample.diagnostics.attempts[1].command, 2);
        assert_eq!(sample.diagnostics.attempts[2].command, 3);
        assert_eq!(sample.diagnostics.decision, LatchWireDecision::Corroborated);
    }

    #[test]
    fn suspicious_post_observation_shares_one_three_read_budget() {
        let mut invalid = status(BASE1, 0x0001);
        invalid.active_width = 960;
        invalid.active_height = 0;
        invalid.active_stride = 0;
        let valid_stale = status(BASE1, 0x0001);
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(invalid), Ok(valid_stale), Ok(valid_stale)],
            status_diagnostics: vec![
                wire_diagnostics(1),
                wire_diagnostics(2),
                wire_diagnostics(3),
            ],
            ..FakeHardware::default()
        };
        presenter.verify_capabilities(&mut hardware).unwrap();
        let mut budget = LogicalStatusReadBudget::new();

        let (sample, _, logical_reads, wire_attempts) =
            read_post_status(77, &mut budget, |budget| {
                presenter.read_geometry_safe_status_with_budget(&mut hardware, budget)
            })
            .unwrap();

        assert_eq!(logical_reads, 3);
        assert_eq!(wire_attempts, 3);
        assert_eq!(hardware.read_count, 3);
        assert!(!posted_sequence_observed(sample.status, 77));
    }

    #[test]
    fn unsupported_status_disables_presenter() {
        let mut unsupported = status(FRONT_BASE, 0x0001);
        unsupported.magic_hi = 0;
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(unsupported)],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        assert!(present(&mut presenter, &mut hardware, &mut display).is_err());
        assert!(
            present(&mut presenter, &mut hardware, &mut display)
                .unwrap_err()
                .to_string()
                .contains("disabled")
        );
        assert_eq!(hardware.read_count, 1);
    }

    #[test]
    fn v5_geometry_fault_requires_two_matching_valid_samples_before_copy_or_post() {
        let mut bad_geometry = status(BASE1, 0x0001);
        bad_geometry.active_width = 960;
        bad_geometry.active_height = 0;
        bad_geometry.active_stride = 0;
        let valid = status(BASE1, 0x0001);
        let events = EventLog::default();
        let mut presenter = presenter_with_events(Rc::clone(&events));
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(bad_geometry),
                Ok(valid),
                Ok(valid),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE2, 0x0001)),
            ],
            events: Some(Rc::clone(&events)),
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let stats = present(&mut presenter, &mut hardware, &mut display).unwrap();

        assert_eq!(stats.buffer_index, 2);
        assert_eq!(hardware.read_count, 5);
        assert_eq!(
            &events.borrow()[..4],
            &[
                TestEvent::ReadStatus,
                TestEvent::ReadStatus,
                TestEvent::ReadStatus,
                TestEvent::Copy,
            ]
        );
        assert_eq!(hardware.post_bases, [BASE2]);
    }

    #[test]
    fn two_identical_invalid_v5_samples_fail_early_without_copy_or_post() {
        let mut bad_geometry = status(BASE1, 0x0001);
        bad_geometry.active_width = 960;
        bad_geometry.active_height = 0;
        bad_geometry.active_stride = 0;
        let events = EventLog::default();
        let mut presenter = presenter_with_events(Rc::clone(&events));
        let mut hardware = FakeHardware {
            statuses: vec![Ok(bad_geometry), Ok(bad_geometry)],
            events: Some(Rc::clone(&events)),
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let error = present(&mut presenter, &mut hardware, &mut display).unwrap_err();
        let diagnostics = error.wire_diagnostics.as_ref().unwrap();

        assert_eq!(hardware.read_count, 2);
        assert_eq!(diagnostics.decision, LatchWireDecision::Rejected);
        assert_eq!(diagnostics.protocol_version, Some(5));
        assert_eq!(
            diagnostics.capability_flags,
            Some(mister_magik_latch_contract::REQUIRED_CAPS)
        );
        assert_eq!(
            events.borrow().as_slice(),
            &[TestEvent::ReadStatus, TestEvent::ReadStatus]
        );
        assert!(hardware.post_bases.is_empty());
        let evidence = mister_magik_fb::latch_readiness::LatchFailureEvidence::from(&error);
        assert_eq!(evidence.schema, "mister-magik-latch-failure-v3");
    }

    #[test]
    fn changing_invalid_v5_samples_then_one_valid_sample_remain_rejected() {
        let mut invalid_a = status(BASE1, 0x0001);
        invalid_a.active_width = 960;
        invalid_a.active_height = 0;
        invalid_a.active_stride = 0;
        let mut invalid_b = invalid_a;
        invalid_b.active_width -= 1;
        let events = EventLog::default();
        let mut presenter = presenter_with_events(Rc::clone(&events));
        let mut hardware = FakeHardware {
            statuses: vec![Ok(invalid_a), Ok(invalid_b), Ok(status(BASE1, 0x0001))],
            events: Some(Rc::clone(&events)),
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let error = present(&mut presenter, &mut hardware, &mut display).unwrap_err();

        assert_eq!(error.reason, LatchFailureReason::ActiveGeometryMismatch);
        assert_eq!(hardware.read_count, 3);
        assert_eq!(
            events.borrow().as_slice(),
            &[
                TestEvent::ReadStatus,
                TestEvent::ReadStatus,
                TestEvent::ReadStatus,
            ]
        );
        assert!(hardware.post_bases.is_empty());
    }

    #[test]
    fn pending_to_active_transition_is_not_mistaken_for_two_matching_valid_samples() {
        let mut invalid = status(BASE1, 0x0001);
        invalid.active_width = 960;
        invalid.active_height = 0;
        invalid.active_stride = 0;
        let mut pending = status(BASE1, 0x0001 | 0x0002 | 0x0004);
        pending.active_sequence = 11;
        pending.pending_sequence = 12;
        let mut active = status(BASE2, 0x0001);
        active.active_sequence = 12;
        active.pending_sequence = 0;
        let events = EventLog::default();
        let mut presenter = presenter_with_events(Rc::clone(&events));
        let mut hardware = FakeHardware {
            statuses: vec![Ok(invalid), Ok(pending), Ok(active)],
            events: Some(Rc::clone(&events)),
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let error = present(&mut presenter, &mut hardware, &mut display).unwrap_err();

        assert_eq!(error.reason, LatchFailureReason::ActiveGeometryMismatch);
        assert_eq!(hardware.read_count, 3);
        assert_eq!(
            events.borrow().as_slice(),
            &[
                TestEvent::ReadStatus,
                TestEvent::ReadStatus,
                TestEvent::ReadStatus,
            ]
        );
        assert!(hardware.post_bases.is_empty());
    }

    #[test]
    fn direct_grant_uses_the_same_v5_status_corroboration_policy() {
        let mut invalid = status(BASE1, 0x0001);
        invalid.active_width = 960;
        invalid.active_height = 0;
        invalid.active_stride = 0;
        let valid = status(BASE1, 0x0001);
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(invalid), Ok(valid), Ok(valid)],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let grant = presenter
            .try_issue_hidden_slot_render_grant(&mut hardware, &mut display)
            .unwrap()
            .expect("corroborated direct grant");

        assert_eq!(grant.slot_index, 2);
        assert_eq!(hardware.read_count, 3);
    }

    #[test]
    fn failed_post_keeps_slot_retryable_and_skips_second_status_read() {
        let before = status(FRONT_BASE, 0x0001);
        let after = status(BASE1, 0x0001);
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(before), Ok(before), Ok(before), Ok(before), Ok(after)],
            posts: vec![Err(io::Error::other("post failed"))],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        assert!(present(&mut presenter, &mut hardware, &mut display).is_err());
        assert_eq!(hardware.read_count, 2);
        let stats = present(&mut presenter, &mut hardware, &mut display).unwrap();

        assert_eq!(stats.buffer_index, 1);
        assert_eq!(hardware.post_bases, [BASE1, BASE1]);
        assert_eq!(hardware.read_count, 5);
    }

    #[test]
    fn completion_wait_accepts_immediate_and_wrapped_sequences() {
        for sequence in [17, u16::MAX] {
            let mut settled = status(BASE1, 0x0001);
            settled.active_sequence = sequence;
            settled.pending_sequence = 0;
            let completion =
                wait_for_latch_completion_with(|| Ok(settled), sequence, Duration::ZERO, || {})
                    .unwrap();
            assert_eq!(completion.status.active_sequence, sequence);
            assert_eq!(completion.poll_count, 1);
        }
    }

    #[test]
    fn completion_wait_handles_early_vsync_pending_then_active() {
        let mut pending = status(BASE1, 0x0001 | 0x0004);
        pending.active_sequence = 41;
        pending.pending_sequence = 42;
        let mut settled = status(BASE2, 0x0001);
        settled.active_sequence = 42;
        settled.pending_sequence = 0;
        let mut statuses = vec![pending, settled].into_iter();

        let completion = wait_for_latch_completion_with(
            || Ok(statuses.next().unwrap()),
            42,
            Duration::from_millis(1),
            || {},
        )
        .unwrap();

        assert_eq!(completion.status.active_sequence, 42);
        assert_eq!(completion.poll_count, 2);
    }

    #[test]
    fn completion_wait_tolerates_transient_cleared_pending_before_active_advances() {
        let mut pending = status(BASE1, 0x0001 | 0x0004);
        pending.active_sequence = 218;
        pending.pending_sequence = 219;
        let mut transient = status(BASE1, 0x0001);
        transient.active_sequence = 218;
        transient.pending_sequence = 0;
        let mut settled = status(BASE2, 0x0001);
        settled.active_sequence = 219;
        settled.pending_sequence = 0;
        let mut statuses = vec![pending, transient, settled].into_iter();

        let completion = wait_for_latch_completion_with(
            || Ok(statuses.next().unwrap()),
            219,
            Duration::from_millis(1),
            || {},
        )
        .unwrap();

        assert_eq!(completion.status.active_sequence, 219);
        assert_eq!(completion.poll_count, 3);
    }

    #[test]
    fn completion_wait_rejects_timeout_unsupported_and_transport_failure() {
        let stale = status(BASE1, 0x0001);
        let timeout =
            wait_for_latch_completion_with(|| Ok(stale), 99, Duration::ZERO, || {}).unwrap_err();
        assert_eq!(timeout.reason, LatchFailureReason::PostedSequenceUnverified);

        let mut unsupported = stale;
        unsupported.magic_hi = 0;
        let unsupported =
            wait_for_latch_completion_with(|| Ok(unsupported), 99, Duration::ZERO, || {})
                .unwrap_err();
        assert_eq!(
            unsupported.reason,
            LatchFailureReason::FpgaStatusUnsupported
        );

        let transport = wait_for_latch_completion_with(
            || Err(io::Error::other("read failed")),
            99,
            Duration::ZERO,
            || {},
        )
        .unwrap_err();
        assert_eq!(transport.reason, LatchFailureReason::FpgaTransportFailed);
    }

    #[test]
    fn production_completion_wait_retains_wire_samples_on_failure() {
        let mut hardware = FakeHardware {
            statuses: vec![Ok(status(BASE1, 0x0001))],
            status_diagnostics: vec![wire_diagnostics(0x58)],
            ..FakeHardware::default()
        };

        let error = wait_for_latch_completion(&mut hardware, 99, Duration::ZERO).unwrap_err();
        let diagnostics = error.wire_diagnostics.expect("wire diagnostics");

        assert_eq!(diagnostics.attempt_count, 1);
        assert_eq!(diagnostics.attempts[0].command, 0x58);
    }

    #[test]
    fn production_completion_wait_preserves_terminal_transport_decision_and_profile() {
        let mut hardware = FakeHardware {
            statuses: vec![Ok(status(BASE1, 0x0001)), Err(io::Error::other("failed"))],
            status_diagnostics: vec![wire_diagnostics(1), wire_transport_failure(&[2, 3])],
            ..FakeHardware::default()
        };

        let error =
            wait_for_latch_completion(&mut hardware, 99, Duration::from_millis(1)).unwrap_err();
        let diagnostics = error.wire_diagnostics.expect("wire diagnostics");

        assert_eq!(
            diagnostics.decision,
            LatchWireDecision::TransportRetryFailed
        );
        assert_eq!(diagnostics.protocol_version, Some(5));
        assert_eq!(
            diagnostics.capability_flags,
            Some(mister_magik_latch_contract::REQUIRED_CAPS)
        );
        assert_eq!(diagnostics.attempt_count, 3);
        assert_eq!(diagnostics.attempts[0].command, 1);
        assert_eq!(diagnostics.attempts[1].command, 2);
        assert_eq!(diagnostics.attempts[2].command, 3);
    }

    #[test]
    fn production_completion_wait_preserves_immediate_transport_failure() {
        let mut hardware = FakeHardware {
            statuses: vec![Err(io::Error::other("failed"))],
            status_diagnostics: vec![wire_transport_failure(&[2, 3])],
            ..FakeHardware::default()
        };

        let error = wait_for_latch_completion(&mut hardware, 99, Duration::ZERO).unwrap_err();
        let diagnostics = error.wire_diagnostics.expect("wire diagnostics");

        assert_eq!(
            diagnostics.decision,
            LatchWireDecision::TransportRetryFailed
        );
        assert_eq!(diagnostics.protocol_version, Some(5));
        assert_eq!(diagnostics.attempt_count, 2);
    }
}
