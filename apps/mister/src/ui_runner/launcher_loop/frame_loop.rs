// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! The launcher frame loop, owned as domain structs with the frame phases as methods.

use super::*;

/// The borrowed display, framebuffer and window handles the loop runs against.
pub(in crate::ui_runner) struct Env<'a> {
    pub(in crate::ui_runner) secs: u64,
    pub(in crate::ui_runner) ui: &'a UiDisplay,
    pub(in crate::ui_runner) disp: &'a mut MappedRgb565Framebuffer,
    pub(in crate::ui_runner) f: &'a mut Fpga,
    pub(in crate::ui_runner) display_session: &'a mut LauncherDisplaySession,
    pub(in crate::ui_runner) window: &'a Rc<MisterSoftwareWindow>,
    pub(in crate::ui_runner) target: &'a mut UiFrameTarget,
    pub(in crate::ui_runner) pad: PadPool,
    pub(in crate::ui_runner) app: slint_ui::launcher::Launcher,
    pub(in crate::ui_runner) animation_clock: &'a AnimationClock,
    pub(in crate::ui_runner) launcher_config:
        mister_magik_fb::process_config::LauncherProcessConfig,
}

/// The catalog, its workers, the preview and media sessions, and the launch lifecycle.
pub(super) struct Library {
    pub(super) scheduler: LauncherScheduler,
    pub(super) catalog_events: CatalogJobEventBuf,
    pub(super) deferred_catalog_events: VecDeque<CatalogWorkerMessage>,
    pub(super) pending_catalog_ready: Option<CatalogWorkerMessage>,
    pub(super) pending_collection_entry: Option<PendingCollectionEntry>,
    pub(super) deferred_navigation_hydration_finish: Option<String>,
    pub(super) catalog_ready_deferred_since: Option<Instant>,
    pub(super) catalog_ready_stationary_edge_since: Option<Instant>,
    pub(super) media_events: MediaJobEventBuf,
    pub(super) lifecycle_effects: LifecycleEffects,
    pub(super) preview_systems_entered: BTreeSet<String>,
    pub(super) preview_initial_lists_ready: BTreeSet<String>,
    pub(super) pending_system_entry_benchmark: Option<String>,
    pub(super) start_screen: Screen,
    pub(super) lock_screen: Option<Screen>,
    pub(super) launch_return_session: LaunchReturnSession,
    pub(super) pending_start_system: Option<String>,
    pub(super) loading_title: String,
    pub(super) preview: PreviewState,
    pub(super) catalog_version: usize,
    pub(super) user_state_session: UserStateSession,
    pub(super) user_state_catalog_version: Option<usize>,
    pub(super) arcade_root: String,
    pub(super) catalog: ArcadeCatalog,
    pub(super) catalog_ready: bool,
    pub(super) return_capsule_active: bool,
    pub(super) lifecycle: LauncherLifecycle,
    pub(super) catalog_session: LauncherCatalogSession,
    pub(super) media_session: ScreenshotMediaUpdateSession,
    pub(super) catalog_generation: CatalogGenerationState,
    pub(super) card_level: crate::launcher_home::CardLevelSnapshot,
    pub(super) card_prefetch_key: (String, usize),
    pub(super) arcade_screen_pending: bool,
    pub(super) update_check: UpdateCheck,
    pub(super) memory_guard: crate::memory_pressure::MemoryPressureGuard,
    pub(super) catalog_contention_quiet_previews: bool,
    pub(super) catalog_idle_candidate_since: Option<Instant>,
    pub(super) catalog_work_telemetry: CatalogWorkModeTelemetry,
    pub(super) background_maintenance_deferral: mister_magik_catalog::ui_motion::Deferral,
    pub(super) status_write_deferral: mister_magik_catalog::ui_motion::Deferral,
}

/// Navigation, the projected Slint models and their dirty flags.
pub(super) struct Ui {
    pub(super) preview_route: PreviewRoutePolicy,
    pub(super) nav: LauncherNav,
    pub(super) bridge_models: LauncherViewPresenters,
    pub(super) catalog_scan_blink: CatalogScanBlink,
    pub(super) navigation_source_bridge_sync_pending: bool,
}

/// Controller input, the UI action queue, setup, settings persistence and confirmations.
pub(super) struct Input {
    pub(super) launcher_ui_actions: LauncherUiActionsAdapter,
    pub(super) deferred_settings_activation: DeferredSettingsActivation,
    pub(super) settings_store: FileSettingsStore,
    pub(super) display_confirmation: DisplayConfirmation,
    pub(super) orientation_confirmation: OrientationConfirmation,
    pub(super) setup: SetupNav,
    pub(super) input_router: InputRouter,
    pub(super) setup_disconnect_notice: bool,
    pub(super) input_observation_probe: Option<crate::input_hub::InputObservationProbe>,
    pub(super) library_reset: LibraryResetState,
    pub(super) library_reset_bridge_dirty: bool,
    pub(super) last_clock_update: Instant,
    pub(super) last_clock_text: String,
    pub(super) latency_critical_input_pending: bool,
    pub(super) input_observation: crate::input_hub::InputObservation,
}

/// Composition, layers, the presenter, pacing and the frame clock.
pub(super) struct Output {
    pub(super) start: Instant,
    pub(super) frame_clock: mister_magik_core::frame_clock::FrameClock,
    pub(super) idle_slept_since: Option<Instant>,
    pub(super) frames: u64,
    pub(super) launcher_presenter: LauncherPresenter,
    pub(super) launcher_readiness: super::super::launcher_readiness::LauncherReadiness,
    pub(super) crt_layout: bool,
    pub(super) crt_metrics: CrtUiMetrics,
    pub(super) layout: UiLayoutGeometry,
    pub(super) layout_epoch: u64,
    pub(super) preview_compositor: Option<PreviewCompositor>,
    pub(super) preview_compositor_start_attempted: bool,
    pub(super) director: PresentationDirector,
    pub(super) orientation_full_redraw_pending: bool,
    pub(super) pacer: VsyncPacer,
    pub(super) pacing_policy: LauncherFramePacingPolicy,
    pub(super) phase_alignment: LauncherPhaseAlignment,
    pub(super) present_timing: PresentTiming,
    pub(super) arcade_list_renderer: ArcadeListRenderer,
    pub(super) crt_backdrop: Option<CrtBackdropController>,
    pub(super) crt_arcade_overlay: CrtArcadeOverlayState,
    pub(super) launcher_preview_version: u64,
    pub(super) launcher_arcade_version: u64,
    pub(super) launcher_arcade_scroll_offset: LayerOffset,
    pub(super) launcher_arcade_content_generation: u64,
    pub(super) launcher_preview_publication: Option<PhysicalLayerPublication>,
    pub(super) launcher_arcade_publication: Option<PhysicalLayerPublication>,
    pub(super) arcade_drawer_view_cache: ArcadeDrawerViewCache,
    pub(super) native_device_background: NativeDeviceBackground,
    pub(super) run_start: Instant,
    pub(super) last_home_pan_scroll_x: i32,
    pub(super) home_pan_present_until: Option<Instant>,
    pub(super) unpublished_cached_frame_present: bool,
}

/// The screensaver, the startup intro and the Home card session.
pub(super) struct Effects {
    pub(super) screensaver_preview_waits_for_analytics: bool,
    pub(super) screensaver: ScreensaverControl,
    pub(super) screensaver_pipeline: Option<ScreensaverRenderAhead>,
    pub(super) retiring_screensaver_pipelines: Vec<ScreensaverRenderAhead>,
    pub(super) screensaver_loader: Option<LauncherScreensaverLoader>,
    pub(super) screensaver_launcher_frame: Option<Vec<Rgb565Pixel>>,
    pub(super) screensaver_frame_visible: bool,
    pub(super) screensaver_active_cards: usize,
    pub(super) screensaver_render_sequence: u64,
    pub(super) screensaver_starvation_count: u64,
    pub(super) settings_cog_render_ahead: SettingsCogSession,
    pub(super) card_frame_rendered_last_iteration: bool,
    pub(super) launcher_card_home:
        Option<super::super::launcher_card_home::LauncherCardHomeSession>,
    pub(super) startup_intro: Option<StartupIntroSession>,
    pub(super) startup_intro_launcher_frame_ready: bool,
    pub(super) startup_intro_bridge_dirty_pending: bool,
    pub(super) startup_intro_catalog_ui_replay: Option<LauncherWorkerUiIntent>,
    pub(super) startup_intro_catalog_shells_pending: bool,
}

/// Profiling, accounting, tooling and benchmark hooks.
pub(super) struct Diagnostics {
    pub(super) ui_test_fixture: bool,
    pub(super) ui_action_sequence: u64,
    pub(super) startup_monotonic_us: u64,
    pub(super) profile_config: mister_magik_fb::process_config::ProfileProcessConfig,
    pub(super) orientation_preparation_trace: OrientationPreparationTrace,
    pub(super) input_integrity_stall: Option<u64>,
    pub(super) input_integrity_trace: InputIntegrityTrace,
    pub(super) launcher_response_trace: LauncherResponseTrace,
    pub(super) gui_profiling: GuiProfilingController,
    pub(super) bridge_churn_playback: BridgeChurnPlayback,
    pub(super) input_latency_lab: InputLatencyLab,
    pub(super) auto_launch_selected: bool,
    pub(super) auto_launch_selected_done: bool,
    pub(super) preview_transition: PreviewTransitionDemo,
    pub(super) cpu: Option<cpu_profile::CpuProfiler>,
    pub(super) system_entry_cpu_profile: Option<cpu_profile::CpuProfiler>,
    pub(super) screensaver_cpu_profile: cpu_profile::ScreensaverProfiler,
    pub(super) catalog_publication_test: CatalogPublicationTestDriver,
    pub(super) library_changed_dialog_test: LibraryChangedDialogTestDriver,
    pub(super) launcher_automation: LauncherAutomation,
    pub(super) modal_input_test_dialog_pending: bool,
    pub(super) auto_launch_gate: Option<PathBuf>,
    pub(super) modal_input_test_bridge_sync_pending: bool,
    #[cfg(feature = "ui-device-tests")]
    pub(super) _ui_test_sandbox: Option<UiTestSandbox>,
    pub(super) preview_scroll_exit_at: Option<Instant>,
    pub(super) first_render_logged: bool,
    pub(super) first_vsync_logged: bool,
    pub(super) first_launcher_frame_logged: bool,
    pub(super) frame_accounting: LauncherFrameAccounting,
    pub(super) arcade_entry_latency: ArcadeEntryLatencyTracker,
    #[cfg(feature = "tooling")]
    pub(super) tooling: Option<mister_magik_tooling_support::Session>,
    #[cfg(feature = "tooling")]
    pub(super) renderer_profile_requested: bool,
    #[cfg(feature = "tooling")]
    pub(super) tooling_carousel_release: Option<crate::input_event::InputEvent>,
    #[cfg(feature = "tooling")]
    pub(super) card_presentation_measurement_enabled: bool,
    #[cfg(feature = "tooling")]
    pub(super) tooling_drop_baseline: Option<ToolingPresentationObservation>,
    #[cfg(feature = "tooling")]
    pub(super) tooling_reject_baseline: Option<u16>,
    #[cfg(feature = "tooling")]
    pub(super) tooling_attempt_id: u64,
    #[cfg(feature = "tooling")]
    pub(super) tooling_input_epoch: u64,
    #[cfg(feature = "tooling")]
    pub(super) tooling_produced_id: u64,
}

/// The launcher frame loop: its state grouped by domain.
pub(super) struct FrameLoop<'a> {
    pub(super) env: Env<'a>,
    pub(super) lib: Library,
    pub(super) ui: Ui,
    pub(super) inp: Input,
    pub(super) out: Output,
    pub(super) fx: Effects,
    pub(super) diag: Diagnostics,
}

/// Why a phase ended the frame early.
enum Exit {
    /// Start the next frame.
    Skip,
}

/// Records when pre-input step `$index` ended, for the tooling frame evidence.
macro_rules! note_pre_input_boundary {
    ($evidence:expr, $run_start:expr, $index:expr) => {
        #[cfg(feature = "tooling")]
        if let Some(frame) = $evidence.as_mut()
            && frame.phases_enabled
        {
            frame.pre_input_boundaries_us[$index] = duration_us($run_start, Instant::now());
        }
    };
}

/// The catalog-side state, borrowed together for the worker-message handlers.
macro_rules! catalog_domain {
    ($frame:ident, $dirty:expr) => {
        CatalogDomain {
            nav: &mut $frame.ui.nav,
            catalog: &mut $frame.lib.catalog,
            catalog_ready: &mut $frame.lib.catalog_ready,
            catalog_version: &mut $frame.lib.catalog_version,
            return_capsule_active: &mut $frame.lib.return_capsule_active,
            catalog_generation: &mut $frame.lib.catalog_generation,
            launch_return_session: &mut $frame.lib.launch_return_session,
            preview: &mut $frame.lib.preview,
            scheduler: &mut $frame.lib.scheduler,
            catalog_session: &mut $frame.lib.catalog_session,
            lifecycle: &mut $frame.lib.lifecycle,
            lifecycle_effects: &mut $frame.lib.lifecycle_effects,
            full_bridge_dirty: &mut $dirty,
            startup_intro_catalog_ui_replay: &mut $frame.fx.startup_intro_catalog_ui_replay,
            startup_intro_catalog_shells_pending: &mut $frame
                .fx
                .startup_intro_catalog_shells_pending,
        }
    };
}

/// What `begin` hands to the later phases.
pub(super) struct BeginFrame {
    #[cfg(feature = "tooling")]
    pub(super) tooling_frame_begin: Instant,
    #[cfg(feature = "tooling")]
    pub(super) tooling_frame_evidence:
        Option<mister_magik_tooling_support::frame_evidence::FrameEvidence>,
    #[cfg(feature = "tooling")]
    pub(super) tooling_tick_us: u64,
}

/// What `pre_input` hands to the later phases.
pub(super) struct PreInputFrame {
    pub(super) scheduler_phase: LauncherResponseSchedulerBoundary,
    pub(super) loop_start: Instant,
    pub(super) animation_now: Instant,
    pub(super) animation_us: u64,
    pub(super) directional_input_held: bool,
    pub(super) background_work_allowed: bool,
    pub(super) full_bridge_dirty: bool,
    pub(super) frame_analytics_mode: FrameAnalyticsMode,
    pub(super) cpu_loop_start: FrameAnalyticsCpuStamp,
    pub(super) arcade_visual_index_at_loop_start: f32,
    pub(super) arcade_filter_visual_index_at_loop_start: f32,
    pub(super) prepare_trace_enabled: bool,
    pub(super) prepare_trace: LauncherPrepareTrace,
    pub(super) bridge_churn_frame_start: crate::launcher_presentation::BridgeChurnCounters,
    pub(super) effective_view: EffectiveLauncherView,
    pub(super) launching: bool,
    pub(super) setup_active: bool,
    pub(super) light_bridge_dirty: bool,
    pub(super) pad_changed_for_input: Option<bool>,
    pub(super) route_action: mister_magik_fb::framebuffer::ownership::FramebufferRouteAction,
    pub(super) defer_selected_preview: bool,
    pub(super) preview_scheduled_this_loop: bool,
    pub(super) clock_update_due: bool,
    pub(super) clock_update_us: u128,
    pub(super) slint_animation_active: bool,
    pub(super) media_message_seen: bool,
}

/// What `input` hands to the later phases.
pub(super) struct InputFrame {
    pub(super) input_phase_yielded: bool,
    pub(super) input_batch_empty: bool,
}

/// What `project` hands to the later phases.
pub(super) struct ProjectFrame {
    pub(super) startup_intro_prepare_live_launcher: bool,
    pub(super) startup_intro_suppress_launcher_ui: bool,
    pub(super) startup_reveal_suppress_launcher_ui: bool,
    pub(super) gui_bridge_phase: GuiBridgeProfilePhase,
    pub(super) response_projected_at_us: u64,
    pub(super) response_projected_execution: Option<ThreadExecutionStamp>,
    pub(super) catalog_scan_visible: bool,
    pub(super) catalog_scan_percent: i32,
    pub(super) catalog_background_scan_visible: bool,
    pub(super) confirm_visible: bool,
    pub(super) confirm_selected: i32,
    pub(super) status_write_due: bool,
    pub(super) status_text: Option<LauncherStatusTextSnapshot>,
    pub(super) status_string_copy_bytes: usize,
    pub(super) arcade_status_only: bool,
    pub(super) arcade_scroll_active: bool,
    pub(super) arcade_turbo_active: bool,
    pub(super) full_frame_present: bool,
    pub(super) wants_arcade_list: bool,
    pub(super) crt_backdrop_eligible: bool,
    pub(super) crt_backdrop_was_eligible: bool,
    pub(super) crt_backdrop_leaving: bool,
    pub(super) wants_preview_layer: bool,
    pub(super) wants_preview: bool,
    pub(super) preview_cache_state_before_composition: &'static str,
    pub(super) composition_decision: UiCompositionDecision,
    pub(super) composition_status: UiCompositionStatus,
    pub(super) automation_frame_stamp: AutomationFrameStamp,
    pub(super) native_device_base: bool,
    pub(super) custom_home_active: bool,
    pub(super) custom_home_scene_ready: bool,
    pub(super) custom_home_needs_render: bool,
    pub(super) home_pan_present_active: bool,
    pub(super) home_horizontal_input_held: bool,
    pub(super) stream_motion_before_render: bool,
    pub(super) wake_reasons: LauncherWakeReasons,
    pub(super) scheduled_frame_class: FrameProductionClass,
}

/// What `render` hands to the later phases.
pub(super) struct RenderFrame {
    pub(super) frame_start_phase_us: u64,
    pub(super) redraw_pending_for_trace: bool,
    pub(super) wake_reasons_bits: u64,
    pub(super) latch_backend_active: bool,
    pub(super) cpu_t0: FrameAnalyticsCpuStamp,
    pub(super) frame_t0: Instant,
    pub(super) prepare_us: u128,
    pub(super) pre_render_pace: Option<(
        mister_magik_fb::framebuffer::vsync::VsyncPace,
        Instant,
        u128,
    )>,
    pub(super) pre_render_wait_us: u128,
    pub(super) cpu_t1: FrameAnalyticsCpuStamp,
    pub(super) frame_t1: Instant,
    #[cfg(feature = "tooling")]
    pub(super) tooling_animation_active: bool,
    pub(super) frame_production_trace: FrameProductionTrace,
    pub(super) frame_production_completed_at: Option<Instant>,
    pub(super) screensaver_render_trace: ScreensaverRenderTrace,
    pub(super) accepted_screensaver_frame: bool,
    pub(super) screensaver_buffer_to_recycle_after_present: Option<Vec<Rgb565Pixel>>,
    pub(super) completed_hidden_frame_for_present: Option<CompletedHiddenFrame>,
    pub(super) card_direct_frame_rendered: bool,
    #[cfg(feature = "tooling")]
    pub(super) card_direct_measurement: Option<(u64, u64, u64, u64)>,
    #[cfg(feature = "tooling")]
    pub(super) card_work_timing: Option<mister_magik_tooling_support::measurement::FrameWorkTiming>,
    pub(super) accepted_startup_intro_frame: bool,
    pub(super) orientation_capture_source_carrier_rendered: bool,
    pub(super) card_direct_waiting_on_slot: bool,
    pub(super) full_screen_transition_release_raster_rendered: bool,
    pub(super) full_screen_transition_live_endpoint_rendered: bool,
    pub(super) gui_raster_phase: GuiRasterProfilePhase,
    pub(super) this_rect: Option<DirtyRect>,
    pub(super) frame_plan_pmu: Option<mister_magik_perf_events::SampledSpan>,
    pub(super) launcher_response_frame_stamp: Option<LauncherResponseFrameStamp>,
    pub(super) cpu_t2: FrameAnalyticsCpuStamp,
    pub(super) frame_t2: Instant,
    pub(super) cpu_custom_draw_start: FrameAnalyticsCpuStamp,
    pub(super) custom_draw_start: Instant,
    pub(super) arcade_list_rect: Option<PhysicalLayerUpdate>,
    pub(super) preview_transition_trace: PreviewTransitionTrace,
    pub(super) navigation_transition_composition_active: bool,
    pub(super) navigation_transition_frame_active: bool,
    #[cfg(feature = "tooling")]
    pub(super) navigation_transition_route: &'static str,
    #[cfg(feature = "tooling")]
    pub(super) navigation_transition_renderer: &'static str,
    pub(super) navigation_transition_frame_started: Option<Instant>,
    #[cfg(feature = "tooling")]
    pub(super) navigation_endpoint_rendered: bool,
    pub(super) custom_draw_trace: LauncherCustomDrawTrace,
    pub(super) cpu_custom_draw_done: FrameAnalyticsCpuStamp,
    pub(super) custom_draw_done: Instant,
    pub(super) raw_preview_direct_rect: Option<DirtyRect>,
    pub(super) preview_publication: Option<PhysicalLayerPublication>,
    pub(super) preview_desired: Option<PhysicalLayerState>,
    pub(super) arcade_publication: Option<PhysicalLayerPublication>,
    pub(super) arcade_desired: Option<PhysicalLayerState>,
    pub(super) cached_damage: DirtyRectList,
    pub(super) preview_presentation_commit: Option<crate::preview_state::PreviewPresentationCommit>,
}

impl<'a> FrameLoop<'a> {
    pub(super) fn new(
        env: Env<'a>,
        process_entry_cpu_profile: Option<cpu_profile::CpuProfiler>,
    ) -> Self {
        let Env {
            secs,
            ui,
            disp,
            f,
            display_session,
            window,
            target,
            mut pad,
            app,
            animation_clock,
            launcher_config,
        } = env;
        let startup::LoopState {
            launcher_ui_actions,
            start,
            frame_clock,
            idle_slept_since,
            ui_test_fixture,
            ui_action_sequence,
            startup_monotonic_us,
            frames,
            profile_config,
            screensaver_preview_waits_for_analytics,
            screensaver,
            screensaver_pipeline,
            retiring_screensaver_pipelines,
            screensaver_loader,
            screensaver_launcher_frame,
            screensaver_frame_visible,
            screensaver_active_cards,
            screensaver_render_sequence,
            screensaver_starvation_count,
            launcher_presenter,
            launcher_readiness,
            scheduler,
            catalog_events,
            deferred_catalog_events,
            pending_catalog_ready,
            pending_collection_entry,
            deferred_settings_activation,
            deferred_navigation_hydration_finish,
            catalog_ready_deferred_since,
            catalog_ready_stationary_edge_since,
            media_events,
            lifecycle_effects,
            preview_systems_entered,
            preview_initial_lists_ready,
            pending_system_entry_benchmark,
            start_screen,
            lock_screen,
            launch_return_session,
            arcade_catalog_required_at_start,
            pending_start_system,
            crt_layout,
            crt_metrics,
            preview_route,
            nav,
            // Named, not `_`: it must live to the end of the run, because dropping it
            // deletes the UI-test sandbox the loop is still writing to.
            #[cfg(feature = "ui-device-tests")]
                ui_test_sandbox: _ui_test_sandbox,
            settings_store,
            layout,
            layout_epoch,
            preview_compositor,
            preview_compositor_start_attempted,
            director,
            settings_cog_render_ahead,
            display_confirmation,
            orientation_confirmation,
            orientation_full_redraw_pending,
            orientation_preparation_trace,
            setup,
            input_router,
            setup_disconnect_notice,
            input_integrity_stall,
            input_integrity_trace,
            input_observation_probe,
            launcher_response_trace,
            gui_profiling,
            bridge_churn_playback,
            input_latency_lab,
            loading_title,
            library_reset,
            library_reset_bridge_dirty,
            last_clock_update,
            last_clock_text,
            auto_launch_selected,
            auto_launch_selected_done,
            pacer,
            pacing_policy,
            phase_alignment,
            present_timing,
            preview,
            preview_transition,
            arcade_list_renderer,
            crt_backdrop,
            crt_arcade_overlay,
            launcher_preview_version,
            launcher_arcade_version,
            launcher_arcade_scroll_offset,
            launcher_arcade_content_generation,
            launcher_preview_publication,
            launcher_arcade_publication,
            arcade_drawer_view_cache,
            cpu,
            system_entry_cpu_profile,
            screensaver_cpu_profile,
            bridge_models,
            native_device_background,
            catalog_version,
            user_state_session,
            user_state_catalog_version,
            arcade_root,
            catalog,
            catalog_ready,
            return_capsule_active,
            lifecycle,
            catalog_session,
            catalog_publication_test,
            media_session,
            library_changed_dialog_test,
            launcher_automation,
            catalog_generation,
            card_level,
            card_prefetch_key,
            card_frame_rendered_last_iteration,
            launcher_card_home,
            arcade_screen_pending,
            update_check,
            modal_input_test_dialog_pending,
            auto_launch_gate,
            modal_input_test_bridge_sync_pending,
            startup_intro,
            startup_intro_launcher_frame_ready,
            startup_intro_bridge_dirty_pending,
            startup_intro_catalog_ui_replay,
            startup_intro_catalog_shells_pending,
        } = startup::build_loop_state(
            secs,
            ui,
            window,
            &mut pad,
            &app,
            animation_clock,
            process_entry_cpu_profile,
            &launcher_config,
        );
        let run_start = if arcade_catalog_required_at_start
            && arcade_navigation_ready(catalog_ready, &catalog)
        {
            Instant::now()
        } else {
            start
        };
        let preview_scroll_exit_at = preview_scroll_exit_after_trace_deadline(run_start);
        let first_render_logged = false;
        let first_vsync_logged = false;
        let first_launcher_frame_logged = false;
        let mut frame_accounting = LauncherFrameAccounting::new(
            run_start,
            ui.output_route().label(),
            ui.crt_font_experiment().label(),
            ui.fb_w(),
            ui.fb_h(),
            profile_config.frame().fps_log_enabled(),
        );
        if let Some(failure) = launcher_presenter.latch_failure() {
            frame_accounting.record_latch_failure(failure);
        }
        let arcade_entry_latency =
            ArcadeEntryLatencyTracker::from_config(launcher_config.readiness().entry_trace());
        let memory_guard = crate::memory_pressure::MemoryPressureGuard::from_env();
        let catalog_contention_quiet_previews = matches!(
            std::env::var("MISTER_CATALOG_CONTENTION_QUIET_PREVIEWS")
                .ok()
                .as_deref(),
            Some("1") | Some("on") | Some("true") | Some("yes")
        );
        let last_home_pan_scroll_x = nav.scroll_x;
        let home_pan_present_until = None;
        let catalog_scan_blink = CatalogScanBlink::default();
        let navigation_source_bridge_sync_pending = false;
        let latency_critical_input_pending = false;
        let unpublished_cached_frame_present = false;
        let input_observation = input_observation_probe
            .as_ref()
            .map(crate::input_hub::InputObservationProbe::observe)
            .unwrap_or_default();
        let catalog_idle_candidate_since = None;
        let catalog_work_telemetry = CatalogWorkModeTelemetry::new(run_start);
        #[cfg(feature = "tooling")]
        let mut tooling = mister_magik_tooling_support::Session::from_environment();
        #[cfg(feature = "tooling")]
        let renderer_profile_requested = std::env::var_os("MISTER_MAGIK2_PROFILE_DIR").is_some();
        #[cfg(feature = "tooling")]
        let tooling_carousel_release: Option<crate::input_event::InputEvent> = None;
        // Grade artwork against the requested pose in ordinary measurements too.
        // Reusing an unchanged quantized pose is valid; CPU sampling is separate.
        #[cfg(feature = "tooling")]
        let card_presentation_measurement_enabled = tooling.is_some();
        #[cfg(feature = "tooling")]
        if let Some(session) = tooling.as_mut() {
            let paths = launcher_config.device_paths();
            let catalog = launcher_config.catalog_paths();
            session.metrics.render_timing_scope =
                Some("before-custom-draw; excludes custom drawing, latch post and completion");
            session.metrics.context = serde_json::json!({
                "data_root":paths.app_dir(), "main":paths.main_path(),
                "settings":paths.app_path("settings.json"), "controllers":paths.app_path("controllers.json"),
                "catalog":catalog.sharded_catalog_dir(), "library":catalog.library_sqlite(),
                "user_state":catalog.user_state_sqlite(), "assets":catalog.media_asset_dir(),
                "animation_clock": {"mode":"vsync-locked-v1", "period_ns":frame_clock.period().as_nanos()},
                "card_sampler": if cfg!(feature = "card-axis-filter") { "independent-vertical-prefilter" } else { "current" },
                "card_quantiser": if cfg!(feature = "card-fast-quantisation") { "centred-bayer-shifts" } else { "existing-bayer" },
                "native_device_plane": if !layout.is_portrait() && !ui.output_route().is_crt()
                    && (layout.logical_w(), layout.logical_h()) == (960, 540)
                    { "exposed-hdmi-v1" } else { "disabled" },
                "system_hub_axis": if layout.is_portrait() || ui.output_route().is_crt()
                    { "vertical" } else { "horizontal" },
                "card_helper_ahead": if !layout.is_portrait() && !ui.output_route().is_crt()
                    && (layout.logical_w(), layout.logical_h()) == (960, 540)
                    { "native-browse-tricks-v2" } else { "disabled" },
            });
            crate::ui_logln!("magik_context {}", session.metrics.context);
        }
        // Launcher-thread maintenance and status publication yield to UI motion.
        // Both are decided before this frame's motion is known, so they use the
        // signal published by the previous frame; one frame of lag is harmless.
        let background_maintenance_deferral = mister_magik_catalog::ui_motion::Deferral::default();
        let status_write_deferral = mister_magik_catalog::ui_motion::Deferral::default();
        #[cfg(feature = "tooling")]
        let tooling_drop_baseline: Option<
            super::launcher_frame_accounting::ToolingPresentationObservation,
        > = None;
        #[cfg(feature = "tooling")]
        let tooling_reject_baseline: Option<u16> = None;
        #[cfg(feature = "tooling")]
        let tooling_attempt_id = 0u64;
        #[cfg(feature = "tooling")]
        let tooling_input_epoch = 0u64;
        #[cfg(feature = "tooling")]
        let tooling_produced_id = 0u64;
        #[cfg(feature = "tooling")]
        crate::catalog_equivalence::start_requested_probe();
        super::phase_profile::set_budget_us(u32::try_from(pacer.period_us()).unwrap_or(u32::MAX));
        Self {
            env: Env {
                secs,
                ui,
                disp,
                f,
                display_session,
                window,
                target,
                pad,
                app,
                animation_clock,
                launcher_config,
            },
            lib: Library {
                scheduler,
                catalog_events,
                deferred_catalog_events,
                pending_catalog_ready,
                pending_collection_entry,
                deferred_navigation_hydration_finish,
                catalog_ready_deferred_since,
                catalog_ready_stationary_edge_since,
                media_events,
                lifecycle_effects,
                preview_systems_entered,
                preview_initial_lists_ready,
                pending_system_entry_benchmark,
                start_screen,
                lock_screen,
                launch_return_session,
                pending_start_system,
                loading_title,
                preview,
                catalog_version,
                user_state_session,
                user_state_catalog_version,
                arcade_root,
                catalog,
                catalog_ready,
                return_capsule_active,
                lifecycle,
                catalog_session,
                media_session,
                catalog_generation,
                card_level,
                card_prefetch_key,
                arcade_screen_pending,
                update_check,
                memory_guard,
                catalog_contention_quiet_previews,
                catalog_idle_candidate_since,
                catalog_work_telemetry,
                background_maintenance_deferral,
                status_write_deferral,
            },
            ui: Ui {
                preview_route,
                nav,
                bridge_models,
                catalog_scan_blink,
                navigation_source_bridge_sync_pending,
            },
            inp: Input {
                launcher_ui_actions,
                deferred_settings_activation,
                settings_store,
                display_confirmation,
                orientation_confirmation,
                setup,
                input_router,
                setup_disconnect_notice,
                input_observation_probe,
                library_reset,
                library_reset_bridge_dirty,
                last_clock_update,
                last_clock_text,
                latency_critical_input_pending,
                input_observation,
            },
            out: Output {
                start,
                frame_clock,
                idle_slept_since,
                frames,
                launcher_presenter,
                launcher_readiness,
                crt_layout,
                crt_metrics,
                layout,
                layout_epoch,
                preview_compositor,
                preview_compositor_start_attempted,
                director,
                orientation_full_redraw_pending,
                pacer,
                pacing_policy,
                phase_alignment,
                present_timing,
                arcade_list_renderer,
                crt_backdrop,
                crt_arcade_overlay,
                launcher_preview_version,
                launcher_arcade_version,
                launcher_arcade_scroll_offset,
                launcher_arcade_content_generation,
                launcher_preview_publication,
                launcher_arcade_publication,
                arcade_drawer_view_cache,
                native_device_background,
                run_start,
                last_home_pan_scroll_x,
                home_pan_present_until,
                unpublished_cached_frame_present,
            },
            fx: Effects {
                screensaver_preview_waits_for_analytics,
                screensaver,
                screensaver_pipeline,
                retiring_screensaver_pipelines,
                screensaver_loader,
                screensaver_launcher_frame,
                screensaver_frame_visible,
                screensaver_active_cards,
                screensaver_render_sequence,
                screensaver_starvation_count,
                settings_cog_render_ahead,
                card_frame_rendered_last_iteration,
                launcher_card_home,
                startup_intro,
                startup_intro_launcher_frame_ready,
                startup_intro_bridge_dirty_pending,
                startup_intro_catalog_ui_replay,
                startup_intro_catalog_shells_pending,
            },
            diag: Diagnostics {
                ui_test_fixture,
                ui_action_sequence,
                startup_monotonic_us,
                profile_config,
                orientation_preparation_trace,
                input_integrity_stall,
                input_integrity_trace,
                launcher_response_trace,
                gui_profiling,
                bridge_churn_playback,
                input_latency_lab,
                auto_launch_selected,
                auto_launch_selected_done,
                preview_transition,
                cpu,
                system_entry_cpu_profile,
                screensaver_cpu_profile,
                catalog_publication_test,
                library_changed_dialog_test,
                launcher_automation,
                modal_input_test_dialog_pending,
                auto_launch_gate,
                modal_input_test_bridge_sync_pending,
                #[cfg(feature = "ui-device-tests")]
                _ui_test_sandbox,
                preview_scroll_exit_at,
                first_render_logged,
                first_vsync_logged,
                first_launcher_frame_logged,
                frame_accounting,
                arcade_entry_latency,
                #[cfg(feature = "tooling")]
                tooling,
                #[cfg(feature = "tooling")]
                renderer_profile_requested,
                #[cfg(feature = "tooling")]
                tooling_carousel_release,
                #[cfg(feature = "tooling")]
                card_presentation_measurement_enabled,
                #[cfg(feature = "tooling")]
                tooling_drop_baseline,
                #[cfg(feature = "tooling")]
                tooling_reject_baseline,
                #[cfg(feature = "tooling")]
                tooling_attempt_id,
                #[cfg(feature = "tooling")]
                tooling_input_epoch,
                #[cfg(feature = "tooling")]
                tooling_produced_id,
            },
        }
    }

    pub(super) fn run(&mut self) {
        while self.keep_running() {
            match self.frame() {
                Err(Exit::Skip) | Ok(()) => {}
            }
        }
        if self.fx.startup_intro.take().is_some() {
            self.out
                .launcher_presenter
                .invalidate_external_hidden_mode();
        }
        // Preserve the continuous background permission for a later launcher run
        // in the same process (notably host tests and diagnostic runners).
        self.lib.catalog_work_telemetry.account(Instant::now());
        crate::ui_logln!(
            "catalog_work_mode_summary_tsv\ttransitions={}\tcpu0_us={}\tpaused_us={}\tburst_us={}",
            self.lib.catalog_work_telemetry.transitions,
            self.lib.catalog_work_telemetry.cpu0_us,
            self.lib.catalog_work_telemetry.paused_us,
            self.lib.catalog_work_telemetry.burst_us,
        );
        self.diag.frame_accounting.finish_preview_scroll_trace();
        let elapsed = self.out.run_start.elapsed().as_secs_f64();
        crate::ui_logln!(
            "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
            self.out.frames as f64 / elapsed,
            frames = self.out.frames
        );
        if let Err(e) = cpu_profile::finish(self.diag.cpu.take()) {
            crate::ui_errln!("{e}");
        }
        // Input processing has ended. Finish accepted writes without making a
        // setup action or normal frame wait for filesystem I/O.
        if let Err(error) = self
            .env
            .pad
            .shutdown_controller_saves(Duration::from_secs(2))
        {
            crate::ui_errln!("controller setup: shutdown save incomplete: {error}");
        }
    }

    /// Whether the loop should run another frame.
    fn keep_running(&self) -> bool {
        (self.env.secs == 0 || self.out.run_start.elapsed().as_secs() < self.env.secs)
            && self
                .diag
                .preview_scroll_exit_at
                .is_none_or(|deadline| Instant::now() < deadline)
    }

    /// One pass of the loop: each phase in order, stopping early when one ends the frame.
    fn frame(&mut self) -> Result<(), Exit> {
        let mut begin = self.begin()?;
        let mut pre_input = self.pre_input(&mut begin)?;
        let mut input = self.input(&mut begin, &mut pre_input)?;
        let mut project = self.project(&mut begin, &mut pre_input, &mut input)?;
        let render = self.render(&mut begin, &mut pre_input, input, &mut project)?;
        self.present(begin, pre_input, project, render)?;
        Ok(())
    }

