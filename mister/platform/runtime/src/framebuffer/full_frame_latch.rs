// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared production-grade latch posting and confirmation core.

#[cfg(feature = "ui")]
use super::downsample::Rgb565FrameView;
#[cfg(feature = "ui")]
use super::target::{CachedFrameView, DirtyRect};
use crate::fpga::{
    Fpga, FpgaUioGuard, LatchedFbufGeometry, LatchedFbufPostAttempt, LatchedFbufPostError,
    LatchedFbufStatus, LatchedFbufStatusReadError, LatchedFbufStatusSample, MAGIK_FBUF_LATCH_MAGIC,
};
use crate::latch_readiness::{
    LatchFailure, LatchFailureReason, LatchFailureStage, LatchPostDiagnostics,
    LatchRejectionObservation, LatchWireDecision, LatchWireDiagnostics,
};
use std::io;
use std::time::{Duration, Instant};

#[cfg(feature = "app-runtime")]
const DEV_LATCH_TIMEOUT_ENV: &str = "MISTER_MAGIK_DEV_LATCH_STATUS_TIMEOUT_AT";
#[cfg(feature = "app-runtime")]
const DEV_LATCH_POST_SKIP_ENV: &str = "MISTER_MAGIK_DEV_LATCH_POST_SKIP_WORD_INDEX";

#[cfg(feature = "app-runtime")]
static DEV_LATCH_STATUS_READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "app-runtime")]
static DEV_LATCH_TIMEOUT_AT: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
#[cfg(feature = "app-runtime")]
static DEV_LATCH_POST_SKIP_INDEX: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
#[cfg(feature = "app-runtime")]
static DEV_LATCH_POST_SKIP_USED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub trait LatchHardware {
    fn lock_latch_transaction(&mut self) -> io::Result<Option<FpgaUioGuard>> {
        Ok(None)
    }

    fn read_latch_capabilities(
        &mut self,
    ) -> io::Result<(u16, u16, mister_magik_latch_contract::LatchCapabilities)>;

    fn read_latched_status(
        &mut self,
    ) -> Result<LatchedFbufStatusSample, LatchedFbufStatusReadError>;

    fn negotiated_latch_capabilities(
        &self,
    ) -> Option<mister_magik_latch_contract::LatchCapabilities> {
        None
    }

    fn post_latched_rgb565(
        &mut self,
        sequence: u16,
        base_addr: u32,
        fb_width: u16,
        fb_height: u16,
        geometry: LatchedFbufGeometry,
    ) -> Result<LatchedFbufPostAttempt, LatchedFbufPostError>;

    fn read_latch_rejection_diagnostics(
        &mut self,
    ) -> io::Result<Option<LatchRejectionObservation>> {
        Ok(None)
    }
}

impl LatchHardware for Fpga {
    fn lock_latch_transaction(&mut self) -> io::Result<Option<FpgaUioGuard>> {
        Fpga::lock_latch_transaction(self)
    }

    fn read_latch_capabilities(
        &mut self,
    ) -> io::Result<(u16, u16, mister_magik_latch_contract::LatchCapabilities)> {
        self.read_magik_latched_fbuf_capabilities()
    }

    fn read_latched_status(
        &mut self,
    ) -> Result<LatchedFbufStatusSample, LatchedFbufStatusReadError> {
        #[cfg(feature = "app-runtime")]
        {
            let read_number =
                DEV_LATCH_STATUS_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if dev_latch_timeout_at() == Some(read_number) {
                return Err(LatchedFbufStatusReadError::from_io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "FPGA SPI timeout waiting for ACK high on word 0x0058 (development injection read {read_number})"
                    ),
                )));
            }
        }
        self.read_magik_latched_fbuf_status_sample()
    }

    fn negotiated_latch_capabilities(
        &self,
    ) -> Option<mister_magik_latch_contract::LatchCapabilities> {
        self.negotiated_magik_latch_capabilities()
    }

    fn post_latched_rgb565(
        &mut self,
        sequence: u16,
        base_addr: u32,
        fb_width: u16,
        fb_height: u16,
        geometry: LatchedFbufGeometry,
    ) -> Result<LatchedFbufPostAttempt, LatchedFbufPostError> {
        self.post_magik_latched_fbuf_rgb565_observed(
            sequence,
            base_addr,
            fb_width,
            fb_height,
            geometry,
            configured_dev_latch_post_skip_word_index(),
        )
    }

    fn read_latch_rejection_diagnostics(
        &mut self,
    ) -> io::Result<Option<LatchRejectionObservation>> {
        self.read_magik_latched_fbuf_rejection_diagnostics()
    }
}

