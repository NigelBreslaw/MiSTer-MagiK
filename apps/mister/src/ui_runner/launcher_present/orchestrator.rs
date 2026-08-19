// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::super::*;
use crate::ui_runner::launcher_pacing::LauncherPacingTrace;
use crate::ui_runner::launcher_readiness::{PostedSourceFrameEvidence, SourceFrameEvidence};
use mister_magik_fb::framebuffer::vsync::VsyncPace;
use mister_magik_fb::latch_readiness::{LatchFailure, LatchFailureReason, LatchFailureStage};

enum LauncherPresenterState<L> {
    ExplicitFb0,
    Latch(L),
    Frozen { failure: LatchFailure },
}

fn presenter_state_uses_latch<L>(state: &LauncherPresenterState<L>) -> bool {
    matches!(state, LauncherPresenterState::Latch(_))
}

fn direct_hidden_framebuffer_geometry_available(ui: &UiDisplay) -> bool {
    ui.render_w() == ui.fb_w() && ui.render_h() == ui.fb_h() && !ui.output_route().is_crt()
}

fn startup_intro_native_hidden_geometry_available(ui: &UiDisplay) -> bool {
    ui.is_native_composition()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LatchAutoRetryState {
    Disabled,
    Ready,
    InProgress,
}

const LATCH_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(60),
];
const MAX_AUTO_RETRY_ATTEMPTS: u8 = LATCH_RETRY_DELAYS.len() as u8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhysicalOverlayRole {
    Preview,
    Arcade,
}

impl PhysicalOverlayRole {
    const fn label(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Arcade => "arcade",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArcadeOverlayCopySource {
    PublishedPhysical,
    CachedLogical,
    MissingRequiredPublication,
}

const fn arcade_overlay_copy_source(
    publication_available: bool,
    publication_required: bool,
) -> ArcadeOverlayCopySource {
    if publication_available {
        ArcadeOverlayCopySource::PublishedPhysical
    } else if publication_required {
        ArcadeOverlayCopySource::MissingRequiredPublication
    } else {
        ArcadeOverlayCopySource::CachedLogical
    }
}

#[derive(Debug, Eq, PartialEq)]
struct PhysicalOverlayFailure {
    role: PhysicalOverlayRole,
    slot_index: u8,
    rect: DirtyRect,
    expected_rows: u32,
    copied_rows: u32,
    layout_generation: u64,
    content_generation: u64,
    backing_key: String,
    cause: Option<String>,
}

impl std::fmt::Display for PhysicalOverlayFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "physical overlay copy failed: role={} slot={} rect={:?} expected_rows={} copied_rows={} layout_generation={} content_generation={} backing_key={}",
            self.role.label(),
            self.slot_index,
            self.rect,
            self.expected_rows,
            self.copied_rows,
            self.layout_generation,
            self.content_generation,
            self.backing_key,
        )?;
        if let Some(cause) = &self.cause {
            write!(formatter, " cause={cause}")?;
        }
        Ok(())
    }
}

fn require_complete_overlay_copy(
    role: PhysicalOverlayRole,
    slot_index: u8,
    rect: DirtyRect,
    copied_rows: u32,
    layout_generation: u64,
    content_generation: u64,
    backing_key: impl FnOnce() -> String,
) -> Result<(), String> {
    let expected_rows = rect.rows();
    if copied_rows == expected_rows {
        Ok(())
    } else {
        Err(PhysicalOverlayFailure {
            role,
            slot_index,
            rect,
            expected_rows,
            copied_rows,
            layout_generation,
            content_generation,
            backing_key: backing_key(),
            cause: None,
        }
        .to_string())
    }
}

fn copy_published_overlay_rects(
    hidden: &mut ScanoutSlotsRgb565Framebuffer,
    publication: &PhysicalLayerPublication,
    role: PhysicalOverlayRole,
    slot_index: u8,
    rects: DirtyRectList,
) -> Result<(u32, usize), String> {
    let view = publication.view();
    let mut rows = 0_u32;
    let mut pixels = 0_usize;
    for rect in rects.iter() {
        let copied_rows = copy_physical_layer_rect_to_hidden(hidden, view, rect);
        require_complete_overlay_copy(
            role,
            slot_index,
            rect,
            copied_rows,
            publication.layout_generation(),
            publication.content_generation(),
            || format!("{:?}", publication.backing_key()),
        )?;
        rows = rows.saturating_add(copied_rows);
        pixels = pixels.saturating_add(rect.width().saturating_mul(copied_rows as usize));
    }
    Ok((rows, pixels))
}

fn refresh_physical_layer_mirror(
    mirror: &mut PhysicalLayerSlotMirror,
    publication: &PhysicalLayerPublication,
) -> bool {
    let view = publication.view();
    let rect = view.rect();
    let len = rect.width().saturating_mul(rect.rows() as usize);
    mirror.pixels.resize(len, Rgb565Pixel(0));
    for row in 0..rect.rows() as usize {
        let Some(source) = view.row(rect, row) else {
            mirror.invalidate();
            return false;
        };
        let start = row * rect.width();
        mirror.pixels[start..start + rect.width()].copy_from_slice(source);
    }
    mirror.rect = Some(rect);
    true
}

fn copy_published_arcade_with_mirror(
    hidden: &mut ScanoutSlotsRgb565Framebuffer,
    publication: &PhysicalLayerPublication,
    mirror: &mut PhysicalLayerSlotMirror,
    slot_index: u8,
    diff_safe: bool,
    update: PhysicalLayerUpdate,
) -> Result<(PresentCopyStats, PhysicalLayerCopyTrace), String> {
    let view = publication.view();
    let rect = update.dirty_rect();
    if view.rect() != rect {
        return Err(format!(
            "physical Arcade update rect mismatch: requested={rect:?} backing={:?}",
            view.rect()
        ));
    }
    let dense_pixels = rect.width().saturating_mul(rect.rows() as usize);
    if matches!(update, PhysicalLayerUpdate::Scroll { .. }) {
        // A physical scroll changes most destination pixels. Reading the
        // write-combined scanout slot is prohibitively slow, while comparing
        // against and refreshing a normal-RAM mirror cannot reduce the dense
        // write. Keep the safe source-to-slot copy and skip both mirror costs.
        mirror.invalidate();
        let write_started = Instant::now();
        let (rows, written_pixels) = copy_published_overlay_rects(
            hidden,
            publication,
            PhysicalOverlayRole::Arcade,
            slot_index,
            DirtyRectList::from_one(rect),
        )?;
        let write_us = write_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        return Ok((
            PresentCopyStats {
                rows,
                bytes: written_pixels.saturating_mul(2),
            },
            PhysicalLayerCopyTrace {
                decision: PhysicalLayerCopyDecision::ScrollRecovery,
                diff_safe,
                write_us,
                written_pixels: written_pixels as u64,
                changed_rows: rows,
                ..PhysicalLayerCopyTrace::default()
            },
        ));
    }
    let mirror_valid =
        diff_safe && mirror.rect == Some(rect) && mirror.pixels.len() == dense_pixels;
    let mut compare_us = 0_u64;
    if mirror_valid {
        let compare_started = Instant::now();
        let span_pixels = collect_rgb565_row_spans(
            view.pixels(),
            &mirror.pixels,
            rect.width(),
            &mut mirror.row_spans,
        )
        .ok_or_else(|| "physical Arcade mirror geometry is invalid".to_string())?;
        compare_us = compare_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        if span_pixels.saturating_mul(2) < dense_pixels {
            let write_started = Instant::now();
            let mut rows = 0_u32;
            for &(row, x0, x1) in &mirror.row_spans {
                let damage = DirtyRect {
                    x0: rect.x0 + x0,
                    y0: rect.y0 + row,
                    x1: rect.x0 + x1,
                    y1: rect.y0 + row + 1,
                };
                let (copied_rows, _) = copy_published_overlay_rects(
                    hidden,
                    publication,
                    PhysicalOverlayRole::Arcade,
                    slot_index,
                    DirtyRectList::from_one(damage),
                )?;
                rows = rows.saturating_add(copied_rows);
            }
            let write_us = write_started
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            let mirror_started = Instant::now();
            if !refresh_physical_layer_mirror(mirror, publication) {
                return Err("physical Arcade mirror refresh failed".into());
            }
            let mirror_refresh_us = mirror_started
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            return Ok((
                PresentCopyStats {
                    rows,
                    bytes: span_pixels.saturating_mul(2),
                },
                PhysicalLayerCopyTrace {
                    decision: PhysicalLayerCopyDecision::SparseDiff,
                    diff_safe,
                    mirror_valid: true,
                    compare_us,
                    write_us,
                    mirror_refresh_us,
                    compared_pixels: dense_pixels as u64,
                    written_pixels: span_pixels as u64,
                    mirror_refresh_pixels: dense_pixels as u64,
                    changed_rows: rows,
                },
            ));
        }
    }

    let write_started = Instant::now();
    let (rows, written_pixels) = copy_published_overlay_rects(
        hidden,
        publication,
        PhysicalOverlayRole::Arcade,
        slot_index,
        DirtyRectList::from_one(rect),
    )?;
    let write_us = write_started
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    let mirror_started = Instant::now();
    if !refresh_physical_layer_mirror(mirror, publication) {
        return Err("physical Arcade mirror recovery failed".into());
    }
    let mirror_refresh_us = mirror_started
        .elapsed()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64;
    let decision = if mirror_valid {
        PhysicalLayerCopyDecision::FullCopy
    } else if diff_safe {
        PhysicalLayerCopyDecision::MirrorRecovery
    } else {
        PhysicalLayerCopyDecision::FullCopy
    };
    Ok((
        PresentCopyStats {
            rows,
            bytes: written_pixels.saturating_mul(2),
        },
        PhysicalLayerCopyTrace {
            decision,
            diff_safe,
            mirror_valid,
            compare_us,
            write_us,
            mirror_refresh_us,
            written_pixels: written_pixels as u64,
            compared_pixels: if mirror_valid { dense_pixels as u64 } else { 0 },
            mirror_refresh_pixels: dense_pixels as u64,
            changed_rows: rows,
            ..PhysicalLayerCopyTrace::default()
        },
    ))
}

