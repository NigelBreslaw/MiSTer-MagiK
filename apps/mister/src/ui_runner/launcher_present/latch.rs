// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::super::*;
use mister_magik_fb::framebuffer::downsample::Rgb565FrameView;
use mister_magik_fb::latch_readiness::{LatchFailure, LatchFailureReason, LatchFailureStage};
use std::io;

const TRANSIENT_PENDING_SETTLE_TIMEOUT: Duration = Duration::from_millis(100);

pub(in crate::ui_runner) trait LatchHardware: LauncherDisplayHardware {
    fn read_latch_capabilities(
        &mut self,
    ) -> io::Result<(u16, u16, mister_magik_latch_contract::LatchCapabilities)>;

    fn read_latched_status(&mut self) -> io::Result<crate::fpga::LatchedFbufStatus>;

    fn post_latched_rgb565(
        &mut self,
        sequence: u16,
        base_addr: u32,
        fb_width: u16,
        fb_height: u16,
        geometry: crate::fpga::LatchedFbufGeometry,
    ) -> io::Result<(u16, u16)>;
}

impl LatchHardware for Fpga {
    fn read_latch_capabilities(
        &mut self,
    ) -> io::Result<(u16, u16, mister_magik_latch_contract::LatchCapabilities)> {
        self.read_magik_latched_fbuf_capabilities()
    }

    fn read_latched_status(&mut self) -> io::Result<crate::fpga::LatchedFbufStatus> {
        self.read_magik_latched_fbuf_status()
    }

    fn post_latched_rgb565(
        &mut self,
        sequence: u16,
        base_addr: u32,
        fb_width: u16,
        fb_height: u16,
        geometry: crate::fpga::LatchedFbufGeometry,
    ) -> io::Result<(u16, u16)> {
        self.post_magik_latched_fbuf_rgb565(sequence, base_addr, fb_width, fb_height, geometry)
    }
}

pub(in crate::ui_runner) trait LatchFrameBuffers {
    type Buffer;

    fn base_addr(&self, slot_index: u8) -> u32;
    fn buffer_mut(&mut self, slot_index: u8) -> &mut Self::Buffer;
    fn frame_view(&self, slot_index: u8, width: usize, height: usize) -> Rgb565FrameView<'_>;
    fn copy_rect(
        buffer: &mut Self::Buffer,
        cached: CachedFrameView<'_>,
        rect: DirtyRect,
    ) -> Result<usize, String>;
}

pub(in crate::ui_runner) struct PluginLatchFrameBuffers {
    buffer1: ScanoutSlotsRgb565Framebuffer,
    buffer2: ScanoutSlotsRgb565Framebuffer,
    base1: u32,
    base2: u32,
}

