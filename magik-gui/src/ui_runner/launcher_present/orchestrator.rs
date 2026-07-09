use super::super::*;
use crate::ui_runner::launcher_pacing::LauncherPacingTrace;
use mister_magik_fb::framebuffer::vsync::VsyncPace;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fb0PresentReason {
    Explicit,
    LatchUnavailable,
    RuntimeLatchFailure,
    RouteRecoveryRetry,
}

enum LauncherPresenterState<L> {
    ExplicitFb0,
    Latch(L),
    LatchUnavailable,
}

pub(in crate::ui_runner) struct LauncherPresenter<L = FpgaVblankLatchHiddenPresenter> {
    state: LauncherPresenterState<L>,
    failure_logged: bool,
    fb0_route_pending: bool,
}

pub(in crate::ui_runner) struct LauncherPresentFrame {
    pub(in crate::ui_runner) plan: LauncherFramePlan,
    pub(in crate::ui_runner) startup_can_present: bool,
    pub(in crate::ui_runner) first_visible_copy_done: bool,
    pub(in crate::ui_runner) frame_start_phase_us: u64,
    pub(in crate::ui_runner) pre_render_pace: Option<(VsyncPace, Instant, u128)>,
    pub(in crate::ui_runner) frame_analytics_mode: FrameAnalyticsMode,
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
    ) -> Result<Self::Output, String>;

    fn present_fb0(
        &mut self,
        frame: LauncherFramePlan,
        reason: Fb0PresentReason,
    ) -> Fb0AdapterOutput<Self::Output>;
}

struct Fb0AdapterOutput<T> {
    output: T,
    route_active: bool,
}

impl LauncherPresenter<FpgaVblankLatchHiddenPresenter> {
    pub(in crate::ui_runner) fn new(ui: &UiDisplay) -> Self {
        let state = match launcher_present_backend() {
            LauncherPresentBackend::FpgaVblankLatchHidden => {
                match FpgaVblankLatchHiddenPresenter::open(ui) {
                    Some(presenter) => LauncherPresenterState::Latch(presenter),
                    None => LauncherPresenterState::LatchUnavailable,
                }
            }
            LauncherPresentBackend::None | LauncherPresentBackend::Fb0Dirty => {
                LauncherPresenterState::ExplicitFb0
            }
        };
        Self {
            state,
            failure_logged: false,
            fb0_route_pending: false,
        }
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
        };
        if !frame.startup_can_present {
            return adapters.present_suppressed();
        }
        self.present_with(frame.plan, &mut adapters)
    }
}

impl<L> LauncherPresenter<L> {
    pub(in crate::ui_runner) fn pacing_backend(&self) -> LauncherPresentBackend {
        match self.state {
            LauncherPresenterState::Latch(_) => LauncherPresentBackend::FpgaVblankLatchHidden,
            LauncherPresenterState::ExplicitFb0 | LauncherPresenterState::LatchUnavailable => {
                LauncherPresentBackend::Fb0Dirty
            }
        }
    }

    pub(in crate::ui_runner) fn needs_frame(&self) -> bool {
        self.fb0_route_pending
    }

    fn present_with<A>(&mut self, frame: LauncherFramePlan, adapters: &mut A) -> A::Output
    where
        A: PresentationAdapters<L>,
    {
        let latch_error = match &mut self.state {
            LauncherPresenterState::ExplicitFb0 => {
                return adapters
                    .present_fb0(frame, Fb0PresentReason::Explicit)
                    .output;
            }
            LauncherPresenterState::LatchUnavailable => {
                let reason = if self.fb0_route_pending {
                    Fb0PresentReason::RouteRecoveryRetry
                } else {
                    Fb0PresentReason::LatchUnavailable
                };
                let result = adapters.present_fb0(frame, reason);
                if result.route_active {
                    self.fb0_route_pending = false;
                }
                return result.output;
            }
            LauncherPresenterState::Latch(latch) => match adapters.present_latch(latch, frame) {
                Ok(output) => return output,
                Err(error) => error,
            },
        };

        self.state = LauncherPresenterState::LatchUnavailable;
        self.fb0_route_pending = true;
        if !self.failure_logged {
            self.failure_logged = true;
            crate::ui_errln!(
                "fpga_vblank_latch_hidden_present_failed: {latch_error}; switching permanently to fb0-dirty"
            );
            boot_analytics::event(
                "fpga_vblank_latch_hidden_present_failed",
                format!("fallback=fb0-dirty error={latch_error}"),
            );
        }
        let result = adapters.present_fb0(frame, Fb0PresentReason::RuntimeLatchFailure);
        if result.route_active {
            self.fb0_route_pending = false;
        }
        result.output
    }
}

