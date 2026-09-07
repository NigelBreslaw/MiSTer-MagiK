// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deliberately small consumer application for Tooling 2.0.

use mister_magik_framebuffer_scenes::Rgb565Pixel as ScenePixel;
use mister_magik_framebuffer_scenes::launcher::{LauncherCard, LauncherData, LauncherScene};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowsePhase, LauncherBrowser,
};
use mister_magik_mister_runtime::display_plan::query_main_display_plan;
use mister_magik_mister_runtime::framebuffer::hidden_latch::HiddenLatchPresenter;
use mister_magik_mister_runtime::framebuffer::rgb565::Rgb565;
use mister_magik_mister_runtime::main_input::{MainInputDirection, MainInputPhase, MainProxyInput};
use slint::platform::software_renderer::{RepaintBufferType, Rgb565Pixel, SoftwareRenderer};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};
use slint::{EventLoopError, PhysicalSize, Window};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mister_magik_tooling_support::Session;

slint::include_modules!();

type EventLoopCallback = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone, Default)]
struct ProbeEventLoop {
    callbacks: Arc<Mutex<VecDeque<EventLoopCallback>>>,
    terminated: Arc<AtomicBool>,
}

impl ProbeEventLoop {
    fn process_pending_callbacks(&self) {
        loop {
            let callback = self
                .callbacks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front();
            let Some(callback) = callback else {
                return;
            };
            callback();
        }
    }
}

impl EventLoopProxy for ProbeEventLoop {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        self.terminated.store(true, Ordering::Release);
        Ok(())
    }

    fn invoke_from_event_loop(&self, event: EventLoopCallback) -> Result<(), EventLoopError> {
        if self.terminated.load(Ordering::Acquire) {
            return Err(EventLoopError::EventLoopTerminated);
        }
        self.callbacks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(event);
        Ok(())
    }
}

struct ProbeWindow {
    window: Window,
    renderer: SoftwareRenderer,
    redraw_pending: Cell<bool>,
    size: Cell<PhysicalSize>,
    event_loop: ProbeEventLoop,
}

impl ProbeWindow {
    fn new() -> Rc<Self> {
        Rc::new_cyclic(|weak: &Weak<Self>| Self {
            window: Window::new(weak.clone()),
            renderer: SoftwareRenderer::new_with_repaint_buffer_type(
                RepaintBufferType::ReusedBuffer,
            ),
            redraw_pending: Cell::new(false),
            size: Cell::new(PhysicalSize::default()),
            event_loop: ProbeEventLoop::default(),
        })
    }

    fn draw_if_needed(&self, render: impl FnOnce(&SoftwareRenderer, bool)) -> bool {
        self.event_loop.process_pending_callbacks();
        let redraw_requested = self.redraw_pending.replace(false);
        render(&self.renderer, redraw_requested);
        redraw_requested
    }

    fn set_size(&self, size: PhysicalSize) {
        self.window.set_size(size);
    }
}

impl WindowAdapter for ProbeWindow {
    fn window(&self) -> &Window {
        &self.window
    }

    fn renderer(&self) -> &dyn slint::platform::Renderer {
        &self.renderer
    }

    fn size(&self) -> PhysicalSize {
        self.size.get()
    }

    fn set_size(&self, size: slint::WindowSize) {
        let scale_factor = self.window.scale_factor();
        self.size.set(size.to_physical(scale_factor));
        self.window
            .dispatch_event(slint::platform::WindowEvent::Resized {
                size: size.to_logical(scale_factor),
            });
    }

    fn request_redraw(&self) {
        self.redraw_pending.set(true);
    }
}

impl std::ops::Deref for ProbeWindow {
    type Target = Window;

    fn deref(&self) -> &Self::Target {
        &self.window
    }
}

struct ProbePlatform {
    window: Rc<ProbeWindow>,
    start: Instant,
}

impl Platform for ProbePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(self.window.event_loop.clone()))
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

