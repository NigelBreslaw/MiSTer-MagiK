use super::launcher_frame_accounting::{LauncherFrameAccounting, LauncherPresentedFrame};
use super::*;
use mister_magik_fb::framebuffer_ownership::{
    should_present_full_frame, FramebufferRouteAction, FramebufferRouteGuard,
};

pub(super) fn recover_launcher_ui(f: &mut Fpga, ui: &UiDisplay, spawned_mister: &mut bool) {
    if *spawned_mister {
        launcher::stop_mister();
        if let Err(e) = f.fb_enable_format(
            0,
            ui.fb_w() as u16,
            ui.fb_h() as u16,
            ui_fpga_scaled_mode(),
            Some(0),
            Some(0),
            std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
            FramebufferFormat::from_env(),
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
    let draw_t = Instant::now();
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        let _ = target.render(renderer, ui);
    });
    let render_us = draw_t.elapsed().as_micros();
    let copy_t = Instant::now();
    target.present_rows(f, disp, ui, 0, ui.render_h());
    print_startup_event(
        start,
        "startup_splash_presented",
        format!(
            "render_us={} copy_us={}",
            render_us,
            copy_t.elapsed().as_micros()
        ),
    );
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
    let start_screen = launcher_start_screen_from_env()
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade))
        .unwrap_or(Screen::Home);
    let lock_screen = launcher_lock_screen_from_env()
        .or_else(|| bench_starts_on_arcade.then_some(Screen::Arcade));
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
    let mut launcher_bench_next_step = Instant::now();
    let mut launcher_bench_step_idx = 0usize;
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
        FramebufferFormat::from_env().label()
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
    let mut preview = PreviewState::new();
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
    let catalog_refresh = catalog_refresh_requested();
    let catalog_rx;
    let mut catalog_refresh_done = false;
    let mut catalog_persisted_summary_seen = false;
    match library_db::load_arcade_catalog_from_sqlite(&arcade_root) {
        Ok(loaded) if !loaded.catalog.games.is_empty() => {
            print_startup_event(
                start,
                "catalog_cache_load_sync",
                catalog_load_timing_detail(&loaded),
            );
            catalog = loaded.catalog;
            catalog_ready = true;
            catalog_version = catalog_version.wrapping_add(1);
            apply_forced_arcade_selected(&mut nav, &catalog);
            if ready_catalog_background_worker_needed(
                catalog_refresh,
                !arcade_catalog_required_at_start,
            ) {
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                catalog_rx = Some(start_library_catalog_worker(
                    arcade_root.clone(),
                    catalog_refresh,
                ));
            } else {
                print_startup_event(
                    start,
                    "catalog_refresh_decision",
                    "cache_state=ready refresh_requested=false background_validation=false plan=use_cache_only",
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
            print_startup_event(start, "catalog_worker_start", &arcade_root);
            catalog_rx = Some(start_library_catalog_worker(
                arcade_root.clone(),
                catalog_refresh,
            ));
        }
        Err(e) => {
            eprintln!("arcade catalog cache load failed: {e}");
            print_startup_event(start, "catalog_cache_load_failed", e);
            print_startup_event(start, "catalog_worker_start", &arcade_root);
            catalog_rx = Some(start_library_catalog_worker(
                arcade_root.clone(),
                catalog_refresh,
            ));
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
    bridge.set_catalog_scan_visible(initial_catalog_scan_visible(
        catalog_ready,
        arcade_catalog_required_at_start,
    ));
    bridge.set_catalog_scan_title(if catalog_ready {
        if catalog_refresh {
            "Validating library".into()
        } else {
            "".into()
        }
    } else {
        "Indexing library".into()
    });
    bridge.set_catalog_scan_detail(if catalog_ready {
        format!("Using cached {} games", catalog.len()).into()
    } else {
        "No cached catalog; scanning library...".into()
    });
    bridge.set_catalog_scan_percent(-1);
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
    let mut first_render_logged = false;
    let mut first_vsync_logged = false;
    let mut frame_accounting = LauncherFrameAccounting::new(run_start);
    let mut catalog_scan_redraw = CatalogScanRedraw::new();
    let mut games_found_counter = GamesFoundCounter::default();
    while secs == 0 || run_start.elapsed().as_secs() < secs {
        let loop_start = Instant::now();
        let launching = launcher::launch_in_progress() || !loading_title.is_empty();
        let setup_active = setup.is_active();
        let mut light_bridge_dirty = false;
        let mut full_bridge_dirty = false;
        let mut route_action = FramebufferRouteAction {
            reassert_route: false,
            force_full_present: false,
        };
        if !launching {
            route_action = route_guard.tick(frames);
            if route_action.reassert_route {
                match f.fb_enable_format(
                    0,
                    ui.fb_w() as u16,
                    ui.fb_h() as u16,
                    ui_fpga_scaled_mode(),
                    Some(0),
                    Some(0),
                    std::env::var_os("MISTER_DIRECT_VIDEO").is_some(),
                    FramebufferFormat::from_env(),
                ) {
                    Ok(flag) => {
                        boot_analytics::event(
                            "launcher_fb_route_reasserted",
                            format!("frame={frames} support_flag={flag}"),
                        );
                    }
                    Err(e) => {
                        eprintln!("failed to reassert Slint framebuffer route: {e}");
                        route_action.force_full_present = false;
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
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        let visible =
                            catalog_scan_progress_visible(catalog_ready, nav.screen, &title);
                        let detail = games_found_counter
                            .progress_detail(&title, &detail, loop_start)
                            .unwrap_or(detail);
                        bridge.set_catalog_scan_visible(visible);
                        bridge.set_catalog_scan_title(title.into());
                        bridge.set_catalog_scan_detail(detail.into());
                        bridge.set_catalog_scan_percent(percent);
                        full_bridge_dirty = true;
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
                            apply_forced_arcade_selected(&mut nav, &catalog);
                            print_startup_event(
                                start,
                                "library_ready",
                                format!("games={} load_us={load_us}", catalog.len()),
                            );
                        }
                        if let Some(summary) = summary {
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
                            continue;
                        }
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        bridge.set_catalog_scan_visible(false);
                        bridge.set_catalog_scan_percent(-1);
                        games_found_counter.reset();
                        if cached_before_refresh {
                            bridge.set_catalog_scan_title("Validating library".into());
                            bridge.set_catalog_scan_detail(
                                format!(
                                    "Using cached {} games while checking for changes",
                                    catalog.len()
                                )
                                .into(),
                            );
                        } else {
                            bridge.set_catalog_scan_title("".into());
                            bridge.set_catalog_scan_detail("".into());
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
                        full_bridge_dirty = true;
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
                        print_startup_event(start, "library_db_save_failed", error);
                    }
                    CatalogWorkerMessage::Unchanged { summary } => {
                        catalog_refresh_done = true;
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
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        bridge.set_catalog_scan_visible(false);
                        bridge.set_catalog_scan_title("".into());
                        bridge.set_catalog_scan_detail("".into());
                        bridge.set_catalog_scan_percent(-1);
                        games_found_counter.reset();
                        full_bridge_dirty = true;
                    }
                    CatalogWorkerMessage::Done => {
                        catalog_refresh_done = true;
                        if catalog_ready {
                            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                            bridge.set_catalog_scan_visible(false);
                            bridge.set_catalog_scan_title("".into());
                            bridge.set_catalog_scan_detail("".into());
                            bridge.set_catalog_scan_percent(-1);
                            games_found_counter.reset();
                            full_bridge_dirty = true;
                        }
                    }
                }
            }
        }

        if let Some(scenario) = launcher_bench_scenario {
            if catalog_ready && launcher_bench_next_step.elapsed() >= scenario.period() {
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
                    if let Some(event) = nav.handle_input(&state, frame_now, &catalog) {
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
                                loading_title = "Resetting database…".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Rebooting MiSTer",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                match launcher::reset_database_and_reboot() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("reset database failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::Restart => {
                                loading_title = "Restarting MiSTer…".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Please wait",
                                    Some(&catalog),
                                    &mut preview,
                                    &mut bridge_models,
                                    catalog_version,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = target.render(renderer, ui);
                                    let _ = region;
                                });
                                let _pace = pacer.wait();
                                target.present_rows(f, disp, ui, 0, ui.render_h());
                                match launcher::reboot_mister() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("restart failed: {e}");
                                        loading_title.clear();
                                    }
                                }
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
                        );
                        window.request_redraw();
                        update_slint_animations(animation_clock);
                        window.draw_if_needed(|renderer| {
                            let region = target.render(renderer, ui);
                            let _ = region;
                        });
                        let _pace = pacer.wait();
                        target.present_rows(f, disp, ui, 0, ui.render_h());
                        match launcher::execute_game_launch(&mra) {
                            Ok(spawned) => {
                                launch_started = Instant::now();
                                launch_spawned_mister = spawned;
                            }
                            Err(e) => {
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
        let games_found_detail_changed = if catalog_scan_visible && catalog_scan_percent < 0 {
            games_found_counter.tick(loop_start).is_some_and(|detail| {
                bridge.set_catalog_scan_detail(detail.into());
                true
            })
        } else {
            false
        };
        if launching
            || games_found_detail_changed
            || catalog_scan_redraw.should_request(
                catalog_scan_visible,
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
            ) {
                window.request_redraw();
            }
        }
        if !launching && apply_ready_preview(&app, &mut preview) {
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
        if full_frame_present {
            let cached_copy_start = Instant::now();
            copied_rows = target.present_rows(f, disp, ui, 0, ui.render_h());
            cached_present_frame_us = cached_copy_start.elapsed().as_micros();
        } else if let Some(rect) = this_rect {
            let cached_copy_start = Instant::now();
            copied_rows = target.present_rect(f, disp, ui, rect);
            cached_present_frame_us = cached_copy_start.elapsed().as_micros();
        }
        if let Some(rect) = raw_preview_rect {
            let cached_copy_start = Instant::now();
            copied_rows += target.present_rect(f, disp, ui, rect);
            cached_present_frame_us += cached_copy_start.elapsed().as_micros();
        }
        if let Some(rect) = effect_label_rect {
            if !this_rect.is_some_and(|slint_rect| slint_rect.contains(rect)) {
                let cached_copy_start = Instant::now();
                copied_rows += target.present_rect(f, disp, ui, rect);
                cached_present_frame_us += cached_copy_start.elapsed().as_micros();
            }
        }
        let arcade_update_label = match arcade_list_rect.as_ref() {
            Some(ArcadeListUpdate::Full(_)) => "full".to_string(),
            Some(ArcadeListUpdate::Scroll { delta_y }) => format!("scroll:{delta_y}"),
            None => "none".to_string(),
        };
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
                arcade_update_label,
                preview_cache_state: preview.trace_cache_state(),
                preview_transition: preview_transition_trace,
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

    fn should_request(&mut self, visible: bool, percent: i32, now: Instant) -> bool {
        if !visible {
            return false;
        }
        if percent >= 0 {
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
}

impl GamesFoundCounter {
    fn progress_detail(&mut self, title: &str, detail: &str, now: Instant) -> Option<String> {
        let target = if title == "Classifying library" {
            parse_games_found_detail(detail)
        } else {
            None
        };
        let Some(target) = target else {
            self.reset();
            return None;
        };
        if !self.active || target < self.displayed {
            self.displayed = self.displayed.min(target);
            self.last_tick = Some(now);
        }
        self.target = target;
        self.active = true;
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
        let step = games_found_count_step(self.displayed, self.target, elapsed);
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
    }
}

fn parse_games_found_detail(detail: &str) -> Option<usize> {
    detail.strip_prefix("Games found: ")?.trim().parse().ok()
}

fn format_games_found(count: usize) -> String {
    format!("Games found: {count}")
}

fn games_found_count_step(displayed: usize, target: usize, elapsed: Duration) -> usize {
    if target <= displayed {
        return 0;
    }
    let lag = target - displayed;
    let elapsed_ms = elapsed.as_millis().max(1) as usize;
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

fn initial_catalog_scan_visible(
    catalog_ready: bool,
    _arcade_catalog_required_at_start: bool,
) -> bool {
    !catalog_ready
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

fn catalog_scan_progress_visible(catalog_ready: bool, screen: Screen, title: &str) -> bool {
    if matches!(title, "Library scan failed" | "Library load failed") {
        return true;
    }
    if !catalog_ready {
        return screen == Screen::Home || screen == Screen::Arcade || title == "Indexing library";
    }
    matches!(
        title,
        "Library changed" | "Indexing library" | "Loading library"
    )
}

fn ready_catalog_background_worker_needed(
    refresh_requested: bool,
    background_validation: bool,
) -> bool {
    refresh_requested || background_validation
}

fn duplicate_cached_catalog_ready(catalog_ready: bool, cached_before_refresh: bool) -> bool {
    catalog_ready && cached_before_refresh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_effect_bench::{EffectFill, EffectTarget};

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

    #[test]
    pub(super) fn home_boot_with_ready_catalog_hides_catalog_popup() {
        assert!(!initial_catalog_scan_visible(true, false));
    }

    #[test]
    pub(super) fn missing_catalog_shows_catalog_popup_on_home_or_arcade_boot() {
        assert!(initial_catalog_scan_visible(false, false));
        assert!(initial_catalog_scan_visible(false, true));
        assert!(!initial_catalog_scan_visible(true, true));
    }

    #[test]
    pub(super) fn catalog_scan_redraw_throttles_indeterminate_animation() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now,
            period: Duration::from_millis(66),
        };
        assert!(!redraw.should_request(true, -1, now + Duration::from_millis(20)));
        assert!(redraw.should_request(true, -1, now + Duration::from_millis(70)));
    }

    #[test]
    pub(super) fn catalog_scan_redraw_skips_determinate_periodic_frames() {
        let now = Instant::now();
        let mut redraw = CatalogScanRedraw {
            last_request: now - Duration::from_secs(1),
            period: Duration::from_millis(66),
        };
        assert!(!redraw.should_request(true, 90, now));
        assert!(!redraw.should_request(false, -1, now));
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

        let next = counter
            .tick(now + Duration::from_millis(132))
            .expect("counter should keep moving");
        let count = parse_games_found_detail(&next).expect("parse counter detail");
        assert!(count > 37);
        assert!(count < 250);
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
            "Validating library"
        ));
        assert!(!catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Preview images changed"
        ));
    }

    #[test]
    pub(super) fn missing_catalog_and_rebuild_progress_are_visible() {
        assert!(catalog_scan_progress_visible(
            false,
            Screen::Home,
            "Indexing library"
        ));
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Library changed"
        ));
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Indexing library"
        ));
    }

    #[test]
    pub(super) fn catalog_scan_failures_are_visible_even_with_cache() {
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Home,
            "Library scan failed"
        ));
        assert!(catalog_scan_progress_visible(
            true,
            Screen::Arcade,
            "Library load failed"
        ));
    }

    #[test]
    pub(super) fn ready_catalog_uses_background_worker_for_refresh_or_home_validation() {
        assert!(!ready_catalog_background_worker_needed(false, false));
        assert!(ready_catalog_background_worker_needed(true, true));
        assert!(ready_catalog_background_worker_needed(false, true));
    }

    #[test]
    pub(super) fn duplicate_cached_catalog_ready_is_skipped_after_sync_load() {
        assert!(duplicate_cached_catalog_ready(true, true));
        assert!(!duplicate_cached_catalog_ready(false, true));
        assert!(!duplicate_cached_catalog_ready(true, false));
    }
}