impl PluginLatchFrameBuffers {
    fn open(width: usize, height: usize) -> Result<Self, LatchFailure> {
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
    ) -> Result<usize, String> {
        buffer
            .copy_rect(cached.pixels(), cached.stride(), rect)
            .map_err(|e| e.to_string())
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
    buffers: B,
    disabled: bool,
    sequence: u16,
    width: usize,
    height: usize,
    latch_geometry: crate::fpga::LatchedFbufGeometry,
    hidden_active_verified: bool,
    capabilities_verified: bool,
    last_committed_buffer: Option<u8>,
    latch_state: TwoBufferLatchState,
}

#[derive(Debug)]
pub(in crate::ui_runner) struct FpgaVblankLatchHiddenPresentStats {
    pub(in crate::ui_runner) copied_bytes: usize,
    pub(in crate::ui_runner) invalid_bytes: usize,
    pub(in crate::ui_runner) rect_count: u32,
    pub(in crate::ui_runner) catchup_bytes: usize,
    pub(in crate::ui_runner) full_copy: bool,
    pub(in crate::ui_runner) buffer_index: u8,
    pub(in crate::ui_runner) copied_rows: u32,
    pub(in crate::ui_runner) copy_us: u128,
    pub(in crate::ui_runner) post_us: u128,
    pub(in crate::ui_runner) set_vga_fb_us: u128,
    pub(in crate::ui_runner) status_us: u64,
    pub(in crate::ui_runner) set_supported: bool,
    pub(in crate::ui_runner) status_supported: bool,
    pub(in crate::ui_runner) flip_count: u16,
}

impl FpgaVblankLatchHiddenPresenter<PluginLatchFrameBuffers> {
    pub(in crate::ui_runner) fn open(ui: &UiDisplay) -> Result<Self, LatchFailure> {
        let width = ui.render_w();
        let height = ui.render_h();
        let buffers = PluginLatchFrameBuffers::open(width, height)?;
        let route = LauncherFramebufferRoute::for_scan(ui.scan_w(), ui.scan_h(), ui.direct_video());
        Ok(Self::new(
            buffers,
            width,
            height,
            crate::fpga::LatchedFbufGeometry::new(
                width as u16,
                route.mode(),
                configured_fpga_latch_right_guard_cols(),
            ),
        ))
    }
}

impl<B: LatchFrameBuffers> FpgaVblankLatchHiddenPresenter<B> {
    fn new(
        buffers: B,
        width: usize,
        height: usize,
        latch_geometry: crate::fpga::LatchedFbufGeometry,
    ) -> Self {
        Self {
            buffers,
            disabled: false,
            sequence: 1,
            width,
            height,
            latch_geometry,
            hidden_active_verified: false,
            capabilities_verified: false,
            last_committed_buffer: None,
            latch_state: TwoBufferLatchState::new(width, height),
        }
    }