pub(in crate::ui_runner) struct LauncherPresenter<L = FpgaVblankLatchHiddenPresenter> {
    state: LauncherPresenterState<L>,
    failure_transitions: u64,
    first_failure: Option<LatchFailure>,
    latest_failure: Option<LatchFailure>,
    failure_history: Vec<LatchFailure>,
    retry_attempts: u8,
    auto_retry: LatchAutoRetryState,
    next_retry_at: Option<Instant>,
    latest_retry_result: &'static str,
    recovery_state: &'static str,
    supervised_restart_requested: bool,
}

pub(in crate::ui_runner) struct LauncherPresentFrame {
    pub(in crate::ui_runner) plan: LauncherFramePlan,
    pub(in crate::ui_runner) startup_can_present: bool,
    pub(in crate::ui_runner) first_visible_copy_done: bool,
    pub(in crate::ui_runner) frame_start_phase_us: u64,
    pub(in crate::ui_runner) pre_render_pace: Option<(VsyncPace, Instant, u128)>,
    pub(in crate::ui_runner) frame_analytics_mode: FrameAnalyticsMode,
    pub(in crate::ui_runner) stream_motion_active: bool,
    pub(in crate::ui_runner) direct_hidden_mode: bool,
    pub(in crate::ui_runner) completed_hidden_frame: Option<CompletedHiddenFrame>,
    pub(in crate::ui_runner) capture_readiness_source: bool,
    pub(in crate::ui_runner) profile_latch_phases: bool,
}

pub(in crate::ui_runner) struct LauncherPresentTargets<'a, 'target> {
    pub(in crate::ui_runner) layer_target: &'a LayerTarget<'target>,
    pub(in crate::ui_runner) fb0: &'a mut MappedRgb565Framebuffer,
    pub(in crate::ui_runner) hardware: &'a mut Fpga,
    pub(in crate::ui_runner) arcade_list_renderer: &'a mut ArcadeListRenderer,
    pub(in crate::ui_runner) pacer: &'a mut VsyncPacer,
    pub(in crate::ui_runner) present_timing: PresentTiming,
}

pub(in crate::ui_runner) struct LauncherPresentCycle {
    pub(in crate::ui_runner) presentation: LauncherPresentResult,
    pub(in crate::ui_runner) frame_t3: Instant,
    pub(in crate::ui_runner) frame_t4: Instant,
    pub(in crate::ui_runner) cpu_t3: FrameAnalyticsCpuStamp,
    pub(in crate::ui_runner) cpu_t4: FrameAnalyticsCpuStamp,
    pub(in crate::ui_runner) pacing_trace: LauncherPacingTrace,
}

trait PresentationAdapters<L> {
    type Output;

    fn present_latch(
        &mut self,
        latch: &mut L,
        frame: LauncherFramePlan,
    ) -> Result<Self::Output, LatchFailure>;

    fn present_frozen(&mut self) -> Self::Output;

    fn present_fb0(&mut self, frame: LauncherFramePlan) -> Self::Output;
}

impl LauncherPresenter<FpgaVblankLatchHiddenPresenter> {
    pub(in crate::ui_runner) fn new(
        ui: &UiDisplay,
        present_backend: LauncherPresentBackend,
    ) -> Self {
        let mut first_failure = None;
        let mut latest_failure = None;
        let mut failure_history = Vec::new();
        let mut auto_retry = LatchAutoRetryState::Disabled;
        let mut recovery_state = "not-needed";
        let state = match present_backend {
            LauncherPresentBackend::FpgaVblankLatchHidden => {
                match FpgaVblankLatchHiddenPresenter::open(ui) {
                    Ok(presenter) => LauncherPresenterState::Latch(presenter),
                    Err(failure) => {
                        first_failure = Some(failure.clone());
                        latest_failure = Some(failure.clone());
                        failure_history.push(failure.clone());
                        if failure.is_transient_runtime_failure() {
                            auto_retry = LatchAutoRetryState::Ready;
                            recovery_state = "output-frozen";
                        } else {
                            recovery_state = "terminal-failure";
                        }
                        crate::ui_errln!(
                            "latch_failure_tsv\tvalid=0\tstate={}\tstage={}\treason={}\taction=freeze-last-good\tdetail={}",
                            failure.state.code(),
                            failure.stage.code(),
                            failure.reason_code(),
                            failure.detail.replace(['\t', '\n', '\r'], " ")
                        );
                        LauncherPresenterState::Frozen { failure }
                    }
                }
            }
            LauncherPresentBackend::None | LauncherPresentBackend::Fb0Dirty => {
                LauncherPresenterState::ExplicitFb0
            }
        };
        let failure_transitions = u64::from(matches!(state, LauncherPresenterState::Frozen { .. }));
        let mut presenter = Self {
            state,
            failure_transitions,
            first_failure,
            latest_failure,
            failure_history,
            retry_attempts: 0,
            auto_retry,
            next_retry_at: None,
            latest_retry_result: "not-attempted",
            recovery_state,
            supervised_restart_requested: false,
        };
        if presenter.auto_retry == LatchAutoRetryState::Ready {
            presenter.schedule_automatic_retry_at(Instant::now());
        }
        presenter.persist_recovery_evidence();
        presenter
    }

    pub(in crate::ui_runner) fn present(
        &mut self,
        frame: LauncherPresentFrame,
        targets: LauncherPresentTargets<'_, '_>,
        display: &mut LauncherDisplaySession,
    ) -> LauncherPresentCycle {
        let mut adapters = LivePresentationAdapters {
            targets,
            display,
            first_visible_copy_done: frame.first_visible_copy_done,
            frame_start_phase_us: frame.frame_start_phase_us,
            pre_render_pace: frame.pre_render_pace,
            frame_analytics_mode: frame.frame_analytics_mode,
            stream_motion_active: frame.stream_motion_active,
            direct_hidden_mode: frame.direct_hidden_mode,
            completed_hidden_frame: frame.completed_hidden_frame,
            capture_readiness_source: frame.capture_readiness_source,
            profile_latch_phases: frame.profile_latch_phases,
        };
        if !frame.startup_can_present {
            return adapters.present_suppressed();
        }
        self.present_with(frame.plan, &mut adapters)
    }

