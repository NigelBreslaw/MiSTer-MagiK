// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deliberately small consumer application for Tooling 2.0.

use mister_magik_mister_runtime::framebuffer::mapped::MappedRgb565Framebuffer;
use slint::platform::software_renderer::{RepaintBufferType, Rgb565Pixel, SoftwareRenderer};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};
use slint::{EventLoopError, PhysicalSize, Window};
use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    let mut framebuffer =
        MappedRgb565Framebuffer::open_current_rgb565().map_err(|error| error.to_string())?;
    let window = ProbeWindow::new();
    slint::platform::set_platform(Box::new(ProbePlatform {
        window: window.clone(),
        start: Instant::now(),
    }))
    .map_err(|error| error.to_string())?;

    let probe = Probe::new().map_err(|error| error.to_string())?;
    window.set_size(PhysicalSize::new(
        framebuffer
            .width()
            .try_into()
            .map_err(|error: std::num::TryFromIntError| error.to_string())?,
        framebuffer
            .height()
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
    let timer = motion_timer.clone();
    let weak = probe.as_weak();
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
        let weak = probe.as_weak();
        let callback_timer = timer.clone();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                let Some(probe) = weak.upgrade() else {
                    callback_timer.stop();
                    return;
                };
                let next = probe.get_motion_step() + 1;
                probe.set_motion_step(next);
                if next >= 600 {
                    probe.set_motion_running(false);
                    probe.set_motion_complete(true);
                    callback_timer.stop();
                    eprintln!("magik2-probe motion-complete frames={next}");
                }
            },
        );
        eprintln!("magik2-probe motion-start");
    });

    let width = framebuffer.width();
    let height = framebuffer.height();
    let mut cached = vec![Rgb565Pixel(0); width * height];
    let mut presentations = 0u64;
    loop {
        slint::platform::update_timers_and_animations();
        let rendered = window.draw_if_needed(|renderer| {
            renderer.render(&mut cached, width);
            framebuffer
                .present_rows_565(&cached, 0, height)
                .expect("present cached RGB565 frame");
            presentations += 1;
        });
        if presentations == 1 {
            write_readiness(width, height, presentations)?;
            eprintln!(
                "magik2-probe ready width={width} height={height} presentations={presentations}"
            );
        }
        if !rendered {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn write_readiness(width: usize, height: usize, presentations: u64) -> Result<(), String> {
    let state_root = std::env::var("MISTER_MAGIK2_STATE_ROOT")
        .unwrap_or_else(|_| "/tmp/mister-magik2".to_owned());
    let root = std::path::PathBuf::from(state_root);
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let ready = root.join("probe-ready.json");
    let temporary = root.join("probe-ready.json.next");
    std::fs::write(
        &temporary,
        format!("{{\"width\":{width},\"height\":{height},\"presentations\":{presentations}}}\n"),
    )
    .map_err(|error| error.to_string())?;
    std::fs::rename(temporary, ready).map_err(|error| error.to_string())
}