    pub(in crate::ui_runner) fn present_cached_full_frame<H, F>(
        &mut self,
        cached: CachedFrameView<'_>,
        input: LauncherFramePlan,
        hardware: &mut H,
        display_session: &mut LauncherDisplaySession,
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

        if !self.capabilities_verified {
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
            self.capabilities_verified = true;
        }

        let status_start = Instant::now();
        let mut before_status = hardware.read_latched_status().map_err(|e| {
            LatchFailure::runtime(
                LatchFailureStage::FpgaStatus,
                LatchFailureReason::FpgaTransportFailed,
                e.to_string(),
            )
        })?;
        let mut status_us = status_start.elapsed().as_micros() as u64;
        self.sync_latch_state_from_status(before_status, display_session)?;

        let mut plan = self.latch_state.plan_next(input);
        if plan.is_none() && before_status.pending() {
            let settle_started = Instant::now();
            while settle_started.elapsed() < TRANSIENT_PENDING_SETTLE_TIMEOUT {
                std::thread::sleep(Duration::from_millis(1));
                let retry_started = Instant::now();
                before_status = hardware.read_latched_status().map_err(|e| {
                    LatchFailure::runtime(
                        LatchFailureStage::FpgaStatus,
                        LatchFailureReason::FpgaTransportFailed,
                        e.to_string(),
                    )
                })?;
                status_us = status_us.saturating_add(retry_started.elapsed().as_micros() as u64);
                self.sync_latch_state_from_status(before_status, display_session)?;
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
        })?;
        let buffer_index = plan.slot_index;
        let invalid_bytes = self.latch_state.restore_bytes_for_slot(buffer_index);
        let rect_count = plan.restore_rects.len() as u32;
        let copied_rows = plan.restore_rects.iter().map(DirtyRect::rows).sum::<u32>();
        let full_copy = rect_list_contains(plan.restore_rects, self.full_rect());
        let catchup_bytes = plan.restore_rects.total_rgb565_bytes();
        let base_addr = self.buffers.base_addr(buffer_index);
        let buffer = self.buffers.buffer_mut(buffer_index);
        let copy_start = Instant::now();
        let mut copied_bytes = 0usize;
        for rect in plan.restore_rects.iter() {
            match B::copy_rect(buffer, cached, rect) {
                Ok(bytes) => copied_bytes = copied_bytes.saturating_add(bytes),
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

        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let post_start = Instant::now();
        let ack = match hardware.post_latched_rgb565(
            sequence,
            base_addr,
            self.width as u16,
            self.height as u16,
            self.latch_geometry,
        ) {
            Ok(ack) => ack,
            Err(e) => {
                self.latch_state.mark_attempt_failed(buffer_index);
                return Err(LatchFailure::runtime(
                    LatchFailureStage::LatchPost,
                    LatchFailureReason::LatchPostFailed,
                    e.to_string(),
                ));
            }
        };
        let post_us = post_start.elapsed().as_micros();
        let set_vga_fb_us = match display_session.arm_latch_route_with_hardware(hardware) {
            Ok(elapsed_us) => elapsed_us,
            Err(e) => {
                self.latch_state.mark_attempt_failed(buffer_index);
                return Err(LatchFailure::runtime(
                    LatchFailureStage::RouteArm,
                    LatchFailureReason::RouteArmFailed,
                    e.to_string(),
                ));
            }
        };
        let set_supported = ack.0 == crate::fpga::MAGIK_FBUF_LATCH_MAGIC
            || ack.1 == crate::fpga::MAGIK_FBUF_LATCH_MAGIC;

        let status_start = Instant::now();
        let after_status = match hardware.read_latched_status() {
            Ok(status) => status,
            Err(e) => {
                self.latch_state.mark_attempt_failed(buffer_index);
                return Err(LatchFailure::runtime(
                    LatchFailureStage::FpgaStatus,
                    LatchFailureReason::FpgaTransportFailed,
                    e.to_string(),
                ));
            }
        };
        status_us = status_us.saturating_add(status_start.elapsed().as_micros() as u64);
        let status_supported = after_status.supported();
        let flip_count = after_status.flip_count;

        if !set_supported || !status_supported {
            self.latch_state.mark_attempt_failed(buffer_index);
            self.disabled = true;
            return Err(LatchFailure::incompatible(
                LatchFailureStage::FpgaStatus,
                LatchFailureReason::FpgaStatusUnsupported,
                format!(
                "unsupported latch core set_supported={} status_supported={} ack_high=0x{:04x} ack_low=0x{:04x} status_high=0x{:04x} status_low=0x{:04x}",
                u8::from(set_supported),
                u8::from(status_supported),
                ack.0,
                ack.1,
                after_status.magic_hi,
                after_status.magic_lo
            )));
        }

        let posted_sequence_observed = after_status.active_sequence == sequence
            || (after_status.pending() && after_status.pending_sequence == sequence);
        if !posted_sequence_observed {
            self.latch_state.mark_attempt_failed(buffer_index);
            return Err(LatchFailure::runtime(
                LatchFailureStage::PostVerification,
                LatchFailureReason::PostedSequenceUnverified,
                format!(
                    "posted={} active={} pending={} pending_sequence={}",
                    sequence,
                    after_status.active_sequence,
                    u8::from(after_status.pending()),
                    after_status.pending_sequence
                ),
            ));
        }

        self.latch_state.mark_post_success(plan);
        self.last_committed_buffer = Some(buffer_index);
        Ok(FpgaVblankLatchHiddenPresentStats {
            copied_bytes,
            invalid_bytes,
            rect_count,
            catchup_bytes,
            full_copy,
            buffer_index,
            copied_rows,
            copy_us,
            post_us,
            set_vga_fb_us,
            status_us,
            set_supported,
            status_supported,
            flip_count,
        })
    }

    pub(in crate::ui_runner) fn committed_frame_view(
        &self,
        buffer_index: u8,
    ) -> Rgb565FrameView<'_> {
        self.buffers
            .frame_view(buffer_index, self.width, self.height)
    }

    pub(in crate::ui_runner) fn publish_requested_full_snapshot(&self) -> bool {
        if !mister_magik_fb::framebuffer::stream::adaptive_full_snapshot_due() {
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
            x1: self.width,
            y1: self.height,
        }
    }

    fn sync_latch_state_from_status(
        &mut self,
        status: crate::fpga::LatchedFbufStatus,
        display_session: &mut LauncherDisplaySession,
    ) -> Result<(), LatchFailure> {
        let sync = match classify_latch_status(
            status,
            self.buffers.base_addr(1),
            self.buffers.base_addr(2),
            self.width,
            self.height,
            self.hidden_active_verified,
        ) {
            Ok(sync) => sync,
            Err(LatchStatusSyncError::Unsupported { magic_hi, magic_lo }) => {
                self.disabled = true;
                return Err(LatchFailure::incompatible(
                    LatchFailureStage::FpgaStatus,
                    LatchFailureReason::FpgaStatusUnsupported,
                    format!("ack_high=0x{magic_hi:04x} ack_low=0x{magic_lo:04x}"),
                ));
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
                display_session.note_latch_route_lost();
                self.hidden_active_verified = false;
                return Err(LatchFailure::runtime(
                    LatchFailureStage::PostVerification,
                    LatchFailureReason::ActiveGeometryMismatch,
                    format!(
                    "latched framebuffer geometry mismatch active={active_width}x{active_height} stride={active_stride} expected={expected_width}x{expected_height} stride={expected_stride}"
                )));
            }
        };

        if sync.recovered_non_hidden_active {
            self.latch_state.invalidate_all();
            display_session.note_latch_route_lost();
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
        Ok(())
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

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestEvent {
        ReadStatus,
        Copy,
        Overlay,
        Post,
        ArmRoute,
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
            if slot_index == 1 {
                BASE1
            } else {
                BASE2
            }
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
        ) -> Result<usize, String> {
            buffer.events.borrow_mut().push(TestEvent::Copy);
            buffer.copy_count += 1;
            for y in rect.y0..rect.y1 {
                let start = y * WIDTH + rect.x0;
                let end = y * WIDTH + rect.x1;
                buffer.pixels[start..end].copy_from_slice(&cached.pixels()[start..end]);
            }
            Ok(rect.width() * rect.rows() as usize * 2)
        }
    }

    #[derive(Default)]
    struct FakeHardware {
        statuses: Vec<io::Result<crate::fpga::LatchedFbufStatus>>,
        posts: Vec<io::Result<(u16, u16)>>,
        read_count: usize,
        post_bases: Vec<u32>,
        last_posted_sequence: Option<u16>,
        set_vga_fb_calls: usize,
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

        fn set_vga_fb(&mut self, _enable: bool) -> io::Result<()> {
            self.set_vga_fb_calls += 1;
            if let Some(events) = &self.events {
                events.borrow_mut().push(TestEvent::ArmRoute);
            }
            Ok(())
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
                    2,
                    mister_magik_latch_contract::REQUIRED_CAPS,
                    1280,
                    720,
                    2560,
                ])
                .map_err(io::Error::other)?,
            ))
        }