    pub(in crate::ui_runner) fn fail_latch_completion(&mut self, failure: LatchFailure) {
        self.transition_latch_failure(failure);
    }

    pub(in crate::ui_runner) fn retry_latch_automatically(&mut self, ui: &UiDisplay) -> bool {
        if !self.begin_automatic_retry(Instant::now()) {
            return false;
        }
        let result = FpgaVblankLatchHiddenPresenter::open(ui);
        self.apply_retry_result(result);
        true
    }

    pub(in crate::ui_runner) fn publish_stream_refinement_if_due(&self) -> bool {
        match &self.state {
            LauncherPresenterState::Latch(latch) => latch.publish_requested_full_snapshot(),
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Frozen { .. } => false,
        }
    }

    pub(in crate::ui_runner) fn direct_hidden_framebuffer_slots_available(
        &self,
        ui: &UiDisplay,
    ) -> bool {
        if !presenter_state_uses_latch(&self.state) {
            return false;
        }
        matches!(
            &self.state,
            LauncherPresenterState::Latch(latch)
                if latch.exact_identity_geometry()
                    && direct_hidden_framebuffer_geometry_available(ui)
        )
    }

    pub(in crate::ui_runner) fn startup_intro_native_hidden_slots_available(
        &self,
        ui: &UiDisplay,
    ) -> bool {
        matches!(&self.state, LauncherPresenterState::Latch(_))
            && startup_intro_native_hidden_geometry_available(ui)
    }

    pub(in crate::ui_runner) fn try_issue_hidden_slot_render_grant(
        &mut self,
        hardware: &mut Fpga,
        display: &mut LauncherDisplaySession,
    ) -> Result<Option<HiddenSlotRenderGrant>, LatchFailure> {
        match &mut self.state {
            LauncherPresenterState::Latch(latch) => {
                latch.try_issue_hidden_slot_render_grant(hardware, display)
            }
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Frozen { .. } => Ok(None),
        }
    }

    pub(in crate::ui_runner) fn try_render_direct_hidden_frame<R>(
        &mut self,
        hardware: &mut Fpga,
        display: &mut LauncherDisplaySession,
        render: R,
    ) -> Result<Option<CompletedHiddenFrame>, LatchFailure>
    where
        R: FnOnce(&mut [Rgb565Pixel]) -> bool,
    {
        match &mut self.state {
            LauncherPresenterState::Latch(latch) => {
                latch.try_render_direct_hidden_frame(hardware, display, render)
            }
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Frozen { .. } => Ok(None),
        }
    }

    pub(in crate::ui_runner) fn try_issue_startup_intro_hidden_slot_render_grant(
        &mut self,
        hardware: &mut Fpga,
        display: &mut LauncherDisplaySession,
    ) -> Result<Option<HiddenSlotRenderGrant>, LatchFailure> {
        match &mut self.state {
            LauncherPresenterState::Latch(latch) => {
                latch.try_issue_startup_intro_hidden_slot_render_grant(hardware, display)
            }
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Frozen { .. } => Ok(None),
        }
    }

    pub(in crate::ui_runner) fn take_direct_hidden_frame_buffers(
        &mut self,
    ) -> Result<PluginLatchFrameBuffers, LatchFailure> {
        match &mut self.state {
            LauncherPresenterState::Latch(latch) => latch.take_direct_frame_buffers(),
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Frozen { .. } => {
                Err(LatchFailure::runtime(
                    LatchFailureStage::BufferMap,
                    LatchFailureReason::ScanoutMapFailed,
                    "direct hidden mappings require an active latch presenter",
                ))
            }
        }
    }

    pub(in crate::ui_runner) fn restore_direct_hidden_frame_buffers(
        &mut self,
        returned: Option<PluginLatchFrameBuffers>,
    ) -> Result<(), LatchFailure> {
        match &mut self.state {
            LauncherPresenterState::Latch(latch) => latch.restore_direct_frame_buffers(returned),
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Frozen { .. } => {
                Err(LatchFailure::runtime(
                    LatchFailureStage::BufferMap,
                    LatchFailureReason::ScanoutMapFailed,
                    "direct hidden mappings returned without an active latch presenter",
                ))
            }
        }
    }

    pub(in crate::ui_runner) fn invalidate_external_hidden_mode(&mut self) {
        if let LauncherPresenterState::Latch(latch) = &mut self.state {
            latch.invalidate_external_mode();
        }
    }
}

impl<L> LauncherPresenter<L> {
    pub(in crate::ui_runner) fn pacing_backend(&self) -> LauncherPresentBackend {
        match self.state {
            LauncherPresenterState::Latch(_) => LauncherPresentBackend::FpgaVblankLatchHidden,
            LauncherPresenterState::ExplicitFb0 => LauncherPresentBackend::Fb0Dirty,
            LauncherPresenterState::Frozen { .. } => LauncherPresentBackend::None,
        }
    }

    pub(in crate::ui_runner) fn needs_frame(&self) -> bool {
        false
    }

    pub(in crate::ui_runner) fn latch_failure(&self) -> Option<&LatchFailure> {
        match &self.state {
            LauncherPresenterState::Frozen { failure } => Some(failure),
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Latch(_) => None,
        }
    }

    pub(in crate::ui_runner) fn display_frozen(&self) -> bool {
        matches!(self.state, LauncherPresenterState::Frozen { .. })
    }

    pub(in crate::ui_runner) fn failure_transitions(&self) -> u64 {
        self.failure_transitions
    }

    pub(in crate::ui_runner) fn retry_attempts(&self) -> u8 {
        self.retry_attempts
    }

    pub(in crate::ui_runner) fn take_supervised_restart_request(&mut self) -> bool {
        std::mem::take(&mut self.supervised_restart_requested)
    }

    fn transition_latch_failure(&mut self, latch_error: LatchFailure) {
        crate::ui_errln!(
            "latch_failure_tsv\tvalid=0\tstate={}\tstage={}\treason={}\taction=freeze-last-good\tdetail={}",
            latch_error.state.code(),
            latch_error.stage.code(),
            latch_error.reason_code(),
            latch_error.detail.replace(['\t', '\n', '\r'], " ")
        );
        boot_analytics::event(
            "fpga_vblank_latch_output_frozen",
            format!(
                "state={:?} stage={:?} reason={} detail={}",
                latch_error.state,
                latch_error.stage,
                latch_error.reason_code(),
                latch_error.detail
            ),
        );
        if self.first_failure.is_none() {
            self.first_failure = Some(latch_error.clone());
        }
        self.latest_failure = Some(latch_error.clone());
        self.failure_history.push(latch_error.clone());
        let automatic_retry = latch_error.is_transient_runtime_failure();
        self.auto_retry = if automatic_retry && self.retry_attempts < MAX_AUTO_RETRY_ATTEMPTS {
            LatchAutoRetryState::Ready
        } else {
            LatchAutoRetryState::Disabled
        };
        self.next_retry_at = None;
        self.recovery_state = if self.auto_retry == LatchAutoRetryState::Ready {
            "output-frozen"
        } else {
            "terminal-failure"
        };
        if automatic_retry && self.retry_attempts >= MAX_AUTO_RETRY_ATTEMPTS {
            self.supervised_restart_requested = true;
        }
        self.failure_transitions = self.failure_transitions.saturating_add(1);
        self.state = LauncherPresenterState::Frozen {
            failure: latch_error,
        };
        if self.auto_retry == LatchAutoRetryState::Ready {
            self.schedule_automatic_retry_at(Instant::now());
        }
        self.persist_recovery_evidence();
    }

