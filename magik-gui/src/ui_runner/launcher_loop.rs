use super::launcher_frame_accounting::{LauncherFrameAccounting, LauncherPresentedFrame};
use super::launcher_worker_intents::{
    apply_launcher_worker_ui_intent, cached_catalog_validation_intent,
    catalog_rebuild_started_intent, catalog_scan_message, parse_games_found_detail,
    sync_launcher_worker_ui_intent, CatalogCounterPhase, CatalogProgressUiIntent,
    CatalogWorkerUiContext, LauncherWorkerUiIntent, MediaProgressDisplay,
};
#[cfg(test)]
use super::launcher_worker_intents::{
    catalog_background_scan_progress_visible, catalog_scan_progress_visible,
};
use super::*;
use crate::fb::VsyncWaitStatus;
use crate::preview_worker;
use mister_magik_fb::framebuffer_ownership::{
    should_present_full_frame, FramebufferRouteAction, FramebufferRouteGuard,
};
use std::collections::BTreeSet;

const DEFAULT_LAUNCHER_REVEAL_SETTLE_FRAMES: u32 = 3;
const MAX_LAUNCHER_REVEAL_SETTLE_FRAMES: u32 = 30;
const DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY: Duration = Duration::from_secs(2);
const LIBRARY_CHANGED_TEST_ACTION_SETTLE: Duration = Duration::from_millis(1200);

struct DeferredCatalogWorker {
    root: String,
    request: CatalogWorkerRequest,
    start_after: Option<Instant>,
}

pub(super) fn recover_launcher_ui(f: &mut Fpga, ui: &UiDisplay, spawned_mister: &mut bool) {
    if *spawned_mister {
        launcher::stop_mister();
        if let Err(e) = f.fb_enable_format(
            0,
            ui.fb_w() as u16,
            ui.fb_h() as u16,
            ui_fpga_scaled_mode(ui.scan_w(), ui.scan_h()),
            Some(0),
            Some(0),
            ui.direct_video(),
            FramebufferFormat::production_default(),
        ) {
            eprintln!("failed to recover Slint framebuffer route after launch failure: {e}");
        }
        *spawned_mister = false;
    }
}

pub(super) fn present_launcher_startup_frame(
    start: Instant,
    ui: &UiDisplay,
    disp: &mut Display,
    f: &mut Fpga,
    window: &Rc<MinimalSoftwareWindow>,
    target: &mut UiFrameTarget,
) {
    if launcher_return_reveal_enabled() {
        print_startup_event(
            start,
            "startup_splash_skipped",
            "reason=return_to_launcher reveal=first_launcher_frame",
        );
        return;
    }

    let settle_frames = launcher_reveal_settle_frames();
    let render_count = settle_frames.max(1);
    let mut render_us = 0u128;
    let mut vsync_hits = 0u32;
    let mut vsync_timeouts = 0u32;
    let mut vsync_errors = 0u32;

    for frame in 0..render_count {
        let draw_t = Instant::now();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            let _ = target.render(renderer, ui);
        });
        render_us += draw_t.elapsed().as_micros();
        if frame < settle_frames {
            match disp.wait_vsync() {
                VsyncWaitStatus::Hit { .. } => vsync_hits += 1,
                VsyncWaitStatus::Timeout { .. } => vsync_timeouts += 1,
                VsyncWaitStatus::Error { .. } => vsync_errors += 1,
            }
        }
    }

    let copy_t = Instant::now();
    target.present_rows(f, disp, ui, 0, ui.render_h());
    print_startup_event(
        start,
        "startup_splash_presented",
        format!(
            "settle_frames={} render_count={} render_us={} copy_us={} vsync_hits={} vsync_timeouts={} vsync_errors={}",
            settle_frames,
            render_count,
            render_us,
            copy_t.elapsed().as_micros(),
            vsync_hits,
            vsync_timeouts,
            vsync_errors
        ),
    );
}

