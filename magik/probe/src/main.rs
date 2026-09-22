// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deliberately small consumer application for Tooling.

use mister_magik_core::display::{DisplayGeometry, ResolvedDisplayPlan};
use mister_magik_mister_runtime::framebuffer::damage::{DirtyRect, DirtyRectList};
use mister_magik_mister_runtime::framebuffer::hidden_latch::CachedHiddenLatchPresenter;
use mister_magik_mister_runtime::framebuffer::rgb565::Rgb565;
use mister_magik_visual_concepts::Preset;
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

mod concepts;
mod measurement;

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
    let display = std::env::var("MISTER_MAGIK_MINI_DISPLAY_PLAN")
        .map_err(|_| "native service lacks mini-display-plan-v1")?;
    let fields = display.split(',').collect::<Vec<_>>();
    if fields.len() != 3 {
        return Err("invalid Mini display plan".into());
    }
    let geometry = DisplayGeometry::new(
        fields[1].parse().map_err(|_| "invalid output width")?,
        fields[2].parse().map_err(|_| "invalid output height")?,
    );
    let plan = ResolvedDisplayPlan::from_mode_or_detected(fields[0], Some(geometry))
        .ok_or("invalid Main display plan")?;
    let width = plan.render_w;
    let height = plan.render_h;
    let mut framebuffer = CachedHiddenLatchPresenter::open(plan).map_err(|e| e.to_string())?;
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

    let concepts = Rc::new(RefCell::new(concepts::Concepts::new(width, height)));
    if std::env::var_os("MISTER_MAGIK_MINI_RESUME_CONCEPT").is_some() {
        let root =
            std::env::var_os("MISTER_MAGIK2_STATE_ROOT").ok_or("missing concept state root")?;
        let bytes = std::fs::read(std::path::PathBuf::from(root).join("mini-concept.json"))
            .map_err(|e| e.to_string())?;
        let state: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let name = state["name"]
            .as_str()
            .ok_or("missing retained concept name")?;
        let preset = Preset::parse(state["preset"].as_str().ok_or("missing retained preset")?)?;
        concepts.borrow_mut().select(name, preset);
        if let Some(error) = &concepts.borrow().error {
            return Err(error.clone());
        }
    }
    let control = concepts.clone();
    probe.on_concept_select(move |name, preset| {
        let mut c = control.borrow_mut();
        if plan.output_route.is_crt() {
            c.error = Some("concepts require HDMI".into());
            return;
        }
        match Preset::parse(&preset) {
            Ok(p) => c.select(&name, p),
            Err(e) => c.error = Some(e),
        }
    });
    let control = concepts.clone();
    probe.on_concept_action(move |action| control.borrow_mut().action(&action));
    let mut evidence = measurement::Evidence::default();
    let mut cached = vec![Rgb565Pixel(0); width * height];
    loop {
        window.event_loop.process_pending_callbacks();
        if window.event_loop.terminated.load(Ordering::Acquire) {
            return Ok(());
        }
        slint::platform::update_timers_and_animations();
        {
            let mut c = concepts.borrow_mut();
            probe.set_concept_name(c.name.clone().into());
            probe.set_concept_generation(c.generation);
            probe.set_concept_error(c.error.clone().unwrap_or_default().into());
            probe.set_concept_paused(c.paused);
            if c.measure {
                c.measure = false;
                session.borrow_mut().set_measurement_duration(Some(30_000));
                session.borrow_mut().begin();
                probe.set_concept_measuring(true);
            }
        }

        measurement::resources(&mut session.borrow_mut().metrics);
        if session.borrow_mut().tick(width, height)? {
            concepts.borrow_mut().paused = true;
            probe.set_concept_measuring(false);
            motion_timer.stop();
            probe.set_motion_running(false);
            probe.set_motion_complete(true);
        }
        let mut c = concepts.borrow_mut();
        let is_concept = c.scene.is_some();
        let should_render = if is_concept {
            !c.paused || c.dirty
        } else {
            window.redraw_pending.replace(false)
        };
        let rendered = should_render;
        if should_render {
            if is_concept && !c.paused && c.advance_next {
                c.scene
                    .as_mut()
                    .unwrap()
                    .advance(Duration::from_nanos(16_666_667));
            }
            let started = Instant::now();
            let damage = if is_concept {
                c.dirty = false;
                let scene = c.scene.as_mut().unwrap();
                let d = scene.render()?;
                probe.set_concept_frame((scene.elapsed().as_millis().min(i32::MAX as u128)) as i32);
                session.borrow_mut().metrics.context = serde_json::json!({"concept":c.name,"preset":c.preset.name(),"route":plan.output_route.label(),"storage_bytes":c.scene.as_ref().unwrap().storage_bytes()});
                DirtyRectList::from_one(DirtyRect {
                    x0: d.x0,
                    y0: d.y0,
                    x1: d.x1,
                    y1: d.y1,
                })
            } else {
                window.renderer.render(&mut cached, width);
                DirtyRectList::from_one(DirtyRect {
                    x0: 0,
                    y0: 0,
                    x1: width,
                    y1: height,
                })
            };
            let render_us = started.elapsed().as_micros() as u64;
            // SAFETY: both pixel wrappers are repr(transparent) u16; neither owns resources.
            let pixels = if let Some(scene) = &c.scene {
                // SAFETY: the portable RGB565 pixel is also repr(transparent) u16.
                unsafe {
                    std::slice::from_raw_parts(
                        scene.pixels().as_ptr().cast::<Rgb565>(),
                        scene.pixels().len(),
                    )
                }
            } else {
                unsafe {
                    std::slice::from_raw_parts(cached.as_ptr().cast::<Rgb565>(), cached.len())
                }
            };
            let transfer = Instant::now();
            framebuffer
                .prepare_cached(pixels, &damage)
                .map_err(|e| e.to_string())?;
            let transfer_us = transfer.elapsed().as_micros() as u64;
            framebuffer.post_prepared().map_err(|e| e.to_string())?;
            let presented = framebuffer
                .settle_pending()
                .map_err(|e| e.to_string())?
                .ok_or("latch did not settle")?;
            let mut session = session.borrow_mut();
            let metrics = &mut session.metrics;
            metrics.counters.posts += 1;
            metrics.counters.flips += 1;
            match framebuffer.presentation_telemetry() {
                Ok(sample) => evidence.observe(sample, presented.drop_count, metrics),
                Err(error) => metrics.error = Some(error.to_string()),
            }
            metrics.counters.presentations += 1;
            metrics.last_render_us = render_us;
            metrics.counters.render_us += render_us;
            metrics.counters.transfer_us += transfer_us;
            metrics.counters.render_to_present_us += started.elapsed().as_micros() as u64;
            if metrics.window_start.is_some() && metrics.window.is_none() {
                metrics.frame_timings_us.push([
                    render_us,
                    transfer_us,
                    started.elapsed().as_micros() as u64,
                ]);
            }
            if is_concept {
                c.advance_next = true;
                if c.stop_at
                    .is_some_and(|at| c.scene.as_ref().unwrap().elapsed() >= at)
                {
                    c.stop_at = None;
                    c.paused = true;
                }
            }
        }
        // Qualification never publishes previews. Interactive watch borrows the
        // retained concept buffer rather than allocating a Slint image.
        if !probe.get_concept_measuring() {
            if let Some(scene) = &c.scene {
                // SAFETY: these transparent wrappers have identical layout.
                let pixels = unsafe {
                    std::slice::from_raw_parts(
                        scene.pixels().as_ptr().cast::<Rgb565Pixel>(),
                        scene.pixels().len(),
                    )
                };
                session.borrow_mut().preview(pixels, width, height);
            } else {
                session.borrow_mut().preview(&cached, width, height);
            }
        }
        drop(c);
        if !rendered {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
