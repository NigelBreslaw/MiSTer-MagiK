// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(target_os = "macos")]
mod macos {
    use mister_magik_fb::arcade_catalog::ArcadeGameEntry;
    use mister_magik_fb::framebuffer::target::{FramebufferTargetGeometry, UiFrameTarget};
    use mister_magik_fb::visual_composition::{
        ArcadeVisualLayer, PreviewFrame, PreviewPixels, PreviewSurface, compose_preview_frame,
        hdmi_preview_rect,
    };
    use mister_magik_fb::visual_platform::{MisterPlatform, MisterSoftwareWindow};
    use mister_magik_ui::launcher::{
        ArcadeGame, Launcher, MenuItem, MenuItemKind, MenuItemStatus, MisterBridge, MisterUi,
        ScreenshotPackProgress,
    };
    use slint::platform::software_renderer::{RepaintBufferType, Rgb565Pixel};
    use slint::{ComponentHandle, ModelRc, PhysicalSize, SharedString, VecModel};
    use softbuffer::{Context, Surface};
    use std::error::Error;
    use std::num::NonZeroU32;
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
    const FRAME_PERIOD: Duration = Duration::from_nanos(16_666_667);

    pub fn run() -> Result<(), Box<dyn Error>> {
        let slint_window = MisterSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(MisterPlatform::new(
            Rc::clone(&slint_window),
            None,
        )))?;
        slint_window.set_size(PhysicalSize::new(FRAME_WIDTH as u32, FRAME_HEIGHT as u32));

        let launcher = Launcher::new()?;
        let ui = launcher.global::<MisterUi>();
        ui.set_window_width(FRAME_WIDTH as i32);
        ui.set_window_height(FRAME_HEIGHT as i32);
        ui.set_crt_layout(false);
        let bridge = launcher.global::<MisterBridge>();
        initialize_bridge(&bridge);
        apply_scenario(&launcher, Scenario::Home);
        launcher.show()?;
        slint_window.request_redraw();