        fn read_latched_status(&mut self) -> io::Result<crate::fpga::LatchedFbufStatus> {
            if let Some(events) = &self.events {
                events.borrow_mut().push(TestEvent::ReadStatus);
            }
            self.read_count += 1;
            let mut status = self.statuses.remove(0)?;
            if let Some(sequence) = self.last_posted_sequence.take() {
                status.pending_sequence = sequence;
                status.flags |= 0x0004;
            }
            Ok(status)
        }

        fn post_latched_rgb565(
            &mut self,
            sequence: u16,
            base_addr: u32,
            _fb_width: u16,
            _fb_height: u16,
            _geometry: crate::fpga::LatchedFbufGeometry,
        ) -> io::Result<(u16, u16)> {
            if let Some(events) = &self.events {
                events.borrow_mut().push(TestEvent::Post);
            }
            self.post_bases.push(base_addr);
            let result = if self.posts.is_empty() {
                Ok((crate::fpga::MAGIK_FBUF_LATCH_MAGIC, 0))
            } else {
                self.posts.remove(0)
            };
            if result.is_ok() {
                self.last_posted_sequence = Some(sequence);
            }
            result
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
        }
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
    fn successful_present_orders_copy_overlay_post_route_arm_and_status_reads() {
        let events = EventLog::default();
        let mut presenter = presenter_with_events(events.clone());
        let mut hardware = FakeHardware {
            statuses: vec![Ok(status(FRONT_BASE, 0x0001)), Ok(status(BASE1, 0x0001))],
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
                TestEvent::Post,
                TestEvent::ArmRoute,
                TestEvent::ReadStatus,
            ]
        );
    }

    #[test]
    fn committed_frame_view_contains_final_overlay_pixels() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(status(FRONT_BASE, 0x0001)), Ok(status(BASE1, 0x0001))],
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
    fn cold_front_buffer_uses_first_hidden_slot_and_reads_status_twice() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(status(FRONT_BASE, 0x0001)), Ok(status(BASE1, 0x0001))],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let stats = present(&mut presenter, &mut hardware, &mut display).unwrap();

        assert_eq!(stats.buffer_index, 1);
        assert_eq!(hardware.post_bases, [BASE1]);
        assert_eq!(hardware.read_count, 2);
    }

    #[test]
    fn hidden_active_slot_is_not_selected_for_next_post() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(status(BASE1, 0x0001)), Ok(status(BASE2, 0x0001))],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let stats = present(&mut presenter, &mut hardware, &mut display).unwrap();

        assert_eq!(stats.buffer_index, 2);
        assert_eq!(hardware.post_bases, [BASE2]);
        assert_eq!(hardware.read_count, 2);
    }

    #[test]
    fn front_buffer_after_hidden_verification_invalidates_slots_and_rearms_route() {
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![
                Ok(status(FRONT_BASE, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE1, 0x0001)),
                Ok(status(BASE2, 0x0001)),
                Ok(status(FRONT_BASE, 0x0001)),
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
        assert_eq!(hardware.set_vga_fb_calls, 2);
        assert_eq!(hardware.read_count, 8);
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
                Ok(status(BASE2, 0x0001)),
            ],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let stats = present(&mut presenter, &mut hardware, &mut display)
            .expect("pending latch should settle before allocation");

        assert_eq!(stats.buffer_index, 2);
        assert_eq!(hardware.post_bases, [BASE2]);
        assert_eq!(hardware.read_count, 3);
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
        assert!(present(&mut presenter, &mut hardware, &mut display)
            .unwrap_err()
            .to_string()
            .contains("disabled"));
        assert_eq!(hardware.read_count, 1);
    }

    #[test]
    fn hidden_geometry_mismatch_is_rejected_before_copy_or_post() {
        let mut bad_geometry = status(BASE1, 0x0001);
        bad_geometry.active_height -= 1;
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(bad_geometry)],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        let error = present(&mut presenter, &mut hardware, &mut display).unwrap_err();

        assert!(error.to_string().contains("geometry mismatch"));
        assert!(hardware.post_bases.is_empty());
    }

    #[test]
    fn failed_post_keeps_slot_retryable_and_skips_second_status_read() {
        let before = status(FRONT_BASE, 0x0001);
        let after = status(BASE1, 0x0001);
        let mut presenter = presenter();
        let mut hardware = FakeHardware {
            statuses: vec![Ok(before), Ok(before), Ok(after)],
            posts: vec![Err(io::Error::other("post failed"))],
            ..FakeHardware::default()
        };
        let mut display = display_session();

        assert!(present(&mut presenter, &mut hardware, &mut display).is_err());
        assert_eq!(hardware.read_count, 1);
        let stats = present(&mut presenter, &mut hardware, &mut display).unwrap();

        assert_eq!(stats.buffer_index, 1);
        assert_eq!(hardware.post_bases, [BASE1, BASE1]);
        assert_eq!(hardware.read_count, 3);
    }
}
