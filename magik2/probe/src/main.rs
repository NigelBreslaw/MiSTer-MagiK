// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deliberately small consumer application for Tooling 2.0.

use mister_magik_framebuffer_stream::{
    FrameGeometry, FrameHeader, FrameKind, FrameRect, write_frame as write_preview_frame,
};
use mister_magik_mister_runtime::framebuffer::mapped::MappedRgb565Framebuffer;
use mister_magik_mister_runtime::framebuffer::vsync::VsyncWaitStatus;
use slint::platform::software_renderer::{RepaintBufferType, Rgb565Pixel, SoftwareRenderer};
use slint::platform::{EventLoopProxy, Platform, WindowAdapter};
use slint::{EventLoopError, PhysicalSize, Window};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::Write;
use std::os::unix::net::UnixStream;
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

#[derive(Default)]
struct PresentationMetrics {
    presentations: u64,
    render_us_total: u64,
    last_render_us: u64,
    vsync_hits: u64,
    vsync_misses: u64,
    motion_started_ms: Option<u64>,
    motion_completed_ms: Option<u64>,
}

struct PreviewProducer {
    state_root: std::path::PathBuf,
    last_preview: Instant,
    sequence: u64,
}

impl PreviewProducer {
    fn new() -> Self {
        Self {
            state_root: std::env::var("MISTER_MAGIK2_STATE_ROOT")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| "/tmp/mister-magik2".into()),
            last_preview: Instant::now() - Duration::from_secs(1),
            sequence: 0,
        }
    }

    fn publish_if_watched(
        &mut self,
        pixels: &[Rgb565Pixel],
        width: usize,
        height: usize,
        elapsed: Duration,
    ) {
        if self.last_preview.elapsed() < Duration::from_millis(200) || !self.viewer_is_active() {
            return;
        }
        let Ok(width) = u32::try_from(width) else {
            return;
        };
        let Ok(height) = u32::try_from(height) else {
            return;
        };
        let raw_bytes = pixels.len().saturating_mul(2);
        let Ok(raw_bytes) = u32::try_from(raw_bytes) else {
            return;
        };
        let mut bytes = Vec::with_capacity(raw_bytes as usize);
        for pixel in pixels {
            bytes.extend_from_slice(&pixel.0.to_ne_bytes());
        }
        self.sequence += 1;
        let geometry = FrameGeometry {
            width,
            height,
            stride_pixels: width,
        };
        let header = FrameHeader {
            kind: FrameKind::Keyframe,
            flags: 0,
            sequence: self.sequence,
            timestamp_us: elapsed.as_micros() as u64,
            geometry,
            rect: FrameRect::full(geometry),
            raw_bytes,
            payload_bytes: raw_bytes,
        };
        if let Ok(mut socket) = UnixStream::connect(self.state_root.join("probe-frames.sock"))
            && write_preview_frame(&mut socket, header, &bytes).is_ok()
            && socket.flush().is_ok()
        {
            self.last_preview = Instant::now();
        }
    }

    fn viewer_is_active(&self) -> bool {
        let Ok(deadline) = std::fs::read_to_string(self.state_root.join("viewer-lease")) else {
            return false;
        };
        let Ok(deadline) = deadline.trim().parse::<u128>() else {
            return false;
        };
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .is_ok_and(|now| now.as_millis() < deadline)
    }
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
    probe.show().map_err(|error| error.to_string())?;
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
    let metrics = Rc::new(RefCell::new(PresentationMetrics::default()));
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
        metrics_for_motion.borrow_mut().motion_started_ms =
            Some(metrics_start.elapsed().as_millis() as u64);
        let weak = probe.as_weak();
        let callback_timer = timer.clone();
        let metrics_for_completion = metrics_for_motion.clone();
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
                if next >= 1_800 {
                    probe.set_motion_running(false);
                    probe.set_motion_complete(true);
                    metrics_for_completion.borrow_mut().motion_completed_ms =
                        Some(metrics_start.elapsed().as_millis() as u64);
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
    let mut previews = PreviewProducer::new();
    loop {
        slint::platform::update_timers_and_animations();
        let metrics_for_frame = metrics.clone();
        let rendered = window.draw_if_needed(|renderer| {
            let render_start = Instant::now();
            renderer.render(&mut cached, width);
            let vsync = framebuffer.wait_vsync();
            framebuffer
                .present_rows_565(&cached, 0, height)
                .expect("present cached RGB565 frame");
            previews.publish_if_watched(&cached, width, height, metrics_start.elapsed());
            let mut metrics = metrics_for_frame.borrow_mut();
            metrics.presentations += 1;
            metrics.last_render_us = render_start.elapsed().as_micros() as u64;
            metrics.render_us_total += metrics.last_render_us;
            match vsync {
                VsyncWaitStatus::Hit { .. } => metrics.vsync_hits += 1,
                VsyncWaitStatus::Timeout { .. } | VsyncWaitStatus::Error { .. } => {
                    metrics.vsync_misses += 1
                }
            }
        });
        let metrics = metrics.borrow();
        if rendered && metrics.presentations == 1 {
            write_readiness(width, height, metrics.presentations)?;
            eprintln!(
                "magik2-probe ready width={width} height={height} presentations={}",
                metrics.presentations
            );
        }
        if rendered
            && (metrics.presentations == 1
                || metrics.presentations % 5 == 0
                || probe.get_motion_complete())
        {
            write_metrics(width, height, metrics_start.elapsed(), &metrics)?;
        }
        drop(metrics);
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

fn write_metrics(
    width: usize,
    height: usize,
    elapsed: Duration,
    metrics: &PresentationMetrics,
) -> Result<(), String> {
    let state_root = std::env::var("MISTER_MAGIK2_STATE_ROOT")
        .unwrap_or_else(|_| "/tmp/mister-magik2".to_owned());
    let root = std::path::PathBuf::from(state_root);
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let destination = root.join("probe-metrics.json");
    let temporary = root.join("probe-metrics.json.next");
    std::fs::write(
        &temporary,
        format!(
            "{{\"width\":{},\"height\":{},\"elapsed_ms\":{},\"presentations\":{},\"render_us_total\":{},\"last_render_us\":{},\"vsync_hits\":{},\"vsync_misses\":{},\"motion_started_ms\":{},\"motion_completed_ms\":{}}}\n",
            width,
            height,
            elapsed.as_millis(),
            metrics.presentations,
            metrics.render_us_total,
            metrics.last_render_us,
            metrics.vsync_hits,
            metrics.vsync_misses,
            option_json(metrics.motion_started_ms),
            option_json(metrics.motion_completed_ms),
        ),
    )
    .map_err(|error| error.to_string())?;
    std::fs::rename(temporary, destination).map_err(|error| error.to_string())
}

fn option_json(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}