    /// Opens the frame: tooling evidence, pending Slint callbacks and the tooling session tick.
    fn begin(&mut self) -> Result<BeginFrame, Exit> {
        #[cfg(feature = "tooling")]
        let tooling_frame_begin = Instant::now();
        #[cfg(feature = "tooling")]
        let mut tooling_frame_evidence = {
            self.diag.tooling_attempt_id = self.diag.tooling_attempt_id.wrapping_add(1);
            self.diag.tooling.as_ref().and_then(|session| {
                session.frame_evidence_candidate(
                    self.diag.tooling_attempt_id,
                    duration_us(self.out.run_start, tooling_frame_begin),
                )
            })
        };
        #[cfg(feature = "tooling")]
        if tooling_frame_evidence.is_some()
            && let Some(session) = self.diag.tooling.as_mut()
            && session
                .metrics
                .frame_evidence
                .needs_clock(duration_us(self.out.run_start, tooling_frame_begin))
        {
            let before = Instant::now();
            let monotonic = crate::input_hub::monotonic_us();
            let after = Instant::now();
            session.metrics.frame_evidence.note_clock([
                duration_us(self.out.run_start, before),
                monotonic,
                duration_us(self.out.run_start, after),
            ]);
        }
        #[cfg(feature = "tooling")]
        let mut tooling_tick_us = 0;
        #[cfg(feature = "tooling")]
        super::launcher_frame_accounting::capture_evidence_cpu(
            &mut tooling_frame_evidence,
            0,
            self.out.run_start,
        );
        record_launcher_frame_phase!(LauncherFramePhase::Begin);
        self.env.window.process_pending_callbacks();
        #[cfg(feature = "tooling")]
        if let Some(session) = self.diag.tooling.as_mut() {
            // Direct Arcade scrolling skips the full Slint presenter. Keep its
            // accessibility selection current for the attached tooling session.
            let arcade = self.env.app.global::<slint_ui::launcher::ArcadeView>();
            if self.ui.nav.screen == Screen::Arcade
                && arcade.get_selected_game_index() != self.ui.nav.arcade.selected as i32
            {
                arcade.set_selected_game_index(self.ui.nav.arcade.selected as i32);
            }
            if let Some(duration_us) = self.fx.launcher_card_home.as_mut().and_then(
                super::launcher_card_home::LauncherCardHomeSession::take_preparation_measurement,
            ) {
                let metrics = &mut session.metrics;
                metrics.counters.card_chrome_refreshes += 1;
                metrics.counters.card_prepare_us += duration_us;
                metrics.card_prepare_max_us = metrics.card_prepare_max_us.max(duration_us);
            }
            if self.diag.card_presentation_measurement_enabled {
                session.metrics.process_cpu_us = cpu_process_us();
            }
            let hub_axis = if self.out.layout.is_portrait() || self.env.ui.output_route().is_crt() {
                "vertical"
            } else {
                "horizontal"
            };
            if session.metrics.context["system_hub_axis"].as_str() != Some(hub_axis) {
                session.metrics.context["system_hub_axis"] = hub_axis.into();
            }
            session.set_ui_motion(mister_magik_catalog::ui_motion::active());
            let tooling_tick_start = Instant::now();
            let window_was_open =
                session.metrics.window_start.is_some() && session.metrics.window.is_none();
            if let Err(error) = session.tick(self.env.ui.render_w(), self.env.ui.render_h()) {
                session.metrics.error = Some(error);
            }
            let window_is_open =
                session.metrics.window_start.is_some() && session.metrics.window.is_none();
            if !window_was_open && window_is_open {
                super::phase_profile::begin_measurement();
            } else if window_was_open && !window_is_open {
                let report = super::phase_profile::end_measurement();
                if let Some(window_) = session.metrics.window.as_mut() {
                    window_["phase_profile"] =
                        serde_json::to_value(report).expect("phase profile JSON");
                }
                if let Err(error) =
                    session.publish_metrics(self.env.ui.render_w(), self.env.ui.render_h())
                {
                    session.metrics.error = Some(error);
                }
            }
            if self.diag.renderer_profile_requested && !window_was_open && window_is_open {
                let _ = mister_magik_framebuffer_scenes::launcher_profile::take();
                mister_magik_framebuffer_scenes::launcher_profile::enable_wall_time();
            } else if self.diag.renderer_profile_requested && window_was_open && !window_is_open {
                mister_magik_framebuffer_scenes::launcher_profile::disable();
                let report = mister_magik_framebuffer_scenes::launcher_profile::take();
                if let Some(window_) = session.metrics.window.as_mut() {
                    window_["renderer_profile"] =
                        serde_json::to_value(report).expect("renderer profile JSON");
                    if let Some(home) = self.fx.launcher_card_home.as_ref() {
                        window_["card_preparation"] = home.take_preparation_profile();
                    }
                    window_["renderer_profile_scope"] = serde_json::json!(
                        "summed primary/helper stage wall time; not elapsed critical path or stage CPU time"
                    );
                }
                if let Err(error) =
                    session.publish_metrics(self.env.ui.render_w(), self.env.ui.render_h())
                {
                    session.metrics.error = Some(error);
                }
            }
            tooling_tick_us = duration_us(tooling_tick_start, Instant::now());
            self.out.launcher_presenter.tooling_preview(session);
        }
        Ok(BeginFrame {
            #[cfg(feature = "tooling")]
            tooling_frame_begin,
            #[cfg(feature = "tooling")]
            tooling_frame_evidence,
            #[cfg(feature = "tooling")]
            tooling_tick_us,
        })
    }