    fn begin_automatic_retry(&mut self, now: Instant) -> bool {
        if self.auto_retry != LatchAutoRetryState::Ready
            || self.retry_attempts >= MAX_AUTO_RETRY_ATTEMPTS
            || self.next_retry_at.is_none_or(|deadline| now < deadline)
        {
            return false;
        }
        self.retry_attempts = self.retry_attempts.saturating_add(1);
        self.auto_retry = LatchAutoRetryState::InProgress;
        self.next_retry_at = None;
        self.latest_retry_result = "in-progress";
        self.recovery_state = "automatic-retry";
        self.persist_recovery_evidence();
        true
    }

    fn apply_retry_result(&mut self, result: Result<L, LatchFailure>) -> bool {
        match result {
            Ok(latch) => {
                self.state = LauncherPresenterState::Latch(latch);
                self.auto_retry = LatchAutoRetryState::Disabled;
                self.next_retry_at = None;
                self.latest_retry_result = "success";
                self.recovery_state = "recovered-automatically";
                self.persist_recovery_evidence();
                true
            }
            Err(failure) => {
                if self.retry_attempts > 0 {
                    self.latest_retry_result = "failure";
                }
                self.transition_latch_failure(failure);
                false
            }
        }
    }

    fn schedule_automatic_retry_at(&mut self, now: Instant) {
        let delay_index =
            usize::from(self.retry_attempts).min(LATCH_RETRY_DELAYS.len().saturating_sub(1));
        self.next_retry_at = Some(now + LATCH_RETRY_DELAYS[delay_index]);
        self.persist_recovery_evidence();
    }

    fn persist_recovery_evidence(&self) {
        let (Some(first), Some(latest)) = (&self.first_failure, &self.latest_failure) else {
            return;
        };
        persist_latch_failure(
            first,
            latest,
            &self.failure_history,
            self.retry_attempts,
            self.latest_retry_result,
            self.recovery_state,
        );
    }

    fn present_with<A>(&mut self, frame: LauncherFramePlan, adapters: &mut A) -> A::Output
    where
        A: PresentationAdapters<L>,
    {
        let latch_error = match &mut self.state {
            LauncherPresenterState::ExplicitFb0 => {
                return adapters.present_fb0(frame);
            }
            LauncherPresenterState::Frozen { .. } => return adapters.present_frozen(),
            LauncherPresenterState::Latch(latch) => match adapters.present_latch(latch, frame) {
                Ok(output) => return output,
                Err(error) => error,
            },
        };

        self.transition_latch_failure(latch_error);
        adapters.present_frozen()
    }
}

fn persist_latch_failure(
    first: &LatchFailure,
    latest: &LatchFailure,
    failure_history: &[LatchFailure],
    retry_attempts: u8,
    latest_retry_result: &str,
    recovery_state: &str,
) {
    let evidence = mister_magik_fb::latch_readiness::LatchFailureEvidence::for_recovery(
        first,
        latest,
        failure_history,
        retry_attempts,
        latest_retry_result,
        recovery_state,
    );
    if let Err(error) =
        evidence.write_atomic(mister_magik_fb::latch_readiness::RUNTIME_FAILURE_PATH)
    {
        crate::ui_errln!("latch_failure_evidence_write_failed error={error}");
    }
    let report_path = crate::latch_failure_report::enqueue(evidence);
    crate::ui_errln!("latch_failure_report_queued path={}", report_path.display());
}

struct LivePresentationAdapters<'a, 'target> {
    targets: LauncherPresentTargets<'a, 'target>,
    display: &'a mut LauncherDisplaySession,
    first_visible_copy_done: bool,
    frame_start_phase_us: u64,
    pre_render_pace: Option<(VsyncPace, Instant, u128)>,
    frame_analytics_mode: FrameAnalyticsMode,
    stream_motion_active: bool,
    direct_hidden_mode: bool,
    completed_hidden_frame: Option<CompletedHiddenFrame>,
    capture_readiness_source: bool,
    profile_latch_phases: bool,
}

impl LivePresentationAdapters<'_, '_> {
    fn pace_before_fb0(
        &mut self,
    ) -> (
        Option<VsyncPace>,
        Instant,
        FrameAnalyticsCpuStamp,
        LauncherPacingTrace,
    ) {
        let pace = if self.first_visible_copy_done {
            let (pace, vsync_done) = match self.pre_render_pace.take() {
                Some((pace, vsync_done, _)) => (pace, vsync_done),
                None => {
                    let pace = self.targets.pacer.wait();
                    (pace, Instant::now())
                }
            };
            self.targets
                .present_timing
                .wait_until_present_time(vsync_done);
            let cpu_t3 = FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode);
            let frame_t3 = Instant::now();
            (Some(pace), frame_t3, cpu_t3)
        } else {
            (
                None,
                Instant::now(),
                FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode),
            )
        };
        let pacing_trace = LauncherPacingTrace::from_pace(
            pace.0.as_ref(),
            self.frame_start_phase_us,
            self.targets.pacer.period_us(),
            pace.1,
        );
        (pace.0, pace.1, pace.2, pacing_trace)
    }

    fn present_suppressed(&mut self) -> LauncherPresentCycle {
        let (_, frame_t3, cpu_t3, pacing_trace) = self.pace_before_fb0();
        LauncherPresentCycle {
            presentation: empty_present_result(),
            frame_t3,
            frame_t4: Instant::now(),
            cpu_t3,
            cpu_t4: FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode),
            pacing_trace,
        }
    }
}

impl PresentationAdapters<FpgaVblankLatchHiddenPresenter> for LivePresentationAdapters<'_, '_> {
    type Output = LauncherPresentCycle;

