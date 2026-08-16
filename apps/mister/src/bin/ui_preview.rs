// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(target_os = "macos")]
#[path = "ui_preview/scene_manifest.rs"]
mod ui_preview_scene_manifest;

#[cfg(target_os = "macos")]
#[path = "ui_preview/visual_compare.rs"]
mod ui_preview_visual_compare;

#[cfg(target_os = "macos")]
mod macos {
    use super::ui_preview_scene_manifest::{
        LauncherScene, SceneProfile, SceneScenario, SceneTransitionEdge, launcher_scene_manifest,
    };
    use super::ui_preview_visual_compare::{compare_launcher_matrix, validate_comparison_paths};
    use mister_magik_catalog::portable_catalog_builder::{
        PortableCatalogBuild, publish_portable_catalog,
    };
    use mister_magik_catalog::preview_worker::{
        PreviewPixels as CatalogPreviewPixels, PreviewWorker, load_preview_asset_pixels,
        preview_archive_path_for_system, preview_asset_cache_key, resolved_preview_archive_path,
    };
    use mister_magik_fb::arcade_catalog::{
        ArcadeCatalog, ArcadeFilter, ArcadeGameEntry, MENU_ARCADE_SYSTEM_ID,
    };
    use mister_magik_fb::crt_backdrop::{CRT_BACKDROP_FADE_DURATION, CrtBackdropState};
    use mister_magik_fb::framebuffer::target::{FramebufferTargetGeometry, UiFrameTarget};
    use mister_magik_fb::input_event::{
        InputEvent, InputPhase, InputSourceId, InputSourceKind, LogicalAction, PressId, SourceEpoch,
    };
    use mister_magik_fb::input_state::PadState;
    use mister_magik_fb::launcher::{
        ArcadeSearchPane, LauncherAction, LauncherEvent, LauncherNav, NavigationTransitionState,
        Screen, settings_display_resolution_index, settings_display_resolutions,
    };
    use mister_magik_fb::launcher_presentation::LauncherBridgePresenter;
    use mister_magik_fb::launcher_runtime::catalog::{
        ShardedCatalogSeed, load_sharded_registry_seed_at,
    };
    use mister_magik_fb::launcher_runtime::input_router::{
        DirectionalPolicy, FocusRequest, FocusTarget, InputContextKind, InputOutcome, InputRouter,
    };
    use mister_magik_fb::launcher_runtime::media::{
        MediaWorkerConfig, MediaWorkerHandle, MediaWorkerMessage,
        start_screenshot_media_worker_with_config,
    };
    use mister_magik_fb::launcher_runtime::navigation_transition::{
        CrtNavigationLayout, NavigationTransitionDirection, NavigationTransitionEdge,
        NavigationTransitionEndpoint, NavigationTransitionPhase, NavigationTransitionRoute,
        NavigationTransitionRuntime, crt_navigation_geometry, hdmi_navigation_geometry,
    };
    use mister_magik_fb::launcher_runtime::settings::{FileSettingsStore, SettingsStore};
    use mister_magik_fb::launcher_taxonomy::{
        CONSOLES_MENU_ID, LauncherMenuItemKind, ROOT_MENU_ID,
    };
    use mister_magik_fb::macos_preview_content::{
        ContentMode, PreviewContent, default_settings_path, resolve_preview_content,
    };
    use mister_magik_fb::particle_engine::{ParticleConfig, ParticlePreset};
    use mister_magik_fb::particle_renderer::ParticleRenderer;
    use mister_magik_fb::preview_transition::{
        PreviewTransitionController, Rgb565PreviewTransitionCompositor, transition_duration,
    };
    use mister_magik_fb::production_launcher_screensaver::LauncherScreensaver;
    use mister_magik_fb::ui_display::{
        CrtUiMetrics, ResolvedOutputRoute, ScreenOrientation, UiDisplay, UiDisplayPlan,
        UiFramebufferSizePolicy, UiLayoutGeometry, UiPixelSize,
    };
    use mister_magik_fb::ui_preview_fixtures::{FixtureScreenshot, UiPreviewFixtures};
    use mister_magik_fb::visual_composition::{
        ArcadeVisualLayer, PreviewFrame, PreviewPixels, PreviewSurface, hdmi_preview_rect,
    };
    use mister_magik_fb::visual_platform::{MisterPlatform, MisterSoftwareWindow};
    use mister_magik_framebuffer_scenes::Rgb565SurfaceMut;
    use mister_magik_ui::launcher::{
        ArcadeGame, Launcher, MenuItem, MenuItemKind, MenuItemPresentation, MenuItemStatus,
        MisterBridge, MisterUi, ScreenshotPackProgress,
    };
    use sha2::{Digest, Sha256};
    use slint::platform::software_renderer::{RepaintBufferType, Rgb565Pixel};
    use slint::{ComponentHandle, Model, ModelRc, PhysicalSize, SharedString, VecModel};
    use softbuffer::{Context, Surface};
    use std::cell::Cell;
    use std::collections::{HashMap, VecDeque};
    use std::error::Error;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::num::NonZeroU32;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::keyboard::{KeyCode, PhysicalKey};
    use winit::window::{Window, WindowId};

    const HDMI_FRAME_WIDTH: usize = 960;
    const HDMI_FRAME_HEIGHT: usize = 540;
    const DEFAULT_REFRESH_HZ: u32 = 60;
    const MAX_AUTO_REFRESH_HZ: u32 = 120;
    const PREVIEW_TRANSITION_DURATION: Duration = Duration::from_millis(200);
    const ARCADE_MEDIA_SYSTEM_ID: &str = "arcade";
    const PARTICLE_SCENE_SEED: u64 = 0x4d61_6769_4b;
    const SCREENSHOT_TILE_SEED: u64 = 0x4d61_6769_4b54_696c;
    const CAPTURE_PROVENANCE_SCHEMA: &str = "mister-magik-launcher-capture-v1";
    const PINNED_SLINT_VERSION: &str = "1.17.1";
    const RGB565_CONVERSION_VERSION: &str = "rgb565-le-expand-v1";
    const PREVIEW_RENDERER_ID: &str = "slint-software-rgb565-reused-buffer";
    const PNG_ENCODER_ID: &str = "png-rgb8-filter-none-zlib-best-v1";

    pub fn run() -> Result<(), Box<dyn Error>> {
        let options = PreviewOptions::parse(std::env::args().skip(1))?;
        if options.list_scenes {
            println!(
                "{}",
                serde_json::to_string_pretty(&launcher_scene_manifest()?)?
            );
            return Ok(());
        }
        if let Some(expected_dir) = options.check_baselines.as_deref() {
            run_baseline_check(expected_dir)?;
            return Ok(());
        }
        if let Some(output_dir) = options.matrix_output.as_deref() {
            if let (Some(expected_dir), Some(mismatch_dir)) = (
                options.expected_matrix.as_deref(),
                options.mismatch_output.as_deref(),
            ) {
                validate_comparison_paths(expected_dir, output_dir, mismatch_dir)?;
            }
            run_scene_matrix(output_dir)?;
            if let (Some(expected_dir), Some(mismatch_dir)) = (
                options.expected_matrix.as_deref(),
                options.mismatch_output.as_deref(),
            ) {
                let scene_ids = launcher_scene_manifest()?
                    .scenes
                    .into_iter()
                    .map(|scene| scene.id)
                    .collect::<Vec<_>>();
                compare_launcher_matrix(expected_dir, output_dir, mismatch_dir, &scene_ids)?;
                println!("comparison=passed scenes={}", scene_ids.len());
            }
            return Ok(());
        }
        let headless = options.output.is_some();
        let display = options.display_profile.display();
        let layout = UiLayoutGeometry::for_display(&display, options.orientation);
        let (frame_width, frame_height) = (layout.logical_w(), layout.logical_h());
        let content = resolve_preview_content(
            options.content_mode,
            options.sd_root.as_deref(),
            options.cache_root.as_deref(),
            headless,
        )?;
        let slint_window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let fixed_time = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(MisterPlatform::new(
            Rc::clone(&slint_window),
            Some(Rc::clone(&fixed_time)),
        )))?;
        slint_window.set_size(PhysicalSize::new(frame_width as u32, frame_height as u32));

        let launcher = Launcher::new()?;
        let ui = launcher.global::<MisterUi>();
        configure_display_profile(&ui, options.display_profile, options.orientation);
        let bridge = launcher.global::<MisterBridge>();
        initialize_bridge(&bridge, options.display_profile);
        launcher.show()?;
        slint_window.request_redraw();

