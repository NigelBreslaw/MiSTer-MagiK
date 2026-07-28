// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(target_os = "macos")]
mod macos {
    use mister_magik_fb::arcade_catalog::ArcadeGameEntry;
    use mister_magik_fb::fireworks::{FireworkRenderer, embedded_firework_json};
    use mister_magik_fb::framebuffer::target::{FramebufferTargetGeometry, UiFrameTarget};
    use mister_magik_fb::input_state::PadState;
    use mister_magik_fb::launcher::{LauncherAction, LauncherNav, Screen};
    use mister_magik_fb::launcher_presentation::LauncherBridgePresenter;
    use mister_magik_fb::particle_engine::{ParticleConfig, ParticlePreset};
    use mister_magik_fb::particle_renderer::ParticleRenderer;
    use mister_magik_fb::preview_transition::{
        PreviewTransitionController, Rgb565PreviewTransitionCompositor, transition_duration,
    };
    use mister_magik_fb::ui_preview_fixtures::{FixtureScreenshot, UiPreviewFixtures};
    use mister_magik_fb::visual_composition::{
        ArcadeVisualLayer, PreviewFrame, PreviewPixels, PreviewSurface, ScreenshotTileImage,
        ScreenshotTileWall, hdmi_preview_rect,
    };
    use mister_magik_fb::visual_platform::{MisterPlatform, MisterSoftwareWindow};
    use mister_magik_ui::launcher::{
        ArcadeGame, Launcher, MenuItem, MenuItemKind, MenuItemStatus, MisterBridge, MisterUi,
        ScreenshotPackProgress,
    };
    use slint::platform::software_renderer::{RepaintBufferType, Rgb565Pixel};
    use slint::{ComponentHandle, ModelRc, PhysicalSize, SharedString, VecModel};
    use softbuffer::{Context, Surface};
    use std::cell::Cell;
    use std::error::Error;
    use std::fs::{OpenOptions, read_to_string};
    use std::io::Write;
    use std::num::NonZeroU32;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::{ElementState, WindowEvent};
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::keyboard::{KeyCode, PhysicalKey};
    use winit::window::{Window, WindowId};

    const FRAME_WIDTH: usize = 960;
    const FRAME_HEIGHT: usize = 540;
    const DEFAULT_REFRESH_HZ: u32 = 60;
    const MAX_AUTO_REFRESH_HZ: u32 = 120;
    const PREVIEW_TRANSITION_DURATION: Duration = Duration::from_millis(200);