    fn present_latch(
        &mut self,
        latch: &mut FpgaVblankLatchHiddenPresenter,
        frame: LauncherFramePlan,
    ) -> Result<Self::Output, LatchFailure> {
        let frame_t3 = Instant::now();
        let cpu_t3 = FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode);
        let present_phase_us = self.targets.pacer.age_since_last_hit_us(frame_t3) as u128;
        let mut hidden_preview_compose_us = 0u128;
        let mut hidden_arcade_compose_us = 0u128;
        let mut direct_preview_rows = 0u32;
        let mut arcade_stats = PresentCopyStats::default();
        let mut arcade_copy_trace =
            crate::arcade_list_renderer::PersistentArcadeCopyTrace::default();
        let mut preview_redraw_rect = None;
        let mut arcade_redraw_update = None;
        let layer_target = self.targets.layer_target;
        let hardware = &mut *self.targets.hardware;
        let arcade_list_renderer = &mut *self.targets.arcade_list_renderer;
        if self.direct_hidden_mode {
            let Some(completed) = self.completed_hidden_frame.take() else {
                return Ok(LauncherPresentCycle {
                    presentation: direct_hidden_waiting_present_result(),
                    frame_t3,
                    frame_t4: Instant::now(),
                    cpu_t3,
                    cpu_t4: FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode),
                    pacing_trace: LauncherPacingTrace::from_pace_with_present_phase(
                        None,
                        self.frame_start_phase_us,
                        self.targets.pacer.period_us(),
                        present_phase_us,
                    ),
                });
            };
            let source_evidence = completed.source_evidence.clone();
            let stats = latch.present_completed_hidden_frame(
                completed,
                hardware,
                self.display,
                self.profile_latch_phases,
            )?;
            if let Some(scale) = mister_magik_fb::framebuffer::stream::configured_latch_scale(
                self.stream_motion_active,
            ) && let Some(frame_view) = latch.committed_frame_view_if_mapped(stats.buffer_index)
            {
                let _ =
                    mister_magik_fb::framebuffer::stream::publish_latch_snapshot(frame_view, scale);
            }
            let presentation = latch_present_result(
                stats,
                source_evidence,
                0,
                0,
                0,
                PresentCopyStats::default(),
                crate::arcade_list_renderer::PersistentArcadeCopyTrace::default(),
                None,
                None,
            );
            return Ok(LauncherPresentCycle {
                presentation,
                frame_t3,
                frame_t4: Instant::now(),
                cpu_t3,
                cpu_t4: FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode),
                pacing_trace: LauncherPacingTrace::from_pace_with_present_phase(
                    None,
                    self.frame_start_phase_us,
                    self.targets.pacer.period_us(),
                    present_phase_us,
                ),
            });
        }
        let stats = latch.present_cached_full_frame(
            layer_target.presentation_frame_view(),
            frame,
            hardware,
            self.display,
            self.profile_latch_phases,
            |hidden, plan, preview_publication, arcade_publication, arcade_mirror| {
                preview_redraw_rect = plan.preview_redraw;
                arcade_redraw_update = plan.arcade_redraw;
                if let Some(rect) = plan.preview_redraw {
                    let preview_pmu = self
                        .profile_latch_phases
                        .then(|| {
                            mister_magik_perf_events::sampled_span("gui.latch.preview-overlay-copy")
                        })
                        .flatten();
                    let started = Instant::now();
                    let (layout_generation, content_generation, backing_key) =
                        if let Some(publication) = preview_publication {
                            let view = publication.view();
                            direct_preview_rows =
                                copy_physical_layer_rect_to_hidden(hidden, view, rect);
                            (
                                publication.layout_generation(),
                                publication.content_generation(),
                                format!("{:?}", publication.backing_key()),
                            )
                        } else {
                            direct_preview_rows =
                                layer_target.copy_physical_layer_rect_to_hidden(hidden, rect);
                            (
                                layer_target.output_layout_generation(),
                                plan.preview_state_after().map_or(0, |state| state.version),
                                format!("{:?}", layer_target.direct_preview_backing_diagnostic()),
                            )
                        };
                    hidden_preview_compose_us = started.elapsed().as_micros();
                    drop(preview_pmu);
                    require_complete_overlay_copy(
                        PhysicalOverlayRole::Preview,
                        plan.slot_index,
                        rect,
                        direct_preview_rows,
                        layout_generation,
                        content_generation,
                        || backing_key,
                    )?;
                }
                if let Some(update) = plan.arcade_redraw {
                    let arcade_pmu = self
                        .profile_latch_phases
                        .then(|| {
                            mister_magik_perf_events::sampled_span("gui.latch.arcade-overlay-copy")
                        })
                        .flatten();
                    let started = Instant::now();
                    let arcade_rect = update.dirty_rect();
                    match arcade_overlay_copy_source(
                        arcade_publication.is_some(),
                        layer_target.arcade_overlay_requires_publication(),
                    ) {
                        ArcadeOverlayCopySource::PublishedPhysical => {
                            let publication = arcade_publication
                                .expect("published Arcade copy source has a publication");
                            (arcade_stats, arcade_copy_trace) = copy_published_arcade_with_mirror(
                                hidden,
                                publication,
                                arcade_mirror,
                                plan.slot_index,
                                plan.arcade_redraw_diff_safe,
                                update,
                            )?;
                        }
                        ArcadeOverlayCopySource::CachedLogical => {
                            arcade_stats = layer_target.copy_cached_arcade_list_update_to_hidden(
                                hidden,
                                arcade_list_renderer,
                                update,
                            );
                            require_complete_overlay_copy(
                                PhysicalOverlayRole::Arcade,
                                plan.slot_index,
                                arcade_rect,
                                arcade_stats.rows,
                                layer_target.output_layout_generation(),
                                plan.arcade_state_after().map_or(0, |state| state.version),
                                || "cached-logical-arcade".into(),
                            )?;
                        }
                        ArcadeOverlayCopySource::MissingRequiredPublication => {
                            let arcade_generation =
                                plan.arcade_state_after().map_or(0, |state| state.version);
                            let arcade_backing =
                                arcade_list_renderer.persistent_oriented_layer_diagnostic();
                            return Err(PhysicalOverlayFailure {
                                role: PhysicalOverlayRole::Arcade,
                                slot_index: plan.slot_index,
                                rect: arcade_rect,
                                expected_rows: arcade_rect.rows(),
                                copied_rows: 0,
                                layout_generation: layer_target.output_layout_generation(),
                                content_generation: arcade_generation,
                                backing_key: format!("{arcade_backing:?}"),
                                cause: Some(
                                    "frame plan requested Arcade without its atomic publication"
                                        .into(),
                                ),
                            }
                            .to_string());
                        }
                    }
                    hidden_arcade_compose_us = started.elapsed().as_micros();
                    drop(arcade_pmu);
                }
                Ok(())
            },
        )?;
        if let Some(scale) =
            mister_magik_fb::framebuffer::stream::configured_latch_scale(self.stream_motion_active)
        {
            let frame_view = latch.committed_frame_view(stats.buffer_index);
            let _ = mister_magik_fb::framebuffer::stream::publish_latch_snapshot(frame_view, scale);
        }
        let source_evidence = self
            .capture_readiness_source
            .then(|| {
                let frame = latch.committed_frame_view(stats.buffer_index);
                SourceFrameEvidence::from_rgb565_rows(
                    frame.pixels,
                    frame.width,
                    frame.height,
                    frame.stride_pixels,
                )
            })
            .flatten();
        let presentation = latch_present_result(
            stats,
            source_evidence,
            hidden_preview_compose_us,
            hidden_arcade_compose_us,
            direct_preview_rows,
            arcade_stats,
            arcade_copy_trace,
            preview_redraw_rect,
            arcade_redraw_update,
        );
        let frame_t4 = Instant::now();
        let cpu_t4 = FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode);
        Ok(LauncherPresentCycle {
            presentation,
            frame_t3,
            frame_t4,
            cpu_t3,
            cpu_t4,
            pacing_trace: LauncherPacingTrace::from_pace_with_present_phase(
                None,
                self.frame_start_phase_us,
                self.targets.pacer.period_us(),
                present_phase_us,
            ),
        })
    }

    fn present_frozen(&mut self) -> Self::Output {
        let mut cycle = self.present_suppressed();
        cycle.presentation = frozen_present_result();
        cycle
    }

    fn present_fb0(&mut self, frame: LauncherFramePlan) -> Self::Output {
        let (_, frame_t3, cpu_t3, pacing_trace) = self.pace_before_fb0();
        let cached_frame = self.targets.layer_target.presentation_frame_view();
        let direct_preview = self.targets.layer_target.direct_preview_view();
        let fb0 = &mut *self.targets.fb0;
        let arcade_list_renderer = &mut *self.targets.arcade_list_renderer;
        let stats = Fb0DirtyPresenter::present(Fb0DirtyPresentRequest {
            frame_plan: frame,
            cached_frame,
            direct_preview,
            fb0,
            arcade_list_renderer,
        });
        LauncherPresentCycle {
            presentation: fb0_present_result(stats),
            frame_t3,
            frame_t4: Instant::now(),
            cpu_t3,
            cpu_t4: FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode),
            pacing_trace,
        }
    }
}

fn fb0_present_result(stats: Fb0DirtyPresentStats) -> LauncherPresentResult {
    LauncherPresentResult {
        readiness_source_evidence: None,
        copied_rows: stats.copied_rows,
        direct_preview_rows: stats.direct_preview_rows,
        present_bytes: stats.present_bytes,
        wasted_present_bytes: 0,
        fb_present_us_override: None,
        vsync_us_override: None,
        cached_present_us: stats.cached_present_us,
        hidden_compose_us: 0,
        hidden_preview_compose_us: 0,
        hidden_arcade_compose_us: 0,
        direct_preview_present_us: stats.direct_preview_present_us,
        arcade_list_present_us: stats.arcade_list_present_us,
        arcade_copy_trace: crate::arcade_list_renderer::PersistentArcadeCopyTrace::default(),
        main_present_backend: LauncherPresentBackend::Fb0Dirty,
        main_present_status: LauncherPresentStatus::None,
        main_present_buffer: 0,
        main_present_hidden_copy_us: 0,
        main_present_hidden_publish_us: 0,
        main_present_hidden_copied_bytes: 0,
        main_present_hidden_invalid_bytes: 0,
        main_present_hidden_rect_count: 0,
        main_present_hidden_catchup_bytes: 0,
        main_present_hidden_full_copy: false,
        main_present_copy_path: "none",
        main_present_request_us: 0,
        main_present_set_vga_fb_us: 0,
        main_present_wait_us: 0,
        main_present_sequence: 0,
        main_present_post_active_sequence: 0,
        main_present_post_pending_sequence: 0,
        main_present_post_pending: false,
        main_present_flip_count: 0,
        main_present_drop_count: 0,
        main_present_receipt_crc: 0,
        arcade_update_label: stats.arcade_update_label,
    }
}

