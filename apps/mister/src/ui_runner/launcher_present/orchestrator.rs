// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::super::*;
use crate::ui_runner::launcher_pacing::LauncherPacingTrace;
use mister_magik_fb::framebuffer::vsync::VsyncPace;
use mister_magik_fb::latch_readiness::LatchFailure;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fb0PresentReason {
    Explicit,
    CompatibilityScreen,
}

enum LauncherPresenterState<L> {
    ExplicitFb0,
    Latch(L),
    Compatibility {
        failure: LatchFailure,
        screen_ready: bool,
        route_active: bool,
        prompt_visible: bool,
    },
}

fn presenter_state_uses_latch<L>(state: &LauncherPresenterState<L>) -> bool {
    matches!(state, LauncherPresenterState::Latch(_))
}

fn direct_hidden_framebuffer_geometry_available(ui: &UiDisplay) -> bool {
    ui.render_w() == ui.fb_w() && ui.render_h() == ui.fb_h() && !ui.output_route().is_crt()
}

fn direct_hidden_scan_geometry_available(ui: &UiDisplay) -> bool {
    direct_hidden_framebuffer_geometry_available(ui)
        && ui.render_w() == usize::from(ui.scan_w())
        && ui.render_h() == usize::from(ui.scan_h())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LatchAutoRetryState {
    Disabled,
    AwaitingSafeFrame,
    Ready,
    InProgress,
}

const LATCH_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_millis(250),
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(60),
];

pub(in crate::ui_runner) struct LauncherPresenter<L = FpgaVblankLatchHiddenPresenter> {
    state: LauncherPresenterState<L>,
    compatibility_transitions: u64,
    first_failure: Option<LatchFailure>,
    latest_failure: Option<LatchFailure>,
    retry_attempts: u8,
    auto_retry: LatchAutoRetryState,
    next_retry_at: Option<Instant>,
    latest_retry_result: &'static str,
    recovery_state: &'static str,
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

    fn present_compatibility_black(&mut self) -> Fb0AdapterOutput<Self::Output>;

    fn present_fb0(
        &mut self,
        frame: LauncherFramePlan,
        reason: Fb0PresentReason,
        activate_compatibility_route: bool,
    ) -> Fb0AdapterOutput<Self::Output>;
}

struct Fb0AdapterOutput<T> {
    output: T,
    route_active: bool,
}