        let mut application = PreviewApplication::new(
            launcher,
            slint_window,
            fixed_time,
            Scenario::Home,
            options.refresh_rate,
            headless,
            content,
            !options.no_scan,
            !options.no_download,
            options.display_profile,
            options.orientation,
            options.navigation_transition_demo.is_some()
                || options.settings_page_transition_demo
                || options.navigation_transition_duration_ms.is_some(),
        )?;
        application
            .navigation_transition
            .configure_preview(options.navigation_transition_duration_ms);
        application.select_scenario(if options.settings_page_transition_demo {
            Scenario::Home
        } else {
            options.scenario
        });
        if let Some(edge) = options.navigation_transition_demo {
            application.configure_navigation_transition_demo(edge)?;
        }
        if headless && options.scenario == Scenario::ScreenshotTiles {
            application.load_headless_screenshot_tiles()?;
        }
        if let Some(output) = options.output.as_deref() {
            let mut demo_origin_selected_id = None;
            let mut demo_origin_frame = None;
            let navigation_demo = options.navigation_transition_demo.is_some()
                || options.settings_page_transition_demo;
            if navigation_demo {
                for _ in 0..36 {
                    application.compose_frame();
                }
                demo_origin_selected_id = application
                    .launcher_nav
                    .current_menu_items()
                    .get(application.launcher_nav.selected)
                    .map(|item| item.id.clone());
                demo_origin_frame = Some(application.frame_target.cached_565().to_vec());
                if options.settings_page_transition_demo {
                    application.enqueue_launcher_action(LogicalAction::Home, true);
                } else {
                    application.enqueue_launcher_action(LogicalAction::Activate, true);
                }
                if options.navigation_transition_demo_reverse {
                    let settle_frames = options
                        .navigation_transition_duration_ms
                        .unwrap_or(600)
                        .saturating_mul(options.refresh_rate.headless_hz() as u64)
                        / 1_000
                        + 12;
                    for step in 0..settle_frames {
                        application.compose_frame();
                        if step == 0 {
                            application.enqueue_launcher_action(LogicalAction::Activate, false);
                            application.enqueue_launcher_action(LogicalAction::Home, false);
                        }
                    }
                    let reverse_origin = application.frame_target.cached_565().to_vec();
                    application.enqueue_launcher_action(LogicalAction::Back, true);
                    application.compose_frame();
                    application.enqueue_launcher_action(LogicalAction::Back, false);
                    if let Some((count, min_x, min_y, max_x, max_y)) = frame_difference(
                        &reverse_origin,
                        application.frame_target.cached_565(),
                        frame_width,
                        frame_height,
                    ) {
                        return Err(format!(
                            "reverse demo frame 0 differs from its destination origin: pixels={count} bounds={min_x},{min_y}..{max_x},{max_y}"
                        )
                        .into());
                    }
                }
            }
            let capture_advance_count = if options.navigation_transition_demo_reverse {
                options.frame
            } else {
                options.frame.saturating_add(1)
            };
            for _ in 0..capture_advance_count {
                application.compose_frame();
                application.enqueue_launcher_action(LogicalAction::Activate, false);
                application.enqueue_launcher_action(LogicalAction::Home, false);
            }
            if options.navigation_transition_demo_reverse
                && !application.navigation_transition.is_active()
            {
                let restored_selected_id = application
                    .launcher_nav
                    .current_menu_items()
                    .get(application.launcher_nav.selected)
                    .map(|item| item.id.as_str());
                if restored_selected_id != demo_origin_selected_id.as_deref() {
                    return Err(format!(
                        "reverse demo restored selection {:?}, expected {:?}",
                        restored_selected_id,
                        demo_origin_selected_id.as_deref()
                    )
                    .into());
                }
                if let Some(origin) = demo_origin_frame.as_deref()
                    && let Some((count, min_x, min_y, max_x, max_y)) = frame_difference(
                        origin,
                        application.frame_target.cached_565(),
                        frame_width,
                        frame_height,
                    )
                {
                    return Err(format!(
                        "reverse demo endpoint differs from its originating frame: pixels={count} bounds={min_x},{min_y}..{max_x},{max_y}"
                    )
                    .into());
                }
            }
            if options.scenario == Scenario::ScreenshotTiles {
                application.settle_headless_production_screensaver()?;
            }
            let capture = oriented_capture(application.frame_target.cached_565(), layout);
            let capture_hash = frame_hash(&capture);
            write_capture(
                output,
                &capture,
                layout.composition_w(),
                layout.composition_h(),
            )?;
            if let Some(provenance_output) = options.provenance_output.as_deref() {
                let provenance = CaptureProvenance::for_capture(
                    &options,
                    layout.composition_w(),
                    layout.composition_h(),
                    application.refresh_hz,
                    application.fixed_time.get(),
                    capture_hash,
                );
                write_capture_provenance(provenance_output, &provenance)?;
                println!(
                    "provenance={} sha256={}",
                    provenance_output.display(),
                    provenance.identity()?
                );
            }
            println!(
                "capture={} scenario={} frame={} refresh_hz={} transition_phase={:?} transition_progress_q16={} hash={:016x}",
                output.display(),
                options.scenario.label(),
                options.frame,
                application.refresh_hz,
                application.navigation_transition.frame().phase,
                application.navigation_transition.frame().progress_q16,
                capture_hash
            );
            return Ok(());
        }
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::WaitUntil(application.next_frame_deadline));
        event_loop.run_app(&mut application)?;
        Ok(())
    }

    fn preview_catalog(
        content: &PreviewContent,
        fixture_catalog: ArcadeCatalog,
    ) -> Result<(ArcadeCatalog, Option<u64>, String), String> {
        let Some(layout) = content.card() else {
            return Ok((fixture_catalog, None, "catalog:fixtures".to_owned()));
        };
        let candidates = [
            ("mac-cache", layout.catalog_root.clone()),
            (
                "card-production",
                layout.card_root.join("mister-magik").join("catalog-v3"),
            ),
            (
                "card-development",
                layout.card_root.join("mister-magik-dev").join("catalog-v3"),
            ),
        ];
        let mut selected: Option<(ShardedCatalogSeed, &'static str)> = None;
        let mut failures = Vec::new();
        for (label, path) in candidates {
            match load_sharded_registry_seed_at("/media/fat/_Arcade", &path) {
                Ok(seed)
                    if selected
                        .as_ref()
                        .is_none_or(|(current, _)| seed.generation > current.generation) =>
                {
                    selected = Some((seed, label));
                }
                Ok(_) => {}
                Err(error) => failures.push(format!("{label}={}", error.status)),
            }
        }
        if let Some((seed, label)) = selected {
            let generation = seed.generation;
            return Ok((
                seed.catalog,
                Some(generation),
                format!("catalog:{label}:g{generation}"),
            ));
        }
        eprintln!(
            "catalog: no valid Catalog V3 seed for {}; {}",
            layout.card_root.display(),
            failures.join(" ")
        );
        Ok((
            ArcadeCatalog::new(PathBuf::from("/media/fat/_Arcade"), Vec::new(), Vec::new()),
            None,
            "catalog:missing".to_owned(),
        ))
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TilePackFingerprint {
        path: PathBuf,
        bytes: u64,
        modified_nanos: u128,
    }

    struct TilePackLoader {
        fingerprint: TilePackFingerprint,
        cancelled: Arc<AtomicBool>,
        receiver: mpsc::Receiver<Result<LauncherScreensaver, String>>,
    }

    struct PreviewApplication {
        launcher: Launcher,
        slint_window: Rc<MisterSoftwareWindow>,
        fixed_time: Rc<Cell<Duration>>,
        native_window: Option<Arc<Window>>,
        surface: Option<Surface<Arc<Window>, Arc<Window>>>,
        frame_target: UiFrameTarget,
        frame_width: usize,
        frame_height: usize,
        xrgb8888: Vec<u32>,
        scenario: Scenario,
        selection: usize,
        settings_focused: bool,
        launcher_nav: LauncherNav,
        launcher_pad: PadState,
        input_router: InputRouter,
        launcher_input_events: VecDeque<InputEvent>,
        launcher_active_presses: HashMap<LogicalAction, PressId>,
        launcher_input_sequence: u64,
        launcher_press_sequence: u64,
        launcher_epoch: Instant,
        bridge_presenter: LauncherBridgePresenter,
        content: PreviewContent,
        catalog: ArcadeCatalog,
        catalog_generation: Option<u64>,
        catalog_source: String,
        catalog_worker: Option<mpsc::Receiver<CatalogWorkerEvent>>,
        settings_store: FileSettingsStore,
        fixture_screenshots: Vec<FixtureScreenshot>,
        loaded_screenshots: HashMap<String, FixtureScreenshot>,
        preview_worker: Option<PreviewWorker>,
        requested_preview_key: Option<String>,
        media_worker: Option<MediaWorkerHandle>,
        download_media_configured: bool,
        download_media: bool,
        arcade_layer: ArcadeVisualLayer,
        crt_backdrop: Option<CrtBackdropState>,
        crt_backdrop_target_key: Option<Option<String>>,
        preview_transition: PreviewTransitionController<()>,
        preview_compositor: Rgb565PreviewTransitionCompositor,
        preview_previous_index: Option<usize>,
        preview_current_index: Option<usize>,
        preview_transition_id: u64,
        preview_transition_duration: Duration,
        particle_renderer: Option<ParticleRenderer>,
        production_screensaver: Option<LauncherScreensaver>,
        tile_pack_loader: Option<TilePackLoader>,
        tile_pack_fingerprint: Option<TilePackFingerprint>,
        tile_pack_status: String,
        screensaver_elapsed: Duration,
        screensaver_paused: bool,
        screensaver_return: Option<Scenario>,
        refresh_rate: RefreshRate,
        refresh_hz: u32,
        headless: bool,
        headless_frame: u64,
        last_frame_at: Instant,
        schedule_anchor: Instant,
        schedule_frame: u64,
        next_frame_deadline: Instant,
        focused: bool,
        display_profile: DisplayProfile,
        orientation: ScreenOrientation,
        card_connected: bool,
        next_card_check_at: Instant,
        navigation_transition: NavigationTransitionRuntime,
        pending_navigation_event: Option<LauncherEvent>,
        pending_navigation_committed: bool,
        pending_navigation_source_state: Option<NavigationTransitionState>,
    }

    fn preview_input_focus(nav: &LauncherNav, transition: bool) -> FocusRequest {
        if transition {
            return FocusRequest {
                target: FocusTarget {
                    kind: InputContextKind::Transition,
                    owner: 1,
                },
                directional_policy: DirectionalPolicy::EdgeOnly,
            };
        }
        let (owner, directional_policy) = match nav.screen {
            Screen::Home => (1, DirectionalPolicy::HomeContinuous),
            Screen::SystemHub => (2, DirectionalPolicy::MenuRepeat),
            Screen::Controller => (3, DirectionalPolicy::EdgeOnly),
            Screen::Arcade if nav.arcade_uses_menu_repeat() => (4, DirectionalPolicy::MenuRepeat),
            Screen::Arcade => (4, DirectionalPolicy::ArcadeContinuous),
            Screen::Settings => (5, DirectionalPolicy::MenuRepeat),
            Screen::Screensaver => (6, DirectionalPolicy::EdgeOnly),
            Screen::About => (7, DirectionalPolicy::MenuRepeat),
            Screen::Licenses => (8, DirectionalPolicy::MenuRepeat),
            Screen::Info => (9, DirectionalPolicy::MenuRepeat),
        };
        FocusRequest {
            target: FocusTarget {
                kind: InputContextKind::Screen,
                owner,
            },
            directional_policy,
        }
    }

    fn commit_preview_navigation_destination(
        nav: &mut LauncherNav,
        catalog: &ArcadeCatalog,
        event: Option<&LauncherEvent>,
        transition: &mut NavigationTransitionRuntime,
        now_us: u64,
    ) -> bool {
        let committed = event.is_some_and(|event| nav.commit_navigation_intent(event, catalog));
        if !committed {
            transition.request_reverse(now_us);
        }
        committed
    }

    impl PreviewApplication {
        fn new(
            launcher: Launcher,
            slint_window: Rc<MisterSoftwareWindow>,
            fixed_time: Rc<Cell<Duration>>,
            scenario: Scenario,
            refresh_rate: RefreshRate,
            headless: bool,
            content: PreviewContent,
            scan_card: bool,
            download_media: bool,
            display_profile: DisplayProfile,
            orientation: ScreenOrientation,
            force_navigation_motion: bool,
        ) -> Result<Self, Box<dyn Error>> {
            let fixtures = UiPreviewFixtures::new()?;
            let (catalog, catalog_generation, catalog_source) =
                preview_catalog(&content, fixtures.catalog)?;
            let display = display_profile.display();
            let layout = UiLayoutGeometry::for_display(&display, orientation);
            let frame_width = layout.logical_w();
            let frame_height = layout.logical_h();
            let mut launcher_nav = LauncherNav::for_crt_layout_with_row_height(
                display_profile.is_crt(),
                CrtUiMetrics::for_display(&display).game_row_height,
            );
            let settings_store = FileSettingsStore::new(default_settings_path());
            launcher_nav.settings = settings_store.load();
            launcher_nav.settings.screen_orientation = orientation;
            launcher_nav.set_portrait_layout(layout.is_portrait());
            launcher_nav.sync_orientation_selection();
            launcher_nav.display_selected = display_profile.display_resolution_index();
            launcher_nav.display_highlighted = display_profile
                .settings_display_resolution_index()
                .unwrap_or(0);
            let bridge = launcher.global::<MisterBridge>();
            if let Ok(artwork) = mister_magik_fb::snes_artwork::SnesArtwork::load(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/snes/snes-small-v1.rgb565a"),
            ) {
                let pixels = artwork.rgba8_bytes();
                bridge.set_snes_artwork(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                    slint::Rgba8Pixel,
                >::clone_from_slice(
                    &pixels,
                    artwork.width as u32,
                    artwork.height as u32,
                )));
                bridge.set_snes_artwork_visible(true);
            }
            bridge.set_orientation_active_label(orientation.label().into());
            bridge.set_orientation_selected(launcher_nav.orientation_selected as i32);
            bridge.set_orientation_highlighted(launcher_nav.orientation_highlighted as i32);
            launcher_nav.catalog_build_started();
            for system in &catalog.systems {
                launcher_nav.catalog_system_discovered(&system.id);
                launcher_nav.catalog_system_update_ready(&system.id);
            }
            launcher_nav.sync_launcher_taxonomy(&catalog);
            launcher_nav.catalog_build_finished(&catalog);
            let refresh_hz = refresh_rate.headless_hz();
            let now = Instant::now();
            let next_frame_deadline = now + elapsed_for_frame(1, refresh_hz);
            let catalog_worker = if scan_card && !headless {
                content.card().map(spawn_catalog_worker).transpose()?
            } else {
                None
            };
            let preview_worker = content.card().map(|_| PreviewWorker::new());
            let card_connected = content
                .card()
                .is_none_or(|layout| layout.card_root.is_dir());
            launcher
                .global::<MisterBridge>()
                .set_build_label(format!("Mac visual preview · {}", content.label()).into());
            let navigation_motion_enabled =
                force_navigation_motion || !launcher_nav.settings.reduce_motion;
            let mut application = Self {
                launcher,
                slint_window,
                fixed_time,
                native_window: None,
                surface: None,
                frame_target: UiFrameTarget::cached(FramebufferTargetGeometry::new(
                    frame_width,
                    frame_height,
                )),
                frame_width,
                frame_height,
                xrgb8888: Vec::new(),
                scenario,
                selection: 0,
                settings_focused: false,
                launcher_nav,
                launcher_pad: PadState::default(),
                input_router: InputRouter::new(FocusRequest {
                    target: FocusTarget {
                        kind: InputContextKind::Screen,
                        owner: 1,
                    },
                    directional_policy: DirectionalPolicy::HomeContinuous,
                }),
                launcher_epoch: Instant::now(),
                bridge_presenter: LauncherBridgePresenter::default(),
                content,
                catalog,
                catalog_generation,
                catalog_source,
                catalog_worker,
                settings_store,
                fixture_screenshots: fixtures.screenshots,
                loaded_screenshots: HashMap::new(),
                preview_worker,
                requested_preview_key: None,
                media_worker: None,
                download_media_configured: download_media && !headless,
                download_media: download_media && !headless,
                arcade_layer: ArcadeVisualLayer::new(frame_width, frame_height),
                crt_backdrop: CrtBackdropState::for_display(&display),
                crt_backdrop_target_key: None,
                preview_transition: PreviewTransitionController::default(),
                preview_compositor: Rgb565PreviewTransitionCompositor::new(
                    frame_width,
                    frame_height,
                ),
                preview_previous_index: None,
                preview_current_index: None,
                preview_transition_id: 0,
                preview_transition_duration: PREVIEW_TRANSITION_DURATION,
                particle_renderer: None,
                production_screensaver: None,
                tile_pack_loader: None,
                tile_pack_fingerprint: None,
                tile_pack_status: "tiles:fixtures".to_owned(),
                screensaver_elapsed: Duration::ZERO,
                screensaver_paused: false,
                screensaver_return: None,
                refresh_rate,
                refresh_hz,
                headless,
                headless_frame: 0,
                last_frame_at: now,
                schedule_anchor: now,
                schedule_frame: 1,
                next_frame_deadline,
                focused: true,
                display_profile,
                orientation,
                card_connected,
                next_card_check_at: now + Duration::from_secs(1),
                navigation_transition: NavigationTransitionRuntime::new(
                    frame_width,
                    frame_height,
                    navigation_motion_enabled,
                ),
                pending_navigation_event: None,
                pending_navigation_committed: false,
                pending_navigation_source_state: None,
                launcher_input_events: VecDeque::new(),
                launcher_active_presses: HashMap::new(),
                launcher_input_sequence: 0,
                launcher_press_sequence: 0,
            };
            application.select_scenario(scenario);
            if headless {
                application.load_headless_selected_preview();
            }
            Ok(application)
        }

        fn create_window(&mut self, event_loop: &ActiveEventLoop) {
            let attributes = Window::default_attributes()
                .with_title(self.window_title())
                .with_inner_size(LogicalSize::new(
                    self.frame_width as f64,
                    self.frame_height as f64,
                ))
                .with_min_inner_size(LogicalSize::new(
                    (self.frame_width / 2) as f64,
                    (self.frame_height / 2) as f64,
                ));
            let window = Arc::new(
                event_loop
                    .create_window(attributes)
                    .expect("create preview window"),
            );
            let context = Context::new(Arc::clone(&window)).expect("create softbuffer context");
            let surface =
                Surface::new(&context, Arc::clone(&window)).expect("create preview window surface");
            self.native_window = Some(window);
            self.surface = Some(surface);
            self.refresh_from_monitor();
        }

        fn window_title(&self) -> String {
            format!(
                "MiSTer MagiK UI Preview — {} — {} — {} Hz — {} — {} — {} — {}",
                self.scenario.label(),
                self.scenario.shortcut(),
                self.refresh_hz,
                self.content.label(),
                self.catalog_source,
                self.display_profile.label(),
                self.tile_pack_status,
            )
        }

        fn select_scenario(&mut self, scenario: Scenario) {
            let previous_scenario = self.scenario;
            if scenario == Scenario::ScreenshotTiles
                && previous_scenario != Scenario::ScreenshotTiles
            {
                self.screensaver_return = Some(previous_scenario);
            }
            self.scenario = scenario;
            self.selection = 0;
            self.settings_focused = false;
            if scenario != Scenario::OrientationChoice {
                self.launcher_nav.orientation_combo_open = false;
            }
            if scenario == Scenario::ArcadeCrossfade {
                self.preview_previous_index = None;
                self.preview_current_index = Some(0);
                self.preview_transition.reset();
            }
            if scenario.uses_launcher_navigation() {
                self.configure_launcher_screen(scenario);
            }
            apply_scenario(&self.launcher, scenario);
            self.sync_orientation_geometry();
            if matches!(scenario, Scenario::Arcade | Scenario::ArcadeCrossfade) {
                self.arcade_layer.invalidate();
            }
            if matches!(scenario, Scenario::ParticleScreensaver) {
                self.magik_particle_renderer().invalidate_hidden_slot(1);
                self.screensaver_elapsed = Duration::ZERO;
            }
            if matches!(scenario, Scenario::ScreenshotTiles) {
                self.screensaver_elapsed = Duration::ZERO;
                self.ensure_screenshot_tile_images();
            }
            if scenario.uses_launcher_navigation() {
                self.sync_launcher_navigation();
            } else {
                self.update_selection();
            }
            self.sync_orientation_geometry();
            self.arcade_layer.configure_for_display(
                &self.display_profile.display(),
                self.orientation,
                matches!(scenario, Scenario::ArcadeSearch),
            );
            self.prime_crt_backdrop(scenario);
            if let Some(window) = self.native_window.as_ref() {
                window.set_title(&self.window_title());
                window.request_redraw();
            }
        }

        fn sync_orientation_geometry(&self) {
            let bridge = self.launcher.global::<MisterBridge>();
            bridge.set_orientation_active_label(self.orientation.label().into());
            bridge.set_orientation_combo_open(self.launcher_nav.orientation_combo_open);
            if !self.orientation.is_portrait() {
                return;
            }
            let preview = hdmi_preview_rect(self.frame_width, self.frame_height);
            bridge.set_arcade_preview_box_x(preview.x0 as i32);
            bridge.set_arcade_preview_box_y(preview.y0 as i32);
            bridge.set_arcade_preview_box_width(preview.width() as i32);
            bridge.set_arcade_preview_box_height(preview.rows() as i32);
            let margin = 16usize;
            let search = matches!(self.scenario, Scenario::ArcadeSearch);
            let list_y = if search { 56 } else { preview.y1 + 12 };
            let list_height = if search {
                self.frame_height * 34 / 100
            } else {
                self.frame_height.saturating_sub(list_y + margin)
            };
            bridge.set_arcade_list_x(margin as i32);
            bridge.set_arcade_list_y(list_y as i32);
            bridge.set_arcade_list_width(self.frame_width.saturating_sub(margin * 2) as i32);
            bridge.set_arcade_list_height(list_height as i32);
        }

        fn move_selection(&mut self, delta: isize) {
            let count = match self.scenario {
                Scenario::Home | Scenario::BackgroundScan | Scenario::Confirm => 6,
                Scenario::SystemHub => 4,
                Scenario::Settings => 6,
                Scenario::About => 2,
                Scenario::Licenses => 2,
                Scenario::ScreensaverSettings => 3,
                Scenario::Arcade => self.catalog.system_game_count(MENU_ARCADE_SYSTEM_ID),
                _ => 1,
            };
            self.selection = self
                .selection
                .saturating_add_signed(delta)
                .min(count.saturating_sub(1));
            self.update_selection();
        }

        fn update_selection(&self) {
            let bridge = self.launcher.global::<MisterBridge>();
            bridge.set_selected_index(self.selection as i32);
            bridge.set_system_hub_selected(self.selection as i32);
            bridge.set_settings_focused(self.settings_focused);
            bridge.set_settings_selected(self.selection as i32);
            bridge.set_about_selected(self.selection as i32);
            bridge.set_licenses_selected(self.selection as i32);
            bridge.set_screensaver_settings_selected(self.selection as i32);
            bridge.set_confirm_selected(self.selection.min(1) as i32);
            if matches!(
                self.scenario,
                Scenario::Arcade | Scenario::ArcadeSearch | Scenario::ArcadeCrossfade
            ) {
                let games = self
                    .catalog
                    .games
                    .iter()
                    .filter(|game| game.system_id.as_ref() == MENU_ARCADE_SYSTEM_ID)
                    .collect::<Vec<_>>();
                apply_arcade_fixture_bridge(&self.launcher, "Arcade", &games, self.selection);
            }
            self.slint_window.request_redraw();
        }

        fn set_settings_focused(&mut self, focused: bool) {
            self.settings_focused = focused;
            self.update_selection();
        }

        fn activate_selection(&mut self) {
            if self.scenario == Scenario::Settings && self.selection == 0 {
                let bridge = self.launcher.global::<MisterBridge>();
                bridge.set_display_combo_open(!bridge.get_display_combo_open());
                self.slint_window.request_redraw();
                return;
            }
            if let Some(scenario) =
                activated_scenario(self.scenario, self.selection, self.settings_focused)
            {
                self.select_scenario(scenario);
            }
        }

        fn go_back(&mut self) {
            if let Some(scenario) = back_scenario(self.scenario) {
                self.select_scenario(scenario);
            }
        }

        fn handle_key(&mut self, code: KeyCode) {
            if let Some(scenario) = shortcut_scenario(code) {
                self.select_scenario(scenario);
                return;
            }
            if self.scenario == Scenario::ScreenshotTiles {
                match code {
                    KeyCode::Space => {
                        self.screensaver_paused = !self.screensaver_paused;
                    }
                    KeyCode::Period if self.screensaver_paused => {
                        self.screensaver_elapsed += elapsed_for_frame(1, self.refresh_hz);
                    }
                    _ => self.exit_screenshot_tiles(),
                }
                return;
            }
            if matches!(code, KeyCode::Escape | KeyCode::Backspace) {
                self.go_back();
                return;
            }
            if self.scenario == Scenario::Home {
                match code {
                    KeyCode::ArrowUp => self.set_settings_focused(true),
                    KeyCode::ArrowDown if self.settings_focused => {
                        self.set_settings_focused(false);
                    }
                    KeyCode::ArrowLeft if !self.settings_focused => self.move_selection(-1),
                    KeyCode::ArrowRight if !self.settings_focused => self.move_selection(1),
                    KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => {
                        self.activate_selection();
                    }
                    _ => {}
                }
                return;
            }
            match code {
                KeyCode::ArrowUp | KeyCode::ArrowLeft => self.move_selection(-1),
                KeyCode::ArrowDown | KeyCode::ArrowRight => self.move_selection(1),
                KeyCode::Enter | KeyCode::NumpadEnter => self.activate_selection(),
                KeyCode::Space
                    if matches!(
                        self.scenario,
                        Scenario::ParticleScreensaver | Scenario::ScreenshotTiles
                    ) =>
                {
                    self.screensaver_paused = !self.screensaver_paused;
                }
                KeyCode::Period if self.screensaver_paused => {
                    self.screensaver_elapsed += elapsed_for_frame(1, self.refresh_hz);
                }
                _ => {}
            }
        }

        fn configure_launcher_screen(&mut self, scenario: Scenario) {
            match scenario {
                Scenario::Home => self.launcher_nav.go_root(),
                Scenario::SystemHub => {
                    if !self.launcher_nav.open_system(&self.catalog, "snes") {
                        self.launcher_nav.screen = Screen::SystemHub;
                    }
                }
                Scenario::Arcade | Scenario::ArcadeSearch | Scenario::ArcadeCrossfade => {
                    self.launcher_nav.open_default_arcade(&self.catalog);
                    match scenario {
                        Scenario::ArcadeSearch => {
                            self.launcher_nav.arcade_filter.active = ArcadeFilter::Search;
                            self.launcher_nav.arcade_search.query = "PAC".into();
                            let collection_id = self
                                .launcher_nav
                                .active_collection_id()
                                .unwrap_or(MENU_ARCADE_SYSTEM_ID)
                                .to_owned();
                            self.launcher_nav
                                .refresh_arcade_search_if_active(&self.catalog, &collection_id);
                            self.launcher_nav.arcade_search.selected_key = 15;
                            self.launcher_nav.arcade_search.pane = ArcadeSearchPane::Keyboard;
                        }
                        Scenario::ArcadeCrossfade => {
                            self.launcher_nav.arcade.selected = 1;
                            self.launcher_nav.arcade.snap_to_selected();
                        }
                        _ => {}
                    }
                }
                Scenario::Settings => self.launcher_nav.screen = Screen::Settings,
                Scenario::OrientationChoice => {
                    self.launcher_nav.screen = Screen::Settings;
                    self.launcher_nav.settings_selected = 1;
                    self.launcher_nav.orientation_combo_open = true;
                    self.launcher_nav.orientation_highlighted =
                        self.launcher_nav.orientation_selected;
                }
                Scenario::Controller => self.launcher_nav.screen = Screen::Controller,
                Scenario::About => self.launcher_nav.screen = Screen::About,
                Scenario::Licenses => self.launcher_nav.screen = Screen::Licenses,
                Scenario::Info => self.launcher_nav.screen = Screen::Info,
                Scenario::ScreensaverSettings => self.launcher_nav.screen = Screen::Screensaver,
                _ => {}
            }
            self.launcher_pad = PadState::default();
            self.launcher_input_events.clear();
            self.launcher_active_presses.clear();
            self.input_router = InputRouter::new(preview_input_focus(&self.launcher_nav, false));
        }

        fn configure_navigation_transition_demo(
            &mut self,
            edge: NavigationTransitionEdge,
        ) -> Result<(), Box<dyn Error>> {
            self.scenario = Scenario::Home;
            self.launcher_nav.go_root();
            let target_id = match edge {
                NavigationTransitionEdge::HomeToConsoles => CONSOLES_MENU_ID.to_string(),
                NavigationTransitionEdge::HomeToArcade => MENU_ARCADE_SYSTEM_ID.to_string(),
                NavigationTransitionEdge::ConsolesToSystem => {
                    if !self.launcher_nav.open_menu(CONSOLES_MENU_ID) {
                        return Err("Consoles menu is unavailable in preview content".into());
                    }
                    self.launcher_nav
                        .current_menu_items()
                        .iter()
                        .find(|item| item.kind == LauncherMenuItemKind::Collection)
                        .map(|item| item.id.clone())
                        .ok_or("Consoles menu has no directly launchable system")?
                }
            };
            self.launcher_nav.selected = self
                .launcher_nav
                .current_menu_items()
                .iter()
                .position(|item| item.id == target_id)
                .ok_or("requested transition destination is unavailable")?;
            self.launcher_nav.scroll_x = 0;
            self.launcher_pad = PadState::default();
            self.launcher_input_events.clear();
            self.launcher_active_presses.clear();
            self.input_router = InputRouter::new(preview_input_focus(&self.launcher_nav, false));
            self.sync_launcher_navigation();
            Ok(())
        }

        fn handle_launcher_key(&mut self, code: KeyCode, state: ElementState) -> bool {
            let action = match code {
                KeyCode::ArrowUp => Some(LogicalAction::Up),
                KeyCode::ArrowDown => Some(LogicalAction::Down),
                KeyCode::ArrowLeft => Some(LogicalAction::Left),
                KeyCode::ArrowRight => Some(LogicalAction::Right),
                KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => {
                    Some(LogicalAction::Activate)
                }
                KeyCode::Escape | KeyCode::Backspace => Some(LogicalAction::Back),
                KeyCode::Home => Some(LogicalAction::Home),
                KeyCode::KeyX => Some(LogicalAction::X),
                _ => None,
            };
            let Some(action) = action else {
                return false;
            };
            self.enqueue_launcher_action(action, state == ElementState::Pressed);
            true
        }

        fn enqueue_launcher_action(&mut self, action: LogicalAction, pressed: bool) {
            let (phase, press_id) = if pressed {
                if self.launcher_active_presses.contains_key(&action) {
                    return;
                }
                self.launcher_press_sequence =
                    self.launcher_press_sequence.saturating_add(1).max(1);
                let press_id = PressId(self.launcher_press_sequence);
                self.launcher_active_presses.insert(action, press_id);
                (InputPhase::Pressed, press_id)
            } else {
                let Some(press_id) = self.launcher_active_presses.remove(&action) else {
                    return;
                };
                (InputPhase::Released, press_id)
            };
            self.launcher_input_sequence = self.launcher_input_sequence.saturating_add(1).max(1);
            self.launcher_input_events.push_back(InputEvent {
                source: InputSourceId {
                    kind: InputSourceKind::Preview,
                    instance: 1,
                },
                source_epoch: SourceEpoch(1),
                sequence: self.launcher_input_sequence,
                press_id,
                captured_at_us: self.fixed_time.get().as_micros().min(u64::MAX as u128) as u64,
                action,
                phase,
            });
        }

        fn tick_launcher_navigation(&mut self) {
            if !self.scenario.uses_launcher_navigation() {
                return;
            }
            let frame_now = self.launcher_epoch + self.fixed_time.get();
            let now_us = self.fixed_time.get().as_micros().min(u64::MAX as u128) as u64;
            loop {
                if self.navigation_transition.is_active() {
                    let request = preview_input_focus(&self.launcher_nav, true);
                    self.input_router.set_focus(request);
                    while let Some(event) = self.launcher_input_events.pop_front() {
                        match self.input_router.route_event(event, request, frame_now) {
                            InputOutcome::Dispatch { .. }
                            | InputOutcome::Released { .. }
                            | InputOutcome::WakeScreensaver { .. }
                            | InputOutcome::Consumed { .. } => {}
                        }
                    }
                    if !self.pending_navigation_committed {
                        let committed = commit_preview_navigation_destination(
                            &mut self.launcher_nav,
                            &self.catalog,
                            self.pending_navigation_event.as_ref(),
                            &mut self.navigation_transition,
                            now_us,
                        );
                        if committed {
                            self.pending_navigation_committed = true;
                        }
                    }
                    self.navigation_transition.tick(now_us);
                    self.finish_navigation_tick();
                    return;
                }

                let request = preview_input_focus(&self.launcher_nav, false);
                self.input_router.set_focus(request);
                let mut final_tick = false;
                let routed_event = if let Some(event) = self.launcher_input_events.pop_front() {
                    match self.input_router.route_event(event, request, frame_now) {
                        InputOutcome::Dispatch { event, .. } => Some(event),
                        InputOutcome::Released { event, context, .. }
                            if context == self.input_router.context() =>
                        {
                            Some(event)
                        }
                        InputOutcome::Released { .. }
                        | InputOutcome::WakeScreensaver { .. }
                        | InputOutcome::Consumed { .. } => None,
                    }
                } else if let Some(InputOutcome::Dispatch { event, .. }) =
                    self.input_router.tick_repeat(frame_now)
                {
                    Some(event)
                } else {
                    final_tick = true;
                    None
                };
                self.launcher_pad = PadState::default();
                for action in LogicalAction::ALL {
                    self.launcher_pad
                        .set_logical_action(action, self.input_router.action_held(action));
                }

                let settings_transition_source = (self.navigation_transition.enabled()
                    && routed_event.as_ref().is_some_and(|event| {
                        event.phase == InputPhase::Pressed
                            && matches!(
                                event.action,
                                LogicalAction::Activate | LogicalAction::Back | LogicalAction::Home
                            )
                            && matches!(
                                self.launcher_nav.screen,
                                Screen::Home
                                    | Screen::Settings
                                    | Screen::Screensaver
                                    | Screen::About
                                    | Screen::Info
                                    | Screen::Licenses
                            )
                    }))
                .then(|| {
                    (
                        self.launcher_nav.screen,
                        self.launcher_nav.navigation_transition_state(),
                    )
                });
                let event = if let Some(input_event) = routed_event.as_ref() {
                    self.launcher_nav.handle_action_with_navigation_intents(
                        input_event,
                        frame_now,
                        &self.catalog,
                    )
                } else if final_tick {
                    self.launcher_nav.handle_held_tick_with_navigation_intents(
                        &self.launcher_pad,
                        frame_now,
                        &self.catalog,
                    )
                } else {
                    None
                };
                let event = if !final_tick {
                    event.or_else(|| {
                        self.launcher_nav.handle_held_tick_with_navigation_intents(
                            &self.launcher_pad,
                            frame_now,
                            &self.catalog,
                        )
                    })
                } else {
                    event
                };
                if let Some((source_screen, source_state)) = settings_transition_source
                    && let Some((route, direction)) =
                        settings_page_transition(source_screen, self.launcher_nav.screen)
                    && self
                        .navigation_transition
                        .begin_settings_page(
                            route,
                            direction,
                            self.frame_target.cached_565(),
                            now_us,
                        )
                        .unwrap_or(false)
                {
                    self.pending_navigation_event = Some(LauncherEvent {
                        action: LauncherAction::NavigateBack,
                        path: None,
                        settings: None,
                    });
                    self.pending_navigation_committed = true;
                    self.pending_navigation_source_state = Some(source_state);
                }
                if let Some(event) = event {
                    match event.action {
                        LauncherAction::OpenMenu
                        | LauncherAction::OpenCollection
                        | LauncherAction::NavigateBack
                        | LauncherAction::NavigateHome => {
                            if self.begin_navigation_transition(event.clone(), now_us) {
                                self.pending_navigation_source_state =
                                    Some(self.launcher_nav.navigation_transition_state());
                                self.pending_navigation_event = Some(event);
                                self.pending_navigation_committed = false;
                            } else {
                                self.launcher_nav
                                    .commit_navigation_intent(&event, &self.catalog);
                            }
                        }
                        LauncherAction::PreviewScreensaver => {
                            self.select_scenario(Scenario::ScreenshotTiles);
                            return;
                        }
                        LauncherAction::LaunchGame => {}
                        LauncherAction::PersistSettings => {
                            if let Some(settings) = event.settings.as_ref() {
                                self.navigation_transition.set_enabled(
                                    self.frame_width,
                                    self.frame_height,
                                    !settings.reduce_motion,
                                );
                                if let Err(error) = self.settings_store.save(settings) {
                                    eprintln!(
                                        "settings: failed to save Mac preview settings: {error}"
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if final_tick {
                    break;
                }
            }
            self.finish_navigation_tick();
        }

        fn begin_navigation_transition(&mut self, event: LauncherEvent, now_us: u64) -> bool {
            let Some((edge, direction)) =
                navigation_transition_for_intent(&self.launcher_nav, &event)
            else {
                return false;
            };
            let selected_label = self
                .launcher_nav
                .current_menu_items()
                .get(self.launcher_nav.selected)
                .map(|item| item.title.as_str())
                .unwrap_or("")
                .to_owned();
            let geometry = match direction {
                NavigationTransitionDirection::Forward => {
                    let root_menu = self.launcher_nav.current_menu_id() == ROOT_MENU_ID;
                    if self.display_profile.is_crt() {
                        let display = self.display_profile.display();
                        let content = display.content_rect();
                        let metrics = CrtUiMetrics::for_display(&display);
                        crt_navigation_geometry(
                            self.frame_width,
                            self.frame_height,
                            CrtNavigationLayout {
                                content_x: content.x,
                                content_y: content.y,
                                content_width: content.width,
                                content_height: content.height,
                                grid_x: metrics.grid_x.max(1) as usize,
                                grid_y: metrics.grid_y.max(1) as usize,
                                header_height: metrics.header_height.max(1) as usize,
                                footer_height: metrics.footer_height.max(1) as usize,
                                heading_font_height: metrics.heading_font.pixels().max(1) as usize,
                                title_font_height: metrics.card_title_font.pixels().max(1) as usize,
                                detail_font_height: metrics.card_detail_font.pixels().max(1)
                                    as usize,
                                game_row_height: metrics.game_row_height.max(1) as usize,
                            },
                            self.launcher_nav.selected,
                            self.launcher_nav.current_menu_items().len(),
                            root_menu,
                            edge,
                            &selected_label,
                        )
                    } else {
                        hdmi_navigation_geometry(
                            self.frame_width,
                            self.frame_height,
                            self.launcher_nav.selected,
                            self.launcher_nav.scroll_x,
                            root_menu,
                            edge,
                            &selected_label,
                        )
                    }
                }
                NavigationTransitionDirection::Reverse => {
                    let Some(geometry) = self.navigation_transition.geometry_for_reverse(edge)
                    else {
                        return false;
                    };
                    geometry
                }
            };
            self.navigation_transition
                .begin(
                    edge,
                    direction,
                    geometry,
                    self.frame_target.cached_565(),
                    now_us,
                )
                .unwrap_or(false)
        }

        fn finish_navigation_tick(&mut self) {
            let previous = self.scenario;
            self.scenario = match (previous, self.launcher_nav.screen) {
                (Scenario::ArcadeSearch, Screen::Arcade)
                    if self
                        .launcher_nav
                        .arcade_search
                        .is_active(&self.launcher_nav.arcade_filter.active) =>
                {
                    Scenario::ArcadeSearch
                }
                (Scenario::ArcadeCrossfade, Screen::Arcade) => Scenario::ArcadeCrossfade,
                (Scenario::OrientationChoice, Screen::Settings) => Scenario::OrientationChoice,
                (Scenario::ControllerSetup, Screen::Controller) => Scenario::ControllerSetup,
                _ => Scenario::from_screen(self.launcher_nav.screen),
            };
            self.sync_launcher_navigation();
            self.sync_navigation_transition_active();
            if self.scenario != previous
                && let Some(window) = self.native_window.as_ref()
            {
                window.set_title(&self.window_title());
            }
        }

        fn sync_navigation_transition_active(&self) {
            let bridge = self.launcher.global::<MisterBridge>();
            let active = self.navigation_transition.is_active();
            if bridge.get_navigation_transition_active() != active {
                bridge.set_navigation_transition_active(active);
            }
        }

        fn compose_navigation_transition(&mut self) {
            if !self.navigation_transition.is_active() {
                return;
            }
            let frame_work_started = Instant::now();
            let now_us = self.fixed_time.get().as_micros().min(u64::MAX as u128) as u64;
            let mut render_transition_frame = true;
            if self.pending_navigation_committed && !self.navigation_transition.destination_ready()
            {
                if self
                    .navigation_transition
                    .capture_destination(self.frame_target.cached_565(), now_us)
                    .is_err()
                {
                    self.navigation_transition.settle_at_destination();
                    render_transition_frame = false;
                }
                self.navigation_transition.tick(now_us);
            }
            if render_transition_frame && let Ok(frame) = self.navigation_transition.render() {
                self.frame_target.cached_565_mut().copy_from_slice(frame);
            }
            if self.navigation_transition.frame().phase == NavigationTransitionPhase::Settled {
                let completion = self.navigation_transition.complete();
                if completion.is_some_and(|completion| {
                    completion.endpoint == NavigationTransitionEndpoint::Destination
                }) && self
                    .pending_navigation_event
                    .as_ref()
                    .is_some_and(|event| event.action == LauncherAction::NavigateHome)
                {
                    self.navigation_transition.clear_geometry_history();
                }
                if completion.is_some_and(|completion| {
                    completion.endpoint == NavigationTransitionEndpoint::Source
                }) && let Some(source_state) = self.pending_navigation_source_state.take()
                {
                    self.launcher_nav
                        .restore_navigation_transition_state(source_state);
                }
                self.pending_navigation_event = None;
                self.pending_navigation_committed = false;
                self.pending_navigation_source_state = None;
            }
            self.navigation_transition.note_frame_work_us(
                frame_work_started
                    .elapsed()
                    .as_micros()
                    .min(u64::MAX as u128) as u64,
            );
            self.sync_navigation_transition_active();
        }

        fn exit_screenshot_tiles(&mut self) {
            let scenario = self
                .screensaver_return
                .take()
                .unwrap_or(Scenario::ScreensaverSettings);
            self.select_scenario(scenario);
        }

        fn sync_launcher_navigation(&mut self) {
            if self.launcher_nav.screen == Screen::Arcade {
                let selected = self.launcher_nav.arcade.selected;
                if self.preview_current_index != Some(selected) {
                    self.preview_previous_index = self.preview_current_index;
                    self.preview_current_index = Some(selected);
                    self.preview_transition_id = self.preview_transition_id.wrapping_add(1);
                    self.preview_transition_duration = transition_duration(
                        PREVIEW_TRANSITION_DURATION,
                        if self.launcher_nav.arcade.is_turbo_active() {
                            2
                        } else {
                            1
                        },
                    );
                }
            } else {
                self.preview_previous_index = None;
                self.preview_current_index = None;
                self.preview_transition.reset();
            }
            self.bridge_presenter.sync(
                &self.launcher,
                &self.launcher_nav,
                &self.catalog,
                self.catalog_generation
                    .and_then(|generation| usize::try_from(generation).ok())
                    .or(Some(0)),
                false,
            );
            let (fixture_title, fixture_system_id) = self
                .launcher_nav
                .active_collection()
                .map(|collection| {
                    (
                        collection.title.as_str(),
                        collection
                            .system_id
                            .as_deref()
                            .unwrap_or(collection.legacy_system_id.as_str()),
                    )
                })
                .unwrap_or(("Arcade", MENU_ARCADE_SYSTEM_ID));
            let fixture_games = self
                .catalog
                .games
                .iter()
                .filter(|game| game.system_id.as_ref() == fixture_system_id)
                .collect::<Vec<_>>();
            apply_arcade_fixture_bridge(
                &self.launcher,
                fixture_title,
                &fixture_games,
                self.launcher_nav.arcade.selected,
            );
            self.sync_orientation_geometry();
            self.request_selected_preview();
            self.slint_window.request_redraw();
        }

        fn compose_frame(&mut self) {
            self.poll_catalog_worker();
            self.poll_preview_worker();
            self.poll_media_worker();
            self.poll_screenshot_tile_loader();
            self.poll_card_connection();
            let frame_delta = self.frame_delta();
            self.tick_launcher_navigation();
            slint::platform::update_timers_and_animations();
            self.slint_window.request_redraw();
            self.slint_window.draw_if_needed(|renderer| {
                self.frame_target.render(renderer);
            });
            if matches!(
                self.scenario,
                Scenario::Arcade | Scenario::ArcadeSearch | Scenario::ArcadeCrossfade
            ) {
                let low_resolution_backdrop = self.crt_backdrop.is_some()
                    && matches!(self.scenario, Scenario::Arcade | Scenario::ArcadeCrossfade);
                if low_resolution_backdrop {
                    self.sync_crt_backdrop_target(false);
                    let display = self.display_profile.display();
                    let layout = UiLayoutGeometry::for_display(&display, self.orientation);
                    let metrics = CrtUiMetrics::for_display(&display);
                    if let Some(backdrop) = self.crt_backdrop.as_mut() {
                        let _ = backdrop.compose_product_into(
                            self.fixed_time.get(),
                            self.frame_target.cached_565_mut(),
                            layout.content_rect(),
                            metrics,
                        );
                    }
                }
                let games = self
                    .launcher_nav
                    .active_collection_id()
                    .map(|collection| {
                        self.launcher_nav
                            .active_arcade_game_view(&self.catalog, collection)
                    })
                    .unwrap_or_else(mister_magik_fb::arcade_catalog::ArcadeGameView::empty);
                if let Some(backdrop) = self.crt_backdrop.as_ref().filter(|_| {
                    matches!(self.scenario, Scenario::Arcade | Scenario::ArcadeCrossfade)
                }) {
                    let layout = UiLayoutGeometry::for_display(
                        &self.display_profile.display(),
                        self.orientation,
                    );
                    self.arcade_layer.compose_over_backdrop(
                        &mut self.frame_target,
                        backdrop.pixels(),
                        layout.output_layout(),
                        games,
                        self.launcher_nav.arcade.selected,
                        self.launcher_nav.arcade.visual_index,
                        true,
                    );
                } else {
                    self.arcade_layer.compose(
                        &mut self.frame_target,
                        games,
                        self.launcher_nav.arcade.selected,
                        self.launcher_nav.arcade.visual_index,
                        true,
                    );
                }
                if matches!(self.scenario, Scenario::Arcade | Scenario::ArcadeCrossfade)
                    && !self.display_profile.is_crt()
                {
                    let use_fixtures = matches!(self.content, PreviewContent::Fixtures);
                    let current = self
                        .preview_current_index
                        .or(Some(self.launcher_nav.arcade.selected))
                        .and_then(|index| preview_game(&self.launcher_nav, &self.catalog, index))
                        .and_then(|game| {
                            preview_screenshot(
                                game,
                                &self.loaded_screenshots,
                                &self.fixture_screenshots,
                                use_fixtures,
                            )
                        })
                        .or_else(|| {
                            use_fixtures
                                .then(|| self.fixture_screenshots.first())
                                .flatten()
                        });
                    let previous = self
                        .preview_previous_index
                        .and_then(|index| preview_game(&self.launcher_nav, &self.catalog, index))
                        .and_then(|game| {
                            preview_screenshot(
                                game,
                                &self.loaded_screenshots,
                                &self.fixture_screenshots,
                                use_fixtures,
                            )
                        });
                    if let Some(current) = current {
                        let transition = self.preview_transition.update(
                            Some(self.preview_transition_id),
                            previous.is_some(),
                            (),
                            self.preview_transition_duration,
                            self.fixed_time.get(),
                        );
                        self.preview_compositor.compose(
                            self.frame_target.cached_565_mut(),
                            self.frame_width,
                            self.frame_height,
                            hdmi_preview_rect(self.frame_width, self.frame_height),
                            previous.map(fixture_preview_frame),
                            fixture_preview_frame(current),
                            transition.progress,
                            PreviewSurface::full(self.frame_width),
                        );
                    }
                }
            } else if self.scenario == Scenario::ParticleScreensaver {
                if self.particle_renderer.is_none() {
                    let _ = self.magik_particle_renderer();
                }
                self.particle_renderer
                    .as_mut()
                    .expect("particle renderer initialized for selected scenario")
                    .render(
                        self.frame_target.cached_565_mut(),
                        1,
                        self.screensaver_elapsed,
                    )
                    .expect("render production particle screensaver");
            } else if self.scenario == Scenario::ScreenshotTiles {
                if let Some(screensaver) = self.production_screensaver.as_mut() {
                    screensaver.render_at(
                        self.frame_target.cached_565_mut(),
                        self.frame_width,
                        self.frame_height,
                        self.screensaver_elapsed,
                    );
                } else {
                    self.frame_target.cached_565_mut().fill(Rgb565Pixel(0));
                }
            }
            if matches!(
                self.scenario,
                Scenario::ParticleScreensaver | Scenario::ScreenshotTiles
            ) && !self.screensaver_paused
            {
                self.screensaver_elapsed += frame_delta;
            }
            self.compose_navigation_transition();
            self.fixed_time.set(self.fixed_time.get() + frame_delta);
        }

        fn prime_crt_backdrop(&mut self, scenario: Scenario) {
            if self.crt_backdrop.is_none() {
                return;
            }
            self.crt_backdrop_target_key = None;
            let now = self.fixed_time.get();
            if scenario == Scenario::ArcadeCrossfade {
                let current = self.preview_current_index;
                self.preview_current_index = self.preview_previous_index;
                self.sync_crt_backdrop_target(true);
                self.preview_current_index = current;
                self.crt_backdrop_target_key = None;
                self.sync_crt_backdrop_target(false);
            } else if scenario == Scenario::Arcade {
                self.sync_crt_backdrop_target(true);
            } else if let Some(backdrop) = self.crt_backdrop.as_mut() {
                backdrop.retarget_plain(now);
                let _ = backdrop.compose(now + CRT_BACKDROP_FADE_DURATION);
                self.crt_backdrop_target_key = Some(None);
            }
        }

        fn sync_crt_backdrop_target(&mut self, settle: bool) {
            let Some(index) = self
                .preview_current_index
                .or(Some(self.launcher_nav.arcade.selected))
            else {
                return;
            };
            let use_fixtures = matches!(self.content, PreviewContent::Fixtures);
            let screenshot = preview_game(&self.launcher_nav, &self.catalog, index)
                .and_then(|game| {
                    preview_screenshot(
                        game,
                        &self.loaded_screenshots,
                        &self.fixture_screenshots,
                        use_fixtures,
                    )
                })
                .or_else(|| {
                    use_fixtures
                        .then(|| self.fixture_screenshots.first())
                        .flatten()
                });
            let target_key = screenshot.map(|screenshot| screenshot.key.to_string());
            if self.crt_backdrop_target_key.as_ref() == Some(&target_key) {
                return;
            }
            let now = self.fixed_time.get();
            let Some(backdrop) = self.crt_backdrop.as_mut() else {
                return;
            };
            if let Some(screenshot) = screenshot {
                backdrop.retarget(Some(fixture_preview_frame(screenshot)), now);
            } else {
                backdrop.retarget_plain(now);
            }
            if settle {
                let _ = backdrop.compose(now + CRT_BACKDROP_FADE_DURATION);
            }
            self.crt_backdrop_target_key = Some(target_key);
        }

        fn request_selected_preview(&mut self) {
            let index = self
                .preview_current_index
                .unwrap_or(self.launcher_nav.arcade.selected);
            let Some(game) = preview_game(&self.launcher_nav, &self.catalog, index) else {
                return;
            };
            let Some(path) = self.resolve_preview_archive_path(&game.preview_archive_path) else {
                let system_id = game.system_id.to_string();
                self.ensure_media_download(&system_id);
                return;
            };
            let key = preview_storage_key(&path, &game.preview_asset_key);
            if self.loaded_screenshots.contains_key(&key)
                || self.requested_preview_key.as_deref() == Some(&key)
            {
                return;
            }
            let Some(worker) = self.preview_worker.as_mut() else {
                return;
            };
            worker.request_selected(
                game.title.to_string(),
                path,
                game.preview_asset_key.to_string(),
            );
            self.requested_preview_key = Some(key);
        }

        fn poll_preview_worker(&mut self) {
            let Some(result) = self
                .preview_worker
                .as_ref()
                .and_then(PreviewWorker::take_latest_selected_result)
            else {
                return;
            };
            self.requested_preview_key = None;
            let key = preview_storage_key(&result.preview_archive_path, &result.preview_asset_key);
            let asset_key = result.preview_asset_key.clone();
            let Some(image) = result.image else {
                return;
            };
            if let Some(screenshot) = loaded_screenshot(key, asset_key, image) {
                self.loaded_screenshots
                    .insert(screenshot.key.to_string(), screenshot);
                self.arcade_layer.invalidate();
                self.slint_window.request_redraw();
            }
        }

        fn load_headless_selected_preview(&mut self) {
            let index = self
                .preview_current_index
                .unwrap_or(self.launcher_nav.arcade.selected);
            let Some(game) = preview_game(&self.launcher_nav, &self.catalog, index) else {
                return;
            };
            let Some(path) = self.resolve_preview_archive_path(&game.preview_archive_path) else {
                eprintln!(
                    "preview: no host archive for title={:?} canonical={:?}",
                    game.title, game.preview_archive_path
                );
                return;
            };
            let image = match load_preview_asset_pixels(&path, &game.preview_asset_key) {
                Ok(image) => image,
                Err(error) => {
                    eprintln!(
                        "preview: load failed title={:?} archive={} key={:?}: {error}",
                        game.title, path, game.preview_asset_key
                    );
                    return;
                }
            };
            let key = preview_storage_key(&path, &game.preview_asset_key);
            if let Some(screenshot) =
                loaded_screenshot(key.clone(), game.preview_asset_key.to_string(), image)
            {
                eprintln!(
                    "preview: loaded title={:?} key={:?} geometry={}x{} nonblack={}",
                    game.title,
                    game.preview_asset_key,
                    screenshot.width,
                    screenshot.height,
                    screenshot
                        .pixels
                        .iter()
                        .filter(|pixel| pixel.0 != 0)
                        .count()
                );
                self.loaded_screenshots.insert(key, screenshot);
            }
        }

        fn resolve_preview_archive_path(&self, canonical: &str) -> Option<String> {
            let layout = self.content.card()?;
            let file_name = Path::new(canonical).file_name()?;
            let local = layout.media_root.join(file_name);
            let local = resolved_preview_archive_path(local.to_string_lossy().as_ref());
            if Path::new(&local).is_file() {
                return Some(local);
            }
            let card = layout.to_card_path(canonical).ok()?;
            let card = resolved_preview_archive_path(card.to_string_lossy().as_ref());
            if Path::new(&card).is_file() {
                return Some(card);
            }
            let development = layout
                .card_root
                .join("mister-magik-dev")
                .join("assets")
                .join(file_name);
            let development = resolved_preview_archive_path(development.to_string_lossy().as_ref());
            Path::new(&development).is_file().then_some(development)
        }

        fn ensure_screenshot_tile_images(&mut self) {
            if self.headless {
                return;
            }
            if matches!(self.content, PreviewContent::Fixtures) {
                self.tile_pack_status = "tiles:fixtures".to_owned();
                return;
            }
            let canonical = preview_archive_path_for_system(ARCADE_MEDIA_SYSTEM_ID);
            let Some(path) = self.resolve_preview_archive_path(&canonical) else {
                self.tile_pack_status = if self.download_media {
                    "tiles:downloading-arcade".to_owned()
                } else {
                    "tiles:arcade-pack-missing".to_owned()
                };
                self.ensure_media_download(ARCADE_MEDIA_SYSTEM_ID);
                return;
            };
            let path = PathBuf::from(path);
            let fingerprint = match tile_pack_fingerprint(&path) {
                Ok(fingerprint) => fingerprint,
                Err(error) => {
                    self.tile_pack_status = format!("tiles:error:{error}");
                    return;
                }
            };
            if self.tile_pack_fingerprint.as_ref() == Some(&fingerprint)
                || self
                    .tile_pack_loader
                    .as_ref()
                    .is_some_and(|loader| loader.fingerprint == fingerprint)
            {
                return;
            }
            self.cancel_screenshot_tile_load();
            let cancelled = Arc::new(AtomicBool::new(false));
            let worker_cancelled = Arc::clone(&cancelled);
            let worker_path = path.clone();
            let frame_width = self.frame_width;
            let frame_height = self.frame_height;
            let (sender, receiver) = mpsc::channel();
            let spawn = std::thread::Builder::new()
                .name("mac-screenshot-tiles".into())
                .spawn(move || {
                    let result = LauncherScreensaver::from_archive_path(
                        &worker_path,
                        frame_width,
                        frame_height,
                        SCREENSHOT_TILE_SEED,
                    );
                    if !worker_cancelled.load(Ordering::Relaxed) {
                        let _ = sender.send(result);
                    }
                });
            match spawn {
                Ok(_) => {
                    self.tile_pack_status = format!("tiles:loading:{}", fingerprint.path.display());
                    self.tile_pack_loader = Some(TilePackLoader {
                        fingerprint,
                        cancelled,
                        receiver,
                    });
                }
                Err(error) => {
                    self.tile_pack_status = format!("tiles:error:start loader: {error}");
                }
            }
        }

        fn cancel_screenshot_tile_load(&mut self) {
            if let Some(loader) = self.tile_pack_loader.take() {
                loader.cancelled.store(true, Ordering::Relaxed);
            }
        }

        fn poll_screenshot_tile_loader(&mut self) {
            let Some(loader) = self.tile_pack_loader.take() else {
                return;
            };
            match loader.receiver.try_recv() {
                Ok(Ok(screensaver)) => {
                    let fingerprint = loader.fingerprint;
                    self.production_screensaver = Some(screensaver);
                    self.tile_pack_status =
                        format!("tiles:production-parade:{}", fingerprint.path.display());
                    self.tile_pack_fingerprint = Some(fingerprint);
                }
                Ok(Err(error)) => {
                    self.tile_pack_status = format!("tiles:error:{error}");
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.tile_pack_loader = Some(loader);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.tile_pack_status = "tiles:error:loader disconnected".to_owned();
                }
            }
            if let Some(window) = self.native_window.as_ref() {
                window.set_title(&self.window_title());
                window.request_redraw();
            }
        }

        fn load_headless_screenshot_tiles(&mut self) -> Result<(), String> {
            if matches!(self.content, PreviewContent::Fixtures) {
                return Ok(());
            }
            let canonical = preview_archive_path_for_system(ARCADE_MEDIA_SYSTEM_ID);
            let path = self.resolve_preview_archive_path(&canonical).ok_or_else(|| {
                format!(
                    "headless card screenshot tiles require the Arcade screenshot pack {canonical}"
                )
            })?;
            let path = PathBuf::from(path);
            let fingerprint = tile_pack_fingerprint(&path)?;
            let screensaver = LauncherScreensaver::from_archive_path(
                &path,
                self.frame_width,
                self.frame_height,
                SCREENSHOT_TILE_SEED,
            )?;
            self.production_screensaver = Some(screensaver);
            self.tile_pack_status = format!("tiles:production-parade:{}", path.display());
            self.tile_pack_fingerprint = Some(fingerprint);
            Ok(())
        }

        fn settle_headless_production_screensaver(&mut self) -> Result<(), String> {
            const TIMEOUT: Duration = Duration::from_secs(10);
            let started = Instant::now();
            let Some(screensaver) = self.production_screensaver.as_mut() else {
                return Err("production screenshot screensaver is unavailable".to_owned());
            };
            while (screensaver.active_card_count() == 0 || screensaver.has_pending_card_work())
                && started.elapsed() < TIMEOUT
            {
                screensaver.render_at(
                    self.frame_target.cached_565_mut(),
                    self.frame_width,
                    self.frame_height,
                    self.screensaver_elapsed,
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            let active = screensaver.active_card_count();
            if active == 0 || screensaver.has_pending_card_work() {
                return Err(format!(
                    "production screenshot screensaver did not settle within {} ms (active={active} pending={})",
                    TIMEOUT.as_millis(),
                    screensaver.has_pending_card_work()
                ));
            }
            Ok(())
        }

        fn ensure_media_download(&mut self, system_id: &str) {
            if !self.download_media {
                return;
            }
            if self.media_worker.is_none() {
                let Some(layout) = self.content.card() else {
                    return;
                };
                let config = match MediaWorkerConfig::for_host(
                    layout.media_root.clone(),
                    layout.catalog_root.clone(),
                ) {
                    Ok(config) => config,
                    Err(error) => {
                        eprintln!("media: cannot configure Mac downloader: {error}");
                        self.download_media = false;
                        return;
                    }
                };
                self.media_worker = start_screenshot_media_worker_with_config(config);
            }
            if let Some(worker) = self.media_worker.as_ref() {
                worker.ensure_system(system_id);
            }
        }

        fn poll_media_worker(&mut self) {
            let mut reload_catalog = false;
            let mut reload_screenshot_tiles = false;
            let mut worker_done = false;
            while let Some(message) = self
                .media_worker
                .as_ref()
                .and_then(MediaWorkerHandle::try_recv)
            {
                let bridge = self.launcher.global::<MisterBridge>();
                match message {
                    MediaWorkerMessage::Progress(event) => {
                        let percent = event
                            .bytes_done
                            .min(event.bytes_total)
                            .saturating_mul(100)
                            .checked_div(event.bytes_total)
                            .and_then(|value| i32::try_from(value).ok())
                            .unwrap_or(-1);
                        bridge.set_media_pack_summary("Downloading screenshot packs".into());
                        bridge.set_media_pack_progresses(ModelRc::new(VecModel::from(vec![
                            ScreenshotPackProgress {
                                system: event.system.into(),
                                image_size: event.image_size.into(),
                                phase: event.phase.into(),
                                percent,
                                bytes_label: format!(
                                    "{} / {}",
                                    event.bytes_done, event.bytes_total
                                )
                                .into(),
                                pack_position: format!(
                                    "{} of {}",
                                    event.pack_index, event.pack_count
                                )
                                .into(),
                            },
                        ])));
                    }
                    MediaWorkerMessage::PreviewAvailabilityUpdated { .. } => {
                        reload_catalog = true;
                    }
                    MediaWorkerMessage::Failed { detail }
                    | MediaWorkerMessage::PreviewAvailabilityFailed { detail, .. } => {
                        bridge.set_media_pack_summary(
                            format!("Screenshot download failed: {detail}").into(),
                        );
                    }
                    MediaWorkerMessage::Done { .. } => {
                        bridge.set_media_pack_summary("Screenshot packs ready".into());
                        self.requested_preview_key = None;
                        worker_done = true;
                    }
                    MediaWorkerMessage::PackStatus {
                        system,
                        status,
                        detail,
                        ..
                    } if system == ARCADE_MEDIA_SYSTEM_ID => {
                        let update = screenshot_tile_media_update(&status, &detail);
                        self.tile_pack_status = update.status;
                        reload_screenshot_tiles |= update.reload;
                    }
                    MediaWorkerMessage::Timing { .. }
                    | MediaWorkerMessage::CacheMetadata { .. }
                    | MediaWorkerMessage::PackStatus { .. } => {}
                }
            }
            if reload_catalog {
                self.reload_mac_catalog();
            }
            if worker_done {
                self.media_worker = None;
            }
            if reload_screenshot_tiles {
                self.ensure_screenshot_tile_images();
            }
        }

        fn reload_mac_catalog(&mut self) {
            let Some(layout) = self.content.card() else {
                return;
            };
            let Ok(seed) =
                load_sharded_registry_seed_at("/media/fat/_Arcade", &layout.catalog_root)
            else {
                return;
            };
            self.catalog_generation = Some(seed.generation);
            self.catalog_source = format!("catalog:mac-cache:g{}", seed.generation);
            self.catalog = seed.catalog;
            self.launcher_nav.sync_launcher_taxonomy(&self.catalog);
            self.sync_launcher_navigation();
        }

        fn poll_card_connection(&mut self) {
            let now = Instant::now();
            if now < self.next_card_check_at {
                return;
            }
            self.next_card_check_at = now + Duration::from_secs(1);
            let connected = self
                .content
                .card()
                .is_none_or(|layout| layout.card_root.is_dir());
            if connected == self.card_connected {
                return;
            }
            self.card_connected = connected;
            if connected {
                self.catalog_source = "catalog:card-reconnected".to_owned();
                self.download_media = self.download_media_configured;
                if self.scenario == Scenario::ScreenshotTiles {
                    self.ensure_screenshot_tile_images();
                }
            } else {
                self.catalog_source = "catalog:card-disconnected".to_owned();
                self.catalog_worker = None;
                self.cancel_screenshot_tile_load();
                if let Some(worker) = self.media_worker.as_ref() {
                    worker.finish();
                }
                self.download_media = false;
                if self.tile_pack_fingerprint.is_none() {
                    self.production_screensaver = None;
                    self.tile_pack_status = "tiles:unavailable:card-disconnected".to_owned();
                } else {
                    self.tile_pack_status = "tiles:last-good:card-disconnected".to_owned();
                }
            }
            if let Some(window) = self.native_window.as_ref() {
                window.set_title(&self.window_title());
            }
        }

        fn poll_catalog_worker(&mut self) {
            let Some(receiver) = self.catalog_worker.take() else {
                return;
            };
            let mut keep_receiver = true;
            while let Ok(event) = receiver.try_recv() {
                let bridge = self.launcher.global::<MisterBridge>();
                match event {
                    CatalogWorkerEvent::Progress { title, detail } => {
                        bridge.set_catalog_scan_visible(true);
                        bridge.set_catalog_scan_title(title.into());
                        bridge.set_catalog_scan_message("Reading the mounted card".into());
                        bridge.set_catalog_scan_detail(detail.into());
                        bridge.set_catalog_scan_percent(0);
                    }
                    CatalogWorkerEvent::Ready(build) => {
                        self.catalog = build.catalog;
                        self.catalog_generation = Some(build.generation);
                        self.catalog_source = format!("catalog:mac-cache:g{}", build.generation);
                        self.launcher_nav.catalog_build_started();
                        for system in &self.catalog.systems {
                            self.launcher_nav.catalog_system_discovered(&system.id);
                            self.launcher_nav.catalog_system_update_ready(&system.id);
                        }
                        self.launcher_nav.sync_launcher_taxonomy(&self.catalog);
                        self.launcher_nav.catalog_build_finished(&self.catalog);
                        self.selection = self
                            .selection
                            .min(self.catalog.games.len().saturating_sub(1));
                        bridge.set_catalog_scan_visible(false);
                        self.sync_launcher_navigation();
                        if let Some(window) = self.native_window.as_ref() {
                            window.set_title(&self.window_title());
                        }
                        keep_receiver = false;
                    }
                    CatalogWorkerEvent::Failed(error) => {
                        bridge.set_catalog_scan_visible(true);
                        bridge.set_catalog_scan_title("Mac catalog scan failed".into());
                        bridge.set_catalog_scan_message("The card was not modified".into());
                        bridge.set_catalog_scan_detail(error.clone().into());
                        bridge.set_catalog_scan_percent(0);
                        self.catalog_source = "catalog:scan-failed".to_owned();
                        eprintln!("catalog scan failed: {error}");
                        keep_receiver = false;
                    }
                }
            }
            if keep_receiver {
                self.catalog_worker = Some(receiver);
            }
        }

        fn magik_particle_renderer(&mut self) -> &mut ParticleRenderer {
            self.particle_renderer.get_or_insert_with(|| {
                ParticleRenderer::new_magik(ParticleConfig {
                    count: 16_384,
                    width: self.frame_width,
                    height: self.frame_height,
                    seed: PARTICLE_SCENE_SEED,
                    preset: ParticlePreset::Visual,
                })
                .expect("create production particle screensaver")
            })
        }

        fn frame_delta(&mut self) -> Duration {
            if self.headless {
                let previous = elapsed_for_frame(self.headless_frame, self.refresh_hz);
                self.headless_frame = self.headless_frame.saturating_add(1);
                return elapsed_for_frame(self.headless_frame, self.refresh_hz)
                    .saturating_sub(previous);
            }
            let now = Instant::now();
            let delta = now
                .saturating_duration_since(self.last_frame_at)
                .min(Duration::from_millis(100));
            self.last_frame_at = now;
            delta
        }

        fn refresh_from_monitor(&mut self) {
            if self.refresh_rate != RefreshRate::Auto {
                return;
            }
            let detected = self
                .native_window
                .as_ref()
                .and_then(|window| window.current_monitor())
                .and_then(|monitor| monitor.refresh_rate_millihertz())
                .map(|millihertz| ((millihertz + 500) / 1_000).clamp(30, MAX_AUTO_REFRESH_HZ))
                .unwrap_or(DEFAULT_REFRESH_HZ);
            self.set_refresh_hz(detected);
        }

        fn set_refresh_hz(&mut self, refresh_hz: u32) {
            let refresh_hz = refresh_hz.clamp(1, MAX_AUTO_REFRESH_HZ);
            if self.refresh_hz == refresh_hz && self.schedule_frame > 0 {
                return;
            }
            self.refresh_hz = refresh_hz;
            let now = Instant::now();
            self.last_frame_at = now;
            self.schedule_anchor = now;
            self.schedule_frame = 1;
            self.next_frame_deadline = now + elapsed_for_frame(1, refresh_hz);
            if let Some(window) = self.native_window.as_ref() {
                window.set_title(&self.window_title());
            }
        }

        fn reset_focus_clock(&mut self) {
            let now = Instant::now();
            self.last_frame_at = now;
            self.schedule_anchor = now;
            self.schedule_frame = 1;
            self.next_frame_deadline = now + elapsed_for_frame(1, self.refresh_hz);
        }

        fn schedule_next_frame(&mut self, now: Instant) -> bool {
            if !self.focused {
                return false;
            }
            if now < self.next_frame_deadline {
                return false;
            }
            while self.next_frame_deadline <= now {
                self.schedule_frame = self.schedule_frame.saturating_add(1);
                self.next_frame_deadline =
                    self.schedule_anchor + elapsed_for_frame(self.schedule_frame, self.refresh_hz);
            }
            true
        }

        fn render(&mut self) {
            self.compose_frame();
            let Some(window) = self.native_window.as_ref() else {
                return;
            };
            let Some(surface) = self.surface.as_mut() else {
                return;
            };
            let size = window.inner_size();
            let Some(width) = NonZeroU32::new(size.width) else {
                return;
            };
            let Some(height) = NonZeroU32::new(size.height) else {
                return;
            };
            surface
                .resize(width, height)
                .expect("resize preview surface");

            let output_len = size.width as usize * size.height as usize;
            self.xrgb8888.resize(output_len, 0);
            scale_rgb565_nearest(
                self.frame_target.cached_565(),
                self.frame_width,
                self.frame_height,
                &mut self.xrgb8888,
                size.width as usize,
                size.height as usize,
            );
            let mut buffer = surface.buffer_mut().expect("map preview surface");
            buffer.copy_from_slice(&self.xrgb8888);
            buffer.present().expect("present preview surface");
        }
    }

    impl ApplicationHandler for PreviewApplication {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self.native_window.is_none() {
                self.create_window(event_loop);
            }
            if let Some(window) = self.native_window.as_ref() {
                window.request_redraw();
            }
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            window_id: WindowId,
            event: WindowEvent,
        ) {
            if self.native_window.as_ref().map(|window| window.id()) != Some(window_id) {
                return;
            }
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => self.render(),
                WindowEvent::KeyboardInput { event, .. } if !event.repeat => {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        if event.state == ElementState::Pressed && shortcut_scenario(code).is_some()
                        {
                            self.handle_key(code);
                        } else if self.scenario.uses_launcher_navigation() {
                            self.handle_launcher_key(code, event.state);
                        } else if event.state == ElementState::Pressed {
                            self.handle_key(code);
                        }
                    }
                }
                WindowEvent::Resized(_) => {
                    if let Some(window) = self.native_window.as_ref() {
                        window.request_redraw();
                    }
                }
                WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    self.refresh_from_monitor();
                }
                WindowEvent::Focused(focused) => {
                    self.focused = focused;
                    self.launcher_pad = PadState::default();
                    self.launcher_input_events.clear();
                    self.launcher_active_presses.clear();
                    self.input_router = InputRouter::new(preview_input_focus(
                        &self.launcher_nav,
                        self.navigation_transition.is_active(),
                    ));
                    self.reset_focus_clock();
                    if focused && let Some(window) = self.native_window.as_ref() {
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            if !self.focused {
                event_loop.set_control_flow(ControlFlow::Wait);
                return;
            }
            let now = Instant::now();
            if self.schedule_next_frame(now)
                && let Some(window) = self.native_window.as_ref()
            {
                window.request_redraw();
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame_deadline));
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Scenario {
        Home,
        SystemHub,
        Arcade,
        ArcadeSearch,
        ArcadeCrossfade,
        Settings,
        OrientationChoice,
        Controller,
        ControllerSetup,
        About,
        Licenses,
        Info,
        ScreensaverSettings,
        Startup,
        Confirm,
        CatalogScan,
        BackgroundScan,
        Loading,
        MediaProgress,
        ParticleScreensaver,
        ScreenshotTiles,
    }

    impl Scenario {
        fn uses_launcher_navigation(self) -> bool {
            matches!(
                self,
                Self::Home
                    | Self::SystemHub
                    | Self::Arcade
                    | Self::ArcadeSearch
                    | Self::ArcadeCrossfade
                    | Self::Settings
                    | Self::OrientationChoice
                    | Self::Controller
                    | Self::About
                    | Self::Licenses
                    | Self::Info
                    | Self::ScreensaverSettings
            )
        }

        fn from_screen(screen: Screen) -> Self {
            match screen {
                Screen::Home => Self::Home,
                Screen::SystemHub => Self::SystemHub,
                Screen::Controller => Self::Controller,
                Screen::Arcade => Self::Arcade,
                Screen::Settings => Self::Settings,
                Screen::About => Self::About,
                Screen::Licenses => Self::Licenses,
                Screen::Info => Self::Info,
                Screen::Screensaver => Self::ScreensaverSettings,
            }
        }

        fn parse(value: &str) -> Option<Self> {
            match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
                "home" => Some(Self::Home),
                "system-hub" | "snes-hub" | "snes" => Some(Self::SystemHub),
                "arcade" => Some(Self::Arcade),
                "arcade-search" | "search" => Some(Self::ArcadeSearch),
                "arcade-crossfade" | "crossfade" => Some(Self::ArcadeCrossfade),
                "settings" => Some(Self::Settings),
                "orientation-choice" | "orientation-chooser" => Some(Self::OrientationChoice),
                "controller" => Some(Self::Controller),
                "controller-setup" | "setup" => Some(Self::ControllerSetup),
                "about" => Some(Self::About),
                "licenses" => Some(Self::Licenses),
                "info" => Some(Self::Info),
                "screensaver-settings" => Some(Self::ScreensaverSettings),
                "startup" => Some(Self::Startup),
                "confirm" => Some(Self::Confirm),
                "catalog-scan" => Some(Self::CatalogScan),
                "background-scan" => Some(Self::BackgroundScan),
                "loading" => Some(Self::Loading),
                "media-progress" => Some(Self::MediaProgress),
                "particle" | "particle-screensaver" => Some(Self::ParticleScreensaver),
                "screenshot-screensaver" | "screenshot-tiles" | "tiles" => {
                    Some(Self::ScreenshotTiles)
                }
                _ => None,
            }
        }

        fn label(self) -> &'static str {
            match self {
                Self::Home => "Home",
                Self::SystemHub => "SNES System Hub",
                Self::Arcade => "Arcade",
                Self::ArcadeSearch => "Arcade Search",
                Self::ArcadeCrossfade => "Arcade Crossfade",
                Self::Settings => "Settings",
                Self::OrientationChoice => "Orientation Choice",
                Self::Controller => "Controller",
                Self::ControllerSetup => "Controller Setup",
                Self::About => "About",
                Self::Licenses => "Licenses",
                Self::Info => "Info",
                Self::ScreensaverSettings => "Screensaver Settings",
                Self::Startup => "Startup",
                Self::Confirm => "Confirmation",
                Self::CatalogScan => "Catalog Scan",
                Self::BackgroundScan => "Background Scan",
                Self::Loading => "Loading",
                Self::MediaProgress => "Media Progress",
                Self::ParticleScreensaver => "Particle Screensaver",
                Self::ScreenshotTiles => "Production Screenshot Screensaver",
            }
        }

        fn id(self) -> &'static str {
            match self {
                Self::Home => "home",
                Self::SystemHub => "system-hub",
                Self::Arcade => "arcade",
                Self::ArcadeSearch => "arcade-search",
                Self::ArcadeCrossfade => "arcade-crossfade",
                Self::Settings => "settings",
                Self::OrientationChoice => "orientation-choice",
                Self::Controller => "controller",
                Self::ControllerSetup => "controller-setup",
                Self::About => "about",
                Self::Licenses => "licenses",
                Self::Info => "info",
                Self::ScreensaverSettings => "screensaver-settings",
                Self::Startup => "startup",
                Self::Confirm => "confirm",
                Self::CatalogScan => "catalog-scan",
                Self::BackgroundScan => "background-scan",
                Self::Loading => "loading",
                Self::MediaProgress => "media-progress",
                Self::ParticleScreensaver => "particle-screensaver",
                Self::ScreenshotTiles => "screenshot-tiles",
            }
        }

        const fn deterministic_seed(self) -> Option<u64> {
            match self {
                Self::ParticleScreensaver => Some(PARTICLE_SCENE_SEED),
                Self::ScreenshotTiles => Some(SCREENSHOT_TILE_SEED),
                _ => None,
            }
        }

        fn shortcut(self) -> &'static str {
            match self {
                Self::Home => "1",
                Self::SystemHub => "headless",
                Self::Settings => "2",
                Self::OrientationChoice => "headless",
                Self::Controller => "3",
                Self::About => "4",
                Self::Licenses => "5",
                Self::Info => "6",
                Self::ScreensaverSettings => "7",
                Self::Startup => "8",
                Self::Confirm => "9",
                Self::CatalogScan => "0",
                Self::Arcade => "A",
                Self::ArcadeSearch => "headless",
                Self::ArcadeCrossfade => "headless",
                Self::BackgroundScan => "B",
                Self::Loading => "L",
                Self::MediaProgress => "M",
                Self::ParticleScreensaver => "P",
                Self::ScreenshotTiles => "T",
                Self::ControllerSetup => "S",
            }
        }
    }

    fn shortcut_scenario(code: KeyCode) -> Option<Scenario> {
        match code {
            KeyCode::Digit1 | KeyCode::Numpad1 => Some(Scenario::Home),
            KeyCode::Digit2 | KeyCode::Numpad2 => Some(Scenario::Settings),
            KeyCode::Digit3 | KeyCode::Numpad3 => Some(Scenario::Controller),
            KeyCode::Digit4 | KeyCode::Numpad4 => Some(Scenario::About),
            KeyCode::Digit5 | KeyCode::Numpad5 => Some(Scenario::Licenses),
            KeyCode::Digit6 | KeyCode::Numpad6 => Some(Scenario::Info),
            KeyCode::Digit7 | KeyCode::Numpad7 => Some(Scenario::ScreensaverSettings),
            KeyCode::Digit8 | KeyCode::Numpad8 => Some(Scenario::Startup),
            KeyCode::Digit9 | KeyCode::Numpad9 => Some(Scenario::Confirm),
            KeyCode::Digit0 | KeyCode::Numpad0 => Some(Scenario::CatalogScan),
            KeyCode::KeyA => Some(Scenario::Arcade),
            KeyCode::KeyB => Some(Scenario::BackgroundScan),
            KeyCode::KeyL => Some(Scenario::Loading),
            KeyCode::KeyM => Some(Scenario::MediaProgress),
            KeyCode::KeyP => Some(Scenario::ParticleScreensaver),
            KeyCode::KeyT => Some(Scenario::ScreenshotTiles),
            KeyCode::KeyS => Some(Scenario::ControllerSetup),
            _ => None,
        }
    }

    fn activated_scenario(
        scenario: Scenario,
        selection: usize,
        settings_focused: bool,
    ) -> Option<Scenario> {
        match (scenario, selection, settings_focused) {
            (Scenario::Home, _, true) => Some(Scenario::Settings),
            (Scenario::Home, 0, false) => Some(Scenario::Arcade),
            (Scenario::Settings, 1, _) => Some(Scenario::ScreensaverSettings),
            (Scenario::Settings, 5, _) => Some(Scenario::About),
            (Scenario::About, 0, _) => Some(Scenario::Info),
            (Scenario::About, 1, _) => Some(Scenario::Licenses),
            _ => None,
        }
    }

    fn back_scenario(scenario: Scenario) -> Option<Scenario> {
        match scenario {
            Scenario::Home => None,
            Scenario::ScreensaverSettings | Scenario::About => Some(Scenario::Settings),
            Scenario::Info | Scenario::Licenses => Some(Scenario::About),
            _ => Some(Scenario::Home),
        }
    }

    fn navigation_transition_for_intent(
        nav: &LauncherNav,
        event: &LauncherEvent,
    ) -> Option<(NavigationTransitionEdge, NavigationTransitionDirection)> {
        match event.action {
            LauncherAction::OpenMenu => Some((
                NavigationTransitionEdge::HomeToConsoles,
                NavigationTransitionDirection::Forward,
            )),
            LauncherAction::OpenCollection if nav.current_menu_id() == ROOT_MENU_ID => Some((
                NavigationTransitionEdge::HomeToArcade,
                NavigationTransitionDirection::Forward,
            )),
            LauncherAction::OpenCollection => Some((
                NavigationTransitionEdge::ConsolesToSystem,
                NavigationTransitionDirection::Forward,
            )),
            LauncherAction::NavigateBack if nav.screen == Screen::Home => Some((
                NavigationTransitionEdge::HomeToConsoles,
                NavigationTransitionDirection::Reverse,
            )),
            LauncherAction::NavigateBack
                if nav.screen == Screen::Arcade && nav.current_menu_id() == ROOT_MENU_ID =>
            {
                Some((
                    NavigationTransitionEdge::HomeToArcade,
                    NavigationTransitionDirection::Reverse,
                ))
            }
            LauncherAction::NavigateBack if nav.screen == Screen::Arcade => Some((
                NavigationTransitionEdge::ConsolesToSystem,
                NavigationTransitionDirection::Reverse,
            )),
            LauncherAction::NavigateHome if nav.screen == Screen::Home => Some((
                NavigationTransitionEdge::HomeToConsoles,
                NavigationTransitionDirection::Reverse,
            )),
            LauncherAction::NavigateHome
                if nav.screen == Screen::Arcade && nav.current_menu_id() == ROOT_MENU_ID =>
            {
                Some((
                    NavigationTransitionEdge::HomeToArcade,
                    NavigationTransitionDirection::Reverse,
                ))
            }
            LauncherAction::NavigateHome if nav.screen == Screen::Arcade => Some((
                NavigationTransitionEdge::ConsolesToSystem,
                NavigationTransitionDirection::Reverse,
            )),
            _ => None,
        }
    }

    fn settings_page_transition(
        source: Screen,
        destination: Screen,
    ) -> Option<(NavigationTransitionRoute, NavigationTransitionDirection)> {
        let source_depth = settings_page_depth(source)?;
        let destination_depth = settings_page_depth(destination)?;
        let route = match (source, destination) {
            (Screen::Home, Screen::Settings) | (Screen::Settings, Screen::Home) => {
                Some(NavigationTransitionRoute::HomeToSettings)
            }
            (Screen::Settings, Screen::Screensaver) | (Screen::Screensaver, Screen::Settings) => {
                Some(NavigationTransitionRoute::SettingsToScreensaver)
            }
            (Screen::Settings, Screen::About) | (Screen::About, Screen::Settings) => {
                Some(NavigationTransitionRoute::SettingsToAbout)
            }
            (Screen::About, Screen::Info) | (Screen::Info, Screen::About) => {
                Some(NavigationTransitionRoute::AboutToInfo)
            }
            (Screen::About, Screen::Licenses) | (Screen::Licenses, Screen::About) => {
                Some(NavigationTransitionRoute::AboutToLicenses)
            }
            (source, Screen::Home) if source != Screen::Home => {
                Some(NavigationTransitionRoute::NestedToHome)
            }
            _ => None,
        }?;
        let adjacent = matches!(
            (source, destination),
            (Screen::Home, Screen::Settings)
                | (Screen::Settings, Screen::Home)
                | (Screen::Settings, Screen::Screensaver | Screen::About)
                | (Screen::Screensaver | Screen::About, Screen::Settings)
                | (Screen::About, Screen::Info | Screen::Licenses)
                | (Screen::Info | Screen::Licenses, Screen::About)
        );
        let direct_home = source != Screen::Home && destination == Screen::Home;
        (adjacent || direct_home).then_some((
            route,
            if destination_depth > source_depth {
                NavigationTransitionDirection::Forward
            } else {
                NavigationTransitionDirection::Reverse
            },
        ))
    }

    const fn settings_page_depth(screen: Screen) -> Option<u8> {
        match screen {
            Screen::Home => Some(0),
            Screen::Settings => Some(1),
            Screen::Screensaver | Screen::About => Some(2),
            Screen::Info | Screen::Licenses => Some(3),
            Screen::Controller | Screen::Arcade | Screen::SystemHub => None,
        }
    }

    fn run_scene_matrix(output_dir: &Path) -> Result<(), Box<dyn Error>> {
        std::fs::create_dir(output_dir)?;
        let executable = std::env::current_exe()?;
        let manifest = launcher_scene_manifest()?;
        for scene in &manifest.scenes {
            let output = Command::new(&executable)
                .args(scene_arguments(scene, output_dir))
                .output()?;
            if !output.status.success() {
                return Err(format!(
                    "launcher scene {:?} failed: {}",
                    scene.id,
                    String::from_utf8_lossy(&output.stderr).trim()
                )
                .into());
            }
            let provenance_path = output_dir.join(format!("{}.json", scene.id));
            let provenance: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&provenance_path)?)?;
            let (expected_width, expected_height) = match scene.profile {
                SceneProfile::Hdmi => (HDMI_FRAME_WIDTH as u64, HDMI_FRAME_HEIGHT as u64),
                // CRT-240p renders a 640x480 logical surface before the
                // output plan expands it to the physical 640x240 mode.
                SceneProfile::Crt240p => (640, 480),
                SceneProfile::Crt480p => (640, 480),
            };
            if provenance.get("width").and_then(serde_json::Value::as_u64) != Some(expected_width)
                || provenance.get("height").and_then(serde_json::Value::as_u64)
                    != Some(expected_height)
            {
                return Err(
                    format!("launcher scene {:?} produced unexpected geometry", scene.id).into(),
                );
            }
            let image_path = output_dir.join(format!("{}.png", scene.id));
            if !image_path.is_file() {
                return Err(format!("launcher scene {:?} produced no PNG", scene.id).into());
            }
            let rgb565_hash = provenance
                .get("rgb565_hash")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("launcher scene {:?} has no RGB565 hash", scene.id))?;
            println!("scene={} status=passed hash={rgb565_hash}", scene.id);
        }
        println!("matrix=passed scenes={}", manifest.scenes.len());
        Ok(())
    }

    fn run_baseline_check(expected_dir: &Path) -> Result<(), Box<dyn Error>> {
        let check_root =
            PathBuf::from("outputs").join(format!("launcher-visual-check-{}", std::process::id()));
        let actual_dir = check_root.join("actual");
        let mismatch_dir = check_root.join("mismatch");
        if check_root.exists() {
            return Err(format!(
                "launcher visual check directory already exists: {}",
                check_root.display()
            )
            .into());
        }
        std::fs::create_dir_all(&check_root)?;
        let result = run_scene_matrix(&actual_dir).and_then(|()| {
            let scene_ids = launcher_scene_manifest()?
                .scenes
                .into_iter()
                .map(|scene| scene.id)
                .collect::<Vec<_>>();
            compare_launcher_matrix(expected_dir, &actual_dir, &mismatch_dir, &scene_ids)?;
            println!("comparison=passed scenes={}", scene_ids.len());
            Ok(())
        });
        if result.is_ok() {
            std::fs::remove_dir_all(&check_root)?;
        } else {
            eprintln!("visual_artifacts={}", check_root.display());
        }
        result
    }

    fn scene_arguments(scene: &LauncherScene, output_dir: &Path) -> Vec<std::ffi::OsString> {
        let scenario = match scene.scenario {
            SceneScenario::Home | SceneScenario::NavigationTransitionMidpoint => "home",
            SceneScenario::Arcade => "arcade",
            SceneScenario::Settings => "settings",
            SceneScenario::ControllerSetup => "controller-setup",
            SceneScenario::CatalogScan => "catalog-scan",
        };
        let profile = match scene.profile {
            SceneProfile::Hdmi => "hdmi",
            SceneProfile::Crt240p => "crt-240p",
            SceneProfile::Crt480p => "crt-480p",
        };
        let mut arguments = [
            "--content",
            "fixtures",
            "--no-scan",
            "--no-download",
            "--scenario",
            scenario,
            "--display-profile",
            profile,
            "--orientation",
            "normal",
            "--refresh-rate",
            "60",
            "--frame",
        ]
        .map(std::ffi::OsString::from)
        .to_vec();
        arguments.push(scene.frame.to_string().into());
        arguments.push("--output".into());
        arguments.push(
            output_dir
                .join(format!("{}.png", scene.id))
                .into_os_string(),
        );
        arguments.push("--provenance-output".into());
        arguments.push(
            output_dir
                .join(format!("{}.json", scene.id))
                .into_os_string(),
        );
        if let Some(transition) = &scene.transition {
            match transition.edge {
                SceneTransitionEdge::HomeArcade => {
                    arguments.push("--navigation-transition-demo".into());
                    arguments.push("home-arcade".into());
                }
            }
            arguments.push("--navigation-transition-duration-ms".into());
            arguments.push(transition.duration_ms.to_string().into());
        }
        arguments
    }

    struct PreviewOptions {
        scenario: Scenario,
        frame: u64,
        output: Option<PathBuf>,
        provenance_output: Option<PathBuf>,
        refresh_rate: RefreshRate,
        content_mode: ContentMode,
        sd_root: Option<PathBuf>,
        cache_root: Option<PathBuf>,
        no_scan: bool,
        no_download: bool,
        display_profile: DisplayProfile,
        orientation: ScreenOrientation,
        navigation_transition_demo: Option<NavigationTransitionEdge>,
        settings_page_transition_demo: bool,
        navigation_transition_demo_reverse: bool,
        navigation_transition_duration_ms: Option<u64>,
        list_scenes: bool,
        matrix_output: Option<PathBuf>,
        expected_matrix: Option<PathBuf>,
        mismatch_output: Option<PathBuf>,
        check_baselines: Option<PathBuf>,
    }

    impl PreviewOptions {
        fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
            let mut scenario = Scenario::Home;
            let mut frame = 0;
            let mut output = None;
            let mut provenance_output = None;
            let mut refresh_rate = RefreshRate::Auto;
            let mut content_mode = ContentMode::Auto;
            let mut sd_root = None;
            let mut cache_root = None;
            let mut no_scan = false;
            let mut no_download = false;
            let mut display_profile = DisplayProfile::Hdmi;
            let mut orientation = ScreenOrientation::Normal;
            let mut navigation_transition_demo = None;
            let mut settings_page_transition_demo = false;
            let mut navigation_transition_demo_reverse = false;
            let mut navigation_transition_duration_ms = None;
            let mut list_scenes = false;
            let mut matrix_output = None;
            let mut expected_matrix = None;
            let mut mismatch_output = None;
            let mut check_baselines = None;
            let mut arguments = arguments.into_iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "--scenario" => {
                        let value = arguments
                            .next()
                            .ok_or("--scenario requires a scenario name")?;
                        scenario = Scenario::parse(&value)
                            .ok_or_else(|| format!("unknown preview scenario {value:?}"))?;
                    }
                    "--frame" => {
                        let value = arguments.next().ok_or("--frame requires a frame number")?;
                        frame = value
                            .parse::<u64>()
                            .map_err(|_| format!("invalid frame number {value:?}"))?;
                    }
                    "--output" => {
                        let value = arguments.next().ok_or("--output requires a file path")?;
                        output = Some(PathBuf::from(value));
                    }
                    "--provenance-output" => {
                        let value = arguments
                            .next()
                            .ok_or("--provenance-output requires a file path")?;
                        provenance_output = Some(PathBuf::from(value));
                    }
                    "--refresh-rate" => {
                        let value = arguments
                            .next()
                            .ok_or("--refresh-rate requires auto, 60, or 120")?;
                        refresh_rate = RefreshRate::parse(&value)?;
                    }
                    "--content" => {
                        let value = arguments
                            .next()
                            .ok_or("--content requires auto, fixtures, or card")?;
                        content_mode = ContentMode::parse(&value)?;
                    }
                    "--sd-root" => {
                        sd_root = Some(PathBuf::from(
                            arguments.next().ok_or("--sd-root requires a path")?,
                        ));
                    }
                    "--cache-root" => {
                        cache_root = Some(PathBuf::from(
                            arguments.next().ok_or("--cache-root requires a path")?,
                        ));
                    }
                    "--no-scan" => no_scan = true,
                    "--no-download" => no_download = true,
                    "--navigation-transition-duration-ms" => {
                        let value = arguments
                            .next()
                            .ok_or("--navigation-transition-duration-ms requires milliseconds")?;
                        let duration = value
                            .parse::<u64>()
                            .map_err(|_| format!("invalid transition duration {value:?}"))?;
                        if !(100..=10_000).contains(&duration) {
                            return Err(
                                "--navigation-transition-duration-ms must be 100..=10000".into()
                            );
                        }
                        navigation_transition_duration_ms = Some(duration);
                    }
                    "--navigation-transition-demo" => {
                        let value = arguments
                            .next()
                            .ok_or("--navigation-transition-demo requires an edge")?;
                        navigation_transition_demo =
                            Some(NavigationTransitionEdge::parse(&value).ok_or_else(|| {
                                format!(
                                    "invalid navigation transition edge {value:?}; expected home-consoles, home-arcade, or consoles-system"
                                )
                            })?);
                    }
                    "--settings-page-transition-demo" => {
                        settings_page_transition_demo = true;
                    }
                    "--navigation-transition-demo-reverse" => {
                        navigation_transition_demo_reverse = true;
                    }
                    "--list-scenes" => list_scenes = true,
                    "--matrix-output" => {
                        matrix_output = Some(PathBuf::from(
                            arguments
                                .next()
                                .ok_or("--matrix-output requires a directory")?,
                        ));
                    }
                    "--expected-matrix" => {
                        expected_matrix = Some(PathBuf::from(
                            arguments
                                .next()
                                .ok_or("--expected-matrix requires a directory")?,
                        ));
                    }
                    "--mismatch-output" => {
                        mismatch_output = Some(PathBuf::from(
                            arguments
                                .next()
                                .ok_or("--mismatch-output requires a directory")?,
                        ));
                    }
                    "--check-baselines" => {
                        check_baselines = Some(PathBuf::from(
                            arguments
                                .next()
                                .ok_or("--check-baselines requires a directory")?,
                        ));
                    }
                    "--display-profile" => {
                        let value = arguments
                            .next()
                            .ok_or(
                                "--display-profile requires hdmi, crt-240p, crt-288p, crt-480p, or crt-576p",
                            )?;
                        display_profile = DisplayProfile::parse(&value)?;
                    }
                    "--orientation" => {
                        let value = arguments.next().ok_or(
                            "--orientation requires normal, monitor-clockwise, or monitor-counterclockwise",
                        )?;
                        orientation = ScreenOrientation::parse(&value).ok_or_else(|| {
                            format!(
                                "invalid orientation {value:?}; expected normal, monitor-clockwise, or monitor-counterclockwise"
                            )
                        })?;
                    }
                    "--help" | "-h" => {
                        return Err(
                            "usage: mister-magik-ui-preview [--list-scenes] [--check-baselines DIR | --matrix-output DIR [--expected-matrix DIR --mismatch-output DIR]] [--content auto|fixtures|card] [--sd-root PATH] [--cache-root PATH] [--no-scan] [--no-download] [--navigation-transition-duration-ms 100..10000] [--navigation-transition-demo home-consoles|home-arcade|consoles-system] [--settings-page-transition-demo] [--navigation-transition-demo-reverse] [--display-profile hdmi|crt-240p|crt-288p|crt-480p|crt-576p] [--orientation normal|monitor-clockwise|monitor-counterclockwise] [--scenario NAME] [--refresh-rate auto|60|120] [--frame N] [--output FILE.ppm|FILE.png] [--provenance-output FILE.json]"
                                .into(),
                        );
                    }
                    other => return Err(format!("unknown preview argument {other:?}")),
                }
            }
            if frame > 0 && output.is_none() {
                return Err("--frame requires --output".into());
            }
            if provenance_output.is_some() && output.is_none() {
                return Err("--provenance-output requires --output".into());
            }
            if provenance_output == output && output.is_some() {
                return Err("--provenance-output must differ from --output".into());
            }
            if expected_matrix.is_some() != mismatch_output.is_some() {
                return Err(
                    "--expected-matrix and --mismatch-output must be provided together".into(),
                );
            }
            if expected_matrix.is_some() && matrix_output.is_none() {
                return Err("--expected-matrix requires --matrix-output".into());
            }
            if check_baselines.is_some()
                && (matrix_output.is_some()
                    || output.is_some()
                    || provenance_output.is_some()
                    || list_scenes)
            {
                return Err(
                    "--check-baselines is a read-only command and cannot be combined with output modes"
                        .into(),
                );
            }
            if navigation_transition_demo_reverse
                && navigation_transition_demo.is_none()
                && !settings_page_transition_demo
            {
                return Err(
                    "--navigation-transition-demo-reverse requires a navigation transition demo"
                        .into(),
                );
            }
            Ok(Self {
                scenario,
                frame,
                output,
                provenance_output,
                refresh_rate,
                content_mode,
                sd_root,
                cache_root,
                no_scan,
                no_download,
                display_profile,
                orientation,
                navigation_transition_demo,
                settings_page_transition_demo,
                navigation_transition_demo_reverse,
                navigation_transition_duration_ms,
                list_scenes,
                matrix_output,
                expected_matrix,
                mismatch_output,
                check_baselines,
            })
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    enum DisplayProfile {
        #[default]
        Hdmi,
        Crt240p,
        Crt288p,
        Crt480p,
        Crt576p,
    }

    impl DisplayProfile {
        fn parse(value: &str) -> Result<Self, String> {
            match value.trim().to_ascii_lowercase().as_str() {
                "hdmi" => Ok(Self::Hdmi),
                "crt-240p" | "crt-240p60" => Ok(Self::Crt240p),
                "crt-288p" | "crt-288p50" => Ok(Self::Crt288p),
                "crt" | "crt-480p" | "crt-480p60" => Ok(Self::Crt480p),
                "crt-576p" | "crt-576p50" => Ok(Self::Crt576p),
                _ => Err(format!(
                    "invalid display profile {value:?}; expected hdmi, crt-240p, crt-288p, crt-480p, or crt-576p"
                )),
            }
        }

        const fn is_crt(self) -> bool {
            !matches!(self, Self::Hdmi)
        }

        const fn route(self) -> ResolvedOutputRoute {
            match self {
                Self::Hdmi => ResolvedOutputRoute::Hdmi,
                Self::Crt240p => ResolvedOutputRoute::Crt240p60,
                Self::Crt288p => ResolvedOutputRoute::Crt288p50,
                Self::Crt480p => ResolvedOutputRoute::Crt480p60,
                Self::Crt576p => ResolvedOutputRoute::Crt576p50,
            }
        }

        const fn framebuffer_size(self) -> (usize, usize) {
            match self {
                Self::Hdmi => (HDMI_FRAME_WIDTH, HDMI_FRAME_HEIGHT),
                Self::Crt240p => (640, 240),
                Self::Crt288p => (640, 288),
                Self::Crt480p => (640, 480),
                Self::Crt576p => (640, 576),
            }
        }

        const fn render_size(self) -> (usize, usize) {
            match self {
                Self::Crt240p => (640, 480),
                _ => self.framebuffer_size(),
            }
        }

        fn display(self) -> UiDisplay {
            let (fb_w, fb_h) = self.framebuffer_size();
            let (render_w, render_h) = self.render_size();
            UiDisplay::for_plan(UiDisplayPlan {
                fb_w,
                fb_h,
                render_w,
                render_h,
                output_w: fb_w as u16,
                output_h: fb_h as u16,
                scan_w: fb_w as u16,
                scan_h: fb_h as u16,
                direct_video: self.is_crt(),
                output_route: self.route(),
                fb_policy: UiFramebufferSizePolicy::Auto,
                source: "ui-preview-display-profile",
                fallback: false,
            })
        }

        fn display_resolution_index(self) -> usize {
            let id = match self {
                Self::Hdmi => "hdmi-1920x1080p60",
                Self::Crt240p => "crt-240p60",
                Self::Crt288p => "crt-288p50",
                Self::Crt480p => "crt-480p60",
                Self::Crt576p => "crt-576p50",
            };
            mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
                .iter()
                .position(|mode| mode.id == id)
                .expect("preview display profile has a launcher resolution")
        }

        fn settings_display_resolution_index(self) -> Option<usize> {
            let id = mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
                [self.display_resolution_index()]
            .id;
            settings_display_resolution_index(id)
        }

        const fn label(self) -> &'static str {
            match self {
                Self::Hdmi => "display:hdmi",
                Self::Crt240p => "display:crt-240p",
                Self::Crt288p => "display:crt-288p",
                Self::Crt480p => "display:crt-480p",
                Self::Crt576p => "display:crt-576p",
            }
        }

        const fn id(self) -> &'static str {
            match self {
                Self::Hdmi => "hdmi",
                Self::Crt240p => "crt-240p",
                Self::Crt288p => "crt-288p",
                Self::Crt480p => "crt-480p",
                Self::Crt576p => "crt-576p",
            }
        }
    }

    fn configure_display_profile(
        ui: &MisterUi,
        profile: DisplayProfile,
        orientation: ScreenOrientation,
    ) {
        let display = profile.display();
        let layout = UiLayoutGeometry::for_display(&display, orientation);
        ui.set_window_width(layout.logical_w() as i32);
        ui.set_window_height(layout.logical_h() as i32);
        ui.set_crt_layout(profile.is_crt());
        ui.set_screen_orientation(match orientation {
            ScreenOrientation::Normal => 0,
            ScreenOrientation::MonitorClockwise => 1,
            ScreenOrientation::MonitorCounterclockwise => 2,
        });
        if !profile.is_crt() {
            return;
        }
        let metrics = CrtUiMetrics::for_display(&display);
        let content = layout.content_rect();
        ui.set_crt_grid_x(metrics.grid_x);
        ui.set_crt_grid_y(metrics.grid_y);
        ui.set_crt_border_x(metrics.border_x);
        ui.set_crt_border_y(metrics.border_y);
        let crt_text_size = |size| match size {
            UiPixelSize::Px8 => mister_magik_ui::launcher::Terminus8x14Size::Px8,
            UiPixelSize::Px16 => mister_magik_ui::launcher::Terminus8x14Size::Px16,
            UiPixelSize::Px24 => mister_magik_ui::launcher::Terminus8x14Size::Px24,
            UiPixelSize::Px32 => mister_magik_ui::launcher::Terminus8x14Size::Px32,
        };
        ui.set_crt_body_font(crt_text_size(metrics.body_font));
        ui.set_crt_heading_font(crt_text_size(metrics.heading_font));
        ui.set_crt_card_title_font(crt_text_size(metrics.card_title_font));
        ui.set_crt_card_detail_font(crt_text_size(metrics.card_detail_font));
        ui.set_crt_header_height(metrics.header_height);
        ui.set_crt_footer_height(metrics.footer_height);
        ui.set_crt_game_row_height(metrics.game_row_height);
        ui.set_crt_content_x(content.x as i32);
        ui.set_crt_content_y(content.y as i32);
        ui.set_crt_content_width(content.width as i32);
        ui.set_crt_content_height(content.height as i32);
    }

    enum CatalogWorkerEvent {
        Progress { title: String, detail: String },
        Ready(PortableCatalogBuild),
        Failed(String),
    }

    fn spawn_catalog_worker(
        layout: &mister_magik_fb::macos_preview_content::HostContentLayout,
    ) -> Result<mpsc::Receiver<CatalogWorkerEvent>, String> {
        let layout = layout.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("mac-catalog-scan".into())
            .spawn(move || {
                let source_roots = mister_magik_catalog::catalog_config::DEFAULT_ROOTS
                    .iter()
                    .filter_map(|root| layout.to_card_path(root).ok())
                    .filter(|root| root.is_dir())
                    .collect::<Vec<_>>();
                let progress_sender = sender.clone();
                let mut progress = move |title: &str, detail: &str| {
                    let _ = progress_sender.send(CatalogWorkerEvent::Progress {
                        title: title.to_owned(),
                        detail: detail.to_owned(),
                    });
                };
                let result = publish_portable_catalog(
                    source_roots,
                    &layout.card_root,
                    Path::new("/media/fat"),
                    Path::new("/media/fat/_Arcade"),
                    &layout.catalog_root,
                    &mut progress,
                );
                let event = match result {
                    Ok(build) => CatalogWorkerEvent::Ready(build),
                    Err(error) => CatalogWorkerEvent::Failed(error),
                };
                let _ = sender.send(event);
            })
            .map_err(|error| format!("start Mac catalog scanner: {error}"))?;
        Ok(receiver)
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    enum RefreshRate {
        #[default]
        Auto,
        Hz60,
        Hz120,
    }

    impl RefreshRate {
        fn parse(value: &str) -> Result<Self, String> {
            match value.trim().to_ascii_lowercase().as_str() {
                "auto" => Ok(Self::Auto),
                "60" => Ok(Self::Hz60),
                "120" => Ok(Self::Hz120),
                _ => Err(format!(
                    "invalid refresh rate {value:?}; expected auto, 60, or 120"
                )),
            }
        }

        fn headless_hz(self) -> u32 {
            match self {
                Self::Auto | Self::Hz60 => 60,
                Self::Hz120 => 120,
            }
        }
    }

    fn elapsed_for_frame(frame: u64, refresh_hz: u32) -> Duration {
        let nanos = u128::from(frame)
            .saturating_mul(1_000_000_000)
            .checked_div(u128::from(refresh_hz.max(1)))
            .unwrap_or(0)
            .min(u128::from(u64::MAX));
        Duration::from_nanos(nanos as u64)
    }

    #[derive(Clone, Debug, serde::Serialize)]
    struct CaptureProvenance {
        schema: &'static str,
        scenario: &'static str,
        frame: u64,
        width: u32,
        height: u32,
        orientation: &'static str,
        display_profile: &'static str,
        refresh_hz: u32,
        fixed_time_us: u64,
        scene_seed: Option<String>,
        rgb565_hash: String,
        rgb565_conversion: &'static str,
        renderer: &'static str,
        slint_version: &'static str,
        png_encoder: &'static str,
        fixture_sha256: String,
        asset_bundle_sha256: String,
        font_bundle_sha256: String,
    }

    impl CaptureProvenance {
        fn for_capture(
            options: &PreviewOptions,
            width: usize,
            height: usize,
            refresh_hz: u32,
            fixed_time: Duration,
            rgb565_hash: u64,
        ) -> Self {
            Self {
                schema: CAPTURE_PROVENANCE_SCHEMA,
                scenario: options.scenario.id(),
                frame: options.frame,
                width: u32::try_from(width).expect("preview width fits provenance format"),
                height: u32::try_from(height).expect("preview height fits provenance format"),
                orientation: orientation_id(options.orientation),
                display_profile: options.display_profile.id(),
                refresh_hz,
                fixed_time_us: fixed_time.as_micros().min(u128::from(u64::MAX)) as u64,
                scene_seed: options
                    .scenario
                    .deterministic_seed()
                    .map(|seed| format!("{seed:016x}")),
                rgb565_hash: format!("{rgb565_hash:016x}"),
                rgb565_conversion: RGB565_CONVERSION_VERSION,
                renderer: PREVIEW_RENDERER_ID,
                slint_version: PINNED_SLINT_VERSION,
                png_encoder: PNG_ENCODER_ID,
                fixture_sha256: bundle_sha256(&[(
                    "ui_preview_fixtures.rs",
                    include_bytes!("../ui_preview_fixtures.rs"),
                )]),
                asset_bundle_sha256: bundle_sha256(&[
                    (
                        "snes-small-v1.rgb565a",
                        include_bytes!("../../assets/snes/snes-small-v1.rgb565a"),
                    ),
                    (
                        "jersey25-41px.mmbf",
                        include_bytes!("../../assets/fonts/jersey25-41px.mmbf"),
                    ),
                ]),
                font_bundle_sha256: bundle_sha256(&[(
                    "PressStart2P-Regular.ttf",
                    include_bytes!("../../ui/fonts/PressStart2P-Regular.ttf"),
                )]),
            }
        }

        fn encoded(&self) -> Result<Vec<u8>, serde_json::Error> {
            let mut encoded = serde_json::to_vec_pretty(self)?;
            encoded.push(b'\n');
            Ok(encoded)
        }

        fn identity(&self) -> Result<String, serde_json::Error> {
            Ok(sha256_hex(&self.encoded()?))
        }
    }

    const fn orientation_id(orientation: ScreenOrientation) -> &'static str {
        match orientation {
            ScreenOrientation::Normal => "normal",
            ScreenOrientation::MonitorClockwise => "monitor-clockwise",
            ScreenOrientation::MonitorCounterclockwise => "monitor-counterclockwise",
        }
    }

    fn write_capture_provenance(
        path: &Path,
        provenance: &CaptureProvenance,
    ) -> Result<(), Box<dyn Error>> {
        let encoded = provenance.encoded()?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(&encoded)?;
        Ok(())
    }

    fn bundle_sha256(entries: &[(&str, &[u8])]) -> String {
        let mut hasher = Sha256::new();
        for (name, bytes) in entries {
            hasher.update((name.len() as u64).to_be_bytes());
            hasher.update(name.as_bytes());
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
        hex_bytes(&hasher.finalize())
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex_bytes(&Sha256::digest(bytes))
    }

    fn hex_bytes(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
        for &byte in bytes {
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        encoded
    }

    fn write_capture(
        path: &Path,
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
    ) -> Result<(), Box<dyn Error>> {
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        {
            write_png(path, pixels, width, height)
        } else {
            write_ppm(path, pixels, width, height)
        }
    }

    fn write_png(
        path: &Path,
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
    ) -> Result<(), Box<dyn Error>> {
        let encoded = encode_png(pixels, width, height)?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(&encoded)?;
        Ok(())
    }

    fn encode_png(
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let pixel_count = width
            .checked_mul(height)
            .ok_or("capture dimensions overflow")?;
        if pixels.len() != pixel_count {
            return Err("capture pixel count does not match dimensions".into());
        }
        if width == 0 || height == 0 {
            return Err("PNG capture dimensions must be non-zero".into());
        }
        let png_width = u32::try_from(width).map_err(|_| "capture width exceeds PNG limits")?;
        let png_height = u32::try_from(height).map_err(|_| "capture height exceeds PNG limits")?;
        let row_len = width
            .checked_mul(3)
            .and_then(|len| len.checked_add(1))
            .ok_or("PNG scanline length overflow")?;
        let raw_len = row_len
            .checked_mul(height)
            .ok_or("PNG capture length overflow")?;
        let mut raw = Vec::with_capacity(raw_len);
        for row in pixels.chunks_exact(width) {
            raw.push(0);
            for &pixel in row {
                let color = rgb565_to_xrgb8888(pixel);
                raw.extend_from_slice(&[
                    ((color >> 16) & 0xff) as u8,
                    ((color >> 8) & 0xff) as u8,
                    (color & 0xff) as u8,
                ]);
            }
        }

        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&raw)?;
        let compressed = encoder.finish()?;

        let mut encoded = Vec::with_capacity(compressed.len().saturating_add(57));
        encoded.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut header = [0u8; 13];
        header[..4].copy_from_slice(&png_width.to_be_bytes());
        header[4..8].copy_from_slice(&png_height.to_be_bytes());
        header[8] = 8;
        header[9] = 2;
        append_png_chunk(&mut encoded, *b"IHDR", &header)?;
        append_png_chunk(&mut encoded, *b"IDAT", &compressed)?;
        append_png_chunk(&mut encoded, *b"IEND", &[])?;
        Ok(encoded)
    }

    fn append_png_chunk(
        encoded: &mut Vec<u8>,
        kind: [u8; 4],
        data: &[u8],
    ) -> Result<(), Box<dyn Error>> {
        let length = u32::try_from(data.len()).map_err(|_| "PNG chunk exceeds format limits")?;
        encoded.extend_from_slice(&length.to_be_bytes());
        encoded.extend_from_slice(&kind);
        encoded.extend_from_slice(data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(&kind);
        crc.update(data);
        encoded.extend_from_slice(&crc.finalize().to_be_bytes());
        Ok(())
    }

    fn write_ppm(
        path: &Path,
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
    ) -> Result<(), Box<dyn Error>> {
        if pixels.len() != width.saturating_mul(height) {
            return Err("capture pixel count does not match dimensions".into());
        }
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        write!(file, "P6\n{width} {height}\n255\n")?;
        let mut row = Vec::with_capacity(width * 3);
        for pixels in pixels.chunks_exact(width) {
            row.clear();
            for &pixel in pixels {
                let color = rgb565_to_xrgb8888(pixel);
                row.extend_from_slice(&[
                    ((color >> 16) & 0xff) as u8,
                    ((color >> 8) & 0xff) as u8,
                    (color & 0xff) as u8,
                ]);
            }
            file.write_all(&row)?;
        }
        Ok(())
    }

    fn frame_difference(
        first: &[Rgb565Pixel],
        second: &[Rgb565Pixel],
        width: usize,
        height: usize,
    ) -> Option<(usize, usize, usize, usize, usize)> {
        if first.len() != width.saturating_mul(height) || second.len() != first.len() {
            return Some((usize::MAX, 0, 0, width, height));
        }
        let mut difference = None;
        for (offset, pair) in first.iter().zip(second).enumerate() {
            let x = offset % width.max(1);
            let y = offset / width.max(1);
            if pair.0 != pair.1 {
                let (count, min_x, min_y, max_x, max_y) = difference.get_or_insert((0, x, y, x, y));
                *count += 1;
                *min_x = (*min_x).min(x);
                *min_y = (*min_y).min(y);
                *max_x = (*max_x).max(x);
                *max_y = (*max_y).max(y);
            }
        }
        difference
    }

    fn frame_hash(pixels: &[Rgb565Pixel]) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for pixel in pixels {
            for byte in pixel.0.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
        hash
    }

    fn oriented_capture(logical: &[Rgb565Pixel], layout: UiLayoutGeometry) -> Vec<Rgb565Pixel> {
        if !layout.is_portrait() {
            return logical.to_vec();
        }
        let mut composition = vec![Rgb565Pixel(0); layout.output_layout().len()];
        let mut surface = Rgb565SurfaceMut::new(&mut composition, layout.output_layout())
            .expect("preview output layout matches its capture buffer");
        assert!(surface.copy_rect_strided(
            0,
            0,
            layout.logical_w(),
            layout.logical_h(),
            logical,
            layout.logical_w(),
            0,
            0,
        ));
        composition
    }

    fn initialize_bridge(bridge: &MisterBridge, display_profile: DisplayProfile) {
        bridge.set_clock_text("12:34".into());
        bridge.set_build_label("Mac visual preview".into());
        bridge.set_present_mode_label("RGB565 host composition".into());
        bridge.set_capture_available(true);
        bridge.set_device_label("Controller 1".into());
        bridge.set_device_name("Fixture Gamepad".into());
        bridge.set_usb_port("USB 1-2".into());
        bridge.set_usb_id("045e:028e".into());
        bridge.set_serial_id("PREVIEW-0001".into());
        bridge.set_js_counts("16 buttons · 6 axes".into());
        bridge.set_pressed_now("A · D-pad Right".into());
        bridge.set_last_event_label("Button A pressed".into());
        bridge.set_last_raw_event("type=1 code=304 value=1".into());
        let display_resolutions = settings_display_resolutions().collect::<Vec<_>>();
        bridge.set_display_options(ModelRc::new(VecModel::from(
            display_resolutions
                .iter()
                .map(|mode| SharedString::from(mode.label))
                .collect::<Vec<_>>(),
        )));
        let active_resolution =
            mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS
                [display_profile.display_resolution_index()];
        let display_selected = display_profile.settings_display_resolution_index();
        bridge.set_display_active_label(active_resolution.label.into());
        bridge.set_display_selected(display_selected.map_or(-1, |index| index as i32));
        bridge.set_display_highlighted(display_selected.map_or(0, |index| index as i32));
        bridge.set_orientation_options(ModelRc::new(VecModel::from(
            ScreenOrientation::ALL
                .iter()
                .map(|orientation| SharedString::from(orientation.label()))
                .collect::<Vec<_>>(),
        )));
        bridge.set_arcade_search_keys(strings(&[
            "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q",
            "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
        ]));
        bridge.set_license_lines(strings(&[
            "MiSTer MagiK",
            "",
            "GNU General Public License, version 3",
            "",
            "Slint 1.17.1",
            "FFmpeg 8.1",
            "Rust third-party license inventory",
        ]));
    }

    fn apply_scenario(launcher: &Launcher, scenario: Scenario) {
        let bridge = launcher.global::<MisterBridge>();
        reset_transient_bridge(&bridge);
        bridge.set_screen_mode(match scenario {
            Scenario::Controller | Scenario::ControllerSetup => 1,
            Scenario::SystemHub => 8,
            Scenario::Arcade | Scenario::ArcadeSearch | Scenario::ArcadeCrossfade => 2,
            Scenario::Settings | Scenario::OrientationChoice => 3,
            Scenario::About => 4,
            Scenario::Licenses => 5,
            Scenario::Info => 6,
            Scenario::ScreensaverSettings => 7,
            _ => 0,
        });
        bridge.set_effective_view(
            match scenario {
                Scenario::Arcade | Scenario::ArcadeSearch | Scenario::ArcadeCrossfade => "arcade",
                Scenario::SystemHub => "system-hub",
                Scenario::Settings | Scenario::OrientationChoice => "settings",
                Scenario::Controller | Scenario::ControllerSetup => "controller",
                Scenario::About => "about",
                Scenario::Licenses => "licenses",
                Scenario::Info => "info",
                Scenario::ScreensaverSettings => "screensaver-settings",
                Scenario::ParticleScreensaver | Scenario::ScreenshotTiles => "screensaver",
                _ => "home",
            }
            .into(),
        );
        bridge.set_menu_title("MiSTer MagiK".into());
        bridge.set_menu_breadcrumb("Systems".into());
        bridge.set_dev_mode(true);
        if !scenario.uses_launcher_navigation() {
            let menu_items = home_menu_items();
            bridge.set_menu_item_presentation(home_menu_presentation(menu_items.row_count()));
            bridge.set_menu_items(menu_items);
        }
        bridge.set_selected_index(0);
        bridge.set_settings_focused(false);
        bridge.set_settings_selected(0);
        bridge.set_about_selected(0);
        bridge.set_licenses_selected(0);
        bridge.set_screensaver_settings_selected(0);
        bridge.set_screensaver_enabled(true);
        bridge.set_screensaver_delay_minutes(5);
        bridge.set_simple_joystick_handling(true);
        bridge.set_reduce_motion(false);
        bridge.set_info_kernel_version("Linux 6.6.68-MiSTer".into());
        bridge.set_info_database_build("1,284 ms · 12,846 games".into());
        bridge.set_system_hub_selected(0);
        bridge.set_system_hub_games_count(1_482);
        bridge.set_system_hub_recent_count(12);
        bridge.set_system_hub_favourites_count(28);

        match scenario {
            Scenario::Startup => bridge.set_startup_visible(true),
            Scenario::Confirm => {
                bridge.set_confirm_visible(true);
                bridge.set_confirm_title("Rebuild Database?".into());
                bridge.set_confirm_message(
                    "Rebuild all library systems in the background? Games and screenshots remain available."
                        .into(),
                );
                bridge.set_confirm_left_label("Cancel".into());
                bridge.set_confirm_right_label("Rebuild".into());
            }
            Scenario::CatalogScan => {
                bridge.set_catalog_scan_visible(true);
                bridge.set_catalog_scan_title("Building game library".into());
                bridge.set_catalog_scan_message("Scanning Arcade".into());
                bridge.set_catalog_scan_detail("4,812 of 12,846 games".into());
                bridge.set_catalog_scan_percent(37);
            }
            Scenario::BackgroundScan => bridge.set_catalog_background_scan_visible(true),
            Scenario::Loading => {
                bridge.set_loading_message("Launching Out Run".into());
                bridge.set_loading_detail("Preparing core handoff".into());
            }
            Scenario::MediaProgress => {
                bridge.set_media_pack_summary("Downloading screenshot packs".into());
                bridge.set_media_pack_progresses(ModelRc::new(VecModel::from(vec![
                    ScreenshotPackProgress {
                        system: "Arcade".into(),
                        image_size: "320px".into(),
                        phase: "Downloading".into(),
                        percent: 68,
                        bytes_label: "42 MB / 61 MB".into(),
                        pack_position: "2 of 4".into(),
                    },
                    ScreenshotPackProgress {
                        system: "Neo Geo".into(),
                        image_size: "320px".into(),
                        phase: "Queued".into(),
                        percent: 0,
                        bytes_label: "0 MB / 18 MB".into(),
                        pack_position: "3 of 4".into(),
                    },
                ])));
            }
            Scenario::ControllerSetup => {
                bridge.set_setup_visible(true);
                bridge.set_setup_phase(4);
                bridge.set_setup_title("Configure Fixture Gamepad".into());
                bridge.set_setup_subtitle("Press the requested control".into());
                bridge.set_setup_list(strings(&["D-pad", "Buttons", "Shoulders", "System"]));
                bridge.set_setup_selected(1);
                bridge.set_setup_config_labels(strings(&["A", "B", "X", "Y", "Start"]));
                bridge.set_setup_config_values(strings(&["Button 0", "Button 1", "—", "—", "—"]));
                bridge.set_setup_name("Fixture Gamepad".into());
                bridge.set_setup_kind_label("Standard controller".into());
            }
            _ => {}
        }
        launcher.window().request_redraw();
    }

    fn reset_transient_bridge(bridge: &MisterBridge) {
        bridge.set_startup_visible(false);
        bridge.set_confirm_visible(false);
        bridge.set_catalog_scan_visible(false);
        bridge.set_catalog_background_scan_visible(false);
        bridge.set_loading_message("".into());
        bridge.set_loading_detail("".into());
        bridge.set_media_pack_progresses(ModelRc::new(VecModel::from(
            Vec::<ScreenshotPackProgress>::new(),
        )));
        bridge.set_media_pack_summary("".into());
        bridge.set_setup_visible(false);
        bridge.set_display_combo_open(false);
        bridge.set_arcade_games_loading(false);
        bridge.set_arcade_preview_placeholder_visible(true);
    }

    fn home_menu_items() -> ModelRc<MenuItem> {
        let definitions = [
            ("ready", "Ready", "2,184 games", MenuItemStatus::Ready, true),
            (
                "published-scanning",
                "Published",
                "842 games available",
                MenuItemStatus::Scanning,
                true,
            ),
            (
                "new-scanning",
                "New System",
                "Scanning…",
                MenuItemStatus::Scanning,
                false,
            ),
            (
                "update-failed",
                "Update Failed",
                "Update failed • 68 games",
                MenuItemStatus::UpdateFailed,
                true,
            ),
            (
                "computer",
                "Partial",
                "1,126 games",
                MenuItemStatus::Partial,
                true,
            ),
            (
                "new-failed",
                "Unavailable",
                "Scan failed",
                MenuItemStatus::Failed,
                false,
            ),
        ];
        ModelRc::new(VecModel::from(
            definitions
                .into_iter()
                .map(|(id, label, subtitle, status, available)| MenuItem {
                    id: id.into(),
                    label: label.into(),
                    subtitle: subtitle.into(),
                    available,
                    node_kind: MenuItemKind::Collection,
                    status,
                })
                .collect::<Vec<_>>(),
        ))
    }

    fn home_menu_presentation(count: usize) -> ModelRc<MenuItemPresentation> {
        ModelRc::new(VecModel::from(
            (0..count)
                .map(|index| MenuItemPresentation {
                    selected: index == 0,
                    acknowledged: false,
                })
                .collect::<Vec<_>>(),
        ))
    }

    fn strings(values: &[&str]) -> ModelRc<SharedString> {
        ModelRc::new(VecModel::from(
            values
                .iter()
                .copied()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        ))
    }

    fn fixture_preview_frame(screenshot: &FixtureScreenshot) -> PreviewFrame<'_> {
        PreviewFrame {
            pixels: PreviewPixels::Rgb565 {
                pixels: &screenshot.pixels,
                stride_pixels: screenshot.stride,
            },
            source_width: screenshot.width,
            source_height: screenshot.height,
            display_width: 320,
            display_height: 240,
        }
    }

    fn loaded_screenshot(
        key: String,
        _asset_key: String,
        image: CatalogPreviewPixels,
    ) -> Option<FixtureScreenshot> {
        match image {
            CatalogPreviewPixels::Rgb565 {
                width,
                height,
                stride_bytes,
                words,
            } => {
                let width = usize::try_from(width).ok()?;
                let height = usize::try_from(height).ok()?;
                let stride = usize::try_from(stride_bytes).ok()?.checked_div(2)?;
                Some(FixtureScreenshot {
                    key: key.into(),
                    pixels: words.iter().copied().map(Rgb565Pixel).collect(),
                    width,
                    height,
                    stride,
                })
            }
        }
    }

    struct ScreenshotTileMediaUpdate {
        status: String,
        reload: bool,
    }

    fn screenshot_tile_media_update(status: &str, detail: &str) -> ScreenshotTileMediaUpdate {
        match status {
            "current" | "downloaded" => ScreenshotTileMediaUpdate {
                status: format!("tiles:{status}:arcade"),
                reload: true,
            },
            "failed" => ScreenshotTileMediaUpdate {
                status: format!("tiles:error:Arcade download failed: {detail}"),
                reload: false,
            },
            _ => ScreenshotTileMediaUpdate {
                status: format!("tiles:{status}:arcade"),
                reload: false,
            },
        }
    }

    fn tile_pack_fingerprint(path: &Path) -> Result<TilePackFingerprint, String> {
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("read Arcade screenshot pack metadata: {error}"))?;
        let modified_nanos = metadata
            .modified()
            .ok()
            .and_then(|modified| {
                modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_nanos())
            })
            .unwrap_or(0);
        Ok(TilePackFingerprint {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            modified_nanos,
        })
    }

    fn fixture_screenshot<'a>(
        screenshots: &'a [FixtureScreenshot],
        key: &str,
    ) -> Option<&'a FixtureScreenshot> {
        screenshots
            .iter()
            .find(|screenshot| screenshot.key.as_ref() == key)
    }

    fn preview_storage_key(archive_path: &str, asset_key: &str) -> String {
        let archive_name = Path::new(archive_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(archive_path);
        let pack_id = archive_name
            .split_once("-screenshots")
            .map(|(system, _)| system)
            .unwrap_or(archive_name);
        preview_asset_cache_key(pack_id, asset_key)
    }

    fn preview_screenshot<'a>(
        game: &ArcadeGameEntry,
        loaded: &'a HashMap<String, FixtureScreenshot>,
        fixtures: &'a [FixtureScreenshot],
        use_fixtures: bool,
    ) -> Option<&'a FixtureScreenshot> {
        let key = preview_storage_key(&game.preview_archive_path, &game.preview_asset_key);
        loaded.get(&key).or_else(|| {
            use_fixtures
                .then(|| fixture_screenshot(fixtures, &game.preview_asset_key))
                .flatten()
        })
    }

    fn preview_game<'a>(
        navigation: &'a LauncherNav,
        catalog: &'a ArcadeCatalog,
        index: usize,
    ) -> Option<&'a ArcadeGameEntry> {
        navigation
            .active_collection_id()
            .and_then(|collection| navigation.active_arcade_game_at(catalog, collection, index))
            .or_else(|| catalog.system_game_at(MENU_ARCADE_SYSTEM_ID, index))
    }

    fn apply_arcade_fixture_bridge(
        launcher: &Launcher,
        title: &str,
        games: &[&ArcadeGameEntry],
        selected: usize,
    ) {
        let bridge = launcher.global::<MisterBridge>();
        bridge.set_active_system_title(title.into());
        bridge.set_active_system_count(games.len() as i32);
        bridge.set_arcade_games(ModelRc::new(VecModel::from(
            games
                .iter()
                .map(|game| ArcadeGame {
                    title: game.title.as_ref().into(),
                    mra_path: game.mra_path.as_ref().into(),
                    preview_archive_path: game.preview_archive_path.as_ref().into(),
                    preview_asset_key: game.preview_asset_key.as_ref().into(),
                    has_preview: game.has_preview,
                    system_id: game.system_id.as_ref().into(),
                    is_new: game.is_new,
                })
                .collect::<Vec<_>>(),
        )));
        bridge.set_arcade_list_x(8);
        bridge.set_arcade_list_y(56);
        bridge.set_arcade_list_width(510);
        bridge.set_arcade_list_height(452);
        bridge.set_arcade_list_visible(true);
        bridge.set_arcade_preview_placeholder_visible(false);
        bridge.set_arcade_preview_status(mister_magik_ui::launcher::PreviewStatus::Ready);
        bridge.set_arcade_preview_title(
            games
                .get(selected)
                .map(|game| game.title.as_ref())
                .unwrap_or("No game")
                .into(),
        );
        bridge.set_arcade_preview_source_width(160);
        bridge.set_arcade_preview_source_height(120);
        bridge.set_arcade_preview_display_width(320);
        bridge.set_arcade_preview_display_height(240);
        bridge.set_arcade_preview_box_x(8);
        bridge.set_arcade_preview_box_y(92);
        bridge.set_arcade_preview_box_width(320);
        bridge.set_arcade_preview_box_height(320);
    }

    fn scale_rgb565_nearest(
        source: &[Rgb565Pixel],
        source_width: usize,
        source_height: usize,
        destination: &mut [u32],
        destination_width: usize,
        destination_height: usize,
    ) {
        if source_width == 0
            || source_height == 0
            || destination_width == 0
            || destination_height == 0
        {
            return;
        }
        let scale = (destination_width / source_width)
            .min(destination_height / source_height)
            .max(1);
        let content_width = (source_width * scale).min(destination_width);
        let content_height = (source_height * scale).min(destination_height);
        let offset_x = (destination_width - content_width) / 2;
        let offset_y = (destination_height - content_height) / 2;
        destination.fill(0);
        for destination_y in 0..content_height {
            let source_y = destination_y * source_height / content_height;
            for destination_x in 0..content_width {
                let source_x = destination_x * source_width / content_width;
                destination
                    [(offset_y + destination_y) * destination_width + offset_x + destination_x] =
                    rgb565_to_xrgb8888(source[source_y * source_width + source_x]);
            }
        }
    }

    fn rgb565_to_xrgb8888(pixel: Rgb565Pixel) -> u32 {
        let value = pixel.0;
        let red = u32::from((value >> 11) & 0x1f);
        let green = u32::from((value >> 5) & 0x3f);
        let blue = u32::from(value & 0x1f);
        let red = (red << 3) | (red >> 2);
        let green = (green << 2) | (green >> 4);
        let blue = (blue << 3) | (blue >> 2);
        (red << 16) | (green << 8) | blue
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use mister_magik_fb::launcher_runtime::navigation_transition::NavigationTransitionGeometry;
        use std::sync::atomic::AtomicU64;

        static NEXT_CAPTURE_PATH: AtomicU64 = AtomicU64::new(0);

        #[test]
        fn rgb565_primary_channels_expand_to_xrgb8888() {
            assert_eq!(rgb565_to_xrgb8888(Rgb565Pixel(0xf800)), 0x00ff0000);
            assert_eq!(rgb565_to_xrgb8888(Rgb565Pixel(0x07e0)), 0x0000ff00);
            assert_eq!(rgb565_to_xrgb8888(Rgb565Pixel(0x001f)), 0x000000ff);
        }

        #[test]
        fn portrait_capture_uses_the_shared_output_layout() {
            let display = UiDisplay::for_framebuffer(3, 2);
            let layout =
                UiLayoutGeometry::for_display(&display, ScreenOrientation::MonitorCounterclockwise);
            let logical = (1..=6).map(Rgb565Pixel).collect::<Vec<_>>();

            let physical = oriented_capture(&logical, layout);

            for y in 0..layout.logical_h() {
                for x in 0..layout.logical_w() {
                    assert_eq!(
                        physical[layout.output_layout().physical_offset(x, y)],
                        logical[y * layout.logical_w() + x]
                    );
                }
            }
        }

        #[test]
        fn preview_options_parse_deterministic_capture() {
            let options = PreviewOptions::parse(
                [
                    "--scenario",
                    "screenshot-tiles",
                    "--frame",
                    "12",
                    "--output",
                    "out.ppm",
                ]
                .map(String::from),
            )
            .unwrap();
            assert_eq!(options.scenario, Scenario::ScreenshotTiles);
            assert_eq!(options.frame, 12);
            assert_eq!(options.output, Some(PathBuf::from("out.ppm")));
            assert_eq!(options.refresh_rate, RefreshRate::Auto);
            assert_eq!(options.content_mode, ContentMode::Auto);
            assert_eq!(options.display_profile, DisplayProfile::Hdmi);
            assert_eq!(options.orientation, ScreenOrientation::Normal);
        }

        #[test]
        fn preview_options_parse_both_monitor_orientations() {
            let clockwise =
                PreviewOptions::parse(["--orientation", "monitor-clockwise"].map(String::from))
                    .unwrap();
            let counterclockwise = PreviewOptions::parse(
                ["--orientation", "monitor-counterclockwise"].map(String::from),
            )
            .unwrap();

            assert_eq!(clockwise.orientation, ScreenOrientation::MonitorClockwise);
            assert_eq!(
                counterclockwise.orientation,
                ScreenOrientation::MonitorCounterclockwise
            );
        }

        #[test]
        fn preview_options_parse_orientation_choice_aliases() {
            for scenario in ["orientation-choice", "orientation-chooser"] {
                let options =
                    PreviewOptions::parse(["--scenario", scenario].map(String::from)).unwrap();

                assert_eq!(options.scenario, Scenario::OrientationChoice);
            }
        }

        #[test]
        fn preview_options_parse_crt_and_offline_card_controls() {
            let options = PreviewOptions::parse(
                ["--display-profile", "crt", "--no-scan", "--no-download"].map(String::from),
            )
            .unwrap();

            assert_eq!(options.display_profile, DisplayProfile::Crt480p);
            assert!(options.no_scan);
            assert!(options.no_download);
        }

        #[test]
        fn preview_options_parse_navigation_transition_debug_controls() {
            let options = PreviewOptions::parse(
                [
                    "--navigation-transition-demo",
                    "home-arcade",
                    "--navigation-transition-duration-ms",
                    "4000",
                    "--navigation-transition-demo-reverse",
                ]
                .map(String::from),
            )
            .unwrap();

            assert_eq!(
                options.navigation_transition_demo,
                Some(NavigationTransitionEdge::HomeToArcade)
            );
            assert_eq!(options.navigation_transition_duration_ms, Some(4_000));
            assert!(options.navigation_transition_demo_reverse);
        }

        #[test]
        fn preview_options_parse_portrait_settings_transition_demo() {
            let options = PreviewOptions::parse(
                [
                    "--settings-page-transition-demo",
                    "--navigation-transition-demo-reverse",
                ]
                .map(String::from),
            )
            .unwrap();

            assert!(options.settings_page_transition_demo);
            assert!(options.navigation_transition_demo_reverse);
        }

        #[test]
        fn display_profiles_use_route_geometry_fonts_and_metrics() {
            for (name, profile, route, render_size, body_font, font_family) in [
                (
                    "hdmi",
                    DisplayProfile::Hdmi,
                    ResolvedOutputRoute::Hdmi,
                    (960, 540),
                    UiPixelSize::Px8,
                    "Press Start 2P",
                ),
                (
                    "crt-240p",
                    DisplayProfile::Crt240p,
                    ResolvedOutputRoute::Crt240p60,
                    (640, 480),
                    UiPixelSize::Px16,
                    "Press Start 2P",
                ),
                (
                    "crt-288p",
                    DisplayProfile::Crt288p,
                    ResolvedOutputRoute::Crt288p50,
                    (640, 288),
                    UiPixelSize::Px16,
                    "Press Start 2P",
                ),
                (
                    "crt-480p",
                    DisplayProfile::Crt480p,
                    ResolvedOutputRoute::Crt480p60,
                    (640, 480),
                    UiPixelSize::Px8,
                    "Press Start 2P",
                ),
                (
                    "crt-576p",
                    DisplayProfile::Crt576p,
                    ResolvedOutputRoute::Crt576p50,
                    (640, 576),
                    UiPixelSize::Px8,
                    "Press Start 2P",
                ),
            ] {
                assert_eq!(DisplayProfile::parse(name).unwrap(), profile);
                assert_eq!(profile.route(), route);
                assert_eq!(profile.render_size(), render_size);
                let display = profile.display();
                assert_eq!(
                    (display.render_w(), display.render_h()),
                    profile.render_size()
                );
                assert_eq!(
                    CrtUiMetrics::for_display(&display).font_family.label(),
                    font_family
                );
                assert_eq!(CrtUiMetrics::for_display(&display).body_font, body_font);
            }
        }

        #[test]
        fn display_profiles_bind_their_launcher_resolution() {
            for (profile, expected_id) in [
                (DisplayProfile::Hdmi, "hdmi-1920x1080p60"),
                (DisplayProfile::Crt240p, "crt-240p60"),
                (DisplayProfile::Crt288p, "crt-288p50"),
                (DisplayProfile::Crt480p, "crt-480p60"),
                (DisplayProfile::Crt576p, "crt-576p50"),
            ] {
                let index = profile.display_resolution_index();
                assert_eq!(
                    mister_magik_mister_runtime::display_resolution::DISPLAY_RESOLUTIONS[index].id,
                    expected_id
                );
            }
        }

        #[test]
        fn start2p_visual_scenarios_are_headless_selectable() {
            for scenario in [
                "home",
                "arcade",
                "arcade-search",
                "settings",
                "controller",
                "controller-setup",
                "about",
                "info",
                "licenses",
                "screensaver-settings",
                "startup",
                "confirm",
                "catalog-scan",
                "background-scan",
                "loading",
                "media-progress",
            ] {
                assert!(Scenario::parse(scenario).is_some(), "{scenario}");
            }
        }

        #[test]
        fn preview_options_parse_explicit_card_paths() {
            let options = PreviewOptions::parse(
                [
                    "--content",
                    "card",
                    "--sd-root",
                    "/Volumes/MiSTer_Data",
                    "--cache-root",
                    "/tmp/mister-preview-cache",
                ]
                .map(String::from),
            )
            .unwrap();

            assert_eq!(options.content_mode, ContentMode::Card);
            assert_eq!(options.sd_root, Some(PathBuf::from("/Volumes/MiSTer_Data")));
            assert_eq!(
                options.cache_root,
                Some(PathBuf::from("/tmp/mister-preview-cache"))
            );
        }

        #[test]
        fn provenance_output_requires_a_distinct_capture_output() {
            assert!(
                PreviewOptions::parse(
                    ["--provenance-output", "/tmp/capture.json"].map(String::from)
                )
                .is_err()
            );
            assert!(
                PreviewOptions::parse(
                    [
                        "--output",
                        "/tmp/capture.png",
                        "--provenance-output",
                        "/tmp/capture.png",
                    ]
                    .map(String::from)
                )
                .is_err()
            );
            let options = PreviewOptions::parse(
                [
                    "--output",
                    "/tmp/capture.png",
                    "--provenance-output",
                    "/tmp/capture.json",
                ]
                .map(String::from),
            )
            .unwrap();
            assert_eq!(
                options.provenance_output,
                Some(PathBuf::from("/tmp/capture.json"))
            );
        }

        #[test]
        fn matrix_comparison_requires_all_three_explicit_paths() {
            assert!(
                PreviewOptions::parse(["--expected-matrix", "/tmp/expected"].map(String::from))
                    .is_err()
            );
            assert!(
                PreviewOptions::parse(
                    [
                        "--expected-matrix",
                        "/tmp/expected",
                        "--mismatch-output",
                        "/tmp/mismatch",
                    ]
                    .map(String::from)
                )
                .is_err()
            );
            let options = PreviewOptions::parse(
                [
                    "--matrix-output",
                    "/tmp/actual",
                    "--expected-matrix",
                    "/tmp/expected",
                    "--mismatch-output",
                    "/tmp/mismatch",
                ]
                .map(String::from),
            )
            .expect("complete matrix comparison paths");
            assert_eq!(options.matrix_output, Some(PathBuf::from("/tmp/actual")));
            assert_eq!(
                options.expected_matrix,
                Some(PathBuf::from("/tmp/expected"))
            );
            assert_eq!(
                options.mismatch_output,
                Some(PathBuf::from("/tmp/mismatch"))
            );
        }

        #[test]
        fn baseline_check_is_separate_from_generation_and_capture() {
            let options = PreviewOptions::parse(
                [
                    "--check-baselines",
                    "apps/mister/tests/visual-baselines/launcher",
                ]
                .map(String::from),
            )
            .expect("read-only baseline check");
            assert_eq!(
                options.check_baselines,
                Some(PathBuf::from("apps/mister/tests/visual-baselines/launcher"))
            );
            assert!(
                PreviewOptions::parse(
                    [
                        "--check-baselines",
                        "/tmp/expected",
                        "--matrix-output",
                        "/tmp/actual",
                    ]
                    .map(String::from)
                )
                .is_err()
            );
            assert!(
                PreviewOptions::parse(
                    [
                        "--check-baselines",
                        "/tmp/expected",
                        "--output",
                        "/tmp/capture.png",
                    ]
                    .map(String::from)
                )
                .is_err()
            );
        }

        #[test]
        fn matrix_runner_maps_every_manifest_scene_to_explicit_outputs() {
            let manifest = launcher_scene_manifest().unwrap();
            for scene in &manifest.scenes {
                let arguments = scene_arguments(scene, Path::new("/tmp/matrix"))
                    .into_iter()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                assert!(
                    arguments
                        .windows(2)
                        .any(|pair| pair == ["--content", "fixtures"])
                );
                assert!(arguments.contains(&"--no-scan".to_owned()));
                assert!(arguments.contains(&"--no-download".to_owned()));
                assert!(arguments.contains(&format!("/tmp/matrix/{}.png", scene.id)));
                assert!(arguments.contains(&format!("/tmp/matrix/{}.json", scene.id)));
                assert_eq!(
                    arguments.contains(&"--navigation-transition-demo".to_owned()),
                    scene.scenario == SceneScenario::NavigationTransitionMidpoint
                );
            }

            let options =
                PreviewOptions::parse(["--matrix-output", "/tmp/matrix"].map(String::from))
                    .unwrap();
            assert_eq!(options.matrix_output, Some(PathBuf::from("/tmp/matrix")));
        }

        #[test]
        fn explicit_refresh_rate_gives_time_based_headless_frames() {
            let options =
                PreviewOptions::parse(["--refresh-rate", "120"].map(String::from)).unwrap();
            assert_eq!(options.refresh_rate, RefreshRate::Hz120);
            assert_eq!(elapsed_for_frame(6, 60), Duration::from_millis(100));
            assert_eq!(elapsed_for_frame(12, 120), Duration::from_millis(100));
        }

        #[test]
        fn production_home_velocity_matches_at_60_and_120_hz() {
            let sixty = run_home_velocity(60);
            let one_twenty = run_home_velocity(120);
            assert_eq!(sixty.0, one_twenty.0);
            assert!((sixty.1 - one_twenty.1).abs() <= 1);
        }

        #[test]
        fn production_arcade_velocity_matches_at_60_and_120_hz() {
            let sixty = run_arcade_velocity(60);
            let one_twenty = run_arcade_velocity(120);
            assert_eq!(sixty.0, one_twenty.0);
            assert!((sixty.1 - one_twenty.1).abs() <= 1);
        }

        #[test]
        fn home_settings_focus_activates_settings() {
            assert_eq!(
                activated_scenario(Scenario::Home, 0, true),
                Some(Scenario::Settings)
            );
            assert_eq!(
                activated_scenario(Scenario::Home, 0, false),
                Some(Scenario::Arcade)
            );
        }

        #[test]
        fn settings_subpages_and_back_routes_match_launcher_hierarchy() {
            assert_eq!(
                activated_scenario(Scenario::Settings, 1, false),
                Some(Scenario::ScreensaverSettings)
            );
            assert_eq!(
                activated_scenario(Scenario::Settings, 5, false),
                Some(Scenario::About)
            );
            assert_eq!(
                back_scenario(Scenario::ScreensaverSettings),
                Some(Scenario::Settings)
            );
            assert_eq!(back_scenario(Scenario::Info), Some(Scenario::About));
        }

        #[test]
        fn numeric_keypad_shortcuts_open_scenarios() {
            assert_eq!(
                shortcut_scenario(KeyCode::Numpad2),
                Some(Scenario::Settings)
            );
            assert_eq!(
                shortcut_scenario(KeyCode::Numpad7),
                Some(Scenario::ScreensaverSettings)
            );
        }

        #[test]
        fn frame_hash_tracks_rgb565_bytes() {
            assert_eq!(
                frame_hash(&[Rgb565Pixel(0x1234), Rgb565Pixel(0xabcd)]),
                0x462038d925b18c13
            );
        }

        #[test]
        fn png_capture_is_deterministic_rgb_with_pinned_dimensions() {
            let pixels = [
                Rgb565Pixel(0xf800),
                Rgb565Pixel(0x07e0),
                Rgb565Pixel(0x001f),
                Rgb565Pixel(0xffff),
            ];

            let first = encode_png(&pixels, 2, 2).expect("encode first PNG");
            let second = encode_png(&pixels, 2, 2).expect("encode second PNG");

            assert_eq!(first, second);
            assert_eq!(&first[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(&first[12..16], b"IHDR");
            assert_eq!(u32::from_be_bytes(first[16..20].try_into().unwrap()), 2);
            assert_eq!(u32::from_be_bytes(first[20..24].try_into().unwrap()), 2);
            assert_eq!(first[24], 8);
            assert_eq!(first[25], 2);
        }

        #[test]
        fn capture_provenance_is_stable_and_every_pinned_fact_changes_identity() {
            let options = PreviewOptions::parse(
                [
                    "--scenario",
                    "particle-screensaver",
                    "--frame",
                    "7",
                    "--refresh-rate",
                    "60",
                    "--output",
                    "/tmp/capture.png",
                ]
                .map(String::from),
            )
            .unwrap();
            let base = CaptureProvenance::for_capture(
                &options,
                960,
                540,
                60,
                Duration::from_micros(116_666),
                0x0123_4567_89ab_cdef,
            );
            let repeated = CaptureProvenance::for_capture(
                &options,
                960,
                540,
                60,
                Duration::from_micros(116_666),
                0x0123_4567_89ab_cdef,
            );

            assert_eq!(base.encoded().unwrap(), repeated.encoded().unwrap());
            assert_eq!(base.identity().unwrap(), repeated.identity().unwrap());
            assert_eq!(base.identity().unwrap().len(), 64);
            assert_eq!(base.scene_seed.as_deref(), Some("0000004d6167694b"));
            let json = String::from_utf8(base.encoded().unwrap()).unwrap();
            assert!(!json.contains("/tmp/capture.png"));
            assert!(!json.contains("/Users/"));

            assert_provenance_change(&base, |value| value.frame += 1);
            assert_provenance_change(&base, |value| value.width += 1);
            assert_provenance_change(&base, |value| value.height += 1);
            assert_provenance_change(&base, |value| value.orientation = "monitor-clockwise");
            assert_provenance_change(&base, |value| value.display_profile = "crt-480p");
            assert_provenance_change(&base, |value| value.refresh_hz = 120);
            assert_provenance_change(&base, |value| value.fixed_time_us += 1);
            assert_provenance_change(&base, |value| value.scene_seed = None);
            assert_provenance_change(&base, |value| value.rgb565_hash.push('0'));
            assert_provenance_change(&base, |value| value.fixture_sha256.push('0'));
            assert_provenance_change(&base, |value| value.asset_bundle_sha256.push('0'));
            assert_provenance_change(&base, |value| value.font_bundle_sha256.push('0'));
        }

        #[test]
        fn provenance_writer_refuses_overwrite_and_slint_version_is_pinned() {
            assert!(include_str!("../../Cargo.toml").contains("slint = { version = \"=1.17.1\""));
            let options =
                PreviewOptions::parse(["--output", "/tmp/capture.png"].map(String::from)).unwrap();
            let provenance =
                CaptureProvenance::for_capture(&options, 2, 1, 60, Duration::ZERO, 0x1234);
            let suffix = NEXT_CAPTURE_PATH.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mister-magik-ui-preview-provenance-{}-{suffix}.json",
                std::process::id()
            ));

            write_capture_provenance(&path, &provenance).expect("write provenance");
            let first = std::fs::read(&path).expect("read provenance");
            assert!(write_capture_provenance(&path, &provenance).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), first);
            let _ = std::fs::remove_file(path);
        }

        fn assert_provenance_change(
            base: &CaptureProvenance,
            mutate: impl FnOnce(&mut CaptureProvenance),
        ) {
            let mut changed = base.clone();
            mutate(&mut changed);
            assert_ne!(base.identity().unwrap(), changed.identity().unwrap());
        }

        #[test]
        fn capture_writer_preserves_ppm_and_refuses_overwrite() {
            let suffix = NEXT_CAPTURE_PATH.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir();
            let png = root.join(format!(
                "mister-magik-ui-preview-{}-{suffix}.png",
                std::process::id()
            ));
            let ppm = root.join(format!(
                "mister-magik-ui-preview-{}-{suffix}.ppm",
                std::process::id()
            ));
            let pixels = [Rgb565Pixel(0xf800), Rgb565Pixel(0x07e0)];

            write_capture(&png, &pixels, 2, 1).expect("write PNG capture");
            let png_bytes = std::fs::read(&png).expect("read PNG capture");
            assert_eq!(&png_bytes[..8], b"\x89PNG\r\n\x1a\n");
            assert!(write_capture(&png, &pixels, 2, 1).is_err());
            assert_eq!(std::fs::read(&png).unwrap(), png_bytes);

            write_capture(&ppm, &pixels, 2, 1).expect("write PPM capture");
            let ppm_bytes = std::fs::read(&ppm).expect("read PPM capture");
            assert!(ppm_bytes.starts_with(b"P6\n2 1\n255\n"));
            assert!(write_capture(&ppm, &pixels, 2, 1).is_err());
            assert_eq!(std::fs::read(&ppm).unwrap(), ppm_bytes);

            let _ = std::fs::remove_file(png);
            let _ = std::fs::remove_file(ppm);
        }

        #[test]
        fn reverse_endpoint_comparison_checks_the_complete_frame() {
            let width = 320;
            let height = 64;
            let first = vec![Rgb565Pixel(0x1234); width * height];
            let mut second = first.clone();
            second[8 * width + 26] = Rgb565Pixel(0xabcd);
            assert_eq!(
                frame_difference(&first, &second, width, height),
                Some((1, 26, 8, 26, 8))
            );

            second[40 * width + 26] = Rgb565Pixel(0xabcd);
            assert_eq!(
                frame_difference(&first, &second, width, height),
                Some((2, 26, 8, 26, 40))
            );
        }

        #[test]
        fn arcade_media_completion_triggers_tile_pack_reload() {
            assert!(
                preview_archive_path_for_system(ARCADE_MEDIA_SYSTEM_ID)
                    .ends_with("/arcade-screenshots.mmlz4b")
            );
            for status in ["current", "downloaded"] {
                let update = screenshot_tile_media_update(status, "ready");
                assert!(update.reload);
                assert_eq!(update.status, format!("tiles:{status}:arcade"));
            }
            let failed = screenshot_tile_media_update("failed", "checksum mismatch");
            assert!(!failed.reload);
            assert!(failed.status.contains("checksum mismatch"));
            let missing = screenshot_tile_media_update("missing", "not cached");
            assert!(!missing.reload);
            assert_eq!(missing.status, "tiles:missing:arcade");
        }

        #[test]
        fn tile_pack_fingerprint_detects_republished_archive() {
            let path = std::env::temp_dir().join(format!(
                "mister-magik-ui-preview-fingerprint-{}",
                std::process::id()
            ));
            std::fs::write(&path, b"first").expect("write first pack");
            let first = tile_pack_fingerprint(&path).expect("first fingerprint");
            std::fs::write(&path, b"second-version").expect("replace pack");
            let second = tile_pack_fingerprint(&path).expect("second fingerprint");

            assert_ne!(first, second);
            let _ = std::fs::remove_file(path);
        }

        fn prepared_nav() -> (UiPreviewFixtures, LauncherNav) {
            let fixtures = UiPreviewFixtures::new().expect("preview fixtures");
            let mut nav = LauncherNav::new();
            nav.catalog_build_started();
            for system_id in &fixtures.shell_system_ids {
                nav.catalog_system_discovered(system_id);
                nav.catalog_system_update_ready(system_id);
            }
            nav.sync_launcher_taxonomy(&fixtures.catalog);
            nav.catalog_build_finished(&fixtures.catalog);
            (fixtures, nav)
        }

        #[test]
        fn forward_system_transition_commits_after_source_capture_has_advanced() {
            let (fixtures, mut nav) = prepared_nav();
            assert!(nav.open_menu(CONSOLES_MENU_ID));
            let collection_id = nav
                .current_menu_items()
                .iter()
                .find(|item| item.kind == LauncherMenuItemKind::Collection)
                .map(|item| item.id.clone())
                .expect("fixture consoles menu has a collection");
            let event = LauncherEvent {
                action: LauncherAction::OpenCollection,
                path: Some(collection_id.clone()),
                settings: None,
            };
            let mut transition =
                NavigationTransitionRuntime::new(HDMI_FRAME_WIDTH, HDMI_FRAME_HEIGHT, true);
            let source = vec![Rgb565Pixel(0); HDMI_FRAME_WIDTH * HDMI_FRAME_HEIGHT];
            assert!(
                transition
                    .begin(
                        NavigationTransitionEdge::ConsolesToSystem,
                        NavigationTransitionDirection::Forward,
                        NavigationTransitionGeometry::default(),
                        &source,
                        0,
                    )
                    .expect("begin transition")
            );
            assert_eq!(transition.frame().phase, NavigationTransitionPhase::Expand);

            assert!(commit_preview_navigation_destination(
                &mut nav,
                &fixtures.catalog,
                Some(&event),
                &mut transition,
                1,
            ));
            assert_eq!(nav.active_collection_id(), Some(collection_id.as_str()));
            assert_eq!(nav.screen, Screen::Arcade);
        }

        fn run_home_velocity(refresh_hz: u32) -> (usize, i32) {
            let (fixtures, mut nav) = prepared_nav();
            let epoch = Instant::now();
            let mut pad = PadState {
                dpad_right: true,
                ..PadState::default()
            };
            pad.rebuild_pressed_now();
            for frame in 0..refresh_hz {
                nav.handle_held_tick_with_navigation_intents(
                    &pad,
                    epoch + elapsed_for_frame(u64::from(frame), refresh_hz),
                    &fixtures.catalog,
                );
            }
            pad = PadState::default();
            for frame in refresh_hz..=refresh_hz * 2 {
                nav.handle_held_tick_with_navigation_intents(
                    &pad,
                    epoch + elapsed_for_frame(u64::from(frame), refresh_hz),
                    &fixtures.catalog,
                );
            }
            (nav.selected, nav.scroll_x)
        }

        fn run_arcade_velocity(refresh_hz: u32) -> (usize, i32) {
            let (fixtures, mut nav) = prepared_nav();
            assert!(nav.open_default_arcade(&fixtures.catalog));
            let epoch = Instant::now();
            let mut pad = PadState {
                dpad_down: true,
                ..PadState::default()
            };
            pad.rebuild_pressed_now();
            for frame in 0..refresh_hz {
                nav.handle_held_tick_with_navigation_intents(
                    &pad,
                    epoch + elapsed_for_frame(u64::from(frame), refresh_hz),
                    &fixtures.catalog,
                );
            }
            pad = PadState::default();
            for frame in refresh_hz..=refresh_hz * 2 {
                nav.handle_held_tick_with_navigation_intents(
                    &pad,
                    epoch + elapsed_for_frame(u64::from(frame), refresh_hz),
                    &fixtures.catalog,
                );
            }
            (nav.arcade.selected, nav.arcade.scroll_y)
        }
    }
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    macos::run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("mister-magik-ui-preview is available on macOS only");
}