fn direct_hidden_waiting_present_result() -> LauncherPresentResult {
    empty_present_result()
}

fn frozen_present_result() -> LauncherPresentResult {
    let mut result = empty_present_result();
    result.main_present_status = LauncherPresentStatus::Frozen;
    result
}

fn latch_present_result(
    stats: FpgaVblankLatchHiddenPresentStats,
    source_evidence: Option<SourceFrameEvidence>,
    hidden_preview_compose_us: u128,
    hidden_arcade_compose_us: u128,
    direct_preview_rows: u32,
    arcade_stats: PresentCopyStats,
    arcade_copy_trace: crate::arcade_list_renderer::PersistentArcadeCopyTrace,
    preview_redraw_rect: Option<DirtyRect>,
    arcade_redraw_update: Option<ArcadeListUpdate>,
) -> LauncherPresentResult {
    let present_us = stats.copy_us
        + stats.publish_us
        + stats.post_us
        + stats.set_vga_fb_us
        + u128::from(stats.status_us);
    let preview_present_bytes = preview_redraw_rect
        .map(|rect| {
            rect.width()
                .saturating_mul(rect.rows() as usize)
                .saturating_mul(2)
        })
        .unwrap_or(0);
    LauncherPresentResult {
        readiness_source_evidence: source_evidence.map(|source| {
            PostedSourceFrameEvidence::new(stats.posted_sequence, stats.buffer_index, source)
        }),
        copied_rows: stats.copied_rows + direct_preview_rows + arcade_stats.rows,
        direct_preview_rows,
        present_bytes: stats.copied_bytes + preview_present_bytes + arcade_stats.bytes,
        wasted_present_bytes: 0,
        fb_present_us_override: Some(present_us),
        vsync_us_override: Some(0),
        cached_present_us: stats.copy_us,
        hidden_compose_us: hidden_preview_compose_us + hidden_arcade_compose_us,
        hidden_preview_compose_us,
        hidden_arcade_compose_us,
        direct_preview_present_us: hidden_preview_compose_us,
        arcade_list_present_us: hidden_arcade_compose_us,
        arcade_copy_trace,
        main_present_backend: LauncherPresentBackend::FpgaVblankLatchHidden,
        main_present_status: if stats.set_supported && stats.status_supported {
            LauncherPresentStatus::Ok
        } else {
            LauncherPresentStatus::Unsupported
        },
        main_present_buffer: stats.buffer_index,
        main_present_hidden_copy_us: stats.copy_us,
        main_present_hidden_publish_us: stats.publish_us,
        main_present_hidden_copied_bytes: stats.copied_bytes,
        main_present_hidden_invalid_bytes: stats.invalid_bytes,
        main_present_hidden_rect_count: stats.rect_count,
        main_present_hidden_catchup_bytes: stats.catchup_bytes,
        main_present_hidden_full_copy: stats.full_copy,
        main_present_copy_path: stats.copy_path.label(),
        main_present_request_us: stats.post_us + stats.set_vga_fb_us,
        main_present_set_vga_fb_us: stats.set_vga_fb_us,
        main_present_wait_us: stats.status_us,
        main_present_sequence: stats.posted_sequence,
        main_present_post_active_sequence: stats.post_active_sequence,
        main_present_post_pending_sequence: stats.post_pending_sequence,
        main_present_post_pending: stats.post_pending,
        main_present_flip_count: stats.flip_count,
        main_present_drop_count: stats.drop_count,
        main_present_receipt_crc: stats.receipt_crc,
        arcade_update_label: ArcadeUpdateTrace::from_update(arcade_redraw_update.as_ref()),
    }
}

