// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deliberately small consumer application for Tooling 2.0.

use mister_magik_framebuffer_scenes::Rgb565Pixel as ScenePixel;
use mister_magik_framebuffer_scenes::launcher::{LauncherCard, LauncherData, LauncherScene};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase,
};
use mister_magik_mister_runtime::display_plan::query_main_display_plan;
use mister_magik_mister_runtime::framebuffer::hidden_latch::HiddenLatchPresenter;
use mister_magik_mister_runtime::framebuffer::rgb565::Rgb565;
use mister_magik_mister_runtime::main_input::{
    INPUT_BATCH_CAPACITY, MainInputDirection, MainInputPhase, MainProxyInput,
};
mod launcher_control;
use launcher_control::LauncherControl;
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
    // A supervised child inherits Main's display contracts. Do not request the
    // command FIFO while Main is waiting for this child's ready acknowledgement.
    let plan = if std::env::var_os("MISTER_MAGIK_STARTUP_TOKEN").is_some() {
        let mut fpga =
            mister_magik_mister_runtime::fpga::Fpga::open().map_err(|e| e.to_string())?;
        mister_magik_mister_runtime::display_plan::detect_runtime_display_plan(&mut fpga)
            .map_err(|e| e.to_string())?
            .plan
    } else {
        query_main_display_plan().map_err(|error| format!("resolve Main display: {error}"))?
    };
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
        if let Some(probe) = weak.upgrade()
            && !probe.get_launcher_mode()
        {
            probe.set_counter(probe.get_counter() + 1);
        }
    });
    let weak = probe.as_weak();
    probe.on_reset(move || {
        if let Some(probe) = weak.upgrade()
            && !probe.get_launcher_mode()
        {
            probe.set_counter(0);
        }
    });
    let weak = probe.as_weak();
    probe.on_toggle_details(move || {
        if let Some(probe) = weak.upgrade()
            && !probe.get_launcher_mode()
        {
            probe.set_details_open(!probe.get_details_open());
        }
    });
    let motion_timer = Rc::new(slint::Timer::default());
    let launcher_mode = Rc::new(Cell::new(true));
    let launcher_dirty = Rc::new(Cell::new(true));
    let launcher_scene = LauncherScene::new(width, height);
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
    let mut prepared = launcher_scene.prepare(LauncherData {
        cards: &launcher_cards,
        selected: 0,
        library_games: 6842,
        collections: 18,
        favourites: 126,
        clock: "21:37",
    });
    let launcher_labels: Vec<slint::SharedString> =
        launcher_cards.iter().map(|card| card.name.into()).collect();
    let control = Rc::new(RefCell::new(LauncherControl::new(launcher_cards.len())));
    let launcher_epoch = Rc::new(Instant::now());
    let measure_running = Rc::new(Cell::new(false));
    let measure_direction = Rc::new(Cell::new(BrowseDirection::Right));
    let session = Rc::new(RefCell::new(
        Session::from_environment().ok_or("missing tooling state root")?,
    ));
    probe.set_launcher_mode(true);
    probe.set_launcher_ready(false);
    probe.set_launcher_selection("ARCADE".into());

    let install_mode = |launcher: bool| {
        let mode = launcher_mode.clone();
        let dirty = launcher_dirty.clone();
        let control = control.clone();
        let weak = probe.as_weak();
        let timer = motion_timer.clone();
        let measure = measure_running.clone();
        let session = session.clone();
        move || {
            mode.set(launcher);
            dirty.set(launcher);
            control.borrow_mut().reset(false);
            timer.stop();
            measure.set(false);
            let mut session = session.borrow_mut();
            session.metrics.motion_started_ms = None;
            session.metrics.window_start = None;
            session.metrics.window = None;
            if let Some(probe) = weak.upgrade() {
                probe.set_launcher_mode(launcher);
                probe.set_launcher_ready(false);
                probe.set_motion_running(false);
                probe.set_launcher_measurement("idle".into());
            }
        }
    };
    probe.on_show_launcher(install_mode(true));
    probe.on_show_probe(install_mode(false));

    let install_input = |direction, down| {
        let control = control.clone();
        let mode = launcher_mode.clone();
        let epoch = launcher_epoch.clone();
        move || {
            if mode.get() {
                control.borrow_mut().input(
                    false,
                    direction,
                    down,
                    epoch.elapsed().as_millis() as u64,
                );
            }
        }
    };
    probe.on_launcher_left_down(install_input(BrowseDirection::Left, true));
    probe.on_launcher_left_up(install_input(BrowseDirection::Left, false));
    probe.on_launcher_right_down(install_input(BrowseDirection::Right, true));
    probe.on_launcher_right_up(install_input(BrowseDirection::Right, false));
    let reset_control = control.clone();
    let dirty = launcher_dirty.clone();
    let mode = launcher_mode.clone();
    probe.on_launcher_input_reset(move || {
        if mode.get() {
            reset_control.borrow_mut().reset(true);
            dirty.set(true);
        }
    });
    let browser = control.clone();
    let mode = launcher_mode.clone();
    let epoch = launcher_epoch.clone();
    let measure = measure_running.clone();
    let direction = measure_direction.clone();
    let session_for_launcher = session.clone();
    let weak = probe.as_weak();
    let dirty = launcher_dirty.clone();
    probe.on_start_launcher_motion(move |right| {
        if !mode.get() || measure.get() {
            return;
        }
        let chosen = if right {
            BrowseDirection::Right
        } else {
            BrowseDirection::Left
        };
        browser.borrow_mut().reset(true);
        browser
            .borrow_mut()
            .input(false, chosen, true, epoch.elapsed().as_millis() as u64);
        direction.set(chosen);
        measure.set(true);
        dirty.set(true);
        let mut session = session_for_launcher.borrow_mut();
        session.metrics.context["workload"] = "launcher-slide".into();
        if let Some(context) = session.metrics.context.as_object_mut() {
            for name in [
                "launcher_telemetry_error",
                "launcher_owned_vblanks",
                "launcher_presented_vblanks",
                "launcher_repeated_vblanks",
                "launcher_ownership_losses",
            ] {
                context.remove(name);
            }
        }
        session.begin();
        if let Some(probe) = weak.upgrade() {
            probe.set_launcher_measurement("running".into());
        }
    });
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
        if probe.get_launcher_mode() || probe.get_motion_running() {
            return;
        }
        probe.set_motion_step(0);
        probe.set_motion_complete(false);
        probe.set_motion_running(true);
        session_for_motion.borrow_mut().metrics.context["workload"] = "probe-motion".into();
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
    let mut scene_pixels = vec![ScenePixel(0); width * height];
    if std::env::var_os("MISTER_MAGIK_STARTUP_TOKEN").is_some() {
        prepared.render_into(control.borrow_mut().frame(0), &mut scene_pixels);
        let startup_pixels: Vec<_> = scene_pixels.iter().map(|p| Rgb565(p.0)).collect();
        mister_magik_mister_runtime::main_ready::notify_main_ready(
            &mut framebuffer,
            &startup_pixels,
        )?;
    }
    let mut launcher_input: Option<MainProxyInput> = None;
    let mut physical_input_edges = 0_u64;
    let mut input_events = Vec::with_capacity(INPUT_BATCH_CAPACITY);
    let mut next_input_open_ms = 0;
    let mut last_presented: Option<BrowseFrame> = None;
    let mut pending: Option<(Option<BrowseFrame>, Instant)> = None;
    let mut telemetry_start: Option<(u32, u32, u32, u32)> = None;
    loop {
        window.event_loop.process_pending_callbacks();
        slint::platform::update_timers_and_animations();
        if window.event_loop.terminated.load(Ordering::Acquire) {
            return Ok(());
        }
        let now_ms = launcher_epoch.elapsed().as_millis() as u64;
        if control.borrow().rearm_input {
            control.borrow_mut().rearm_input = false;
            launcher_input = None;
            next_input_open_ms = 0;
        }
        if launcher_input.is_none() && now_ms >= next_input_open_ms {
            next_input_open_ms = now_ms.saturating_add(1000);
            match MainProxyInput::open() {
                Ok(input) => launcher_input = Some(input),
                Err(error) => {
                    session.borrow_mut().metrics.context["input_error"] = error.to_string().into();
                }
            }
        }
        if let Some(input) = launcher_input.as_mut() {
            match input.poll_into(&mut input_events) {
                Ok(()) => {
                    if launcher_mode.get() {
                        for event in input_events.drain(..) {
                            physical_input_edges += 1;
                            let mut session = session.borrow_mut();
                            session.metrics.context["physical_input_edges"] =
                                physical_input_edges.into();
                            session.metrics.context["physical_input_last"] =
                                format!("{:?}:{:?}", event.direction, event.phase).into();
                            drop(session);
                            let direction = match event.direction {
                                MainInputDirection::Left => BrowseDirection::Left,
                                MainInputDirection::Right => BrowseDirection::Right,
                            };
                            control.borrow_mut().input(
                                true,
                                direction,
                                event.phase == MainInputPhase::Pressed,
                                now_ms,
                            );
                        }
                    }
                }
                Err(error) => {
                    input_events.clear();
                    control.borrow_mut().reset(false);
                    control.borrow_mut().rearm_input = false;
                    launcher_dirty.set(launcher_mode.get());
                    launcher_input = None;
                    next_input_open_ms = now_ms.saturating_add(1000);
                    session.borrow_mut().metrics.context["input_error"] = error.to_string().into();
                }
            }
        }
        let input_source = match launcher_input.as_ref() {
            Some(input) if input.ready() => "main-proxy",
            Some(_) => "main-proxy-awaiting-neutral",
            None => "main-proxy-unavailable",
        };
        if probe.get_launcher_input_source() != input_source {
            probe.set_launcher_input_source(input_source.into());
            session.borrow_mut().metrics.context["input_source"] = input_source.into();
        }
        let completed = session.borrow_mut().tick(width, height)?;
        if measure_running.get() {
            if session.borrow().metrics.window_start.is_none() {
                telemetry_start = None;
            }
            if telemetry_start.is_none() && session.borrow().metrics.window_start.is_some() {
                match framebuffer.presentation_telemetry() {
                    Ok(t)
                        if t.lifetime_invariant_valid() && t.magik_ownership() && !t.pending() =>
                    {
                        telemetry_start = Some((
                            t.owned_vblank_count,
                            t.presented_vblank_count,
                            t.repeated_vblank_count,
                            t.ownership_loss_count,
                        ));
                    }
                    _ => {
                        session.borrow_mut().metrics.context["launcher_telemetry_error"] =
                            "invalid start telemetry".into()
                    }
                }
            }
            if completed {
                if let Some((owned, presented, repeated, losses)) = telemetry_start.take() {
                    match framebuffer.presentation_telemetry() {
                        Ok(t)
                            if t.lifetime_invariant_valid()
                                && t.magik_ownership()
                                && !t.pending() =>
                        {
                            let mut session = session.borrow_mut();
                            let c = &mut session.metrics.context;
                            c["launcher_owned_vblanks"] =
                                u64::from(t.owned_vblank_count.wrapping_sub(owned)).into();
                            c["launcher_presented_vblanks"] =
                                u64::from(t.presented_vblank_count.wrapping_sub(presented)).into();
                            c["launcher_repeated_vblanks"] =
                                u64::from(t.repeated_vblank_count.wrapping_sub(repeated)).into();
                            c["launcher_ownership_losses"] =
                                u64::from(t.ownership_loss_count.wrapping_sub(losses)).into();
                        }
                        _ => {
                            session.borrow_mut().metrics.context["launcher_telemetry_error"] =
                                "invalid end telemetry".into()
                        }
                    }
                }
                control
                    .borrow_mut()
                    .input(false, measure_direction.get(), false, now_ms);
                measure_running.set(false);
                probe.set_launcher_measurement("draining".into());
            }
        } else if completed {
            motion_timer.stop();
            probe.set_motion_running(false);
            probe.set_motion_complete(true);
        }

        let redraw_requested = window.redraw_pending.replace(false);
        let frame = if pending.is_none() {
            Some(control.borrow_mut().frame(now_ms))
        } else {
            None
        };
        if pending.is_none() {
            let launcher_frame = frame.expect("no pending presentation");
            let render_launcher = launcher_mode.get()
                && (launcher_dirty.get() || last_presented != Some(launcher_frame));
            if render_launcher || (!launcher_mode.get() && redraw_requested) {
                let render_start = Instant::now();
                if render_launcher {
                    probe.set_launcher_ready(false);
                    prepared.render_into(launcher_frame, &mut scene_pixels);
                    for (destination, source) in cached.iter_mut().zip(&scene_pixels) {
                        *destination = Rgb565Pixel(source.0);
                    }
                } else {
                    window.renderer.render(&mut cached, width);
                }
                let render_us = render_start.elapsed().as_micros() as u64;
                let stride = framebuffer.stride_pixels();
                for row in 0..height {
                    for (destination, source) in framebuffer.pixels_mut()
                        [row * stride..row * stride + width]
                        .iter_mut()
                        .zip(&cached[row * width..(row + 1) * width])
                    {
                        *destination = Rgb565(source.0);
                    }
                }
                let mut session = session.borrow_mut();
                session.metrics.last_render_us = render_us;
                session.metrics.counters.render_us += render_us;
                match framebuffer.post() {
                    Ok(_) => {
                        session.metrics.counters.posts += 1;
                        pending = Some((render_launcher.then_some(launcher_frame), render_start));
                    }
                    Err(error) => {
                        session.metrics.counters.rejections += 1;
                        session.metrics.error = Some(error.to_string());
                        launcher_dirty.set(true);
                        window.redraw_pending.set(true);
                    }
                }
            }
        }
        let mut presented_this_loop = false;
        if let Some((posted_frame, started)) = pending {
            match framebuffer.settle_pending() {
                Ok(Some(receipt)) => {
                    pending = None;
                    presented_this_loop = true;
                    let mut session = session.borrow_mut();
                    let metrics = &mut session.metrics;
                    metrics.counters.flips += 1;
                    metrics.counters.presentations += 1;
                    metrics.counters.drops +=
                        metrics.last_physical_drop_count.map_or(0, |previous| {
                            u64::from(receipt.drop_count.wrapping_sub(previous))
                        });
                    metrics.last_physical_drop_count = Some(receipt.drop_count);
                    metrics.counters.render_to_present_us += started.elapsed().as_micros() as u64;
                    if let Some(frame) = posted_frame {
                        last_presented = Some(frame);
                        metrics.context["selected_category"] =
                            launcher_cards[frame.selected].name.into();
                        launcher_dirty.set(false);
                        if probe.get_launcher_selection() != launcher_labels[frame.selected] {
                            probe.set_launcher_selection(launcher_labels[frame.selected].clone());
                        }
                        probe.set_launcher_selected_index(frame.selected as i32);
                        if probe.get_launcher_target() != launcher_labels[frame.target] {
                            probe.set_launcher_target(launcher_labels[frame.target].clone());
                        }
                        let phase = match frame.phase {
                            BrowsePhase::Settled => "settled",
                            BrowsePhase::Sliding => "sliding",
                            BrowsePhase::Held => "held",
                        };
                        if probe.get_launcher_motion_state() != phase {
                            probe.set_launcher_motion_state(phase.into());
                        }
                        probe.set_launcher_ready(frame.phase == BrowsePhase::Settled);
                        if frame.phase == BrowsePhase::Settled
                            && probe.get_launcher_measurement() == "draining"
                        {
                            probe.set_launcher_measurement("complete".into());
                        }
                    } else {
                        last_presented = None;
                    }
                }
                Ok(None) => {
                    session.borrow_mut().metrics.error =
                        Some("posted frame missing pending latch".into());
                    pending = None;
                    launcher_dirty.set(true);
                }
                Err(error) => {
                    session.borrow_mut().metrics.error = Some(error.to_string());
                    // Retain pending state and retry settlement before touching a slot.
                }
            }
        }
        session.borrow_mut().preview(&cached, width, height);
        if !presented_this_loop {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
