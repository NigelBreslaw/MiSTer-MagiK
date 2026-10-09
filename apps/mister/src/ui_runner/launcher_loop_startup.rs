// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Everything `run_launcher_loop` builds before its first frame.
//!
//! The statements are the loop's own startup, moved here in order and unchanged.
//! The locals the frame loop uses come back as one struct that the loop
//! destructures into locals of the same names, so the loop body is untouched.

use super::super::launcher_card_home::LauncherCardHomeSession;
use super::super::launcher_readiness::LauncherReadiness;
use super::*;
use crate::cpu_profile::{CpuProfiler, ScreensaverProfiler};
use crate::input_hub::InputObservationProbe;
use crate::launcher_home::CardLevelSnapshot;
use crate::process_config::ProfileProcessConfig;
use mister_magik_core::frame_clock::FrameClock;

pub(super) struct LoopState {
    pub(super) launcher_ui_actions: LauncherUiActionsAdapter,
    pub(super) start: Instant,
    pub(super) frame_clock: FrameClock,
    pub(super) idle_slept_since: Option<Instant>,
    pub(super) ui_action_sequence: u64,
    pub(super) startup_monotonic_us: u64,
    pub(super) frames: u64,
    pub(super) profile_config: ProfileProcessConfig,
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
    pub(super) launcher_presenter: LauncherPresenter,
    pub(super) launcher_readiness: LauncherReadiness,
    pub(super) scheduler: LauncherScheduler,
    pub(super) catalog_events: CatalogJobEventBuf,
    pub(super) deferred_catalog_events: VecDeque<CatalogWorkerMessage>,
    pub(super) pending_catalog_ready: Option<CatalogWorkerMessage>,
    pub(super) pending_collection_entry: Option<PendingCollectionEntry>,
    pub(super) deferred_settings_activation: DeferredSettingsActivation,
    pub(super) deferred_navigation_hydration_finish: Option<String>,
    pub(super) catalog_ready_deferred_since: Option<Instant>,
    pub(super) catalog_ready_stationary_edge_since: Option<Instant>,
    pub(super) media_events: MediaJobEventBuf,
    pub(super) lifecycle_effects: LifecycleEffects,
    pub(super) preview_systems_entered: BTreeSet<String>,
    pub(super) preview_initial_lists_ready: BTreeSet<String>,
    pub(super) start_screen: Screen,
    pub(super) lock_screen: Option<Screen>,
    pub(super) launch_return_session: LaunchReturnSession,
    pub(super) arcade_catalog_required_at_start: bool,
    pub(super) pending_start_system: Option<String>,
    pub(super) crt_layout: bool,
    pub(super) crt_metrics: CrtUiMetrics,
    pub(super) preview_route: PreviewRoutePolicy,
    pub(super) nav: LauncherNav,
    pub(super) settings_store: FileSettingsStore,
    pub(super) layout: UiLayoutGeometry,
    pub(super) layout_epoch: u64,
    pub(super) preview_compositor: Option<PreviewCompositor>,
    pub(super) preview_compositor_start_attempted: bool,
    pub(super) director: PresentationDirector,
    pub(super) settings_cog_render_ahead: SettingsCogSession,
    pub(super) display_confirmation: DisplayConfirmation,
    pub(super) orientation_confirmation: OrientationConfirmation,
    pub(super) orientation_full_redraw_pending: bool,
    pub(super) orientation_preparation_trace: OrientationPreparationTrace,
    pub(super) setup: SetupNav,
    pub(super) input_router: InputRouter,
    pub(super) setup_disconnect_notice: bool,
    pub(super) input_integrity_stall: Option<u64>,
    pub(super) input_integrity_trace: InputIntegrityTrace,
    pub(super) input_observation_probe: Option<InputObservationProbe>,
    pub(super) launcher_response_trace: LauncherResponseTrace,
    pub(super) gui_profiling: GuiProfilingController,
    pub(super) bridge_churn_playback: BridgeChurnPlayback,
    pub(super) input_latency_lab: InputLatencyLab,
    pub(super) loading_title: String,
    pub(super) library_reset: LibraryResetState,
    pub(super) library_reset_bridge_dirty: bool,
    pub(super) last_clock_update: Instant,
    pub(super) last_clock_text: String,
    pub(super) pacer: VsyncPacer,
    pub(super) pacing_policy: LauncherFramePacingPolicy,
    pub(super) phase_alignment: LauncherPhaseAlignment,
    pub(super) present_timing: PresentTiming,
    pub(super) preview: PreviewState,
    pub(super) preview_transition: PreviewTransitionDemo,
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
    pub(super) cpu: Option<CpuProfiler>,
    pub(super) system_entry_cpu_profile: Option<cpu_profile::CpuProfiler>,
    pub(super) screensaver_cpu_profile: ScreensaverProfiler,
    pub(super) bridge_models: LauncherViewPresenters,
    pub(super) native_device_background: NativeDeviceBackground,
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
    pub(super) card_level: CardLevelSnapshot,
    pub(super) card_prefetch_key: (String, usize),
    pub(super) card_frame_rendered_last_iteration: bool,
    pub(super) launcher_card_home: Option<LauncherCardHomeSession>,
    pub(super) arcade_screen_pending: bool,
    pub(super) update_check: UpdateCheck,
    pub(super) startup_intro: Option<StartupIntroSession>,
    pub(super) startup_intro_launcher_frame_ready: bool,
    pub(super) startup_intro_bridge_dirty_pending: bool,
    pub(super) startup_intro_catalog_ui_replay: Option<LauncherWorkerUiIntent>,
    pub(super) startup_intro_catalog_shells_pending: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_loop_state(
    secs: u64,
    ui: &UiDisplay,
    window: &Rc<MisterSoftwareWindow>,
    pad: &mut PadPool,
    app: &slint_ui::launcher::Launcher,
    animation_clock: &AnimationClock,
    process_entry_cpu_profile: Option<cpu_profile::CpuProfiler>,
    launcher_config: &mister_magik_fb::process_config::LauncherProcessConfig,
) -> LoopState {
    let launcher_ui_actions = LauncherUiActionsAdapter::install(app);
    #[cfg(feature = "tooling")]
    app.set_development_keyboard_input(true);
    let start = Instant::now();
    // The only animation time in the launcher: one display period per produced
    // frame, on the same step Slint uses. Nothing animated may read a real clock.
    let frame_clock =
        mister_magik_core::frame_clock::FrameClock::new(start, animation_clock.fixed_step());
    crate::launcher::set_frame_period(frame_clock.period());
    // When the previous iteration slept with nothing to animate, when it began.
    let idle_slept_since: Option<Instant> = None;
    let ui_action_sequence = 0u64;
    let startup_monotonic_us = monotonic_clock_us().unwrap_or(0);
    let frames = 0u64;
    let profile_config = launcher_config.profiles().clone();
    let benchmark_config = launcher_config.benchmark().clone();
    let screensaver_start_mode = launcher_config.screensaver().start_mode();
    let screensaver_preview_waits_for_analytics =
        launcher_config.screensaver().preview_waits_for_analytics();
    let screensaver = ScreensaverControl::new(Instant::now(), screensaver_start_mode);
    let screensaver_pipeline: Option<ScreensaverRenderAhead> = None;
    let retiring_screensaver_pipelines: Vec<ScreensaverRenderAhead> = Vec::new();
    let screensaver_loader: Option<LauncherScreensaverLoader> = None;
    let screensaver_launcher_frame: Option<Vec<Rgb565Pixel>> = None;
    let screensaver_frame_visible = false;
    let screensaver_active_cards = 0usize;
    let screensaver_render_sequence = 0u64;
    let screensaver_starvation_count = 0u64;
    let present_backend =
        LauncherPresentBackend::from_config(launcher_config.presentation_backend());
    present_backend.log_if_experimental();
    let launcher_presenter = LauncherPresenter::new(ui, present_backend);
    let launcher_readiness = super::launcher_readiness::LauncherReadiness::from_process_config(
        launcher_config.readiness().clone(),
    );
    let mut scheduler = LauncherScheduler::with_runtime_config(
        false,
        launcher_config.catalog_paths().clone(),
        launcher_config.archive_cache().clone(),
        launcher_config.media_worker().clone(),
        benchmark_config
            .launch_return_pmu_handoff_out()
            .map(str::to_owned),
    );
    let catalog_events = CatalogJobEventBuf::new();
    let deferred_catalog_events: VecDeque<CatalogWorkerMessage> = VecDeque::new();
    let pending_catalog_ready: Option<CatalogWorkerMessage> = None;
    let pending_collection_entry: Option<PendingCollectionEntry> = None;
    let deferred_settings_activation = DeferredSettingsActivation::default();
    let deferred_navigation_hydration_finish: Option<String> = None;
    let catalog_ready_deferred_since: Option<Instant> = None;
    let catalog_ready_stationary_edge_since: Option<Instant> = None;
    let media_events = MediaJobEventBuf::new();
    let mut lifecycle_effects = LifecycleEffects::new();
    let preview_systems_entered = BTreeSet::new();
    let preview_initial_lists_ready = BTreeSet::new();
    let env_start_screen = benchmark_config.start_screen();
    let env_start_system = benchmark_config.start_system().map(str::to_owned);
    let start_screen = env_start_screen
        .or_else(|| env_start_system.as_ref().map(|_| Screen::Arcade))
        .unwrap_or(Screen::Home);
    let lock_screen = benchmark_config.lock_screen().or_else(|| {
        env_start_system.as_ref().map(|_| {
            env_start_screen
                .filter(|screen| *screen == Screen::Arcade)
                .unwrap_or(Screen::Arcade)
        })
    });
    let launch_return_restore_allowed = launcher_return_to_launcher_requested()
        && env_start_screen.is_none()
        && lock_screen.is_none();
    let mut launch_return_session = LaunchReturnSession::new(
        launcher::take_launch_return_state().filter(|_| launch_return_restore_allowed),
    );
    if !launch_return_restore_allowed || !launch_return_session.requested() {
        return_catalog_capsule::remove_return_catalog_capsule();
    }
    let startup_return_requested = launch_return_session.requested();
    let mut launch_return_restored = false;
    let arcade_catalog_required_at_start =
        matches!(start_screen, Screen::Arcade) || matches!(lock_screen, Some(Screen::Arcade));
    let pending_start_system = env_start_system.clone();
    let crt_layout = ui.output_route().is_crt();
    let crt_metrics = crate::ui_display::CrtUiMetrics::for_display(ui);
    let preview_route = PreviewRoutePolicy::for_output_route(ui.output_route());
    let mut nav =
        LauncherNav::for_crt_layout_with_row_height(crt_layout, crt_metrics.game_row_height);
    let settings_store =
        FileSettingsStore::new(launcher_config.device_paths().app_path("settings.json"));
    let orientation_store = ConfirmedOrientationStore::for_runtime(settings_store.clone());
    nav.settings = settings_store.load();
    if let Err(error) = orientation_store.reconcile_osd_rotation(nav.settings.screen_orientation) {
        crate::ui_errln!("settings: failed to reconcile MiSTer OSD rotation: {error}");
    }
    let layout = UiLayoutGeometry::for_display(ui, nav.settings.screen_orientation);
    let layout_epoch = 1_u64;
    let preview_compositor = None;
    let preview_compositor_start_attempted = false;
    nav.set_portrait_layout(layout.is_portrait());
    sync_license_viewport(&mut nav, layout);
    if crt_layout {
        nav.set_arcade_row_height(crt_arcade_row_height(
            crt_metrics.game_row_height,
            layout.is_portrait(),
        ));
    }
    nav.sync_orientation_selection();
    let navigation_motion_enabled =
        !nav.settings.reduce_motion || profile_config.cpu().navigation_transition_requested();
    let director = PresentationDirector::new(
        NavigationTransitionRuntime::new(
            layout.logical_w(),
            layout.logical_h(),
            navigation_motion_enabled,
        ),
        OrientationTransitionRuntime::new(ui.render_w(), ui.render_h()),
    );
    let settings_cog_render_ahead = SettingsCogSession::new();
    nav.screen = start_screen;
    let mut display_confirmation = DisplayConfirmation::new();
    let orientation_confirmation = OrientationConfirmation::new(orientation_store);
    let orientation_full_redraw_pending = layout.is_portrait();
    let orientation_preparation_trace = OrientationPreparationTrace::default();
    // Main owns the active display mode; the launcher only mirrors its reported state.
    if std::env::var_os("MISTER_MAGIK_PARENT").is_some()
        && let Ok(state) = launcher::try_display_state()
    {
        let selected_id = state.pending.as_deref().unwrap_or(&state.active);
        if let Some(index) = mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
            .iter()
            .position(|mode| mode.id == selected_id)
        {
            nav.display_selected = index;
            nav.display_highlighted =
                launcher::settings_display_selection_index(index).unwrap_or(0);
        }
        if state.return_to_settings {
            nav.screen = Screen::Settings;
            nav.settings_selected = 0;
            if let Some(error) = state.error.as_deref() {
                nav.display_error = Some(format!(
                    "The previous resolution was restored after a display failure: {error}"
                ));
                nav.confirm_action = Some(launcher::ConfirmAction::DisplayResolutionError);
                nav.confirm_selected = 0;
            }
        }
        display_confirmation.adopt_startup_pending(
            &mut nav,
            &state,
            display_confirmation_ui_enabled(
                std::env::var_os("MISTER_MAGIK_DISPLAY_CONFIRM_UI").as_deref(),
            ),
            Instant::now(),
        );
    }
    let mut setup = SetupNav::new();
    let input_router = InputRouter::new(launcher_input_focus(
        false, false, false, false, false, false, &nav,
    ));
    let setup_disconnect_notice = false;
    let input_integrity_stall = launcher_config.input().integrity_stall_ms();
    let input_integrity_trace =
        InputIntegrityTrace::new(launcher_config.input().integrity_trace(), Instant::now());
    let input_observation_probe = pad.input_observation_probe();
    let launcher_response_trace = LauncherResponseTrace::from_config(
        launcher_config.readiness().response_trace(),
        launcher_config.readiness().entry_trace(),
        &nav,
        input_observation_probe.clone(),
    );
    let gui_profiling = GuiProfilingController::from_config(profile_config.gui().clone());
    reset_media_progress_bridge();
    let bridge_churn_playback = BridgeChurnPlayback::new(gui_profiling.bridge_churn_route());
    let input_latency_lab = InputLatencyLab::from_config(
        launcher_config.input().latency_lab(),
        input_observation_probe.clone(),
    );
    let loading_title = String::new();
    let library_reset = LibraryResetState::Idle;
    let library_reset_bridge_dirty = false;
    let last_clock_update = Instant::now() - Duration::from_secs(2);
    let last_clock_text = launcher_clock_text();
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    crate::ui_logln!(
        "launcher running {label} — {} pad(s), D-pad to move, A to select, Home to go back...",
        pad.len()
    );
    crate::ui_logln!(
        "launcher_mode={} fb_format={}",
        "launcher",
        production_label()
    );
    crate::ui_logln!(
        "launcher_start_screen={} launcher_lock_screen={}",
        screen_label(start_screen),
        lock_screen.map(screen_label).unwrap_or("none")
    );
    if let Some(system_id) = env_start_system.as_ref() {
        crate::ui_logln!("launcher_start_system={system_id}");
    }
    boot_analytics::event(
        "launcher_loop_start",
        format!("label={label} pads={}", pad.len()),
    );
    if AUTO_CONTROLLER_SETUP_ENABLED
        && let Some(device) = pad.device_needing_setup()
        && let Some(info) = pad.info_for_device(&device)
    {
        let status = pad.db().registry_status(info);
        crate::ui_errln!(
            "controller setup: {} generation {} needs setup ({status:?}) - showing prompt",
            device.plug_id,
            device.generation
        );
        setup.open_for(status, device);
    }
    let pacer = ui
        .output_route()
        .nominal_period_us()
        .map(|period| {
            VsyncPacer::from_config_with_default_period(
                launcher_config.display_pacing().vsync(),
                period,
            )
        })
        .unwrap_or_else(|| VsyncPacer::from_config(launcher_config.display_pacing().vsync()));
    let pacing_policy = LauncherFramePacingPolicy;
    let phase_alignment = LauncherPhaseAlignment::default();
    let present_timing = launcher_config.display_pacing().present_timing();
    let mut preview = PreviewState::new_with_config(start, launcher_config.preview().clone());
    let preview_transition =
        PreviewTransitionDemo::from_config(launcher_config.preview_transition().clone());
    let mut arcade_list_renderer = if crt_layout {
        ArcadeListRenderer::new_for_crt_display(crt_metrics, ui)
    } else {
        ArcadeListRenderer::new()
    };
    arcade_list_renderer.set_crt_portrait_rows(layout.is_portrait());
    let crt_backdrop = CrtBackdropController::for_display(ui);
    let crt_arcade_overlay = CrtArcadeOverlayState::new();
    let launcher_preview_version = 1u64;
    let launcher_arcade_version = 1u64;
    let launcher_arcade_scroll_offset = LayerOffset::ZERO;
    let launcher_arcade_content_generation = 1u64;
    let launcher_preview_publication: Option<PhysicalLayerPublication> = None;
    let launcher_arcade_publication: Option<PhysicalLayerPublication> = None;
    let arcade_drawer_view_cache = ArcadeDrawerViewCache::default();
    let cpu = process_entry_cpu_profile.or_else(|| cpu_profile::start(profile_config.cpu()));
    let system_entry_cpu_profile = None;
    let screensaver_cpu_profile =
        cpu_profile::ScreensaverProfiler::from_config(profile_config.cpu());
    let mut bridge_models = LauncherViewModels::default();
    let native_device_background = super::launcher_compositor::NativeDeviceBackground::default();
    let mut catalog_version = 0usize;
    let (user_state_path, user_state_media_root) = (
        launcher_config
            .catalog_paths()
            .user_state_sqlite()
            .to_path_buf(),
        PathBuf::from("/media/fat"),
    );
    let user_state_session = UserStateSession::start(user_state_path, user_state_media_root);
    let user_state_catalog_version = None;
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    crate::ui_logln!(
        "preview_visual_pct={} preview_blitter=raw",
        launcher_config.preview().visual_pct()
    );
    crate::ui_logln!(
        "preview_transition={} segment_secs={} duration_ms={}",
        preview_transition.labels(),
        preview_transition.segment.as_secs(),
        preview_transition.duration.as_millis()
    );
    crate::ui_logln!(
        "fb_present_delay_us={} vsync_fresh_hit_max_age_us={}",
        present_timing.delay_us(),
        pacer.fresh_hit_max_age_us()
    );
    let predecessor_catalog_migration_required =
        mister_magik_catalog::predecessor_cleanup::predecessor_catalog_artifacts_present(
            launcher_config.catalog_paths(),
        );
    if predecessor_catalog_migration_required {
        print_startup_event(
            start,
            "predecessor_catalog_detected",
            format!(
                "path={}",
                launcher_config
                    .catalog_paths()
                    .sharded_catalog_dir()
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("catalog-v3")
                    .display()
            ),
        );
        return_catalog_capsule::remove_return_catalog_capsule();
    }
    let return_capsule_target = if predecessor_catalog_migration_required {
        None
    } else {
        launch_return_session.state().and_then(|state| {
            Some((
                state.collection_id()?.to_string(),
                state.game_path().to_string(),
            ))
        })
    };
    let return_capsule = return_capsule_target.and_then(|(collection_id, game_path)| {
        let capsule_started = Instant::now();
        match return_catalog_capsule::take_return_catalog_capsule(
            Path::new(&arcade_root),
            &collection_id,
            &game_path,
        ) {
            Ok(capsule) => {
                print_startup_event(
                    start,
                    "return_catalog_capsule_decoded",
                    format!("elapsed_us={}", capsule_started.elapsed().as_micros()),
                );
                Some(capsule)
            }
            Err(error) => {
                print_startup_event(
                    start,
                    "return_catalog_capsule_rejected",
                    format!(
                        "elapsed_us={} error={}",
                        capsule_started.elapsed().as_micros(),
                        error.replace('\t', " ")
                    ),
                );
                launch_return_session.note_capsule_failure(error);
                None
            }
        }
    });
    let return_capsule_fingerprint = return_capsule
        .as_ref()
        .map(|capsule| capsule.durable_catalog_fingerprint.clone());
    let mut catalog = return_capsule
        .map(|capsule| capsule.catalog)
        .unwrap_or_else(|| empty_arcade_catalog(&arcade_root));
    let mut catalog_ready = !catalog.is_empty();
    let mut return_capsule_active = catalog_ready;
    let catalog_refresh_policy = catalog_refresh_policy();
    let catalog_refresh = catalog_refresh_policy.force_requested();
    let catalog_worker_enabled =
        predecessor_catalog_migration_required || catalog_refresh_policy.worker_enabled();
    let mut lifecycle = LauncherLifecycle::new(
        LauncherLifecycleConfig {
            catalog_worker_enabled,
        },
        start,
    );
    lifecycle.set_catalog_root(arcade_root.clone());
    let deferred_library_rebuild = consume_library_rebuild_marker(catalog_worker_enabled, start);
    // A forced replacement is not a foreground operation when a capsule,
    // sharded registry, summary, or existing database can seed the launcher.
    // First creation remains foreground through the !catalog_ready lifecycle.
    let mut catalog_session = LauncherCatalogSession::new(false);
    let media_session = ScreenshotMediaUpdateSession::default();
    let capsule_seed_ready = catalog_ready;
    let warm_registry_hydration_pending = !predecessor_catalog_migration_required
        && defer_warm_registry_hydration(
            capsule_seed_ready,
            startup_return_requested,
            mister_magik_catalog::shard_registry::manifest_slots_present(
                launcher_config.catalog_paths().sharded_catalog_dir(),
            ),
            catalog_refresh,
        );
    let sharded_seed = (!predecessor_catalog_migration_required
        && !capsule_seed_ready
        && !warm_registry_hydration_pending)
        .then(|| {
            read_sharded_registry_seed(
                &arcade_root,
                launcher_config.catalog_paths().sharded_catalog_dir(),
                start,
            )
        })
        .flatten();
    let sharded_seed_ready = sharded_seed.is_some();
    let sharded_catalog_fingerprint = sharded_seed
        .as_ref()
        .map(|seed| seed.catalog_fingerprint.clone());
    if let Some(seed) = sharded_seed {
        catalog = seed.catalog;
        catalog_ready = true;
    }
    let initial_catalog_fingerprint = return_capsule_fingerprint.or(sharded_catalog_fingerprint);
    let catalog_generation =
        initialize_catalog_generation(&mut scheduler, initial_catalog_fingerprint);
    if initial_system_entry_reader_required(capsule_seed_ready, sharded_seed_ready) {
        match scheduler.open_system_entry_reader() {
            Ok(elapsed_us) => print_startup_event(
                start,
                "system_entry_reader_opened",
                format!(
                    "generation={} elapsed_us={} cpu=0 preludes=on-demand",
                    catalog_generation.current.as_deref().unwrap_or("unknown"),
                    elapsed_us,
                ),
            ),
            Err(error) => print_startup_event(
                start,
                "system_entry_reader_open_failed",
                format!("error={}", error.replace('\t', " ")),
            ),
        }
    }
    let mut startup_ready_catalog_source = CatalogSource::FreshBuild;
    if capsule_seed_ready {
        startup_ready_catalog_source = CatalogSource::ReturnCapsule;
        catalog_session.note_summary_seed_ready();
        catalog_version = catalog_version.wrapping_add(1);
        let request = summary_seed_catalog_worker_request(
            catalog_refresh_policy,
            deferred_library_rebuild,
            true,
        )
        .unwrap_or(CatalogWorkerRequest::LoadOnly);
        let initial_cache = CatalogWorkerInitialCache::AlreadyLoadedReady;
        print_startup_event(
            start,
            "return_catalog_capsule_ready",
            format!(
                "root={} games={} request={}",
                arcade_root,
                catalog.len(),
                request.label()
            ),
        );
        let execution_mode = CatalogExecutionMode::BackgroundInteractive;
        scheduler.start_catalog_worker(arcade_root.clone(), request, initial_cache, execution_mode);
    } else if sharded_seed_ready {
        startup_ready_catalog_source = CatalogSource::ShardedRegistry;
        catalog_session.note_summary_seed_ready();
        catalog_version = catalog_version.wrapping_add(1);
        let return_catalog_hydration_needed = startup_return_requested;
        let request = summary_seed_catalog_worker_request(
            catalog_refresh_policy,
            deferred_library_rebuild,
            return_catalog_hydration_needed,
        );
        if let Some(request) = request {
            // NavPack and per-system SQLite rows are the hydration authority.
            // Source validation remains separate from the selected-system hot path.
            let initial_cache = CatalogWorkerInitialCache::AlreadyLoadedReady;
            if summary_seed_catalog_worker_starts_immediately(
                request,
                return_catalog_hydration_needed,
            ) {
                let execution_mode = CatalogExecutionMode::BackgroundInteractive;
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                scheduler.start_catalog_worker(
                    arcade_root.clone(),
                    request,
                    initial_cache,
                    execution_mode,
                );
            } else {
                let delay = catalog_background_validation_delay();
                print_startup_event(
                    start,
                    "catalog_worker_deferred",
                    format!(
                        "root={} request={} delay_ms={} reason=sharded_registry_hydration",
                        arcade_root,
                        request.label(),
                        delay.as_millis()
                    ),
                );
                catalog_session.defer_catalog_worker(
                    arcade_root.clone(),
                    request,
                    initial_cache,
                    CatalogExecutionMode::BackgroundInteractive,
                );
            }
        } else {
            catalog_session.mark_refresh_done();
        }
    } else if warm_registry_hydration_pending {
        print_startup_event(
            start,
            "catalog_registry_deferred",
            format!(
                "path={} reveal=catalog_ready",
                launcher_config
                    .catalog_paths()
                    .sharded_catalog_dir()
                    .display()
            ),
        );
        catalog_session.defer_catalog_worker(
            arcade_root.clone(),
            CatalogWorkerRequest::StrictLoad,
            CatalogWorkerInitialCache::AlreadyProbedMissing,
            CatalogExecutionMode::BackgroundInteractive,
        );
    } else {
        match catalog_startup_without_registry_plan(catalog_worker_enabled) {
            CatalogStartupWithoutSummaryPlan::DeferredWorker {
                request,
                initial_cache,
                execution_mode,
            } => {
                print_startup_event(
                    start,
                    "catalog_worker_deferred",
                    format!(
                        "root={} request={} reason=first_visible_copy",
                        arcade_root,
                        request.label(),
                    ),
                );
                catalog_session.defer_catalog_worker(
                    arcade_root.clone(),
                    request,
                    initial_cache,
                    execution_mode,
                );
            }
            CatalogStartupWithoutSummaryPlan::NoCatalog => {
                print_startup_event(
                    start,
                    "catalog_refresh_decision",
                    format!(
                        "cache_state=missing refresh_policy={} background_validation=false plan=load_only",
                        catalog_refresh_policy.label()
                    ),
                );
                catalog_session.mark_refresh_done();
            }
        }
    }
    nav.sync_launcher_taxonomy(&catalog);
    if sharded_seed_ready && !capsule_seed_ready {
        launch_return_restored =
            launch_return_session.apply(&mut nav, &catalog, CatalogSource::ShardedRegistry);
    }
    if !capsule_seed_ready && !launch_return_restored {
        let _ = request_pending_launch_return_shard(
            launch_return_session.state(),
            &catalog,
            catalog_version,
            &mut nav,
            &mut scheduler,
            Instant::now(),
            start,
        );
    }
    if capsule_seed_ready {
        launch_return_restored =
            launch_return_session.apply(&mut nav, &catalog, CatalogSource::ReturnCapsule);
        if !launch_return_restored {
            crate::ui_errln!("return catalog capsule could not restore saved destination");
            catalog = empty_arcade_catalog(&arcade_root);
            catalog_ready = false;
            return_capsule_active = false;
            startup_ready_catalog_source = CatalogSource::FreshBuild;
            nav.sync_launcher_taxonomy(&catalog);
        }
    }
    nav.set_arcade_exit_locked(return_capsule_active);
    apply_home_selected(&mut nav, &catalog, benchmark_config.home_selected(), start);
    crate::device_art::warm_in_background();
    // One snapshot of the visible card level, rebuilt only when it no longer
    // matches navigation so the render loop does not allocate labels per frame.
    let card_level = crate::launcher_home::CardLevelSnapshot::from_runtime(&nav, &catalog);
    // The level and card the neighbours were last prepared for.
    let card_prefetch_key: (String, usize) = (String::new(), usize::MAX);
    let card_frame_rendered_last_iteration = false;
    let launcher_card_home = match super::launcher_card_home::LauncherCardHomeSession::new(
        super::launcher_card_home::scene_for_display(ui, layout),
        card_level.clone(),
        nav.selected,
        &last_clock_text,
    ) {
        Ok(session) => Some(session),
        Err(error) => {
            crate::ui_errln!("launcher card home initialization failed: {error}");
            None
        }
    };
    let bridge_systems_t = Instant::now();
    let arcade_screen_pending = (start_screen == Screen::Arcade
        || lock_screen == Some(Screen::Arcade))
        && !arcade_navigation_ready(catalog_ready, &catalog);
    let navigation = app.global::<slint_ui::launcher::NavigationView>();
    let menu_title = slint::SharedString::from(nav.current_menu_title());
    let menu_breadcrumb = slint::SharedString::from(nav.current_menu_breadcrumb());
    navigation.set_menu_hierarchy(crate::launcher_view_types::menu_hierarchy(
        nav.current_menu_id() == crate::launcher_taxonomy::ROOT_MENU_ID,
    ));
    navigation.set_menu_title(menu_title);
    navigation.set_menu_breadcrumb(menu_breadcrumb);
    set_launcher_update_available(app, false);
    let menu_items = bridge_models.menu_items(&nav, catalog_version);
    let menu_item_presentation = bridge_models.menu_item_presentation();
    navigation.set_menu_item_presentation(menu_item_presentation);
    navigation.set_menu_items(menu_items);
    let update_check = UpdateCheck::start(should_check_for_updates(
        false,
        navigation.get_development_build(),
    ));
    print_startup_event(
        start,
        "catalog_bridge_systems",
        format!(
            "catalog_ready={} systems={} elapsed_us={}",
            catalog_ready,
            catalog.systems.len(),
            bridge_systems_t.elapsed().as_micros()
        ),
    );
    let catalog_scan_title = if catalog_ready {
        if catalog_session.foreground_update() {
            "Indexing library".to_string()
        } else if catalog_refresh {
            "Validating library".to_string()
        } else {
            String::new()
        }
    } else if !catalog_worker_enabled {
        String::new()
    } else {
        "Indexing library".to_string()
    };
    let catalog_scan_detail = if catalog_ready {
        if catalog_session.foreground_update() {
            "Rebuilding catalog with latest games...".to_string()
        } else {
            format!("Using cached {} games", catalog.len())
        }
    } else if !catalog_worker_enabled {
        "Catalog worker disabled for benchmark restart".to_string()
    } else {
        "No cached catalog; scanning library...".to_string()
    };
    LauncherStatusPresenter::new(app).sync_catalog_scan(CatalogScanBridgeStatus::new(
        initial_catalog_scan_visible(
            catalog_ready,
            catalog_worker_enabled,
            catalog_session.foreground_update(),
            warm_registry_hydration_pending,
        ),
        false,
        catalog_scan_message(catalog_session.foreground_update()),
        catalog_scan_title,
        catalog_scan_detail,
        -1,
    ));
    let bridge_sync_t = Instant::now();
    sync_bridge_launcher(
        app,
        pad,
        &nav,
        &lifecycle,
        &setup,
        "",
        "",
        &catalog,
        &mut preview,
        &mut bridge_models,
        catalog_version,
        false,
        false,
        ui,
    );
    print_startup_event(
        start,
        "catalog_bridge_sync",
        format!(
            "catalog_ready={} games={} elapsed_us={}",
            catalog_ready,
            catalog.len(),
            bridge_sync_t.elapsed().as_micros()
        ),
    );
    lifecycle_effects.clear();
    let startup_catalog_state = if catalog_ready {
        StartupCatalogState::Ready {
            source: startup_ready_catalog_source,
            validation_scheduled: scheduler.catalog_worker_running()
                || !catalog_session.refresh_done(),
        }
    } else {
        StartupCatalogState::Building {
            mode: CatalogBuildMode::FirstBuild,
            foreground_catalog_update: catalog_session.foreground_update(),
            has_stale_catalog: false,
        }
    };
    let startup_mode = if predecessor_catalog_migration_required {
        StartupMode::ColdNoCatalog
    } else if startup_return_requested || launch_return_restored {
        StartupMode::ReturnFromGame
    } else if warm_registry_hydration_pending {
        StartupMode::WarmCatalogHydrating
    } else if catalog_ready {
        StartupMode::WarmCatalog
    } else {
        StartupMode::ColdNoCatalog
    };
    lifecycle.begin_startup_reveal(startup_mode, start, &mut lifecycle_effects);
    if !predecessor_catalog_migration_required
        && startup_return_requested
        && !launch_return_restored
    {
        lifecycle.handle(
            LauncherLifecycleInput::StartupReturnCatalogHydrationNeeded,
            &mut lifecycle_effects,
        );
    }
    if launch_return_restored {
        emit_return_context_restored(
            &mut lifecycle,
            &mut lifecycle_effects,
            &nav,
            &catalog,
            &preview,
            &mut launch_return_session,
            start,
        );
    }
    let _ = lifecycle.classify_startup_catalog(startup_catalog_state, &mut lifecycle_effects);
    apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
    window.request_redraw();
    let startup_intro_eligible = startup_intro_is_eligible(
        startup_mode,
        predecessor_catalog_migration_required,
        false,
        screensaver_start_mode,
        layout.is_portrait(),
    );
    let startup_intro = if startup_intro_eligible
        && launcher_presenter.startup_intro_native_hidden_slots_available(ui)
    {
        match PreparedStartupIntro::new(ui) {
            Ok(prepared) => {
                print_startup_event(
                    start,
                    "startup_intro_started",
                    format!("width={} height={} fps=60", ui.fb_w(), ui.fb_h()),
                );
                Some(prepared.start())
            }
            Err(error) => {
                crate::ui_errln!("startup intro preparation failed: {error}");
                None
            }
        }
    } else {
        if startup_intro_eligible {
            print_startup_event(
                start,
                "startup_intro_skipped",
                "reason=direct-hidden-route-unavailable",
            );
        }
        None
    };
    // The particle scene owns the visible output. Keep Slint and its bridge
    // dormant until the existing launcher reveal transition fires, then build
    // exactly one off-screen launcher frame for the live morph target.
    let startup_intro_launcher_frame_ready = false;
    let startup_intro_bridge_dirty_pending = false;
    let startup_intro_catalog_ui_replay = None;
    let startup_intro_catalog_shells_pending = false;
    if startup_intro.is_some()
        && let Some(worker) = catalog_session.maybe_start_deferred_worker(
            scheduler.catalog_worker_running(),
            true,
            true,
            Instant::now(),
            Duration::ZERO,
        )
    {
        print_startup_event(start, "catalog_worker_start", &worker.root);
        // A missing catalog always needs the first-visible Build operation,
        // even when a force-refresh request selected Reconcile before the
        // cache probe. The intro also owns CPU1, so override the ordinary cold
        // foreground mode at this boundary.
        let request = startup_intro_catalog_worker_request(worker.request);
        let execution_mode = CatalogExecutionMode::BackgroundInteractive;
        let lifecycle_input = deferred_catalog_worker_lifecycle_input(execution_mode, request);
        lifecycle.handle(lifecycle_input, &mut lifecycle_effects);
        apply_lifecycle_effects(&mut lifecycle_effects, &mut scheduler, start);
        scheduler.start_catalog_worker(worker.root, request, worker.initial_cache, execution_mode);
    }
    LoopState {
        launcher_ui_actions,
        start,
        frame_clock,
        idle_slept_since,
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
        start_screen,
        lock_screen,
        launch_return_session,
        arcade_catalog_required_at_start,
        pending_start_system,
        crt_layout,
        crt_metrics,
        preview_route,
        nav,
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
        media_session,
        catalog_generation,
        card_level,
        card_prefetch_key,
        card_frame_rendered_last_iteration,
        launcher_card_home,
        arcade_screen_pending,
        update_check,
        startup_intro,
        startup_intro_launcher_frame_ready,
        startup_intro_bridge_dirty_pending,
        startup_intro_catalog_ui_replay,
        startup_intro_catalog_shells_pending,
    }
}