fn empty_present_result() -> LauncherPresentResult {
    LauncherPresentResult {
        readiness_source_evidence: None,
        copied_rows: 0,
        direct_preview_rows: 0,
        present_bytes: 0,
        wasted_present_bytes: 0,
        fb_present_us_override: None,
        vsync_us_override: None,
        cached_present_us: 0,
        hidden_compose_us: 0,
        hidden_preview_compose_us: 0,
        hidden_arcade_compose_us: 0,
        direct_preview_present_us: 0,
        arcade_list_present_us: 0,
        arcade_copy_trace: crate::arcade_list_renderer::PersistentArcadeCopyTrace::default(),
        main_present_backend: LauncherPresentBackend::None,
        main_present_status: LauncherPresentStatus::None,
        main_present_buffer: 0,
        main_present_hidden_copy_us: 0,
        main_present_hidden_publish_us: 0,
        main_present_hidden_copied_bytes: 0,
        main_present_hidden_invalid_bytes: 0,
        main_present_hidden_rect_count: 0,
        main_present_hidden_catchup_bytes: 0,
        main_present_hidden_full_copy: false,
        main_present_copy_path: "none",
        main_present_request_us: 0,
        main_present_set_vga_fb_us: 0,
        main_present_wait_us: 0,
        main_present_sequence: 0,
        main_present_post_active_sequence: 0,
        main_present_post_pending_sequence: 0,
        main_present_post_pending: false,
        main_present_flip_count: 0,
        main_present_drop_count: 0,
        main_present_receipt_crc: 0,
        arcade_update_label: ArcadeUpdateTrace::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcade_overlay_copy_source_preserves_cached_and_published_ownership() {
        assert_eq!(
            arcade_overlay_copy_source(false, false),
            ArcadeOverlayCopySource::CachedLogical
        );
        assert_eq!(
            arcade_overlay_copy_source(true, true),
            ArcadeOverlayCopySource::PublishedPhysical
        );
        assert_eq!(
            arcade_overlay_copy_source(false, true),
            ArcadeOverlayCopySource::MissingRequiredPublication
        );
    }

    #[test]
    fn preview_layer_ownership_requires_the_complete_requested_copy() {
        let rect = DirtyRect {
            x0: 10,
            y0: 20,
            x1: 30,
            y1: 24,
        };

        assert!(
            require_complete_overlay_copy(PhysicalOverlayRole::Preview, 2, rect, 4, 17, 23, || {
                "preview-cache-5".into()
            },)
            .is_ok()
        );
        let error =
            require_complete_overlay_copy(PhysicalOverlayRole::Preview, 2, rect, 0, 17, 23, || {
                "preview-cache-5".into()
            })
            .unwrap_err();
        assert!(error.contains("role=preview slot=2"));
        assert!(error.contains("expected_rows=4 copied_rows=0"));
        assert!(error.contains("layout_generation=17 content_generation=23"));
        assert!(error.contains("backing_key=preview-cache-5"));
    }

    #[test]
    fn arcade_overlay_failure_preserves_role_and_source_cause() {
        let failure = PhysicalOverlayFailure {
            role: PhysicalOverlayRole::Arcade,
            slot_index: 1,
            rect: DirtyRect {
                x0: 4,
                y0: 8,
                x1: 44,
                y1: 28,
            },
            expected_rows: 20,
            copied_rows: 0,
            layout_generation: 31,
            content_generation: 47,
            backing_key: "arcade-ring-9".into(),
            cause: Some("physical Arcade full copy incomplete".into()),
        }
        .to_string();

        assert!(failure.contains("role=arcade slot=1"));
        assert!(failure.contains("layout_generation=31 content_generation=47"));
        assert!(failure.contains("backing_key=arcade-ring-9"));
        assert!(failure.contains("cause=physical Arcade full copy incomplete"));
    }

    #[test]
    fn unfinished_direct_frame_does_not_claim_a_latch_post() {
        let result = direct_hidden_waiting_present_result();
        assert_eq!(result.main_present_backend, LauncherPresentBackend::None);
        assert_eq!(result.main_present_status, LauncherPresentStatus::None);
        assert_eq!(result.main_present_sequence, 0);
        assert_eq!(result.main_present_copy_path, "none");
    }

    #[test]
    fn direct_hidden_slots_are_unavailable_on_fb0_and_frozen_routes() {
        assert!(!presenter_state_uses_latch::<FakeLatch>(
            &LauncherPresenterState::ExplicitFb0
        ));
        assert!(!presenter_state_uses_latch::<FakeLatch>(
            &LauncherPresenterState::Frozen {
                failure: LatchFailure::runtime(
                    mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
                    mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
                    "failed latch",
                ),
            }
        ));
        assert!(presenter_state_uses_latch(&LauncherPresenterState::Latch(
            FakeLatch
        )));
    }

    #[test]
    fn framebuffer_direct_hidden_geometry_accepts_half_resolution_hdmi() {
        let ui = UiDisplay::for_plan(UiDisplayPlan::from_runtime_geometry(
            RuntimeDisplayGeometry {
                output_w: 1920,
                output_h: 1080,
                scan_w: 1920,
                scan_h: 1080,
            },
            false,
        ));
        assert_eq!((ui.render_w(), ui.render_h()), (960, 540));
        assert_eq!(
            (usize::from(ui.scan_w()), usize::from(ui.scan_h())),
            (1920, 1080)
        );
        assert!(direct_hidden_framebuffer_geometry_available(&ui));
    }

    #[test]
    fn startup_intro_accepts_every_native_crt_route_without_enabling_screensavers() {
        let runtime = RuntimeDisplayGeometry {
            output_w: 1920,
            output_h: 1080,
            scan_w: 1920,
            scan_h: 1080,
        };
        for route in ["crt-240p60", "crt-288p50", "crt-480p60", "crt-576p50"] {
            let plan = if route == "crt-240p60" {
                UiDisplayPlan::from_geometry_with_route_and_composition(
                    crate::ui_display::ResolvedOutputRoute::Crt240p60
                        .progressive_geometry()
                        .expect("CRT240 geometry"),
                    crate::ui_display::ResolvedOutputRoute::Crt240p60,
                    "test-native-crt240",
                    crate::ui_display::UiFramebufferSizePolicy::Auto,
                    crate::ui_display::Crt240Composition::Native240,
                )
            } else {
                UiDisplayPlan::from_runtime_or_mister_ini_text(
                    Some(runtime),
                    "[Menu]\nvideo_mode=8\n",
                    Some(&format!("schema=1&output={route}")),
                    None,
                )
                .expect("supported CRT route")
            };
            let ui = UiDisplay::for_plan(plan);

            assert!(
                startup_intro_native_hidden_geometry_available(&ui),
                "{route}"
            );
            assert!(
                !direct_hidden_framebuffer_geometry_available(&ui),
                "{route}"
            );
        }
    }

    #[test]
    fn startup_intro_rejects_a_mismatched_native_crt_geometry() {
        let mut plan = UiDisplayPlan::from_runtime_or_mister_ini_text(
            None,
            "[Menu]\nvideo_mode=8\n",
            Some("schema=1&output=crt-240p60"),
            None,
        )
        .expect("supported CRT route");
        plan.fb_h = 241;
        let ui = UiDisplay::for_plan(plan);

        assert!(!startup_intro_native_hidden_geometry_available(&ui));
    }

    #[derive(Debug)]
    struct FakeLatch;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Event {
        Latch,
        Frozen,
        Fb0,
    }

    struct FakeAdapters {
        latch_result: Result<u8, LatchFailure>,
        events: Vec<Event>,
        latch_frames: Vec<LauncherFramePlan>,
        fb0_frames: Vec<LauncherFramePlan>,
    }

    impl FakeAdapters {
        fn succeeding() -> Self {
            Self {
                latch_result: Ok(1),
                events: Vec::new(),
                latch_frames: Vec::new(),
                fb0_frames: Vec::new(),
            }
        }

        fn failing() -> Self {
            Self {
                latch_result: Err(LatchFailure::runtime(
                    mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
                    mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
                    "failed latch",
                )),
                ..Self::succeeding()
            }
        }
    }

    impl PresentationAdapters<FakeLatch> for FakeAdapters {
        type Output = u8;

        fn present_latch(
            &mut self,
            _latch: &mut FakeLatch,
            frame: LauncherFramePlan,
        ) -> Result<Self::Output, LatchFailure> {
            self.events.push(Event::Latch);
            self.latch_frames.push(frame);
            self.latch_result.clone()
        }

        fn present_frozen(&mut self) -> Self::Output {
            self.events.push(Event::Frozen);
            3
        }

        fn present_fb0(&mut self, frame: LauncherFramePlan) -> Self::Output {
            self.events.push(Event::Fb0);
            self.fb0_frames.push(frame);
            2
        }
    }

    fn frame() -> LauncherFramePlan {
        LauncherFramePlan::from_cached_layers(
            DirtyRectList::from_one(DirtyRect {
                x0: 1,
                y0: 2,
                x1: 3,
                y1: 4,
            }),
            None,
            None,
            None,
            None,
        )
    }

    fn presenter(state: LauncherPresenterState<FakeLatch>) -> LauncherPresenter<FakeLatch> {
        let failure = match &state {
            LauncherPresenterState::Frozen { failure } => Some(failure.clone()),
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Latch(_) => None,
        };
        LauncherPresenter {
            state,
            failure_transitions: 0,
            first_failure: failure.clone(),
            latest_failure: failure.clone(),
            failure_history: failure.into_iter().collect(),
            retry_attempts: 0,
            auto_retry: LatchAutoRetryState::Disabled,
            next_retry_at: None,
            latest_retry_result: "not-attempted",
            recovery_state: "output-frozen",
            supervised_restart_requested: false,
        }
    }

    #[test]
    fn explicit_fb0_uses_only_fb0_adapter() {
        let mut presenter = presenter(LauncherPresenterState::ExplicitFb0);
        let mut adapters = FakeAdapters::succeeding();

        assert_eq!(presenter.present_with(frame(), &mut adapters), 2);
        assert_eq!(adapters.events, [Event::Fb0]);
        assert_eq!(presenter.pacing_backend(), LauncherPresentBackend::Fb0Dirty);
    }

    #[test]
    fn production_presenter_never_waits_directly_on_framebuffer_vsync() {
        let source = include_str!("orchestrator.rs");
        assert!(!source.contains(&["fb0", ".wait_vsync()"].concat()));
        assert!(!source.contains(&["fb0", ".wait_vsync_status()"].concat()));
    }

    #[test]
    fn latch_success_uses_only_latch_adapter() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut adapters = FakeAdapters::succeeding();

        assert_eq!(presenter.present_with(frame(), &mut adapters), 1);
        assert_eq!(adapters.events, [Event::Latch]);
        assert_eq!(
            presenter.pacing_backend(),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
    }

    #[test]
    fn startup_failure_freezes_without_presenting_fb0() {
        let mut presenter = presenter(LauncherPresenterState::Frozen {
            failure: LatchFailure::incompatible(
                mister_magik_fb::latch_readiness::LatchFailureStage::BufferMap,
                mister_magik_fb::latch_readiness::LatchFailureReason::ScanoutDeviceMissing,
                "missing",
            ),
        });
        let mut adapters = FakeAdapters::succeeding();

        assert_eq!(presenter.present_with(frame(), &mut adapters), 3);
        assert_eq!(adapters.events, [Event::Frozen]);
        assert_eq!(presenter.pacing_backend(), LauncherPresentBackend::None);
        assert!(adapters.fb0_frames.is_empty());
    }

    #[test]
    fn runtime_failure_freezes_last_good_instead_of_presenting_fb0() {
        let expected = frame();
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut adapters = FakeAdapters::failing();

        assert_eq!(presenter.present_with(expected.clone(), &mut adapters), 3);
        assert_eq!(adapters.events, [Event::Latch, Event::Frozen]);
        assert_eq!(adapters.latch_frames, [expected]);
        assert!(adapters.fb0_frames.is_empty());
        assert_eq!(presenter.pacing_backend(), LauncherPresentBackend::None);
        assert_eq!(
            presenter
                .latch_failure()
                .expect("preserved failure")
                .reason_code(),
            "latch-post-failed"
        );
        assert_eq!(presenter.failure_transitions(), 1);
    }

    #[test]
    fn frame_after_runtime_failure_remains_frozen() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut first = FakeAdapters::failing();
        presenter.present_with(frame(), &mut first);
        let mut second = FakeAdapters::succeeding();

        assert_eq!(presenter.present_with(frame(), &mut second), 3);
        assert_eq!(second.events, [Event::Frozen]);
        assert!(second.fb0_frames.is_empty());
    }

    #[test]
    fn successful_retry_restores_latch_without_clearing_failure_history() {
        let mut presenter = presenter(LauncherPresenterState::Frozen {
            failure: LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
                mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
                "first failure",
            ),
        });
        presenter.first_failure = presenter.latch_failure().cloned();
        presenter.latest_failure = presenter.first_failure.clone();

        assert!(presenter.apply_retry_result(Ok(FakeLatch)));
        assert!(!presenter.display_frozen());
        assert!(presenter.latch_failure().is_none());
        assert_eq!(
            presenter.first_failure.as_ref().unwrap().detail,
            "first failure"
        );
        assert_eq!(
            presenter.pacing_backend(),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
    }

    #[test]
    fn transient_failure_freezes_and_retries_after_250_ms() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut adapters = FakeAdapters::failing();

        assert_eq!(presenter.present_with(frame(), &mut adapters), 3);
        assert_eq!(adapters.events, [Event::Latch, Event::Frozen]);
        assert_eq!(presenter.auto_retry, LatchAutoRetryState::Ready);
        assert_eq!(presenter.retry_attempts, 0);

        let first_retry = presenter.next_retry_at.unwrap();
        assert!(!presenter.begin_automatic_retry(first_retry - Duration::from_millis(1)));
        assert!(presenter.begin_automatic_retry(first_retry));
        assert!(presenter.apply_retry_result(Ok(FakeLatch)));
        assert_eq!(presenter.retry_attempts, 1);
        assert_eq!(presenter.recovery_state, "recovered-automatically");
        assert!(!presenter.begin_automatic_retry(first_retry));
        assert_eq!(
            presenter.pacing_backend(),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
    }

    #[test]
    fn failed_automatic_retry_preserves_origin_and_schedules_one_second_backoff() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut adapters = FakeAdapters::failing();
        presenter.present_with(frame(), &mut adapters);
        let retry_failure = LatchFailure::runtime(
            mister_magik_fb::latch_readiness::LatchFailureStage::PostVerification,
            mister_magik_fb::latch_readiness::LatchFailureReason::PostedSequenceUnverified,
            "retry failed",
        );

        let first_retry = presenter.next_retry_at.unwrap();
        assert!(presenter.begin_automatic_retry(first_retry));
        assert!(!presenter.apply_retry_result(Err(retry_failure)));
        assert_eq!(
            presenter.first_failure.as_ref().unwrap().detail,
            "failed latch"
        );
        assert_eq!(
            presenter.latest_failure.as_ref().unwrap().detail,
            "retry failed"
        );
        assert_eq!(
            presenter
                .failure_history
                .iter()
                .map(|failure| failure.detail.as_str())
                .collect::<Vec<_>>(),
            ["failed latch", "retry failed"]
        );
        assert_eq!(presenter.retry_attempts, 1);
        assert_eq!(presenter.auto_retry, LatchAutoRetryState::Ready);

        let second_retry = presenter.next_retry_at.unwrap();
        assert!(!presenter.begin_automatic_retry(second_retry - Duration::from_millis(1)));
        assert!(presenter.begin_automatic_retry(second_retry));
        assert_eq!(presenter.retry_attempts, 2);
    }

    #[test]
    fn deterministic_and_platform_failures_do_not_auto_retry() {
        for failure in [
            LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::FrameCopy,
                mister_magik_fb::latch_readiness::LatchFailureReason::FrameCopyFailed,
                "copy failed",
            ),
            LatchFailure::incompatible(
                mister_magik_fb::latch_readiness::LatchFailureStage::ModuleLayout,
                mister_magik_fb::latch_readiness::LatchFailureReason::ScanoutLayoutMismatch,
                "layout mismatch",
            ),
        ] {
            let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
            presenter.transition_latch_failure(failure);
            assert!(presenter.display_frozen());
            assert_eq!(presenter.auto_retry, LatchAutoRetryState::Disabled);
            assert!(!presenter.begin_automatic_retry(Instant::now()));
        }
    }

    #[test]
    fn automatic_recovery_is_bounded_to_four_attempts() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        presenter.transition_latch_failure(LatchFailure::runtime(
            mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
            mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
            "origin",
        ));

        for attempt in 1..=MAX_AUTO_RETRY_ATTEMPTS {
            let deadline = presenter.next_retry_at.expect("scheduled retry");
            assert!(presenter.begin_automatic_retry(deadline));
            assert_eq!(presenter.retry_attempts, attempt);
            assert!(!presenter.apply_retry_result(Err(LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::RouteArm,
                mister_magik_fb::latch_readiness::LatchFailureReason::RouteArmFailed,
                format!("retry {attempt} failed"),
            )),));
        }

        assert_eq!(presenter.auto_retry, LatchAutoRetryState::Disabled);
        assert!(presenter.next_retry_at.is_none());
        assert_eq!(presenter.recovery_state, "terminal-failure");
        assert!(presenter.take_supervised_restart_request());
        assert!(!presenter.take_supervised_restart_request());
        assert_eq!(presenter.first_failure.as_ref().unwrap().detail, "origin");
        assert_eq!(
            presenter.latest_failure.as_ref().unwrap().detail,
            "retry 4 failed"
        );
        assert_eq!(presenter.failure_history.len(), 5);
    }

    #[test]
    fn retry_schedule_uses_250ms_1s_5s_and_60s_delays() {
        let mut presenter = presenter(LauncherPresenterState::Frozen {
            failure: LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
                mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
                "origin",
            ),
        });
        presenter.auto_retry = LatchAutoRetryState::Ready;
        let origin = Instant::now();
        for (index, delay) in LATCH_RETRY_DELAYS.into_iter().enumerate() {
            presenter.retry_attempts = index as u8;
            presenter.schedule_automatic_retry_at(origin);
            assert_eq!(presenter.next_retry_at, Some(origin + delay));
        }
    }

    #[test]
    fn physical_layer_row_spans_bound_each_changed_row() {
        let previous = [
            Rgb565Pixel(1),
            Rgb565Pixel(2),
            Rgb565Pixel(3),
            Rgb565Pixel(4),
            Rgb565Pixel(5),
            Rgb565Pixel(6),
            Rgb565Pixel(7),
            Rgb565Pixel(8),
        ];
        let current = [
            Rgb565Pixel(1),
            Rgb565Pixel(9),
            Rgb565Pixel(3),
            Rgb565Pixel(4),
            Rgb565Pixel(0),
            Rgb565Pixel(6),
            Rgb565Pixel(0),
            Rgb565Pixel(8),
        ];
        let mut spans = Vec::new();

        assert_eq!(
            collect_rgb565_row_spans(&current, &previous, 4, &mut spans),
            Some(4)
        );
        assert_eq!(spans, [(0, 1, 2), (1, 0, 3)]);
    }

    #[test]
    fn physical_layer_row_spans_reject_mismatched_mirrors() {
        let mut spans = vec![(9, 9, 9)];
        assert_eq!(
            collect_rgb565_row_spans(
                &[Rgb565Pixel(1), Rgb565Pixel(2)],
                &[Rgb565Pixel(1)],
                2,
                &mut spans,
            ),
            None
        );
    }
}