        let event_loop = EventLoop::new()?;
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME_PERIOD));
        let mut application = PreviewApplication::new(launcher, slint_window, Scenario::Home);
        event_loop.run_app(&mut application)?;
        Ok(())
    }

    struct PreviewApplication {
        launcher: Launcher,
        slint_window: Rc<MisterSoftwareWindow>,
        native_window: Option<Arc<Window>>,
        surface: Option<Surface<Arc<Window>, Arc<Window>>>,
        frame_target: UiFrameTarget,
        xrgb8888: Vec<u32>,
        scenario: Scenario,
        selection: usize,
        arcade_layer: ArcadeVisualLayer,
        arcade_games: Vec<ArcadeGameEntry>,
        preview_pixels: Vec<Rgb565Pixel>,
    }

    impl PreviewApplication {
        fn new(
            launcher: Launcher,
            slint_window: Rc<MisterSoftwareWindow>,
            scenario: Scenario,
        ) -> Self {
            Self {
                launcher,
                slint_window,
                native_window: None,
                surface: None,
                frame_target: UiFrameTarget::cached(FramebufferTargetGeometry::new(
                    FRAME_WIDTH,
                    FRAME_HEIGHT,
                )),
                xrgb8888: Vec::new(),
                scenario,
                selection: 0,
                arcade_layer: ArcadeVisualLayer::new(FRAME_WIDTH, FRAME_HEIGHT),
                arcade_games: fixture_arcade_games(),
                preview_pixels: fixture_preview_pixels(160, 120),
            }
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
        }

        fn window_title(&self) -> String {
            format!(
                "MiSTer MagiK UI Preview — {} — {}",
                self.scenario.label(),
                self.scenario.shortcut()
            )
        }

        fn select_scenario(&mut self, scenario: Scenario) {
            self.scenario = scenario;
            self.selection = 0;
            apply_scenario(&self.launcher, scenario);
            if scenario == Scenario::Arcade {
                apply_arcade_bridge(&self.launcher, &self.arcade_games, self.selection);
                self.arcade_layer.invalidate();
            }
            self.update_selection();
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
                Scenario::ScreensaverSettings => 2,
                Scenario::Arcade => self.arcade_games.len(),
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
            bridge.set_settings_selected(self.selection as i32);
            bridge.set_about_selected(self.selection as i32);
            bridge.set_licenses_selected(self.selection as i32);
            bridge.set_screensaver_settings_selected(self.selection as i32);
            bridge.set_confirm_selected(self.selection.min(1) as i32);
            if self.scenario == Scenario::Arcade {
                apply_arcade_bridge(&self.launcher, &self.arcade_games, self.selection);
            }
            if matches!(
                self.scenario,
                Scenario::Home | Scenario::BackgroundScan | Scenario::Confirm
            ) {
                bridge.set_menu_items(home_menu_items(self.selection));
            }
            self.slint_window.request_redraw();
        }

        fn handle_key(&mut self, code: KeyCode) {
            let scenario = match code {
                KeyCode::Digit1 => Some(Scenario::Home),
                KeyCode::Digit2 => Some(Scenario::Settings),
                KeyCode::Digit3 => Some(Scenario::Controller),
                KeyCode::Digit4 => Some(Scenario::About),
                KeyCode::Digit5 => Some(Scenario::Licenses),
                KeyCode::Digit6 => Some(Scenario::Info),
                KeyCode::Digit7 => Some(Scenario::ScreensaverSettings),
                KeyCode::Digit8 => Some(Scenario::Startup),
                KeyCode::Digit9 => Some(Scenario::Confirm),
                KeyCode::Digit0 => Some(Scenario::CatalogScan),
                KeyCode::KeyA => Some(Scenario::Arcade),
                KeyCode::KeyB => Some(Scenario::BackgroundScan),
                KeyCode::KeyC => Some(Scenario::Compatibility),
                KeyCode::KeyL => Some(Scenario::Loading),
                KeyCode::KeyM => Some(Scenario::MediaProgress),
                KeyCode::KeyS => Some(Scenario::ControllerSetup),
                _ => None,
            };
            if let Some(scenario) = scenario {
                self.select_scenario(scenario);
                return;
            }
            match code {
                KeyCode::ArrowUp | KeyCode::ArrowLeft => self.move_selection(-1),
                KeyCode::ArrowDown | KeyCode::ArrowRight => self.move_selection(1),
                _ => {}
            }
        }

        fn render(&mut self) {
            slint::platform::update_timers_and_animations();
            self.slint_window.request_redraw();
            self.slint_window.draw_if_needed(|renderer| {
                self.frame_target.render(
                    renderer,
                    FramebufferTargetGeometry::new(FRAME_WIDTH, FRAME_HEIGHT),
                );
            });
            if self.scenario == Scenario::Arcade {
                self.arcade_layer.compose(
                    &mut self.frame_target,
                    &self.arcade_games,
                    self.selection,
                    true,
                );
                compose_preview_frame(
                    self.frame_target.cached_565_mut(),
                    FRAME_WIDTH,
                    FRAME_HEIGHT,
                    hdmi_preview_rect(FRAME_WIDTH, FRAME_HEIGHT),
                    PreviewFrame {
                        pixels: PreviewPixels::Rgb565 {
                            pixels: &self.preview_pixels,
                            stride_pixels: 160,
                        },
                        source_width: 160,
                        source_height: 120,
                        display_width: 320,
                        display_height: 240,
                    },
                    true,
                    PreviewSurface::full(FRAME_WIDTH),
                );
            }

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
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed && !event.repeat =>
                {
                    if let PhysicalKey::Code(code) = event.physical_key {
                        self.handle_key(code);
                    }
                }
                WindowEvent::Resized(_) => {
                    if let Some(window) = self.native_window.as_ref() {
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }

        fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME_PERIOD));
            if let Some(window) = self.native_window.as_ref() {
                window.request_redraw();
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Scenario {
        Home,
        Arcade,
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
    }

    impl Scenario {
        fn label(self) -> &'static str {
            match self {
                Self::Home => "Home",
                Self::Arcade => "Arcade",
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
                Self::BackgroundScan => "B",
                Self::Compatibility => "C",
                Self::Loading => "L",
                Self::MediaProgress => "M",
                Self::ControllerSetup => "S",
            }
        }
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
                _ => "home",
            }
            .into(),
        );
        bridge.set_menu_title("MiSTer MagiK".into());
        bridge.set_menu_breadcrumb("Systems".into());
        bridge.set_menu_items(home_menu_items(0));
        bridge.set_selected_index(0);
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

    fn apply_arcade_bridge(launcher: &Launcher, games: &[ArcadeGameEntry], selected: usize) {
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
        bridge.set_arcade_selected(selected as i32);
        bridge.set_arcade_scroll_y((selected as i32 * 48).saturating_sub(5 * 48));
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

    fn fixture_arcade_games() -> Vec<ArcadeGameEntry> {
        const TITLES: [&str; 16] = [
            "1942",
            "Alien Syndrome",
            "Bubble Bobble",
            "Centipede",
            "Donkey Kong",
            "Elevator Action",
            "Frogger",
            "Galaga",
            "Hyper Sports",
            "Ikari Warriors",
            "Joust",
            "Klax",
            "Metal Slug",
            "Out Run",
            "Pac-Man",
            "R-Type",
        ];
        (0..48)
            .map(|index| {
                let title = if index < TITLES.len() {
                    TITLES[index].to_string()
                } else {
                    format!("Fixture Arcade Game {:02}", index + 1)
                };
                ArcadeGameEntry {
                    title: Arc::from(title),
                    mra_path: Arc::from(format!("/fixture/{index:02}.mra")),
                    preview_archive_path: Arc::from("/fixture/arcade.zip"),
                    preview_asset_key: Arc::from(format!("fixture-{index:02}")),
                    has_preview: index % 7 != 0,
                    system_id: Arc::from("arcade"),
                    year: Some(1980 + (index % 20) as u16),
                    manufacturer: Arc::from(if index % 2 == 0 { "Sega" } else { "Namco" }),
                    players: Some(if index % 3 == 0 { 2 } else { 1 }),
                    control: Arc::from("Joystick"),
                    is_new: index < 3,
                }
            })
            .collect()
    }

    fn fixture_preview_pixels(width: usize, height: usize) -> Vec<Rgb565Pixel> {
        let mut pixels = vec![Rgb565Pixel(0); width * height];
        for y in 0..height {
            for x in 0..width {
                let red = ((x * 31) / width) as u16;
                let green = ((y * 63) / height) as u16;
                let checker = ((x / 12) + (y / 12)) % 2;
                let blue = if checker == 0 { 8 } else { 28 };
                pixels[y * width + x] = Rgb565Pixel((red << 11) | (green << 5) | blue);
            }
        }
        pixels
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