fn launcher_return_reveal_enabled() -> bool {
    matches!(
        std::env::var("MISTER_MAGIK_RETURN_TO_LAUNCHER")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn launcher_reveal_settle_frames() -> u32 {
    std::env::var("MISTER_LAUNCHER_REVEAL_SETTLE_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(DEFAULT_LAUNCHER_REVEAL_SETTLE_FRAMES)
        .min(MAX_LAUNCHER_REVEAL_SETTLE_FRAMES)
}

pub(super) fn run_launcher_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    f: &mut Fpga,
    window: &Rc<MinimalSoftwareWindow>,
    target: &mut UiFrameTarget,
    mut pad: PadPool,
    app: slint_ui::launcher::Launcher,
    animation_clock: &AnimationClock,
) {
    let start = Instant::now();
    let mut frames = 0u64;
    let launcher_bench_scenario = LauncherBenchScenario::from_env();
    let bench_starts_on_arcade =
        launcher_bench_scenario.is_some_and(|scenario| scenario.starts_on_arcade());
    let env_start_screen = launcher_start_screen_from_env();
    let start_screen = env_start_screen
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade))
        .unwrap_or(Screen::Home);
    let lock_screen = launcher_lock_screen_from_env()
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade));
    let launch_return_restore_allowed =
        env_start_screen.is_none() && launcher_bench_scenario.is_none() && lock_screen.is_none();
    let mut pending_launch_return_state =
        launcher::take_launch_return_state().filter(|_| launch_return_restore_allowed);
    let arcade_catalog_required_at_start =
        start_screen == Screen::Arcade || lock_screen == Some(Screen::Arcade);
    let mut nav = LauncherNav::new();
    nav.screen = start_screen;
    let mut setup = SetupNav::new();
    let mut loading_title = String::new();
    let mut launch_started = Instant::now();
    let mut launch_spawned_mister = false;
    let mut last_clock_update = Instant::now() - Duration::from_secs(2);
    let mut last_clock_text = launcher_clock_text();
    let mut launcher_bench_next_step: Instant;
    let mut launcher_bench_step_idx = 0usize;
    let auto_launch_selected = launcher_auto_launch_selected_enabled();
    let mut auto_launch_selected_done = false;
    let dirty_opt = launcher_dirty_opt_enabled();
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "launcher running {label} — {} pad(s), D-pad to move, A to select, Home to go back...",
        pad.len()
    );
    println!(
        "launcher_mode={} fb_format={}",
        "launcher",
        FramebufferFormat::production_default().label()
    );
    if let Some(scenario) = launcher_bench_scenario {
        println!("launcher_bench_scenario={}", scenario.label());
    }
    println!(
        "launcher_start_screen={} launcher_lock_screen={}",
        screen_label(start_screen),
        lock_screen.map(screen_label).unwrap_or("none")
    );
    println!(
        "launcher_dirty_opt={}",
        if dirty_opt { "on" } else { "off" }
    );
    boot_analytics::event(
        "launcher_loop_start",
        format!("label={label} pads={}", pad.len()),
    );
    if AUTO_CONTROLLER_SETUP_ENABLED {
        if let Some(idx) = pad.index_needing_setup() {
            let status = pad.db().registry_status(pad.info_at(idx));
            eprintln!("controller setup: pad {idx} needs setup ({status:?}) - showing prompt");
            setup.open_for(status, idx);
        }
    }
    let mut pacer = VsyncPacer::from_env();
    let mut present_probe = PresentProbe::from_env();
    if launcher_bench_scenario.is_some() {
        let warm_t = Instant::now();
        match preview_worker::warm_preview_archives_from_env() {
            Ok(loaded) => print_startup_event(
                start,
                "preview_archive_warm",
                format!(
                    "loaded={} elapsed_us={}",
                    if loaded { 1 } else { 0 },
                    warm_t.elapsed().as_micros()
                ),
            ),
            Err(e) => {
                eprintln!("preview archive warm failed before launcher benchmark: {e}");
                print_startup_event(start, "preview_archive_warm_failed", e);
                std::process::exit(13);
            }
        }
    }
    let mut preview = PreviewState::new();
    let mut launcher_bench_waiting_for_initial_preview =
        launcher_bench_scenario.is_some_and(|scenario| scenario.starts_on_arcade());
    let mut route_guard = FramebufferRouteGuard::from_env();
    let mut preview_transition = PreviewTransitionDemo::from_env();
    let mut effect_label_overlay = preview_transition
        .label_overlay_enabled()
        .then(EffectLabelOverlay::new);
    let transition_picker_enabled = preview_transition.picker_enabled();
    let mut transition_picker_prev_left = false;
    let mut transition_picker_prev_right = false;
    let mut arcade_list_renderer = ArcadeListRenderer::new();
    let cpu = cpu_profile::start();
    let mut bridge_models = LauncherBridgeModels::default();
    let mut catalog_version = 0usize;
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    println!(
        "preview_visual_pct={} preview_blitter=raw",
        preview_visual_pct()
    );
    println!(
        "preview_transition={} segment_secs={} duration_ms={}",
        preview_transition.labels(),
        preview_transition.segment.as_secs(),
        preview_transition.duration.as_millis()
    );
    let mut catalog = empty_arcade_catalog(&arcade_root);
    let mut catalog_ready = false;
    let catalog_refresh_policy = catalog_refresh_policy();
    let catalog_refresh = catalog_refresh_policy.force_requested();
    let catalog_worker_enabled = catalog_refresh_policy.worker_enabled();
    let deferred_library_rebuild = if catalog_worker_enabled {
        match launcher::consume_library_rebuild_on_next_boot() {
            Ok(pending) => {
                if pending {
                    print_startup_event(start, "library_rebuild_marker_consumed", "pending=1");
                }
                pending
            }
            Err(e) => {
                eprintln!("failed to consume library rebuild marker: {e}");
                print_startup_event(start, "library_rebuild_marker_consume_failed", e);
                false
            }
        }
    } else {
        false
    };
    let mut catalog_foreground_update = deferred_library_rebuild;
    let mut catalog_rx = None;
    let mut deferred_catalog_worker = None;
    let mut catalog_refresh_done = false;
    let mut media_handle = None;
    let mut media_worker_unavailable = false;
    let mut media_catalog_seed_pending = false;
    let mut media_progress_display = MediaProgressDisplay::default();
    let mut catalog_persisted_summary_seen = false;
    let library_changed_test_action = library_changed_test_action_from_env(start);
    let mut library_changed_test_action_armed = library_changed_test_action.is_some();
    let mut library_changed_test_dialog_seen_at: Option<Instant> = None;
    match library_db::load_arcade_catalog_from_sqlite(&arcade_root) {
        Ok(loaded) if !loaded.catalog.games.is_empty() => {
            print_startup_event(
                start,
                "catalog_cache_load_sync",
                catalog_load_timing_detail(&loaded),
            );
            catalog = loaded.catalog;
            catalog_ready = true;
            media_catalog_seed_pending = true;
            catalog_version = catalog_version.wrapping_add(1);
            apply_forced_arcade_selected(&mut nav, &catalog);
            apply_pending_launch_return_state(&mut nav, &catalog, &mut pending_launch_return_state);
            let request = ready_catalog_worker_request(catalog_refresh_policy);
            if deferred_library_rebuild {
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                catalog_rx = Some(start_library_catalog_worker(
                    arcade_root.clone(),
                    CatalogWorkerRequest::ForceBuild,
                    CatalogWorkerInitialCache::AlreadyLoadedReady,
                ));
            } else if request != CatalogWorkerRequest::LoadOnly {
                let delay = catalog_background_validation_delay();
                print_startup_event(
                    start,
                    "catalog_worker_deferred",
                    format!(
                        "root={} request={} delay_ms={}",
                        arcade_root,
                        request.label(),
                        delay.as_millis()
                    ),
                );
                deferred_catalog_worker = Some(DeferredCatalogWorker {
                    root: arcade_root.clone(),
                    request,
                    start_after: None,
                });
            } else {
                print_startup_event(
                    start,
                    "catalog_refresh_decision",
                    format!(
                        "cache_state=ready refresh_policy={} background_validation=false plan=load_only",
                        catalog_refresh_policy.label()
                    ),
                );
                catalog_rx = None;
                catalog_refresh_done = true;
            }
        }
        Ok(loaded) => {
            print_startup_event(
                start,
                "catalog_cache_empty",
                catalog_load_timing_detail(&loaded),
            );
            if catalog_worker_enabled {
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                catalog_rx = Some(start_library_catalog_worker(
                    arcade_root.clone(),
                    CatalogWorkerRequest::ForceBuild,
                    CatalogWorkerInitialCache::ProbeSqlite,
                ));
            } else {
                print_startup_event(
                    start,
                    "catalog_refresh_decision",
                    format!(
                        "cache_state=empty refresh_policy={} background_validation=false plan=load_only",
                        catalog_refresh_policy.label()
                    ),
                );
                catalog_refresh_done = true;
            }
        }
        Err(e) => {
            eprintln!("arcade catalog cache load failed: {e}");
            print_startup_event(start, "catalog_cache_load_failed", e);
            if catalog_worker_enabled {
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                catalog_rx = Some(start_library_catalog_worker(
                    arcade_root.clone(),
                    CatalogWorkerRequest::ForceBuild,
                    CatalogWorkerInitialCache::ProbeSqlite,
                ));
            } else {
                print_startup_event(
                    start,
                    "catalog_refresh_decision",
                    format!(
                        "cache_state=missing refresh_policy={} background_validation=false plan=load_only",
                        catalog_refresh_policy.label()
                    ),
                );
                catalog_refresh_done = true;
            }
        }
    }
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let bridge_systems_t = Instant::now();
    bridge.set_game_systems(bridge_models.game_systems(&catalog, catalog_version));
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
        if catalog_foreground_update {
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
        if catalog_foreground_update {
            "Rebuilding catalog with latest games...".to_string()
        } else {
            format!("Using cached {} games", catalog.len())
        }
    } else if !catalog_worker_enabled {
        "Catalog worker disabled for benchmark restart".to_string()
    } else {
        "No cached catalog; scanning library...".to_string()
    };
    LauncherStatusPresenter::new(&bridge).sync_catalog_scan(CatalogScanBridgeStatus::new(
        initial_catalog_scan_visible(
            catalog_ready,
            arcade_catalog_required_at_start,
            catalog_worker_enabled,
            catalog_foreground_update,
        ),
        false,
        catalog_scan_message(catalog_foreground_update),
        catalog_scan_title,
        catalog_scan_detail,
        -1,
    ));
    let bridge_sync_t = Instant::now();
    sync_bridge_launcher(
        &app,
        &pad,
        &nav,
        &setup,
        "",
        "",
        Some(&catalog),
        &mut preview,
        &mut bridge_models,
        catalog_version,
        false,
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
    window.request_redraw();
    let run_start = if arcade_catalog_required_at_start && catalog_ready {
        Instant::now()
    } else {
        start
    };
    launcher_bench_next_step = run_start;
    let preview_scroll_exit_at = preview_scroll_exit_after_trace_deadline(run_start);
    let mut first_render_logged = false;
    let mut first_vsync_logged = false;
    let mut frame_accounting = LauncherFrameAccounting::new(run_start);
    let mut catalog_scan_redraw = CatalogScanRedraw::new();
    let mut games_found_counter = GamesFoundCounter::default();
    let mut bootstrap_counter_climb_logged = false;
    let mut bootstrap_counter_sustained_climb_logged = false;
    let mut full_scan_counter_climb_logged = false;
    let mut catalog_refresh_failed = false;
    let mut route_reassert_count = 0u64;
    let mut last_route_reassert_frame = 0u64;
    let mut last_route_reassert_ok = false;
    let mut last_route_reassert_error = String::new();
    while (secs == 0 || run_start.elapsed().as_secs() < secs)
        && preview_scroll_exit_at.is_none_or(|deadline| Instant::now() < deadline)
    {
        let loop_start = Instant::now();
        let launching = launcher::launch_in_progress() || !loading_title.is_empty();
        let setup_active = setup.is_active();
        let mut light_bridge_dirty = false;
        let mut full_bridge_dirty = false;
        let mut route_action = FramebufferRouteAction {
            reassert_route: false,
            force_full_present: false,
        };
        let defer_selected_preview = false;
        if first_render_logged && media_catalog_seed_pending {
            media_catalog_seed_pending = false;
            ensure_media_for_catalog_systems(
                &catalog,
                &mut media_handle,
                &mut media_worker_unavailable,
                start,
            );
        }
        if !launching {
            route_action = route_guard.tick(frames);
            if route_action.reassert_route {
                match f.fb_enable_format(
                    0,
                    ui.fb_w() as u16,
                    ui.fb_h() as u16,
                    ui_fpga_scaled_mode(ui.scan_w(), ui.scan_h()),
                    Some(0),
                    Some(0),
                    ui.direct_video(),
                    FramebufferFormat::production_default(),
                ) {
                    Ok(flag) => {
                        route_reassert_count = route_reassert_count.saturating_add(1);
                        last_route_reassert_frame = frames;
                        last_route_reassert_ok = true;
                        last_route_reassert_error.clear();
                        boot_analytics::event(
                            "launcher_fb_route_reasserted",
                            format!("frame={frames} support_flag={flag}"),
                        );
                    }
                    Err(e) => {
                        eprintln!("failed to reassert Slint framebuffer route: {e}");
                        route_action.force_full_present = false;
                        route_reassert_count = route_reassert_count.saturating_add(1);
                        last_route_reassert_frame = frames;
                        last_route_reassert_ok = false;
                        last_route_reassert_error = e.to_string();
                        boot_analytics::event(
                            "launcher_fb_route_reassert_failed",
                            format!("frame={frames} error={e}"),
                        );
                    }
                }
            }
        }
        if last_clock_update.elapsed() >= Duration::from_secs(1) {
            let clock_text = launcher_clock_text();
            if dirty_opt {
                if clock_text != last_clock_text {
                    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                    bridge.set_clock_text(clock_text.clone().into());
                    last_clock_text = clock_text;
                    light_bridge_dirty = true;
                }
            } else {
                let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                bridge.set_clock_text(clock_text.clone().into());
                last_clock_text = clock_text;
                full_bridge_dirty = true;
            }
            last_clock_update = Instant::now();
        }

        if !catalog_refresh_done && catalog_rx.is_none() {
            let mut start_deferred_worker = false;
            if let Some(deferred) = deferred_catalog_worker.as_mut() {
                if frame_accounting.first_visible_copy_done() {
                    let start_after = *deferred
                        .start_after
                        .get_or_insert_with(|| loop_start + catalog_background_validation_delay());
                    start_deferred_worker = loop_start >= start_after;
                }
            }
            if start_deferred_worker {
                if let Some(deferred) = deferred_catalog_worker.take() {
                    print_startup_event(start, "catalog_worker_start", &deferred.root);
                    catalog_rx = Some(start_library_catalog_worker(
                        deferred.root,
                        deferred.request,
                        CatalogWorkerInitialCache::AlreadyLoadedReady,
                    ));
                }
            }
        }

        if !catalog_refresh_done {
            while let Some(message) = catalog_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                match message {
                    CatalogWorkerMessage::Timing { name, detail } => {
                        print_startup_event(start, &name, detail);
                    }
                    CatalogWorkerMessage::Progress {
                        title,
                        detail,
                        percent,
                    } => {
                        let intent = CatalogProgressUiIntent::from_worker_progress(
                            CatalogWorkerUiContext {
                                catalog_ready,
                                screen: nav.screen,
                                foreground_update: catalog_foreground_update,
                            },
                            title,
                            detail,
                            percent,
                        );
                        if intent.failed {
                            catalog_refresh_failed = true;
                        }
                        if let Some(counter_target) = intent
                            .counter_target
                            .filter(|target| counter_climb_target_is_meaningful(target.target))
                        {
                            let target = counter_target.target;
                            let visible_counter_before = games_found_counter.displayed;
                            if counter_target.phase == CatalogCounterPhase::Bootstrap
                                && !bootstrap_counter_climb_logged
                            {
                                bootstrap_counter_climb_logged = true;
                                print_startup_event(
                                    start,
                                    "bootstrap_counter_climb",
                                    format!("target={target}"),
                                );
                            }
                            if counter_target.phase == CatalogCounterPhase::Bootstrap
                                && !bootstrap_counter_sustained_climb_logged
                                && counter_climb_target_is_sustained(target)
                            {
                                bootstrap_counter_sustained_climb_logged = true;
                                print_startup_event(
                                    start,
                                    "bootstrap_counter_sustained_climb",
                                    format!("target={target}"),
                                );
                            }
                            if counter_target.phase == CatalogCounterPhase::FullScan
                                && !full_scan_counter_climb_logged
                                && counter_climb_target_overtakes_visible(
                                    target,
                                    visible_counter_before,
                                )
                            {
                                full_scan_counter_climb_logged = true;
                                print_startup_event(
                                    start,
                                    "full_scan_counter_climb",
                                    format!("target={target}"),
                                );
                            }
                        }
                        let detail = games_found_counter.progress_detail(
                            &intent.title,
                            &intent.detail,
                            loop_start,
                        );
                        apply_launcher_worker_ui_intent(
                            &app,
                            intent.ui_with_detail(detail),
                            &mut full_bridge_dirty,
                        );
                    }
                    CatalogWorkerMessage::SystemDiscovered { system_id } => {
                        print_startup_event(
                            start,
                            "catalog_system_discovered",
                            format!("system={system_id}"),
                        );
                        if media_handle.is_none() && !media_worker_unavailable {
                            media_handle = start_screenshot_media_worker();
                            if media_handle.is_some() {
                                print_startup_event(
                                    start,
                                    "screenshot_media_worker_start",
                                    "mode=discovered-system",
                                );
                            } else {
                                media_worker_unavailable = true;
                                print_startup_event(
                                    start,
                                    "screenshot_media_worker_skip",
                                    "mode=discovered-system",
                                );
                            }
                        }
                        if let Some(handle) = media_handle.as_ref() {
                            handle.ensure_system(&system_id);
                        }
                    }
                    CatalogWorkerMessage::Ready {
                        catalog: ready_catalog,
                        summary,
                        load_us,
                    } => {
                        let cached_before_refresh = summary.is_none();
                        let duplicate_cached_catalog =
                            duplicate_cached_catalog_ready(catalog_ready, cached_before_refresh);
                        catalog_refresh_done = !cached_before_refresh;
                        if !duplicate_cached_catalog {
                            catalog = ready_catalog;
                            catalog_version = catalog_version.wrapping_add(1);
                            catalog_ready = true;
                            media_catalog_seed_pending = true;
                            apply_forced_arcade_selected(&mut nav, &catalog);
                            apply_pending_launch_return_state(
                                &mut nav,
                                &catalog,
                                &mut pending_launch_return_state,
                            );
                            print_startup_event(
                                start,
                                "library_ready",
                                format!("games={} load_us={load_us}", catalog.len()),
                            );
                        }
                        if let Some(summary) = summary {
                            if media_catalog_seed_pending {
                                media_catalog_seed_pending = false;
                                ensure_media_for_catalog_systems(
                                    &catalog,
                                    &mut media_handle,
                                    &mut media_worker_unavailable,
                                    start,
                                );
                            }
                            if let Some(handle) = media_handle.as_ref() {
                                handle.finish();
                            }
                            catalog_foreground_update = false;
                            catalog_refresh_failed = false;
                            let event = if summary.skipped {
                                "library_db_unchanged"
                            } else {
                                "library_db_saved"
                            };
                            if !catalog_persisted_summary_seen {
                                print_startup_event(
                                    start,
                                    event,
                                    format_library_refresh_summary(&summary),
                                );
                            }
                        }
                        if duplicate_cached_catalog {
                            if catalog_refresh_failed || catalog_foreground_update {
                                catalog_refresh_done = true;
                                catalog_foreground_update = false;
                                apply_launcher_worker_ui_intent(
                                    &app,
                                    LauncherWorkerUiIntent::ClearCatalogScan,
                                    &mut full_bridge_dirty,
                                );
                                games_found_counter.reset();
                                nav.confirm_action =
                                    Some(launcher::ConfirmAction::LibraryUpdateFailed);
                                nav.confirm_selected = 0;
                                print_startup_event(
                                    start,
                                    "library_rebuild_fallback_catalog_ready",
                                    format!("games={} load_us={load_us}", catalog.len()),
                                );
                                full_bridge_dirty = true;
                            }
                            continue;
                        }
                        games_found_counter.reset();
                        if cached_before_refresh {
                            apply_launcher_worker_ui_intent(
                                &app,
                                cached_catalog_validation_intent(
                                    catalog_foreground_update,
                                    catalog.len(),
                                ),
                                &mut full_bridge_dirty,
                            );
                        } else {
                            apply_launcher_worker_ui_intent(
                                &app,
                                LauncherWorkerUiIntent::ClearCatalogScan,
                                &mut full_bridge_dirty,
                            );
                        }
                        let bridge_sync_t = Instant::now();
                        sync_bridge_launcher(
                            &app,
                            &pad,
                            &nav,
                            &setup,
                            &loading_title,
                            "",
                            Some(&catalog),
                            &mut preview,
                            &mut bridge_models,
                            catalog_version,
                            false,
                        );
                        print_startup_event(
                            start,
                            "catalog_bridge_sync_update",
                            format!(
                                "games={} elapsed_us={}",
                                catalog.len(),
                                bridge_sync_t.elapsed().as_micros()
                            ),
                        );
                    }
                    CatalogWorkerMessage::Persisted { summary } => {
                        catalog_persisted_summary_seen = true;
                        print_startup_event(
                            start,
                            "library_db_saved",
                            format_library_refresh_summary(&summary),
                        );
                    }
                    CatalogWorkerMessage::PersistenceFailed { error } => {
                        catalog_refresh_done = true;
                        catalog_foreground_update = false;
                        catalog_refresh_failed = true;
                        if let Some(handle) = media_handle.as_ref() {
                            handle.finish();
                        }
                        print_startup_event(start, "library_db_save_failed", error);
                        apply_launcher_worker_ui_intent(
                            &app,
                            LauncherWorkerUiIntent::HideCatalogBackgroundScan,
                            &mut full_bridge_dirty,
                        );
                    }
                    CatalogWorkerMessage::Unchanged { summary } => {
                        catalog_refresh_done = true;
                        catalog_foreground_update = false;
                        catalog_refresh_failed = false;
                        if let Some(handle) = media_handle.as_ref() {
                            handle.finish();
                        }
                        print_startup_event(
                            start,
                            "library_db_unchanged",
                            format!(
                                "bytes={} scan_us={} discover_us={} classify_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
                                summary.bytes,
                                summary.scan_us,
                                summary.discover_us,
                                summary.classify_us,
                                summary.import_us,
                                summary.discoveries,
                                summary.normal_files,
                                summary.containers,
                                summary.entries
                            ),
                        );
                        apply_launcher_worker_ui_intent(
                            &app,
                            LauncherWorkerUiIntent::ClearCatalogScan,
                            &mut full_bridge_dirty,
                        );
                        games_found_counter.reset();
                    }
                    CatalogWorkerMessage::Changed { detail } => {
                        catalog_refresh_done = true;
                        catalog_foreground_update = false;
                        catalog_refresh_failed = false;
                        if let Some(handle) = media_handle.as_ref() {
                            handle.finish();
                        }
                        print_startup_event(start, "library_changed_detected", &detail);
                        nav.confirm_action = Some(launcher::ConfirmAction::LibraryChanged);
                        nav.confirm_selected = 0;
                        apply_launcher_worker_ui_intent(
                            &app,
                            LauncherWorkerUiIntent::ClearCatalogScan,
                            &mut full_bridge_dirty,
                        );
                        games_found_counter.reset();
                    }
                    CatalogWorkerMessage::Done => {
                        catalog_refresh_done = true;
                        catalog_foreground_update = false;
                        catalog_refresh_failed = false;
                        if let Some(handle) = media_handle.as_ref() {
                            handle.finish();
                        }
                        if catalog_ready {
                            apply_launcher_worker_ui_intent(
                                &app,
                                LauncherWorkerUiIntent::ClearCatalogScan,
                                &mut full_bridge_dirty,
                            );
                            games_found_counter.reset();
                        }
                    }
                }
            }
        }

        while let Some(message) = media_handle.as_ref().and_then(|handle| handle.try_recv()) {
            match message {
                MediaWorkerMessage::Timing { name, detail } => {
                    print_startup_event(start, &name, detail);
                }
                MediaWorkerMessage::Progress(event) => {
                    print_startup_event(start, "screenshot_media_progress", event.log_detail());
                    apply_launcher_worker_ui_intent(
                        &app,
                        media_progress_display.progress_intent(&event),
                        &mut full_bridge_dirty,
                    );
                }
                MediaWorkerMessage::CacheMetadata { scope, metadata } => {
                    print_startup_event(
                        start,
                        "screenshot_media_cache_metadata",
                        metadata.log_detail(&scope),
                    );
                }
                MediaWorkerMessage::PackStatus {
                    system,
                    image_size,
                    status,
                    detail,
                } => {
                    print_startup_event(
                        start,
                        "screenshot_media_pack_status",
                        format!("system={system} image_size={image_size} status={status} {detail}"),
                    );
                }
                MediaWorkerMessage::Failed { detail } => {
                    print_startup_event(start, "screenshot_media_update_failed", detail);
                    media_worker_unavailable = true;
                    apply_launcher_worker_ui_intent(
                        &app,
                        media_progress_display.clear_intent(),
                        &mut full_bridge_dirty,
                    );
                    media_handle = None;
                    break;
                }
                MediaWorkerMessage::Done { detail } => {
                    print_startup_event(start, "screenshot_media_update_done", detail);
                    apply_launcher_worker_ui_intent(
                        &app,
                        media_progress_display.clear_intent(),
                        &mut full_bridge_dirty,
                    );
                    media_handle = None;
                    break;
                }
            }
        }

        if let Some(scenario) = launcher_bench_scenario {
            if catalog_ready && launcher_bench_waiting_for_initial_preview {
                let cache_state = preview.trace_cache_state();
                if launcher_bench_initial_preview_ready(scenario, cache_state) {
                    launcher_bench_waiting_for_initial_preview = false;
                    launcher_bench_next_step = Instant::now();
                    print_startup_event(
                        start,
                        "launcher_bench_preview_ready",
                        format!("cache_state={cache_state}"),
                    );
                }
            }
            if catalog_ready
                && !launcher_bench_waiting_for_initial_preview
                && launcher_bench_next_step.elapsed() >= scenario.period()
            {
                let before = LauncherBridgeKey::from_nav(&nav);
                if launcher_bench_step(
                    scenario,
                    &mut nav,
                    &catalog,
                    None,
                    launcher_bench_step_idx,
                    Instant::now(),
                ) {
                    let after = LauncherBridgeKey::from_nav(&nav);
                    if before != after {
                        if !dirty_opt || before.screen != after.screen {
                            full_bridge_dirty = true;
                        } else {
                            light_bridge_dirty = true;
                        }
                    }
                }
                launcher_bench_step_idx = launcher_bench_step_idx.wrapping_add(1);
                launcher_bench_next_step = Instant::now();
            }
        }

        if let Some(screen) = lock_screen {
            nav.screen = screen;
        }

        if !launching {
            let pad_changed = pad.poll_with_debug_labels(setup_active);
            let frame_now = Instant::now();
            let state = pad.state();
            let active_idx = pad.active_idx();
            let info = pad.info();

            if setup_active && setup.target_pad_idx >= pad.len() {
                eprintln!(
                    "controller setup: pad {} disappeared; closing setup flow",
                    setup.target_pad_idx
                );
                setup.advance_to_next_pad(&pad);
                full_bridge_dirty = true;
            }

            if launcher_bench_scenario.is_none() && setup.is_active() {
                let setup_before = SetupBridgeKey::from_setup(&setup);
                let setup_info = pad.info_at(setup.target_pad_idx);
                match setup.handle_input(&state, frame_now, setup_info, pad.db()) {
                    SetupAction::None => {}
                    SetupAction::RegisterNew => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.register_new_at(idx) {
                            eprintln!("controller setup: register new: {e}");
                        }
                    }
                    SetupAction::ClaimExisting { list_index } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.claim_existing_at(idx, list_index) {
                            eprintln!("controller setup: claim existing: {e}");
                        }
                    }
                    SetupAction::SaveFinish { label, kind } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.finish_setup_at(idx, label, kind) {
                            eprintln!("controller setup: save: {e}");
                        } else {
                            eprintln!(
                                "controller setup: saved \"{}\" ({})",
                                pad.db().display_label(pad.info_at(idx)),
                                kind.as_str()
                            );
                        }
                        setup.advance_to_next_pad(&pad);
                    }
                    SetupAction::Done => {
                        setup.advance_to_next_pad(&pad);
                    }
                }
                let setup_after = SetupBridgeKey::from_setup(&setup);
                full_bridge_dirty |= pad_changed || setup_before != setup_after;
            } else if launcher_bench_scenario.is_none() {
                if AUTO_CONTROLLER_SETUP_ENABLED && pad_changed {
                    let setup_before = SetupBridgeKey::from_setup(&setup);
                    setup.maybe_open(info, active_idx, pad.db(), true);
                    full_bridge_dirty |= setup_before != SetupBridgeKey::from_setup(&setup);
                }
                if !setup.is_active() {
                    let nav_before = LauncherBridgeKey::from_nav(&nav);
                    if transition_picker_enabled && nav.screen == Screen::Arcade {
                        let left = state.dpad_left && !transition_picker_prev_left;
                        let right = state.dpad_right && !transition_picker_prev_right;
                        let changed = if left {
                            preview_transition.cycle_picker(-1)
                        } else if right {
                            preview_transition.cycle_picker(1)
                        } else {
                            false
                        };
                        if changed {
                            println!(
                                "preview_transition_picker={}",
                                preview_transition
                                    .current_label(frame_now.duration_since(run_start))
                            );
                            window.request_redraw();
                        }
                    }
                    transition_picker_prev_left = state.dpad_left;
                    transition_picker_prev_right = state.dpad_right;
                    let test_library_changed_event = if library_changed_test_action_armed {
                        library_changed_test_event(
                            &nav,
                            library_changed_test_action,
                            &mut library_changed_test_dialog_seen_at,
                            loop_start,
                            start,
                        )
                        .inspect(|_| {
                            library_changed_test_action_armed = false;
                        })
                    } else {
                        None
                    };
                    let event = if test_library_changed_event.is_some() {
                        test_library_changed_event
                    } else if auto_launch_selected
                        && !auto_launch_selected_done
                        && catalog_ready
                        && nav.screen == Screen::Arcade
                    {
                        auto_launch_selected_done = true;
                        active_system(&catalog, &nav)
                            .and_then(|system| {
                                catalog.system_game_at(&system.id, nav.arcade.selected)
                            })
                            .map(|game| launcher::LauncherEvent {
                                action: LauncherAction::LaunchGame,
                                path: Some(game.mra_path.to_string()),
                            })
                    } else {
                        nav.handle_input(&state, frame_now, &catalog)
                    };
                    if let Some(event) = event {
                        match event.action {
                            LauncherAction::ExitToMister => {
                                loading_title = "Exit to MiSTer".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Return to MiSTer MagiK after reboot",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                match launcher::exit_to_mister() {
                                    Ok(()) => std::process::exit(0),
                                    Err(e) => {
                                        eprintln!("exit to MiSTer failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::ResetDatabase => {
                                loading_title = "Shutting down…".to_string();
                                if let Some(handle) = media_handle.as_ref() {
                                    handle.finish();
                                }
                                media_worker_unavailable = true;
                                media_handle = None;
                                sync_launcher_worker_ui_intent(
                                    &app,
                                    media_progress_display.clear_intent(),
                                );
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Restarting MiSTer",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                std::thread::sleep(Duration::from_millis(250));
                                match launcher::reset_database_and_reboot() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("reset database failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::Restart => {
                                loading_title = "Shutting down…".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Restarting MiSTer",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                std::thread::sleep(Duration::from_millis(250));
                                match launcher::reboot_mister() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("restart failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::ContinueWithStaleLibrary => {
                                match launcher::request_library_rebuild_on_next_boot() {
                                    Ok(()) => print_startup_event(
                                        start,
                                        "library_rebuild_deferred",
                                        "marker=written",
                                    ),
                                    Err(e) => {
                                        eprintln!("failed to defer library rebuild: {e}");
                                        print_startup_event(
                                            start,
                                            "library_rebuild_defer_failed",
                                            e,
                                        );
                                    }
                                }
                                catalog_refresh_done = true;
                                catalog_foreground_update = false;
                                deferred_catalog_worker = None;
                                apply_launcher_worker_ui_intent(
                                    &app,
                                    LauncherWorkerUiIntent::ClearCatalogScan,
                                    &mut full_bridge_dirty,
                                );
                                games_found_counter.reset();
                                catalog_refresh_failed = false;
                                window.request_redraw();
                                continue;
                            }
                            LauncherAction::RebuildLibrary => {
                                print_startup_event(
                                    start,
                                    "library_rebuild_requested",
                                    "source=dialog",
                                );
                                catalog_refresh_done = false;
                                catalog_foreground_update = true;
                                deferred_catalog_worker = None;
                                games_found_counter.reset();
                                catalog_refresh_failed = false;
                                bootstrap_counter_climb_logged = false;
                                bootstrap_counter_sustained_climb_logged = false;
                                full_scan_counter_climb_logged = false;
                                catalog_rx = Some(start_library_catalog_worker(
                                    arcade_root.clone(),
                                    CatalogWorkerRequest::ForceBuild,
                                    CatalogWorkerInitialCache::AlreadyLoadedReady,
                                ));
                                apply_launcher_worker_ui_intent(
                                    &app,
                                    catalog_rebuild_started_intent(catalog_foreground_update),
                                    &mut full_bridge_dirty,
                                );
                                window.request_redraw();
                                continue;
                            }
                            LauncherAction::LaunchGame => {}
                        }
                        let Some(mra) = event.path else {
                            continue;
                        };
                        loading_title =
                            format!("Loading {}…", launcher::game_title(&catalog, &mra));
                        sync_bridge_launcher(
                            &app,
                            &pad,
                            &nav,
                            &setup,
                            &loading_title,
                            "",
                            Some(&catalog),
                            &mut preview,
                            &mut bridge_models,
                            catalog_version,
                            false,
                        );
                        window.request_redraw();
                        update_slint_animations(animation_clock);
                        window.draw_if_needed(|renderer| {
                            let region = target.render(renderer, ui);
                            let _ = region;
                        });
                        let _pace = pacer.wait();
                        target.present_rows(f, disp, ui, 0, ui.render_h());
                        if let Some(state) =
                            launcher::capture_launch_return_state(&nav, &catalog, &mra)
                        {
                            if let Err(e) = launcher::save_launch_return_state(&state) {
                                eprintln!("failed to save launch return state: {e}");
                            }
                        }
                        match launcher::execute_game_launch(&mra) {
                            Ok(spawned) => {
                                launch_started = Instant::now();
                                launch_spawned_mister = spawned;
                            }
                            Err(e) => {
                                launcher::remove_launch_return_state();
                                eprintln!("game launch failed: {e}");
                                launch_spawned_mister |= e.spawned_mister();
                                loading_title.clear();
                                launcher::reset_launch();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    "",
                                    "",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                    false,
                                );
                                recover_launcher_ui(f, ui, &mut launch_spawned_mister);
                            }
                        }
                        window.request_redraw();
                    }
                    let nav_after = LauncherBridgeKey::from_nav(&nav);
                    if pad_changed && nav.screen == Screen::Controller {
                        full_bridge_dirty = true;
                    } else if pad_changed && !dirty_opt {
                        full_bridge_dirty = true;
                    }
                    if nav_before != nav_after {
                        if !dirty_opt || nav_before.screen != nav_after.screen {
                            full_bridge_dirty = true;
                        } else {
                            light_bridge_dirty = true;
                        }
                    }
                }
            }

            if let Some(screen) = lock_screen {
                nav.screen = screen;
            }

            if full_bridge_dirty {
                sync_bridge_launcher(
                    &app,
                    &pad,
                    &nav,
                    &setup,
                    &loading_title,
                    "",
                    Some(&catalog),
                    &mut preview,
                    &mut bridge_models,
                    catalog_version,
                    defer_selected_preview,
                );
                window.request_redraw();
            } else if light_bridge_dirty {
                let active_games = if nav.screen == Screen::Arcade {
                    Some(active_system_game_slice(&catalog, &nav))
                } else {
                    None
                };
                sync_bridge_launcher_light(
                    &app,
                    &nav,
                    &setup,
                    &loading_title,
                    "",
                    &catalog,
                    active_games,
                    &mut preview,
                    defer_selected_preview,
                );
                window.request_redraw();
            }
        } else {
            let _ = pad.poll();
            if launcher::mister_running_arcade_core()
                && launch_started.elapsed() > Duration::from_millis(500)
            {
                println!("arcade core running — handing off to MiSTer");
                std::process::exit(0);
            } else if launch_started.elapsed() > Duration::from_secs(90) {
                eprintln!("game launch timed out");
                recover_launcher_ui(f, ui, &mut launch_spawned_mister);
                std::process::exit(1);
            }
        }

        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
        let catalog_scan_visible = bridge.get_catalog_scan_visible();
        let catalog_scan_percent = bridge.get_catalog_scan_percent();
        let catalog_background_scan_visible = bridge.get_catalog_background_scan_visible();
        let catalog_scan_message = bridge.get_catalog_scan_message().to_string();
        let confirm_visible = bridge.get_confirm_visible();
        let confirm_title = bridge.get_confirm_title().to_string();
        let confirm_selected = bridge.get_confirm_selected();
        let confirm_left_label = bridge.get_confirm_left_label().to_string();
        let confirm_right_label = bridge.get_confirm_right_label().to_string();
        let status_write_due = frame_accounting.status_write_due();
        let status_string_copy_start = (status_write_due
            && frame_accounting.preview_scroll_trace_enabled())
        .then(Instant::now);
        let status_catalog_scan_text = status_write_due.then(|| {
            (
                bridge.get_catalog_scan_title().to_string(),
                bridge.get_catalog_scan_detail().to_string(),
            )
        });
        let status_string_copy_us = status_string_copy_start
            .map(|start| start.elapsed().as_micros())
            .unwrap_or(0);
        let status_string_copy_bytes = status_catalog_scan_text
            .as_ref()
            .map(|(title, detail)| title.len() + detail.len())
            .unwrap_or(0);
        let games_found_detail_changed = if catalog_scan_visible && catalog_scan_percent < 0 {
            games_found_counter.tick(loop_start).is_some_and(|detail| {
                LauncherStatusPresenter::new(&bridge).sync_catalog_scan_detail(detail);
                true
            })
        } else {
            false
        };
        if launching
            || games_found_detail_changed
            || catalog_scan_redraw.should_request(
                catalog_scan_visible,
                catalog_background_scan_visible,
                catalog_scan_percent,
                loop_start,
            )
        {
            window.request_redraw();
        }
        let active_arcade_games = if !launching && nav.screen == Screen::Arcade {
            active_system_game_slice(&catalog, &nav)
        } else {
            &[]
        };
        if dirty_opt && !launching && nav.screen == Screen::Arcade {
            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
            if schedule_arcade_preview_window(
                &bridge,
                active_arcade_games,
                nav.arcade.selected,
                &mut preview,
                defer_selected_preview,
            ) {
                window.request_redraw();
            }
        }
        if !launching && apply_ready_preview(&app, &mut preview, defer_selected_preview) {
            window.request_redraw();
        }

        let frame_t0 = Instant::now();
        let prepare_us = (frame_t0 - loop_start).as_micros();
        update_slint_animations(animation_clock);
        let frame_t1 = Instant::now();
        let mut this_rect: Option<DirtyRect> = None;
        window.draw_if_needed(|renderer| {
            let region = target.render(renderer, ui);
            this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        let frame_t2 = Instant::now();
        let custom_draw_start = Instant::now();
        let full_frame_present = should_present_full_frame(launching, route_action);
        let arcade_list_rect = if !launching && nav.screen == Screen::Arcade {
            let force_arcade_redraw =
                arcade_list_needs_forced_redraw(this_rect, full_frame_present);
            arcade_list_renderer.draw(
                active_arcade_games,
                nav.arcade.visual_index,
                force_arcade_redraw,
            )
        } else {
            None
        };
        let (raw_preview_rect, preview_transition_trace) = blit_raw_preview_if_needed(
            target,
            ui,
            &mut preview,
            &mut preview_transition,
            loop_start.duration_since(run_start),
            this_rect,
        );
        if preview_transition_trace.active {
            window.request_redraw();
        }
        let effect_label_rect = effect_label_overlay
            .as_mut()
            .map(|overlay| overlay.draw(target, ui, preview_transition_trace.effect.label()));
        let custom_draw_done = Instant::now();
        if !first_render_logged {
            first_render_logged = true;
            boot_analytics::event(
                "first_render",
                format!("frame={frames} dirty_rect={}", format_dirty_rect(this_rect)),
            );
        }
        let pace = if frame_accounting.first_visible_copy_done() {
            let pace = pacer.wait();
            let frame_t3 = Instant::now();
            (Some(pace), frame_t3)
        } else {
            (None, Instant::now())
        };
        let frame_t3 = pace.1;
        let vsync_source = pace.0.as_ref().map(|pace| pace.source);
        let vsync_period_us = pace
            .0
            .as_ref()
            .map(|pace| pace.period_us)
            .unwrap_or_else(|| pacer.period_us());
        let vsync_miss_streak = pace.0.as_ref().map(|pace| pace.miss_streak).unwrap_or(0);
        if !first_vsync_logged
            && pace
                .0
                .as_ref()
                .is_some_and(|p| p.source == VsyncPaceSource::Vsync)
        {
            first_vsync_logged = true;
            boot_analytics::event("first_vsync", format!("frame={frames}"));
        }
        let mut copied_rows = 0u32;
        let mut cached_present_frame_us = 0u128;
        let arcade_update_label = ArcadeUpdateTrace::from_update(arcade_list_rect.as_ref());
        let arcade_overlay_rect = arcade_list_rect.as_ref().map(arcade_update_dirty_rect);
        let cached_base_rect = if full_frame_present {
            Some(DirtyRect {
                x0: 0,
                y0: 0,
                x1: ui.render_w(),
                y1: ui.render_h(),
            })
        } else {
            this_rect
        };
        let mut cached_overlays = DirtyRectList::new();
        cached_overlays.push_if_some(raw_preview_rect);
        cached_overlays.push_if_some(effect_label_rect);
        let mut direct_overlays = DirtyRectList::new();
        direct_overlays.push_if_some(arcade_overlay_rect);
        let cached_present_rects =
            build_launcher_present_plan(cached_base_rect, &cached_overlays, &direct_overlays);
        for rect in cached_present_rects.iter() {
            let cached_copy_start = Instant::now();
            copied_rows += target.present_rect(f, disp, ui, rect);
            cached_present_frame_us += cached_copy_start.elapsed().as_micros();
        }
        let mut overlay_present_frame_us = 0u128;
        if let Some(update) = arcade_list_rect {
            let overlay_copy_start = Instant::now();
            copied_rows +=
                copy_arcade_list_update(target, disp, ui, &mut arcade_list_renderer, update);
            overlay_present_frame_us = overlay_copy_start.elapsed().as_micros();
        }
        let mut present_probe_frame_us = 0u128;
        if let Some(probe) = present_probe.as_mut() {
            let probe_copy_start = Instant::now();
            copied_rows += probe.present(disp, frames);
            present_probe_frame_us = probe_copy_start.elapsed().as_micros();
        }
        let frame_t4 = Instant::now();
        frame_accounting.finish_frame(
            LauncherPresentedFrame {
                frames,
                selected: nav.arcade.selected,
                visual_index: nav.arcade.visual_index,
                run_start,
                loop_start,
                frame_t0,
                frame_t1,
                frame_t2,
                frame_t3,
                frame_t4,
                custom_draw_start,
                custom_draw_done,
                prepare_us,
                dirty_rect: this_rect,
                copied_rows,
                cached_present_us: cached_present_frame_us,
                overlay_present_us: overlay_present_frame_us,
                present_probe_us: present_probe_frame_us,
                vsync_source,
                vsync_period_us,
                vsync_miss_streak,
                arcade_update_label,
                preview_cache_state: preview.trace_cache_state(),
                preview_transition: preview_transition_trace,
                status_write_due,
                status_string_copy_us,
                status_string_copy_bytes,
            },
            start,
            disp,
            &nav,
            &pad,
            &catalog,
            catalog_ready,
            catalog_refresh_done,
            launching,
            &loading_title,
            catalog_scan_visible,
            status_catalog_scan_text
                .as_ref()
                .map(|(title, _)| title.as_str())
                .unwrap_or(""),
            status_catalog_scan_text
                .as_ref()
                .map(|(_, detail)| detail.as_str())
                .unwrap_or(""),
            catalog_scan_percent,
            catalog_background_scan_visible,
            &catalog_scan_message,
            confirm_visible,
            &confirm_title,
            confirm_selected,
            &confirm_left_label,
            &confirm_right_label,
            launcher_bench_scenario,
            start_screen,
            lock_screen,
            route_reassert_count,
            last_route_reassert_frame,
            last_route_reassert_ok,
            &last_route_reassert_error,
        );
        frames += 1;
    }
    let elapsed = run_start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Err(e) = cpu_profile::finish(cpu) {
        eprintln!("{e}");
    }
}

fn preview_scroll_exit_after_trace_deadline(run_start: Instant) -> Option<Instant> {
    if !matches!(
        std::env::var("MISTER_PREVIEW_SCROLL_EXIT_AFTER_TRACE").as_deref(),
        Ok("1") | Ok("on") | Ok("true") | Ok("yes")
    ) {
        return None;
    }
    let secs = std::env::var("MISTER_PREVIEW_SCROLL_TRACE_SECS")
        .ok()?
        .parse::<u64>()
        .ok()?;
    (secs > 0).then(|| run_start + Duration::from_secs(secs))
}

fn launcher_auto_launch_selected_enabled() -> bool {
    matches!(
        std::env::var("MISTER_LAUNCHER_AUTO_LAUNCH_SELECTED")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn apply_pending_launch_return_state(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    pending: &mut Option<launcher::LaunchReturnState>,
) -> bool {
    let Some(state) = pending.take() else {
        return false;
    };
    launcher::apply_launch_return_state(nav, catalog, state)
}

fn ensure_media_for_catalog_systems(
    catalog: &ArcadeCatalog,
    media_handle: &mut Option<MediaWorkerHandle>,
    media_worker_unavailable: &mut bool,
    start: Instant,
) {
    let systems = catalog_media_system_ids(catalog);
    if systems.is_empty() {
        return;
    }
    if media_handle.is_none() && !*media_worker_unavailable {
        *media_handle = start_screenshot_media_worker();
        if media_handle.is_some() {
            print_startup_event(
                start,
                "screenshot_media_worker_start",
                format!("mode=catalog-systems systems={}", systems.len()),
            );
        } else {
            *media_worker_unavailable = true;
            print_startup_event(
                start,
                "screenshot_media_worker_skip",
                "mode=catalog-systems",
            );
        }
    }
    let Some(handle) = media_handle.as_ref() else {
        return;
    };
    for system_id in systems {
        print_startup_event(
            start,
            "screenshot_media_catalog_ensure",
            format!("system={system_id}"),
        );
        handle.ensure_system(&system_id);
    }
}

fn catalog_media_system_ids(catalog: &ArcadeCatalog) -> Vec<String> {
    let mut seen = BTreeSet::new();
    catalog
        .systems
        .iter()
        .filter_map(|system| {
            let id = system.id.as_str();
            (mister_magik_fb::media_update::is_supported_pack_id(id)
                && catalog.system_game_count(id) > 0
                && seen.insert(system.id.clone()))
            .then(|| system.id.clone())
        })
        .collect()
}

#[derive(Debug)]
struct CatalogScanRedraw {
    last_request: Instant,
    period: Duration,
}

impl CatalogScanRedraw {
    fn new() -> Self {
        Self {
            last_request: Instant::now() - catalog_scan_redraw_period(),
            period: catalog_scan_redraw_period(),
        }
    }

    fn should_request(
        &mut self,
        visible: bool,
        background_visible: bool,
        percent: i32,
        now: Instant,
    ) -> bool {
        if !visible && !background_visible {
            return false;
        }
        if visible && !background_visible && percent >= 0 {
            return false;
        }
        if now.duration_since(self.last_request) < self.period {
            return false;
        }
        self.last_request = now;
        true
    }
}

#[derive(Debug, Default)]
struct GamesFoundCounter {
    displayed: usize,
    target: usize,
    active: bool,
    last_tick: Option<Instant>,
    phase: Option<CatalogCounterPhase>,
}

impl GamesFoundCounter {
    fn progress_detail(&mut self, title: &str, detail: &str, now: Instant) -> Option<String> {
        let phase = CatalogCounterPhase::for_title(title);
        let target = phase.and_then(|_| parse_games_found_detail(detail));
        let Some(target) = target else {
            self.reset();
            return None;
        };
        let phase = phase.expect("phase exists when target parses");
        let target = match phase {
            CatalogCounterPhase::Bootstrap if target >= 500 => target.max(1000),
            _ => target,
        };
        if phase == CatalogCounterPhase::FullScan && target <= self.displayed {
            return Some(format_games_found(self.displayed));
        }
        if !self.active || target < self.displayed {
            self.displayed = self.displayed.min(target);
            self.last_tick = Some(now);
        }
        self.target = target;
        self.active = true;
        self.phase = Some(phase);
        Some(format_games_found(self.displayed))
    }

    fn tick(&mut self, now: Instant) -> Option<String> {
        if !self.active || self.displayed >= self.target {
            self.last_tick = Some(now);
            return None;
        }
        let elapsed = self
            .last_tick
            .map(|last| now.duration_since(last))
            .unwrap_or(Duration::from_millis(66));
        let step = games_found_count_step(
            self.displayed,
            self.target,
            elapsed,
            self.phase.unwrap_or(CatalogCounterPhase::FullScan),
        );
        if step == 0 {
            return None;
        }
        self.displayed = self.displayed.saturating_add(step).min(self.target);
        self.last_tick = Some(now);
        Some(format_games_found(self.displayed))
    }

    fn reset(&mut self) {
        self.displayed = 0;
        self.target = 0;
        self.active = false;
        self.last_tick = None;
        self.phase = None;
    }
}

fn format_games_found(count: usize) -> String {
    format!("Games found: {count}")
}

fn counter_climb_target_is_meaningful(target: usize) -> bool {
    target >= 50
}

fn counter_climb_target_is_sustained(target: usize) -> bool {
    target >= 500
}

fn counter_climb_target_overtakes_visible(target: usize, displayed: usize) -> bool {
    target > displayed
}

fn games_found_count_step(
    displayed: usize,
    target: usize,
    elapsed: Duration,
    phase: CatalogCounterPhase,
) -> usize {
    if target <= displayed {
        return 0;
    }
    let lag = target - displayed;
    let elapsed_ms = elapsed.as_millis().max(1) as usize;
    if phase == CatalogCounterPhase::Bootstrap {
        let bootstrap_games_per_second = 55usize;
        return ((bootstrap_games_per_second * elapsed_ms).div_ceil(1000)).clamp(1, lag);
    }
    let catchup_ms = 450usize;
    ((lag * elapsed_ms).div_ceil(catchup_ms)).clamp(1, lag)
}

fn catalog_scan_redraw_period() -> Duration {
    let fps = std::env::var("MISTER_CATALOG_SCAN_FPS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(15)
        .clamp(1, 60);
    Duration::from_millis((1000 / fps).max(1))
}

fn catalog_background_validation_delay() -> Duration {
    std::env::var("MISTER_CATALOG_BACKGROUND_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_CATALOG_BACKGROUND_VALIDATION_DELAY)
}

fn library_changed_test_action_from_env(
    start: Instant,
) -> Option<launcher::LibraryChangedTestAction> {
    let value = std::env::var("MISTER_MAGIK_TEST_LIBRARY_CHANGED_ACTION").ok()?;
    match launcher::parse_library_changed_test_action(&value) {
        Ok(action) => action,
        Err(e) => {
            eprintln!("{e}");
            print_startup_event(start, "library_changed_test_action_invalid", e);
            None
        }
    }
}

fn library_changed_test_event(
    nav: &LauncherNav,
    action: Option<launcher::LibraryChangedTestAction>,
    dialog_seen_at: &mut Option<Instant>,
    now: Instant,
    start: Instant,
) -> Option<launcher::LauncherEvent> {
    if nav.confirm_action != Some(launcher::ConfirmAction::LibraryChanged) {
        *dialog_seen_at = None;
        return None;
    }
    let action = action?;
    let seen_at = *dialog_seen_at.get_or_insert(now);
    if now.duration_since(seen_at) < LIBRARY_CHANGED_TEST_ACTION_SETTLE {
        return None;
    }
    print_startup_event(
        start,
        "library_changed_test_action",
        format!("action={}", action.label()),
    );
    launcher::library_changed_test_action_event(nav.confirm_action, action)
}

fn initial_catalog_scan_visible(
    catalog_ready: bool,
    _arcade_catalog_required_at_start: bool,
    catalog_worker_enabled: bool,
    foreground_update: bool,
) -> bool {
    catalog_worker_enabled && (foreground_update || !catalog_ready)
}

fn format_library_refresh_summary(summary: &library_db::LibraryRefreshSummary) -> String {
    format!(
        "bytes={} scan_us={} discover_us={} classify_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
        summary.bytes,
        summary.scan_us,
        summary.discover_us,
        summary.classify_us,
        summary.import_us,
        summary.discoveries,
        summary.normal_files,
        summary.containers,
        summary.entries
    )
}

fn ready_catalog_worker_request(refresh_policy: CatalogRefreshPolicy) -> CatalogWorkerRequest {
    if refresh_policy == CatalogRefreshPolicy::Off {
        CatalogWorkerRequest::LoadOnly
    } else if refresh_policy.force_requested() {
        CatalogWorkerRequest::ForceBuild
    } else {
        CatalogWorkerRequest::CheckStamp
    }
}

fn duplicate_cached_catalog_ready(catalog_ready: bool, cached_before_refresh: bool) -> bool {
    catalog_ready && cached_before_refresh
}

fn launcher_bench_initial_preview_ready(
    scenario: LauncherBenchScenario,
    preview_cache_state: &str,
) -> bool {
    !scenario.starts_on_arcade() || matches!(preview_cache_state, "exact" | "empty")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(mister_experiments)]
    use crate::ui_effect_bench::{EffectFill, EffectTarget};
    #[cfg(mister_experiments)]
    use mister_magik_fb::effects::EffectSize;
    use std::sync::Arc;

    #[cfg(mister_experiments)]
    #[test]
    pub(super) fn effect_half_target_allows_640x448_at_native_scale() {
        let ui = UiDisplay::for_framebuffer(1920, 1080);
        let target = EffectTarget::new(EffectFill::Half, EffectSize { w: 640, h: 448 }, &ui)
            .expect("640x448 should fit in half-fill benchmark mode");

        assert_eq!(target.physical_w, 640);
        assert_eq!(target.physical_h, 448);
        assert_eq!(target.render_w, 640);
        assert_eq!(target.render_h, 448);
        assert_eq!(target.scale, 1);
    }

    #[test]
    pub(super) fn dirty_rect_ignores_fully_negative_bounds() {
        assert_eq!(dirty_rect_from_bounds(-40, 10, 20, 20, 960, 540), None);
        assert_eq!(dirty_rect_from_bounds(10, -40, 20, 20, 960, 540), None);
    }

    #[test]
    pub(super) fn dirty_rect_clips_partially_negative_bounds() {
        assert_eq!(
            dirty_rect_from_bounds(-10, -5, 30, 20, 960, 540),
            Some(DirtyRect {
                x0: 0,
                y0: 0,
                x1: 20,
                y1: 15
            })
        );
    }

    #[test]
    pub(super) fn dirty_rect_ignores_zero_area_bounds() {
        assert_eq!(dirty_rect_from_bounds(10, 10, 0, 20, 960, 540), None);
        assert_eq!(dirty_rect_from_bounds(10, 10, 20, 0, 960, 540), None);
    }

    #[test]
    pub(super) fn dirty_rect_keeps_in_bounds_rect() {
        assert_eq!(
            dirty_rect_from_bounds(10, 20, 30, 40, 960, 540),
            Some(DirtyRect {
                x0: 10,
                y0: 20,
                x1: 40,
                y1: 60
            })
        );
    }

    fn catalog_for_media_systems(system_ids: &[&str]) -> ArcadeCatalog {
        let mut games = Vec::new();
        let mut systems = Vec::new();
        for system_id in system_ids {
            games.push(ArcadeGameEntry {
                title: Arc::from(format!("{system_id} game")),
                mra_path: Arc::from(format!("/media/fat/_Arcade/{system_id}.mra")),
                preview_archive_path: Arc::from(format!(
                    "/media/fat/mister-magik/assets/{system_id}-screenshots.mmlz4b"
                )),
                preview_asset_key: Arc::from(format!("{system_id}.raw565")),
                has_preview: true,
                system_id: Arc::from(*system_id),
                is_new: false,
            });
            systems.push(arcade_catalog::GameSystemEntry {
                id: (*system_id).to_string(),
                title: (*system_id).to_string(),
                count: 1,
            });
        }
        ArcadeCatalog::new(PathBuf::from("/media/fat/_Arcade"), games, systems)
    }

    #[test]
    pub(super) fn catalog_media_system_ids_are_selective_and_supported() {
        let catalog = catalog_for_media_systems(&["arcade", "pcengine", "neogeo", "arcade"]);

        assert_eq!(
            catalog_media_system_ids(&catalog),
            vec!["arcade".to_string(), "neogeo".to_string()]
        );
    }

    #[test]
    pub(super) fn home_boot_with_ready_catalog_hides_catalog_popup() {
        assert!(!initial_catalog_scan_visible(true, false, true, false));
        assert!(initial_catalog_scan_visible(true, false, true, true));
    }

    #[test]
    pub(super) fn library_changed_test_hook_waits_for_dialog_settle() {
        let start = Instant::now();
        let mut nav = LauncherNav::new();
        let mut seen_at = None;

        assert!(library_changed_test_event(
            &nav,
            Some(launcher::LibraryChangedTestAction::Continue),
            &mut seen_at,
            start,
            start,
        )
        .is_none());

        nav.confirm_action = Some(launcher::ConfirmAction::LibraryChanged);
        assert!(library_changed_test_event(
            &nav,
            Some(launcher::LibraryChangedTestAction::Continue),
            &mut seen_at,
            start,
            start,
        )
        .is_none());
        assert!(library_changed_test_event(
            &nav,
            Some(launcher::LibraryChangedTestAction::Continue),
            &mut seen_at,
            start + LIBRARY_CHANGED_TEST_ACTION_SETTLE,
            start,
        )
        .is_some());
    }

    #[test]
    pub(super) fn arcade_bench_waits_for_initial_visible_preview() {
        let scenario = LauncherBenchScenario::HeldScroll;

        assert!(!launcher_bench_initial_preview_ready(
            scenario,
            "placeholder"
        ));
        assert!(!launcher_bench_initial_preview_ready(scenario, "cached"));
        assert!(!launcher_bench_initial_preview_ready(scenario, "stale"));
        assert!(launcher_bench_initial_preview_ready(scenario, "exact"));
        assert!(launcher_bench_initial_preview_ready(scenario, "empty"));
    }

    #[test]
    pub(super) fn non_arcade_bench_does_not_wait_for_preview() {
        assert!(launcher_bench_initial_preview_ready(
            LauncherBenchScenario::HomeNav,
            "placeholder"
        ));
    }

    #[test]
    pub(super) fn missing_catalog_shows_catalog_popup_on_home_or_arcade_boot() {
        assert!(initial_catalog_scan_visible(false, false, true, false));
        assert!(initial_catalog_scan_visible(false, true, true, false));
        assert!(!initial_catalog_scan_visible(true, true, true, false));
        assert!(!initial_catalog_scan_visible(false, true, false, false));
    }

    #[test]
    pub(super) fn ready_catalog_rebuild_progress_uses_background_badge() {
        for title in ["Indexing library", "Loading library"] {
            let full_visible = catalog_scan_progress_visible(true, Screen::Home, title, false);
            assert!(!full_visible, "{title} should not cover a ready catalog");
            assert!(catalog_background_scan_progress_visible(
                true,
                full_visible,
                title
            ));
        }
    }

    #[test]
    pub(super) fn catalog_scan_redraw_throttles_indeterminate_animation() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now,
            period: Duration::from_millis(66),
        };
        assert!(!redraw.should_request(true, false, -1, now + Duration::from_millis(20)));
        assert!(redraw.should_request(true, false, -1, now + Duration::from_millis(70)));
    }

    #[test]
    pub(super) fn catalog_scan_redraw_skips_determinate_periodic_frames() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now - Duration::from_secs(1),
            period: Duration::from_millis(66),
        };
        assert!(!redraw.should_request(true, false, 90, now));
        assert!(!redraw.should_request(false, false, -1, now));
    }

    #[test]
    pub(super) fn catalog_scan_redraw_animates_background_badge() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now - Duration::from_secs(1),
            period: Duration::from_millis(66),
        };
        assert!(redraw.should_request(false, true, -1, now));
    }

    #[test]
    pub(super) fn games_found_counter_eases_toward_real_scan_count() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 250", now),
            Some("Games found: 0".to_string())
        );
        assert_eq!(
            counter.tick(now + Duration::from_millis(66)),
            Some("Games found: 37".to_string())
        );
        assert_eq!(
            counter.progress_detail(
                "Classifying library",
                "Games found: 500",
                now + Duration::from_millis(132)
            ),
            Some("Games found: 37".to_string())
        );
        let next = counter
            .tick(now + Duration::from_millis(198))
            .expect("counter should move after the target increases");
        let first_tick_count = parse_games_found_detail(&next).expect("parse counter detail");
        assert!(first_tick_count > 37);
        assert!(first_tick_count < 500);

        let next = counter
            .tick(now + Duration::from_millis(264))
            .expect("counter should keep moving");
        let count = parse_games_found_detail(&next).expect("parse counter detail");
        assert!(count > first_tick_count);
        assert!(count < 500);
    }

    #[test]
    pub(super) fn counter_climb_metric_waits_for_meaningful_target() {
        assert!(!counter_climb_target_is_meaningful(1));
        assert!(!counter_climb_target_is_meaningful(49));
        assert!(counter_climb_target_is_meaningful(50));
        assert!(counter_climb_target_is_meaningful(250));
        assert!(!counter_climb_target_is_sustained(499));
        assert!(counter_climb_target_is_sustained(500));
    }

    #[test]
    pub(super) fn games_found_counter_accepts_bootstrap_title() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        assert_eq!(
            counter.progress_detail("Finding games", "Games found: 50", now),
            Some("Games found: 0".to_string())
        );
        assert_eq!(
            counter.tick(now + Duration::from_millis(66)),
            Some("Games found: 4".to_string())
        );
    }

    #[test]
    pub(super) fn games_found_counter_uses_slow_bootstrap_target_floor() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        assert_eq!(
            counter.progress_detail("Finding games", "Games found: 911", now),
            Some("Games found: 0".to_string())
        );
        assert_eq!(counter.target, 1000);
        for frame in 1..=20 {
            counter.tick(now + Duration::from_millis(frame * 66));
        }

        assert!(counter.displayed > 50);
        assert!(counter.displayed < 125);
    }

    #[test]
    pub(super) fn games_found_counter_does_not_drop_when_full_scan_starts_lower() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        counter.progress_detail("Finding games", "Games found: 911", now);
        counter.displayed = 650;
        counter.target = 1000;
        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 50", now),
            Some("Games found: 650".to_string())
        );
        assert_eq!(counter.displayed, 650);
        assert_eq!(counter.target, 1000);

        assert_eq!(
            counter.progress_detail("Classifying library", "Games found: 700", now),
            Some("Games found: 650".to_string())
        );
        assert_eq!(counter.target, 700);
        assert_eq!(counter.phase, Some(CatalogCounterPhase::FullScan));
    }

    #[test]
    pub(super) fn full_scan_counter_takeover_requires_visible_overtake() {
        assert!(!counter_climb_target_overtakes_visible(50, 650));
        assert!(!counter_climb_target_overtakes_visible(650, 650));
        assert!(counter_climb_target_overtakes_visible(700, 650));
    }

    #[test]
    pub(super) fn games_found_counter_catches_large_lag_without_overshoot() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        counter.progress_detail("Classifying library", "Games found: 1000", now);
        for frame in 1..20 {
            counter.tick(now + Duration::from_millis(frame * 66));
        }

        assert!(counter.displayed > 900);
        assert!(counter.displayed <= 1000);
    }

    #[test]
    pub(super) fn games_found_counter_ignores_other_scan_phases() {
        let now = Instant::now();
        let mut counter = GamesFoundCounter::default();

        counter.progress_detail("Classifying library", "Games found: 100", now);
        assert_eq!(
            counter.progress_detail(
                "Saving library",
                "Writing 0 of 100 games into SQLite...",
                now
            ),
            None
        );
        assert!(!counter.active);
    }

    #[test]
    pub(super) fn cached_home_validation_progress_stays_hidden() {
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Validating library",
            false
        ));
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Preview images changed",
            false
        ));
        assert!(catalog_background_scan_progress_visible(
            true,
            false,
            "Validating library"
        ));
        assert!(catalog_background_scan_progress_visible(
            true,
            false,
            "Checking library"
        ));
    }

    #[test]
    pub(super) fn missing_catalog_and_rebuild_progress_are_visible() {
        assert!(catalog_scan_progress_visible(
            false,
            Screen::Home,
            "Indexing library",
            false
        ));
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Indexing library",
            true
        ));
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Library changed",
            false
        ));
        assert!(!catalog_background_scan_progress_visible(
            true,
            true,
            "Indexing library"
        ));
    }

    #[test]
    pub(super) fn catalog_scan_failures_are_visible_even_with_cache() {
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Library scan failed",
            false
        ));
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Arcade,
            "Library load failed",
            false
        ));
        assert!(!catalog_background_scan_progress_visible(
            true,
            true,
            "Library scan failed"
        ));
    }

    #[test]
    pub(super) fn ready_catalog_uses_background_worker_for_refresh_or_home_validation() {
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Default),
            CatalogWorkerRequest::CheckStamp
        );
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Force),
            CatalogWorkerRequest::ForceBuild
        );
        assert_eq!(
            ready_catalog_worker_request(CatalogRefreshPolicy::Off),
            CatalogWorkerRequest::LoadOnly
        );
    }

    #[test]
    pub(super) fn duplicate_cached_catalog_ready_is_skipped_after_sync_load() {
        assert!(duplicate_cached_catalog_ready(true, true));
        assert!(!duplicate_cached_catalog_ready(false, true));
        assert!(!duplicate_cached_catalog_ready(true, false));
    }
}