    pub fn run() -> Result<(), Box<dyn Error>> {
        let options = PreviewOptions::parse(std::env::args().skip(1))?;
        let slint_window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let fixed_time = Rc::new(Cell::new(Duration::ZERO));
        slint::platform::set_platform(Box::new(MisterPlatform::new(
            Rc::clone(&slint_window),
            Some(Rc::clone(&fixed_time)),
        )))?;
        slint_window.set_size(PhysicalSize::new(FRAME_WIDTH as u32, FRAME_HEIGHT as u32));

        let launcher = Launcher::new()?;
        let ui = launcher.global::<MisterUi>();
        ui.set_window_width(FRAME_WIDTH as i32);
        ui.set_window_height(FRAME_HEIGHT as i32);
        ui.set_crt_layout(false);
        let bridge = launcher.global::<MisterBridge>();
        initialize_bridge(&bridge);
        launcher.show()?;
        slint_window.request_redraw();

        let headless = options.output.is_some();
        let firework_renderer = if options.scenario == Scenario::Fireworks {
            let json = if let Some(path) = options.firework_spec.as_ref() {
                read_to_string(path)
                    .map_err(|error| format!("read firework spec {}: {error}", path.display()))?
            } else {
                embedded_firework_json(&options.firework)
                    .ok_or_else(|| format!("unknown firework {:?}", options.firework))?
                    .to_owned()
            };
            Some(FireworkRenderer::from_json(
                &json,
                FRAME_WIDTH,
                FRAME_HEIGHT,
                0x4d61_6769_4b,
            )?)
        } else {
            None
        };
        let mut application = PreviewApplication::new(
            launcher,
            slint_window,
            fixed_time,
            Scenario::Home,
            options.refresh_rate,
            headless,
            firework_renderer,
        )?;
        application.select_scenario(options.scenario);
        if let Some(output) = options.output {
            if let Some(time_ms) = options.time_ms {
                application.screensaver_elapsed = Duration::from_millis(time_ms);
                application.compose_frame();
            } else {
                for _ in 0..=options.frame {
                    application.compose_frame();
                }
            }
            write_ppm(
                &output,
                application.frame_target.cached_565(),
                FRAME_WIDTH,
                FRAME_HEIGHT,
            )?;
            println!(
                "capture={} scenario={} frame={} time_ms={} refresh_hz={} hud={} hash={:016x}",
                output.display(),
                options.scenario.label(),
                options.frame,
                options.time_ms.unwrap_or(0),
                application.refresh_hz,
                options.hud,
                frame_hash(application.frame_target.cached_565())
            );
            return Ok(());
        }
        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::WaitUntil(application.next_frame_deadline));
        event_loop.run_app(&mut application)?;
        Ok(())
    }

    struct PreviewApplication {
        launcher: Launcher,
        slint_window: Rc<MisterSoftwareWindow>,
        fixed_time: Rc<Cell<Duration>>,
        native_window: Option<Arc<Window>>,
        surface: Option<Surface<Arc<Window>, Arc<Window>>>,
        frame_target: UiFrameTarget,
        xrgb8888: Vec<u32>,
        scenario: Scenario,
        selection: usize,
        settings_focused: bool,
        launcher_nav: LauncherNav,
        launcher_pad: PadState,
        launcher_epoch: Instant,
        bridge_presenter: LauncherBridgePresenter,
        fixtures: UiPreviewFixtures,
        arcade_layer: ArcadeVisualLayer,
        preview_transition: PreviewTransitionController<()>,
        preview_compositor: Rgb565PreviewTransitionCompositor,
        preview_previous_index: Option<usize>,
        preview_current_index: Option<usize>,
        preview_transition_id: u64,
        preview_transition_duration: Duration,
        particle_renderer: Option<ParticleRenderer>,
        firework_renderer: Option<FireworkRenderer>,
        tile_wall: ScreenshotTileWall,
        tile_images: Vec<ScreenshotTileImage>,
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
    }

    impl PreviewApplication {
        fn new(
            launcher: Launcher,
            slint_window: Rc<MisterSoftwareWindow>,
            fixed_time: Rc<Cell<Duration>>,
            scenario: Scenario,
            refresh_rate: RefreshRate,
            headless: bool,
            firework_renderer: Option<FireworkRenderer>,
        ) -> Result<Self, Box<dyn Error>> {
            let fixtures = UiPreviewFixtures::new()?;
            let mut launcher_nav = LauncherNav::new();
            launcher_nav.catalog_build_started();
            for system_id in &fixtures.shell_system_ids {
                launcher_nav.catalog_system_discovered(system_id);
                launcher_nav.catalog_system_ready(system_id);
            }
            launcher_nav.sync_launcher_taxonomy(&fixtures.catalog);
            launcher_nav.catalog_build_finished(&fixtures.catalog);
            let tile_images = fixtures
                .screenshots
                .iter()
                .map(|image| ScreenshotTileImage {
                    pixels: image.pixels.clone(),
                    w: image.width,
                    h: image.height,
                    stride: image.stride,
                })
                .collect();
            let refresh_hz = refresh_rate.headless_hz();
            let now = Instant::now();
            let next_frame_deadline = now + elapsed_for_frame(1, refresh_hz);
            let mut application = Self {
                launcher,
                slint_window,
                fixed_time,
                native_window: None,
                surface: None,
                frame_target: UiFrameTarget::cached(FramebufferTargetGeometry::new(
                    FRAME_WIDTH,
                    FRAME_HEIGHT,
                )),
                xrgb8888: Vec::new(),
                scenario,
                selection: 0,
                settings_focused: false,
                launcher_nav,
                launcher_pad: PadState::default(),
                launcher_epoch: Instant::now(),
                bridge_presenter: LauncherBridgePresenter::default(),
                fixtures,
                arcade_layer: ArcadeVisualLayer::new(FRAME_WIDTH, FRAME_HEIGHT),
                preview_transition: PreviewTransitionController::default(),
                preview_compositor: Rgb565PreviewTransitionCompositor::new(
                    FRAME_WIDTH,
                    FRAME_HEIGHT,
                ),
                preview_previous_index: None,
                preview_current_index: None,
                preview_transition_id: 0,
                preview_transition_duration: PREVIEW_TRANSITION_DURATION,
                particle_renderer: None,
                firework_renderer,
                tile_wall: ScreenshotTileWall::new(FRAME_WIDTH, FRAME_HEIGHT),
                tile_images,
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
            };
            application.select_scenario(scenario);
            Ok(application)
        }

        fn create_window(&mut self, event_loop: &ActiveEventLoop) {
            let attributes = Window::default_attributes()
                .with_title(self.window_title())
                .with_inner_size(LogicalSize::new(FRAME_WIDTH as f64, FRAME_HEIGHT as f64))
                .with_min_inner_size(LogicalSize::new(
                    (FRAME_WIDTH / 2) as f64,
                    (FRAME_HEIGHT / 2) as f64,
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
                "MiSTer MagiK UI Preview — {} — {} — {} Hz",
                self.scenario.label(),
                self.scenario.shortcut(),
                self.refresh_hz
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
            if scenario == Scenario::ArcadeCrossfade {
                self.preview_previous_index = None;
                self.preview_current_index = Some(0);
                self.preview_transition.reset();
            }
            if scenario.uses_launcher_navigation() {
                self.configure_launcher_screen(scenario);
            }
            apply_scenario(&self.launcher, scenario);
            if matches!(scenario, Scenario::Arcade | Scenario::ArcadeCrossfade) {
                self.arcade_layer.invalidate();
            }
            if matches!(scenario, Scenario::ParticleScreensaver) {
                self.magik_particle_renderer().invalidate_hidden_slot(1);
                self.screensaver_elapsed = Duration::ZERO;
            }
            if matches!(scenario, Scenario::Fireworks) {
                self.screensaver_elapsed = Duration::ZERO;
            }
            if matches!(scenario, Scenario::ScreenshotTiles) {
                self.tile_wall.invalidate();
                self.screensaver_elapsed = Duration::ZERO;
            }
            if scenario.uses_launcher_navigation() {
                self.sync_launcher_navigation();
            } else {
                self.update_selection();
            }
            if let Some(window) = self.native_window.as_ref() {
                window.set_title(&self.window_title());
                window.request_redraw();
            }
        }

        fn move_selection(&mut self, delta: isize) {
            let count = match self.scenario {
                Scenario::Home | Scenario::BackgroundScan | Scenario::Confirm => 6,
                Scenario::Settings => 5,
                Scenario::About => 2,
                Scenario::Licenses => 2,
                Scenario::ScreensaverSettings => 3,
                Scenario::Arcade => self.fixtures.arcade_games().len(),
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
            bridge.set_settings_focused(self.settings_focused);
            bridge.set_settings_selected(self.selection as i32);
            bridge.set_about_selected(self.selection as i32);
            bridge.set_licenses_selected(self.selection as i32);
            bridge.set_screensaver_settings_selected(self.selection as i32);
            bridge.set_confirm_selected(self.selection.min(1) as i32);
            if self.scenario == Scenario::Arcade {
                apply_arcade_fixture_bridge(
                    &self.launcher,
                    self.fixtures.arcade_games(),
                    self.selection,
                );
            }
            if matches!(
                self.scenario,
                Scenario::Home | Scenario::BackgroundScan | Scenario::Confirm
            ) {
                bridge.set_menu_items(home_menu_items(self.selection));
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
                Scenario::Arcade | Scenario::ArcadeCrossfade => {
                    self.launcher_nav
                        .open_default_arcade(&self.fixtures.catalog);
                    if scenario == Scenario::ArcadeCrossfade {
                        self.launcher_nav.arcade.selected = 1;
                        self.launcher_nav.arcade.snap_to_selected();
                    }
                }
                Scenario::Settings => self.launcher_nav.screen = Screen::Settings,
                Scenario::Controller => self.launcher_nav.screen = Screen::Controller,
                Scenario::About => self.launcher_nav.screen = Screen::About,
                Scenario::Licenses => self.launcher_nav.screen = Screen::Licenses,
                Scenario::Info => self.launcher_nav.screen = Screen::Info,
                Scenario::ScreensaverSettings => self.launcher_nav.screen = Screen::Screensaver,
                _ => {}
            }
            self.launcher_pad = PadState::default();
            self.launcher_nav.absorb_input(&self.launcher_pad);
        }

        fn handle_launcher_key(&mut self, code: KeyCode, state: ElementState) -> bool {
            let pressed = state == ElementState::Pressed;
            let field = match code {
                KeyCode::ArrowUp => Some(&mut self.launcher_pad.dpad_up),
                KeyCode::ArrowDown => Some(&mut self.launcher_pad.dpad_down),
                KeyCode::ArrowLeft => Some(&mut self.launcher_pad.dpad_left),
                KeyCode::ArrowRight => Some(&mut self.launcher_pad.dpad_right),
                KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space => {
                    Some(&mut self.launcher_pad.btn_a)
                }
                KeyCode::Escape | KeyCode::Backspace => Some(&mut self.launcher_pad.btn_b),
                KeyCode::Home => Some(&mut self.launcher_pad.btn_home),
                _ => None,
            };
            if let Some(field) = field {
                *field = pressed;
                self.launcher_pad.rebuild_pressed_now();
                true
            } else {
                false
            }
        }

        fn tick_launcher_navigation(&mut self) {
            if !self.scenario.uses_launcher_navigation() {
                return;
            }
            let frame_now = self.launcher_epoch + self.fixed_time.get();
            let event = self.launcher_nav.handle_input(
                &self.launcher_pad,
                frame_now,
                &self.fixtures.catalog,
            );
            if let Some(event) = event {
                match event.action {
                    LauncherAction::PreviewScreensaver => {
                        self.select_scenario(Scenario::ScreenshotTiles);
                        return;
                    }
                    LauncherAction::LaunchGame => {}
                    _ => {}
                }
            }
            let previous = self.scenario;
            self.scenario = Scenario::from_screen(self.launcher_nav.screen);
            self.sync_launcher_navigation();
            if self.scenario != previous
                && let Some(window) = self.native_window.as_ref()
            {
                window.set_title(&self.window_title());
            }
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
                &self.fixtures.catalog,
                Some(1),
                false,
            );
            apply_arcade_fixture_bridge(
                &self.launcher,
                self.fixtures.arcade_games(),
                self.launcher_nav.arcade.selected,
            );
            self.slint_window.request_redraw();
        }

        fn compose_frame(&mut self) {
            let frame_delta = self.frame_delta();
            self.tick_launcher_navigation();
            slint::platform::update_timers_and_animations();
            self.slint_window.request_redraw();
            self.slint_window.draw_if_needed(|renderer| {
                self.frame_target.render(
                    renderer,
                    FramebufferTargetGeometry::new(FRAME_WIDTH, FRAME_HEIGHT),
                );
            });
            if self.scenario == Scenario::Arcade {
                let games = self
                    .launcher_nav
                    .active_collection_id()
                    .map(|collection| {
                        self.launcher_nav
                            .active_arcade_game_view(&self.fixtures.catalog, collection)
                    })
                    .unwrap_or_else(mister_magik_fb::arcade_catalog::ArcadeGameView::empty);
                self.arcade_layer.compose(
                    &mut self.frame_target,
                    games,
                    self.launcher_nav.arcade.selected,
                    self.launcher_nav.arcade.visual_index,
                    true,
                );
                let current = self
                    .preview_current_index
                    .and_then(|index| self.fixtures.arcade_games().get(index))
                    .and_then(|game| self.fixtures.screenshot(&game.preview_asset_key))
                    .or_else(|| self.fixtures.screenshots.first());
                let previous = self
                    .preview_previous_index
                    .and_then(|index| self.fixtures.arcade_games().get(index))
                    .and_then(|game| self.fixtures.screenshot(&game.preview_asset_key));
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
                        FRAME_WIDTH,
                        FRAME_HEIGHT,
                        hdmi_preview_rect(FRAME_WIDTH, FRAME_HEIGHT),
                        previous.map(fixture_preview_frame),
                        fixture_preview_frame(current),
                        transition.progress,
                        PreviewSurface::full(FRAME_WIDTH),
                    );
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
                self.tile_wall.render(
                    self.frame_target.cached_565_mut(),
                    FRAME_WIDTH,
                    FRAME_HEIGHT,
                    &self.tile_images,
                    self.screensaver_elapsed,
                );
            } else if self.scenario == Scenario::Fireworks {
                self.firework_renderer
                    .as_ref()
                    .expect("firework renderer initialized for fireworks scenario")
                    .render(self.frame_target.cached_565_mut(), self.screensaver_elapsed)
                    .expect("render declarative firework");
            }
            if matches!(
                self.scenario,
                Scenario::ParticleScreensaver | Scenario::ScreenshotTiles | Scenario::Fireworks
            ) && !self.screensaver_paused
            {
                self.screensaver_elapsed += frame_delta;
            }
            self.fixed_time.set(self.fixed_time.get() + frame_delta);
        }

        fn magik_particle_renderer(&mut self) -> &mut ParticleRenderer {
            self.particle_renderer.get_or_insert_with(|| {
                ParticleRenderer::new_magik(ParticleConfig {
                    count: 16_384,
                    width: FRAME_WIDTH,
                    height: FRAME_HEIGHT,
                    seed: 0x4d61_6769_4b,
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
                FRAME_WIDTH,
                FRAME_HEIGHT,
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
                    self.launcher_nav.absorb_input(&self.launcher_pad);
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
        Arcade,
        ArcadeCrossfade,
        Settings,
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
        Compatibility,
        Loading,
        MediaProgress,
        ParticleScreensaver,
        Fireworks,
        ScreenshotTiles,
    }

    impl Scenario {
        fn uses_launcher_navigation(self) -> bool {
            matches!(
                self,
                Self::Home
                    | Self::Arcade
                    | Self::ArcadeCrossfade
                    | Self::Settings
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
                "arcade" => Some(Self::Arcade),
                "arcade-crossfade" | "crossfade" => Some(Self::ArcadeCrossfade),
                "settings" => Some(Self::Settings),
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
                "compatibility" => Some(Self::Compatibility),
                "loading" => Some(Self::Loading),
                "media-progress" => Some(Self::MediaProgress),
                "particle" | "particle-screensaver" => Some(Self::ParticleScreensaver),
                "firework" | "fireworks" => Some(Self::Fireworks),
                "screenshot-tiles" | "tiles" => Some(Self::ScreenshotTiles),
                _ => None,
            }
        }

        fn label(self) -> &'static str {
            match self {
                Self::Home => "Home",
                Self::Arcade => "Arcade",
                Self::ArcadeCrossfade => "Arcade Crossfade",
                Self::Settings => "Settings",
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
                Self::Compatibility => "Compatibility",
                Self::Loading => "Loading",
                Self::MediaProgress => "Media Progress",
                Self::ParticleScreensaver => "Particle Screensaver",
                Self::Fireworks => "Fireworks",
                Self::ScreenshotTiles => "Screenshot Tile Screensaver",
            }
        }

        fn shortcut(self) -> &'static str {
            match self {
                Self::Home => "1",
                Self::Settings => "2",
                Self::Controller => "3",
                Self::About => "4",
                Self::Licenses => "5",
                Self::Info => "6",
                Self::ScreensaverSettings => "7",
                Self::Startup => "8",
                Self::Confirm => "9",
                Self::CatalogScan => "0",
                Self::Arcade => "A",
                Self::ArcadeCrossfade => "headless",
                Self::BackgroundScan => "B",
                Self::Compatibility => "C",
                Self::Loading => "L",
                Self::MediaProgress => "M",
                Self::ParticleScreensaver => "P",
                Self::Fireworks => "headless",
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
            KeyCode::KeyC => Some(Scenario::Compatibility),
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
            (Scenario::Settings, 4, _) => Some(Scenario::About),
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

    struct PreviewOptions {
        scenario: Scenario,
        frame: u64,
        output: Option<PathBuf>,
        refresh_rate: RefreshRate,
        firework: String,
        firework_spec: Option<PathBuf>,
        time_ms: Option<u64>,
        hud: bool,
    }

    impl PreviewOptions {
        fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
            let mut scenario = Scenario::Home;
            let mut frame = 0;
            let mut output = None;
            let mut refresh_rate = RefreshRate::Auto;
            let mut firework = "oled-peony".to_owned();
            let mut firework_spec = None;
            let mut time_ms = None;
            let mut hud = true;
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
                    "--refresh-rate" => {
                        let value = arguments
                            .next()
                            .ok_or("--refresh-rate requires auto, 60, or 120")?;
                        refresh_rate = RefreshRate::parse(&value)?;
                    }
                    "--firework" => {
                        firework = arguments.next().ok_or("--firework requires a preset id")?;
                    }
                    "--firework-spec" => {
                        firework_spec = Some(PathBuf::from(
                            arguments
                                .next()
                                .ok_or("--firework-spec requires a file path")?,
                        ));
                    }
                    "--time-ms" => {
                        let value = arguments.next().ok_or("--time-ms requires milliseconds")?;
                        time_ms = Some(
                            value
                                .parse::<u64>()
                                .map_err(|_| format!("invalid time in milliseconds {value:?}"))?,
                        );
                    }
                    "--hud" => {
                        let value = arguments.next().ok_or("--hud requires on or off")?;
                        hud = match value.as_str() {
                            "on" => true,
                            "off" => false,
                            _ => return Err("--hud requires on or off".into()),
                        };
                    }
                    "--help" | "-h" => {
                        return Err(
                            "usage: mister-magik-ui-preview [--scenario NAME] [--refresh-rate auto|60|120] [--frame N | --time-ms N] [--firework ID] [--firework-spec FILE.json] [--hud on|off] --output FILE.ppm"
                                .into(),
                        );
                    }
                    other => return Err(format!("unknown preview argument {other:?}")),
                }
            }
            if frame > 0 && output.is_none() {
                return Err("--frame requires --output".into());
            }
            if time_ms.is_some() && output.is_none() {
                return Err("--time-ms requires --output".into());
            }
            if time_ms.is_some() && scenario != Scenario::Fireworks {
                return Err("--time-ms is only available for the fireworks scenario".into());
            }
            Ok(Self {
                scenario,
                frame,
                output,
                refresh_rate,
                firework,
                firework_spec,
                time_ms,
                hud,
            })
        }
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

    fn initialize_bridge(bridge: &MisterBridge) {
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
        bridge.set_display_options(strings(&[
            "Current mode",
            "1920×1080 60 Hz",
            "1280×720 60 Hz",
            "CRT 240p 60 Hz",
        ]));
        bridge.set_display_active_label("1920×1080 60 Hz".into());
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
            Scenario::Arcade => 2,
            Scenario::Settings => 3,
            Scenario::About => 4,
            Scenario::Licenses => 5,
            Scenario::Info => 6,
            Scenario::ScreensaverSettings => 7,
            _ => 0,
        });
        bridge.set_effective_view(
            match scenario {
                Scenario::Arcade => "arcade",
                Scenario::Settings => "settings",
                Scenario::Controller | Scenario::ControllerSetup => "controller",
                Scenario::About => "about",
                Scenario::Licenses => "licenses",
                Scenario::Info => "info",
                Scenario::ScreensaverSettings => "screensaver-settings",
                Scenario::ParticleScreensaver | Scenario::Fireworks | Scenario::ScreenshotTiles => {
                    "screensaver"
                }
                _ => "home",
            }
            .into(),
        );
        bridge.set_menu_title("MiSTer MagiK".into());
        bridge.set_menu_breadcrumb("Systems".into());
        if !scenario.uses_launcher_navigation() {
            bridge.set_menu_items(home_menu_items(0));
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
        bridge.set_info_kernel_version("Linux 6.6.68-MiSTer".into());
        bridge.set_info_database_build("1,284 ms · 12,846 games".into());

        match scenario {
            Scenario::Startup => bridge.set_startup_visible(true),
            Scenario::Confirm => {
                bridge.set_confirm_visible(true);
                bridge.set_confirm_title("Rebuild game database?".into());
                bridge.set_confirm_message(
                    "Existing catalog data will be replaced after the next launcher start.".into(),
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
            Scenario::Compatibility => {
                bridge.set_compatibility_visible(true);
                bridge.set_compatibility_reason("Display route needs attention".into());
                bridge.set_compatibility_detail(
                    "Previewing the launcher while the production route is unavailable.".into(),
                );
            }
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
        bridge.set_compatibility_visible(false);
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

    fn home_menu_items(selected: usize) -> ModelRc<MenuItem> {
        let definitions = [
            ("arcade", "Arcade", "2,184 games", MenuItemStatus::Ready),
            ("console", "Consoles", "6,420 games", MenuItemStatus::Ready),
            (
                "computer",
                "Computers",
                "Scanning…",
                MenuItemStatus::Scanning,
            ),
            (
                "handheld",
                "Handhelds",
                "1,126 games",
                MenuItemStatus::Partial,
            ),
            ("favorites", "Favourites", "42 games", MenuItemStatus::Ready),
            (
                "recent",
                "Recently Added",
                "Scan failed",
                MenuItemStatus::Failed,
            ),
        ];
        ModelRc::new(VecModel::from(
            definitions
                .into_iter()
                .enumerate()
                .map(|(index, (id, label, subtitle, status))| MenuItem {
                    id: id.into(),
                    label: label.into(),
                    subtitle: subtitle.into(),
                    focused: index == selected,
                    available: status != MenuItemStatus::Failed,
                    node_kind: MenuItemKind::Collection,
                    status,
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

    fn apply_arcade_fixture_bridge(
        launcher: &Launcher,
        games: &[ArcadeGameEntry],
        selected: usize,
    ) {
        let bridge = launcher.global::<MisterBridge>();
        bridge.set_active_system_title("Arcade".into());
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

        #[test]
        fn rgb565_primary_channels_expand_to_xrgb8888() {
            assert_eq!(rgb565_to_xrgb8888(Rgb565Pixel(0xf800)), 0x00ff0000);
            assert_eq!(rgb565_to_xrgb8888(Rgb565Pixel(0x07e0)), 0x0000ff00);
            assert_eq!(rgb565_to_xrgb8888(Rgb565Pixel(0x001f)), 0x000000ff);
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
        }

        #[test]
        fn preview_options_parse_exact_firework_capture() {
            let options = PreviewOptions::parse(
                [
                    "--scenario",
                    "fireworks",
                    "--firework",
                    "oled-peony",
                    "--time-ms",
                    "2000",
                    "--hud",
                    "off",
                    "--output",
                    "oled.ppm",
                ]
                .map(String::from),
            )
            .unwrap();
            assert_eq!(options.scenario, Scenario::Fireworks);
            assert_eq!(options.firework, "oled-peony");
            assert_eq!(options.time_ms, Some(2000));
            assert!(!options.hud);
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
                activated_scenario(Scenario::Settings, 4, false),
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

        fn prepared_nav() -> (UiPreviewFixtures, LauncherNav) {
            let fixtures = UiPreviewFixtures::new().expect("preview fixtures");
            let mut nav = LauncherNav::new();
            nav.catalog_build_started();
            for system_id in &fixtures.shell_system_ids {
                nav.catalog_system_discovered(system_id);
                nav.catalog_system_ready(system_id);
            }
            nav.sync_launcher_taxonomy(&fixtures.catalog);
            nav.catalog_build_finished(&fixtures.catalog);
            (fixtures, nav)
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
                nav.handle_input(
                    &pad,
                    epoch + elapsed_for_frame(u64::from(frame), refresh_hz),
                    &fixtures.catalog,
                );
            }
            pad = PadState::default();
            for frame in refresh_hz..=refresh_hz * 2 {
                nav.handle_input(
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
                nav.handle_input(
                    &pad,
                    epoch + elapsed_for_frame(u64::from(frame), refresh_hz),
                    &fixtures.catalog,
                );
            }
            pad = PadState::default();
            for frame in refresh_hz..=refresh_hz * 2 {
                nav.handle_input(
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
