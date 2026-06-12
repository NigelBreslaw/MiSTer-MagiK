use super::*;

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
    launcher_mode: LauncherRunMode,
) {
    let start = Instant::now();
    let mut frames = 0u64;
    let mut nav = LauncherNav::new();
    nav.screen = launcher_mode.initial_screen();
    let mut setup = SetupNav::new();
    let mut loading_title = String::new();
    let mut launch_started = Instant::now();
    let mut launch_spawned_mister = false;
    let mut last_clock_update = Instant::now() - Duration::from_secs(2);
    let mut last_clock_text = launcher_clock_text();
    let mut last_status_write = Instant::now() - Duration::from_secs(2);
    let launcher_bench_scenario = LauncherBenchScenario::from_env();
    let mut launcher_bench_next_step = Instant::now();
    let mut launcher_bench_step_idx = 0usize;
    let mut launcher_fps_window_start;
    let mut launcher_fps_frames = 0u64;
    let mut launcher_prepare_us = 0u128;
    let mut launcher_render_us = 0u128;
    let mut launcher_custom_draw_us = 0u128;
    let mut launcher_vsync_us = 0u128;
    let mut launcher_copy_us = 0u128;
    let mut launcher_cached_present_us = 0u128;
    let mut launcher_overlay_present_us = 0u128;
    let mut launcher_rows = 0u128;
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
        launcher_mode.label(),
        FramebufferFormat::from_env().label()
    );
    if let Some(scenario) = launcher_bench_scenario {
        println!("launcher_bench_scenario={}", scenario.label());
    }
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
    let mut boot_frame_profile = boot_analytics::LauncherFrameWriter::from_env();
    let mut preview = PreviewState::new();
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
    let mut preview_scroll_trace = std::env::var("MISTER_PREVIEW_SCROLL_TRACE")
        .ok()
        .and_then(|path| {
            let mut file = std::fs::File::create(&path)
                .map_err(|e| eprintln!("preview scroll trace: create {path} failed: {e}"))
                .ok()?;
            std::io::Write::write_all(
                &mut file,
                b"frame\telapsed_us\tselected\tvisual_index\tcache_state\ttransition_effect\ttransition_progress\tarcade_update\trows\tprepare_us\tslint_render_us\tcustom_draw_us\tvsync_us\tfb_present_us\tcached_present_us\toverlay_present_us\tpresent_probe_us\twall_us\n",
            )
            .map_err(|e| eprintln!("preview scroll trace: header write failed: {e}"))
            .ok()?;
            println!("preview_scroll_trace={path}");
            Some(file)
        });
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    let preview_stress = preview_stress_enabled();
    println!(
        "preview_stress={} preview_visual_pct={} preview_blitter={}",
        if preview_stress { "on" } else { "off" },
        preview_visual_pct(),
        if preview_raw_blitter_enabled() {
            "raw"
        } else {
            "slint"
        }
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
    if launcher_mode == LauncherRunMode::Arcade {
        match library_db::load_arcade_catalog_from_sqlite(&arcade_root) {
            Ok(loaded) if !loaded.catalog.games.is_empty() => {
                catalog = loaded.catalog;
                catalog_ready = true;
                catalog_version = catalog_version.wrapping_add(1);
                apply_forced_arcade_selected(&mut nav, &catalog);
                print_startup_event(
                    start,
                    "catalog_cache_load_sync",
                    format!("games={} load_us={}", catalog.len(), loaded.us),
                );
                if catalog_refresh {
                    print_startup_event(start, "catalog_worker_start", &arcade_root);
                    catalog_rx = Some(start_library_catalog_worker(arcade_root.clone()));
                } else {
                    catalog_rx = None;
                    catalog_refresh_done = true;
                }
            }
            Ok(loaded) => {
                print_startup_event(
                    start,
                    "catalog_cache_empty",
                    format!("games={} load_us={}", loaded.catalog.len(), loaded.us),
                );
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                catalog_rx = Some(start_library_catalog_worker(arcade_root.clone()));
            }
            Err(e) => {
                eprintln!("arcade catalog cache load failed: {e}");
                print_startup_event(start, "catalog_cache_load_failed", e);
                print_startup_event(start, "catalog_worker_start", &arcade_root);
                catalog_rx = Some(start_library_catalog_worker(arcade_root.clone()));
            }
        }
    } else {
        print_startup_event(start, "catalog_cache_load_deferred", &arcade_root);
        print_startup_event(start, "catalog_worker_start", &arcade_root);
        catalog_rx = Some(start_library_catalog_worker(arcade_root.clone()));
    }
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_game_systems(bridge_models.game_systems(&catalog, catalog_version));
    bridge.set_catalog_scan_visible(!catalog_ready);
    bridge.set_catalog_scan_title(if catalog_ready {
        if catalog_refresh {
            "Refreshing library".into()
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
    window.request_redraw();
    let run_start = if launcher_mode == LauncherRunMode::Arcade && catalog_ready {
        Instant::now()
    } else {
        start
    };
    launcher_fps_window_start = run_start;
    let mut first_frame_logged = false;
    let mut first_render_logged = false;
    let mut first_vsync_logged = false;
    let mut first_copy_logged = false;
    let mut first_visible_copy_done = false;
    let mut stable_frame_logged = false;
    while secs == 0 || run_start.elapsed().as_secs() < secs {
        let loop_start = Instant::now();
        let launching = launcher::launch_in_progress() || !loading_title.is_empty();
        let setup_active = setup.is_active();
        let mut light_bridge_dirty = false;
        let mut full_bridge_dirty = false;
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
                    CatalogWorkerMessage::Progress { title, detail } => {
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        let visible = if catalog_ready && launcher_mode == LauncherRunMode::Arcade {
                            title == "Library scan failed" || title == "Library load failed"
                        } else {
                            !catalog_ready
                                || title == "Indexing library"
                                || title == "Library changed"
                                || title == "Library scan failed"
                                || title == "Library load failed"
                        };
                        bridge.set_catalog_scan_visible(visible);
                        bridge.set_catalog_scan_title(title.into());
                        bridge.set_catalog_scan_detail(detail.into());
                        full_bridge_dirty = true;
                    }
                    CatalogWorkerMessage::Ready {
                        catalog: ready_catalog,
                        summary,
                        load_us,
                    } => {
                        catalog = ready_catalog;
                        catalog_version = catalog_version.wrapping_add(1);
                        catalog_ready = true;
                        apply_forced_arcade_selected(&mut nav, &catalog);
                        let cached_before_refresh = summary.is_none();
                        catalog_refresh_done = !cached_before_refresh;
                        print_startup_event(
                            start,
                            "library_ready",
                            format!("games={} load_us={load_us}", catalog.len()),
                        );
                        if let Some(summary) = summary {
                            let event = if summary.skipped {
                                "library_db_unchanged"
                            } else {
                                "library_db_saved"
                            };
                            print_startup_event(
                                start,
                                event,
                                format!(
                                    "bytes={} scan_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
                                    summary.bytes,
                                    summary.scan_us,
                                    summary.import_us,
                                    summary.discoveries,
                                    summary.normal_files,
                                    summary.containers,
                                    summary.entries
                                ),
                            );
                        }
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        bridge.set_catalog_scan_visible(false);
                        if cached_before_refresh {
                            bridge.set_catalog_scan_title("Refreshing library".into());
                            bridge.set_catalog_scan_detail(
                                format!("Using cached {} games", catalog.len()).into(),
                            );
                        } else {
                            bridge.set_catalog_scan_title("".into());
                            bridge.set_catalog_scan_detail("".into());
                        }
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
                        full_bridge_dirty = true;
                    }
                    CatalogWorkerMessage::Unchanged { summary } => {
                        catalog_refresh_done = true;
                        print_startup_event(
                            start,
                            "library_db_unchanged",
                            format!(
                                "bytes={} scan_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
                                summary.bytes,
                                summary.scan_us,
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
                        full_bridge_dirty = true;
                    }
                    CatalogWorkerMessage::Done => {
                        catalog_refresh_done = true;
                        if catalog_ready {
                            let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                            bridge.set_catalog_scan_visible(false);
                            bridge.set_catalog_scan_title("".into());
                            bridge.set_catalog_scan_detail("".into());
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

        launcher_mode.enforce(&mut nav);

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

            launcher_mode.enforce(&mut nav);

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

        if launching {
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
        let arcade_list_rect = if !preview_stress && !launching && nav.screen == Screen::Arcade {
            let force_arcade_redraw = this_rect.is_some_and(|rect| {
                rect.intersection(ArcadeListRenderer::dirty_rect())
                    .is_some()
            });
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
        let pace = if first_visible_copy_done {
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
        if launching {
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
        if let Some(file) = preview_scroll_trace.as_mut() {
            let cache_state = preview.trace_cache_state();
            let _ = std::io::Write::write_fmt(
                file,
                format_args!(
                    "{}\t{}\t{}\t{:.6}\t{}\t{}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                    frames,
                    loop_start.duration_since(run_start).as_micros(),
                    nav.arcade.selected,
                    nav.arcade.visual_index,
                    cache_state,
                    preview_transition_trace.effect.label(),
                    preview_transition_trace.progress,
                    arcade_update_label,
                    copied_rows,
                    prepare_us,
                    (frame_t2 - frame_t1).as_micros(),
                    (custom_draw_done - custom_draw_start).as_micros(),
                    (frame_t3 - custom_draw_done).as_micros(),
                    (frame_t4 - frame_t3).as_micros(),
                    cached_present_frame_us,
                    overlay_present_frame_us,
                    present_probe_frame_us,
                    (frame_t4 - loop_start).as_micros()
                ),
            );
        }
        if copied_rows > 0 && !first_copy_logged {
            first_copy_logged = true;
            boot_analytics::event(
                if first_visible_copy_done {
                    "first_copy"
                } else {
                    "first_copy_immediate"
                },
                format!(
                    "frame={frames} rows={copied_rows} dirty_rect={}",
                    format_dirty_rect(this_rect)
                ),
            );
            disp.record_visual_sample("after_first_copy");
        }
        if copied_rows > 0 {
            first_visible_copy_done = true;
        }
        launcher_fps_frames += 1;
        launcher_prepare_us += prepare_us;
        launcher_render_us += (frame_t2 - frame_t1).as_micros();
        launcher_custom_draw_us += (custom_draw_done - custom_draw_start).as_micros();
        launcher_vsync_us += (frame_t3 - custom_draw_done).as_micros();
        launcher_copy_us += (frame_t4 - frame_t3).as_micros();
        launcher_cached_present_us += cached_present_frame_us;
        launcher_overlay_present_us += overlay_present_frame_us;
        launcher_rows += copied_rows as u128;
        if launcher_fps_window_start.elapsed() >= Duration::from_secs(1) {
            let n = launcher_fps_frames.max(1) as u128;
            println!(
                "launcher fps ~ {} prepare {}us slint-render {}us custom-draw {}us vsync-wait {}us fb-present {}us cached-present {}us overlay-present {}us ({} rows avg)",
                launcher_fps_frames,
                launcher_prepare_us / n,
                launcher_render_us / n,
                launcher_custom_draw_us / n,
                launcher_vsync_us / n,
                launcher_copy_us / n,
                launcher_cached_present_us / n,
                launcher_overlay_present_us / n,
                launcher_rows / n
            );
            launcher_fps_window_start = Instant::now();
            launcher_fps_frames = 0;
            launcher_prepare_us = 0;
            launcher_render_us = 0;
            launcher_custom_draw_us = 0;
            launcher_vsync_us = 0;
            launcher_copy_us = 0;
            launcher_cached_present_us = 0;
            launcher_overlay_present_us = 0;
            launcher_rows = 0;
        }
        if frames == 30 && !stable_frame_logged {
            stable_frame_logged = true;
            boot_analytics::event("stable_frame", "frame=30");
            disp.record_visual_sample("stable_frame_30");
        } else if frames == 120 {
            disp.record_visual_sample("sample_frame_120");
        } else if frames == 240 {
            disp.record_visual_sample("sample_frame_240");
        }
        let reasserted = false;
        if boot_frame_profile
            .as_ref()
            .is_some_and(|profile| !profile.should_record(frames))
        {
            boot_frame_profile = None;
        }
        if let Some(profile) = boot_frame_profile.as_mut() {
            let (edge1_hash, edge1_nonzero) = disp.right_edge_signature(1);
            let (edge8_hash, edge8_nonzero) = disp.right_edge_signature(8);
            let (left8_hash, left8_nonzero) = disp.left_edge_signature(8);
            let (top8_hash, top8_nonzero) = disp.top_edge_signature(8);
            let (bottom8_hash, bottom8_nonzero) = disp.bottom_edge_signature(8);
            let (full_sample_hash, full_sample_nonzero) = disp.sampled_signature();
            profile.record(
                frames,
                (frame_t1 - frame_t0).as_micros() as u64,
                (frame_t2 - frame_t1).as_micros() as u64,
                (frame_t3 - frame_t2).as_micros() as u64,
                (frame_t4 - frame_t3).as_micros() as u64,
                copied_rows,
                reasserted,
                edge1_hash,
                edge1_nonzero,
                edge8_hash,
                edge8_nonzero,
                left8_hash,
                left8_nonzero,
                top8_hash,
                top8_nonzero,
                bottom8_hash,
                bottom8_nonzero,
                full_sample_hash,
                full_sample_nonzero,
            );
        }
        if !first_frame_logged {
            first_frame_logged = true;
            boot_analytics::event("first_frame", format!("catalog_ready={catalog_ready}"));
            print_startup_event(
                start,
                "first_frame",
                format!("catalog_ready={catalog_ready}"),
            );
        }
        if last_status_write.elapsed() >= Duration::from_secs(1) {
            let fps_estimate = if run_start.elapsed().as_secs_f64() > 0.0 {
                frames as f64 / run_start.elapsed().as_secs_f64()
            } else {
                0.0
            };
            runtime_status::write_launcher_status(LauncherStatus {
                scene: launcher_mode.label(),
                screen: screen_label(nav.screen),
                frames,
                fps_estimate,
                last_frame_ms_ago: 0,
                catalog_ready,
                catalog_games: catalog.len(),
                catalog_systems: catalog.systems.len(),
                catalog_refresh_done,
                launch_state: if launching { "launching" } else { "idle" },
                loading_title: &loading_title,
                input_pad_count: pad.len(),
                active_pad_index: pad.active_idx(),
            });
            last_status_write = Instant::now();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