impl LauncherPresenter<FpgaVblankLatchHiddenPresenter> {
    pub(in crate::ui_runner) fn new(ui: &UiDisplay) -> Self {
        let mut first_failure = None;
        let mut latest_failure = None;
        let mut auto_retry = LatchAutoRetryState::Disabled;
        let mut recovery_state = "not-needed";
        let state = match launcher_present_backend() {
            LauncherPresentBackend::FpgaVblankLatchHidden => {
                match FpgaVblankLatchHiddenPresenter::open(ui) {
                    Ok(presenter) => LauncherPresenterState::Latch(presenter),
                    Err(failure) => {
                        first_failure = Some(failure.clone());
                        latest_failure = Some(failure.clone());
                        if failure.is_transient_runtime_failure() {
                            auto_retry = LatchAutoRetryState::AwaitingSafeFrame;
                            recovery_state = "awaiting-safe-frame";
                        } else {
                            recovery_state = "compatibility-prompt";
                        }
                        crate::ui_errln!(
                            "latch_failure_tsv\tvalid=0\tstate={}\tstage={}\treason={}\taction=compatibility-screen\tdetail={}",
                            failure.state.code(),
                            failure.stage.code(),
                            failure.reason_code(),
                            failure.detail.replace(['\t', '\n', '\r'], " ")
                        );
                        LauncherPresenterState::Compatibility {
                            failure,
                            screen_ready: false,
                            route_active: false,
                            prompt_visible: auto_retry == LatchAutoRetryState::Disabled,
                        }
                    }
                }
            }
            LauncherPresentBackend::None
            | LauncherPresentBackend::Fb0Dirty
            | LauncherPresentBackend::CompatibilityFb0 => LauncherPresenterState::ExplicitFb0,
        };
        let compatibility_transitions = u64::from(matches!(
            state,
            LauncherPresenterState::Compatibility { .. }
        ));
        let presenter = Self {
            state,
            compatibility_transitions,
            first_failure,
            latest_failure,
            retry_attempts: 0,
            auto_retry,
            next_retry_at: None,
            latest_retry_result: "not-attempted",
            recovery_state,
        };
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
        };
        if !frame.startup_can_present {
            return adapters.present_suppressed();
        }
        self.present_with(frame.plan, &mut adapters)
    }

    pub(in crate::ui_runner) fn fail_latch_completion(&mut self, failure: LatchFailure) {
        self.transition_latch_failure(failure);
    }

    pub(in crate::ui_runner) fn retry_latch(&mut self, ui: &UiDisplay) -> bool {
        if !self.compatibility_prompt_visible() {
            return false;
        }
        self.retry_attempts = self.retry_attempts.saturating_add(1);
        self.latest_retry_result = "in-progress";
        self.recovery_state = "manual-retry";
        let result = FpgaVblankLatchHiddenPresenter::open(ui);
        self.apply_retry_result(result, false)
    }

    pub(in crate::ui_runner) fn retry_latch_automatically(&mut self, ui: &UiDisplay) -> bool {
        if !self.begin_automatic_retry(Instant::now()) {
            return false;
        }
        let result = FpgaVblankLatchHiddenPresenter::open(ui);
        self.apply_retry_result(result, true);
        true
    }

    pub(in crate::ui_runner) fn publish_stream_refinement_if_due(&self) -> bool {
        match &self.state {
            LauncherPresenterState::Latch(latch) => latch.publish_requested_full_snapshot(),
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Compatibility { .. } => {
                false
            }
        }
    }

    pub(in crate::ui_runner) fn direct_hidden_slots_available(&self, ui: &UiDisplay) -> bool {
        self.direct_hidden_framebuffer_slots_available(ui)
            && direct_hidden_scan_geometry_available(ui)
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

    pub(in crate::ui_runner) fn try_issue_hidden_slot_render_grant(
        &mut self,
        hardware: &mut Fpga,
        display: &mut LauncherDisplaySession,
    ) -> Result<Option<HiddenSlotRenderGrant>, LatchFailure> {
        match &mut self.state {
            LauncherPresenterState::Latch(latch) => {
                latch.try_issue_hidden_slot_render_grant(hardware, display)
            }
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Compatibility { .. } => {
                Ok(None)
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
            LauncherPresenterState::Compatibility { .. } => {
                LauncherPresentBackend::CompatibilityFb0
            }
        }
    }

    pub(in crate::ui_runner) fn needs_frame(&self) -> bool {
        matches!(
            self.state,
            LauncherPresenterState::Compatibility {
                screen_ready: false,
                ..
            }
        )
    }

    pub(in crate::ui_runner) fn compatibility_failure(&self) -> Option<&LatchFailure> {
        match &self.state {
            LauncherPresenterState::Compatibility { failure, .. } => Some(failure),
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Latch(_) => None,
        }
    }

    pub(in crate::ui_runner) fn compatibility_prompt_visible(&self) -> bool {
        matches!(
            self.state,
            LauncherPresenterState::Compatibility {
                prompt_visible: true,
                ..
            }
        )
    }

    pub(in crate::ui_runner) fn compatibility_blocks_app(&self) -> bool {
        matches!(self.state, LauncherPresenterState::Compatibility { .. })
            && (self.compatibility_prompt_visible()
                || self.auto_retry != LatchAutoRetryState::Disabled)
    }

    pub(in crate::ui_runner) fn continue_in_compatibility(&mut self) -> bool {
        let LauncherPresenterState::Compatibility { prompt_visible, .. } = &mut self.state else {
            return false;
        };
        let changed = *prompt_visible;
        *prompt_visible = false;
        if changed {
            self.auto_retry = LatchAutoRetryState::Disabled;
            self.next_retry_at = None;
            self.recovery_state = "continued-compatibility";
            self.persist_recovery_evidence();
        }
        changed
    }

    pub(in crate::ui_runner) fn compatibility_transitions(&self) -> u64 {
        self.compatibility_transitions
    }

    pub(in crate::ui_runner) fn retry_attempts(&self) -> u8 {
        self.retry_attempts
    }

    fn transition_latch_failure(&mut self, latch_error: LatchFailure) {
        crate::ui_errln!(
            "latch_failure_tsv\tvalid=0\tstate={}\tstage={}\treason={}\taction=compatibility-screen\tdetail={}",
            latch_error.state.code(),
            latch_error.stage.code(),
            latch_error.reason_code(),
            latch_error.detail.replace(['\t', '\n', '\r'], " ")
        );
        boot_analytics::event(
            "fpga_vblank_latch_compatibility",
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
        let automatic_retry = latch_error.is_transient_runtime_failure();
        self.auto_retry = if automatic_retry {
            LatchAutoRetryState::AwaitingSafeFrame
        } else {
            LatchAutoRetryState::Disabled
        };
        self.next_retry_at = None;
        self.recovery_state = if automatic_retry {
            "awaiting-safe-frame"
        } else {
            "compatibility-prompt"
        };
        self.compatibility_transitions = self.compatibility_transitions.saturating_add(1);
        self.state = LauncherPresenterState::Compatibility {
            failure: latch_error,
            screen_ready: false,
            route_active: false,
            prompt_visible: !automatic_retry,
        };
        self.persist_recovery_evidence();
    }

    fn begin_automatic_retry(&mut self, now: Instant) -> bool {
        if self.auto_retry != LatchAutoRetryState::Ready
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

    fn apply_retry_result(&mut self, result: Result<L, LatchFailure>, automatic: bool) -> bool {
        match result {
            Ok(latch) => {
                self.state = LauncherPresenterState::Latch(latch);
                self.auto_retry = LatchAutoRetryState::Disabled;
                self.next_retry_at = None;
                self.latest_retry_result = "success";
                self.recovery_state = if automatic {
                    "recovered-automatically"
                } else {
                    "recovered-manually"
                };
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

    fn mark_safe_frame_complete(&mut self) {
        self.mark_safe_frame_complete_at(Instant::now());
    }

    fn mark_safe_frame_complete_at(&mut self, now: Instant) {
        if self.auto_retry == LatchAutoRetryState::AwaitingSafeFrame {
            self.auto_retry = LatchAutoRetryState::Ready;
            let delay_index =
                usize::from(self.retry_attempts).min(LATCH_RETRY_DELAYS.len().saturating_sub(1));
            self.next_retry_at = Some(now + LATCH_RETRY_DELAYS[delay_index]);
            self.recovery_state = "safe-frame-complete";
            self.persist_recovery_evidence();
        }
    }

    fn persist_recovery_evidence(&self) {
        let (Some(first), Some(latest)) = (&self.first_failure, &self.latest_failure) else {
            return;
        };
        persist_latch_failure(
            first,
            latest,
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
                return adapters
                    .present_fb0(frame, Fb0PresentReason::Explicit, false)
                    .output;
            }
            LauncherPresenterState::Compatibility {
                screen_ready,
                route_active,
                ..
            } => {
                if !*screen_ready {
                    *screen_ready = true;
                    let result = adapters.present_compatibility_black();
                    *route_active = result.route_active;
                    self.mark_safe_frame_complete();
                    return result.output;
                }
                let result = adapters.present_fb0(
                    frame,
                    Fb0PresentReason::CompatibilityScreen,
                    !*route_active,
                );
                *route_active |= result.route_active;
                return result.output;
            }
            LauncherPresenterState::Latch(latch) => match adapters.present_latch(latch, frame) {
                Ok(output) => return output,
                Err(error) => error,
            },
        };

        self.transition_latch_failure(latch_error);
        let result = adapters.present_compatibility_black();
        if let LauncherPresenterState::Compatibility {
            screen_ready,
            route_active,
            ..
        } = &mut self.state
        {
            *screen_ready = true;
            *route_active = result.route_active;
        }
        self.mark_safe_frame_complete();
        result.output
    }
}

fn persist_latch_failure(
    first: &LatchFailure,
    latest: &LatchFailure,
    retry_attempts: u8,
    latest_retry_result: &str,
    recovery_state: &str,
) {
    let evidence = mister_magik_fb::latch_readiness::LatchFailureEvidence::for_recovery(
        first,
        latest,
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
}

fn run_fb0_transaction<T>(
    frame: LauncherFramePlan,
    activate_compatibility_route: bool,
    present: impl FnOnce(LauncherFramePlan) -> T,
    activate_route: impl FnOnce() -> bool,
) -> Fb0AdapterOutput<T> {
    let output = present(frame);
    let route_active = !activate_compatibility_route || activate_route();
    Fb0AdapterOutput {
        output,
        route_active,
    }
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
            let stats = latch.present_completed_hidden_frame(completed, hardware, self.display)?;
            if let Some(scale) = mister_magik_fb::framebuffer::stream::configured_latch_scale(
                self.stream_motion_active,
            ) {
                let frame_view = latch.committed_frame_view(stats.buffer_index);
                let _ =
                    mister_magik_fb::framebuffer::stream::publish_latch_snapshot(frame_view, scale);
            }
            let presentation =
                latch_present_result(stats, 0, 0, 0, PresentCopyStats::default(), None, None);
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
            layer_target.cached_frame_view(),
            frame,
            hardware,
            self.display,
            |hidden, plan| {
                preview_redraw_rect = plan.preview_redraw;
                arcade_redraw_update = plan.arcade_redraw;
                if let Some(rect) = plan.preview_redraw {
                    let started = Instant::now();
                    direct_preview_rows =
                        layer_target.copy_direct_preview_rect_to_hidden(hidden, rect);
                    hidden_preview_compose_us = started.elapsed().as_micros();
                }
                if let Some(update) = plan.arcade_redraw {
                    let started = Instant::now();
                    arcade_stats = layer_target.copy_arcade_list_update_to_hidden(
                        hidden,
                        arcade_list_renderer,
                        update,
                    );
                    hidden_arcade_compose_us = started.elapsed().as_micros();
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
        let presentation = latch_present_result(
            stats,
            hidden_preview_compose_us,
            hidden_arcade_compose_us,
            direct_preview_rows,
            arcade_stats,
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

    fn present_compatibility_black(&mut self) -> Fb0AdapterOutput<Self::Output> {
        let (_, frame_t3, cpu_t3, pacing_trace) = self.pace_before_fb0();
        self.targets.fb0.clear_black();
        let route_active = match self
            .display
            .activate_fb0_route_with_hardware(self.targets.hardware)
        {
            Ok(_) => true,
            Err(error) => {
                crate::ui_errln!("compatibility_fb0_route_failed: {error}");
                boot_analytics::event("compatibility_fb0_route_failed", error);
                false
            }
        };
        Fb0AdapterOutput {
            output: LauncherPresentCycle {
                presentation: compatibility_present_result(),
                frame_t3,
                frame_t4: Instant::now(),
                cpu_t3,
                cpu_t4: FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode),
                pacing_trace,
            },
            route_active,
        }
    }

    fn present_fb0(
        &mut self,
        frame: LauncherFramePlan,
        reason: Fb0PresentReason,
        activate_compatibility_route: bool,
    ) -> Fb0AdapterOutput<Self::Output> {
        let (_, frame_t3, cpu_t3, pacing_trace) = self.pace_before_fb0();
        let cached_frame = self.targets.layer_target.cached_frame_view();
        let direct_preview = self.targets.layer_target.direct_preview_view();
        let fb0 = &mut *self.targets.fb0;
        let arcade_list_renderer = &mut *self.targets.arcade_list_renderer;
        let display = &mut *self.display;
        let hardware = &mut *self.targets.hardware;
        let transaction = run_fb0_transaction(
            frame,
            activate_compatibility_route,
            |frame_plan| {
                Fb0DirtyPresenter::present(Fb0DirtyPresentRequest {
                    frame_plan,
                    cached_frame,
                    direct_preview,
                    fb0,
                    arcade_list_renderer,
                })
            },
            || match display.activate_fb0_route_with_hardware(hardware) {
                Ok(_) => true,
                Err(error) => {
                    static WARNED: OnceLock<()> = OnceLock::new();
                    WARNED.get_or_init(|| {
                        crate::ui_errln!(
                            "failed to activate fb0 route after latch failure: {error}; retrying"
                        );
                        boot_analytics::event(
                            "launcher_fb0_fallback_route_failed",
                            format!("error={error} retrying=1"),
                        );
                    });
                    false
                }
            },
        );
        Fb0AdapterOutput {
            output: LauncherPresentCycle {
                presentation: fb0_present_result(transaction.output, reason),
                frame_t3,
                frame_t4: Instant::now(),
                cpu_t3,
                cpu_t4: FrameAnalyticsCpuStamp::capture(self.frame_analytics_mode),
                pacing_trace,
            },
            route_active: transaction.route_active,
        }
    }
}

fn fb0_present_result(
    stats: Fb0DirtyPresentStats,
    reason: Fb0PresentReason,
) -> LauncherPresentResult {
    LauncherPresentResult {
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
        main_present_backend: if reason == Fb0PresentReason::CompatibilityScreen {
            LauncherPresentBackend::CompatibilityFb0
        } else {
            LauncherPresentBackend::Fb0Dirty
        },
        main_present_status: if reason == Fb0PresentReason::CompatibilityScreen {
            LauncherPresentStatus::Compatibility
        } else {
            LauncherPresentStatus::None
        },
        main_present_buffer: 0,
        main_present_hidden_copy_us: 0,
        main_present_hidden_publish_us: 0,
        main_present_hidden_invalid_bytes: 0,
        main_present_hidden_rect_count: 0,
        main_present_hidden_catchup_bytes: 0,
        main_present_hidden_full_copy: false,
        main_present_copy_path: "none",
        main_present_request_us: 0,
        main_present_set_vga_fb_us: 0,
        main_present_wait_us: 0,
        main_present_sequence: 0,
        main_present_flip_count: 0,
        main_present_drop_count: 0,
        arcade_update_label: stats.arcade_update_label,
    }
}

fn compatibility_present_result() -> LauncherPresentResult {
    let mut result = empty_present_result();
    result.main_present_backend = LauncherPresentBackend::CompatibilityFb0;
    result.main_present_status = LauncherPresentStatus::Compatibility;
    result
}

fn direct_hidden_waiting_present_result() -> LauncherPresentResult {
    empty_present_result()
}

fn latch_present_result(
    stats: FpgaVblankLatchHiddenPresentStats,
    hidden_preview_compose_us: u128,
    hidden_arcade_compose_us: u128,
    direct_preview_rows: u32,
    arcade_stats: PresentCopyStats,
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
        main_present_backend: LauncherPresentBackend::FpgaVblankLatchHidden,
        main_present_status: if stats.set_supported && stats.status_supported {
            LauncherPresentStatus::Ok
        } else {
            LauncherPresentStatus::Unsupported
        },
        main_present_buffer: stats.buffer_index,
        main_present_hidden_copy_us: stats.copy_us,
        main_present_hidden_publish_us: stats.publish_us,
        main_present_hidden_invalid_bytes: stats.invalid_bytes,
        main_present_hidden_rect_count: stats.rect_count,
        main_present_hidden_catchup_bytes: stats.catchup_bytes,
        main_present_hidden_full_copy: stats.full_copy,
        main_present_copy_path: stats.copy_path.label(),
        main_present_request_us: stats.post_us + stats.set_vga_fb_us,
        main_present_set_vga_fb_us: stats.set_vga_fb_us,
        main_present_wait_us: stats.status_us,
        main_present_sequence: stats.posted_sequence,
        main_present_flip_count: stats.flip_count,
        main_present_drop_count: stats.drop_count,
        arcade_update_label: ArcadeUpdateTrace::from_update(arcade_redraw_update.as_ref()),
    }
}

fn empty_present_result() -> LauncherPresentResult {
    LauncherPresentResult {
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
        main_present_backend: LauncherPresentBackend::None,
        main_present_status: LauncherPresentStatus::None,
        main_present_buffer: 0,
        main_present_hidden_copy_us: 0,
        main_present_hidden_publish_us: 0,
        main_present_hidden_invalid_bytes: 0,
        main_present_hidden_rect_count: 0,
        main_present_hidden_catchup_bytes: 0,
        main_present_hidden_full_copy: false,
        main_present_copy_path: "none",
        main_present_request_us: 0,
        main_present_set_vga_fb_us: 0,
        main_present_wait_us: 0,
        main_present_sequence: 0,
        main_present_flip_count: 0,
        main_present_drop_count: 0,
        arcade_update_label: ArcadeUpdateTrace::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn unfinished_direct_frame_does_not_claim_a_latch_post() {
        let result = direct_hidden_waiting_present_result();
        assert_eq!(result.main_present_backend, LauncherPresentBackend::None);
        assert_eq!(result.main_present_status, LauncherPresentStatus::None);
        assert_eq!(result.main_present_sequence, 0);
        assert_eq!(result.main_present_copy_path, "none");
    }

    #[test]
    fn direct_hidden_slots_are_unavailable_on_fb0_and_compatibility_routes() {
        assert!(!presenter_state_uses_latch::<FakeLatch>(
            &LauncherPresenterState::ExplicitFb0
        ));
        assert!(!presenter_state_uses_latch::<FakeLatch>(
            &LauncherPresenterState::Compatibility {
                failure: LatchFailure::runtime(
                    mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
                    mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
                    "failed latch",
                ),
                screen_ready: true,
                route_active: true,
                prompt_visible: false,
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
        assert!(!direct_hidden_scan_geometry_available(&ui));
    }

    #[derive(Debug)]
    struct FakeLatch;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Event {
        Latch,
        CompatibilityBlack,
        Fb0(Fb0PresentReason),
    }

    struct FakeAdapters {
        latch_result: Result<u8, LatchFailure>,
        events: Vec<Event>,
        latch_frames: Vec<LauncherFramePlan>,
        fb0_frames: Vec<LauncherFramePlan>,
        route_active: bool,
        route_activation_requests: Vec<bool>,
    }

    impl FakeAdapters {
        fn succeeding() -> Self {
            Self {
                latch_result: Ok(1),
                events: Vec::new(),
                latch_frames: Vec::new(),
                fb0_frames: Vec::new(),
                route_active: true,
                route_activation_requests: Vec::new(),
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

        fn present_compatibility_black(&mut self) -> Fb0AdapterOutput<Self::Output> {
            self.events.push(Event::CompatibilityBlack);
            Fb0AdapterOutput {
                output: 3,
                route_active: self.route_active,
            }
        }

        fn present_fb0(
            &mut self,
            frame: LauncherFramePlan,
            reason: Fb0PresentReason,
            activate_compatibility_route: bool,
        ) -> Fb0AdapterOutput<Self::Output> {
            self.events.push(Event::Fb0(reason));
            self.fb0_frames.push(frame);
            self.route_activation_requests
                .push(activate_compatibility_route);
            Fb0AdapterOutput {
                output: 2,
                route_active: self.route_active,
            }
        }
    }

    fn frame() -> LauncherFramePlan {
        LauncherFramePlan::new(
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
            LauncherPresenterState::Compatibility { failure, .. } => Some(failure.clone()),
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::Latch(_) => None,
        };
        LauncherPresenter {
            state,
            compatibility_transitions: 0,
            first_failure: failure.clone(),
            latest_failure: failure,
            retry_attempts: 0,
            auto_retry: LatchAutoRetryState::Disabled,
            next_retry_at: None,
            latest_retry_result: "not-attempted",
            recovery_state: "compatibility-prompt",
        }
    }

    #[test]
    fn explicit_fb0_uses_only_fb0_adapter() {
        let mut presenter = presenter(LauncherPresenterState::ExplicitFb0);
        let mut adapters = FakeAdapters::succeeding();

        assert_eq!(presenter.present_with(frame(), &mut adapters), 2);
        assert_eq!(adapters.events, [Event::Fb0(Fb0PresentReason::Explicit)]);
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
    fn startup_failure_blacks_before_presenting_compatibility_screen() {
        let mut presenter = presenter(LauncherPresenterState::Compatibility {
            failure: LatchFailure::incompatible(
                mister_magik_fb::latch_readiness::LatchFailureStage::BufferMap,
                mister_magik_fb::latch_readiness::LatchFailureReason::ScanoutDeviceMissing,
                "missing",
            ),
            screen_ready: false,
            route_active: false,
            prompt_visible: true,
        });
        let mut adapters = FakeAdapters::succeeding();

        assert_eq!(presenter.present_with(frame(), &mut adapters), 3);
        assert_eq!(adapters.events, [Event::CompatibilityBlack]);
        assert_eq!(
            presenter.pacing_backend(),
            LauncherPresentBackend::CompatibilityFb0
        );
    }

    #[test]
    fn runtime_failure_blacks_instead_of_presenting_launcher_through_fb0() {
        let expected = frame();
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut adapters = FakeAdapters::failing();

        assert_eq!(presenter.present_with(expected, &mut adapters), 3);
        assert_eq!(adapters.events, [Event::Latch, Event::CompatibilityBlack]);
        assert_eq!(adapters.latch_frames, [expected]);
        assert!(adapters.fb0_frames.is_empty());
    }

    #[test]
    fn runtime_failure_enters_structured_compatibility_state() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut adapters = FakeAdapters::failing();

        presenter.present_with(frame(), &mut adapters);

        assert_eq!(
            presenter.pacing_backend(),
            LauncherPresentBackend::CompatibilityFb0
        );
        assert_eq!(
            presenter
                .compatibility_failure()
                .expect("compatibility failure")
                .reason_code(),
            "latch-post-failed"
        );
        assert_eq!(presenter.compatibility_transitions(), 1);
    }

    #[test]
    fn frame_after_runtime_failure_renders_only_compatibility_screen() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut first = FakeAdapters::failing();
        presenter.present_with(frame(), &mut first);
        let mut second = FakeAdapters::succeeding();

        assert_eq!(presenter.present_with(frame(), &mut second), 2);
        assert_eq!(
            second.events,
            [Event::Fb0(Fb0PresentReason::CompatibilityScreen)]
        );
        assert_eq!(
            presenter.pacing_backend(),
            LauncherPresentBackend::CompatibilityFb0
        );
        assert_eq!(second.route_activation_requests, [false]);
    }

    #[test]
    fn compatibility_route_activation_retries_only_after_failure() {
        let mut presenter = presenter(LauncherPresenterState::Compatibility {
            failure: LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
                mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
                "failed latch",
            ),
            screen_ready: true,
            route_active: false,
            prompt_visible: true,
        });
        let mut failed_route = FakeAdapters::succeeding();
        failed_route.route_active = false;
        presenter.present_with(frame(), &mut failed_route);
        assert_eq!(failed_route.route_activation_requests, [true]);

        let mut recovered_route = FakeAdapters::succeeding();
        presenter.present_with(frame(), &mut recovered_route);
        assert_eq!(recovered_route.route_activation_requests, [true]);

        let mut stable_route = FakeAdapters::succeeding();
        presenter.present_with(frame(), &mut stable_route);
        assert_eq!(stable_route.route_activation_requests, [false]);
    }

    #[test]
    fn compatibility_continue_hides_prompt_without_discarding_failure() {
        let mut presenter = presenter(LauncherPresenterState::Compatibility {
            failure: LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
                mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
                "failed latch",
            ),
            screen_ready: true,
            route_active: true,
            prompt_visible: true,
        });

        assert!(presenter.continue_in_compatibility());
        assert!(!presenter.compatibility_prompt_visible());
        assert!(presenter.compatibility_failure().is_some());
        assert!(!presenter.continue_in_compatibility());
    }

    #[test]
    fn compatibility_retry_can_fail_repeatedly_then_restore_latch() {
        let mut presenter = presenter(LauncherPresenterState::Compatibility {
            failure: LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
                mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
                "first failure",
            ),
            screen_ready: true,
            route_active: true,
            prompt_visible: true,
        });
        let retry_failure = || {
            LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::PostVerification,
                mister_magik_fb::latch_readiness::LatchFailureReason::PostedSequenceUnverified,
                "retry failed",
            )
        };

        presenter.retry_attempts = 1;
        assert!(!presenter.apply_retry_result(Err(retry_failure()), false));
        presenter.retry_attempts = 2;
        assert!(!presenter.apply_retry_result(Err(retry_failure()), false));
        assert!(presenter.compatibility_prompt_visible());
        assert_eq!(presenter.compatibility_transitions(), 2);

        assert!(presenter.apply_retry_result(Ok(FakeLatch), false));
        assert!(!presenter.compatibility_prompt_visible());
        assert!(presenter.compatibility_failure().is_none());
        assert_eq!(
            presenter.pacing_backend(),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
    }

    #[test]
    fn transient_failure_waits_for_safe_frame_then_retries_on_schedule() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut adapters = FakeAdapters::failing();

        assert_eq!(presenter.present_with(frame(), &mut adapters), 3);
        assert_eq!(adapters.events, [Event::Latch, Event::CompatibilityBlack]);
        assert_eq!(presenter.auto_retry, LatchAutoRetryState::Ready);
        assert_eq!(presenter.retry_attempts, 0);

        let first_retry = presenter.next_retry_at.unwrap();
        assert!(!presenter.begin_automatic_retry(first_retry - Duration::from_millis(1)));
        assert!(presenter.begin_automatic_retry(first_retry));
        assert!(presenter.apply_retry_result(Ok(FakeLatch), true));
        assert_eq!(presenter.retry_attempts, 1);
        assert_eq!(presenter.recovery_state, "recovered-automatically");
        assert!(!presenter.begin_automatic_retry(first_retry));
        assert_eq!(
            presenter.pacing_backend(),
            LauncherPresentBackend::FpgaVblankLatchHidden
        );
    }

    #[test]
    fn failed_automatic_retry_preserves_origin_and_schedules_backoff() {
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
        assert!(!presenter.apply_retry_result(Err(retry_failure), true));
        assert_eq!(
            presenter.first_failure.as_ref().unwrap().detail,
            "failed latch"
        );
        assert_eq!(
            presenter.latest_failure.as_ref().unwrap().detail,
            "retry failed"
        );
        assert_eq!(presenter.retry_attempts, 1);
        assert!(!presenter.compatibility_prompt_visible());
        assert_eq!(presenter.auto_retry, LatchAutoRetryState::AwaitingSafeFrame);

        let safe_frame_at = first_retry + Duration::from_millis(10);
        presenter.mark_safe_frame_complete_at(safe_frame_at);
        let second_retry = safe_frame_at + Duration::from_secs(1);
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
            assert!(presenter.compatibility_prompt_visible());
            assert_eq!(presenter.auto_retry, LatchAutoRetryState::Disabled);
            assert!(!presenter.begin_automatic_retry(Instant::now()));
        }
    }

    #[test]
    fn transient_manual_retry_failure_rejoins_automatic_recovery() {
        let mut presenter = presenter(LauncherPresenterState::Compatibility {
            failure: LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::LatchPost,
                mister_magik_fb::latch_readiness::LatchFailureReason::LatchPostFailed,
                "origin",
            ),
            screen_ready: true,
            route_active: true,
            prompt_visible: true,
        });
        presenter.retry_attempts = 1;

        assert!(!presenter.apply_retry_result(
            Err(LatchFailure::runtime(
                mister_magik_fb::latch_readiness::LatchFailureStage::RouteArm,
                mister_magik_fb::latch_readiness::LatchFailureReason::RouteArmFailed,
                "manual retry failed",
            )),
            false,
        ));
        assert!(!presenter.compatibility_prompt_visible());
        assert_eq!(presenter.auto_retry, LatchAutoRetryState::AwaitingSafeFrame);
        let safe_frame_at = Instant::now();
        presenter.mark_safe_frame_complete_at(safe_frame_at);
        assert!(!presenter.begin_automatic_retry(safe_frame_at + Duration::from_millis(999)));
        assert!(presenter.begin_automatic_retry(safe_frame_at + Duration::from_secs(1)));
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TransactionEvent {
        Cached,
        Preview,
        Arcade,
        RouteFailed,
        RouteOk,
    }

    struct TransactionSink {
        events: Rc<RefCell<Vec<TransactionEvent>>>,
    }

    impl Fb0DirtyCopySink for TransactionSink {
        fn copy_cached(&mut self, _view: CachedFrameView<'_>, rect: DirtyRect) -> u32 {
            self.events.borrow_mut().push(TransactionEvent::Cached);
            rect.rows()
        }

        fn copy_direct_preview(&mut self, _view: DirectPreviewView<'_>, rect: DirtyRect) -> u32 {
            self.events.borrow_mut().push(TransactionEvent::Preview);
            rect.rows()
        }

        fn copy_arcade_list(&mut self, update: ArcadeListUpdate) -> PresentCopyStats {
            self.events.borrow_mut().push(TransactionEvent::Arcade);
            PresentCopyStats {
                rows: arcade_update_dirty_rect(&update).rows(),
                bytes: 1,
            }
        }
    }

    #[test]
    fn compatibility_screen_copy_completes_before_route_activation() {
        let logical_frame = LauncherFramePlan::new(
            DirtyRectList::from_one(DirtyRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            }),
            None,
            None,
            None,
            None,
        );
        let pixels = vec![Rgb565Pixel(0); 100];
        let cached = CachedFrameView::new(&pixels, 10, 10);
        let target = UiFrameTarget::cached(FramebufferTargetGeometry::new(10, 10));
        let direct_preview = target.direct_preview_view();
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut sink = TransactionSink {
            events: events.clone(),
        };

        let failed = run_fb0_transaction(
            logical_frame,
            true,
            |frame| Fb0DirtyPresenter::present_to(frame, cached, direct_preview, &mut sink),
            || {
                events.borrow_mut().push(TransactionEvent::RouteFailed);
                false
            },
        );

        assert!(!failed.route_active);
        let first_events = events.borrow().clone();
        assert_eq!(
            first_events,
            [TransactionEvent::Cached, TransactionEvent::RouteFailed]
        );

        events.borrow_mut().clear();
        let retried = run_fb0_transaction(
            logical_frame,
            true,
            |frame| Fb0DirtyPresenter::present_to(frame, cached, direct_preview, &mut sink),
            || {
                events.borrow_mut().push(TransactionEvent::RouteOk);
                true
            },
        );

        assert!(retried.route_active);
        assert_eq!(events.borrow().last(), Some(&TransactionEvent::RouteOk));
    }
}