#[cfg(feature = "app-runtime")]
fn dev_latch_timeout_at() -> Option<u64> {
    *DEV_LATCH_TIMEOUT_AT.get_or_init(|| {
        let executable = std::env::current_exe().ok()?;
        dev_latch_timeout_for(
            &executable,
            std::env::var(DEV_LATCH_TIMEOUT_ENV).ok().as_deref(),
        )
    })
}

#[cfg(feature = "app-runtime")]
fn configured_dev_latch_post_skip_word_index() -> Option<usize> {
    let configured = *DEV_LATCH_POST_SKIP_INDEX.get_or_init(|| {
        let executable = std::env::current_exe().ok()?;
        dev_latch_post_skip_for(
            &executable,
            std::env::var(DEV_LATCH_POST_SKIP_ENV).ok().as_deref(),
        )
    });
    configured
        .filter(|_| !DEV_LATCH_POST_SKIP_USED.swap(true, std::sync::atomic::Ordering::Relaxed))
}

#[cfg(not(feature = "app-runtime"))]
const fn configured_dev_latch_post_skip_word_index() -> Option<usize> {
    None
}

#[cfg(feature = "app-runtime")]
pub fn dev_latch_timeout_for(
    executable: &std::path::Path,
    configured: Option<&str>,
) -> Option<u64> {
    (mister_magik_catalog::device_layout::DeviceLayout::for_executable(executable)
        == mister_magik_catalog::device_layout::DeviceLayout::Dev)
        .then_some(())?;
    configured?.parse::<u64>().ok().filter(|read| *read > 0)
}

#[cfg(feature = "app-runtime")]
pub fn dev_latch_post_skip_for(
    executable: &std::path::Path,
    configured: Option<&str>,
) -> Option<usize> {
    (mister_magik_catalog::device_layout::DeviceLayout::for_executable(executable)
        == mister_magik_catalog::device_layout::DeviceLayout::Dev)
        .then_some(())?;
    configured?
        .parse::<usize>()
        .ok()
        .filter(|index| *index < mister_magik_latch_contract::V5_SET_WORDS)
}

#[cfg(feature = "ui")]
pub trait LatchFrameBuffers {
    type Buffer;