fn main() -> Result<(), String> {
    let plan =
        query_main_display_plan().map_err(|error| format!("resolve Main display: {error}"))?;
    let (width, height) = (plan.fb_w, plan.fb_h);
    let (scan_width, scan_height) = (plan.scan_w, plan.scan_h);
    let mut framebuffer =
        HiddenLatchPresenter::open_for_plan(plan).map_err(|error| error.to_string())?;
    eprintln!(
        "mini-display source={width}x{height} destination={}x{} authority=main-display-state",
        framebuffer.destination_width(),
        framebuffer.destination_height(),
    );
    let window = ProbeWindow::new();
    slint::platform::set_platform(Box::new(ProbePlatform {
        window: window.clone(),
        start: Instant::now(),
    }))
    .map_err(|error| error.to_string())?;

    let probe = Probe::new().map_err(|error| error.to_string())?;
    probe.set_build_label(
        std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256")
            .unwrap_or_else(|_| "host-preview".into())
            .into(),
    );
    probe.show().map_err(|error| error.to_string())?;
    window.set_size(PhysicalSize::new(
        width
            .try_into()
            .map_err(|error: std::num::TryFromIntError| error.to_string())?,
        height
            .try_into()
            .map_err(|error: std::num::TryFromIntError| error.to_string())?,
    ));
    let weak = probe.as_weak();
    probe.on_increment(move || {
        if let Some(probe) = weak.upgrade() {
            probe.set_counter(probe.get_counter() + 1);
        }
    });
    let weak = probe.as_weak();
    probe.on_reset(move || {
        if let Some(probe) = weak.upgrade() {
            probe.set_counter(0);
        }
    });
    let weak = probe.as_weak();
    probe.on_toggle_details(move || {
        if let Some(probe) = weak.upgrade() {
            probe.set_details_open(!probe.get_details_open());
        }
    });
    let motion_timer = Rc::new(slint::Timer::default());
    let launcher_mode = Rc::new(Cell::new(true));
    let launcher_dirty = Rc::new(Cell::new(true));
    let launcher_scene = LauncherScene::new(width, height);
    let launcher_cached = Rc::new(RefCell::new(None::<Vec<ScenePixel>>));
    let launcher_cards = [
        LauncherCard {
            name: "ARCADE",
            games: 1752,
            colour: 0x88a6,
        },
        LauncherCard {
            name: "SNK NEOGEO",
            games: 324,
            colour: 0x195f,
        },
        LauncherCard {
            name: "CONSOLES",
            games: 842,
            colour: 0xc5b5,
        },
        LauncherCard {
            name: "HANDHELDS",
            games: 126,
            colour: 0x2c92,
        },
        LauncherCard {
            name: "COMPUTERS",
            games: 86,
            colour: 0xb9a6,
        },
    ];
    let launcher_browser = Rc::new(RefCell::new(LauncherBrowser::new(launcher_cards.len(), 0)));
    launcher_browser.borrow_mut().neutral();
    let launcher_input = Rc::new(RefCell::new(MainProxyInput::open().ok()));
    probe.set_launcher_input_source(if launcher_input.borrow().is_some() {
        "main-proxy".into()
    } else {
        "main-proxy-unavailable".into()
    });
    probe.set_launcher_mode(true);
    probe.set_launcher_ready(false);
    probe.set_launcher_selection("ARCADE".into());
    let mode = launcher_mode.clone();
    let dirty = launcher_dirty.clone();
    let weak = probe.as_weak();
    let timer = motion_timer.clone();
    probe.on_show_launcher(move || {
        mode.set(true);
        dirty.set(true);
        timer.stop();
        if let Some(probe) = weak.upgrade() {
            probe.set_launcher_mode(true);
            probe.set_launcher_ready(false);
            probe.set_motion_running(false);
        }
    });
    let launcher_epoch = Rc::new(Instant::now());
    let weak = probe.as_weak();
    let browser = launcher_browser.clone();
    let dirty = launcher_dirty.clone();
    let epoch = launcher_epoch.clone();
    probe.on_launcher_left_down(move || {
        browser
            .borrow_mut()
            .press(BrowseDirection::Left, epoch.elapsed().as_millis() as u64);
        dirty.set(true);
        if let Some(probe) = weak.upgrade() {
            probe.set_launcher_motion_state("sliding".into());
        }
    });
    let browser = launcher_browser.clone();
    probe.on_launcher_left_up(move || browser.borrow_mut().release(BrowseDirection::Left));
    let weak = probe.as_weak();
    let browser = launcher_browser.clone();
    let dirty = launcher_dirty.clone();
    let epoch = launcher_epoch.clone();
    probe.on_launcher_right_down(move || {
        browser
            .borrow_mut()
            .press(BrowseDirection::Right, epoch.elapsed().as_millis() as u64);
        dirty.set(true);
        if let Some(probe) = weak.upgrade() {
            probe.set_launcher_motion_state("sliding".into());
        }
    });
    let browser = launcher_browser.clone();
    probe.on_launcher_right_up(move || browser.borrow_mut().release(BrowseDirection::Right));
    let browser = launcher_browser.clone();
    let dirty = launcher_dirty.clone();
    probe.on_launcher_input_reset(move || {
        browser.borrow_mut().reset();
        dirty.set(true);
    });
    let mode = launcher_mode.clone();
    let dirty = launcher_dirty.clone();
    let weak = probe.as_weak();
    let timer = motion_timer.clone();
    probe.on_show_probe(move || {
        mode.set(false);
        dirty.set(false);
        timer.stop();
        if let Some(probe) = weak.upgrade() {
            probe.set_launcher_mode(false);
            probe.set_launcher_ready(false);
            probe.set_motion_running(false);
        }
    });
    let session = Rc::new(RefCell::new(
        Session::from_environment().ok_or("missing tooling state root")?,
    ));
    {
        let mut session = session.borrow_mut();
        let context = &mut session.metrics.context;
        context["source_width"] = (width as u64).into();
        context["source_height"] = (height as u64).into();
        context["scan_width"] = u64::from(scan_width).into();
        context["scan_height"] = u64::from(scan_height).into();
        context["destination_width"] = (framebuffer.destination_width() as u64).into();
        context["destination_height"] = (framebuffer.destination_height() as u64).into();
    }
    let timer = motion_timer.clone();
    let weak = probe.as_weak();
    let session_for_motion = session.clone();
    probe.on_start_motion(move || {
        let Some(probe) = weak.upgrade() else {
            return;
        };
        if probe.get_motion_running() {
            return;
        }
        probe.set_motion_step(0);
        probe.set_motion_complete(false);
        probe.set_motion_running(true);
        session_for_motion.borrow_mut().begin();
        let weak = probe.as_weak();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                if let Some(probe) = weak.upgrade() {
                    probe.set_motion_step(probe.get_motion_step() + 1);
                }
            },
        );
    });

    let mut cached = vec![Rgb565Pixel(0); width * height];
    let mut launcher_input = launcher_input.borrow_mut().take();
    let mut input_events = Vec::with_capacity(8);
    let mut last_input_open_ms = 0_u64;
    loop {
        let now_ms = launcher_epoch.elapsed().as_millis() as u64;
        if let Some(input) = launcher_input.as_mut() {
            if input.poll_into(&mut input_events).is_err() {
                launcher_browser.borrow_mut().reset();
                launcher_dirty.set(true);
                launcher_input = None;
                probe.set_launcher_input_source("main-proxy-unavailable".into());
            }
            for event in input_events.drain(..) {
                let mut browser = launcher_browser.borrow_mut();
                match (event.direction, event.phase) {
                    (MainInputDirection::Left, MainInputPhase::Pressed) => {
                        browser.press(BrowseDirection::Left, now_ms)
                    }
                    (MainInputDirection::Left, MainInputPhase::Released) => {
                        browser.release(BrowseDirection::Left)
                    }
                    (MainInputDirection::Right, MainInputPhase::Pressed) => {
                        browser.press(BrowseDirection::Right, now_ms)
                    }
                    (MainInputDirection::Right, MainInputPhase::Released) => {
                        browser.release(BrowseDirection::Right)
                    }
                }
                launcher_dirty.set(true);
            }
        } else if now_ms.saturating_sub(last_input_open_ms) >= 1_000 {
            last_input_open_ms = now_ms;
            if let Ok(input) = MainProxyInput::open() {
                launcher_input = Some(input);
                probe.set_launcher_input_source("main-proxy".into());
            }
        }
        let launcher_frame = launcher_browser.borrow_mut().frame(now_ms);
        if launcher_frame.phase != BrowsePhase::Settled {
            launcher_dirty.set(true);
        }
        probe.set_launcher_motion_state(
            match launcher_frame.phase {
                BrowsePhase::Settled => "settled",
                BrowsePhase::Sliding => "sliding",
                BrowsePhase::Held => "held",
            }
            .into(),
        );
        if launcher_frame.selected < launcher_cards.len() {
            probe.set_launcher_selection(launcher_cards[launcher_frame.selected].name.into());
        }
        if launcher_frame.target < launcher_cards.len() {
            probe.set_launcher_target(launcher_cards[launcher_frame.target].name.into());
        }
        slint::platform::update_timers_and_animations();
        if session.borrow_mut().tick(width, height)? {
            motion_timer.stop();
            probe.set_motion_running(false);
            probe.set_motion_complete(true);
        }
        let session_for_frame = session.clone();
        let mode_for_frame = launcher_mode.clone();
        let dirty_for_frame = launcher_dirty.clone();
        let probe_for_frame = probe.as_weak();
        let launcher_cached_for_frame = launcher_cached.clone();
        let launcher_frame_for_frame = launcher_frame;
        let rendered = window.draw_if_needed(|renderer, redraw_requested| {
            if mode_for_frame.get() {
                if !dirty_for_frame.replace(false) {
                    return;
                }
                *launcher_cached_for_frame.borrow_mut() = None;
                let render_start = Instant::now();
                if launcher_cached_for_frame.borrow().is_none() {
                    *launcher_cached_for_frame.borrow_mut() = Some(
                        launcher_scene.render_browse(
                            LauncherData {
                                cards: &launcher_cards,
                                selected: launcher_frame_for_frame.selected,
                                library_games: 6842,
                                collections: 18,
                                favourites: 126,
                                clock: "21:37",
                            },
                            (launcher_frame_for_frame.phase != BrowsePhase::Settled)
                                .then_some(launcher_frame_for_frame),
                        ),
                    );
                }
                let packed = launcher_cached_for_frame.borrow();
                let packed = packed
                    .as_deref()
                    .expect("launcher cache was just populated");
                for (destination, source) in cached.iter_mut().zip(packed) {
                    *destination = Rgb565Pixel(source.0);
                }
                let render_us = render_start.elapsed().as_micros() as u64;
                let stride = framebuffer.stride_pixels();
                for row in 0..height {
                    let destination =
                        &mut framebuffer.pixels_mut()[row * stride..row * stride + width];
                    let source = &cached[row * width..(row + 1) * width];
                    for (destination, source) in destination.iter_mut().zip(source) {
                        *destination = Rgb565(source.0);
                    }
                }
                let mut session = session_for_frame.borrow_mut();
                let metrics = &mut session.metrics;
                metrics.last_render_us = render_us;
                metrics.counters.render_us += render_us;
                match framebuffer.post() {
                    Ok(_) => metrics.counters.posts += 1,
                    Err(error) => {
                        metrics.counters.rejections += 1;
                        metrics.error = Some(error.to_string());
                        return;
                    }
                }
                match framebuffer.settle_pending() {
                    Ok(Some(presented)) => {
                        metrics.counters.flips += 1;
                        metrics.counters.drops +=
                            metrics.last_physical_drop_count.map_or(0, |previous| {
                                u64::from(presented.drop_count.wrapping_sub(previous))
                            });
                        metrics.last_physical_drop_count = Some(presented.drop_count);
                        metrics.counters.presentations += 1;
                        metrics.counters.render_to_present_us +=
                            render_start.elapsed().as_micros() as u64;
                        if let Some(probe) = probe_for_frame.upgrade() {
                            probe.set_launcher_ready(true);
                        }
                    }
                    _ => metrics.error = Some("physical latch did not settle".into()),
                }
                return;
            }
            if !redraw_requested {
                return;
            }
            let render_start = Instant::now();
            renderer.render(&mut cached, width);
            let render_us = render_start.elapsed().as_micros() as u64;
            for (destination, source) in framebuffer.pixels_mut().iter_mut().zip(&cached) {
                *destination = Rgb565(source.0);
            }
            let mut session = session_for_frame.borrow_mut();
            let metrics = &mut session.metrics;
            match framebuffer.post() {
                Ok(_) => metrics.counters.posts += 1,
                Err(error) => {
                    metrics.counters.rejections += 1;
                    metrics.error = Some(error.to_string());
                    return;
                }
            }
            match framebuffer.settle_pending() {
                Ok(Some(presented)) => {
                    metrics.counters.flips += 1;
                    metrics.counters.drops +=
                        metrics.last_physical_drop_count.map_or(0, |previous| {
                            u64::from(presented.drop_count.wrapping_sub(previous))
                        });
                    metrics.last_physical_drop_count = Some(presented.drop_count);
                }
                _ => {
                    metrics.error = Some("physical latch did not settle".into());
                    return;
                }
            }
            metrics.counters.presentations += 1;
            metrics.last_render_us = render_us;
            metrics.counters.render_us += render_us;
            metrics.counters.render_to_present_us += render_start.elapsed().as_micros() as u64;
        });
        session.borrow_mut().preview(&cached, width, height);
        if !rendered {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