struct LivePresentationAdapters<'a, 'target> {
    targets: LauncherPresentTargets<'a, 'target>,
    display: &'a mut LauncherDisplaySession,
    first_visible_copy_done: bool,
    frame_start_phase_us: u64,
    pre_render_pace: Option<(VsyncPace, Instant, u128)>,
    frame_analytics_mode: FrameAnalyticsMode,
}

fn run_fb0_transaction<T>(
    frame: LauncherFramePlan,
    reason: Fb0PresentReason,
    full_rect: DirtyRect,
    present: impl FnOnce(LauncherFramePlan) -> T,
    activate_route: impl FnOnce() -> bool,
) -> Fb0AdapterOutput<T> {
    let frame = if reason == Fb0PresentReason::RuntimeLatchFailure {
        frame.for_fb0_recovery(full_rect)
    } else {
        frame
    };
    let output = present(frame);
    let restore_route = matches!(
        reason,
        Fb0PresentReason::RuntimeLatchFailure | Fb0PresentReason::RouteRecoveryRetry
    );
    let route_active = !restore_route || activate_route();
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
        let _ = self.targets.fb0.wait_vsync();
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
    ) -> Result<Self::Output, String> {
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

    fn present_fb0(
        &mut self,
        frame: LauncherFramePlan,
        reason: Fb0PresentReason,
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
            reason,
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: cached_frame.width(),
                y1: cached_frame.height(),
            },
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
                presentation: fb0_present_result(transaction.output),
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

fn fb0_present_result(stats: Fb0DirtyPresentStats) -> LauncherPresentResult {
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
        main_present_backend: LauncherPresentBackend::Fb0Dirty,
        main_present_status: LauncherPresentStatus::None,
        main_present_buffer: 0,
        main_present_hidden_copy_us: 0,
        main_present_hidden_invalid_bytes: 0,
        main_present_hidden_rect_count: 0,
        main_present_hidden_catchup_bytes: 0,
        main_present_hidden_full_copy: false,
        main_present_request_us: 0,
        main_present_set_vga_fb_us: 0,
        main_present_wait_us: 0,
        main_present_route_us: 0,
        arcade_update_label: stats.arcade_update_label,
    }
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
    let present_us =
        stats.copy_us + stats.post_us + stats.set_vga_fb_us + u128::from(stats.status_us);
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
        main_present_hidden_invalid_bytes: stats.invalid_bytes,
        main_present_hidden_rect_count: stats.rect_count,
        main_present_hidden_catchup_bytes: stats.catchup_bytes,
        main_present_hidden_full_copy: stats.full_copy,
        main_present_request_us: stats.post_us + stats.set_vga_fb_us,
        main_present_set_vga_fb_us: stats.set_vga_fb_us,
        main_present_wait_us: stats.status_us,
        main_present_route_us: u64::from(stats.flip_count),
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
        main_present_hidden_invalid_bytes: 0,
        main_present_hidden_rect_count: 0,
        main_present_hidden_catchup_bytes: 0,
        main_present_hidden_full_copy: false,
        main_present_request_us: 0,
        main_present_set_vga_fb_us: 0,
        main_present_wait_us: 0,
        main_present_route_us: 0,
        arcade_update_label: ArcadeUpdateTrace::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Debug)]
    struct FakeLatch;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Event {
        Latch,
        Fb0(Fb0PresentReason),
    }

    struct FakeAdapters {
        latch_result: Result<u8, String>,
        events: Vec<Event>,
        latch_frames: Vec<LauncherFramePlan>,
        fb0_frames: Vec<LauncherFramePlan>,
        route_active: bool,
    }

    impl FakeAdapters {
        fn succeeding() -> Self {
            Self {
                latch_result: Ok(1),
                events: Vec::new(),
                latch_frames: Vec::new(),
                fb0_frames: Vec::new(),
                route_active: true,
            }
        }

        fn failing() -> Self {
            Self {
                latch_result: Err("failed latch".to_string()),
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
        ) -> Result<Self::Output, String> {
            self.events.push(Event::Latch);
            self.latch_frames.push(frame);
            self.latch_result.clone()
        }

        fn present_fb0(
            &mut self,
            frame: LauncherFramePlan,
            reason: Fb0PresentReason,
        ) -> Fb0AdapterOutput<Self::Output> {
            self.events.push(Event::Fb0(reason));
            self.fb0_frames.push(frame);
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
        LauncherPresenter {
            state,
            failure_logged: false,
            fb0_route_pending: false,
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
    fn unavailable_latch_uses_fb0_without_retrying_latch() {
        let mut presenter = presenter(LauncherPresenterState::LatchUnavailable);
        let mut adapters = FakeAdapters::succeeding();

        assert_eq!(presenter.present_with(frame(), &mut adapters), 2);
        assert_eq!(
            adapters.events,
            [Event::Fb0(Fb0PresentReason::LatchUnavailable)]
        );
        assert_eq!(presenter.pacing_backend(), LauncherPresentBackend::Fb0Dirty);
    }

    #[test]
    fn runtime_failure_immediately_falls_back_with_same_frame() {
        let expected = frame();
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut adapters = FakeAdapters::failing();

        assert_eq!(presenter.present_with(expected, &mut adapters), 2);
        assert_eq!(
            adapters.events,
            [
                Event::Latch,
                Event::Fb0(Fb0PresentReason::RuntimeLatchFailure)
            ]
        );
        assert_eq!(adapters.latch_frames, [expected]);
        assert_eq!(adapters.fb0_frames, [expected]);
    }

    #[test]
    fn runtime_failure_marks_latch_unavailable() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut adapters = FakeAdapters::failing();

        presenter.present_with(frame(), &mut adapters);

        assert_eq!(presenter.pacing_backend(), LauncherPresentBackend::Fb0Dirty);
    }

    #[test]
    fn frame_after_runtime_failure_uses_fb0_pacing_and_skips_latch() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut first = FakeAdapters::failing();
        presenter.present_with(frame(), &mut first);
        let mut second = FakeAdapters::succeeding();

        assert_eq!(presenter.present_with(frame(), &mut second), 2);
        assert_eq!(
            second.events,
            [Event::Fb0(Fb0PresentReason::LatchUnavailable)]
        );
        assert_eq!(presenter.pacing_backend(), LauncherPresentBackend::Fb0Dirty);
    }

    #[test]
    fn failed_fb0_route_recovery_is_retried_without_retrying_latch() {
        let mut presenter = presenter(LauncherPresenterState::Latch(FakeLatch));
        let mut first = FakeAdapters::failing();
        first.route_active = false;
        presenter.present_with(frame(), &mut first);
        assert!(presenter.needs_frame());
        let mut second = FakeAdapters::succeeding();

        presenter.present_with(frame(), &mut second);

        assert_eq!(
            second.events,
            [Event::Fb0(Fb0PresentReason::RouteRecoveryRetry)]
        );
        assert_eq!(presenter.pacing_backend(), LauncherPresentBackend::Fb0Dirty);
        assert!(!presenter.needs_frame());
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
    fn live_fb0_fallback_restores_layers_before_route_and_retries_failed_route() {
        let full = DirtyRect {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        };
        let preview = DirtyRect {
            x0: 2,
            y0: 2,
            x1: 4,
            y1: 4,
        };
        let arcade = DirtyRect {
            x0: 6,
            y0: 6,
            x1: 8,
            y1: 8,
        };
        let logical_frame = LauncherFramePlan::new(
            DirtyRectList::from_one(DirtyRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            }),
            Some(DirectLayerState::new(preview, 1)),
            None,
            Some(DirectLayerState::new(arcade, 1)),
            None,
        );
        let pixels = vec![Rgb565Pixel(0); 100];
        let cached = CachedFrameView::new(&pixels, 10, 10);
        let mut target = UiFrameTarget::cached(FramebufferTargetGeometry::new(10, 10));
        target.direct_preview_565_rect_mut(preview);
        let direct_preview = target.direct_preview_view();
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut sink = TransactionSink {
            events: events.clone(),
        };

        let failed = run_fb0_transaction(
            logical_frame,
            Fb0PresentReason::RuntimeLatchFailure,
            full,
            |frame| Fb0DirtyPresenter::present_to(frame, cached, direct_preview, &mut sink),
            || {
                events.borrow_mut().push(TransactionEvent::RouteFailed);
                false
            },
        );

        assert!(!failed.route_active);
        let first_events = events.borrow().clone();
        let preview_index = first_events
            .iter()
            .position(|event| *event == TransactionEvent::Preview)
            .expect("preview copy");
        assert!(first_events[..preview_index]
            .iter()
            .all(|event| *event == TransactionEvent::Cached));
        assert_eq!(
            &first_events[preview_index..],
            &[
                TransactionEvent::Preview,
                TransactionEvent::Arcade,
                TransactionEvent::RouteFailed,
            ]
        );

        events.borrow_mut().clear();
        let retried = run_fb0_transaction(
            logical_frame,
            Fb0PresentReason::RouteRecoveryRetry,
            full,
            |frame| Fb0DirtyPresenter::present_to(frame, cached, direct_preview, &mut sink),
            || {
                events.borrow_mut().push(TransactionEvent::RouteOk);
                true
            },
        );

        assert!(retried.route_active);
        assert_eq!(events.borrow().last(), Some(&TransactionEvent::RouteOk));
        assert!(events
            .borrow()
            .iter()
            .take(events.borrow().len() - 1)
            .all(|event| *event == TransactionEvent::Cached));
    }
}