    fn base_addr(&self, slot_index: u8) -> u32;
    fn buffer_mut(&mut self, slot_index: u8) -> &mut Self::Buffer;
    fn frame_view(&self, slot_index: u8, width: usize, height: usize) -> Rgb565FrameView<'_>;
    fn copy_rect(
        buffer: &mut Self::Buffer,
        cached: CachedFrameView<'_>,
        rect: DirtyRect,
    ) -> Result<LatchCopyResult, String>;
    fn publish_writes(buffer: &mut Self::Buffer);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LatchCopyPath {
    IdentityFull,
    VerticalFull,
    VerticalPartial,
    ExternalDirect,
}

impl LatchCopyPath {
    pub const fn label(self) -> &'static str {
        match self {
            Self::IdentityFull => "identity-full",
            Self::VerticalFull => "vertical-full",
            Self::VerticalPartial => "vertical-partial",
            Self::ExternalDirect => "external-direct",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LatchCopyResult {
    pub bytes: usize,
    pub path: LatchCopyPath,
}

#[derive(Clone, Copy, Debug)]
pub struct LatchPostRequest {
    pub sequence: u16,
    pub slot_index: u8,
    pub base_addr: u32,
    pub width: u16,
    pub height: u16,
    pub geometry: LatchedFbufGeometry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LatchPostReceipt {
    pub status: LatchedFbufStatus,
    pub post_us: u128,
    pub status_us: u64,
    pub status_reads: u8,
    pub status_wire_attempts: u8,
    pub set_supported: bool,
    pub status_supported: bool,
    pub receipt_crc: u16,
}

pub struct LogicalStatusReadBudget {
    reads: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalStatusReadBudgetExhausted;

impl LogicalStatusReadBudget {
    const MAX_READS: u8 = 3;

    pub const fn new() -> Self {
        Self { reads: 0 }
    }

    pub fn consume(&mut self) -> Result<(), LogicalStatusReadBudgetExhausted> {
        if self.reads >= Self::MAX_READS {
            return Err(LogicalStatusReadBudgetExhausted);
        }
        self.reads += 1;
        Ok(())
    }

    pub const fn exhausted(&self) -> bool {
        self.reads >= Self::MAX_READS
    }
}

impl Default for LogicalStatusReadBudget {
    fn default() -> Self {
        Self::new()
    }
}

pub fn post_confirm_prepared_frame<H, R>(
    hardware: &mut H,
    request: LatchPostRequest,
    mut read_status: R,
) -> Result<LatchPostReceipt, LatchFailure>
where
    H: LatchHardware,
    R: FnMut(&mut H, &mut LogicalStatusReadBudget) -> Result<LatchedFbufStatusSample, LatchFailure>,
{
    let _transaction_guard = hardware.lock_latch_transaction().map_err(|error| {
        LatchFailure::runtime(
            LatchFailureStage::LatchPost,
            LatchFailureReason::FpgaTransportFailed,
            format!("failed to lock complete latch transaction: {error}"),
        )
    })?;
    let mut locked_read_budget = LogicalStatusReadBudget::new();
    let locked_status_started = Instant::now();
    let locked_before = read_status(hardware, &mut locked_read_budget)?;
    let locked_status_us = locked_status_started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);
    if locked_before.status.pending()
        || (locked_before.status.active_enabled()
            && locked_before.status.active_base == request.base_addr)
    {
        return Err(LatchFailure::runtime(
            LatchFailureStage::PostVerification,
            LatchFailureReason::NoWritableHiddenBuffer,
            format!(
                "slot {} became active or pending inside locked transaction",
                request.slot_index
            ),
        )
        .with_wire_diagnostics(rejected_wire_diagnostics(locked_before.diagnostics)));
    }

    let post_start = Instant::now();
    let post = hardware
        .post_latched_rgb565(
            request.sequence,
            request.base_addr,
            request.width,
            request.height,
            request.geometry,
        )
        .map_err(|error| {
            LatchFailure::runtime(
                LatchFailureStage::LatchPost,
                LatchFailureReason::LatchPostFailed,
                error.to_string(),
            )
            .with_post_diagnostics(*error.diagnostics)
        })?;
    let post_us = post_start.elapsed().as_micros();
    let mut post_read_budget = LogicalStatusReadBudget::new();
    let (after_sample, post_status_us, status_reads, status_wire_attempts) =
        read_post_status(request.sequence, &mut post_read_budget, |budget| {
            read_status(hardware, budget)
        })
        .map_err(|failure| with_post_failure_evidence(hardware, failure, post.diagnostics, None))?;
    let status = after_sample.status;
    let set_supported =
        post.ack_high == MAGIK_FBUF_LATCH_MAGIC || post.ack_low == MAGIK_FBUF_LATCH_MAGIC;
    let status_supported = status.supported();
    if !set_supported || !status_supported {
        let failure = LatchFailure::incompatible(
            LatchFailureStage::FpgaStatus,
            LatchFailureReason::FpgaStatusUnsupported,
            format!(
                "unsupported latch core set_supported={} status_supported={} ack_high=0x{:04x} ack_low=0x{:04x} status_high=0x{:04x} status_low=0x{:04x}",
                u8::from(set_supported),
                u8::from(status_supported),
                post.ack_high,
                post.ack_low,
                status.magic_hi,
                status.magic_lo
            ),
        )
        .with_wire_diagnostics(rejected_wire_diagnostics(after_sample.diagnostics));
        return Err(with_post_failure_evidence(
            hardware,
            failure,
            post.diagnostics,
            Some(status),
        ));
    }
    if !posted_sequence_observed(status, request.sequence) {
        let failure = LatchFailure::runtime(
            LatchFailureStage::PostVerification,
            LatchFailureReason::PostedSequenceUnverified,
            format!(
                "posted={} active={} pending={} pending_sequence={}",
                request.sequence,
                status.active_sequence,
                u8::from(status.pending()),
                status.pending_sequence
            ),
        )
        .with_wire_diagnostics(rejected_wire_diagnostics(after_sample.diagnostics));
        return Err(with_post_failure_evidence(
            hardware,
            failure,
            post.diagnostics,
            Some(status),
        ));
    }
    Ok(LatchPostReceipt {
        status,
        post_us,
        status_us: locked_status_us.saturating_add(post_status_us),
        status_reads,
        status_wire_attempts,
        set_supported,
        status_supported,
        receipt_crc: post.diagnostics.receipt_crc,
    })
}

pub fn read_status_sample(
    hardware: &mut impl LatchHardware,
    capabilities: Option<mister_magik_latch_contract::LatchCapabilities>,
) -> Result<LatchedFbufStatusSample, LatchedFbufStatusReadError> {
    match hardware.read_latched_status() {
        Ok(mut sample) => {
            stamp_wire_capabilities(&mut sample.diagnostics, capabilities);
            Ok(sample)
        }
        Err(mut error) => {
            stamp_wire_capabilities(&mut error.diagnostics, capabilities);
            Err(error)
        }
    }
}

pub fn latch_status_read_failure(
    stage: LatchFailureStage,
    error: LatchedFbufStatusReadError,
) -> LatchFailure {
    let detail = error.to_string();
    LatchFailure::runtime(stage, LatchFailureReason::FpgaTransportFailed, detail)
        .with_wire_diagnostics(*error.diagnostics)
}

pub fn rejected_wire_diagnostics(mut diagnostics: LatchWireDiagnostics) -> LatchWireDiagnostics {
    diagnostics.decision = LatchWireDecision::Rejected;
    diagnostics
}

pub fn posted_sequence_observed(status: LatchedFbufStatus, sequence: u16) -> bool {
    status.active_sequence == sequence || (status.pending() && status.pending_sequence == sequence)
}

pub fn wait_for_latch_completion(
    hardware: &mut impl LatchHardware,
    posted_sequence: u16,
    timeout: Duration,
) -> Result<LatchCompletion, LatchFailure> {
    let started = Instant::now();
    let cpu_started = thread_cpu_us();
    let mut poll_count = 0u16;
    let mut post_observed = false;
    let mut diagnostics = LatchWireDiagnostics::default();
    let capabilities = hardware.negotiated_latch_capabilities();
    loop {
        let status = match read_status_sample(hardware, capabilities) {
            Ok(sample) => {
                diagnostics.append(&sample.diagnostics);
                sample.status
            }
            Err(mut error) => {
                let terminal_decision = error.diagnostics.decision;
                diagnostics.append(&error.diagnostics);
                diagnostics.decision = terminal_decision;
                error.diagnostics = Box::new(diagnostics);
                return Err(latch_status_read_failure(
                    LatchFailureStage::PostVerification,
                    error,
                ));
            }
        };
        poll_count = poll_count.saturating_add(1);
        if !status.supported() {
            return Err(LatchFailure::incompatible(
                LatchFailureStage::PostVerification,
                LatchFailureReason::FpgaStatusUnsupported,
                "latch completion status is unsupported",
            )
            .with_wire_diagnostics(rejected_wire_diagnostics(diagnostics)));
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
            )
            .with_wire_diagnostics(rejected_wire_diagnostics(diagnostics)));
        }
        std::thread::yield_now();
    }
}

#[derive(Debug)]
pub struct LatchCompletion {
    pub status: LatchedFbufStatus,
    pub poll_count: u16,
    pub wall_us: u64,
    pub cpu_us: u64,
}

pub fn read_post_status(
    sequence: u16,
    budget: &mut LogicalStatusReadBudget,
    mut read_status: impl FnMut(
        &mut LogicalStatusReadBudget,
    ) -> Result<LatchedFbufStatusSample, LatchFailure>,
) -> Result<(LatchedFbufStatusSample, u64, u8, u8), LatchFailure> {
    let started = Instant::now();
    let mut sample = read_status(budget)?;
    let mut diagnostics = std::mem::take(&mut sample.diagnostics);
    while !budget.exhausted() {
        if !sample.status.supported() || posted_sequence_observed(sample.status, sequence) {
            break;
        }
        std::thread::yield_now();
        sample = match read_status(budget) {
            Ok(mut next) => {
                diagnostics.append(&next.diagnostics);
                next.diagnostics = Default::default();
                next
            }
            Err(mut failure) => {
                if let Some(terminal) = failure.wire_diagnostics.take() {
                    let terminal_decision = terminal.decision;
                    diagnostics.append(&terminal);
                    diagnostics.decision = terminal_decision;
                    failure.wire_diagnostics = Some(Box::new(diagnostics));
                }
                return Err(failure);
            }
        };
    }
    if budget.reads > 1 {
        diagnostics.decision = LatchWireDecision::Corroborated;
    }
    let wire_attempts = diagnostics.attempt_count;
    sample.diagnostics = diagnostics;
    Ok((
        sample,
        started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        budget.reads,
        wire_attempts,
    ))
}

fn with_post_failure_evidence(
    hardware: &mut impl LatchHardware,
    mut failure: LatchFailure,
    post: LatchPostDiagnostics,
    status: Option<LatchedFbufStatus>,
) -> LatchFailure {
    failure = failure.with_post_diagnostics(post);
    if let Some(status) = status {
        failure = failure.with_status_observation(status.into());
        if status.rejection_reason() != mister_magik_latch_contract::REJECT_NONE
            && let Ok(Some(observation)) = hardware.read_latch_rejection_diagnostics()
        {
            failure = failure.with_rejection_observation(observation);
        }
    }
    failure
}

fn stamp_wire_capabilities(
    diagnostics: &mut LatchWireDiagnostics,
    capabilities: Option<mister_magik_latch_contract::LatchCapabilities>,
) {
    if let Some(capabilities) = capabilities {
        diagnostics.protocol_version = Some(capabilities.protocol_version);
        diagnostics.capability_flags = Some(capabilities.flags);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_status_reads_are_strictly_bounded() {
        let mut budget = LogicalStatusReadBudget::new();
        assert_eq!(budget.consume(), Ok(()));
        assert_eq!(budget.consume(), Ok(()));
        assert_eq!(budget.consume(), Ok(()));
        assert_eq!(budget.consume(), Err(LogicalStatusReadBudgetExhausted));
        assert!(budget.exhausted());
    }
}
