// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deliberately small consumer application for Tooling 2.0.

use mister_magik_mister_runtime::framebuffer::hidden_latch::HiddenLatchPresenter;
use mister_magik_mister_runtime::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_mister_runtime::framebuffer::rgb565::Rgb565;
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

    fn draw_if_needed(&self, render: impl FnOnce(&SoftwareRenderer)) -> bool {
        self.event_loop.process_pending_callbacks();
        if self.redraw_pending.replace(false) {
            render(&self.renderer);
            true
        } else {
            false
        }
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
    let direct_framebuffer =
        MappedRgb565Framebuffer::open_current_rgb565().map_err(|error| error.to_string())?;
    let width = direct_framebuffer.width();
    let height = direct_framebuffer.height();
    drop(direct_framebuffer);
    let mut framebuffer = HiddenLatchPresenter::open(
        u16::try_from(width).map_err(|error: std::num::TryFromIntError| error.to_string())?,
        u16::try_from(height).map_err(|error: std::num::TryFromIntError| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
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
    let session = Rc::new(RefCell::new(
        Session::from_environment().ok_or("missing tooling state root")?,
    ));
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
    loop {
        slint::platform::update_timers_and_animations();
        if session.borrow_mut().tick(width, height)? {
            motion_timer.stop();
            probe.set_motion_running(false);
            probe.set_motion_complete(true);
        }
        let session_for_frame = session.clone();
        let rendered = window.draw_if_needed(|renderer| {
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
