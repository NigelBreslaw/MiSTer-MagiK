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

mod measurement;
mod preview;
mod profile;
use measurement::PresentationMetrics;
use preview::PreviewProducer;
use profile::CpuProfile;

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
    let metrics = Rc::new(RefCell::new(PresentationMetrics::default()));
    let mut profile: Option<CpuProfile> = None;
    let instrumented = std::env::var_os("MISTER_MAGIK2_PROFILE_DIR").is_some();
    let measured_ms = if instrumented { 10_000 } else { 5_000 };
    let metrics_start = Instant::now();
    let timer = motion_timer.clone();
    let weak = probe.as_weak();
    let metrics_for_motion = metrics.clone();
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
        let mut metrics = metrics_for_motion.borrow_mut();
        metrics.motion_started_ms = Some(metrics_start.elapsed().as_millis() as u64);
        metrics.window_start = None;
        metrics.window = None;
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
    let mut previews = PreviewProducer::new();
    loop {
        slint::platform::update_timers_and_animations();
        let now = metrics_start.elapsed().as_millis() as u64;
        if probe.get_motion_running() {
            let mut counters = metrics.borrow_mut();
            if counters.window_start.is_none()
                && counters
                    .motion_started_ms
                    .is_some_and(|start| now - start >= 2000)
            {
                counters.window_start = Some((now, counters.counters.clone()));
                match CpuProfile::start() {
                    Ok(session) => profile = session,
                    Err(error) => counters.error = Some(error),
                }
            }
            if counters
                .window_start
                .as_ref()
                .is_some_and(|(start, _)| now - start >= measured_ms)
            {
                counters.finish_window(now, width, height, instrumented);
                motion_timer.stop();
                probe.set_motion_running(false);
                probe.set_motion_complete(true);
                if let Some(session) = profile.take()
                    && let Err(error) = session.finish()
                {
                    counters.error = Some(error);
                }
            }
        }
        let metrics_for_frame = metrics.clone();
        let rendered = window.draw_if_needed(|renderer| {
            let render_start = Instant::now();
            renderer.render(&mut cached, width);
            let render_us = render_start.elapsed().as_micros() as u64;
            for (destination, source) in framebuffer.pixels_mut().iter_mut().zip(&cached) {
                *destination = Rgb565(source.0);
            }
            let mut metrics = metrics_for_frame.borrow_mut();
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
        let metrics = metrics.borrow();
        if rendered && metrics.counters.presentations == 1 {
            write_readiness(width, height, metrics.counters.presentations)?;
            eprintln!(
                "magik2-probe ready width={width} height={height} presentations={}",
                metrics.counters.presentations
            );
        }
        if rendered
            && (metrics.counters.presentations == 1
                || metrics.counters.presentations % 5 == 0
                || probe.get_motion_complete())
        {
            write_metrics(width, height, metrics_start.elapsed(), &metrics)?;
        }
        drop(metrics);
        previews.publish_if_watched(&cached, width, height, metrics_start.elapsed());
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
        format!("{{\"pid\":{},\"sha256\":\"{}\",\"width\":{width},\"height\":{height},\"presentations\":{presentations}}}\n", std::process::id(), std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256").unwrap_or_default()),
    )
    .map_err(|error| error.to_string())?;
    std::fs::rename(temporary, ready).map_err(|error| error.to_string())
}

fn write_metrics(
    width: usize,
    height: usize,
    elapsed: Duration,
    metrics: &PresentationMetrics,
) -> Result<(), String> {
    let root = std::path::PathBuf::from(
        std::env::var("MISTER_MAGIK2_STATE_ROOT").unwrap_or_else(|_| "/tmp/mister-magik2".into()),
    );
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let temporary = root.join("probe-metrics.json.next");
    std::fs::write(
        &temporary,
        metrics
            .json(width, height, elapsed.as_millis() as u64)
            .to_string(),
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, root.join("probe-metrics.json")).map_err(|e| e.to_string())
}