    /// Everything that runs before input is read: timers, lifecycle, catalog and media workers, launch completion, pending starts, the benchmark hooks and the screensaver.
    fn pre_input(
        &mut self,
        #[cfg_attr(not(feature = "tooling"), allow(unused_variables, unused_mut))]
        begin: &mut BeginFrame,
    ) -> Result<PreInputFrame, Exit> {
        self.diag.gui_profiling.tick(Instant::now());
        let mut scheduler_phase = self.diag.launcher_response_trace.scheduler_boundary();
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 0);
        self.diag.screensaver_cpu_profile.poll(self.out.frames);
        if self
            .diag
            .catalog_publication_test
            .wait_for_first_frame_release(Instant::now(), self.out.start)
        {
            std::thread::sleep(Duration::from_millis(16));
            return Err(Exit::Skip);
        }
        let loop_start = Instant::now();
        // A launcher that slept with nothing to animate counts that sleep in
        // whole display periods so gaps and holds span it. Produced frames
        // always advance exactly one period (see `FrameClock`).
        if let Some(slept_since) = self.out.idle_slept_since.take() {
            self.out
                .frame_clock
                .advance_idle(loop_start.saturating_duration_since(slept_since));
        }
        let animation_now = self.out.frame_clock.now();
        let animation_us = self.out.frame_clock.elapsed_us();
        match self.inp.library_reset.poll(loop_start) {
            Ok(true) => {
                let _pace = self.out.pacer.wait();
                return Err(Exit::Skip);
            }
            Ok(false) => {}
            Err(error) => {
                crate::ui_errln!("library reset failed: {error}");
                self.lib.loading_title.clear();
                self.ui.nav.show_library_reset_error(error);
                self.inp.library_reset_bridge_dirty = true;
                self.env.window.request_redraw();
            }
        }
        let slint_timer_dispatch_started = Instant::now();
        let gui_timer_dispatch_pmu = self.diag.gui_profiling.span("gui.timer-dispatch");
        let full_screen_transition_policy_at_loop_start = self.out.director.chart.policy();
        let full_screen_transition_owned_at_loop_start = !self.out.director.chart.is_live();
        let current_pad_state = self.env.pad.state();
        let directional_input_held = current_pad_state.dpad_up
            || current_pad_state.dpad_down
            || current_pad_state.dpad_left
            || current_pad_state.dpad_right
            || self.diag.launcher_automation.directional_input_held();
        let input_pending_before_route = self
            .inp
            .input_observation_probe
            .as_ref()
            .is_some_and(|probe| probe.changed_since(self.inp.input_observation));
        let mut background_work_allowed = !input_pending_before_route
            && !should_defer_launcher_background_work(
                0,
                self.out.director.navigation.is_active(),
                self.out.director.orientation.is_active(),
                directional_input_held,
            )
            && !full_screen_transition_owned_at_loop_start
            && self.lib.background_maintenance_deferral.allows(loop_start);
        let startup_intro_needs_live_launcher = startup_intro_launcher_ui_plan(
            self.fx.startup_intro.is_some(),
            self.lib.lifecycle.startup_status().state,
            self.fx.startup_intro_launcher_frame_ready,
        ) == StartupIntroLauncherUiPlan::PrepareLiveFrame;
        if !input_pending_before_route
            && full_screen_transition_policy_at_loop_start.advance_slint_timers
            && (self.fx.startup_intro.is_none() || startup_intro_needs_live_launcher)
        {
            slint::platform::update_timers_and_animations();
        }
        let slint_timer_dispatch_us = slint_timer_dispatch_started.elapsed().as_micros();
        drop(gui_timer_dispatch_pmu);
        if self
            .inp
            .input_observation_probe
            .as_ref()
            .is_some_and(|probe| probe.changed_since(self.inp.input_observation))
        {
            background_work_allowed = false;
        }
        let mut full_bridge_dirty =
            std::mem::take(&mut self.ui.navigation_source_bridge_sync_pending)
                || std::mem::take(&mut self.diag.modal_input_test_bridge_sync_pending)
                || std::mem::take(&mut self.inp.library_reset_bridge_dirty);
        // The catalog-side locals, borrowed together for the worker-message handlers.
        if self.fx.startup_intro.is_none() {
            #[cfg(test)]
            if self.fx.startup_intro_catalog_shells_pending
                || self.fx.startup_intro_catalog_ui_replay.is_some()
            {
                record_launcher_frame_phase!(LauncherFramePhase::StartupCatalogReplay);
            }
            if std::mem::take(&mut self.fx.startup_intro_catalog_shells_pending) {
                self.lib.catalog = self
                    .ui
                    .nav
                    .catalog_with_build_shells(self.lib.catalog.clone());
                self.lib.catalog_version = self.lib.catalog_version.wrapping_add(1);
                self.ui.nav.sync_launcher_taxonomy(&self.lib.catalog);
                let _ = reapply_pending_launch_return_state(
                    &mut self.ui.nav,
                    &self.lib.catalog,
                    &mut self.lib.launch_return_session,
                );
                full_bridge_dirty = true;
            }
            if let Some(intent) = self.fx.startup_intro_catalog_ui_replay.take() {
                apply_launcher_worker_ui_intent(&self.env.app, intent, &mut full_bridge_dirty);
                self.env.window.request_redraw();
            }
        }
        let current_feedback_target =
            discrete_selection_feedback_target(&self.ui.nav, &self.inp.setup, &self.lib.lifecycle);
        if self
            .ui
            .bridge_models
            .sync_selection_feedback_surface(current_feedback_target.as_ref())
        {
            full_bridge_dirty = true;
            self.env.window.request_redraw();
        }
        if self
            .ui
            .bridge_models
            .expire_selection_feedback(animation_now)
        {
            full_bridge_dirty = true;
            self.env.window.request_redraw();
        }
        if let Some(collection_id) = self.lib.deferred_navigation_hydration_finish.take() {
            self.ui
                .nav
                .catalog_system_hydration_finished(&collection_id);
            full_bridge_dirty = true;
        }
        self.inp
            .display_confirmation
            .update_remaining(&mut self.ui.nav, loop_start);
        if self
            .inp
            .orientation_confirmation
            .update_remaining(&mut self.ui.nav, loop_start)
        {
            if let Some(previous) = self
                .inp
                .orientation_confirmation
                .finish_expired(&mut self.ui.nav)
            {
                let from = self.ui.nav.settings.screen_orientation;
                begin_orientation_transition(
                    &self.env.app,
                    self.env.window,
                    self.env.ui,
                    self.env.target,
                    from,
                    previous,
                    animation_now,
                    self.ui.nav.settings.reduce_motion,
                    &mut self.ui.nav,
                    &mut self.out.layout,
                    &mut self.out.layout_epoch,
                    &mut self.out.director,
                    &mut self.diag.orientation_preparation_trace,
                    OrientationIntent::Rollback,
                );
            }
            self.out.orientation_full_redraw_pending = true;
            full_bridge_dirty = true;
        }
        while let Some(result) = self.inp.orientation_confirmation.try_recv() {
            self.inp
                .orientation_confirmation
                .apply_result(&mut self.ui.nav, result);
            full_bridge_dirty = true;
            self.env.window.request_redraw();
        }
        while let Some(result) = self.inp.display_confirmation.try_recv() {
            self.out.pacer.rearm_after_display_mode_change();
            self.inp
                .display_confirmation
                .apply_result(&mut self.ui.nav, result, Instant::now());
            full_bridge_dirty = true;
            self.env.window.request_redraw();
        }
        scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-input-timers-feedback", scheduler_phase);
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 1);
        let frame_analytics_mode = self.diag.frame_accounting.frame_analytics_mode();
        let cpu_loop_start = FrameAnalyticsCpuStamp::capture(frame_analytics_mode);
        let arcade_visual_index_at_loop_start = self.ui.nav.arcade.visual_index;
        let arcade_filter_visual_index_at_loop_start = self.ui.nav.arcade_filter.visual_index;
        let prepare_trace_enabled = self.diag.frame_accounting.preview_scroll_trace_enabled()
            || frame_analytics_mode.records_wall();
        let mut prepare_trace = LauncherPrepareTrace {
            slint_timer_dispatch_us,
            ..LauncherPrepareTrace::default()
        };
        let bridge_churn_frame_start = crate::launcher_presentation::bridge_churn_snapshot();
        if background_work_allowed
            && self.lib.catalog_ready
            && self.lib.user_state_session.available()
            && self.lib.user_state_catalog_version != Some(self.lib.catalog_version)
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|duration| i64::try_from(duration.as_secs()).ok())
                .unwrap_or(0);
            if let Err(error) = self.lib.user_state_session.refresh(&self.lib.catalog, now) {
                crate::ui_errln!("user-state: {error}");
            }
            self.lib.user_state_catalog_version = Some(self.lib.catalog_version);
        }
        while background_work_allowed && let Some(event) = self.lib.user_state_session.poll() {
            match event {
                UserStateEvent::Snapshot { snapshot, .. } => {
                    if self
                        .ui
                        .nav
                        .set_user_state_snapshot(&self.lib.catalog, snapshot)
                    {
                        full_bridge_dirty = true;
                        self.env.window.request_redraw();
                    }
                }
                UserStateEvent::Failed {
                    error,
                    completed_favourite,
                } => {
                    crate::ui_errln!("user-state: {error}");
                    if completed_favourite.is_some() {
                        // One fresh read after a failed write/projection; a failed
                        // Refresh itself does not schedule another retry.
                        self.lib.user_state_catalog_version = None;
                    }
                }
                UserStateEvent::Unavailable { error } => {
                    crate::ui_errln!("user-state: {error}");
                }
            }
        }
        let return_was_waiting = self.lib.lifecycle.startup_status().mode
            == StartupMode::ReturnFromGame
            && !self.lib.lifecycle.startup_can_present_frame();
        self.lib.lifecycle.tick_startup_reveal(
            loop_start,
            startup_catalog_ready_for_reveal(
                self.fx.startup_intro.is_some(),
                self.lib.catalog_ready,
                self.lib.catalog_session.refresh_done(),
            ),
            &mut self.lib.lifecycle_effects,
        );
        if return_black_timeout_requires_home_fallback(
            return_was_waiting,
            &self.lib.lifecycle_effects,
        ) {
            self.lib
                .launch_return_session
                .fallback_to_home(&mut self.ui.nav);
            full_bridge_dirty = true;
            self.env.window.request_redraw();
        }
        apply_lifecycle_effects(
            &mut self.lib.lifecycle_effects,
            &mut self.lib.scheduler,
            self.out.start,
        );
        self.lib.scheduler.record_loading_frame(loop_start);
        if self
            .out
            .launcher_presenter
            .retry_latch_automatically(self.env.ui)
        {
            runtime_status::event(
                "launcher_latch_recovery",
                format!(
                    "action=automatic-retry attempt={}",
                    self.out.launcher_presenter.retry_attempts()
                ),
            );
            self.env.window.request_redraw();
        }
        if self
            .out
            .launcher_presenter
            .take_supervised_restart_request()
        {
            match launcher::request_supervised_launcher_restart() {
                Ok(()) => runtime_status::event(
                    "launcher_latch_recovery",
                    "action=supervised-restart-requested",
                ),
                Err(error) => runtime_status::event(
                    "launcher_latch_recovery",
                    format!("action=supervised-restart-failed error={error}"),
                ),
            }
        }
        self.diag
            .frame_accounting
            .set_display_frozen(self.out.launcher_presenter.display_frozen());
        let lifecycle_launch_active = matches!(
            self.lib.lifecycle.state(),
            LauncherLifecycleState::Launching { .. } | LauncherLifecycleState::Handoff { .. }
        );
        if self
            .lib
            .scheduler
            .recover_stale_launch_transport(lifecycle_launch_active)
        {
            runtime_status::event(
                "launcher_state_invariant_recovered",
                "kind=stale-launch-transport lifecycle=interactive",
            );
        }
        if lifecycle_launch_active && self.fx.screensaver.cancel_for_exclusive_view(loop_start) {
            runtime_status::event(
                "launcher_state_invariant_recovered",
                "kind=screensaver-during-launch action=cancel-screensaver",
            );
            self.env.window.request_redraw();
        }
        let mut effective_view = EffectiveLauncherView::resolve(
            &self.lib.lifecycle,
            self.fx.screensaver.active,
            self.ui.nav.screen,
        );
        let mut launching = effective_view.launch_active();
        let setup_active = self.inp.setup.is_active();
        let loop_elapsed_ms = loop_start
            .saturating_duration_since(self.out.start)
            .as_millis()
            .min(u64::MAX as u128) as u64;
        if self.lib.catalog_ready
            && self.lib.lifecycle.startup_input_enabled()
            && system_entry_benchmark_settled(
                loop_elapsed_ms,
                self.lib.lifecycle.startup_status().input_enabled_ms,
            )
            && effective_view.accepts_application_input()
            && self.ui.nav.screen == Screen::Home
            && self.lib.pending_collection_entry.is_none()
            && let Some(collection_id) = self.lib.pending_system_entry_benchmark.take()
        {
            let requested_at = Instant::now();
            mister_magik_perf_events::clear_process_profiles();
            self.diag.system_entry_cpu_profile =
                cpu_profile::start_system_entry(self.diag.profile_config.cpu());
            self.diag.arcade_entry_latency.capture_presentation_start(
                self.env.f.read_magik_presentation_telemetry().ok(),
                self.diag.frame_accounting.last_latch_drop_count(),
            );
            if collection_has_resident_rows(&self.lib.catalog, &collection_id) {
                self.diag
                    .arcade_entry_latency
                    .record_collection_enter_input(
                        self.out.start,
                        requested_at,
                        &self.lib.lifecycle,
                        &collection_id,
                        "benchmark-direct",
                        true,
                    );
                if self.ui.nav.open_system(&self.lib.catalog, &collection_id) {
                    if self.ui.nav.is_system_hub() {
                        self.ui.nav.set_arcade_user_list_mode(
                            &self.lib.catalog,
                            launcher::ArcadeUserListMode::Games,
                        );
                        self.ui.nav.system_page_mode = launcher::SystemPageMode::List;
                    }
                    self.diag.arcade_entry_latency.record_rows_ready(
                        self.out.start,
                        requested_at,
                        &self.lib.lifecycle,
                        &self.lib.catalog,
                        &self.ui.nav,
                    );
                    full_bridge_dirty = true;
                    self.env.window.request_redraw();
                }
            } else {
                let entry = begin_cold_collection_entry(
                    &mut self.lib.scheduler,
                    &mut self.ui.nav,
                    &mut self.lib.preview,
                    &self.lib.catalog,
                    self.lib.catalog_version,
                    &collection_id,
                    requested_at,
                    "benchmark-direct",
                    true,
                    &mut self.diag.arcade_entry_latency,
                    &self.lib.lifecycle,
                    self.out.start,
                );
                full_bridge_dirty |= entry.bridge_dirty;
                self.lib.pending_collection_entry = entry.pending;
            }
        }
        scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-input-lifecycle-state", scheduler_phase);
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 2);
        let mut light_bridge_dirty = false;
        let pad_changed_for_input = if effective_view.accepts_application_input()
            && self.lib.lifecycle.startup_input_enabled()
        {
            Some(self.env.pad.poll_with_debug_labels(setup_active))
        } else {
            None
        };
        scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-input-raw-device-poll", scheduler_phase);
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 3);
        if background_work_allowed
            && let Some(sample) = self.lib.memory_guard.tick(loop_start)
            && sample.changed
        {
            runtime_status::event(
                "memory_pressure",
                format!(
                    "active={} available_kib={} threshold_kib={}",
                    u8::from(sample.active),
                    sample.available_kib,
                    sample.threshold_kib
                ),
            );
            if sample.active {
                let bridge = self.env.app.global::<slint_ui::launcher::ArcadeView>();
                self.lib.preview.clear(&bridge);
                apply_screenshot_media_update_effects(
                    self.lib.media_session.pause_for_low_memory(),
                    &self.env.app,
                    &mut self.lib.catalog,
                    &mut self.lib.scheduler,
                    Some(&mut self.lib.preview),
                    &mut full_bridge_dirty,
                    self.out.start,
                );
                full_bridge_dirty = true;
            }
        }
        if background_work_allowed {
            apply_screenshot_media_update_effects(
                self.lib.media_session.clear_progress_if_due(loop_start),
                &self.env.app,
                &mut self.lib.catalog,
                &mut self.lib.scheduler,
                Some(&mut self.lib.preview),
                &mut full_bridge_dirty,
                self.out.start,
            );
        }
        self.out.launcher_readiness.poll();
        let mut route_action =
            self.env
                .display_session
                .begin_frame(self.out.frames, launching, self.env.f);
        route_action.force_full_present |= self.out.launcher_readiness.needs_full_present();
        // The catalog contention harness first proves one exact preview, then
        // freezes further selected-preview work so frame failures can be
        // attributed to the catalog rather than an independent image decode.
        let defer_selected_preview = self.lib.catalog_contention_quiet_previews
            && self.lib.preview.trace_cache_state() == "exact";
        let preview_scheduled_this_loop = false;
        #[cfg(feature = "tooling")]
        let forced_clock = self
            .diag
            .tooling
            .as_mut()
            .and_then(|session| session.launcher_clock());
        #[cfg(not(feature = "tooling"))]
        let forced_clock: Option<&str> = None;
        let clock_update_due = forced_clock.is_some_and(|clock| clock != self.inp.last_clock_text)
            || background_work_allowed
                && self.inp.last_clock_update.elapsed() >= Duration::from_secs(1);
        let clock_update_start = clock_update_due.then(Instant::now);
        if clock_update_due {
            if self.fx.startup_intro.is_some() {
                self.fx.startup_intro_bridge_dirty_pending = true;
            } else {
                let clock_text = forced_clock
                    .map(str::to_owned)
                    .unwrap_or_else(launcher_clock_text);
                if clock_text != self.inp.last_clock_text {
                    set_launcher_clock_text(&self.env.app, &clock_text);
                    self.inp.last_clock_text = clock_text;
                    light_bridge_dirty = true;
                }
            }
            self.inp.last_clock_update = Instant::now();
        }
        let clock_update_us = clock_update_start
            .map(|started| started.elapsed().as_micros())
            .unwrap_or(0);
        if background_work_allowed
            && let Some(available) = self.lib.update_check.try_recv()
            && available
        {
            set_launcher_update_available(&self.env.app, true);
            light_bridge_dirty = true;
            runtime_status::event("update_available", "source=downloader_mister_magik");
        }

        if self
            .inp
            .input_observation_probe
            .as_ref()
            .is_some_and(|probe| probe.changed_since(self.inp.input_observation))
        {
            background_work_allowed = false;
        }

        scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-input-readiness-maintenance", scheduler_phase);
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 4);

        let catalog_worker_trace_start = prepare_trace_enabled.then(Instant::now);
        let slint_animation_active = self.env.app.window().has_active_animations();
        let startup_return_waiting_for_catalog =
            self.lib.lifecycle.startup_waiting_for_return_catalog();
        if self.lib.scheduler.system_entry_prepare_active() {
            background_work_allowed = false;
        }
        let catalog_interaction_active = self.lib.scheduler.system_entry_prepare_active()
            || !background_work_allowed
            || directional_input_held
            || self.inp.latency_critical_input_pending
            || self.out.director.navigation.is_active()
            || self.out.director.orientation.is_active()
            || full_screen_transition_owned_at_loop_start
            || self.ui.nav.arcade.is_scroll_active()
            || (self.ui.nav.arcade_filter.drawer_open
                && self.ui.nav.arcade_filter.is_scroll_active());
        let catalog_work_mode = launcher_catalog_work_mode(
            self.diag.frame_accounting.first_visible_copy_done(),
            catalog_interaction_active,
            self.fx.startup_intro.is_some() || slint_animation_active,
            loop_start,
            &mut self.lib.catalog_idle_candidate_since,
        );
        if self
            .lib
            .catalog_work_telemetry
            .observe(catalog_work_mode, loop_start)
        {
            crate::ui_logln!(
                "catalog_work_mode_tsv\tmode={:?}\tinteraction={}\tvisible_animation={}",
                catalog_work_mode,
                u8::from(catalog_interaction_active),
                u8::from(self.fx.startup_intro.is_some() || slint_animation_active),
            );
        }
        let catalog_worker_work_allowed = catalog_work_mode != CatalogWorkMode::Paused;
        self.lib
            .scheduler
            .tick_catalog_progress(catalog_worker_work_allowed, loop_start);
        if background_work_allowed
            && let Some(request) = self
                .ui
                .nav
                .take_arcade_search_request(&self.lib.catalog, self.lib.catalog_version)
        {
            self.lib.scheduler.request_arcade_search(request);
        }
        let deferred_worker_policy = deferred_catalog_worker_start_policy(
            self.lib.catalog_ready,
            self.diag.frame_accounting.first_visible_copy_done(),
            startup_return_waiting_for_catalog,
            self.lib.lifecycle.startup_waiting_for_initial_catalog(),
            self.lib
                .lifecycle
                .catalog_worker_start_delay(catalog_background_validation_delay()),
        );
        if background_work_allowed
            && let Some(worker) = self.lib.catalog_session.maybe_start_deferred_worker(
                self.lib.scheduler.catalog_worker_running(),
                self.diag.frame_accounting.first_visible_copy_done()
                    || startup_return_waiting_for_catalog
                    || self.lib.lifecycle.startup_waiting_for_initial_catalog(),
                deferred_worker_policy.allowed
                    && self.diag.catalog_publication_test.catalog_worker_allowed(),
                loop_start,
                deferred_worker_policy.delay,
            )
        {
            print_startup_event(self.out.start, "catalog_worker_start", &worker.root);
            let lifecycle_input =
                deferred_catalog_worker_lifecycle_input(worker.execution_mode, worker.request);
            self.lib
                .lifecycle
                .handle(lifecycle_input, &mut self.lib.lifecycle_effects);
            apply_lifecycle_effects(
                &mut self.lib.lifecycle_effects,
                &mut self.lib.scheduler,
                self.out.start,
            );
            self.lib.scheduler.start_catalog_worker(
                worker.root,
                worker.request,
                worker.initial_cache,
                worker.execution_mode,
            );
        }

        if background_work_allowed
            && let Some(message) = self
                .diag
                .catalog_publication_test
                .tick(loop_start, self.out.start)
        {
            self.lib.deferred_catalog_events.push_back(message);
        }
        let system_entry_handoff_only = should_poll_system_entry_handoff(
            background_work_allowed,
            self.lib.pending_collection_entry.is_some(),
            self.lib
                .launch_return_session
                .protects_hydrating_collection(&self.ui.nav),
            self.lib.scheduler.system_entry_prepare_active(),
        );
        let catalog_poll_scope = catalog_poll_scope(
            background_work_allowed,
            full_screen_transition_owned_at_loop_start,
            system_entry_handoff_only,
        );
        if let Some(catalog_poll_scope) = catalog_poll_scope
            && catalog_messages_need_polling(
                self.lib.pending_catalog_ready.is_some(),
                self.lib.catalog_session.refresh_done(),
                self.lib.scheduler.catalog_messages_running()
                    || !self.lib.deferred_catalog_events.is_empty(),
            )
        {
            let _catalog_disconnected = self
                .lib
                .scheduler
                .poll_catalog(&mut self.lib.catalog_events, catalog_poll_scope);
            self.lib
                .deferred_catalog_events
                .extend(self.lib.catalog_events.drain());

            let mut catalog_messages_processed = 0usize;
            if let Some(message) = self.lib.pending_catalog_ready.take() {
                self.lib.catalog_ready_stationary_edge_since =
                    update_catalog_ready_stationary_edge_since(
                        &self.ui.nav,
                        self.lib.catalog_ready_stationary_edge_since,
                        loop_start,
                    );
                if should_defer_catalog_message(
                    &message,
                    self.lib.catalog_ready,
                    &self.ui.nav,
                    self.lib.catalog_ready_stationary_edge_since,
                    loop_start,
                ) {
                    let deferred_since = *self
                        .lib
                        .catalog_ready_deferred_since
                        .get_or_insert(loop_start);
                    self.lib.pending_catalog_ready = Some(message);
                    prepare_trace.catalog_ready_deferred = true;
                    prepare_trace.catalog_ready_deferred_age_us = loop_start
                        .saturating_duration_since(deferred_since)
                        .as_micros();
                } else {
                    self.lib.catalog_ready_deferred_since = None;
                    self.lib.catalog_ready_stationary_edge_since = None;
                    process_catalog_worker_message(
                        message,
                        &mut prepare_trace,
                        &mut self.diag.launcher_response_trace,
                        loop_start,
                        &self.env.app,
                        catalog_domain!(self, full_bridge_dirty),
                        self.fx.startup_intro.is_some(),
                        self.out.start,
                    );
                    catalog_messages_processed = catalog_messages_processed.saturating_add(1);
                }
            }

            while catalog_messages_processed < CATALOG_MESSAGES_PER_FRAME {
                let Some(message) = self.lib.deferred_catalog_events.pop_front() else {
                    break;
                };
                self.lib.catalog_ready_stationary_edge_since =
                    update_catalog_ready_stationary_edge_since(
                        &self.ui.nav,
                        self.lib.catalog_ready_stationary_edge_since,
                        loop_start,
                    );
                if should_defer_catalog_message(
                    &message,
                    self.lib.catalog_ready,
                    &self.ui.nav,
                    self.lib.catalog_ready_stationary_edge_since,
                    loop_start,
                ) {
                    let deferred_since = *self
                        .lib
                        .catalog_ready_deferred_since
                        .get_or_insert(loop_start);
                    if self.lib.pending_catalog_ready.is_none() {
                        self.lib.pending_catalog_ready = Some(message);
                    } else {
                        self.lib.deferred_catalog_events.push_front(message);
                        break;
                    }
                    prepare_trace.catalog_ready_deferred = true;
                    prepare_trace.catalog_ready_deferred_age_us = loop_start
                        .saturating_duration_since(deferred_since)
                        .as_micros();
                    continue;
                }
                process_catalog_worker_message(
                    message,
                    &mut prepare_trace,
                    &mut self.diag.launcher_response_trace,
                    loop_start,
                    &self.env.app,
                    catalog_domain!(self, full_bridge_dirty),
                    self.fx.startup_intro.is_some(),
                    self.out.start,
                );
                catalog_messages_processed = catalog_messages_processed.saturating_add(1);
            }
            prepare_trace.catalog_backlog = self
                .lib
                .deferred_catalog_events
                .len()
                .saturating_add(usize::from(self.lib.pending_catalog_ready.is_some()))
                .min(u32::MAX as usize) as u32;
            if self.lib.deferred_catalog_events.is_empty()
                && self.lib.pending_catalog_ready.is_none()
            {
                self.lib.catalog_ready_deferred_since = None;
                self.lib.catalog_ready_stationary_edge_since = None;
            }
        }
        if let Some(trace_start) = catalog_worker_trace_start {
            prepare_trace.catalog_worker_us = trace_start.elapsed().as_micros();
        }
        scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-input-catalog", scheduler_phase);
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 5);
        if maybe_present_modal_input_test_dialog(
            &mut self.diag.modal_input_test_dialog_pending,
            self.lib.catalog_ready,
            &mut self.lib.lifecycle,
            &mut self.lib.lifecycle_effects,
            &mut self.lib.scheduler,
            self.out.start,
        ) {
            full_bridge_dirty = true;
            self.env.window.request_redraw();
        }
        let media_worker_trace_start = prepare_trace_enabled.then(Instant::now);
        let mut media_message_seen = false;
        if background_work_allowed {
            self.lib.scheduler.poll_media(&mut self.lib.media_events);
            for message in self.lib.media_events.drain() {
                media_message_seen = true;
                let catalog_scan_visible = self
                    .env
                    .app
                    .global::<slint_ui::launcher::CatalogView>()
                    .get_activity()
                    == slint_ui::launcher::CatalogActivity::Foreground;
                let effects = self.lib.media_session.handle_worker_message(
                    message,
                    catalog_scan_visible,
                    loop_start,
                );
                apply_screenshot_media_update_effects(
                    effects,
                    &self.env.app,
                    &mut self.lib.catalog,
                    &mut self.lib.scheduler,
                    Some(&mut self.lib.preview),
                    &mut full_bridge_dirty,
                    self.out.start,
                );
            }
        }
        if let Some(trace_start) = media_worker_trace_start {
            prepare_trace.media_worker_us = trace_start.elapsed().as_micros();
        }
        scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-input-media", scheduler_phase);
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 6);

        if let Some(completion) = self.lib.scheduler.poll_launch_completion(Instant::now()) {
            match completion {
                LaunchHandoffCompletion::Success { benchmark_terminal } => {
                    let input = if benchmark_terminal {
                        LauncherLifecycleInput::BenchmarkLaunchCompleted
                    } else {
                        LauncherLifecycleInput::LaunchSucceeded {
                            spawned_mister: false,
                        }
                    };
                    self.lib
                        .lifecycle
                        .handle(input, &mut self.lib.lifecycle_effects);
                    apply_lifecycle_effects(
                        &mut self.lib.lifecycle_effects,
                        &mut self.lib.scheduler,
                        self.out.start,
                    );
                }
                LaunchHandoffCompletion::Failure { title, error } => {
                    self.lib.lifecycle.handle(
                        LauncherLifecycleInput::LaunchFailed {
                            title,
                            kind: error.kind(),
                            detail: error.to_string(),
                        },
                        &mut self.lib.lifecycle_effects,
                    );
                    apply_lifecycle_effects(
                        &mut self.lib.lifecycle_effects,
                        &mut self.lib.scheduler,
                        self.out.start,
                    );
                    if self.lib.scheduler.stop_spawned_mister_for_recovery()
                        && let Err(e) = self
                            .env
                            .display_session
                            .recover_after_launch_failure(self.out.frames, self.env.f)
                    {
                        crate::ui_errln!(
                            "failed to recover Slint framebuffer route after launch failure: {e}"
                        );
                    }
                    sync_bridge_launcher(
                        &self.env.app,
                        &self.env.pad,
                        &self.ui.nav,
                        &self.lib.lifecycle,
                        &self.inp.setup,
                        "",
                        "",
                        &self.lib.catalog,
                        &mut self.lib.preview,
                        &mut self.ui.bridge_models,
                        self.lib.catalog_version,
                        false,
                        false,
                        self.env.ui,
                    );
                    update_slint_animations(self.env.animation_clock);
                    let recovery_rect = render_immediate_launcher_frame(
                        self.env.window,
                        self.env.target,
                        self.out.layout,
                    );
                    if let Some(rect) = recovery_rect {
                        let _ = copy_cached_rect_565(
                            self.env.disp,
                            self.env.target.cached_frame_view(),
                            rect,
                        );
                    } else {
                        copy_cached_rows_565(
                            self.env.disp,
                            self.env.target.cached_frame_view(),
                            0,
                            self.env.ui.render_h(),
                        );
                    }
                    let recovery_presented = Instant::now();
                    self.env.window.request_redraw();
                    self.lib
                        .scheduler
                        .finish_launch_failure_recovery(recovery_presented);
                    record_launcher_frame_phase!(LauncherFramePhase::LaunchRecoveryApplied);
                    crate::ui_errln!("game launch failed: {error}");
                }
            }
        }
        scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-input-launch-lifecycle", scheduler_phase);
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 7);

        if self.lib.arcade_screen_pending
            && arcade_navigation_ready(self.lib.catalog_ready, &self.lib.catalog)
        {
            let before = LauncherProjectionKey::from_nav(&self.ui.nav);
            if self.ui.nav.active_collection().is_none() {
                let _ = self.ui.nav.open_default_arcade(&self.lib.catalog);
            } else {
                self.ui.nav.screen = Screen::Arcade;
            }
            self.lib.arcade_screen_pending = false;
            full_bridge_dirty = true;
            let after = LauncherProjectionKey::from_nav(&self.ui.nav);
            if before != after {
                self.lib
                    .media_session
                    .note_nav_change(&before, &after, Instant::now());
            }
        }

        if !self.out.director.navigation.is_active()
            && commit_pending_collection_entry(
                &mut self.lib.pending_collection_entry,
                &mut self.ui.nav,
                &self.lib.catalog,
                self.out.start,
            )
        {
            self.diag.arcade_entry_latency.record_rows_ready(
                self.out.start,
                loop_start,
                &self.lib.lifecycle,
                &self.lib.catalog,
                &self.ui.nav,
            );
            full_bridge_dirty = true;
            self.env.window.request_redraw();
        } else if restore_failed_pending_collection_entry(
            &mut self.lib.pending_collection_entry,
            &mut self.ui.nav,
            self.out.start,
        ) {
            self.lib.preview.cancel_system_entry_preview();
            self.diag.arcade_entry_latency.cancel_enter();
            full_bridge_dirty = true;
            if self.out.director.navigation.is_active() {
                self.out.director.navigation.request_reverse(animation_us);
            }
        }

        if self.out.director.navigation.is_active() {
            self.out.director.navigation.tick(animation_us);
            let should_commit = self.out.director.pending.as_ref().is_some_and(|pending| {
                if pending.committed {
                    return false;
                }
                pending.event.action != LauncherAction::OpenCollection
                    || self
                        .lib
                        .pending_collection_entry
                        .as_ref()
                        .is_none_or(|entry| {
                            collection_has_resident_rows(&self.lib.catalog, &entry.collection_id)
                        })
            });
            if should_commit {
                let navigation_commit_started = Instant::now();
                let event = self
                    .out
                    .director
                    .pending
                    .as_ref()
                    .map(|pending| pending.event.clone())
                    .expect("checked pending transition");
                let before = LauncherProjectionKey::from_nav(&self.ui.nav);
                let committing_cold_collection = event.action == LauncherAction::OpenCollection
                    && self.lib.pending_collection_entry.is_some();
                let committed = if committing_cold_collection {
                    commit_pending_collection_entry(
                        &mut self.lib.pending_collection_entry,
                        &mut self.ui.nav,
                        &self.lib.catalog,
                        self.out.start,
                    )
                } else {
                    self.ui
                        .nav
                        .commit_navigation_intent(&event, &self.lib.catalog)
                };
                if committed {
                    if committing_cold_collection {
                        self.diag.arcade_entry_latency.record_rows_ready(
                            self.out.start,
                            loop_start,
                            &self.lib.lifecycle,
                            &self.lib.catalog,
                            &self.ui.nav,
                        );
                    }
                    if let Some(pending) = self.out.director.pending.as_mut() {
                        pending.committed = true;
                    }
                    let after = LauncherProjectionKey::from_nav(&self.ui.nav);
                    if before != after {
                        self.lib
                            .media_session
                            .note_nav_change(&before, &after, Instant::now());
                    }
                    full_bridge_dirty = true;
                    self.env.window.request_redraw();
                } else if event.action != LauncherAction::OpenCollection
                    || self.lib.pending_collection_entry.is_none()
                {
                    self.out.director.navigation.request_reverse(animation_us);
                }
                prepare_trace.navigation_commit_us = prepare_trace
                    .navigation_commit_us
                    .saturating_add(navigation_commit_started.elapsed().as_micros());
            }
            self.env.window.request_redraw();
        }

        if let Some(system_id) = self.lib.pending_start_system.take() {
            if arcade_navigation_ready(self.lib.catalog_ready, &self.lib.catalog) {
                let before = LauncherProjectionKey::from_nav(&self.ui.nav);
                if apply_start_system_from_env(
                    &mut self.ui.nav,
                    &self.lib.catalog,
                    &system_id,
                    ui_frame_target::forced_arcade_selected_index(),
                ) {
                    print_startup_event(
                        self.out.start,
                        "launcher_start_system_applied",
                        format!("system={system_id}"),
                    );
                    let after = LauncherProjectionKey::from_nav(&self.ui.nav);
                    if before != after {
                        self.lib
                            .media_session
                            .note_nav_change(&before, &after, Instant::now());
                        full_bridge_dirty = true;
                    }
                } else {
                    print_startup_event(
                        self.out.start,
                        "launcher_start_system_fallback",
                        format!("system={system_id} reason=missing"),
                    );
                    self.ui.nav.go_root();
                    full_bridge_dirty = true;
                }
            } else {
                self.lib.pending_start_system = Some(system_id);
            }
        }

        scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-input-navigation", scheduler_phase);
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 8);

        if let Some(screen) = effective_lock_screen(
            self.lib.lock_screen,
            self.lib.catalog_ready,
            &self.lib.catalog,
        ) {
            self.ui.nav.screen = screen;
        }

        let catalog_build_busy = screensaver_catalog_busy(
            self.lib.scheduler.catalog_worker_running(),
            self.lib.catalog_session.refresh_done(),
        );
        let restore_before = self.fx.screensaver.restore_full_frame;
        // A screensaver measurement starts it at once and keeps it up for the
        // window; the user's setting and delay apply again afterwards.
        #[cfg(feature = "tooling")]
        let screensaver_measured = self
            .diag
            .tooling
            .as_ref()
            .is_some_and(mister_magik_tooling_support::Session::screensaver_requested);
        #[cfg(not(feature = "tooling"))]
        let screensaver_measured = false;
        self.fx.screensaver.update(
            Instant::now(),
            self.ui.nav.settings.screensaver_enabled || screensaver_measured,
            if screensaver_measured {
                Duration::ZERO
            } else {
                Duration::from_secs(u64::from(self.ui.nav.settings.screensaver_delay_minutes) * 60)
            },
            catalog_build_busy,
            screensaver_preview_start_ready(
                self.lib.catalog_ready,
                self.fx.screensaver_preview_waits_for_analytics,
                self.diag.frame_accounting.frame_analytics_mode(),
            ),
        );
        if !restore_before && self.fx.screensaver.restore_full_frame {
            self.env.window.request_redraw();
        }
        effective_view = EffectiveLauncherView::resolve(
            &self.lib.lifecycle,
            self.fx.screensaver.active,
            self.ui.nav.screen,
        );
        launching = effective_view.launch_active();
        self.diag
            .frame_accounting
            .set_effective_view(effective_view.label());
        self.diag
            .frame_accounting
            .set_catalog_generation(self.lib.catalog_generation.current.as_deref());

        scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-input-view-housekeeping", scheduler_phase);
        note_pre_input_boundary!(begin.tooling_frame_evidence, self.out.run_start, 11);
        Ok(PreInputFrame {
            scheduler_phase,
            loop_start,
            animation_now,
            animation_us,
            directional_input_held,
            background_work_allowed,
            full_bridge_dirty,
            frame_analytics_mode,
            cpu_loop_start,
            arcade_visual_index_at_loop_start,
            arcade_filter_visual_index_at_loop_start,
            prepare_trace_enabled,
            prepare_trace,
            bridge_churn_frame_start,
            effective_view,
            launching,
            setup_active,
            light_bridge_dirty,
            pad_changed_for_input,
            route_action,
            defer_selected_preview,
            preview_scheduled_this_loop,
            clock_update_due,
            clock_update_us,
            slint_animation_active,
            media_message_seen,
        })
    }

    /// Drains the controller batch and routes it to setup, the active screen and the UI actions.
    fn input(
        &mut self,
        #[cfg_attr(not(feature = "tooling"), allow(unused_variables, unused_mut))]
        begin: &mut BeginFrame,
        pre_input: &mut PreInputFrame,
    ) -> Result<InputFrame, Exit> {
        let input_fault_notice: Option<&'static str>;
        record_launcher_frame_phase!(LauncherFramePhase::PreInputMaintenance);
        let (input_phase_yielded, input_batch_empty) = 'input_phase: {
            // Drain immediately before routing so catalog, timer, lifecycle,
            // and bridge housekeeping cannot sit between capture and dispatch.
            let drained_input = self.env.pad.drain_input_batch();
            record_launcher_frame_phase!(LauncherFramePhase::InputCaptured);
            self.inp.input_observation = drained_input.observation;
            self.diag
                .launcher_response_trace
                .observe_drained_input(&drained_input);
            #[cfg(feature = "tooling")]
            if let Some(frame) = begin.tooling_frame_evidence.as_mut() {
                frame.input_sequence = drained_input.batch.last_sequence;
                frame.input_captured_monotonic_us = drained_input
                    .batch
                    .events
                    .last()
                    .map(|event| event.captured_at_us);
                frame.input_dequeued_us = Some(duration_us(self.out.run_start, Instant::now()));
            }
            let input_batch = drained_input.batch;
            let input_batch_empty =
                input_batch.events.is_empty() && !self.inp.launcher_ui_actions.has_pending();
            #[cfg(feature = "tooling")]
            {
                self.diag.tooling_input_epoch += u64::from(!input_batch_empty);
                if let Some(frame) = begin.tooling_frame_evidence.as_mut() {
                    frame.input_epoch = self.diag.tooling_input_epoch;
                }
            }
            let input_route_pmu = self.diag.launcher_response_trace.input_pmu_span(
                !input_batch.events.is_empty(),
                "launcher-response.input-route",
            );
            self.diag.input_integrity_trace.observe_batch(&input_batch);
            self.diag
                .launcher_response_trace
                .record_lab(self.diag.input_latency_lab.before_input_route());
            if !input_batch.events.is_empty()
                && let Some(stall_ms) = self.diag.input_integrity_stall.take()
            {
                std::thread::sleep(Duration::from_millis(stall_ms));
            }
            let pad_changed = pre_input
                .pad_changed_for_input
                .take()
                .unwrap_or_else(|| self.env.pad.poll_with_debug_labels(pre_input.setup_active));
            let frame_now = Instant::now();
            let mut incoming_input_events = VecDeque::new();
            let mut screensaver_wake = false;
            let input_batch_result =
                if ui_test_uses_automation_only_input(self.diag.ui_test_fixture, &input_batch) {
                    Ok(())
                } else {
                    self.inp.input_router.accept_batch(&input_batch)
                };
            self.diag
                .launcher_response_trace
                .record_input_batch_gate(&input_batch, input_batch_result.as_ref().err().copied());
            let input_batch_healthy = match input_batch_result {
                Ok(()) => {
                    input_fault_notice = None;
                    incoming_input_events.extend(input_batch.events.iter().copied());
                    true
                }
                Err(fault) => {
                    input_fault_notice = Some(fault.notice());
                    false
                }
            };
            let mut physical_for_automation = PadState::default();
            for action in crate::input_event::LogicalAction::ALL {
                physical_for_automation
                    .set_logical_action(action, input_batch.held_after_last.is_held(action));
            }
            if input_batch_healthy {
                incoming_input_events.extend(self.diag.launcher_automation.poll_events(
                    &physical_for_automation,
                    pre_input.effective_view.accepts_application_input()
                        && self.lib.lifecycle.startup_input_enabled(),
                    self.inp.setup.is_active(),
                    pre_input.animation_now,
                ));
                if let Some(event) = self.diag.library_changed_dialog_test.event_for(
                    &self.ui.nav,
                    pre_input.animation_now,
                    self.out.start,
                ) {
                    incoming_input_events.push_back(event);
                }
            }
            #[cfg(feature = "tooling")]
            if let Some(held) = self
                .diag
                .tooling
                .as_mut()
                .and_then(|session| session.hold_change())
            {
                self.diag.ui_action_sequence = self.diag.ui_action_sequence.saturating_add(1);
                let captured_at_us = frame_now
                    .saturating_duration_since(self.out.start)
                    .as_micros()
                    .min(u64::MAX as u128) as u64;
                if held {
                    let direction = match self
                        .diag
                        .tooling
                        .as_ref()
                        .map(|session| session.hold_direction())
                    {
                        Some(mister_magik_tooling_support::HoldDirection::Down) => {
                            slint_ui::launcher::NavigationDirection::Down
                        }
                        Some(mister_magik_tooling_support::HoldDirection::Up) => {
                            slint_ui::launcher::NavigationDirection::Up
                        }
                        _ => slint_ui::launcher::NavigationDirection::Right,
                    };
                    let [mut pressed, _] = LauncherUiAction::Navigate(direction)
                        .input_pulse(self.diag.ui_action_sequence, captured_at_us)
                        .unwrap();
                    pressed.source = crate::input_event::InputSourceId {
                        kind: crate::input_event::InputSourceKind::Automation,
                        instance: 0x43415244,
                    };
                    pressed.source_epoch = crate::input_event::SourceEpoch(1);
                    let released = crate::input_event::InputEvent {
                        phase: crate::input_event::InputPhase::Released,
                        ..pressed
                    };
                    incoming_input_events.push_back(pressed);
                    self.diag.tooling_carousel_release = Some(released);
                } else if let Some(mut released) = self.diag.tooling_carousel_release.take() {
                    released.sequence = self.diag.ui_action_sequence;
                    released.captured_at_us = captured_at_us;
                    incoming_input_events.push_back(released);
                }
            }
            // A requested sequence taps like the development keyboard bridge:
            // one press/release pulse through the same router as a held press.
            #[cfg(feature = "tooling")]
            if self
                .diag
                .tooling
                .as_mut()
                .is_some_and(|session| session.carousel_tap_due())
            {
                self.diag.ui_action_sequence = self.diag.ui_action_sequence.saturating_add(1);
                let captured_at_us = frame_now
                    .saturating_duration_since(self.out.start)
                    .as_micros()
                    .min(u64::MAX as u128) as u64;
                for mut event in
                    LauncherUiAction::Navigate(slint_ui::launcher::NavigationDirection::Right)
                        .input_pulse(self.diag.ui_action_sequence, captured_at_us)
                        .unwrap()
                {
                    event.source = crate::input_event::InputSourceId {
                        kind: crate::input_event::InputSourceKind::Automation,
                        instance: 0x43415244,
                    };
                    event.source_epoch = crate::input_event::SourceEpoch(1);
                    incoming_input_events.push_back(event);
                }
            }
            for event in incoming_input_events.iter().copied() {
                self.diag.gui_profiling.observe_route_action(
                    screen_label(self.ui.nav.screen),
                    event,
                    frame_now,
                );
                if self.ui.nav.screen == Screen::Arcade
                    && event.action == LogicalAction::Down
                    && event.phase == InputPhase::Pressed
                    && self.diag.gui_profiling.arcade_scroll_phase_started()
                {
                    self.diag
                        .screensaver_cpu_profile
                        .begin_arcade_velocity_scroll(self.out.frames.saturating_add(1));
                }
            }
            if self.fx.screensaver.active {
                while let Some(event) = incoming_input_events.pop_front() {
                    let focus =
                        launcher_input_focus(true, true, false, false, false, false, &self.ui.nav);
                    let outcome =
                        self.inp
                            .input_router
                            .route_event(event, focus, pre_input.animation_now);
                    self.diag
                        .launcher_response_trace
                        .record_route(event, outcome);
                    if matches!(outcome, InputOutcome::WakeScreensaver { .. }) {
                        self.inp.latency_critical_input_pending = true;
                        screensaver_wake = true;
                        let _ = self.inp.input_router.consume_remaining_batch(
                            incoming_input_events.drain(..),
                            ConsumedReason::ExclusiveBatch,
                        );
                        break;
                    }
                }
            }
            if let Some(completion) = self.env.pad.take_controller_save_completion() {
                match completion.result {
                    Ok(()) => crate::ui_errln!(
                        "controller setup: persisted registry revision {}",
                        completion.revision
                    ),
                    Err(error) => crate::ui_errln!(
                        "controller setup: revision {} save failed: {error}",
                        completion.revision
                    ),
                }
                pre_input.full_bridge_dirty = true;
            }
            let controller_save_notice = self.env.pad.controller_save_notice();
            let input_notice = input_fault_notice.or_else(|| {
                self.inp.setup_disconnect_notice.then_some(
                    "Controller disconnected. Press a button after reconnecting to restart setup.",
                )
            });
            let input = self.env.app.global::<slint_ui::launcher::InputView>();
            input.set_fault_notice(
                input_notice
                    .or(controller_save_notice)
                    .unwrap_or_default()
                    .into(),
            );
            input.set_input_availability(if input_notice.is_some() {
                slint_ui::launcher::InputAvailability::Unavailable
            } else {
                slint_ui::launcher::InputAvailability::Available
            });
            self.diag
                .frame_accounting
                .set_automation_action_sequence(self.diag.launcher_automation.action_sequence());

            let application_input_enabled = pre_input.effective_view.accepts_application_input()
                && self.lib.lifecycle.startup_input_enabled();
            if !application_input_enabled {
                self.inp.launcher_ui_actions.discard_all();
                let disabled =
                    launcher_input_focus(false, false, false, false, false, false, &self.ui.nav);
                self.inp.input_router.set_focus(disabled);
                for event in incoming_input_events.drain(..) {
                    let outcome =
                        self.inp
                            .input_router
                            .route_event(event, disabled, pre_input.animation_now);
                    self.diag
                        .launcher_response_trace
                        .record_route(event, outcome);
                }
            }

            if application_input_enabled {
                if self.inp.setup.is_active()
                    && self
                        .inp
                        .setup
                        .target_device
                        .as_ref()
                        .is_none_or(|device| self.env.pad.info_for_device(device).is_none())
                {
                    crate::ui_errln!("controller setup: target disconnected; closing setup flow");
                    self.inp.setup.cancel_disconnected();
                    self.inp.setup_disconnect_notice = true;
                    input.set_fault_notice(
                        "Controller disconnected. Press a button after reconnecting to restart setup."
                            .into(),
                    );
                    input
                        .set_input_availability(slint_ui::launcher::InputAvailability::Unavailable);
                    pre_input.full_bridge_dirty = true;
                }

                let ui_input_pending = self.inp.launcher_ui_actions.has_pending();
                let raw_screensaver_input_activity = self.env.pad.user_activity()
                    || self.diag.launcher_automation.active()
                    || ui_input_pending;
                let physical_input_held =
                    input_batch_healthy && pad_state_has_active_input(&physical_for_automation);
                let screensaver_input_held = self.fx.screensaver.input_held_for_control(
                    screensaver_wake,
                    physical_input_held || ui_input_pending,
                );
                if self.fx.screensaver.handle_input(
                    frame_now,
                    screensaver_input_held,
                    raw_screensaver_input_activity,
                ) {
                    self.inp.launcher_ui_actions.discard_all();
                    record_launcher_frame_phase!(LauncherFramePhase::InputConsumed);
                    self.env.window.request_redraw();
                    record_launcher_frame_phase!(LauncherFramePhase::Yielded);
                    break 'input_phase (true, input_batch_empty);
                }
                let active_device = self.env.pad.active_device();
                let info = self.env.pad.info().clone();
                loop {
                    let lifecycle_view = self.lib.lifecycle.view();
                    let card_home_animating = self.fx.launcher_card_home.as_ref().is_some_and(
                        super::launcher_card_home::LauncherCardHomeSession::is_animating,
                    );
                    let level_trick_active = self.fx.launcher_card_home.as_ref().is_some_and(
                        super::launcher_card_home::LauncherCardHomeSession::is_level_trick_active,
                    );
                    let deferred_settings_event = self
                        .inp
                        .deferred_settings_activation
                        .take_when_settled(card_home_animating);
                    let focus = launcher_input_focus(
                        true,
                        false,
                        lifecycle_view.launch_failure_dialog().is_some()
                            || lifecycle_view.catalog_recovery_dialog().is_some(),
                        self.inp.setup.is_active(),
                        self.ui.nav.confirm_action.is_some(),
                        self.out.director.navigation.is_active()
                            || self.out.director.orientation.is_active()
                            || !self.out.director.chart.is_live()
                            || self.inp.deferred_settings_activation.is_pending()
                            || level_trick_active,
                        &self.ui.nav,
                    );
                    self.inp.input_router.set_focus(focus);
                    let mut final_input_tick = false;
                    let mut direct_ui_action_this_loop = None;
                    let mut routed_event_this_loop = if let Some(event) = deferred_settings_event {
                        Some(event)
                    } else if let Some(event) = incoming_input_events.pop_front() {
                        let outcome = self.inp.input_router.route_event(
                            event,
                            focus,
                            pre_input.animation_now,
                        );
                        self.diag.input_integrity_trace.record_outcome(outcome);
                        self.diag
                            .launcher_response_trace
                            .record_route(event, outcome);
                        self.inp.latency_critical_input_pending |= matches!(
                            outcome,
                            InputOutcome::Dispatch { .. } | InputOutcome::WakeScreensaver { .. }
                        );
                        match outcome {
                            InputOutcome::Dispatch { event, .. } => Some(event),
                            InputOutcome::Released { event, context, .. }
                                if context == self.inp.input_router.context() =>
                            {
                                Some(event)
                            }
                            InputOutcome::Released { .. } => None,
                            InputOutcome::WakeScreensaver { .. }
                            | InputOutcome::Consumed { .. } => None,
                        }
                    } else if let Some(action) = self
                        .inp
                        .launcher_ui_actions
                        .pop_routable(focus.target.kind != InputContextKind::Transition)
                    {
                        self.diag.ui_action_sequence =
                            self.diag.ui_action_sequence.saturating_add(1);
                        if let Some([event, released]) = action.input_pulse(
                            self.diag.ui_action_sequence,
                            frame_now
                                .saturating_duration_since(self.out.start)
                                .as_micros()
                                .min(u64::MAX as u128) as u64,
                        ) {
                            self.inp.latency_critical_input_pending = true;
                            incoming_input_events.push_front(released);
                            Some(event)
                        } else {
                            self.inp.latency_critical_input_pending = true;
                            direct_ui_action_this_loop = Some(action);
                            None
                        }
                    } else if focus.target.kind != InputContextKind::Transition
                        && let Some(outcome @ InputOutcome::Dispatch { event, .. }) =
                            self.inp.input_router.tick_repeat(pre_input.animation_now)
                    {
                        self.diag.input_integrity_trace.record_outcome(outcome);
                        self.diag
                            .launcher_response_trace
                            .record_route(event, outcome);
                        self.inp.latency_critical_input_pending = true;
                        Some(event)
                    } else {
                        final_input_tick = true;
                        None
                    };
                    let mut launcher_state = PadState::default();
                    for action in crate::input_event::LogicalAction::ALL {
                        launcher_state
                            .set_logical_action(action, self.inp.input_router.action_held(action));
                    }
                    let selection_feedback_before = discrete_selection_feedback_target(
                        &self.ui.nav,
                        &self.inp.setup,
                        &self.lib.lifecycle,
                    );
                    let selection_feedback_input =
                        accepted_selection_feedback_input(routed_event_this_loop.as_ref())
                            || direct_ui_action_this_loop.is_some();

                    if self.inp.setup.is_active() {
                        let setup_before = SetupBridgeKey::from_setup(&self.inp.setup);
                        let target_device = self
                            .inp
                            .setup
                            .target_device
                            .clone()
                            .expect("active setup has an exact device identity");
                        let Some(setup_info) =
                            self.env.pad.info_for_device(&target_device).cloned()
                        else {
                            self.inp.setup.cancel_disconnected();
                            self.inp.setup_disconnect_notice = true;
                            pre_input.full_bridge_dirty = true;
                            continue;
                        };
                        if let Some(LauncherUiAction::SelectSetupEntry(index)) =
                            direct_ui_action_this_loop.as_ref()
                        {
                            self.inp.setup.list_index = *index;
                        }
                        let setup_action =
                            routed_event_this_loop.map_or(SetupAction::None, |event| {
                                self.inp.setup.handle_action(
                                    &event,
                                    pre_input.animation_now,
                                    &setup_info,
                                    self.env.pad.db(),
                                )
                            });
                        match setup_action {
                            SetupAction::None => {}
                            SetupAction::RegisterNew => {
                                if let Err(e) = self.env.pad.register_new(&target_device) {
                                    crate::ui_errln!("controller setup: register new: {e}");
                                }
                            }
                            SetupAction::ClaimExisting { list_index } => {
                                if let Err(e) =
                                    self.env.pad.claim_existing(&target_device, list_index)
                                {
                                    crate::ui_errln!("controller setup: claim existing: {e}");
                                }
                            }
                            SetupAction::SaveFinish { label, kind } => {
                                if let Err(e) =
                                    self.env.pad.finish_setup(&target_device, label, kind)
                                {
                                    crate::ui_errln!("controller setup: save: {e}");
                                } else {
                                    crate::ui_errln!(
                                        "controller setup: queued registry revision {}",
                                        self.env.pad.controller_save_status().requested
                                    );
                                }
                                self.inp.setup.advance_to_next_pad(&self.env.pad);
                            }
                            SetupAction::Done => {
                                if self.env.pad.controller_save_status().is_failed()
                                    && let Err(error) = self.env.pad.retry_controller_save()
                                {
                                    crate::ui_errln!(
                                        "controller setup: retry could not be queued: {error}"
                                    );
                                }
                                self.inp.setup.advance_to_next_pad(&self.env.pad);
                            }
                        }
                        let setup_after = SetupBridgeKey::from_setup(&self.inp.setup);
                        pre_input.full_bridge_dirty |= pad_changed || setup_before != setup_after;
                    } else {
                        if AUTO_CONTROLLER_SETUP_ENABLED && pad_changed {
                            let setup_before = SetupBridgeKey::from_setup(&self.inp.setup);
                            self.inp.setup.maybe_open(
                                &info,
                                active_device.clone(),
                                self.env.pad.db(),
                                true,
                            );
                            if self.inp.setup.is_active() {
                                self.inp.setup_disconnect_notice = false;
                            }
                            pre_input.full_bridge_dirty |=
                                setup_before != SetupBridgeKey::from_setup(&self.inp.setup);
                        }
                        if !self.inp.setup.is_active() {
                            let nav_before = LauncherProjectionKey::from_nav(&self.ui.nav);
                            let arcade_selected_before_input = self.ui.nav.arcade.selected;
                            let lifecycle_view = self.lib.lifecycle.view();
                            let launch_failure_visible =
                                lifecycle_view.launch_failure_dialog().is_some();
                            let recovery_dialog_visible =
                                lifecycle_view.catalog_recovery_dialog().is_some();
                            if self
                                .inp
                                .deferred_settings_activation
                                .intercept_while_cards_move(
                                    &self.ui.nav,
                                    card_home_animating,
                                    &mut routed_event_this_loop,
                                )
                            {
                                self.env.window.request_redraw();
                            }
                            let pending_collection_cancelled =
                                cancel_pending_collection_entry_for_input(
                                    &mut self.lib.pending_collection_entry,
                                    &mut self.ui.nav,
                                    routed_event_this_loop.as_ref(),
                                    self.out.start,
                                );
                            if pending_collection_cancelled {
                                self.lib.preview.cancel_system_entry_preview();
                                self.diag.arcade_entry_latency.cancel_enter();
                                if self.out.director.navigation.is_active() {
                                    self.out
                                        .director
                                        .navigation
                                        .request_reverse(pre_input.animation_us);
                                }
                            }
                            let settings_transition_source = (!launch_failure_visible
                                && !recovery_dialog_visible
                                && !self.out.director.navigation.is_active()
                                && self.out.director.navigation.enabled()
                                && settings_navigation_source_candidate(
                                    &self.ui.nav,
                                    routed_event_this_loop.as_ref(),
                                ))
                            .then(|| {
                                (
                                    self.ui.nav.screen,
                                    self.ui.nav.navigation_transition_state(),
                                )
                            });
                            let event = if self.out.director.orientation.is_active()
                                || self.out.director.chart.owner()
                                    == Some(FullScreenTransitionOwner::Orientation)
                                || self.out.director.navigation.is_active()
                            {
                                None
                            } else if launch_failure_visible || recovery_dialog_visible {
                                let ui_inputs =
                                    direct_ui_action_this_loop.as_ref().and_then(|action| {
                                        lifecycle_dialog_ui_inputs(
                                            action,
                                            launch_failure_visible,
                                            recovery_dialog_visible,
                                        )
                                    });
                                let routed_input = route_lifecycle_dialog_input(
                                    routed_event_this_loop.as_ref(),
                                    launch_failure_visible,
                                    recovery_dialog_visible,
                                );
                                if let Some(inputs) =
                                    ui_inputs.or_else(|| routed_input.map(|input| vec![input]))
                                {
                                    for input in inputs {
                                        self.lib
                                            .lifecycle
                                            .handle(input, &mut self.lib.lifecycle_effects);
                                    }
                                    apply_lifecycle_effects(
                                        &mut self.lib.lifecycle_effects,
                                        &mut self.lib.scheduler,
                                        self.out.start,
                                    );
                                    pre_input.full_bridge_dirty = true;
                                }
                                None
                            } else if self.lib.scheduler.should_request_benchmark_launch()
                                && self.lib.catalog_ready
                                && self.ui.nav.screen == Screen::Arcade
                            {
                                active_system(&self.lib.catalog, &self.ui.nav)
                                    .and_then(|system| {
                                        self.ui.nav.active_arcade_game_at(
                                            &self.lib.catalog,
                                            &system.id,
                                            self.ui.nav.arcade.selected,
                                        )
                                    })
                                    .map(|game| launcher::LauncherEvent {
                                        action: LauncherAction::LaunchGame,
                                        path: Some(game.mra_path.to_string()),
                                        settings: None,
                                    })
                            } else if self.diag.auto_launch_selected
                                && !self.diag.auto_launch_selected_done
                                && launcher_auto_launch_gate_ready(
                                    self.diag.auto_launch_gate.as_deref(),
                                )
                                && self.lib.catalog_ready
                                && self.ui.nav.screen == Screen::Arcade
                            {
                                let event = active_system(&self.lib.catalog, &self.ui.nav)
                                    .and_then(|system| {
                                        self.ui.nav.active_arcade_game_at(
                                            &self.lib.catalog,
                                            &system.id,
                                            self.ui.nav.arcade.selected,
                                        )
                                    })
                                    .map(|game| launcher::LauncherEvent {
                                        action: LauncherAction::LaunchGame,
                                        path: Some(game.mra_path.to_string()),
                                        settings: None,
                                    });
                                self.diag.auto_launch_selected_done = event.is_some();
                                event
                            } else if self.lib.scheduler.launch_benchmark_enabled() {
                                None
                            } else if let Some(input_event) = routed_event_this_loop.as_ref() {
                                self.ui.nav.handle_action_with_navigation_intents(
                                    input_event,
                                    pre_input.animation_now,
                                    &self.lib.catalog,
                                )
                            } else if let Some(action) = direct_ui_action_this_loop.take() {
                                apply_navigation_action(
                                    action,
                                    &mut self.ui.nav,
                                    &self.lib.catalog,
                                    pre_input.animation_now,
                                )
                            } else if final_input_tick {
                                self.ui.nav.handle_held_tick_with_navigation_intents(
                                    &launcher_state,
                                    pre_input.animation_now,
                                    &self.lib.catalog,
                                )
                            } else {
                                None
                            };
                            let event = if !final_input_tick
                                && focus.target.kind == InputContextKind::Screen
                            {
                                event.or_else(|| {
                                    self.ui.nav.handle_held_tick_with_navigation_intents(
                                        &launcher_state,
                                        pre_input.animation_now,
                                        &self.lib.catalog,
                                    )
                                })
                            } else {
                                event
                            };
                            if let Some((source_screen, source_state)) = settings_transition_source
                                && let Some((route, direction)) =
                                    settings_page_transition(source_screen, self.ui.nav.screen)
                            {
                                let started = begin_settings_transition(
                                    &mut self.out.director.navigation,
                                    self.fx.launcher_card_home.as_mut(),
                                    &SettingsInputs {
                                        route,
                                        direction,
                                        orientation: self.ui.nav.settings.screen_orientation,
                                        render_w: self.env.ui.render_w(),
                                        render_h: self.env.ui.render_h(),
                                        reduce_motion: self.ui.nav.settings.reduce_motion,
                                        composed: self.env.target.cached_565(),
                                        now_us: pre_input.animation_us,
                                    },
                                );
                                if started
                                    && self.out.director.adopt_navigation(|| PendingNavigation {
                                        event: launcher::LauncherEvent {
                                            action: LauncherAction::NavigateBack,
                                            path: None,
                                            settings: None,
                                        },
                                        source_state,
                                        source_was_arcade: false,
                                        committed: true,
                                        status_quiesce_started_at: None,
                                    })
                                {
                                    pre_input.full_bridge_dirty = true;
                                    self.env.window.request_redraw();
                                } else if started {
                                    self.out.director.unwind_navigation();
                                }
                            }
                            if let Some(event) = event {
                                match event.action {
                                    LauncherAction::OpenMenu
                                    | LauncherAction::OpenCollection
                                    | LauncherAction::NavigateBack
                                    | LauncherAction::NavigateHome
                                    | LauncherAction::ToggleSystemPage
                                    | LauncherAction::OpenSystemSection => {
                                        let collection_id = (event.action
                                            == LauncherAction::OpenCollection)
                                            .then(|| event.path.clone())
                                            .flatten();
                                        if let Some(collection_id) = collection_id.as_deref()
                                            && !collection_has_resident_rows(
                                                &self.lib.catalog,
                                                collection_id,
                                            )
                                        {
                                            let requested_at = Instant::now();
                                            let entry = begin_cold_collection_entry(
                                                &mut self.lib.scheduler,
                                                &mut self.ui.nav,
                                                &mut self.lib.preview,
                                                &self.lib.catalog,
                                                self.lib.catalog_version,
                                                collection_id,
                                                requested_at,
                                                "open-collection-intent",
                                                false,
                                                &mut self.diag.arcade_entry_latency,
                                                &self.lib.lifecycle,
                                                self.out.start,
                                            );
                                            pre_input.full_bridge_dirty |= entry.bridge_dirty;
                                            if entry.pending.is_some() {
                                                self.lib.pending_collection_entry = entry.pending;
                                            }
                                        }

                                        let transition_spec = navigation_transition_for_intent(
                                            &self.ui.nav,
                                            &event,
                                            self.fx.launcher_card_home.is_some(),
                                        );
                                        if transition_spec.is_some()
                                            && self.ui.nav.screen == Screen::Arcade
                                            && !self.out.crt_layout
                                            && !self.out.layout.is_portrait()
                                        {
                                            if !self.ui.nav.is_system_hub() {
                                                self.out
                                                    .arcade_list_renderer
                                                    .compose_layer_to_cached(self.env.target, true);
                                            }
                                            let _ = self.env.target.compose_direct_preview_rect(
                                                preview_screen_rect(self.env.ui),
                                            );
                                        }
                                        let navigation_runtime_started = transition_spec
                                            .is_some_and(|(edge, direction)| {
                                                begin_navigation_transition(
                                                    &mut self.out.director.navigation,
                                                    self.fx.launcher_card_home.as_mut(),
                                                    self.env.target.cached_565(),
                                                    &TransitionInputs {
                                                        edge,
                                                        direction,
                                                        nav: &self.ui.nav,
                                                        collection_id: collection_id.as_deref(),
                                                        layout: self.out.layout,
                                                        crt_layout: self.out.crt_layout,
                                                        crt_metrics: &self.out.crt_metrics,
                                                        crt_backdrop: self
                                                            .out
                                                            .crt_backdrop
                                                            .as_ref()
                                                            .map_or(&[], |b| b.pixels()),
                                                        now_us: pre_input.animation_us,
                                                    },
                                                    || {
                                                        selected_device_reveal_image(
                                                            &self.lib.preview,
                                                            self.out.crt_backdrop.as_ref(),
                                                            self.out.layout,
                                                        )
                                                    },
                                                )
                                            });
                                        let transition_started = navigation_runtime_started
                                            && self.out.director.adopt_navigation(|| {
                                                PendingNavigation {
                                                    event: event.clone(),
                                                    source_state: self
                                                        .ui
                                                        .nav
                                                        .navigation_transition_state(),
                                                    source_was_arcade: self.ui.nav.screen
                                                        == Screen::Arcade,
                                                    committed: false,
                                                    status_quiesce_started_at: None,
                                                }
                                            });
                                        if transition_started {
                                            pre_input.full_bridge_dirty = true;
                                            self.env.window.request_redraw();
                                        } else if navigation_runtime_started {
                                            self.out.director.unwind_navigation();
                                        } else if (collection_id.is_none()
                                            || collection_id.as_deref().is_some_and(
                                                |collection_id| {
                                                    collection_id
                                                        == arcade_catalog::MENU_ARCADE_SYSTEM_ID
                                                        || collection_has_resident_rows(
                                                            &self.lib.catalog,
                                                            collection_id,
                                                        )
                                                },
                                            ))
                                            && self
                                                .ui
                                                .nav
                                                .commit_navigation_intent(&event, &self.lib.catalog)
                                        {
                                            if let Some(collection_id) = collection_id.as_deref() {
                                                print_startup_event(
                                                    self.out.start,
                                                    "catalog_system_entry_immediate",
                                                    format!(
                                                        "system={collection_id} resident_rows={}",
                                                        self.lib
                                                            .catalog
                                                            .system_game_count(collection_id)
                                                    ),
                                                );
                                            }
                                            pre_input.full_bridge_dirty = true;
                                            self.env.window.request_redraw();
                                        }
                                    }
                                    LauncherAction::ExitToMister => {
                                        if self.diag.ui_test_fixture {
                                            crate::ui_logln!(
                                                "ui_test_effect_blocked effect=exit_to_mister"
                                            );
                                            return Err(Exit::Skip);
                                        }
                                        self.lib.loading_title = "Exit to MiSTer".to_string();
                                        sync_bridge_launcher(
                                            &self.env.app,
                                            &self.env.pad,
                                            &self.ui.nav,
                                            &self.lib.lifecycle,
                                            &self.inp.setup,
                                            self.lib
                                                .scheduler
                                                .visible_loading_title(&self.lib.loading_title),
                                            "Return to MiSTer MagiK after reboot",
                                            &self.lib.catalog,
                                            &mut self.lib.preview,
                                            &mut self.ui.bridge_models,
                                            self.lib.catalog_version,
                                            false,
                                            false,
                                            self.env.ui,
                                        );
                                        self.env.window.request_redraw();
                                        update_slint_animations(self.env.animation_clock);
                                        let _ = render_immediate_launcher_frame(
                                            self.env.window,
                                            self.env.target,
                                            self.out.layout,
                                        );
                                        let _pace = self.out.pacer.wait();
                                        copy_cached_rows_565(
                                            self.env.disp,
                                            self.env.target.cached_frame_view(),
                                            0,
                                            self.env.ui.render_h(),
                                        );
                                        match launcher::exit_to_mister() {
                                            Ok(()) => std::process::exit(0),
                                            Err(e) => {
                                                crate::ui_errln!("exit to MiSTer failed: {e}");
                                                self.lib.loading_title.clear();
                                            }
                                        }
                                    }
                                    LauncherAction::RefreshDatabase => {
                                        if self.diag.ui_test_fixture {
                                            crate::ui_logln!(
                                                "ui_test_effect_blocked effect=refresh_database"
                                            );
                                            return Err(Exit::Skip);
                                        }
                                        let effects = self.lib.catalog_session.refresh_database(
                                            self.lib.arcade_root.clone(),
                                            self.lib.scheduler.catalog_worker_available(),
                                        );
                                        apply_catalog_session_effects(
                                            effects,
                                            &mut self.diag.launcher_response_trace,
                                            &self.env.app,
                                            catalog_domain!(self, pre_input.full_bridge_dirty),
                                            false,
                                            pre_input.loop_start,
                                            self.out.start,
                                        );
                                        self.env.window.request_redraw();
                                        return Err(Exit::Skip);
                                    }
                                    LauncherAction::Restart | LauncherAction::PurgeLibraryData => {
                                        let resetting =
                                            event.action == LauncherAction::PurgeLibraryData;
                                        if self.diag.ui_test_fixture {
                                            crate::ui_logln!(
                                                "ui_test_effect_blocked effect={}",
                                                if resetting {
                                                    "purge_library_data"
                                                } else {
                                                    "restart"
                                                }
                                            );
                                            return Err(Exit::Skip);
                                        }
                                        if resetting
                                            && (!self.lib.scheduler.catalog_worker_available()
                                                || self.lib.scheduler.media_worker_running())
                                        {
                                            self.ui.nav.show_library_reset_error("Catalog or screenshot work is still running. Wait for it to finish, then hold A for 7 seconds again.".into());
                                            self.inp.library_reset_bridge_dirty = true;
                                            self.env.window.request_redraw();
                                            return Err(Exit::Skip);
                                        }
                                        self.lib.loading_title = if resetting {
                                            "Deleting database and screenshot packs…"
                                        } else {
                                            "Shutting down…"
                                        }
                                        .to_string();
                                        sync_bridge_launcher(
                                            &self.env.app,
                                            &self.env.pad,
                                            &self.ui.nav,
                                            &self.lib.lifecycle,
                                            &self.inp.setup,
                                            self.lib
                                                .scheduler
                                                .visible_loading_title(&self.lib.loading_title),
                                            "Restarting MiSTer",
                                            &self.lib.catalog,
                                            &mut self.lib.preview,
                                            &mut self.ui.bridge_models,
                                            self.lib.catalog_version,
                                            false,
                                            false,
                                            self.env.ui,
                                        );
                                        self.env.window.request_redraw();
                                        update_slint_animations(self.env.animation_clock);
                                        let _ = render_immediate_launcher_frame(
                                            self.env.window,
                                            self.env.target,
                                            self.out.layout,
                                        );
                                        let _pace = self.out.pacer.wait();
                                        copy_cached_rows_565(
                                            self.env.disp,
                                            self.env.target.cached_frame_view(),
                                            0,
                                            self.env.ui.render_h(),
                                        );
                                        if resetting {
                                            let (sender, receiver) = std::sync::mpsc::channel();
                                            match std::thread::Builder::new()
                                                .name("library-reset".into())
                                                .spawn(move || {
                                                    let _ = sender.send(
                                                        launcher::purge_library_data_and_reboot(),
                                                    );
                                                }) {
                                                Ok(_) => {
                                                    self.inp.library_reset =
                                                        LibraryResetState::Deleting(receiver)
                                                }
                                                Err(error) => {
                                                    self.lib.loading_title.clear();
                                                    self.ui.nav.show_library_reset_error(format!(
                                                        "Could not start database reset: {error}"
                                                    ));
                                                    self.inp.library_reset_bridge_dirty = true;
                                                    self.env.window.request_redraw();
                                                }
                                            }
                                            return Err(Exit::Skip);
                                        }
                                        std::thread::sleep(Duration::from_millis(250));
                                        match launcher::reboot_mister() {
                                            Ok(()) => return Err(Exit::Skip),
                                            Err(e) => {
                                                crate::ui_errln!("restart failed: {e}");
                                                self.lib.loading_title.clear();
                                            }
                                        }
                                    }
                                    LauncherAction::ContinueWithStaleLibrary => {
                                        let effects =
                                            self.lib.catalog_session.continue_with_stale_library();
                                        apply_catalog_session_effects(
                                            effects,
                                            &mut self.diag.launcher_response_trace,
                                            &self.env.app,
                                            catalog_domain!(self, pre_input.full_bridge_dirty),
                                            false,
                                            pre_input.loop_start,
                                            self.out.start,
                                        );
                                        self.env.window.request_redraw();
                                        return Err(Exit::Skip);
                                    }
                                    LauncherAction::RebuildLibrary => {
                                        if self.diag.ui_test_fixture {
                                            crate::ui_logln!(
                                                "ui_test_effect_blocked effect=rebuild_library"
                                            );
                                            return Err(Exit::Skip);
                                        }
                                        let effects = self
                                            .lib
                                            .catalog_session
                                            .rebuild_library(self.lib.arcade_root.clone());
                                        apply_catalog_session_effects(
                                            effects,
                                            &mut self.diag.launcher_response_trace,
                                            &self.env.app,
                                            catalog_domain!(self, pre_input.full_bridge_dirty),
                                            false,
                                            pre_input.loop_start,
                                            self.out.start,
                                        );
                                        self.env.window.request_redraw();
                                        return Err(Exit::Skip);
                                    }
                                    LauncherAction::ApplyDisplayResolution => {
                                        if let Some(id) = event.path.as_deref() {
                                            let result = launcher::apply_display_resolution(id);
                                            self.out.pacer.rearm_after_display_mode_change();
                                            if let Err(error) = result {
                                                crate::ui_errln!("display apply failed: {error}");
                                                self.ui.nav.display_error = Some(format!(
                                                    "Could not apply the selected resolution: {error}"
                                                ));
                                                self.ui.nav.confirm_action = Some(
                                                    launcher::ConfirmAction::DisplayResolutionError,
                                                );
                                                self.ui.nav.confirm_selected = 0;
                                            }
                                        }
                                    }
                                    LauncherAction::ConfirmDisplayResolution => {
                                        self.inp
                                            .display_confirmation
                                            .begin_confirm(&mut self.ui.nav);
                                    }
                                    LauncherAction::CancelDisplayResolution => {
                                        let result = launcher::cancel_display_resolution();
                                        self.out.pacer.rearm_after_display_mode_change();
                                        if let Err(error) = result {
                                            crate::ui_errln!("display rollback failed: {error}");
                                            self.ui.nav.display_error = Some(format!(
                                                "Could not restore the previous resolution: {error}"
                                            ));
                                            self.ui.nav.confirm_action = Some(
                                                launcher::ConfirmAction::DisplayResolutionError,
                                            );
                                            self.ui.nav.confirm_selected = 0;
                                        }
                                    }
                                    LauncherAction::ApplyScreenOrientation => {
                                        if let Some(orientation) =
                                            event.path.as_deref().and_then(ScreenOrientation::parse)
                                            && orientation
                                                != self.ui.nav.settings.screen_orientation
                                        {
                                            let previous = self.ui.nav.settings.screen_orientation;
                                            self.inp
                                                .orientation_confirmation
                                                .begin_apply(&mut self.ui.nav, previous);
                                            let animated = begin_orientation_transition(
                                                &self.env.app,
                                                self.env.window,
                                                self.env.ui,
                                                self.env.target,
                                                previous,
                                                orientation,
                                                pre_input.animation_now,
                                                self.ui.nav.settings.reduce_motion,
                                                &mut self.ui.nav,
                                                &mut self.out.layout,
                                                &mut self.out.layout_epoch,
                                                &mut self.out.director,
                                                &mut self.diag.orientation_preparation_trace,
                                                OrientationIntent::Confirm,
                                            );
                                            if !animated {
                                                self.inp
                                                    .orientation_confirmation
                                                    .start_countdown(Instant::now());
                                            }
                                            self.out.orientation_full_redraw_pending = true;
                                            pre_input.full_bridge_dirty = true;
                                        }
                                    }
                                    LauncherAction::ConfirmScreenOrientation => {
                                        self.inp
                                            .orientation_confirmation
                                            .begin_confirm(&mut self.ui.nav);
                                    }
                                    LauncherAction::CancelScreenOrientation => {
                                        if let Some(previous) = self
                                            .inp
                                            .orientation_confirmation
                                            .finish_cancel(&mut self.ui.nav)
                                        {
                                            let from = self.ui.nav.settings.screen_orientation;
                                            begin_orientation_transition(
                                                &self.env.app,
                                                self.env.window,
                                                self.env.ui,
                                                self.env.target,
                                                from,
                                                previous,
                                                pre_input.animation_now,
                                                self.ui.nav.settings.reduce_motion,
                                                &mut self.ui.nav,
                                                &mut self.out.layout,
                                                &mut self.out.layout_epoch,
                                                &mut self.out.director,
                                                &mut self.diag.orientation_preparation_trace,
                                                OrientationIntent::Rollback,
                                            );
                                        }
                                        self.out.orientation_full_redraw_pending = true;
                                        pre_input.full_bridge_dirty = true;
                                    }
                                    LauncherAction::PreviewScreensaver => {
                                        if !self.fx.screensaver.preview_active {
                                            self.fx.screensaver.preview(frame_now);
                                        }
                                        self.env.window.request_redraw();
                                        return Err(Exit::Skip);
                                    }
                                    LauncherAction::PersistSettings => {
                                        if let Some(settings) = event.settings.as_ref() {
                                            self.out.director.navigation.set_enabled(
                                                self.env.ui.render_w(),
                                                self.env.ui.render_h(),
                                                !settings.reduce_motion,
                                            );
                                            if let Err(error) =
                                                self.inp.settings_store.save(settings)
                                            {
                                                crate::ui_errln!(
                                                    "settings: failed to save launcher settings: {error}"
                                                );
                                            }
                                        }
                                    }
                                    LauncherAction::AddFavourite
                                    | LauncherAction::RemoveFavourite => {
                                        let favourite =
                                            event.action == LauncherAction::AddFavourite;
                                        if let Some(launch_ref) = event.path.as_deref()
                                            && let Some(game) = self
                                                .ui
                                                .nav
                                                .active_arcade_game_view(
                                                    &self.lib.catalog,
                                                    self.ui.nav.active_collection_scope_id(
                                                        &self.lib.catalog,
                                                    ),
                                                )
                                                .get(self.ui.nav.arcade.selected)
                                                .filter(|game| game.mra_path.as_ref() == launch_ref)
                                                .map(|game| {
                                                    self.lib
                                                        .catalog
                                                        .user_game_identity_for_entry(game)
                                                })
                                                .or_else(|| {
                                                    self.lib
                                                        .catalog
                                                        .user_game_identity_for_ref(launch_ref)
                                                })
                                        {
                                            let now = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .ok()
                                                .and_then(|duration| {
                                                    i64::try_from(duration.as_secs()).ok()
                                                })
                                                .unwrap_or(0);
                                            if self.diag.ui_test_fixture {
                                                self.ui.nav.reconcile_favourite_state(
                                                    &self.lib.catalog,
                                                    launch_ref,
                                                    favourite,
                                                );
                                                crate::ui_logln!(
                                                    "ui_test_effect_blocked effect=favourite_persist"
                                                );
                                            } else if self.lib.user_state_session.available()
                                                && let Err(error) = self
                                                    .lib
                                                    .user_state_session
                                                    .set_favourite(game, favourite, now)
                                            {
                                                crate::ui_errln!("user-state: {error}");
                                            }
                                            pre_input.full_bridge_dirty = true;
                                            self.env.window.request_redraw();
                                        }
                                    }
                                    LauncherAction::LaunchGame => {}
                                }
                                if event.action == LauncherAction::LaunchGame {
                                    if self.diag.ui_test_fixture {
                                        crate::ui_logln!(
                                            "ui_test_effect_blocked effect=launch_game"
                                        );
                                        return Err(Exit::Skip);
                                    }
                                    let Some(mra) = event.path else {
                                        continue;
                                    };
                                    let lifecycle_step = self.lib.lifecycle.handle(
                                        LauncherLifecycleInput::LaunchRequested {
                                            launch_ref: mra.clone(),
                                        },
                                        &mut self.lib.lifecycle_effects,
                                    );
                                    if !matches!(
                                        lifecycle_step.state,
                                        LauncherLifecycleState::Launching {
                                            phase: LaunchingPhase::LoadingFramePending { ref launch_ref },
                                        } if launch_ref == &mra
                                    ) {
                                        apply_lifecycle_effects(
                                            &mut self.lib.lifecycle_effects,
                                            &mut self.lib.scheduler,
                                            self.out.start,
                                        );
                                        continue;
                                    }
                                    if !self.lib.scheduler.begin_launch(
                                        &self.ui.nav,
                                        &self.lib.catalog,
                                        self.lib.catalog_generation.durable.as_deref(),
                                        &mra,
                                        Instant::now(),
                                    ) {
                                        self.lib.lifecycle.handle(
                                            LauncherLifecycleInput::LaunchFailed {
                                                title: launcher::game_title(
                                                    &self.lib.catalog,
                                                    &mra,
                                                ),
                                                kind: launcher::LaunchFailureKind::Internal,
                                                detail: "launch scheduler rejected request"
                                                    .to_string(),
                                            },
                                            &mut self.lib.lifecycle_effects,
                                        );
                                        apply_lifecycle_effects(
                                            &mut self.lib.lifecycle_effects,
                                            &mut self.lib.scheduler,
                                            self.out.start,
                                        );
                                        continue;
                                    }
                                    apply_lifecycle_effects(
                                        &mut self.lib.lifecycle_effects,
                                        &mut self.lib.scheduler,
                                        self.out.start,
                                    );
                                    sync_bridge_launcher(
                                        &self.env.app,
                                        &self.env.pad,
                                        &self.ui.nav,
                                        &self.lib.lifecycle,
                                        &self.inp.setup,
                                        self.lib.scheduler.launch_loading_title(),
                                        "",
                                        &self.lib.catalog,
                                        &mut self.lib.preview,
                                        &mut self.ui.bridge_models,
                                        self.lib.catalog_version,
                                        false,
                                        false,
                                        self.env.ui,
                                    );
                                    self.env.window.request_redraw();
                                    update_slint_animations(self.env.animation_clock);
                                    let _ = render_immediate_launcher_frame(
                                        self.env.window,
                                        self.env.target,
                                        self.out.layout,
                                    );
                                    let _pace = self.out.pacer.wait();
                                    copy_cached_rows_565(
                                        self.env.disp,
                                        self.env.target.cached_frame_view(),
                                        0,
                                        self.env.ui.render_h(),
                                    );
                                    let loading_presented = Instant::now();
                                    self.lib.lifecycle.loading_frame_presented(
                                        loading_presented,
                                        &mut self.lib.lifecycle_effects,
                                    );
                                    apply_lifecycle_effects(
                                        &mut self.lib.lifecycle_effects,
                                        &mut self.lib.scheduler,
                                        self.out.start,
                                    );
                                    self.env.window.request_redraw();
                                }
                            }
                            let nav_after = LauncherProjectionKey::from_nav(&self.ui.nav);
                            if nav_before != nav_after {
                                if let Some(entry) = self.lib.pending_collection_entry.take() {
                                    self.lib.preview.cancel_system_entry_preview();
                                    self.ui
                                        .nav
                                        .catalog_system_hydration_finished(&entry.collection_id);
                                    print_startup_event(
                                        self.out.start,
                                        "catalog_system_entry_cancelled",
                                        format!(
                                            "system={} reason=navigation-changed",
                                            entry.collection_id
                                        ),
                                    );
                                }
                                self.lib.media_session.note_nav_change(
                                    &nav_before,
                                    &nav_after,
                                    Instant::now(),
                                );
                            }
                            if pad_changed && self.ui.nav.screen == Screen::Controller {
                                pre_input.full_bridge_dirty = true;
                            }
                            if nav_before != nav_after {
                                if nav_before.screen == Screen::Home
                                    && nav_after.screen == Screen::Arcade
                                {
                                    self.diag.arcade_entry_latency.record_enter_input(
                                        self.out.start,
                                        frame_now,
                                        &self.lib.lifecycle,
                                        &self.lib.catalog,
                                        &self.ui.nav,
                                    );
                                    if !active_system_games_loading(&self.lib.catalog, &self.ui.nav)
                                        && let Some(system) =
                                            active_system(&self.lib.catalog, &self.ui.nav)
                                        && self.lib.catalog.system_game_count(&system.id) > 0
                                    {
                                        self.diag.arcade_entry_latency.record_rows_ready(
                                            self.out.start,
                                            frame_now,
                                            &self.lib.lifecycle,
                                            &self.lib.catalog,
                                            &self.ui.nav,
                                        );
                                    }
                                } else if nav_before.screen == Screen::Arcade
                                    && nav_after.screen == Screen::Arcade
                                    && arcade_selected_before_input != self.ui.nav.arcade.selected
                                {
                                    self.diag.arcade_entry_latency.record_first_nav_input(
                                        self.out.start,
                                        frame_now,
                                        &self.lib.lifecycle,
                                        &self.lib.catalog,
                                        &self.ui.nav,
                                    );
                                }
                                if nav_before.screen != nav_after.screen
                                    || nav_before.menu_id != nav_after.menu_id
                                {
                                    pre_input.full_bridge_dirty = true;
                                } else {
                                    pre_input.light_bridge_dirty = true;
                                }
                            }
                        }
                    }
                    let selection_feedback_after = discrete_selection_feedback_target(
                        &self.ui.nav,
                        &self.inp.setup,
                        &self.lib.lifecycle,
                    );
                    let feedback_surface_changed = self
                        .ui
                        .bridge_models
                        .sync_selection_feedback_surface(selection_feedback_after.as_ref());
                    let feedback_registered = selection_feedback_input
                        && self.ui.bridge_models.note_selection_feedback_change(
                            selection_feedback_before.as_ref(),
                            selection_feedback_after.as_ref(),
                        );
                    if feedback_surface_changed || feedback_registered {
                        pre_input.full_bridge_dirty = true;
                        self.env.window.request_redraw();
                    }
                    if final_input_tick {
                        break;
                    }
                    self.diag
                        .launcher_response_trace
                        .observe_state(&self.ui.nav, self.out.director.navigation.is_active());
                }
                self.diag
                    .input_integrity_trace
                    .flush_if_due(Instant::now(), &self.inp.input_router);

                if let Some(screen) = effective_lock_screen(
                    self.lib.lock_screen,
                    self.lib.catalog_ready,
                    &self.lib.catalog,
                ) {
                    self.ui.nav.screen = screen;
                }
            } else {
                if let Some(action) = self.lib.scheduler.launch_runtime_action(Instant::now()) {
                    match action {
                        LaunchHandoffRuntimeAction::ArcadeCoreRunning => {
                            crate::ui_logln!("arcade core running — handing off to MiSTer");
                            std::process::exit(0);
                        }
                        LaunchHandoffRuntimeAction::TimedOut => {
                            crate::ui_errln!("game launch timed out");
                            self.lib.lifecycle.handle(
                                LauncherLifecycleInput::LaunchTimedOut,
                                &mut self.lib.lifecycle_effects,
                            );
                            apply_lifecycle_effects(
                                &mut self.lib.lifecycle_effects,
                                &mut self.lib.scheduler,
                                self.out.start,
                            );
                            if self.lib.scheduler.stop_spawned_mister_for_recovery()
                                && let Err(e) = self
                                    .env
                                    .display_session
                                    .recover_after_launch_failure(self.out.frames, self.env.f)
                            {
                                crate::ui_errln!(
                                    "failed to recover Slint framebuffer route after launch timeout: {e}"
                                );
                            }
                            std::process::exit(1);
                        }
                    }
                }
            }

            self.diag.launcher_response_trace.record_lab(
                self.diag
                    .input_latency_lab
                    .arm_if_computers_ready(&self.ui.nav),
            );
            drop(input_route_pmu);
            record_launcher_frame_phase!(LauncherFramePhase::InputRouted);
            pre_input.scheduler_phase = self
                .diag
                .launcher_response_trace
                .record_scheduler_interval("input-route", pre_input.scheduler_phase);
            (false, input_batch_empty)
        };
        Ok(InputFrame {
            input_phase_yielded,
            input_batch_empty,
        })
    }

    /// Projects the result into the Slint models, runs background work, resolves composition and decides between idling and rendering.
    fn project(
        &mut self,
        #[cfg_attr(not(feature = "tooling"), allow(unused_variables, unused_mut))]
        begin: &mut BeginFrame,
        pre_input: &mut PreInputFrame,
        input: &mut InputFrame,
    ) -> Result<ProjectFrame, Exit> {
        if input.input_phase_yielded {
            return Err(Exit::Skip);
        }
        let interaction_projection_pmu = self.diag.launcher_response_trace.input_pmu_span(
            self.inp.latency_critical_input_pending,
            "launcher-response.interaction-projection",
        );

        if empty_collection_invariant_violated(&self.lib.catalog, &self.ui.nav)
            && !self
                .lib
                .launch_return_session
                .protects_hydrating_collection(&self.ui.nav)
        {
            if let Some(system) = active_system(&self.lib.catalog, &self.ui.nav) {
                crate::ui_errln!(
                    "catalog presentation invariant recovered: system={} registered_rows={} resident_rows=0",
                    system.id,
                    system.count
                );
                runtime_status::event(
                    "catalog_empty_list_invariant",
                    format!("system={} registered_rows={}", system.id, system.count),
                );
            }
            if let Some(system) = active_system(&self.lib.catalog, &self.ui.nav) {
                let id = system.id.clone();
                self.ui.nav.catalog_system_hydration_failed(&id);
            }
            pre_input.full_bridge_dirty = true;
            self.env.window.request_redraw();
        }

        self.diag.bridge_churn_playback.apply(
            self.diag.gui_profiling.phase(),
            &self.env.app,
            &self.ui.nav,
            &self.ui.bridge_models,
            &mut pre_input.full_bridge_dirty,
            &mut pre_input.light_bridge_dirty,
        );
        let startup_intro_launcher_ui_plan = startup_intro_launcher_ui_plan(
            self.fx.startup_intro.is_some(),
            self.lib.lifecycle.startup_status().state,
            self.fx.startup_intro_launcher_frame_ready,
        );
        let startup_intro_prepare_live_launcher =
            startup_intro_launcher_ui_plan == StartupIntroLauncherUiPlan::PrepareLiveFrame;
        let startup_intro_suppress_launcher_ui =
            startup_intro_launcher_ui_plan == StartupIntroLauncherUiPlan::Suppress;
        let startup_reveal_suppress_launcher_ui =
            self.fx.startup_intro.is_none() && !self.lib.lifecycle.startup_can_present_frame();
        if startup_intro_suppress_launcher_ui {
            self.fx.startup_intro_bridge_dirty_pending |=
                pre_input.full_bridge_dirty || pre_input.light_bridge_dirty;
            pre_input.full_bridge_dirty = false;
            pre_input.light_bridge_dirty = false;
        } else {
            if std::mem::take(&mut self.fx.startup_intro_bridge_dirty_pending)
                || startup_intro_prepare_live_launcher
            {
                pre_input.full_bridge_dirty = true;
            }
            if startup_intro_prepare_live_launcher {
                LauncherStatusPresenter::new(&self.env.app).clear_catalog_scan();
                let clock_text = launcher_clock_text();
                set_launcher_clock_text(&self.env.app, &clock_text);
                self.inp.last_clock_text = clock_text;
                self.inp.last_clock_update = Instant::now();
                self.env.window.request_redraw();
            }
            sync_settings_bridge(
                &self.env.app,
                &self.ui.nav,
                &self.lib.lifecycle,
                self.env.ui,
                &mut self.ui.bridge_models,
            );
        }
        let source_was_arcade = self
            .out
            .director
            .pending
            .as_ref()
            .is_some_and(|pending| pending.source_was_arcade);
        let preserve_navigation_source_preview =
            self.out.director.navigation.is_active() && source_was_arcade;
        let defer_or_preserve_selected_preview = should_defer_or_preserve_selected_preview(
            pre_input.defer_selected_preview,
            self.out.director.navigation.is_active(),
            source_was_arcade,
        );
        let bridge_sync_plan = launcher_bridge_sync_plan(
            pre_input.launching,
            pre_input.full_bridge_dirty,
            pre_input.light_bridge_dirty,
        );
        let bridge_sync_started =
            (bridge_sync_plan != LauncherBridgeSyncPlan::None).then(Instant::now);
        let gui_bridge_phase = gui_bridge_profile_phase(
            bridge_sync_plan == LauncherBridgeSyncPlan::Full,
            bridge_sync_plan == LauncherBridgeSyncPlan::Light,
        );
        let gui_bridge_pmu = self
            .diag
            .gui_profiling
            .phase_span(gui_bridge_phase.span_name());
        let mut bridge_model_projection_us = 0u128;
        #[cfg(feature = "tooling")]
        let mut bridge_stage_us = None;
        #[cfg(feature = "tooling")]
        let mut bridge_presenter = None;
        let measure_bridge = self.diag.system_entry_cpu_profile.is_some() || {
            #[cfg(feature = "tooling")]
            {
                begin
                    .tooling_frame_evidence
                    .as_ref()
                    .is_some_and(|frame| frame.phases_enabled)
            }
            #[cfg(not(feature = "tooling"))]
            {
                false
            }
        };
        match bridge_sync_plan {
            LauncherBridgeSyncPlan::Full => {
                #[cfg(feature = "tooling")]
                let _profile =
                    mister_magik_framebuffer_scenes::launcher_profile::span("bridge.full-sync");
                let timing = sync_bridge_launcher(
                    &self.env.app,
                    &self.env.pad,
                    &self.ui.nav,
                    &self.lib.lifecycle,
                    &self.inp.setup,
                    self.lib
                        .scheduler
                        .visible_loading_title(&self.lib.loading_title),
                    "",
                    &self.lib.catalog,
                    &mut self.lib.preview,
                    &mut self.ui.bridge_models,
                    self.lib.catalog_version,
                    defer_or_preserve_selected_preview,
                    measure_bridge,
                    self.env.ui,
                );
                bridge_model_projection_us = timing.model_projection_us;
                #[cfg(feature = "tooling")]
                {
                    bridge_stage_us = timing.stage_us;
                    bridge_presenter = timing.presenter;
                }
                pre_input.preview_scheduled_this_loop = self.ui.nav.screen == Screen::Arcade
                    && self.ui.preview_route.allows_hdmi_preview();
                self.env.window.request_redraw();
            }
            LauncherBridgeSyncPlan::Light => {
                let active_games = if self.ui.nav.screen == Screen::Arcade {
                    Some(active_system_game_view(&self.lib.catalog, &self.ui.nav))
                } else {
                    None
                };
                let timing = sync_bridge_launcher_light(
                    &self.env.app,
                    &self.ui.nav,
                    &self.lib.lifecycle,
                    &mut self.ui.bridge_models,
                    self.lib
                        .scheduler
                        .visible_loading_title(&self.lib.loading_title),
                    "",
                    &self.lib.catalog,
                    active_games,
                    &mut self.lib.preview,
                    should_defer_arcade_overlay_bridge(
                        pre_input.launching,
                        &self.ui.nav,
                        &self.lib.catalog,
                    ),
                    defer_or_preserve_selected_preview,
                    measure_bridge,
                    self.env.ui,
                );
                bridge_model_projection_us = timing.model_projection_us;
                #[cfg(feature = "tooling")]
                {
                    bridge_stage_us = timing.stage_us;
                    bridge_presenter = timing.presenter;
                }
                pre_input.preview_scheduled_this_loop = self.ui.nav.screen == Screen::Arcade
                    && self.ui.preview_route.allows_hdmi_preview();
                self.env.window.request_redraw();
            }
            LauncherBridgeSyncPlan::None => {}
        }
        drop(gui_bridge_pmu);
        pre_input.prepare_trace.bridge_sync_us = bridge_sync_started
            .map(|started| started.elapsed().as_micros())
            .unwrap_or(0);
        pre_input.prepare_trace.bridge_model_projection_us = bridge_model_projection_us;
        let bridge_churn_delta = crate::launcher_presentation::bridge_churn_snapshot()
            .saturating_sub(pre_input.bridge_churn_frame_start);
        pre_input.prepare_trace.bridge_model_replacements = bridge_churn_delta.model_replacements;
        pre_input.prepare_trace.bridge_row_mutations = bridge_churn_delta.row_mutations;
        pre_input.prepare_trace.bridge_row_allocations = bridge_churn_delta.row_allocations;
        pre_input.prepare_trace.bridge_shared_string_constructions =
            bridge_churn_delta.shared_string_constructions;
        pre_input.prepare_trace.bridge_model_allocation_us = bridge_churn_delta.model_allocation_us;
        #[cfg(feature = "tooling")]
        if let Some(frame) = begin.tooling_frame_evidence.as_mut() {
            frame.bridge_us = u128_to_u64(pre_input.prepare_trace.bridge_sync_us);
            frame.bridge_model_us = (measure_bridge
                && bridge_sync_plan != LauncherBridgeSyncPlan::None)
                .then(|| u128_to_u64(pre_input.prepare_trace.bridge_model_projection_us));
            frame.bridge_stages_us = bridge_stage_us;
            frame.bridge_presenter_us = bridge_presenter.map(|timing| timing.stages_us);
            frame.bridge_hub_counts_us = bridge_presenter.map(|timing| timing.hub_counts_us);
            frame.bridge_counters_enabled = bridge_presenter.is_some();
            frame.bridge_allocation_us = pre_input.prepare_trace.bridge_model_allocation_us;
            frame.bridge_models_replaced = pre_input.prepare_trace.bridge_model_replacements;
        }
        let response_projected_at_us = crate::input_hub::monotonic_us();
        let response_projected_execution = self.diag.launcher_response_trace.execution_stamp();
        drop(interaction_projection_pmu);
        pre_input.scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("interaction-projection", pre_input.scheduler_phase);

        let media_gate_trace_start = pre_input.prepare_trace_enabled.then(Instant::now);
        if pre_input.background_work_allowed {
            let visible_media_system_id = matches!(self.ui.nav.screen, Screen::Arcade)
                .then(|| {
                    self.ui
                        .nav
                        .active_collection()
                        .map(|collection| {
                            collection
                                .system_id
                                .as_deref()
                                .unwrap_or(&collection.legacy_system_id)
                        })
                        .unwrap_or_else(|| {
                            self.ui.nav.active_collection_scope_id(&self.lib.catalog)
                        })
                })
                .filter(|system_id| !system_id.is_empty())
                .map(str::to_string);
            self.lib
                .media_session
                .observe_system_entry(visible_media_system_id.as_deref());
            let media_gate = self.lib.media_session.current_gate(
                self.diag.frame_accounting.first_visible_copy_done(),
                self.lib.scheduler.has_pending_launch() || pre_input.launching,
                pre_input.loop_start,
            );
            let media_gate = if self.lib.memory_guard.active() {
                MediaInteractionGate {
                    active: true,
                    reason: "low-memory",
                }
            } else {
                media_gate
            };
            let media_gate =
                catalog_build_media_gate(self.lib.catalog_session.refresh_done(), media_gate);
            apply_screenshot_media_update_effects(
                self.lib.media_session.sync_gate(media_gate),
                &self.env.app,
                &mut self.lib.catalog,
                &mut self.lib.scheduler,
                Some(&mut self.lib.preview),
                &mut pre_input.full_bridge_dirty,
                self.out.start,
            );
            apply_screenshot_media_update_effects(
                self.lib.media_session.apply_gate(media_gate),
                &self.env.app,
                &mut self.lib.catalog,
                &mut self.lib.scheduler,
                Some(&mut self.lib.preview),
                &mut pre_input.full_bridge_dirty,
                self.out.start,
            );
            apply_screenshot_media_update_effects(
                self.lib.media_session.sync_gate(media_gate),
                &self.env.app,
                &mut self.lib.catalog,
                &mut self.lib.scheduler,
                Some(&mut self.lib.preview),
                &mut pre_input.full_bridge_dirty,
                self.out.start,
            );
        }
        if let Some(trace_start) = media_gate_trace_start {
            pre_input.prepare_trace.media_gate_us = trace_start.elapsed().as_micros();
        }

        let catalog_view = self.env.app.global::<slint_ui::launcher::CatalogView>();
        let overlay_view = self.env.app.global::<slint_ui::launcher::OverlayView>();
        let catalog_scan_visible =
            catalog_view.get_activity() == slint_ui::launcher::CatalogActivity::Foreground;
        let catalog_scan_percent = catalog_view.get_percent();
        let catalog_background_scan_visible = catalog_view.get_background_activity_visible();
        if let Some(dot_visible) = self
            .ui
            .catalog_scan_blink
            .update(catalog_scan_visible, pre_input.animation_now)
        {
            catalog_view.set_progress_dot_visible(dot_visible);
            self.env.window.request_redraw();
        }
        let overlay_occlusion = crate::launcher_presentation::SlintOverlayOcclusion::read(
            &self.env.app,
            self.inp.setup.is_active(),
        );
        let confirm_visible = overlay_occlusion.confirm;
        let fullscreen_overlay_visible = overlay_occlusion.fullscreen;
        let confirm_selected =
            if overlay_view.get_selected_choice() == slint_ui::launcher::DialogChoice::Cancel {
                0
            } else {
                1
            };
        // The one-second status publication (its serialization worker shares
        // CPU1) yields to motion without consuming its deadline.
        let status_write_due = self.diag.frame_accounting.status_write_due()
            && self.lib.status_write_deferral.allows(pre_input.loop_start);
        let status_snapshot_due = status_write_due
            && !self.out.director.navigation.is_active()
            && self.out.director.chart.is_live();
        let status_string_copy_start = (status_snapshot_due
            && self.diag.frame_accounting.preview_scroll_trace_enabled())
        .then(Instant::now);
        let status_text = status_snapshot_due
            .then(|| LauncherStatusTextSnapshot::from_views(&catalog_view, &overlay_view));
        let status_string_copy_us = status_string_copy_start
            .map(|start_| start_.elapsed().as_micros())
            .unwrap_or(0);
        pre_input.prepare_trace.status_string_copy_us = status_string_copy_us;
        let status_string_copy_bytes = status_text
            .as_ref()
            .map(LauncherStatusTextSnapshot::bytes_len)
            .unwrap_or(0);
        if pre_input.launching {
            self.env.window.request_redraw();
        }
        let active_arcade_games = if !pre_input.launching && self.ui.nav.screen == Screen::Arcade {
            active_system_game_view(&self.lib.catalog, &self.ui.nav)
        } else {
            ArcadeGameView::empty()
        };
        let active_arcade_games_available = !active_arcade_games.is_empty();
        let arcade_status_only =
            crate::launcher_presentation::active_games_load_state(&self.lib.catalog, &self.ui.nav)
                != slint_ui::launcher::ArcadeLoadState::Ready;
        let arcade_search_active = self
            .ui
            .nav
            .arcade_search
            .is_active(&self.ui.nav.arcade_filter.active);
        if !pre_input.launching
            && self.ui.nav.screen == Screen::Arcade
            && let Some(system) = active_system(&self.lib.catalog, &self.ui.nav)
        {
            let trace_system_id = &system.legacy_system_id;
            if self
                .lib
                .preview_systems_entered
                .insert(trace_system_id.clone())
            {
                crate::ui_logln!(
                    "startup_timing\tpreview_system_entered\t{}ms\tsystem={}\tselected_index={}",
                    self.out.start.elapsed().as_millis(),
                    trace_system_id,
                    self.ui.nav.arcade.selected
                );
            }
            if active_arcade_games_available
                && self
                    .lib
                    .preview_initial_lists_ready
                    .insert(trace_system_id.clone())
            {
                self.diag.arcade_entry_latency.record_rows_ready(
                    self.out.start,
                    Instant::now(),
                    &self.lib.lifecycle,
                    &self.lib.catalog,
                    &self.ui.nav,
                );
                let selected = self
                    .ui
                    .nav
                    .arcade
                    .selected
                    .min(active_arcade_games.len() - 1);
                if let Some(game) = active_arcade_games.get(selected) {
                    crate::ui_logln!(
                        "startup_timing\tpreview_initial_list_ready\t{}ms\tsystem={}\tselected_index={}\ttitle={}\thas_preview={}\tasset_key={}",
                        self.out.start.elapsed().as_millis(),
                        trace_system_id,
                        selected,
                        game.title,
                        if game.has_preview { 1 } else { 0 },
                        game.preview_asset_key
                    );
                } else {
                    crate::ui_logln!(
                        "startup_timing\tpreview_initial_list_ready\t{}ms\tsystem={}\tselected_index={}\ttitle=\thas_preview=0\tasset_key=",
                        self.out.start.elapsed().as_millis(),
                        trace_system_id,
                        selected
                    );
                }
            }
        }
        let arcade_scroll_active =
            self.ui.nav.screen == Screen::Arcade && self.ui.nav.arcade.is_scroll_active();
        let arcade_turbo_active =
            self.ui.nav.screen == Screen::Arcade && self.ui.nav.arcade.is_turbo_active();
        let preview_work_allowed = preview_work_allowed(
            pre_input.background_work_allowed,
            self.diag
                .arcade_entry_latency
                .preview_adoption_in_progress(),
            arcade_scroll_active,
            arcade_turbo_active,
        );
        let preview_schedule_trace_start = pre_input.prepare_trace_enabled.then(Instant::now);
        if preview_work_allowed
            && !pre_input.preview_scheduled_this_loop
            && !pre_input.launching
            && self.ui.nav.screen == Screen::Arcade
            && active_arcade_games_available
            && !arcade_search_active
            && !self.lib.memory_guard.active()
        {
            let bridge = self.env.app.global::<slint_ui::launcher::ArcadeView>();
            if schedule_arcade_preview_window(
                &bridge,
                active_arcade_games,
                self.ui.nav.arcade.selected,
                &mut self.lib.preview,
                defer_or_preserve_selected_preview,
                arcade_scroll_active,
                arcade_turbo_active,
            ) {
                self.env.window.request_redraw();
            }
        }
        if let Some(trace_start) = preview_schedule_trace_start {
            pre_input.prepare_trace.preview_schedule_us = trace_start.elapsed().as_micros();
        }
        let preview_apply_trace_start = pre_input.prepare_trace_enabled.then(Instant::now);
        let mut preview_apply_trace = PreviewApplyTrace::default();
        let preview_apply_dirty = if !pre_input.launching
            && preview_work_allowed
            && !arcade_search_active
            && !self.lib.memory_guard.active()
        {
            let dirty = apply_ready_preview(
                &self.env.app,
                &mut self.lib.preview,
                defer_or_preserve_selected_preview,
                arcade_turbo_active,
            );
            preview_apply_trace = self.lib.preview.last_apply_trace();
            dirty
        } else {
            false
        };
        if preview_apply_dirty {
            self.env.window.request_redraw();
        }
        if let Some(trace_start) = preview_apply_trace_start {
            pre_input.prepare_trace.preview_apply_us = trace_start.elapsed().as_micros();
        }
        pre_input.prepare_trace.preview_worker_drained = preview_apply_trace.worker_drained;
        pre_input.prepare_trace.preview_ready_processed = preview_apply_trace.ready_processed;
        pre_input.prepare_trace.preview_selected_processed = preview_apply_trace.selected_processed;
        pre_input.prepare_trace.preview_prefetch_processed = preview_apply_trace.prefetch_processed;
        pre_input.prepare_trace.preview_stale_results = preview_apply_trace.stale_results;
        pre_input.prepare_trace.preview_cache_inserts = preview_apply_trace.cache_inserts;
        pre_input.prepare_trace.preview_cache_evictions =
            self.lib.preview.take_frame_cache_evictions();
        pre_input.prepare_trace.preview_failed_results = preview_apply_trace.failed_results;
        pre_input.prepare_trace.preview_backlog = preview_apply_trace.backlog_len;
        self.diag.arcade_entry_latency.record_preview_exact(
            self.out.start,
            Instant::now(),
            &self.lib.lifecycle,
            &self.lib.catalog,
            &self.ui.nav,
            &self.lib.preview,
        );
        maybe_mark_return_preview_ready(
            &mut self.lib.lifecycle,
            &mut self.lib.lifecycle_effects,
            &self.ui.nav,
            &self.lib.catalog,
            &self.lib.preview,
            &mut self.lib.launch_return_session,
        );
        apply_lifecycle_effects(
            &mut self.lib.lifecycle_effects,
            &mut self.lib.scheduler,
            self.out.start,
        );
        let startup_reveal_ready =
            self.lib.lifecycle.startup_status().state == StartupRevealState::RevealLauncher;
        pre_input.effective_view = EffectiveLauncherView::resolve(
            &self.lib.lifecycle,
            self.fx.screensaver.active,
            self.ui.nav.screen,
        );
        if pre_input.effective_view.launch_active()
            && self
                .fx
                .screensaver
                .cancel_for_exclusive_view(Instant::now())
        {
            pre_input.effective_view = EffectiveLauncherView::Launching;
            self.env.window.request_redraw();
        }
        pre_input.launching = pre_input.effective_view.launch_active();
        self.diag
            .frame_accounting
            .set_effective_view(pre_input.effective_view.label());
        self.diag
            .frame_accounting
            .set_catalog_generation(self.lib.catalog_generation.current.as_deref());
        let mut full_frame_present = std::mem::take(&mut self.out.orientation_full_redraw_pending)
            || std::mem::take(&mut self.out.unpublished_cached_frame_present)
            || self
                .env
                .display_session
                .should_present_full_frame(pre_input.launching, pre_input.route_action)
            || startup_reveal_ready;
        let wants_arcade_list = !self.fx.screensaver.active
            && !arcade_status_only
            && should_draw_arcade_overlay(
                &self.ui.nav,
                pre_input.launching,
                active_arcade_games_available,
            );
        let presentation_route = if preserve_navigation_source_preview {
            PreviewRoute::Occluded
        } else if self.ui.nav.screen == Screen::Arcade
            && !self.lib.memory_guard.active()
            && !self.fx.screensaver.active
            && !confirm_visible
            && !fullscreen_overlay_visible
            && !self
                .ui
                .nav
                .arcade_search
                .is_active(&self.ui.nav.arcade_filter.active)
        {
            PreviewRoute::Eligible
        } else {
            PreviewRoute::Unavailable
        };
        self.lib.preview.set_route(presentation_route);
        let crt_backdrop_eligible = self.ui.preview_route.allows_crt_backdrop()
            && presentation_route == PreviewRoute::Eligible
            && (wants_arcade_list || self.ui.nav.is_system_hub())
            && !self.ui.nav.arcade_filter.drawer_open;
        let crt_backdrop_was_eligible = self
            .out
            .crt_backdrop
            .as_ref()
            .is_some_and(CrtBackdropController::was_eligible);
        let crt_backdrop_leaving = crt_backdrop_was_eligible && !crt_backdrop_eligible;
        if crt_backdrop_leaving {
            full_frame_present = true;
            self.env.window.request_redraw();
        }
        let preview_frame_intent = self.lib.preview.frame_intent();
        let wants_preview_layer =
            self.ui.preview_route.allows_hdmi_preview() && self.lib.preview.direct_layer_desired();
        let wants_preview = self.ui.preview_route.allows_hdmi_preview()
            && !self.fx.screensaver.active
            && !self
                .ui
                .nav
                .arcade_search
                .is_active(&self.ui.nav.arcade_filter.active)
            && direct_preview_requested(
                self.ui.nav.screen,
                self.lib.memory_guard.active(),
                wants_preview_layer
                    || matches!(preview_frame_intent, PreviewFrameIntent::Present { .. }),
            );
        let preview_frame_status = self.lib.preview.raw_frame_status();
        let preview_cache_state_before_composition = self.lib.preview.trace_cache_state();
        if self.out.director.navigation.is_active()
            && (pre_input.effective_view == EffectiveLauncherView::Screensaver
                || confirm_visible
                || catalog_scan_visible)
        {
            let endpoint = self.out.director.cover_navigation();
            if endpoint == Some(NavigationTransitionEndpoint::Source)
                && let Some(entry) = self.lib.pending_collection_entry.take()
            {
                self.lib.preview.cancel_system_entry_preview();
                self.lib.deferred_navigation_hydration_finish = Some(entry.collection_id);
                self.diag.arcade_entry_latency.cancel_enter();
            }
        }
        let navigation_destination_committed = self.out.director.destination_committed();
        // The list is the navigation destination. Preview media is asynchronous and
        // must never hold the full-screen transition closed after the list is ready.
        let navigation_destination_layers_ready = navigation_destination_committed
            && (self.ui.nav.screen != Screen::Arcade
                || active_arcade_games_available
                || self.ui.nav.active_collection_id()
                    == Some(arcade_catalog::MENU_ARCADE_SYSTEM_ID));
        let composition_decision = self.out.director.compose(CompositionRequest {
            screensaver_active: pre_input.effective_view == EffectiveLauncherView::Screensaver,
            navigation_destination_layers_ready,
            return_screen: pre_input.effective_view.return_screen(),
            confirm_visible,
            fullscreen_overlay_visible,
            arcade_ready: active_arcade_games_available
                || self.ui.nav.active_collection_id()
                    == Some(arcade_catalog::MENU_ARCADE_SYSTEM_ID),
            route_ok: self.env.display_session.route_ok(),
            wants_arcade_list,
            wants_preview: wants_preview_layer,
            preview_cache_exact: preview_cache_state_before_composition == "exact",
            preview_frame_ready: preview_frame_status == PreviewRawFrameStatus::Ready,
        });
        if self.fx.screensaver.active {
            full_frame_present = true;
            self.env.window.request_redraw();
        } else if self.fx.screensaver.start_mode != ScreensaverStartMode::Inactive {
            self.env.window.request_redraw();
        }
        for event in composition_decision.events.iter() {
            runtime_status::event(event.name, event.detail.as_str());
        }
        if !startup_intro_suppress_launcher_ui {
            sync_navigation_transition_active(&self.env.app, &self.out.director.navigation);
        }
        self.diag
            .launcher_response_trace
            .observe_state(&self.ui.nav, self.out.director.navigation.is_active());
        if composition_decision.force_full_slint_present {
            full_frame_present = true;
        }
        if composition_decision.force_full_slint_raster {
            self.env.window.request_redraw();
        }
        if composition_decision.clears_arcade_layer() {
            self.out.arcade_list_renderer.invalidate_presented_layer();
            self.env.window.request_redraw();
        }
        let startup_status = self.lib.lifecycle.startup_status();
        let mut composition_status = composition_decision.status();
        composition_status.preview_state = self.lib.preview.presentation_label();
        composition_status.preview_generation = self.lib.preview.presentation_generation();
        let automation_frame_stamp = if self.diag.launcher_automation.active() {
            let selected_system_id = self.ui.nav.active_collection_scope_id(&self.lib.catalog);
            let selected_game = (self.ui.nav.screen == Screen::Arcade)
                .then(|| {
                    self.ui.nav.active_arcade_game_at(
                        &self.lib.catalog,
                        selected_system_id,
                        self.ui.nav.arcade.selected,
                    )
                })
                .flatten();
            self.diag
                .launcher_automation
                .observe_state(AutomationSemanticState {
                    screen_orientation: self.ui.nav.settings.screen_orientation.label().to_string(),
                    output_route: self.env.ui.output_route().label().to_string(),
                    output_width: self.env.ui.output_w(),
                    output_height: self.env.ui.output_h(),
                    render_width: self.env.ui.render_w(),
                    render_height: self.env.ui.render_h(),
                    effective_view: pre_input.effective_view.label().to_string(),
                    return_screen: screen_label(self.ui.nav.screen).to_string(),
                    menu_id: self.ui.nav.current_menu_id().to_string(),
                    selected_item_id: self.ui.nav.current_menu_selected_item_id().to_string(),
                    active_collection_id: self
                        .ui
                        .nav
                        .active_collection_id()
                        .unwrap_or("")
                        .to_string(),
                    selected_system_id: selected_system_id.to_string(),
                    selected_game_id: selected_game
                        .map_or("", |game| game.mra_path.as_ref())
                        .to_string(),
                    selected_game_title: selected_game
                        .map_or("", |game| game.title.as_ref())
                        .to_string(),
                    selected_index: if self.ui.nav.screen == Screen::Arcade {
                        self.ui.nav.arcade.selected
                    } else {
                        self.ui.nav.selected
                    },
                    selected_count: if self.ui.nav.screen == Screen::Arcade {
                        self.ui
                            .nav
                            .active_arcade_game_count(&self.lib.catalog, selected_system_id)
                    } else {
                        self.ui.nav.current_menu_count()
                    },
                    overlay: if confirm_visible {
                        "confirm"
                    } else if catalog_scan_visible {
                        "catalog-scan"
                    } else if self.inp.setup.is_active() {
                        "controller-setup"
                    } else {
                        "none"
                    }
                    .to_string(),
                    dialog_title: overlay_view.get_confirmation_title().to_string(),
                    dialog_message: overlay_view.get_confirmation_message().to_string(),
                    dialog_selected: confirm_selected,
                    drawer_open: self.ui.nav.arcade_filter.drawer_open,
                    drawer_level: self.ui.nav.arcade_filter.title().to_string(),
                    drawer_selected: self.ui.nav.arcade_filter.selected,
                    search_active: self
                        .ui
                        .nav
                        .arcade_search
                        .is_active(&self.ui.nav.arcade_filter.active),
                    search_status: match self.ui.nav.arcade_search.status {
                        launcher::ArcadeSearchStatus::Idle => "idle",
                        launcher::ArcadeSearchStatus::Searching => "searching",
                        launcher::ArcadeSearchStatus::Ready => "ready",
                        launcher::ArcadeSearchStatus::Failed => "failed",
                    }
                    .to_string(),
                    search_query: self.ui.nav.arcade_search.query.clone(),
                    search_results: self.ui.nav.arcade_search_result_count(),
                    preview_state: self.lib.preview.trace_cache_state().to_string(),
                    launch_state: if pre_input.launching {
                        "launching"
                    } else {
                        "idle"
                    }
                    .to_string(),
                    loading_title: self
                        .lib
                        .scheduler
                        .visible_loading_title(&self.lib.loading_title)
                        .to_string(),
                    catalog_generation: self
                        .lib
                        .catalog_generation
                        .current
                        .clone()
                        .unwrap_or_default(),
                    catalog_ready: self.lib.catalog_ready,
                    settings_selected: self.ui.nav.settings_selected,
                    composition_state: composition_status.state.to_string(),
                    composition_recovery_count: composition_status.recovery_count,
                    navigation_transition_active: self.out.director.navigation.is_active(),
                    input_enabled: startup_status.input_enabled,
                })
        } else {
            AutomationFrameStamp::default()
        };
        // The exposed fixed HDMI device plane can bypass RGB8 image rasterization.
        let native_device_base = self.ui.nav.screen == Screen::Arcade
            && !self.out.layout.is_portrait()
            && !self.env.ui.output_route().is_crt()
            && (self.out.layout.logical_w(), self.out.layout.logical_h()) == (960, 540)
            && !overlay_occlusion.any()
            && !self.fx.screensaver.active
            && !pre_input.launching
            && self
                .env
                .app
                .global::<slint_ui::launcher::SettingsView>()
                .get_popup()
                == slint_ui::launcher::SettingsPopup::None;
        if native_device_base && self.ui.nav.is_system_hub() {
            configure_arcade_list_renderer_geometry(
                &mut self.out.arcade_list_renderer,
                &self.ui.nav,
                self.env.ui,
            );
            self.out.arcade_list_renderer.prepare_visible_rows(
                active_system_game_view(&self.lib.catalog, &self.ui.nav),
                self.ui.nav.arcade.visual_index,
            );
        }
        let global = self.env.app.global::<slint_ui::launcher::MisterUi>();
        if global.get_custom_device_base() != native_device_base {
            global.set_custom_device_base(native_device_base);
            self.out.native_device_background.invalidate();
        }
        let card_frame_just_rendered =
            std::mem::take(&mut self.fx.card_frame_rendered_last_iteration);
        // Every Home level is the Rust card launcher, not only the root.
        let custom_home_active =
            self.fx.launcher_card_home.is_some() && self.ui.nav.screen == Screen::Home;
        self.env
            .app
            .global::<slint_ui::launcher::MisterUi>()
            .set_custom_home_base(custom_home_active);
        if custom_home_active {
            if let Some(session) = self.fx.launcher_card_home.as_mut() {
                let (predicted_selected, predicted_visual_index) =
                    (self.ui.nav.selected, self.ui.nav.home_card_visual_index());
                if !self
                    .lib
                    .card_level
                    .matches_runtime(&self.ui.nav, &self.lib.catalog)
                {
                    self.lib.card_level = crate::launcher_home::CardLevelSnapshot::from_runtime(
                        &self.ui.nav,
                        &self.lib.catalog,
                    );
                }
                let accepted = session.update_from_navigation(
                    super::launcher_card_home::scene_for_display(self.env.ui, self.out.layout),
                    &self.lib.card_level,
                    predicted_selected,
                    predicted_visual_index,
                    &self.inp.last_clock_text,
                    pre_input.animation_us / 1_000,
                    !self.ui.nav.settings.reduce_motion,
                    self.ui
                        .nav
                        .home_card_browse_prediction(pre_input.animation_now),
                    self.ui.nav.home_level_transition(),
                );
                if accepted {
                    self.ui.nav.acknowledge_home_level_transition();
                }
                // Idle on a card: prepare the level it opens and the parent, so
                // the level trick never waits on preparation. The worker shares
                // CPU0 with the card helper; start only once card frames stop,
                // or it preempts the helper while the settle frame renders.
                if !session.is_animating()
                    && !card_frame_just_rendered
                    && (self.lib.card_prefetch_key.0 != self.ui.nav.current_menu_id()
                        || self.lib.card_prefetch_key.1 != self.ui.nav.selected)
                {
                    self.lib.card_prefetch_key = (
                        self.ui.nav.current_menu_id().to_owned(),
                        self.ui.nav.selected,
                    );
                    let levels = [
                        self.ui.nav.selected_child_menu_id(),
                        self.ui.nav.parent_menu_id(),
                    ]
                    .into_iter()
                    .flatten()
                    .map(|id| {
                        crate::launcher_home::CardLevelSnapshot::for_menu(
                            &self.ui.nav,
                            &self.lib.catalog,
                            id,
                        )
                    })
                    .collect();
                    session.prefetch(levels);
                }
            }
        } else if let Some(session) = self.fx.launcher_card_home.as_mut() {
            session.set_inactive();
        }
        let custom_home_scene_ready = self.fx.launcher_card_home.as_ref().is_some_and(|session| {
            session.scene_ready(super::launcher_card_home::scene_for_display(
                self.env.ui,
                self.out.layout,
            ))
        });
        let custom_home_needs_render = self
            .fx
            .launcher_card_home
            .as_ref()
            .is_some_and(super::launcher_card_home::LauncherCardHomeSession::needs_render);
        let custom_home_motion_active = self
            .fx
            .launcher_card_home
            .as_ref()
            .is_some_and(super::launcher_card_home::LauncherCardHomeSession::is_animating);
        let home_pan_present_active = update_home_pan_present_window(
            self.ui.nav.screen,
            self.ui.nav.scroll_x,
            &mut self.out.last_home_pan_scroll_x,
            &mut self.out.home_pan_present_until,
            pre_input.loop_start,
        ) || custom_home_motion_active;
        let home_horizontal_input_held = self.ui.nav.screen == Screen::Home
            && pad_state_home_horizontal_held(self.env.pad.state());
        if home_frame_driven_redraw_active(
            self.ui.nav.screen,
            home_pan_present_active,
            home_horizontal_input_held,
        ) || custom_home_needs_render
        {
            self.env.window.request_redraw();
        }
        if self.ui.nav.licenses_scroll_active() {
            self.env.window.request_redraw();
        }
        let arcade_visual_changed_this_loop = self.ui.nav.arcade.visual_index
            != pre_input.arcade_visual_index_at_loop_start
            || self.ui.nav.arcade_filter.visual_index
                != pre_input.arcade_filter_visual_index_at_loop_start;
        let stream_motion_before_render = self.out.director.navigation.is_active()
            || pre_input.slint_animation_active
            || home_pan_present_active
            || home_horizontal_input_held
            || self.ui.nav.licenses_scroll_active()
            || arcade_visual_changed_this_loop
            || (self.ui.nav.screen == Screen::Arcade && self.ui.nav.arcade.is_scroll_active())
            || (self.ui.nav.screen == Screen::Arcade
                && self.ui.nav.arcade_filter.drawer_open
                && self.ui.nav.arcade_filter.is_scroll_active());
        if !stream_motion_before_render {
            let _ = self
                .out
                .launcher_presenter
                .publish_stream_refinement_if_due();
        }
        let crt_backdrop_prepared = self
            .out
            .crt_backdrop
            .as_mut()
            .is_some_and(CrtBackdropController::poll);
        let mut wake_reasons = LauncherWakeReasons::default();
        wake_reasons.insert_if(
            LauncherWakeReasons::REDRAW_PENDING,
            self.env.window.redraw_pending(),
        );
        wake_reasons.insert_if(LauncherWakeReasons::LAUNCHING, pre_input.launching);
        wake_reasons.insert_if(LauncherWakeReasons::SETUP_ACTIVE, pre_input.setup_active);
        #[cfg(feature = "tooling")]
        let tooling_sequence_pending = self
            .diag
            .tooling
            .as_ref()
            .is_some_and(mister_magik_tooling_support::Session::carousel_sequence_pending);
        #[cfg(not(feature = "tooling"))]
        let tooling_sequence_pending = false;
        wake_reasons.insert_if(
            LauncherWakeReasons::SCRIPTED_INPUT_ACTIVE,
            self.diag.launcher_automation.active() || tooling_sequence_pending,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::ROUTE_FORCES_FULL_PRESENT,
            pre_input.route_action.force_full_present,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::BRIDGE_DIRTY,
            pre_input.full_bridge_dirty || pre_input.light_bridge_dirty,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::LATENCY_CRITICAL_INPUT,
            self.inp.latency_critical_input_pending,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::CATALOG_MESSAGES_ACTIVE,
            pre_input.prepare_trace.catalog_message_count > 0
                || pre_input.prepare_trace.catalog_backlog > 0
                || self.lib.pending_catalog_ready.is_some(),
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::MEDIA_MESSAGE_SEEN,
            pre_input.media_message_seen,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::SLINT_ANIMATION_ACTIVE,
            pre_input.slint_animation_active,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::HOME_PAN_PRESENT_ACTIVE,
            home_pan_present_active,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::HOME_HORIZONTAL_INPUT_HELD,
            home_horizontal_input_held,
        );
        // Arcade list motion lives outside Slint's bridge key, so the final
        // visual tick still has to present before the launcher can idle.
        wake_reasons.insert_if(
            LauncherWakeReasons::ARCADE_VISUAL_CHANGED_THIS_LOOP,
            arcade_visual_changed_this_loop,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::ARCADE_SCROLL_ACTIVE,
            self.ui.nav.screen == Screen::Arcade && self.ui.nav.arcade.is_scroll_active(),
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::ARCADE_FILTER_SCROLL_ACTIVE,
            self.ui.nav.screen == Screen::Arcade
                && self.ui.nav.arcade_filter.drawer_open
                && self.ui.nav.arcade_filter.is_scroll_active(),
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::ARCADE_SEARCH_ACTIVE,
            arcade_search_active,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::PREVIEW_DIRTY,
            preview_frame_intent.is_actionable(),
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::PREVIEW_SCHEDULED_THIS_LOOP,
            pre_input.preview_scheduled_this_loop,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::CRT_BACKDROP_PREPARED,
            crt_backdrop_prepared,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::COMPOSITION_FORCES_FULL_PRESENT,
            composition_decision.force_full_slint_present,
        );
        wake_reasons.insert_if(
            LauncherWakeReasons::COMPOSITION_CLEARS_DIRECT_LAYERS,
            composition_decision.clear_direct_layers,
        );
        let home_motion_active = home_frame_driven_redraw_active(
            self.ui.nav.screen,
            home_pan_present_active,
            home_horizontal_input_held,
        );
        let scheduled_frame_class = frame_production_class(
            self.fx.screensaver.active,
            home_motion_active,
            self.out.director.navigation.is_active(),
        );
        // One app-wide motion signal: anything moving on screen (animations,
        // scrolls, transitions, held directions, the screensaver). Background
        // and periodic work across the process yield to it. Publish before the
        // restart and idle branches so settling clears it at once.
        mister_magik_catalog::ui_motion::set_active(
            stream_motion_before_render
                || scheduled_frame_class != FrameProductionClass::EventDriven
                || self.out.director.orientation.is_active()
                || !self.out.director.chart.is_live()
                || pre_input.directional_input_held,
        );
        let render_intent = LauncherRenderIntent {
            first_visible_copy_done: self.diag.frame_accounting.first_visible_copy_done(),
            startup_input_enabled: startup_status.input_enabled,
            wake_reasons,
        };
        pre_input.scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("post-projection-background", pre_input.scheduler_phase);
        if should_restart_for_urgent_input(
            input.input_batch_empty,
            self.inp.latency_critical_input_pending,
            self.inp
                .input_observation_probe
                .as_ref()
                .is_some_and(|probe| probe.changed_since(self.inp.input_observation)),
        ) {
            self.diag
                .launcher_response_trace
                .record_lab(Some(serde_json::json!({
                    "phase": "input-priority-restart",
                    "checkpoint": "before-render",
                    "at_us": crate::input_hub::monotonic_us(),
                })));
            let _ = self
                .diag
                .launcher_response_trace
                .record_scheduler_interval("input-priority-restart", pre_input.scheduler_phase);
            return Err(Exit::Skip);
        }
        if render_intent.can_sleep() {
            #[cfg(feature = "tooling")]
            if let Some(observation) = self.diag.tooling_drop_baseline.as_mut()
                && observation.retire_motion_for_idle(mister_magik_catalog::ui_motion::active())
                && let Some(session) = self.diag.tooling.as_mut()
            {
                // Completed presents were already accounted. The next render
                // takes a fresh idle baseline, excluding deliberate scanout reuse.
                session.metrics.counters.idle_baseline_resets += 1;
            }
            if let Some(record) = self
                .diag
                .input_latency_lab
                .cooperative_quantum(self.inp.input_observation)
            {
                self.diag.launcher_response_trace.record_lab(Some(record));
                return Err(Exit::Skip);
            }
            self.diag.frame_accounting.finish_idle_loop(
                self.out.frames,
                self.out.run_start,
                Instant::now(),
                FrameStatusView {
                    nav: &self.ui.nav,
                    pad: &self.env.pad,
                    catalog: &self.lib.catalog,
                    catalog_ready: self.lib.catalog_ready,
                    catalog_refresh_done: self.lib.catalog_session.refresh_done(),
                    launching: pre_input.launching,
                    loading_title: self
                        .lib
                        .scheduler
                        .visible_loading_title(&self.lib.loading_title),
                    catalog_scan_visible,
                    catalog_scan_percent,
                    catalog_background_scan_visible,
                    confirm_visible,
                    confirm_selected,
                    status_text: status_text.as_ref(),
                    start_screen: self.lib.start_screen,
                    lock_screen: self.lib.lock_screen,
                    route_reassert_count: self.env.display_session.reassert_count(),
                    last_route_reassert_frame: self.env.display_session.last_reassert_frame(),
                    last_route_reassert_ok: self.env.display_session.last_reassert_ok(),
                    last_route_reassert_error: self.env.display_session.last_reassert_error(),
                    startup_status,
                    return_session: &self.lib.launch_return_session,
                },
                self.ui.nav.arcade.selected,
                self.ui.nav.arcade.visual_index,
                self.lib.preview.trace_cache_state(),
                self.diag
                    .preview_transition
                    .current_label(self.out.frame_clock.elapsed()),
                1.0,
                &composition_status,
            );
            pre_input.scheduler_phase = self
                .diag
                .launcher_response_trace
                .record_scheduler_interval("idle-accounting", pre_input.scheduler_phase);
            record_launcher_frame_phase!(LauncherFramePhase::IdleWait);
            let idle_sleep = self
                .diag
                .input_latency_lab
                .time_until_next_work()
                .map_or_else(
                    || launcher_idle_sleep_duration(&self.out.pacer),
                    |lab| launcher_idle_sleep_duration(&self.out.pacer).min(lab),
                );
            let idle_sleep = if self.fx.launcher_card_home.as_ref().is_some_and(
                super::launcher_card_home::LauncherCardHomeSession::waiting_for_destination,
            ) {
                idle_sleep.min(Duration::from_millis(16))
            } else {
                idle_sleep
            };
            let idle_sleep = self
                .ui
                .catalog_scan_blink
                .time_until_toggle(pre_input.animation_now)
                .map_or(idle_sleep, |blink| idle_sleep.min(blink));
            #[cfg(feature = "tooling")]
            let idle_sleep = if self.diag.tooling.is_some() {
                idle_sleep.min(Duration::from_millis(100))
            } else {
                idle_sleep
            };
            let _ = self
                .env
                .pad
                .wait_for_input(self.inp.input_observation, idle_sleep);
            self.out.idle_slept_since = Some(pre_input.loop_start);
            let _ = self
                .diag
                .launcher_response_trace
                .record_scheduler_interval("idle-input-wait", pre_input.scheduler_phase);
            record_launcher_frame_phase!(LauncherFramePhase::Yielded);
            return Err(Exit::Skip);
        }

        Ok(ProjectFrame {
            startup_intro_prepare_live_launcher,
            startup_intro_suppress_launcher_ui,
            startup_reveal_suppress_launcher_ui,
            gui_bridge_phase,
            response_projected_at_us,
            response_projected_execution,
            catalog_scan_visible,
            catalog_scan_percent,
            catalog_background_scan_visible,
            confirm_visible,
            confirm_selected,
            status_write_due,
            status_text,
            status_string_copy_bytes,
            arcade_status_only,
            arcade_scroll_active,
            arcade_turbo_active,
            full_frame_present,
            wants_arcade_list,
            crt_backdrop_eligible,
            crt_backdrop_was_eligible,
            crt_backdrop_leaving,
            wants_preview_layer,
            wants_preview,
            preview_cache_state_before_composition,
            composition_decision,
            composition_status,
            automation_frame_stamp,
            native_device_base,
            custom_home_active,
            custom_home_scene_ready,
            custom_home_needs_render,
            home_pan_present_active,
            home_horizontal_input_held,
            stream_motion_before_render,
            wake_reasons,
            scheduled_frame_class,
        })
    }

    /// Prepares the frame target, renders the Slint base and the custom layers.
    fn render(
        &mut self,
        #[cfg_attr(not(feature = "tooling"), allow(unused_variables, unused_mut))]
        begin: &mut BeginFrame,
        pre_input: &mut PreInputFrame,
        input: InputFrame,
        project: &mut ProjectFrame,
    ) -> Result<RenderFrame, Exit> {
        let active_arcade_games = if !pre_input.launching && self.ui.nav.screen == Screen::Arcade {
            active_system_game_view(&self.lib.catalog, &self.ui.nav)
        } else {
            ArcadeGameView::empty()
        };
        let frame_start_phase_us = self.out.pacer.age_since_last_hit_us(pre_input.loop_start);
        let redraw_pending_for_trace = self.env.window.redraw_pending();
        let wake_reasons_bits = project.wake_reasons.bits();
        let latch_backend_active = self.out.launcher_presenter.pacing_backend().is_latch();
        let late_frame_start_headroom_us = if latch_backend_active {
            self.out.phase_alignment.required_headroom_us()
        } else {
            FB0_LATE_FRAME_START_HEADROOM_US
        };
        let wait_before_render = latch_late_start_wait_enabled(
            latch_backend_active,
            project.scheduled_frame_class,
            self.inp.latency_critical_input_pending,
        ) && self
            .out
            .pacing_policy
            .decide(LauncherFramePacingInput {
                first_visible_copy_done: self.diag.frame_accounting.first_visible_copy_done(),
                frame_start_phase_us,
                period_us: self.out.pacer.period_us(),
                late_frame_start_headroom_us,
            })
            .wait_before_render;
        let cpu_t0 = FrameAnalyticsCpuStamp::capture(pre_input.frame_analytics_mode);
        let frame_t0 = Instant::now();
        let prepare_us = (frame_t0 - pre_input.loop_start).as_micros();
        pre_input.scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("render-setup", pre_input.scheduler_phase);
        let pre_render_pace = if wait_before_render {
            let wait_start = Instant::now();
            match self.out.pacer.wait_interruptible(|| {
                self.inp
                    .input_observation_probe
                    .as_ref()
                    .is_some_and(|probe| probe.changed_since(self.inp.input_observation))
            }) {
                VsyncWaitOutcome::Pace(pace) => {
                    let wait_done = Instant::now();
                    Some((
                        pace,
                        wait_done,
                        wait_done.saturating_duration_since(wait_start).as_micros(),
                    ))
                }
                VsyncWaitOutcome::Interrupted => {
                    self.diag
                        .launcher_response_trace
                        .record_lab(Some(serde_json::json!({
                            "phase": "pre-render-wait-interrupted-input",
                            "interrupted_at_us": crate::input_hub::monotonic_us(),
                        })));
                    let _ = self.diag.launcher_response_trace.record_scheduler_interval(
                        "pre-render-wait-interrupted-input",
                        pre_input.scheduler_phase,
                    );
                    self.env.window.request_redraw();
                    return Err(Exit::Skip);
                }
            }
        } else {
            None
        };
        let pre_render_wait_us = pre_render_pace
            .as_ref()
            .map(|(_, _, wait_us)| *wait_us)
            .unwrap_or(0);
        pre_input.scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("pre-render-pacing", pre_input.scheduler_phase);
        let full_screen_transition_policy_before_render = self.out.director.chart.policy();
        let navigation_snapshot_locked_before_render =
            full_screen_transition_policy_before_render.snapshot_locked;
        if full_screen_transition_policy_before_render.advance_slint_timers {
            update_slint_animations(self.env.animation_clock);
        }
        let mut layer_target = LayerTarget::new_oriented_with_epoch(
            self.env.target,
            self.out.layout,
            self.out.layout_epoch,
        );
        if project.native_device_base {
            layer_target.attach_device_background(
                &mut self.out.native_device_background,
                self.ui.nav.device_kind(),
                self.env.window,
            );
        }
        let reclaimed_preview_publication =
            layer_target.reclaim_preview_publication(&mut self.out.launcher_preview_publication);
        let cpu_t1 = FrameAnalyticsCpuStamp::capture(pre_input.frame_analytics_mode);
        let frame_t1 = Instant::now();
        #[cfg(feature = "tooling")]
        {
            self.diag.tooling_produced_id = self.diag.tooling_produced_id.wrapping_add(1);
        }
        #[cfg(feature = "tooling")]
        let tooling_animation_active = super::launcher_frame_accounting::capture_evidence_state(
            &mut begin.tooling_frame_evidence,
            &self.ui.nav,
            self.fx.launcher_card_home.as_ref(),
            project.scheduled_frame_class,
            pre_input.animation_us,
            self.inp.input_observation.generation(),
            self.diag.tooling_produced_id,
        );
        #[cfg(feature = "tooling")]
        super::launcher_frame_accounting::capture_evidence_cpu(
            &mut begin.tooling_frame_evidence,
            1,
            self.out.run_start,
        );
        // Repeats while idle are reuse, not drops. Restart an idle baseline at
        // every render start so motion starting from rest is measured from its
        // first frame's render: idle time is excluded. That frame may wait one
        // refresh for scanout; a longer overrun still counts as a drop.
        #[cfg(feature = "tooling")]
        if self.diag.tooling.is_some()
            && self
                .diag
                .tooling_drop_baseline
                .is_some_and(|observation| !observation.motion)
        {
            let before = begin
                .tooling_frame_evidence
                .as_ref()
                .map(|_| Instant::now());
            if let Ok(telemetry) = self.env.f.read_magik_presentation_telemetry() {
                let read_done = Instant::now();
                self.diag.tooling_drop_baseline = Some(
                    super::launcher_frame_accounting::ToolingPresentationObservation::new(
                        telemetry,
                        read_done,
                        false,
                        self.diag.tooling_attempt_id,
                        before,
                        self.out.run_start,
                    ),
                );
                if let Some(frame) = begin.tooling_frame_evidence.as_mut() {
                    frame.baseline_reset = true;
                }
            }
        }
        self.fx
            .retiring_screensaver_pipelines
            .retain_mut(|pipeline| !pipeline.poll_stopped());
        if self.fx.screensaver.take_restore_full_frame() {
            if let Some(mut snapshot) = self.fx.screensaver_launcher_frame.take()
                && !layer_target.swap_presentation_cached(&mut snapshot)
            {
                crate::ui_errln!(
                    "screensaver: launcher frame restore size mismatch snapshot={} cached={}",
                    snapshot.len(),
                    layer_target.cached_frame_view().pixels().len()
                );
            }
            retire_screensaver_pipeline(
                &mut self.fx.screensaver_pipeline,
                &mut self.fx.retiring_screensaver_pipelines,
            );
            self.fx.screensaver_frame_visible = false;
            self.fx.screensaver_active_cards = 0;
            self.env.window.request_redraw();
            project.full_frame_present = true;
        }
        if screensaver_pipeline_start_allowed(
            self.fx.screensaver.active,
            self.fx.screensaver_pipeline.is_some(),
        ) {
            if self.fx.screensaver_loader.is_none() {
                self.fx.screensaver.timeline.log("loader_started");
                self.fx.screensaver_loader = Some(LauncherScreensaverLoader::start(
                    self.out.layout.output_layout(),
                    self.fx.screensaver.timeline.started(),
                    self.env
                        .launcher_config
                        .catalog_paths()
                        .media_asset_dir()
                        .join("arcade-screenshots-320x320.mmlz4b"),
                    self.env.launcher_config.screensaver().seed(),
                ));
            }
            let loader = self.fx.screensaver_loader.as_ref().expect("created above");
            if let Some(ready) = loader.try_ready() {
                self.fx.screensaver.timeline.log("renderer_ready");
                self.fx.screensaver_pipeline = Some(ScreensaverRenderAhead::start(ready));
                self.fx.screensaver_render_sequence = 0;
                self.fx.screensaver_starvation_count = 0;
                self.fx.screensaver_loader = None;
            }
        }
        if !self.fx.screensaver.active {
            self.fx.screensaver_loader = None;
            retire_screensaver_pipeline(
                &mut self.fx.screensaver_pipeline,
                &mut self.fx.retiring_screensaver_pipelines,
            );
            self.fx.screensaver_launcher_frame = None;
            self.fx.screensaver_frame_visible = false;
            self.fx.screensaver_active_cards = 0;
        }
        let screensaver_fade_alpha = self
            .fx
            .screensaver
            .preview_fade_alpha(self.out.frame_clock.period());
        let mut frame_production_trace = FrameProductionTrace {
            class: project.scheduled_frame_class,
            ..FrameProductionTrace::default()
        };
        let mut frame_production_completed_at = None;
        let mut screensaver_render_trace = ScreensaverRenderTrace::default();
        let mut accepted_screensaver_frame = false;
        let mut screensaver_buffer_to_recycle_after_present = None;
        let mut completed_hidden_frame_for_present = None;
        let mut card_direct_frame_rendered = false;
        #[cfg(feature = "tooling")]
        let mut card_direct_measurement = None;
        #[cfg(feature = "tooling")]
        let mut card_work_timing = None;
        let mut accepted_startup_intro_frame = false;
        let mut startup_intro_failure = None;
        let mut navigation_capture_source_carrier_rendered = false;
        let mut orientation_capture_source_carrier_rendered = false;
        #[cfg(feature = "tooling")]
        let force_card_fallback = self
            .diag
            .tooling
            .as_ref()
            .is_some_and(|s| s.card_fallback_forced());
        #[cfg(not(feature = "tooling"))]
        let force_card_fallback = false;
        let card_motion_only = card_direct_hidden_eligible(CardDirectEligibility {
            custom_home_active: project.custom_home_active,
            custom_home_needs_render: project.custom_home_needs_render,
            direct_geometry: (self.out.layout.logical_w() == 960
                && self.out.layout.logical_h() == 540)
                || (self.out.layout.is_portrait() && !self.env.ui.output_route().is_crt()),
            full_frame_present: project.full_frame_present,
            launching: pre_input.launching,
            screensaver_active: self.fx.screensaver.active,
            startup_intro_active: self.fx.startup_intro.is_some(),
            startup_reveal_suppressed: project.startup_reveal_suppress_launcher_ui,
            startup_intro_suppressed: project.startup_intro_suppress_launcher_ui,
            confirm_visible: project.confirm_visible,
            catalog_scan_visible: project.catalog_scan_visible,
            navigation_transition_active: self.out.director.navigation.is_active(),
            orientation_transition_active: self.out.director.orientation.is_active(),
            composition_state: project.composition_decision.state,
            force_full_slint_raster: project.composition_decision.force_full_slint_raster,
            force_full_slint_present: project.composition_decision.force_full_slint_present,
            transition_state: self.out.director.chart.state(),
        });
        if !card_motion_only && let Some(session) = self.fx.launcher_card_home.as_mut() {
            session.invalidate_compositor();
        }
        let card_direct_path_eligible = !force_card_fallback
            && card_motion_only
            && project.custom_home_scene_ready
            && self
                .fx
                .launcher_card_home
                .as_ref()
                .is_some_and(|session| session.can_render_direct());
        if card_direct_path_eligible
            && let Some(session) = self.fx.launcher_card_home.as_mut()
            && session.can_render_direct()
        {
            // Pose time is the frame's animation time, so pacing waits and
            // repeated samples within a frame always agree.
            let pose_at = pre_input.animation_now;
            // Wall time is only for measuring pose-to-present lag, never motion.
            #[cfg(feature = "tooling")]
            let pose_sampled_at = Instant::now();
            let (selected, visual_index) = self.ui.nav.home_card_visual_prediction(pose_at);
            let accepted = session.update_from_navigation(
                super::launcher_card_home::scene_for_display(self.env.ui, self.out.layout),
                &self.lib.card_level,
                selected,
                visual_index,
                &self.inp.last_clock_text,
                pre_input.animation_us / 1_000,
                !self.ui.nav.settings.reduce_motion,
                self.ui.nav.home_card_browse_prediction(pose_at),
                self.ui.nav.home_level_transition(),
            );
            if accepted {
                self.ui.nav.acknowledge_home_level_transition();
            }
            let level_trick = session.is_level_trick_active();
            session.set_output_layout(
                self.out
                    .layout
                    .is_portrait()
                    .then(|| self.out.layout.output_layout()),
            );
            #[cfg(feature = "tooling")]
            let direct_started = Instant::now();
            session.render_direct_bands();
            #[cfg(feature = "tooling")]
            let direct_bands_us = direct_started.elapsed().as_micros() as u64;
            #[cfg(feature = "tooling")]
            let mut direct_rotate_us = 0;
            let request = session.current_request();
            let timing = session.last_timing();
            let chrome_damage = session.chrome_copy_damage(level_trick);
            let identity = mister_magik_framebuffer_scenes::retained_tiles::TileImageIdentity::new(
                session.content_generation(),
                request.generation,
            );
            let output = self.out.layout.output_layout();
            let copied = if self.out.layout.is_portrait() {
                // Rotated output: the scanout slot is physical landscape, so
                // present the frame and helper band already rotated into it.
                #[cfg(feature = "tooling")]
                let rotate_started = Instant::now();
                let rotated = session.direct_physical_bands(output);
                #[cfg(feature = "tooling")]
                {
                    direct_rotate_us = rotate_started.elapsed().as_micros() as u64;
                }
                match rotated {
                    Some(bands) => {
                        let (width, height) = (output.physical_width(), output.physical_height());
                        let cached = card_cached_frame_view(bands.frame, width, height);
                        let helper = card_cached_frame_view(bands.helper, width, height);
                        self.out.launcher_presenter.try_copy_direct_hidden_tiles(
                            self.env.f,
                            self.env.display_session,
                            cached,
                            bands.chrome_damage.as_ref().unwrap_or(&chrome_damage),
                            [cached, helper],
                            bands.damage,
                            identity,
                        )
                    }
                    None => Ok(None),
                }
            } else {
                match (session.current_helper_pixels(), session.rendered_split()) {
                    (Some(helper_pixels), Some(split)) => {
                        let cached = card_cached_frame_view(
                            session.current_primary_pixels(),
                            self.out.layout.logical_w(),
                            self.out.layout.logical_h(),
                        );
                        let helper = card_cached_frame_view(
                            helper_pixels,
                            self.out.layout.logical_w(),
                            self.out.layout.logical_h(),
                        );
                        self.out.launcher_presenter.try_copy_direct_hidden_tiles(
                            self.env.f,
                            self.env.display_session,
                            cached,
                            &chrome_damage,
                            [cached, helper],
                            card_direct_tile_damage(session.carousel_clip().0, level_trick, split),
                            identity,
                        )
                    }
                    // The two-thread renderer stopped: the Slint path presents.
                    _ => Ok(None),
                }
            };
            #[cfg(feature = "tooling")]
            if let Some(tooling_) = self.diag.tooling.as_mut() {
                let counters = &mut tooling_.metrics.counters;
                counters.card_direct_bands_us += direct_bands_us;
                counters.card_direct_rotate_us += direct_rotate_us;
                counters.card_direct_total_us += direct_started.elapsed().as_micros() as u64;
            }
            match copied {
                Ok(Some(copy)) => {
                    frame_production_trace.class = FrameProductionClass::SynchronousAnimation;
                    frame_production_trace.sequence = request.generation;
                    frame_production_trace.render_wall_us = timing.map_or(0, |t| t.total_us);
                    frame_production_completed_at = Some(Instant::now());
                    #[cfg(feature = "tooling")]
                    if self.diag.card_presentation_measurement_enabled {
                        if let Some(frame) = begin.tooling_frame_evidence.as_mut() {
                            frame.direct_hidden_copy_us = copy.copy_us;
                            frame.direct_hidden_copy_bytes = copy.copy.bytes as u64;
                        }
                        let rendered = session.rendered_request();
                        card_direct_measurement = Some((
                            copy.copy_us,
                            rendered.map_or(0, |pose| pose.timestamp_us),
                            rendered.map_or(0, |pose| pose.generation),
                            pose_sampled_at.elapsed().as_micros() as u64,
                        ));
                        if let (Some(tooling_), Some(timing)) = (self.diag.tooling.as_mut(), timing)
                        {
                            tooling_.metrics.counters.card_producer_total_us += timing.total_us;
                            tooling_.metrics.counters.card_primary_tile_us += timing.primary_us;
                            tooling_.metrics.counters.card_secondary_tile_us += timing.secondary_us;
                            tooling_.metrics.counters.card_secondary_wait_us += timing.wait_us;
                            tooling_.metrics.counters.card_rendered_frames += 1;
                            let work = mister_magik_tooling_support::measurement::FrameWorkTiming {
                                producer_us: timing.total_us,
                                helper_ahead: timing.helper_ahead,
                                helper_ahead_lead_us: timing.helper_ahead_lead_us,
                                discarded_helper_us: timing.discarded_helper_us,
                                discarded_helper_cpu_us: timing.discarded_helper_cpu_us,
                                primary_us: timing.primary_us,
                                secondary_us: timing.secondary_us,
                                wait_us: timing.wait_us,
                                helper_start_delay_us: timing.helper_start_delay_us,
                                completion_delivery_us: timing.completion_delivery_us,
                                merge_us: timing.merge_us,
                                primary_cpu_us: timing.primary_cpu_us,
                                secondary_cpu_us: timing.secondary_cpu_us,
                                split: timing.split as u64,
                                primary_run_delay_us: timing.primary_run_delay_us,
                                secondary_run_delay_us: timing.secondary_run_delay_us,
                            };
                            if let Some(frame) = begin.tooling_frame_evidence.as_mut()
                                && frame.phases_enabled
                            {
                                frame.helper = Some(
                                    mister_magik_tooling_support::frame_evidence::HelperEvidence {
                                        renderer_id: timing.renderer_id,
                                        request_generation: timing.request_generation,
                                        request_timestamp_us: timing.request_timestamp_us,
                                        dispatched_us: duration_us(
                                            self.out.run_start,
                                            timing.helper_dispatched_at,
                                        ),
                                        started_us: duration_us(
                                            self.out.run_start,
                                            timing.helper_started_at,
                                        ),
                                        finished_us: duration_us(
                                            self.out.run_start,
                                            timing.helper_finished_at,
                                        ),
                                        received_us: duration_us(
                                            self.out.run_start,
                                            timing.helper_received_at,
                                        ),
                                        discarded_generation: timing.discarded_generation,
                                    },
                                );
                            }
                            card_work_timing = Some(work);
                            if tooling_.metrics.window_start.is_some()
                                && tooling_.metrics.window.is_none()
                            {
                                tooling_.metrics.work_timings.push(work);
                            }
                        }
                    }
                    completed_hidden_frame_for_present = Some(copy.completed);
                    card_direct_frame_rendered = true;
                }
                Ok(None) => {}
                Err(failure) => self.out.launcher_presenter.fail_latch_completion(failure),
            }
        }
        let card_direct_waiting_on_slot =
            card_direct_path_eligible
                && !card_direct_frame_rendered
                && self.fx.launcher_card_home.as_ref().is_some_and(
                    super::launcher_card_home::LauncherCardHomeSession::compositor_stale,
                );
        if self
            .out
            .director
            .navigation_needs_source_carrier(full_screen_transition_policy_before_render)
        {
            let mut direct_render_timing = None;
            match self.out.launcher_presenter.try_render_direct_hidden_frame(
                self.env.f,
                self.env.display_session,
                |_, pixels| {
                    let started = Instant::now();
                    let start_phase_us = self.out.pacer.age_since_last_hit_us(started);
                    let rendered = self.out.director.navigation.render_into(pixels).is_ok();
                    direct_render_timing = Some((started, Instant::now(), start_phase_us));
                    rendered
                },
            ) {
                Ok(Some(completed)) => {
                    let (direct_render_started, direct_render_completed, start_phase_us) =
                        direct_render_timing.expect("successful source carrier was timed");
                    frame_production_trace.class = FrameProductionClass::SynchronousAnimation;
                    frame_production_trace.sequence = completed.grant.generation;
                    frame_production_trace.render_start_phase_us = start_phase_us;
                    frame_production_trace.render_wall_us = direct_render_completed
                        .saturating_duration_since(direct_render_started)
                        .as_micros()
                        .try_into()
                        .unwrap_or(u64::MAX);
                    frame_production_completed_at = Some(direct_render_completed);
                    completed_hidden_frame_for_present = Some(completed);
                    navigation_capture_source_carrier_rendered = true;
                }
                Ok(None) => {}
                Err(failure) => self.out.launcher_presenter.fail_latch_completion(failure),
            }
        }
        if self
            .out
            .director
            .orientation_needs_source_carrier(full_screen_transition_policy_before_render)
        {
            let mut direct_render_timing = None;
            match self.out.launcher_presenter.try_render_direct_hidden_frame(
                self.env.f,
                self.env.display_session,
                |_, pixels| {
                    let started = Instant::now();
                    let start_phase_us = self.out.pacer.age_since_last_hit_us(started);
                    let rendered = self.out.director.orientation.copy_source_into(pixels);
                    direct_render_timing = Some((started, Instant::now(), start_phase_us));
                    rendered
                },
            ) {
                Ok(Some(completed)) => {
                    let (direct_render_started, direct_render_completed, start_phase_us) =
                        direct_render_timing.expect("successful orientation carrier was timed");
                    frame_production_trace.class = FrameProductionClass::SynchronousAnimation;
                    frame_production_trace.sequence = completed.grant.generation;
                    frame_production_trace.render_start_phase_us = start_phase_us;
                    frame_production_trace.render_wall_us = direct_render_completed
                        .saturating_duration_since(direct_render_started)
                        .as_micros()
                        .try_into()
                        .unwrap_or(u64::MAX);
                    frame_production_completed_at = Some(direct_render_completed);
                    completed_hidden_frame_for_present = Some(completed);
                    orientation_capture_source_carrier_rendered = true;
                }
                Ok(None) => {}
                Err(failure) => self.out.launcher_presenter.fail_latch_completion(failure),
            }
        }
        if let Some(intro) = self.fx.startup_intro.as_mut() {
            if intro.snapshot_capture_needed() && self.fx.startup_intro_launcher_frame_ready {
                let launcher_pixels = layer_target.presentation_frame_view().pixels();
                if let Err(error) = intro.begin_launcher_snapshot_preparation(launcher_pixels) {
                    startup_intro_failure = Some(error);
                } else {
                    print_startup_event(
                        self.out.start,
                        "startup_intro_launcher_snapshot_captured",
                        format!(
                            "pixels={} cabinet_wait_frames={}",
                            launcher_pixels.len(),
                            intro.waiting_frames(),
                        ),
                    );
                }
            }
            if startup_intro_failure.is_none() {
                match intro.poll_launcher_snapshot_preparation() {
                    Ok(true) => print_startup_event(
                        self.out.start,
                        "startup_intro_launcher_snapshot_prepared",
                        format!("cabinet_wait_frames={}", intro.waiting_frames()),
                    ),
                    Ok(false) => {}
                    Err(error) => startup_intro_failure = Some(error),
                }
            }
            if startup_intro_failure.is_none() {
                let mut source_evidence = None;
                let mut render_error = None;
                match self
                    .out
                    .launcher_presenter
                    .try_render_startup_intro_hidden_frame(
                        self.env.f,
                        self.env.display_session,
                        |grant, pixels| match intro.render_into(
                            grant,
                            pixels,
                            self.out.launcher_readiness.source_evidence_request(),
                        ) {
                            Ok(evidence) => {
                                source_evidence = evidence;
                                true
                            }
                            Err(error) => {
                                render_error = Some(error);
                                false
                            }
                        },
                    ) {
                    Ok(Some(mut completed)) => {
                        completed.source_evidence = source_evidence;
                        completed_hidden_frame_for_present = Some(completed);
                        accepted_startup_intro_frame = true;
                    }
                    Ok(None) => {}
                    Err(failure) => {
                        self.out.launcher_presenter.fail_latch_completion(failure);
                        startup_intro_failure = Some("hidden-slot grant failed".into());
                    }
                }
                if startup_intro_failure.is_none() {
                    startup_intro_failure = render_error;
                }
            }
        }
        if let Some(error) = startup_intro_failure.take() {
            crate::ui_errln!("startup intro stopped: {error}");
            self.diag
                .launcher_automation
                .note_startup_intro_failure(&error);
            self.fx.startup_intro = None;
            self.out
                .launcher_presenter
                .invalidate_external_hidden_mode();
            project.full_frame_present = true;
            self.env.window.request_redraw();
        }
        if self.fx.startup_intro.is_none() && self.fx.screensaver.active {
            let render_ahead_poll = self
                .fx
                .screensaver_pipeline
                .as_mut()
                .map(ScreensaverRenderAhead::try_next)
                .unwrap_or(RenderAheadPoll::Empty);
            match render_ahead_poll {
                RenderAheadPoll::Frame(frame) => {
                    let mut pixels = frame.pixels;
                    if layer_target.swap_presentation_cached(&mut pixels) {
                        retain_or_defer_screensaver_buffer(
                            &mut self.fx.screensaver_launcher_frame,
                            &mut screensaver_buffer_to_recycle_after_present,
                            pixels,
                        );
                        screensaver_render_trace = frame.trace;
                        self.fx.screensaver_render_sequence = frame.sequence;
                        frame_production_trace.class = FrameProductionClass::Prepared;
                        frame_production_trace.sequence = frame.sequence;
                        frame_production_completed_at = Some(frame.completed_at);
                        frame_production_trace.ready_depth = self
                            .fx
                            .screensaver_pipeline
                            .as_ref()
                            .map(ScreensaverRenderAhead::ready_depth)
                            .unwrap_or(0);
                        frame_production_trace.render_wall_us = frame.render_wall_us;
                        self.fx.screensaver_active_cards = frame.active_cards;
                        self.fx.screensaver_frame_visible = true;
                        accepted_screensaver_frame = true;
                    } else {
                        crate::ui_errln!(
                            "screensaver: render-ahead frame geometry mismatch sequence={} pixels={} cached={}",
                            frame.sequence,
                            pixels.len(),
                            layer_target.cached_frame_view().pixels().len()
                        );
                        if let Some(pipeline) = self.fx.screensaver_pipeline.as_ref() {
                            let _ = pipeline.recycle(pixels);
                        }
                    }
                }
                RenderAheadPoll::Empty => {}
                RenderAheadPoll::SequenceFailure {
                    expected_tick,
                    actual_tick,
                } => {
                    crate::ui_errln!(
                        "screensaver: strict render-ahead sequence failure expected_tick={} actual_tick={}",
                        expected_tick,
                        actual_tick,
                    );
                    self.fx.screensaver.fail_current_activation(Instant::now());
                    retire_screensaver_pipeline(
                        &mut self.fx.screensaver_pipeline,
                        &mut self.fx.retiring_screensaver_pipelines,
                    );
                    self.fx.screensaver_frame_visible = false;
                    self.fx.screensaver_active_cards = 0;
                    self.env.window.request_redraw();
                    project.full_frame_present = true;
                }
                RenderAheadPoll::Disconnected => {
                    crate::ui_errln!(
                        "screensaver: render-ahead pipeline disconnected; suppressing reactivation until fresh user activity"
                    );
                    self.fx.screensaver.fail_current_activation(Instant::now());
                    if let Some(mut snapshot) = self.fx.screensaver_launcher_frame.take()
                        && !layer_target.swap_presentation_cached(&mut snapshot)
                    {
                        crate::ui_errln!(
                            "screensaver: launcher frame restore size mismatch after pipeline disconnect snapshot={} cached={}",
                            snapshot.len(),
                            layer_target.cached_frame_view().pixels().len()
                        );
                    }
                    retire_screensaver_pipeline(
                        &mut self.fx.screensaver_pipeline,
                        &mut self.fx.retiring_screensaver_pipelines,
                    );
                    self.fx.screensaver_frame_visible = false;
                    self.fx.screensaver_active_cards = 0;
                    self.env.window.request_redraw();
                    project.full_frame_present = true;
                }
            }
        }
        if self.fx.screensaver.active
            && self.fx.screensaver_frame_visible
            && !accepted_screensaver_frame
            && self.fx.screensaver_pipeline.is_some()
        {
            self.fx.screensaver_starvation_count =
                self.fx.screensaver_starvation_count.saturating_add(1);
            crate::ui_errln!("screensaver: shared screenshot runtime starved; restoring launcher");
            self.fx.screensaver.fail_current_activation(Instant::now());
            retire_screensaver_pipeline(
                &mut self.fx.screensaver_pipeline,
                &mut self.fx.retiring_screensaver_pipelines,
            );
            self.fx.screensaver_frame_visible = false;
            self.env.window.request_redraw();
            project.full_frame_present = true;
        }
        if self.fx.screensaver.active {
            frame_production_trace.class = FrameProductionClass::Prepared;
            frame_production_trace.sequence = self.fx.screensaver_render_sequence;
            frame_production_trace.ready_depth = self
                .fx
                .screensaver_pipeline
                .as_ref()
                .map(ScreensaverRenderAhead::ready_depth)
                .unwrap_or(0);
            frame_production_trace.starvation_count = self.fx.screensaver_starvation_count;
            frame_production_trace.cancelled = self.fx.screensaver_pipeline.is_none()
                && !self.fx.retiring_screensaver_pipelines.is_empty();
        }
        let mut slint_damage = DirtyRectList::new();
        let mut full_screen_transition_release_raster_rendered = false;
        let mut full_screen_transition_live_endpoint_rendered = false;
        let mut full_screen_controlled_capture_rendered = false;
        let mut orientation_controlled_slint_raster_us = 0;
        let mut gui_raster_phase = GuiRasterProfilePhase::None;
        let response_raster_started_at_us = crate::input_hub::monotonic_us();
        let response_raster_started_execution = self.diag.launcher_response_trace.execution_stamp();
        let raster_pmu = self.diag.launcher_response_trace.input_pmu_span(
            self.inp.latency_critical_input_pending,
            "launcher-response.slint-raster",
        );
        macro_rules! render_launcher_base {
            ($full_slint_raster:expr) => {{
                if project.custom_home_active
                    && project.custom_home_scene_ready
                    && ($full_slint_raster
                        || project.custom_home_needs_render
                        || self.env.window.redraw_pending()
                        || self.fx.launcher_card_home.as_ref().is_some_and(
                            super::launcher_card_home::LauncherCardHomeSession::compositor_stale,
                        ))
                    && let Some(session) = self.fx.launcher_card_home.as_mut()
                {
                    let retain_cache = card_motion_only && !$full_slint_raster;
                    let copy_damage = session.compositor_copy_damage(retain_cache);
                    #[cfg(feature = "tooling")]
                    let home_started = begin
                        .tooling_frame_evidence
                        .as_ref()
                        .is_some_and(|frame| frame.phases_enabled)
                        .then(Instant::now);
                    let native_home_pixels = session.render();
                    #[cfg(feature = "tooling")]
                    let home_native_done = home_started.map(|_| Instant::now());
                    let (dirty, damage, rendered, copied) = layer_target.render_custom_home(
                        &self.env.window,
                        native_home_pixels,
                        $full_slint_raster,
                        copy_damage,
                    );
                    #[cfg(feature = "tooling")]
                    if let Some((started, native_done)) = home_started.zip(home_native_done)
                        && let Some(frame) = begin.tooling_frame_evidence.as_mut()
                    {
                        frame.home_composition_us[0] +=
                            u128_to_u64(native_done.duration_since(started).as_micros());
                        frame.home_composition_us[1] +=
                            u128_to_u64(native_done.elapsed().as_micros());
                    }
                    session.note_compositor_copied(retain_cache && copied.is_some());
                    #[cfg(feature = "tooling")]
                    if let Some(copied) = copied
                        && let Some(tooling_) = self.diag.tooling.as_mut()
                    {
                        tooling_.metrics.counters.card_fallback_copies += 1;
                        tooling_.metrics.counters.card_fallback_copy_pixels +=
                            ((copied.x1 - copied.x0) * (copied.y1 - copied.y0)) as u64;
                    }
                    (dirty, damage, rendered)
                } else if $full_slint_raster {
                    layer_target.render_slint_full(&self.env.window)
                } else {
                    let (dirty, damage) = layer_target.render_slint_base(&self.env.window);
                    (dirty, damage, dirty.is_some())
                }
            }};
        }
        let this_rect = if card_direct_frame_rendered || card_direct_waiting_on_slot {
            if card_direct_waiting_on_slot {
                self.env.window.request_redraw();
            }
            None
        } else if self.fx.screensaver.active && self.fx.screensaver_frame_visible {
            if accepted_screensaver_frame {
                if screensaver_fade_alpha.is_some_and(|alpha| alpha < 255) {
                    Some(
                        layer_target.blend_screensaver_crossfade(
                            self.fx
                                .screensaver_launcher_frame
                                .as_deref()
                                .expect("launcher frame retained by first buffer swap"),
                            screensaver_fade_alpha.expect("checked above"),
                        ),
                    )
                } else {
                    Some(DirtyRect {
                        x0: 0,
                        y0: 0,
                        x1: self.out.layout.composition_w(),
                        y1: self.out.layout.composition_h(),
                    })
                }
            } else {
                None
            }
        } else if self.fx.screensaver.active
            || project.startup_reveal_suppress_launcher_ui
            || project.startup_intro_suppress_launcher_ui
        {
            None
        } else if full_screen_transition_policy_before_render.snapshot_locked {
            if let Some(generation) = self.out.director.chart.generation() {
                let _ = self.out.director.chart.retain_redraw(generation);
            }
            None
        } else if full_screen_transition_policy_before_render.force_live_raster {
            gui_raster_phase = gui_raster_profile_phase(true, true);
            let gui_raster_pmu = self
                .diag
                .gui_profiling
                .phase_span(gui_raster_phase.span_name());
            let (dirty, damage, rendered) = render_launcher_base!(true);
            drop(gui_raster_pmu);
            slint_damage = damage;
            full_screen_transition_release_raster_rendered = rendered;
            dirty
        } else if full_screen_transition_policy_before_render.controlled_capture
            && (project.composition_decision.force_full_slint_raster
                || self.out.director.chart.owner() == Some(FullScreenTransitionOwner::Orientation))
        {
            let authorized = self
                .out
                .director
                .chart
                .generation()
                .is_some_and(|generation| {
                    match self.out.director.chart.take_controlled_capture(generation) {
                        Ok(authorized) => authorized,
                        Err(error) => {
                            crate::ui_errln!("navigation controlled capture rejected: {error:?}");
                            false
                        }
                    }
                });
            if authorized {
                gui_raster_phase = gui_raster_profile_phase(true, true);
                let gui_raster_pmu = self
                    .diag
                    .gui_profiling
                    .phase_span(gui_raster_phase.span_name());
                let controlled_raster_started = Instant::now();
                let (dirty, damage, rendered) = render_launcher_base!(true);
                drop(gui_raster_pmu);
                if self.out.director.chart.owner() == Some(FullScreenTransitionOwner::Orientation) {
                    orientation_controlled_slint_raster_us =
                        controlled_raster_started.elapsed().as_micros();
                }
                slint_damage = damage;
                full_screen_controlled_capture_rendered = rendered;
                if !rendered
                    && let Some(generation) = self.out.director.chart.generation()
                    && let Err(error) = self.out.director.chart.capture_deferred(generation)
                {
                    crate::ui_errln!("full-screen controlled capture defer rejected: {error:?}");
                } else if !rendered {
                    self.env.window.request_redraw();
                }
                dirty
            } else {
                None
            }
        } else if !full_screen_transition_policy_before_render.automatic_slint_raster {
            if let Some(generation) = self.out.director.chart.generation() {
                let _ = self.out.director.chart.retain_redraw(generation);
            }
            None
        } else if project.crt_backdrop_eligible
            && project.crt_backdrop_was_eligible
            && !project.crt_backdrop_leaving
            && !project.full_frame_present
            && (project.arcade_scroll_active || self.lib.preview.raw_transition_frame().is_some())
        {
            // During CRT Arcade motion, the custom compositor owns the
            // changing backdrop, list, and chrome restoration. Keep the
            // launcher cadence alive without rerasterizing the unchanged
            // Slint base on every velocity tick.
            self.env.window.request_redraw();
            None
        } else if project.composition_decision.force_full_slint_raster
            || project.crt_backdrop_leaving
        {
            gui_raster_phase = gui_raster_profile_phase(true, true);
            let gui_raster_pmu = self
                .diag
                .gui_profiling
                .phase_span(gui_raster_phase.span_name());
            let (dirty, damage, _) = render_launcher_base!(true);
            drop(gui_raster_pmu);
            slint_damage = damage;
            dirty
        } else if project.startup_intro_prepare_live_launcher {
            gui_raster_phase = gui_raster_profile_phase(true, false);
            let gui_raster_pmu = self
                .diag
                .gui_profiling
                .phase_span(gui_raster_phase.span_name());
            let (dirty, damage, _) = render_launcher_base!(false);
            drop(gui_raster_pmu);
            slint_damage = damage;
            dirty
        } else {
            gui_raster_phase = gui_raster_profile_phase(true, false);
            let gui_raster_pmu = self
                .diag
                .gui_profiling
                .phase_span(gui_raster_phase.span_name());
            let (dirty, damage, _) = render_launcher_base!(false);
            drop(gui_raster_pmu);
            let expanded = if self.out.layout.is_portrait() {
                dirty
            } else {
                expand_home_pan_dirty_rect(dirty, self.env.ui, project.home_pan_present_active)
            };
            slint_damage = if expanded == dirty {
                damage
            } else {
                expanded.map_or_else(DirtyRectList::new, DirtyRectList::from_one)
            };
            expanded
        };
        let response_raster_completed_at_us = crate::input_hub::monotonic_us();
        let response_raster_completed_execution =
            self.diag.launcher_response_trace.execution_stamp();
        drop(raster_pmu);
        self.diag.gui_profiling.record_frame(
            self.out.frames,
            response_raster_completed_at_us,
            frame_production_trace.class.label(),
            project.gui_bridge_phase,
            gui_raster_phase,
            slint_damage
                .iter()
                .map(|rect| [rect.x0, rect.y0, rect.x1, rect.y1])
                .collect(),
        );
        if can_preempt_disposable_home_raster(
            self.ui.nav.screen,
            input.input_batch_empty,
            self.inp.latency_critical_input_pending,
            self.inp
                .input_observation_probe
                .as_ref()
                .is_some_and(|probe| probe.changed_since(self.inp.input_observation)),
            self.out.director.navigation.is_active()
                || self.out.director.orientation.is_active()
                || !self.out.director.chart.is_live(),
            self.fx.screensaver.active,
            project.composition_decision.state != UiCompositionState::FullSlint
                || project.composition_decision.retirement_generation.is_some(),
            self.fx.startup_intro.is_some(),
        ) {
            restart_unpublished_home_frame(
                &mut completed_hidden_frame_for_present,
                this_rect.is_some() || !slint_damage.is_empty(),
                &mut self.out.unpublished_cached_frame_present,
                |completed| {
                    self.out
                        .launcher_presenter
                        .discard_completed_hidden_frame(completed)
                },
            );
            #[cfg(feature = "tooling")]
            if let Some(frame) = begin.tooling_frame_evidence.as_mut() {
                // Rendered work is counted once even when input prevents delivery.
                frame.record.work = card_work_timing;
                if card_work_timing.is_some() {
                    frame.record.workload =
                        mister_magik_tooling_support::measurement::FrameWorkload::Card;
                }
            }
            self.diag
                .launcher_response_trace
                .record_lab(Some(serde_json::json!({
                    "phase": "input-priority-restart",
                    "checkpoint": "after-slint-raster",
                    "at_us": response_raster_completed_at_us,
                    "slint_damage_rects": slint_damage.len(),
                })));
            let _ = self
                .diag
                .launcher_response_trace
                .record_scheduler_interval("input-priority-restart", pre_input.scheduler_phase);
            #[cfg(feature = "tooling")]
            super::launcher_frame_accounting::record_abandoned_evidence_raster(
                &mut begin.tooling_frame_evidence,
                self.diag.tooling.as_mut(),
                self.out.run_start,
                frame_t1,
                Instant::now(),
            );
            self.env.window.request_redraw();
            return Err(Exit::Skip);
        }
        let frame_plan_pmu = self.diag.launcher_response_trace.input_pmu_span(
            self.inp.latency_critical_input_pending,
            "launcher-response.damage-frame-plan",
        );
        let mut launcher_response_frame_stamp = self.diag.launcher_response_trace.frame_stamp(
            &self.ui.nav,
            project.response_projected_at_us,
            project.response_projected_execution,
            response_raster_started_at_us,
            response_raster_started_execution,
            response_raster_completed_at_us,
            response_raster_completed_execution,
        );
        if let Some(stamp) = launcher_response_frame_stamp.as_mut() {
            stamp.slint_damage_rects.extend(
                slint_damage
                    .iter()
                    .map(|rect| (rect.x0, rect.y0, rect.x1, rect.y1)),
            );
        }
        self.out
            .director
            .abort_stalled_orientation_capture(full_screen_controlled_capture_rendered);
        if project.startup_intro_prepare_live_launcher {
            self.fx.startup_intro_launcher_frame_ready = true;
            print_startup_event(
                self.out.start,
                "startup_intro_launcher_frame_ready",
                format!(
                    "games={} systems={}",
                    self.lib.catalog.len(),
                    self.lib.catalog.systems.len()
                ),
            );
        }
        self.fx
            .screensaver
            .timeline
            .note_rendered(accepted_screensaver_frame);
        let cpu_t2 = FrameAnalyticsCpuStamp::capture(pre_input.frame_analytics_mode);
        let frame_t2 = Instant::now();
        #[cfg(feature = "tooling")]
        super::launcher_frame_accounting::capture_evidence_cpu(
            &mut begin.tooling_frame_evidence,
            2,
            self.out.run_start,
        );
        let cpu_custom_draw_start = FrameAnalyticsCpuStamp::capture(pre_input.frame_analytics_mode);
        let custom_draw_start = Instant::now();
        let logical_slint_rect = this_rect.map(|rect| {
            if self.out.layout.is_portrait() && !slint_damage.is_empty() {
                self.out.layout.composition_rect_to_logical_rect(rect)
            } else {
                rect
            }
        });
        let mut logical_slint_damage_for_custom = DirtyRectList::new();
        if self.out.layout.is_portrait() {
            for rect in slint_damage.iter() {
                logical_slint_damage_for_custom
                    .push(self.out.layout.composition_rect_to_logical_rect(rect));
            }
        } else {
            logical_slint_damage_for_custom.extend_from(&slint_damage);
        }
        let mut arcade_bbox_invalidation = false;
        let mut arcade_rect_invalidation = false;
        let mut arcade_false_positive_invalidation = false;
        let mut preview_bbox_invalidation = false;
        let mut preview_rect_invalidation = false;
        let mut preview_false_positive_invalidation = false;
        let gui_custom_selection = gui_custom_profile_selection(
            project.wants_arcade_list && project.composition_decision.allow_arcade_list_blit,
            (project.wants_preview || self.lib.preview.empty_base_commit_pending())
                && project.composition_decision.allow_preview_blit,
            self.out.director.navigation.is_active(),
            self.out.director.orientation.is_active(),
        );
        let gui_custom_generation_pmu = self.diag.gui_profiling.phase_span(
            gui_custom_selection
                .any()
                .then_some("gui.custom-layer-generation"),
        );
        let arcade_list_update_start = Instant::now();
        let arcade_list_rect =
            if project.wants_arcade_list && project.composition_decision.allow_arcade_list_blit {
                let gui_arcade_pmu = self
                    .diag
                    .gui_profiling
                    .phase_span(gui_custom_selection.arcade_row_update);
                let arcade_list_profile_pmu =
                    mister_magik_perf_events::sampled_span("gui.custom.crt-arcade-list-update");
                self.out
                    .arcade_list_renderer
                    .set_crt_portrait_rows(self.out.layout.is_portrait());
                configure_arcade_list_renderer_geometry(
                    &mut self.out.arcade_list_renderer,
                    &self.ui.nav,
                    self.env.ui,
                );
                let arcade_rect = self.out.arcade_list_renderer.dirty_rect();
                (
                    arcade_bbox_invalidation,
                    arcade_rect_invalidation,
                    arcade_false_positive_invalidation,
                ) = custom_damage_invalidation_comparison(
                    logical_slint_rect,
                    &logical_slint_damage_for_custom,
                    arcade_rect,
                    project.full_frame_present,
                );
                let force_arcade_redraw = if self.out.layout.is_portrait() && !self.out.crt_layout {
                    // The portrait list is a separately versioned physical layer.
                    // Slint/base damage is restored by the latch presenter and
                    // must not force regeneration of unchanged list content.
                    false
                } else {
                    arcade_list_needs_forced_redraw(
                        &self.out.arcade_list_renderer,
                        logical_slint_rect,
                        project.full_frame_present,
                    )
                };
                let update = if self.ui.nav.arcade_filter.drawer_open {
                    let items = self.out.arcade_drawer_view_cache.items(
                        &self.lib.catalog,
                        &self.ui.nav,
                        self.lib.catalog_version,
                    );
                    self.out.arcade_list_renderer.draw_filter_items(
                        items,
                        self.ui.nav.arcade_filter.selected,
                        self.ui.nav.arcade_filter.visual_index,
                        force_arcade_redraw,
                    )
                } else {
                    self.out.arcade_list_renderer.draw(
                        active_arcade_games,
                        self.ui.nav.arcade.selected,
                        self.ui.nav.arcade.visual_index,
                        force_arcade_redraw,
                    )
                };
                drop(arcade_list_profile_pmu);
                drop(gui_arcade_pmu);
                update
            } else {
                None
            };
        let arcade_list_update_us = arcade_list_update_start.elapsed().as_micros();
        let mut portrait_arcade_list_pixels = 0_u64;
        let mut portrait_arcade_list_bytes = 0_u64;
        let preview_blit_start = Instant::now();
        let gui_preview_pmu = self
            .diag
            .gui_profiling
            .phase_span(gui_custom_selection.preview_composition);
        let empty_base_cached_rect = if (self.out.layout.is_portrait()
            || preview_direct_present_enabled())
            && self.ui.preview_route.allows_hdmi_preview()
            && project.composition_decision.allow_preview_blit
            && !self.lib.memory_guard.active()
            && self.lib.preview.empty_base_commit_pending()
        {
            Some(if self.out.layout.is_portrait() {
                layer_target.clear_presentation_preview()
            } else {
                layer_target.clear_cached_preview()
            })
        } else {
            None
        };
        if should_start_preview_compositor(
            project.wants_preview,
            self.ui.preview_route.allows_hdmi_preview(),
            project.composition_decision.allow_preview_blit,
            self.lib.memory_guard.active(),
            self.out.preview_compositor_start_attempted,
        ) {
            self.out.preview_compositor_start_attempted = true;
            match PreviewCompositor::start() {
                Ok(worker) => self.out.preview_compositor = Some(worker),
                Err(error) => crate::ui_errln!("preview_compositor_start_failed: {error}"),
            }
        }
        let (
            raw_preview,
            preview_transition_trace,
            preview_compositor_pending,
            preview_compositor_telemetry,
        ) = if project.wants_preview
            && project.composition_decision.allow_preview_blit
            && !self.lib.memory_guard.active()
        {
            let logical_ui = UiDisplay::for_framebuffer(
                self.out.layout.logical_w(),
                self.out.layout.logical_h(),
            );
            let preview_rect = preview_screen_rect(&logical_ui);
            (
                preview_bbox_invalidation,
                preview_rect_invalidation,
                preview_false_positive_invalidation,
            ) = custom_damage_invalidation_comparison(
                logical_slint_rect,
                &logical_slint_damage_for_custom,
                preview_rect,
                project.full_frame_present,
            );
            layer_target.blit_raw_preview_if_needed(
                &mut self.lib.preview,
                &mut self.diag.preview_transition,
                self.out.frame_clock.elapsed(),
                logical_slint_rect,
                project.full_frame_present,
                self.out.preview_compositor.as_mut(),
            )
        } else {
            (None, PreviewTransitionTrace::default(), false, None)
        };
        if preview_compositor_pending {
            self.env.window.request_redraw();
        }
        drop(gui_preview_pmu);
        let preview_blit_us = preview_blit_start.elapsed().as_micros();
        let portrait_preview_rotation_pixels = if self.out.layout.is_portrait() {
            raw_preview
                .map(|present| match present {
                    RawPreviewPresent::Cached(rect) | RawPreviewPresent::Direct(rect) => rect,
                })
                .map(|rect| (rect.width() as u64).saturating_mul(u64::from(rect.rows())))
                .unwrap_or(0)
        } else {
            0
        };
        let portrait_preview_blend_pixels = if self.out.layout.is_portrait() {
            u64::from(preview_transition_trace.fade.pixels)
        } else {
            0
        };
        if preview_transition_trace.active {
            self.env.window.request_redraw();
        }
        let mut crt_backdrop_full_damage = None;
        let mut crt_backdrop_work_trace = crate::crt_backdrop::CrtBackdropWorkTrace::default();
        let mut crt_backdrop_copy_us = 0_u64;
        let mut crt_backdrop_list_overlay_us = 0_u64;
        let mut crt_backdrop_copy_pixels = 0_u32;
        let mut crt_backdrop_list_overlay_pixels = 0_u32;
        let mut crt_backdrop_list_restore_pixels = 0_u32;
        let mut crt_backdrop_list_foreground_pixels = 0_u32;
        let transition_id = self
            .lib
            .preview
            .raw_transition_frame()
            .as_ref()
            .map(|frame| frame.transition_id);
        // Releasing a full-screen transition performs one live full Slint
        // raster. That raster owns the CRT placeholder background, so restore
        // the settled custom backdrop in the same frame before the list layer.
        let force_crt_backdrop_repaint = full_screen_transition_release_raster_rendered;
        if let Some(backdrop) = self.out.crt_backdrop.as_mut() {
            backdrop.set_hub_mode(self.ui.nav.is_system_hub().then(|| {
                (
                    self.ui.nav.system_hub_selected,
                    Rgb565Pixel(
                        crate::launcher_presentation::device_reveal_spec(
                            self.ui.nav.device_kind(),
                            true,
                            true,
                        )
                        .accent,
                    ),
                )
            }));
            let compose_start = Instant::now();
            let crt_arcade_layout = CrtArcadeLayout::for_layout(
                self.out.layout,
                self.out.crt_metrics,
                self.ui
                    .nav
                    .arcade_search
                    .is_active(&self.ui.nav.arcade_filter.active),
            );
            let result = backdrop.compose(
                project.crt_backdrop_eligible,
                force_crt_backdrop_repaint,
                project.arcade_turbo_active || self.out.director.navigation.is_active(),
                self.ui.nav.arcade.selected,
                transition_id,
                (project.preview_cache_state_before_composition == "exact")
                    .then(|| self.lib.preview.selected_backdrop_source())
                    .flatten(),
                self.out.frame_clock.elapsed(),
                layer_target.presentation_pixels_mut(),
                self.out.layout,
                crt_arcade_layout,
                self.out.crt_metrics,
            );
            crt_backdrop_work_trace = result.trace;
            crt_backdrop_copy_us = compose_start
                .elapsed()
                .as_micros()
                .saturating_sub(u128::from(crt_backdrop_work_trace.blend_us))
                .min(u128::from(u64::MAX)) as u64;
            crt_backdrop_copy_pixels = backdrop
                .width()
                .saturating_mul(backdrop.height())
                .min(u32::MAX as usize) as u32;
            if result.full_damage {
                crt_backdrop_full_damage = Some(DirtyRect {
                    x0: 0,
                    y0: 0,
                    x1: self.out.layout.composition_w(),
                    y1: self.out.layout.composition_h(),
                });
            }
            if crt_backdrop_work_trace.active {
                self.env.window.request_redraw();
            }
        }
        let navigation_transition_composition_active = self.out.director.navigation.is_active();
        if !navigation_transition_composition_active {
            self.fx.settings_cog_render_ahead.clear();
        }
        let navigation_settings_physical_space =
            self.out.director.navigation.settings_physical_space();
        let navigation_transition_frame_active = navigation_transition_composition_active
            && self.out.director.navigation.frame().phase != NavigationTransitionPhase::Capture;
        let (
            navigation_transition_route,
            navigation_transition_direction,
            navigation_transition_renderer,
        ) = if navigation_transition_frame_active {
            self.out
                .director
                .navigation
                .route()
                .zip(self.out.director.navigation.request())
                .map_or(("", "", ""), |(route, request)| {
                    (
                        route.label(),
                        request.direction.label(),
                        request.renderer_label(),
                    )
                })
        } else {
            ("", "", "")
        };
        let navigation_transition_frame_started =
            navigation_transition_frame_active.then_some(pre_input.loop_start);
        let mut navigation_transition_render_us = 0u128;
        let mut navigation_logical_frame_rendered = false;
        #[cfg(feature = "tooling")]
        let mut navigation_frame_rendered = false;
        #[cfg(feature = "tooling")]
        let mut navigation_endpoint_rendered = false;
        if navigation_transition_composition_active {
            let navigation_transition_compositor_started = Instant::now();
            let destination_committed = self.out.director.destination_committed();
            let mut render_transition_frame = !navigation_capture_source_carrier_rendered;
            if destination_committed && !self.out.director.navigation.destination_ready() {
                #[cfg(feature = "tooling")]
                let measure_destination = begin
                    .tooling_frame_evidence
                    .as_ref()
                    .is_some_and(|frame| frame.phases_enabled);
                #[cfg(feature = "tooling")]
                let mut destination_stage_us = [0; 4];
                #[cfg(feature = "tooling")]
                let _profile = mister_magik_framebuffer_scenes::launcher_profile::span(
                    "transition.destination-layers",
                );
                let controlled_destination_raster_ready = full_screen_controlled_capture_rendered
                    || (self.out.director.chart.owner()
                        == Some(FullScreenTransitionOwner::Navigation)
                        && self.out.director.chart.capture_issued());
                let destination_raster_ready =
                    project.composition_decision.prepare_navigation_destination
                        && controlled_destination_raster_ready;
                let mut destination_layers_ready = destination_raster_ready
                    && self.ui.nav.screen != Screen::Arcade
                    && (self.ui.nav.screen != Screen::Home
                        || self.fx.launcher_card_home.as_ref().is_none_or(|session| {
                            session.content_ready(
                                super::launcher_card_home::scene_for_display(
                                    self.env.ui,
                                    self.out.layout,
                                ),
                                &self.lib.card_level,
                            )
                        }));
                if destination_raster_ready && self.ui.nav.screen == Screen::Arcade {
                    #[cfg(feature = "tooling")]
                    let preview_started = measure_destination.then(Instant::now);
                    let preview_expected =
                        selected_arcade_game_has_preview(&self.ui.nav, &self.lib.catalog);
                    let preview_exact = preview_expected
                        && !self.lib.preview.terminal_empty()
                        && self.lib.preview.trace_cache_state() == "exact"
                        && self.lib.preview.raw_frame_status() == PreviewRawFrameStatus::Ready;
                    let preview_surface_ready = if preview_exact {
                        if self.out.director.navigation.settings_physical_space() {
                            let (ready, publication) = layer_target.compose_exact_preview_physical(
                                &self.lib.preview,
                                self.out.launcher_preview_publication.as_ref(),
                                &mut self.out.launcher_preview_version,
                            );
                            if let Some(publication) = publication {
                                self.out.launcher_preview_publication = Some(publication);
                            }
                            ready
                        } else {
                            match layer_target.compose_exact_preview(&self.lib.preview) {
                                Some(RawPreviewPresent::Cached(_)) => true,
                                Some(RawPreviewPresent::Direct(rect)) => {
                                    layer_target.compose_direct_preview_rect(rect) > 0
                                }
                                None => false,
                            }
                        }
                    } else {
                        // Capture a clean list destination now. If an exact preview
                        // arrives later, the normal Arcade presentation path adopts it.
                        if self.out.director.navigation.settings_physical_space() {
                            let _ = layer_target.clear_presentation_preview();
                        } else {
                            let _ = layer_target.clear_cached_preview();
                        }
                        true
                    };
                    #[cfg(feature = "tooling")]
                    if let Some(started) = preview_started {
                        destination_stage_us[0] = u128_to_u64(started.elapsed().as_micros());
                    }
                    if preview_surface_ready
                        && (project.arcade_status_only || self.ui.nav.is_system_hub())
                    {
                        // The Slint status panel is the complete destination for
                        // loading, empty and failed Arcade. Do not paint the old
                        // custom "NO GAMES" layer over it.
                        destination_layers_ready = true;
                    }
                    if preview_surface_ready
                        && !project.arcade_status_only
                        && !self.ui.nav.is_system_hub()
                    {
                        #[cfg(feature = "tooling")]
                        let list_started = measure_destination.then(Instant::now);
                        configure_arcade_list_renderer_geometry(
                            &mut self.out.arcade_list_renderer,
                            &self.ui.nav,
                            self.env.ui,
                        );
                        let list_update = self.out.arcade_list_renderer.draw(
                            active_system_game_view(&self.lib.catalog, &self.ui.nav),
                            self.ui.nav.arcade.selected,
                            self.ui.nav.arcade.visual_index,
                            true,
                        );
                        #[cfg(feature = "tooling")]
                        let list_draw_done = measure_destination.then(Instant::now);
                        if let Some(update) = list_update {
                            if self.out.director.navigation.settings_physical_space() {
                                let _ = layer_target.reclaim_arcade_publication(
                                    &mut self.out.arcade_list_renderer,
                                    &mut self.out.launcher_arcade_publication,
                                );
                                self.out.launcher_arcade_content_generation = self
                                    .out
                                    .launcher_arcade_content_generation
                                    .wrapping_add(1)
                                    .max(1);
                                let (_, publication) = layer_target
                                    .compose_arcade_list_direct_layer_snapshot(
                                        &mut self.out.arcade_list_renderer,
                                        update,
                                        self.lib.catalog_version as u64,
                                        self.out.launcher_arcade_version,
                                        self.out.launcher_arcade_scroll_offset,
                                        self.out.launcher_arcade_content_generation,
                                    );
                                self.out.launcher_arcade_publication = publication;
                            } else {
                                let _ = layer_target.compose_arcade_list_snapshot_update(
                                    &mut self.out.arcade_list_renderer,
                                    update,
                                );
                            }
                        }
                        #[cfg(feature = "tooling")]
                        if let Some(started) = list_started {
                            destination_stage_us[1] = u128_to_u64(started.elapsed().as_micros());
                            if let Some(draw_done) = list_draw_done
                                && let Some(frame) = begin.tooling_frame_evidence.as_mut()
                            {
                                frame.destination_list_us = Some([
                                    u128_to_u64(draw_done.duration_since(started).as_micros()),
                                    u128_to_u64(draw_done.elapsed().as_micros()),
                                ]);
                            }
                        }
                        destination_layers_ready = true;
                    }
                }
                let mut status_quiesce = None;
                if destination_layers_ready {
                    let worker_active = self.diag.frame_accounting.runtime_status_worker_active();
                    if let Some(pending) = self.out.director.pending.as_mut() {
                        let started = pending
                            .status_quiesce_started_at
                            .get_or_insert_with(Instant::now);
                        let waited = started.elapsed();
                        let timed_out = worker_active && waited >= NAVIGATION_STATUS_QUIESCE_LIMIT;
                        status_quiesce = Some((waited, timed_out));
                        if worker_active && !timed_out {
                            destination_layers_ready = false;
                        }
                    }
                }
                if destination_layers_ready
                    && self.out.crt_layout
                    && self.ui.nav.screen == Screen::Arcade
                    && self
                        .out
                        .director
                        .navigation
                        .request()
                        .is_some_and(|r| r.is_device_card())
                    && let Some(source) = self.lib.preview.selected_backdrop_source()
                {
                    destination_layers_ready =
                        self.out.crt_backdrop.as_ref().is_some_and(|backdrop| {
                            backdrop.source_ready(&source, self.out.layout)
                        });
                }
                if destination_layers_ready {
                    if let Some((waited, timed_out)) = status_quiesce {
                        self.out.director.navigation.note_pending_status_quiesce(
                            waited.as_micros().min(u64::MAX as u128) as u64,
                            timed_out,
                        );
                    }
                    // Restore Home immediately before capture: intermediate transition
                    // work can overwrite the target after the earlier full raster.
                    if project.custom_home_active
                        && let Some(session) = self.fx.launcher_card_home.as_mut()
                    {
                        #[cfg(feature = "tooling")]
                        let home_started = measure_destination.then(Instant::now);
                        let _ = layer_target.render_custom_home(
                            self.env.window,
                            session.render(),
                            true,
                            None,
                        );
                        #[cfg(feature = "tooling")]
                        if let Some(started) = home_started {
                            destination_stage_us[2] = u128_to_u64(started.elapsed().as_micros());
                        }
                    }
                    #[cfg(feature = "tooling")]
                    let snapshot_started = measure_destination.then(Instant::now);
                    if self.out.crt_layout {
                        self.out.director.navigation.update_device_reveal_image(
                            selected_device_reveal_image(
                                &self.lib.preview,
                                self.out.crt_backdrop.as_ref(),
                                self.out.layout,
                            ),
                        );
                    }
                    self.out.director.navigation.update_device_reveal_backdrop(
                        self.out.crt_backdrop.as_ref().map_or(&[], |b| b.pixels()),
                    );
                    // The first Slint destination raster can be expensive, but
                    // animation time only moves per produced frame, so cold
                    // preparation is never spent as motion.
                    let destination = if self.out.director.navigation.settings_physical_space() {
                        layer_target.presentation_frame_view().pixels()
                    } else {
                        layer_target.cached_frame_view().pixels()
                    };
                    if !self
                        .out
                        .director
                        .capture_navigation_destination(destination, pre_input.animation_us)
                    {
                        render_transition_frame = false;
                    }
                    self.out.director.navigation.tick(pre_input.animation_us);
                    #[cfg(feature = "tooling")]
                    if let Some(started) = snapshot_started {
                        destination_stage_us[3] = u128_to_u64(started.elapsed().as_micros());
                    }
                }
                #[cfg(feature = "tooling")]
                if let Some(frame) = begin.tooling_frame_evidence.as_mut()
                    && measure_destination
                {
                    frame.destination_stage_us = Some(destination_stage_us);
                }
            }
            if render_transition_frame {
                let gui_navigation_pmu = self
                    .diag
                    .gui_profiling
                    .phase_span(gui_custom_selection.navigation_transition_raster);
                let mut rendered_direct = false;
                if self.out.director.navigation.settings_physical_space() {
                    if (self.out.layout.logical_w(), self.out.layout.logical_h()) == (960, 540)
                        && let Some(input) =
                            self.out.director.navigation.settings_cog_render_input()
                    {
                        const SETTINGS_RENDER_LEAD_VBLANKS: u64 = 2;
                        let lead_ms = self
                            .out
                            .pacer
                            .period_us()
                            .saturating_mul(SETTINGS_RENDER_LEAD_VBLANKS)
                            .saturating_add(999)
                            .saturating_div(1_000)
                            .min(u64::from(u32::MAX)) as u32;
                        let t_ms = match input.direction {
                            NavigationTransitionDirection::Forward => input
                                .t_ms
                                .saturating_add(lead_ms)
                                .min(mister_magik_framebuffer_scenes::settings_cog::SETTINGS_COG_DURATION_MS),
                            NavigationTransitionDirection::Reverse => {
                                input.t_ms.saturating_sub(lead_ms)
                            }
                        };
                        self.fx.settings_cog_render_ahead.submit(
                            input.launcher,
                            input.settings,
                            input.cog,
                            SettingsFrameRequest {
                                target_vblank: self
                                    .out
                                    .pacer
                                    .hits()
                                    .saturating_add(SETTINGS_RENDER_LEAD_VBLANKS),
                                t_ms,
                            },
                        );
                    }
                    let expected_vblank = self.out.pacer.hits().saturating_add(1);
                    let mut prepared = self
                        .fx
                        .settings_cog_render_ahead
                        .take_for_vblank(expected_vblank);
                    let mut direct_render_timing = None;
                    match self.out.launcher_presenter.try_render_direct_hidden_frame(
                        self.env.f,
                        self.env.display_session,
                        |_, pixels| {
                            let started = Instant::now();
                            let start_phase_us = self.out.pacer.age_since_last_hit_us(started);
                            let rendered = if let Some(frame) = prepared.as_ref() {
                                let output = slint_rgb565_as_shared_mut(pixels);
                                if output.len() == frame.pixels().len() {
                                    output.copy_from_slice(frame.pixels());
                                    true
                                } else {
                                    false
                                }
                            } else {
                                // The cog blends by reading its output. Keep those
                                // reads in the existing cached working buffer, then
                                // write linearly into the granted scanout mapping.
                                self.out.director.navigation.render().is_ok_and(|frame| {
                                    if pixels.len() != frame.len() {
                                        return false;
                                    }
                                    pixels.copy_from_slice(frame);
                                    true
                                })
                            };
                            direct_render_timing = Some((started, Instant::now(), start_phase_us));
                            rendered
                        },
                    ) {
                        Ok(Some(completed)) => {
                            let (direct_render_started, direct_render_completed, start_phase_us) =
                                direct_render_timing.expect("successful direct render was timed");
                            if let Some(frame) = prepared.as_ref() {
                                frame_production_trace.class = FrameProductionClass::Prepared;
                                frame_production_trace.sequence = frame.request().target_vblank;
                                frame_production_trace.render_wall_us = frame.render_us();
                                frame_production_completed_at = Some(frame.completed_at());
                            } else {
                                frame_production_trace.class =
                                    FrameProductionClass::SynchronousAnimation;
                                frame_production_trace.sequence = completed.grant.generation;
                                frame_production_trace.render_wall_us = direct_render_completed
                                    .saturating_duration_since(direct_render_started)
                                    .as_micros()
                                    .try_into()
                                    .unwrap_or(u64::MAX);
                                frame_production_completed_at = Some(direct_render_completed);
                            }
                            frame_production_trace.render_start_phase_us = start_phase_us;
                            completed_hidden_frame_for_present = Some(completed);
                            rendered_direct = true;
                        }
                        Ok(None) => {}
                        Err(failure) => self.out.launcher_presenter.fail_latch_completion(failure),
                    }
                    if let Some(frame) = prepared.take() {
                        self.fx.settings_cog_render_ahead.recycle(frame);
                    }
                    if !rendered_direct {
                        let rendered = self
                            .out
                            .director
                            .navigation
                            .render_into(layer_target.presentation_pixels_mut());
                        #[cfg(feature = "tooling")]
                        {
                            navigation_frame_rendered = rendered.is_ok();
                        }
                        #[cfg(not(feature = "tooling"))]
                        let _ = rendered;
                    } else {
                        #[cfg(feature = "tooling")]
                        {
                            navigation_frame_rendered = true;
                        }
                    }
                } else if self
                    .out
                    .director
                    .navigation
                    .render_into(layer_target.presentation_pixels_mut())
                    .is_ok()
                {
                    navigation_logical_frame_rendered = true;
                    #[cfg(feature = "tooling")]
                    {
                        navigation_frame_rendered = true;
                    }
                }
                drop(gui_navigation_pmu);
            }
            project.full_frame_present = true;
            self.env.window.request_redraw();
            if self.out.director.navigation.frame().phase == NavigationTransitionPhase::Settled {
                let endpoint_is_live = navigation_home_endpoint_is_live(
                    self.out.director.navigation.route(),
                    self.out.director.navigation.request(),
                    self.out.director.navigation.frame().endpoint,
                );
                let completion = self.out.director.finish_navigation();
                #[cfg(feature = "tooling")]
                {
                    navigation_endpoint_rendered =
                        navigation_frame_rendered && completion.is_some();
                }
                full_screen_transition_live_endpoint_rendered =
                    endpoint_is_live && completion.is_some();
                let pending = self.out.director.pending.take();
                if completion.is_some_and(|completion| {
                    completion.endpoint == NavigationTransitionEndpoint::Source
                }) {
                    if let Some(entry) = self.lib.pending_collection_entry.take() {
                        self.lib.preview.cancel_system_entry_preview();
                        self.ui
                            .nav
                            .catalog_system_hydration_finished(&entry.collection_id);
                        self.diag.arcade_entry_latency.cancel_enter();
                    }
                    if let Some(pending) = pending {
                        let before = LauncherProjectionKey::from_nav(&self.ui.nav);
                        self.ui
                            .nav
                            .restore_navigation_transition_state(pending.source_state);
                        let after = LauncherProjectionKey::from_nav(&self.ui.nav);
                        if before != after {
                            self.lib
                                .media_session
                                .note_nav_change(&before, &after, Instant::now());
                        }
                        self.ui.navigation_source_bridge_sync_pending = true;
                        self.env.window.request_redraw();
                    }
                }
            }
            navigation_transition_render_us = navigation_transition_compositor_started
                .elapsed()
                .as_micros();
            self.out
                .director
                .navigation
                .note_frame_work_us(navigation_transition_render_us.min(u64::MAX as u128) as u64);
            sync_navigation_transition_active(&self.env.app, &self.out.director.navigation);
        }
        let effect_label_us = navigation_transition_render_us;
        let navigation_telemetry = self.out.director.navigation.telemetry();
        let mut custom_draw_trace = LauncherCustomDrawTrace {
            arcade_bbox_invalidation,
            arcade_rect_invalidation,
            arcade_false_positive_invalidation,
            preview_bbox_invalidation,
            preview_rect_invalidation,
            preview_false_positive_invalidation,
            arcade_list_update_us,
            portrait_arcade_list_pixels,
            portrait_arcade_list_bytes,
            preview_blit_us,
            portrait_preview_rotation_pixels,
            portrait_preview_blend_pixels,
            portrait_preview_worker_queue_replacements: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.queue_replacements)
                .unwrap_or(0),
            portrait_preview_worker_result_replacements: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.result_replacements)
                .unwrap_or(0),
            portrait_preview_worker_stale_results: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.stale_results)
                .unwrap_or(0),
            portrait_preview_worker_age_us: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.worker_age_us)
                .unwrap_or(0),
            portrait_preview_worker_generation_lag: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.generation_lag)
                .unwrap_or(0),
            portrait_preview_worker_affinity_status: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.affinity_status)
                .unwrap_or("inactive"),
            portrait_preview_worker_errors: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.worker_errors)
                .unwrap_or(0),
            portrait_preview_worker_adoption_failures: preview_compositor_telemetry
                .as_ref()
                .map(|telemetry| telemetry.adoption_failures)
                .unwrap_or(0),
            portrait_preview_worker_alive: preview_compositor_telemetry
                .as_ref()
                .is_some_and(|telemetry| telemetry.worker_alive),
            crt_backdrop_prepare_us: crt_backdrop_work_trace.prepare_us,
            crt_backdrop_prepare_pixels: crt_backdrop_work_trace.prepare_pixels,
            crt_backdrop_blend_us: crt_backdrop_work_trace.blend_us,
            crt_backdrop_blend_pixels: crt_backdrop_work_trace.blend_pixels,
            crt_backdrop_copy_us,
            crt_backdrop_copy_pixels,
            crt_backdrop_list_overlay_us,
            crt_backdrop_list_overlay_pixels,
            crt_backdrop_alpha_bucket: crt_backdrop_work_trace.alpha_bucket,
            crt_backdrop_active: crt_backdrop_work_trace.active,
            crt_backdrop_selected: self.ui.nav.arcade.selected,
            crt_backdrop_transition_id: self
                .out
                .crt_backdrop
                .as_ref()
                .and_then(CrtBackdropController::transition_id)
                .unwrap_or(0),
            crt_backdrop_cache_state: project.preview_cache_state_before_composition,
            effect_label_us,
            navigation_transition_base_copy_us: self
                .out
                .director
                .navigation
                .last_render_stats()
                .base_copy_us as u128,
            navigation_transition_settings_blit_us: self
                .out
                .director
                .navigation
                .last_render_stats()
                .settings_blit_us as u128,
            navigation_transition_card_scale_us: self
                .out
                .director
                .navigation
                .last_render_stats()
                .card_scale_us as u128,
            navigation_transition_destination_reveal_us: self
                .out
                .director
                .navigation
                .last_render_stats()
                .destination_reveal_us
                as u128,
            navigation_transition_overlay_us: self
                .out
                .director
                .navigation
                .last_render_stats()
                .overlay_us as u128,
            navigation_transition_edge: navigation_transition_route,
            navigation_transition_route,
            navigation_transition_direction,
            navigation_transition_renderer,
            navigation_transition_orientation: if navigation_transition_frame_active {
                self.ui.nav.settings.screen_orientation.id()
            } else {
                ""
            },
            navigation_snapshot_locked: navigation_snapshot_locked_before_render,
            navigation_slint_render_called: !self.fx.screensaver.active
                && !navigation_snapshot_locked_before_render,
            navigation_status_quiesce_wait_us: navigation_telemetry.status_quiesce_wait_us,
            navigation_status_quiesce_timeout: navigation_telemetry.status_quiesce_timeout,
            ..LauncherCustomDrawTrace::default()
        };
        let cpu_custom_draw_done = FrameAnalyticsCpuStamp::capture(pre_input.frame_analytics_mode);
        let custom_draw_done = Instant::now();
        #[cfg(feature = "tooling")]
        super::launcher_frame_accounting::capture_evidence_cpu(
            &mut begin.tooling_frame_evidence,
            3,
            self.out.run_start,
        );
        if !self.diag.first_render_logged {
            self.diag.first_render_logged = true;
            boot_analytics::event(
                "first_render",
                format!(
                    "frame={frames} dirty_rect={}",
                    format_dirty_rect(this_rect),
                    frames = self.out.frames
                ),
            );
        }
        let full_rect = DirtyRect {
            x0: 0,
            y0: 0,
            x1: self.out.layout.composition_w(),
            y1: self.out.layout.composition_h(),
        };
        let raw_preview_cached_rect = raw_preview.and_then(RawPreviewPresent::cached_rect);
        let logical_raw_preview_rect = (!self.out.layout.is_portrait())
            .then_some(raw_preview_cached_rect)
            .flatten();
        let physical_raw_preview_rect = self
            .out
            .layout
            .is_portrait()
            .then_some(raw_preview_cached_rect)
            .flatten();
        let logical_empty_preview_rect = (!self.out.layout.is_portrait())
            .then_some(empty_base_cached_rect)
            .flatten();
        let physical_empty_preview_rect = self
            .out
            .layout
            .is_portrait()
            .then_some(empty_base_cached_rect)
            .flatten();
        let raw_preview_direct_rect = raw_preview.and_then(RawPreviewPresent::direct_rect);
        if let Some(rect) = raw_preview_direct_rect {
            self.out.launcher_preview_version =
                self.out.launcher_preview_version.wrapping_add(1).max(1);
            if !self.out.crt_layout {
                let state = PhysicalLayerState::new(rect, self.out.launcher_preview_version);
                self.out.launcher_preview_publication = layer_target.capture_preview_publication(
                    state,
                    Some(PhysicalLayerUpdate::Full(rect)),
                    self.out.launcher_preview_version,
                );
            }
        } else if let Some((state, content_generation)) = reclaimed_preview_publication
            && !self.out.crt_layout
            && self.out.launcher_preview_publication.is_none()
        {
            self.out.launcher_preview_publication =
                layer_target.capture_preview_publication(state, None, content_generation);
        }
        let mut physical_arcade_rect = None;
        let mut direct_arcade_update = None;
        if !project.crt_backdrop_eligible {
            self.out.crt_arcade_overlay.clear();
        } else if crt_backdrop_work_trace.active || crt_backdrop_full_damage.is_some() {
            self.out.crt_arcade_overlay.invalidate();
        }
        let cached_arcade_rect = if project.crt_backdrop_eligible && !self.ui.nav.is_system_hub() {
            arcade_list_rect
                .or_else(|| {
                    crt_backdrop_full_damage
                        .map(|_| ArcadeListUpdate::Full(self.out.arcade_list_renderer.dirty_rect()))
                })
                .and_then(|update| {
                    let rect = arcade_update_dirty_rect(&update);
                    let crt_overlay_profile_pmu =
                        mister_magik_perf_events::sampled_span("gui.custom.crt-list-overlay");
                    let composition = self
                        .out
                        .crt_backdrop
                        .as_ref()
                        .map(|backdrop| {
                            layer_target.compose_arcade_list_over_backdrop(
                                &mut self.out.arcade_list_renderer,
                                backdrop.pixels(),
                                update,
                                backdrop.backdrop_revision(),
                                self.lib.catalog_version as u64,
                                crt_backdrop_full_damage.is_some(),
                                !backdrop.is_transitioning() && !crt_backdrop_work_trace.active,
                                project.full_frame_present || crt_backdrop_full_damage.is_some(),
                                &mut self.out.crt_arcade_overlay,
                            )
                        })
                        .unwrap_or_default();
                    crt_backdrop_list_overlay_us = composition.elapsed_us;
                    crt_backdrop_list_restore_pixels = composition.restored_pixels;
                    crt_backdrop_list_foreground_pixels = composition.foreground_pixels;
                    crt_backdrop_list_overlay_pixels = composition
                        .restored_pixels
                        .saturating_add(composition.foreground_pixels);
                    portrait_arcade_list_pixels = u64::from(crt_backdrop_list_overlay_pixels);
                    portrait_arcade_list_bytes = portrait_arcade_list_pixels.saturating_mul(2);
                    drop(crt_overlay_profile_pmu);
                    if self.out.layout.is_portrait() {
                        physical_arcade_rect =
                            Some(self.out.layout.logical_rect_to_composition(rect));
                        None
                    } else {
                        Some(rect)
                    }
                })
        } else if self.out.layout.is_portrait() {
            arcade_list_rect.and_then(|update| {
                let _ = layer_target.reclaim_arcade_publication(
                    &mut self.out.arcade_list_renderer,
                    &mut self.out.launcher_arcade_publication,
                );
                let (composition, physical_update) = layer_target.compose_arcade_list_direct_layer(
                    &mut self.out.arcade_list_renderer,
                    update,
                    self.lib.catalog_version as u64,
                );
                custom_draw_trace.persistent_arcade_composition =
                    self.out.arcade_list_renderer.persistent_composition_trace();
                portrait_arcade_list_bytes = composition.bytes as u64;
                portrait_arcade_list_pixels = composition.bytes.saturating_div(2) as u64;
                direct_arcade_update = Some(physical_update);
                None
            })
        } else if self.out.crt_layout {
            arcade_list_rect.map(|update| {
                let rect = arcade_update_dirty_rect(&update);
                let composition = layer_target
                    .compose_arcade_list_update(&mut self.out.arcade_list_renderer, update);
                portrait_arcade_list_bytes = composition.bytes as u64;
                portrait_arcade_list_pixels = composition.bytes.saturating_div(2) as u64;
                rect
            })
        } else {
            None
        };
        let layer_arcade_update = direct_arcade_update.or(arcade_list_rect);
        if !self.out.crt_layout {
            update_arcade_physical_layer_tracking(
                &mut self.out.launcher_arcade_version,
                &mut self.out.launcher_arcade_scroll_offset,
                layer_arcade_update,
                self.out.layout.is_portrait(),
            );
        }
        if self.out.layout.is_portrait()
            && let Some(update) = direct_arcade_update
            && let Some(rect) = self
                .out
                .arcade_list_renderer
                .persistent_oriented_layer_view()
                .map(PhysicalLayerView::rect)
        {
            self.out.launcher_arcade_content_generation = self
                .out
                .launcher_arcade_content_generation
                .wrapping_add(1)
                .max(1);
            let state = PhysicalLayerState::new(rect, self.out.launcher_arcade_version)
                .with_content_offset(self.out.launcher_arcade_scroll_offset);
            self.out.launcher_arcade_publication = layer_target.capture_arcade_publication(
                &mut self.out.arcade_list_renderer,
                state,
                Some(update),
                self.out.launcher_arcade_content_generation,
            );
        }
        custom_draw_trace.crt_backdrop_copy_us = crt_backdrop_copy_us;
        custom_draw_trace.crt_backdrop_copy_pixels = crt_backdrop_copy_pixels;
        custom_draw_trace.crt_backdrop_list_overlay_us = crt_backdrop_list_overlay_us;
        custom_draw_trace.crt_backdrop_list_overlay_pixels = crt_backdrop_list_overlay_pixels;
        custom_draw_trace.crt_backdrop_list_restore_pixels = crt_backdrop_list_restore_pixels;
        custom_draw_trace.crt_backdrop_list_foreground_pixels = crt_backdrop_list_foreground_pixels;
        custom_draw_trace.portrait_arcade_list_pixels = portrait_arcade_list_pixels;
        custom_draw_trace.portrait_arcade_list_bytes = portrait_arcade_list_bytes;
        let physical_custom_damage = accepted_screensaver_frame.then_some(this_rect).flatten();
        let preview_layer_desired = should_desire_preview_direct_layer(
            project.wants_preview_layer,
            project.composition_decision.allow_preview_blit,
            project.wants_preview,
            preview_compositor_pending,
            self.out.launcher_preview_publication.is_some()
                || layer_target.direct_preview_rect().is_some(),
            raw_preview_direct_rect.is_some(),
        );
        let mut preview_publication =
            if !self.out.crt_layout && preview_layer_desired && preview_direct_present_enabled() {
                self.out
                    .launcher_preview_publication
                    .as_ref()
                    .filter(|publication| {
                        publication.layout_generation() == layer_target.output_layout_generation()
                            && publication.layout_epoch() == layer_target.output_layout_epoch()
                    })
                    .and_then(|publication| {
                        publication.for_frame(
                            publication.state(),
                            raw_preview_direct_rect.map(PhysicalLayerUpdate::Full),
                        )
                    })
            } else {
                None
            };
        let preview_desired = preview_publication
            .as_ref()
            .map(PhysicalLayerPublication::state);
        let mut arcade_publication = if self.out.layout.is_portrait()
            && !self.out.crt_layout
            && should_desire_direct_layer(
                project.wants_arcade_list,
                project.composition_decision.allow_arcade_list_blit,
            ) {
            self.out
                .launcher_arcade_publication
                .as_ref()
                .filter(|publication| {
                    publication.layout_generation() == layer_target.output_layout_generation()
                        && publication.layout_epoch() == layer_target.output_layout_epoch()
                })
                .and_then(|publication| {
                    publication.for_frame(publication.state(), direct_arcade_update)
                })
        } else {
            None
        };
        let arcade_desired = if self.out.layout.is_portrait() {
            arcade_publication
                .as_ref()
                .map(PhysicalLayerPublication::state)
        } else if !self.out.crt_layout
            && should_desire_direct_layer(
                project.wants_arcade_list,
                project.composition_decision.allow_arcade_list_blit,
            )
        {
            let rect = self.out.arcade_list_renderer.dirty_rect();
            Some(
                PhysicalLayerState::new(rect, self.out.launcher_arcade_version)
                    .with_content_offset(self.out.launcher_arcade_scroll_offset),
            )
        } else {
            None
        };
        let mut logical_custom_damage = DirtyRectList::new();
        if navigation_logical_frame_rendered {
            logical_custom_damage.push(DirtyRect {
                x0: 0,
                y0: 0,
                x1: self.out.layout.logical_w(),
                y1: self.out.layout.logical_h(),
            });
        } else if slint_damage.is_empty() && physical_custom_damage.is_none() {
            logical_custom_damage.push_if_some(this_rect);
        }
        logical_custom_damage.push_if_some(logical_empty_preview_rect);
        logical_custom_damage.push_if_some(logical_raw_preview_rect);
        logical_custom_damage.push_if_some(cached_arcade_rect);
        let orientation_damage_rects_before = logical_custom_damage.len() as u32;
        debug_assert!(!self.out.layout.is_portrait() || logical_custom_damage.is_empty());
        let mapped_custom_damage = logical_custom_damage;
        let mut cached_damage = if project.full_frame_present || navigation_settings_physical_space
        {
            DirtyRectList::from_one(full_rect)
        } else {
            let mut damage = slint_damage;
            damage.extend_from(&mapped_custom_damage);
            damage.push_if_some(physical_custom_damage);
            damage.push_if_some(physical_arcade_rect);
            damage.push_if_some(physical_empty_preview_rect);
            damage.push_if_some(physical_raw_preview_rect);
            damage.push_if_some(crt_backdrop_full_damage);
            damage
        };
        // Retain the v1 telemetry field for schema compatibility. Native Slint
        // and custom layer composition no longer run a post-raster rotation.
        let orientation_damage_rotation_us = 0;
        let orientation_damage_rects_after_rotation = cached_damage.len() as u32;
        if self.out.director.orientation.is_active() {
            let orientation_started = Instant::now();
            let transition_from = self.out.director.orientation.from();
            let transition_to = self.out.director.orientation.to();
            custom_draw_trace.orientation_transition_active = true;
            custom_draw_trace.orientation_transition_from = transition_from.id();
            custom_draw_trace.orientation_transition_to = transition_to.id();
            custom_draw_trace.orientation_transition_effect =
                self.out.director.orientation.effect().id();
            let preparation_trace = std::mem::take(&mut self.diag.orientation_preparation_trace);
            custom_draw_trace.orientation_begin_us = preparation_trace.begin_us;
            custom_draw_trace.orientation_source_snapshot_us = preparation_trace.source_snapshot_us;
            custom_draw_trace.orientation_layout_us = preparation_trace.layout_us;
            custom_draw_trace.orientation_source_snapshot_bytes =
                preparation_trace.source_snapshot_bytes;
            custom_draw_trace.orientation_controlled_slint_raster_us =
                orientation_controlled_slint_raster_us;
            custom_draw_trace.orientation_damage_rotation_us = orientation_damage_rotation_us;
            custom_draw_trace.orientation_damage_rects_before = orientation_damage_rects_before;
            custom_draw_trace.orientation_damage_rects_after =
                orientation_damage_rects_after_rotation;
            if !self.out.director.orientation.destination_ready()
                && full_screen_controlled_capture_rendered
            {
                let capture_started = Instant::now();
                let destination_pmu =
                    mister_magik_perf_events::sampled_span(orientation_pmu_label(
                        self.out.director.orientation.effect(),
                        transition_from,
                        transition_to,
                        OrientationPmuPhase::Destination,
                    ));
                self.out.director.capture_orientation_destination(
                    layer_target.presentation_frame_view().pixels(),
                );
                drop(destination_pmu);
                custom_draw_trace.orientation_transition_destination_capture_us =
                    capture_started.elapsed().as_micros();
                custom_draw_trace.orientation_destination_snapshot_bytes = layer_target
                    .presentation_frame_view()
                    .pixels()
                    .len()
                    .saturating_mul(2)
                    as u64;
            }
            let gui_orientation_pmu = self
                .diag
                .gui_profiling
                .phase_span(gui_custom_selection.orientation_transition_raster);
            let orientation_rendered = (!orientation_capture_source_carrier_rendered).then(|| {
                self.out.director.orientation.render_into(
                    layer_target.presentation_pixels_mut(),
                    pre_input.animation_now,
                )
            });
            drop(gui_orientation_pmu);
            if let Some(Some((done, render_stats, transition_damage))) = orientation_rendered {
                custom_draw_trace.orientation_transition_stats = render_stats;
                custom_draw_trace.orientation_effect_read_bytes =
                    render_stats.blended_pixels.saturating_mul(2);
                custom_draw_trace.orientation_effect_write_bytes =
                    render_stats.blended_pixels.saturating_mul(2);
                let damage_build_started = Instant::now();
                cached_damage.clear();
                for row in 0..9 {
                    if let Some((x0, y0, x1, y1)) = transition_damage.rect_for_row(
                        row,
                        self.env.ui.render_w(),
                        self.env.ui.render_h(),
                    ) {
                        cached_damage.push(DirtyRect { x0, y0, x1, y1 });
                    }
                }
                custom_draw_trace.orientation_damage_build_us =
                    damage_build_started.elapsed().as_micros();
                custom_draw_trace.orientation_damage_rects_after = cached_damage.len() as u32;
                if done {
                    match self.out.director.end_orientation() {
                        Some(OrientationIntent::Confirm) => {
                            self.inp
                                .orientation_confirmation
                                .start_countdown(Instant::now());
                        }
                        Some(OrientationIntent::Rollback) | None => {}
                    }
                } else {
                    self.env.window.request_redraw();
                }
            }
            custom_draw_trace.orientation_transition_total_us =
                orientation_started.elapsed().as_micros();
        }
        cached_damage =
            shield_base_damage_under_publication(cached_damage, &mut preview_publication);
        cached_damage =
            shield_base_damage_under_publication(cached_damage, &mut arcade_publication);
        // CRT routes do not own an HDMI preview layer, so the normal preview
        // presentation acknowledgement can never fire for them.  Without a
        // route-specific acknowledgement the preview remains `animating`
        // forever, keeping the launcher awake and allowing the Slint base
        // raster to overwrite the settled CRT backdrop between list ticks.
        let crt_backdrop_target_presented = crt_backdrop_frame_is_presented(
            navigation_transition_composition_active,
            crt_backdrop_full_damage.is_some(),
            crt_backdrop_work_trace.active,
            project.preview_cache_state_before_composition == "exact",
            self.lib.preview.raw_frame_status() == PreviewRawFrameStatus::Ready,
            self.out
                .crt_backdrop
                .as_ref()
                .is_some_and(CrtBackdropController::is_transitioning),
        );
        let final_preview_target_presented = (raw_preview.is_some()
            || crt_backdrop_target_presented)
            && self.lib.preview.presentation_requires_present()
            && preview_transition_trace.progress >= 1.0;
        let cached_empty_target_presented = (self.out.layout.is_portrait()
            || !preview_direct_present_enabled())
            && final_preview_target_presented
            && raw_preview_cached_rect.is_some()
            && matches!(
                self.lib.preview.presentation_state(),
                PreviewPresentationState::Animating {
                    target: PreviewPresentationTarget::Empty,
                    ..
                }
            );
        let preview_presentation_commit = self.lib.preview.presentation_commit(
            final_preview_target_presented,
            empty_base_cached_rect.is_some() || cached_empty_target_presented,
        );
        drop(gui_custom_generation_pmu);
        if !self.out.director.chart.is_live() {
            record_launcher_frame_phase!(LauncherFramePhase::FullScreenTransition);
        }
        Ok(RenderFrame {
            frame_start_phase_us,
            redraw_pending_for_trace,
            wake_reasons_bits,
            latch_backend_active,
            cpu_t0,
            frame_t0,
            prepare_us,
            pre_render_pace,
            pre_render_wait_us,
            cpu_t1,
            frame_t1,
            #[cfg(feature = "tooling")]
            tooling_animation_active,
            frame_production_trace,
            frame_production_completed_at,
            screensaver_render_trace,
            accepted_screensaver_frame,
            screensaver_buffer_to_recycle_after_present,
            completed_hidden_frame_for_present,
            card_direct_frame_rendered,
            #[cfg(feature = "tooling")]
            card_direct_measurement,
            #[cfg(feature = "tooling")]
            card_work_timing,
            accepted_startup_intro_frame,
            orientation_capture_source_carrier_rendered,
            card_direct_waiting_on_slot,
            full_screen_transition_release_raster_rendered,
            full_screen_transition_live_endpoint_rendered,
            gui_raster_phase,
            this_rect,
            frame_plan_pmu,
            launcher_response_frame_stamp,
            cpu_t2,
            frame_t2,
            cpu_custom_draw_start,
            custom_draw_start,
            arcade_list_rect,
            preview_transition_trace,
            navigation_transition_composition_active,
            navigation_transition_frame_active,
            #[cfg(feature = "tooling")]
            navigation_transition_route,
            #[cfg(feature = "tooling")]
            navigation_transition_renderer,
            navigation_transition_frame_started,
            #[cfg(feature = "tooling")]
            navigation_endpoint_rendered,
            custom_draw_trace,
            cpu_custom_draw_done,
            custom_draw_done,
            raw_preview_direct_rect,
            preview_publication,
            preview_desired,
            arcade_publication,
            arcade_desired,
            cached_damage,
            preview_presentation_commit,
        })
    }

    /// Plans the frame, posts it, confirms the presentation and closes the frame.
    fn present(
        &mut self,
        #[cfg_attr(not(feature = "tooling"), allow(unused_variables, unused_mut))]
        mut begin: BeginFrame,
        mut pre_input: PreInputFrame,
        project: ProjectFrame,
        mut render: RenderFrame,
    ) -> Result<(), Exit> {
        let mut layer_target = LayerTarget::new_oriented_with_epoch(
            self.env.target,
            self.out.layout,
            self.out.layout_epoch,
        );
        if project.native_device_base {
            layer_target.attach_device_background(
                &mut self.out.native_device_background,
                self.ui.nav.device_kind(),
                self.env.window,
            );
        }
        let frame_plan = if self.out.layout.is_portrait() {
            LauncherFramePlan::from_publications(
                render.cached_damage,
                render.preview_publication,
                render.arcade_publication,
            )
        } else if !self.out.crt_layout {
            LauncherFramePlan::from_preview_publication_and_cached_arcade(
                render.cached_damage,
                render.preview_publication,
                render.arcade_desired,
                render.arcade_list_rect,
            )
        } else {
            LauncherFramePlan::from_cached_layers(
                render.cached_damage,
                render.preview_desired,
                render.raw_preview_direct_rect,
                render.arcade_desired,
                if self.out.crt_layout {
                    None
                } else {
                    render.arcade_list_rect
                },
            )
        };
        record_launcher_frame_phase!(LauncherFramePhase::FramePlanned);
        let startup_can_present = self.lib.lifecycle.startup_can_present_frame();
        let stream_motion_active = project.stream_motion_before_render
            || render.preview_transition_trace.active
            || render.navigation_transition_composition_active;
        let direct_hidden_present_mode = self.fx.startup_intro.is_some()
            || render.completed_hidden_frame_for_present.is_some()
            || render.card_direct_waiting_on_slot;
        drop(render.frame_plan_pmu);
        let hidden_present_pmu = self.diag.launcher_response_trace.input_pmu_span(
            self.inp.latency_critical_input_pending,
            "launcher-response.hidden-present",
        );
        let present_cycle = self.out.launcher_presenter.present(
            LauncherPresentFrame {
                plan: frame_plan,
                startup_can_present,
                first_visible_copy_done: self.diag.frame_accounting.first_visible_copy_done(),
                frame_start_phase_us: render.frame_start_phase_us,
                pre_render_pace: render.pre_render_pace,
                frame_analytics_mode: pre_input.frame_analytics_mode,
                stream_motion_active,
                direct_hidden_mode: direct_hidden_present_mode,
                completed_hidden_frame: render.completed_hidden_frame_for_present,
                readiness_source_request: self.out.launcher_readiness.source_evidence_request(),
                profile_latch_phases: self.diag.gui_profiling.active(),
            },
            LauncherPresentTargets {
                layer_target: &layer_target,
                fb0: self.env.disp,
                hardware: self.env.f,
                arcade_list_renderer: &mut self.out.arcade_list_renderer,
                pacer: &mut self.out.pacer,
                present_timing: self.out.present_timing,
            },
            self.env.display_session,
        );
        drop(hidden_present_pmu);
        let LauncherPresentCycle {
            presentation,
            frame_t3,
            frame_t4,
            cpu_t3,
            cpu_t4,
            pacing_trace,
            #[cfg(feature = "tooling")]
            post_timing,
        } = present_cycle;
        #[cfg(feature = "tooling")]
        super::launcher_frame_accounting::capture_evidence_cpu(
            &mut begin.tooling_frame_evidence,
            4,
            self.out.run_start,
        );
        record_launcher_frame_phase!(LauncherFramePhase::FrameSubmitted);
        if let Some(worker) = self.out.preview_compositor.as_ref() {
            worker.release_queued();
        }
        let readiness_source_evidence = presentation.readiness_source_evidence.clone();
        self.diag.gui_profiling.record_latch(
            self.out.frames,
            presentation.main_present_hidden_copied_bytes,
            presentation.main_present_hidden_invalid_bytes,
            presentation.main_present_hidden_catchup_bytes,
            presentation.main_present_hidden_rect_count,
            presentation.main_present_hidden_full_copy,
            presentation.main_present_buffer,
            presentation.main_present_copy_path,
            presentation.arcade_copy_trace,
        );
        pre_input.scheduler_phase = self
            .diag
            .launcher_response_trace
            .record_scheduler_interval("raster-and-post", pre_input.scheduler_phase);
        if let Some(completed_at) = render.frame_production_completed_at {
            render.frame_production_trace.ready_age_us = frame_t3
                .saturating_duration_since(completed_at)
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX);
        }
        if let Some(frame_started) = render.navigation_transition_frame_started {
            self.out.director.navigation.note_frame_work_us(
                frame_started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            );
        }
        if render.accepted_screensaver_frame
            && self.fx.screensaver_pipeline.is_some()
            && presentation.main_present_backend.is_latch()
            && presentation.main_present_status == LauncherPresentStatus::Ok
            && let Some(pipeline) = self.fx.screensaver_pipeline.as_mut()
            && let Err(error) = pipeline.confirm_presented(self.fx.screensaver_render_sequence)
        {
            crate::ui_errln!(
                "screensaver: shared screenshot confirmation failed: {error}; restoring launcher"
            );
            self.fx.screensaver.fail_current_activation(Instant::now());
            retire_screensaver_pipeline(
                &mut self.fx.screensaver_pipeline,
                &mut self.fx.retiring_screensaver_pipelines,
            );
            self.fx.screensaver_frame_visible = false;
            self.env.window.request_redraw();
        }
        if let Some(pixels) = render.screensaver_buffer_to_recycle_after_present.take()
            && let Some(pipeline) = self.fx.screensaver_pipeline.as_ref()
        {
            let _ = pipeline.recycle(pixels);
        }
        if presentation.main_present_backend.is_latch() {
            self.out.phase_alignment.observe(
                frame_t4
                    .saturating_duration_since(render.frame_t0)
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
        }
        if let Some(failure) = self.out.launcher_presenter.latch_failure() {
            self.diag.frame_accounting.record_latch_failure(failure);
        }
        set_launcher_present_mode_label(
            &self.env.app,
            present_mode_label_for_backend_status(
                presentation.main_present_backend,
                presentation.main_present_status,
            ),
        );
        let post_present_wait_us = if presentation.main_present_backend.is_latch() {
            presentation.vsync_us_override.unwrap_or(0)
        } else {
            0
        };
        let latch_trace_flush_deferred = presentation.main_present_backend.is_latch();
        if !latch_trace_flush_deferred {
            record_launcher_frame_phase!(LauncherFramePhase::CompatibilityResolved);
        }
        if !self.diag.first_vsync_logged
            && pacing_trace.vsync_source == Some(VsyncPaceSource::Vsync)
        {
            self.diag.first_vsync_logged = true;
            boot_analytics::event(
                "first_vsync",
                format!("frame={frames}", frames = self.out.frames),
            );
        }
        let visible_frame_presented = visible_frame_was_presented(
            presentation.copied_rows,
            presentation.main_present_status,
            presentation.main_present_copy_path,
        );
        if render.card_direct_frame_rendered && visible_frame_presented {
            self.fx.card_frame_rendered_last_iteration = true;
            if let Some(session) = self.fx.launcher_card_home.as_mut() {
                session.note_direct_presented();
            }
        }
        // Posting a buffer and observing it pending proves latch acceptance,
        // not physical presentation. The intro advances only after the final
        // active-sequence confirmation below.
        let startup_intro_frame_posted =
            visible_frame_presented && render.accepted_startup_intro_frame;
        if render.navigation_transition_frame_active && visible_frame_presented {
            self.diag
                .screensaver_cpu_profile
                .begin_navigation_transition(self.out.frames.saturating_add(1));
        }
        if self.fx.screensaver.active && visible_frame_presented {
            // Profile only completed screensaver output. Starting when Preview is pressed
            // includes loader/render-worker startup frames that have no presentation evidence.
            self.diag
                .screensaver_cpu_profile
                .begin_screensaver(self.out.frames.saturating_add(1));
            self.fx
                .screensaver
                .timeline
                .note_presented(render.accepted_screensaver_frame);
        }
        if visible_frame_presented && self.fx.startup_intro.is_none() {
            if !self.diag.first_launcher_frame_logged
                && self.lib.lifecycle.startup_status().state == StartupRevealState::RevealLauncher
            {
                self.diag.first_launcher_frame_logged = true;
                let nav_menu_items = self.ui.nav.current_menu_count();
                let bridge_menu_items = self
                    .env
                    .app
                    .global::<slint_ui::launcher::NavigationView>()
                    .get_menu_items()
                    .row_count();
                print_startup_event(
                    self.out.start,
                    "launcher_first_frame_presented",
                    format!(
                        "screen={} systems={} nav_menu_items={} bridge_menu_items={} catalog_ready={}",
                        screen_label(self.ui.nav.screen),
                        self.lib.catalog.systems.len(),
                        nav_menu_items,
                        bridge_menu_items,
                        u8::from(self.lib.catalog_ready)
                    ),
                );
                self.diag
                    .catalog_publication_test
                    .hold_first_launcher_frame(self.out.start);
            }
            self.lib.lifecycle.note_startup_frame_presented(
                self.out.frames,
                frame_t4,
                &mut self.lib.lifecycle_effects,
            );
            if self.diag.first_launcher_frame_logged
                && self.lib.lifecycle.startup_status().input_enabled
                && self.diag.profile_config.cpu().cold_boot_requested()
                && cold_boot_profile_completion_ready(
                    self.diag.profile_config.cpu().cold_boot_catalog_requested(),
                    self.lib.catalog_ready,
                    self.lib.catalog_session.refresh_done(),
                )
                && self.diag.cpu.is_some()
                && let Err(error) = cpu_profile::finish_cold_boot_async(
                    self.diag.cpu.take(),
                    self.diag.profile_config.cpu(),
                )
            {
                crate::ui_errln!("cold-boot cpu profile finalization failed: {error}");
            }
            if self.lib.lifecycle.startup_status().mode == StartupMode::ReturnFromGame
                && self.lib.lifecycle.startup_status().revealed
            {
                self.lib
                    .launch_return_session
                    .mark_correct_present(&self.ui.nav, &self.lib.catalog);
                if self
                    .lib
                    .launch_return_session
                    .first_correct_present_monotonic_us
                    != 0
                    && self.diag.profile_config.cpu().launch_return_requested()
                    && self.diag.cpu.is_some()
                    && let Err(error) = cpu_profile::finish_launch_return_async(
                        self.diag.cpu.take(),
                        self.diag.profile_config.cpu(),
                    )
                {
                    crate::ui_errln!("launch-return cpu profile finalization failed: {error}");
                }
                if self.lib.catalog_session.refresh_done() {
                    self.lib.launch_return_session.release_if_complete();
                }
            }
            apply_lifecycle_effects(
                &mut self.lib.lifecycle_effects,
                &mut self.lib.scheduler,
                self.out.start,
            );
        }
        let presented_copied_rows = presentation.copied_rows;
        self.diag
            .arcade_entry_latency
            .record_destination_prepared_frame(
                self.out.start,
                frame_t4,
                &self.lib.lifecycle,
                &self.lib.catalog,
                &self.ui.nav,
                &self.lib.preview,
                self.out.frames,
                render.prepare_us,
                presented_copied_rows,
                self.lib.catalog_version,
            );
        self.diag.arcade_entry_latency.record_presented_frame(
            self.out.start,
            frame_t4,
            &self.lib.lifecycle,
            &self.lib.catalog,
            &self.ui.nav,
            &self.lib.preview,
            self.out.frames,
            render.prepare_us,
            presented_copied_rows,
        );
        self.diag.gui_profiling.record_composition(
            self.out.frames,
            &project.composition_status,
            project.composition_decision.force_full_slint_present,
            project.composition_decision.force_full_slint_raster,
            project.full_frame_present,
            self.out.director.navigation.is_active(),
        );
        self.diag
            .gui_profiling
            .record_frame_work(GuiFrameWorkRecord::from_traces(
                self.out.frames,
                frame_t4
                    .saturating_duration_since(pre_input.loop_start)
                    .as_micros(),
                presentation.vsync_us_override.unwrap_or_else(|| {
                    frame_t3
                        .saturating_duration_since(render.custom_draw_done)
                        .as_micros()
                }),
                &render.custom_draw_trace,
                &presentation,
            ));
        #[cfg(feature = "tooling")]
        if let Some(frame) = begin.tooling_frame_evidence.as_mut() {
            frame.destination_reveal_us = u128_to_u64(
                render
                    .custom_draw_trace
                    .navigation_transition_destination_reveal_us,
            );
            frame.card_snapshot_locked |= render.custom_draw_trace.navigation_snapshot_locked;
            frame.producer_ready_depth = render.frame_production_trace.ready_depth;
            frame.producer_ready_age_us = render.frame_production_trace.ready_age_us;
            frame.producer_cancelled = render.frame_production_trace.cancelled;
        }
        #[cfg(feature = "tooling")]
        if let Some(frame) = begin.tooling_frame_evidence.as_mut() {
            frame.request_generation = render.frame_production_trace.sequence;
        }
        let mut presented_frame = LauncherFrameSnapshotBuilder {
            identity: LauncherFrameIdentity {
                frames: self.out.frames,
                automation: project.automation_frame_stamp,
                selection_feedback: self.ui.bridge_models.selection_feedback_stamp(),
                selected: self.ui.nav.arcade.selected,
                visual_index: self.ui.nav.arcade.visual_index,
                #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
                home_trace: LauncherHomeFrameTrace::from_nav(&self.ui.nav),
                search_index_state: match self.ui.nav.arcade_search.status {
                    launcher::ArcadeSearchStatus::Idle => "idle",
                    launcher::ArcadeSearchStatus::Searching => "searching",
                    launcher::ArcadeSearchStatus::Ready => "ready",
                    launcher::ArcadeSearchStatus::Failed => "failed",
                },
            },
            timing: LauncherFrameTiming {
                startup_start: self.out.start,
                startup_monotonic_us: self.diag.startup_monotonic_us,
                run_start: self.out.run_start,
                loop_start: pre_input.loop_start,
                frame_t0: render.frame_t0,
                frame_t1: render.frame_t1,
                frame_t2: render.frame_t2,
                frame_t3,
                frame_t4,
                pre_render_wait_us: render.pre_render_wait_us,
                post_present_wait_us,
                custom_draw_start: render.custom_draw_start,
                custom_draw_done: render.custom_draw_done,
                prepare_us: render.prepare_us,
                home_pan_present_active: project.home_pan_present_active,
                home_horizontal_input_held: project.home_horizontal_input_held,
                redraw_pending: render.redraw_pending_for_trace,
                wake_reasons_bits: render.wake_reasons_bits,
            },
            render: LauncherFrameRenderData {
                custom_draw_trace: render.custom_draw_trace,
                prepare_trace: pre_input.prepare_trace,
                dirty_rect: render.this_rect,
                preview_cache_state: self.lib.preview.trace_cache_state(),
                preview_transition: render.preview_transition_trace,
                composition_status: project.composition_status.clone(),
                screensaver_active: self.fx.screensaver.active
                    && self.fx.screensaver_pipeline.is_some(),
                screensaver_active_cards: self.fx.screensaver_active_cards,
                frame_production_trace: render.frame_production_trace,
                screensaver_render_trace: render.screensaver_render_trace,
            },
            pacing: pacing_trace,
            presentation,
            status: LauncherFrameStatusData {
                status_write_due: project.status_write_due,
                status_string_copy_bytes: project.status_string_copy_bytes,
                clock_update_due: pre_input.clock_update_due,
                clock_update_us: pre_input.clock_update_us,
            },
            cpu: LauncherFrameCpuTrace {
                loop_start: pre_input.cpu_loop_start,
                t0: render.cpu_t0,
                t1: render.cpu_t1,
                t2: render.cpu_t2,
                custom_draw_start: render.cpu_custom_draw_start,
                custom_draw_done: render.cpu_custom_draw_done,
                t3: cpu_t3,
                t4: cpu_t4,
            },
        }
        .build();
        let launcher_response_present_receipt = LauncherResponsePresentReceipt {
            post_accepted_at_us: crate::input_hub::monotonic_us(),
            post_accepted_execution: self.diag.launcher_response_trace.execution_stamp(),
            dirty_rect: presented_frame
                .dirty_rect
                .map(|rect| (rect.x0, rect.y0, rect.x1, rect.y1)),
            present_bytes: presented_frame.present_bytes,
            wasted_present_bytes: presented_frame.wasted_present_bytes,
            cached_present_us: launcher_response_u64(presented_frame.cached_present_us),
            hidden_compose_us: launcher_response_u64(presented_frame.hidden_compose_us),
            hidden_copy_us: launcher_response_u64(presented_frame.main_present_hidden_copy_us),
            hidden_publish_us: launcher_response_u64(
                presented_frame.main_present_hidden_publish_us,
            ),
            hidden_invalid_bytes: presented_frame.main_present_hidden_invalid_bytes,
            hidden_rect_count: presented_frame.main_present_hidden_rect_count,
            hidden_catchup_bytes: presented_frame.main_present_hidden_catchup_bytes,
            hidden_full_copy: presented_frame.main_present_hidden_full_copy,
            hidden_copy_path: presented_frame.main_present_copy_path,
            present_request_us: launcher_response_u64(presented_frame.main_present_request_us),
            set_vga_fb_us: launcher_response_u64(presented_frame.main_present_set_vga_fb_us),
            present_wait_us: presented_frame.main_present_wait_us,
            posted_sequence: presented_frame.main_present_sequence,
            post_active_sequence: presented_frame.main_present_post_active_sequence,
            post_pending_sequence: presented_frame.main_present_post_pending_sequence,
            post_pending: presented_frame.main_present_post_pending,
            refresh_period_us: self.out.pacer.period_us(),
        };
        let selection_feedback_stamp = presented_frame.selection_feedback.clone();
        let mut accepted_and_active_confirmed = false;
        let mut confirmed_present_sequence = 0u16;
        let mut confirmed_presentation = PresentationOutcome::Unacknowledged;
        let mut selection_feedback_confirmed_at = (!latch_trace_flush_deferred
            && visible_frame_presented)
            .then_some(pre_input.animation_now);
        let status = FrameStatusView {
            nav: &self.ui.nav,
            pad: &self.env.pad,
            catalog: &self.lib.catalog,
            catalog_ready: self.lib.catalog_ready,
            catalog_refresh_done: self.lib.catalog_session.refresh_done(),
            launching: pre_input.launching,
            loading_title: self
                .lib
                .scheduler
                .visible_loading_title(&self.lib.loading_title),
            catalog_scan_visible: project.catalog_scan_visible,
            catalog_scan_percent: project.catalog_scan_percent,
            catalog_background_scan_visible: project.catalog_background_scan_visible,
            confirm_visible: project.confirm_visible,
            confirm_selected: project.confirm_selected,
            status_text: project.status_text.as_ref(),
            start_screen: self.lib.start_screen,
            lock_screen: self.lib.lock_screen,
            route_reassert_count: self.env.display_session.reassert_count(),
            last_route_reassert_frame: self.env.display_session.last_reassert_frame(),
            last_route_reassert_ok: self.env.display_session.last_reassert_ok(),
            last_route_reassert_error: self.env.display_session.last_reassert_error(),
            startup_status: self.lib.lifecycle.startup_status(),
            return_session: &self.lib.launch_return_session,
        };
        if latch_trace_flush_deferred {
            let ControlFlow::Continue(LatchWaitOutcome {
                finish_timing,
                #[cfg(feature = "tooling")]
                wait_start,
                pace,
                wait_done,
                readiness_post,
                ..
            }) = post_accounting_and_latch_wait(LatchWait {
                status,
                card_direct_frame_rendered: render.card_direct_frame_rendered,
                #[cfg(feature = "tooling")]
                card_work_timing: render.card_work_timing,
                composition_decision: &project.composition_decision,
                confirmed_presentation: &mut confirmed_presentation,
                #[cfg(feature = "tooling")]
                custom_draw_done: render.custom_draw_done,
                #[cfg(feature = "tooling")]
                custom_draw_start: render.custom_draw_start,
                director: &mut self.out.director,
                f: &mut *self.env.f,
                frame_accounting: &mut self.diag.frame_accounting,
                frame_analytics_mode: pre_input.frame_analytics_mode,
                frame_clock: self.out.frame_clock,
                #[cfg(feature = "tooling")]
                frame_t1: render.frame_t1,
                #[cfg(feature = "tooling")]
                frame_t2: render.frame_t2,
                #[cfg(feature = "tooling")]
                frame_t3,
                #[cfg(feature = "tooling")]
                frame_t4,
                gui_profiling: &self.diag.gui_profiling,
                input_observation: self.inp.input_observation,
                launcher_card_home: &mut self.fx.launcher_card_home,
                launcher_presenter: &mut self.out.launcher_presenter,
                launcher_response_frame_stamp: &render.launcher_response_frame_stamp,
                launcher_response_trace: &mut self.diag.launcher_response_trace,
                nav: &self.ui.nav,
                pacer: &mut self.out.pacer,
                pad: &self.env.pad,
                presented_frame: &mut presented_frame,
                preview_presentation_commit: &render.preview_presentation_commit,
                #[cfg(feature = "tooling")]
                run_start: self.out.run_start,
                scheduler_phase: &mut pre_input.scheduler_phase,
                screensaver: &self.fx.screensaver,
                selection_feedback_stamp: &selection_feedback_stamp,
                startup_intro_frame_posted,
                #[cfg(feature = "tooling")]
                tooling: &mut self.diag.tooling,
                #[cfg(feature = "tooling")]
                tooling_frame_evidence: &mut begin.tooling_frame_evidence,
                #[cfg(feature = "tooling")]
                tooling_reject_baseline: &mut self.diag.tooling_reject_baseline,
                visible_frame_presented,
                window: self.env.window,
            })
            else {
                return Err(Exit::Skip);
            };
            account_confirmed_present(ConfirmedPresent {
                accepted_and_active_confirmed: &mut accepted_and_active_confirmed,
                animation_now: pre_input.animation_now,
                #[cfg(feature = "tooling")]
                app: &self.env.app,
                arcade_entry_latency: &mut self.diag.arcade_entry_latency,
                bridge_churn_playback: &mut self.diag.bridge_churn_playback,
                #[cfg(feature = "tooling")]
                card_direct_frame_rendered: render.card_direct_frame_rendered,
                #[cfg(feature = "tooling")]
                card_direct_measurement: &mut render.card_direct_measurement,
                #[cfg(feature = "tooling")]
                card_presentation_measurement_enabled: self
                    .diag
                    .card_presentation_measurement_enabled,
                #[cfg(feature = "tooling")]
                card_work_timing: render.card_work_timing,
                catalog: &self.lib.catalog,
                catalog_ready: self.lib.catalog_ready,
                catalog_version: self.lib.catalog_version,
                composition_status: project.composition_status,
                confirm_visible: project.confirm_visible,
                confirmed_present_sequence: &mut confirmed_present_sequence,
                crt_backdrop: &self.out.crt_backdrop,
                #[cfg(feature = "tooling")]
                custom_draw_done: render.custom_draw_done,
                #[cfg(feature = "tooling")]
                custom_draw_start: render.custom_draw_start,
                director: &mut self.out.director,
                f: &mut *self.env.f,
                frame_accounting: &mut self.diag.frame_accounting,
                frame_clock: self.out.frame_clock,
                #[cfg(feature = "tooling")]
                frame_start_phase_us: render.frame_start_phase_us,
                #[cfg(feature = "tooling")]
                frame_t1: render.frame_t1,
                #[cfg(feature = "tooling")]
                frame_t2: render.frame_t2,
                #[cfg(feature = "tooling")]
                frame_t3,
                frame_t4,
                frames: self.out.frames,
                full_screen_transition_live_endpoint_rendered: render
                    .full_screen_transition_live_endpoint_rendered,
                full_screen_transition_release_raster_rendered: render
                    .full_screen_transition_release_raster_rendered,
                gui_profiling: &mut self.diag.gui_profiling,
                gui_raster_phase: render.gui_raster_phase,
                #[cfg(feature = "tooling")]
                home_horizontal_input_held: project.home_horizontal_input_held,
                launcher_automation: &mut self.diag.launcher_automation,
                #[cfg(feature = "tooling")]
                launcher_card_home: &self.fx.launcher_card_home,
                launcher_presenter: &mut self.out.launcher_presenter,
                launcher_readiness: &mut self.out.launcher_readiness,
                launcher_response_frame_stamp: &render.launcher_response_frame_stamp,
                launcher_response_present_receipt,
                launcher_response_trace: &mut self.diag.launcher_response_trace,
                layer_target: &mut layer_target,
                lifecycle: &self.lib.lifecycle,
                nav: &self.ui.nav,
                #[cfg(feature = "tooling")]
                navigation_endpoint_rendered: render.navigation_endpoint_rendered,
                #[cfg(feature = "tooling")]
                navigation_transition_composition_active: render
                    .navigation_transition_composition_active,
                #[cfg(feature = "tooling")]
                navigation_transition_renderer: render.navigation_transition_renderer,
                #[cfg(feature = "tooling")]
                navigation_transition_route: render.navigation_transition_route,
                orientation_capture_source_carrier_rendered: render
                    .orientation_capture_source_carrier_rendered,
                pace: &pace,
                #[cfg(feature = "tooling")]
                pacer: &self.out.pacer,
                #[cfg(feature = "tooling")]
                post_timing,
                #[cfg(feature = "tooling")]
                pre_render_wait_us: render.pre_render_wait_us,
                prepare_us: render.prepare_us,
                presented_copied_rows,
                presented_frame: &presented_frame,
                preview: &self.lib.preview,
                preview_compositor: &self.out.preview_compositor,
                preview_route: self.ui.preview_route,
                profile_config: &self.diag.profile_config,
                readiness_post,
                readiness_source_evidence,
                redraw_pending_for_trace: render.redraw_pending_for_trace,
                #[cfg(feature = "tooling")]
                run_start: self.out.run_start,
                #[cfg(feature = "tooling")]
                screensaver: &self.fx.screensaver,
                screensaver_cpu_profile: &mut self.diag.screensaver_cpu_profile,
                selection_feedback_confirmed_at: &mut selection_feedback_confirmed_at,
                start: self.out.start,
                startup_intro: &mut self.fx.startup_intro,
                startup_intro_frame_posted,
                system_entry_cpu_profile: &mut self.diag.system_entry_cpu_profile,
                #[cfg(feature = "tooling")]
                tooling: &mut self.diag.tooling,
                #[cfg(feature = "tooling")]
                tooling_animation_active: render.tooling_animation_active,
                #[cfg(feature = "tooling")]
                tooling_attempt_id: self.diag.tooling_attempt_id,
                #[cfg(feature = "tooling")]
                tooling_drop_baseline: &mut self.diag.tooling_drop_baseline,
                #[cfg(feature = "tooling")]
                tooling_frame_begin: begin.tooling_frame_begin,
                #[cfg(feature = "tooling")]
                tooling_frame_evidence: &mut begin.tooling_frame_evidence,
                #[cfg(feature = "tooling")]
                tooling_tick_us: begin.tooling_tick_us,
                wait_done,
                #[cfg(feature = "tooling")]
                wait_start,
                window: self.env.window,
            });
            self.diag.frame_accounting.record_finished_frame(
                &presented_frame,
                self.out.start,
                self.env.disp,
                self.lib.catalog_ready,
                finish_timing.runtime_status_write_us,
            );
            self.diag.gui_profiling.finalize_frame_timing(
                self.out.frames,
                GuiFrameTimingTrace::from_presented_frame(
                    &presented_frame,
                    finish_timing.frame_finish_us,
                ),
            );
            self.diag.frame_accounting.write_finished_frame_trace(
                &presented_frame,
                finish_timing,
                latch_trace_flush_deferred,
            );
        } else {
            self.diag.gui_profiling.finalize_frame_timing(
                self.out.frames,
                GuiFrameTimingTrace::from_presented_frame(&presented_frame, 0),
            );
            self.diag.frame_accounting.finish_frame(
                presented_frame,
                self.out.start,
                self.env.disp,
                status,
                latch_trace_flush_deferred,
            );
        }
        finish_presented_frame(FrameCloseout {
            accepted_and_active_confirmed,
            bridge_models: &mut self.ui.bridge_models,
            composition_decision: &project.composition_decision,
            confirmed_present_sequence,
            confirmed_presentation,
            director: &mut self.out.director,
            frame_accounting: &mut self.diag.frame_accounting,
            frame_clock: &mut self.out.frame_clock,
            frames: &mut self.out.frames,
            input_latency_lab: &mut self.diag.input_latency_lab,
            input_observation: self.inp.input_observation,
            latch_backend_active: render.latch_backend_active,
            latch_trace_flush_deferred,
            latency_critical_input_pending: &mut self.inp.latency_critical_input_pending,
            launcher_response_frame_stamp: &render.launcher_response_frame_stamp,
            launcher_response_trace: &mut self.diag.launcher_response_trace,
            preview: &mut self.lib.preview,
            preview_presentation_commit: render.preview_presentation_commit,
            #[cfg(feature = "tooling")]
            run_start: self.out.run_start,
            scheduler_phase: &mut pre_input.scheduler_phase,
            screensaver_cpu_profile: &mut self.diag.screensaver_cpu_profile,
            selection_feedback_confirmed_at,
            selection_feedback_stamp: &selection_feedback_stamp,
            #[cfg(feature = "tooling")]
            tooling: &mut self.diag.tooling,
            #[cfg(feature = "tooling")]
            tooling_frame_evidence: &mut begin.tooling_frame_evidence,
            visible_frame_presented,
            window: self.env.window,
        });
        Ok(())
    }
}

/// Runs the launcher frame loop to completion.
pub(in crate::ui_runner) fn run_frame_loop(
    env: Env<'_>,
    process_entry_cpu_profile: Option<cpu_profile::CpuProfiler>,
) {
    FrameLoop::new(env, process_entry_cpu_profile).run();
}
