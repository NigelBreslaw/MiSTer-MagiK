// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

mod agent_client;
mod analytics_ui_state;
mod app_state;
mod file_icons;
mod frame_profile;
#[cfg_attr(not(feature = "compiled-ui"), allow(dead_code))]
mod framebuffer_cadence;
mod library;
#[cfg(target_os = "macos")]
mod macos_display_clock;
#[cfg(target_os = "macos")]
mod macos_titlebar;
mod platform_lifecycle;
mod realtime_frame_chart;
mod sd_card;
mod stream_lifecycle;

use agent_client::{
    DeviceTelemetrySample, DeviceTelemetryStreamControl, FramebufferStreamControl,
    connect_device_telemetry_stream, connect_framebuffer_stream, connect_framebuffer_stream_seeded,
    drain_framebuffer_stream, drain_framebuffer_stream_for, fetch_dashboard,
    fetch_framebuffer_capture, fetch_sd_directory, fetch_sd_item_detail,
};
use app_state::{DEFAULT_HOST, DashboardSnapshot};
use framebuffer_cadence::{CadenceEventKind, FramebufferCadenceTrace};
use realtime_frame_chart::{FrameChartState, FrameSample, RenderedFrameChart};
use sd_card::SdCardBrowser;
#[cfg(feature = "compiled-ui")]
use sd_card::SdTreeRow;
#[cfg(feature = "live-ui")]
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::error::Error;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicI32, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const SD_FOLDER_LOADING_DELAY: Duration = Duration::from_millis(150);

type SharedSdBrowser = Arc<Mutex<SdCardBrowser>>;
type SharedLibraryBrowser = Arc<Mutex<LibraryBrowser>>;
type SharedFramebufferCapture = Arc<Mutex<Option<agent_client::FramebufferCapture>>>;
type SharedLiveStreamGeneration = Arc<AtomicU64>;
type SharedFramebufferStreamControl = Arc<Mutex<Option<(u64, FramebufferStreamControl)>>>;
type SharedRealtimeStreamGeneration = Arc<AtomicU64>;
type SharedRealtimeStreamControl = Arc<Mutex<Option<(u64, DeviceTelemetryStreamControl)>>>;
type SharedRealtimeFrameChart = Arc<Mutex<FrameChartState>>;
type SharedDisplayClockCallback = Rc<RefCell<Box<dyn FnMut(FramebufferDisplayClockTick)>>>;

const DIRTY_RECT_LINGER_FRAMES: usize = 8;
const MAX_DIRTY_RECT_OVERLAYS: usize = 12;
const REALTIME_HISTORY_CAPACITY: usize = 300;
const REALTIME_FRAME_SAMPLE_CAPACITY: usize = 1200;
const REALTIME_IDLE_FRAME_COLUMNS_PER_SAMPLE: u64 = 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FramebufferDisplayClockTick {
    timestamp_us: u64,
    target_timestamp_us: u64,
    duration_us: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum FramebufferDisplayClockKind {
    #[default]
    Unknown = 0,
    MacosDisplayLink = 1,
    SlintTimer = 2,
}

impl FramebufferDisplayClockKind {
    #[cfg_attr(not(feature = "compiled-ui"), allow(dead_code))]
    fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::MacosDisplayLink => "macos-cadisplaylink",
            Self::SlintTimer => "slint-timer",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::MacosDisplayLink,
            2 => Self::SlintTimer,
            _ => Self::Unknown,
        }
    }
}

#[allow(dead_code)] // Variants own the active clock for their Drop/lifetime behavior.
enum FramebufferDisplayClock {
    #[cfg(target_os = "macos")]
    Macos(macos_display_clock::MacDisplayClock),
    Timer(slint::Timer),
}

impl FramebufferDisplayClock {
    fn kind(&self) -> FramebufferDisplayClockKind {
        match self {
            #[cfg(target_os = "macos")]
            Self::Macos(_) => FramebufferDisplayClockKind::MacosDisplayLink,
            Self::Timer(_) => FramebufferDisplayClockKind::SlintTimer,
        }
    }
}

fn start_framebuffer_display_clock(
    window: &slint::Window,
    callback: impl FnMut(FramebufferDisplayClockTick) + 'static,
) -> FramebufferDisplayClock {
    struct Adapter<'a> {
        window: &'a slint::Window,
        callback: SharedDisplayClockCallback,
    }

    impl platform_lifecycle::DisplayClockAdapter for Adapter<'_> {
        type Clock = FramebufferDisplayClock;

        fn start_native(&mut self) -> Option<Self::Clock> {
            #[cfg(target_os = "macos")]
            {
                use slint::winit_030::WinitWindowAccessor;
                let callback = Rc::clone(&self.callback);
                let mac_callback = Rc::new(RefCell::new(Box::new(
                    move |tick: macos_display_clock::MacDisplayLinkTick| {
                        (callback.borrow_mut())(FramebufferDisplayClockTick {
                            timestamp_us: tick.timestamp_us,
                            target_timestamp_us: tick.target_timestamp_us,
                            duration_us: tick.duration_us,
                        });
                    },
                )
                    as Box<dyn FnMut(macos_display_clock::MacDisplayLinkTick)>));
                self.window
                    .with_winit_window(|window| {
                        macos_display_clock::MacDisplayClock::start(window, mac_callback)
                    })
                    .flatten()
                    .map(FramebufferDisplayClock::Macos)
            }
            #[cfg(not(target_os = "macos"))]
            None
        }

        fn start_slint_timer(&mut self) -> Self::Clock {
            let timer = slint::Timer::default();
            let started = Instant::now();
            let callback = Rc::clone(&self.callback);
            timer.start(
                slint::TimerMode::Repeated,
                Duration::from_nanos(16_666_667),
                move || {
                    let tick = platform_lifecycle::timer_tick(
                        started.elapsed(),
                        Duration::from_micros(16_667),
                    );
                    (callback.borrow_mut())(FramebufferDisplayClockTick {
                        timestamp_us: tick.timestamp_us,
                        target_timestamp_us: tick.target_timestamp_us,
                        duration_us: tick.duration_us,
                    });
                },
            );
            FramebufferDisplayClock::Timer(timer)
        }
    }

    let callback = Rc::new(RefCell::new(
        Box::new(callback) as Box<dyn FnMut(FramebufferDisplayClockTick)>
    ));
    let mut adapter = Adapter { window, callback };
    let controller = platform_lifecycle::DisplayClockController::start(&mut adapter);
    let _source = controller.source();
    controller.into_clock()
}

#[derive(Clone, Debug)]
struct DirtyRectOverlayState {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    kind: String,
}

struct LatestMailbox<T> {
    state: Mutex<LatestMailboxState<T>>,
}

struct LatestMailboxState<T> {
    pending: Option<T>,
    wake_outstanding: bool,
    closed: bool,
    published: u64,
    coalesced: u64,
}

impl<T> Default for LatestMailbox<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(LatestMailboxState {
                pending: None,
                wake_outstanding: false,
                closed: false,
                published: 0,
                coalesced: 0,
            }),
        }
    }
}

impl<T> LatestMailbox<T> {
    fn publish(&self, value: T) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.closed {
            return false;
        }
        state.published = state.published.saturating_add(1);
        if state.pending.replace(value).is_some() {
            state.coalesced = state.coalesced.saturating_add(1);
        }
        if state.wake_outstanding {
            false
        } else {
            state.wake_outstanding = true;
            true
        }
    }

    fn take(&self) -> Option<T> {
        self.state.lock().ok()?.pending.take()
    }

    fn complete_apply(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.closed {
            state.wake_outstanding = false;
            return false;
        }
        if state.pending.is_some() {
            true
        } else {
            state.wake_outstanding = false;
            false
        }
    }

    fn stats(&self) -> (u64, u64) {
        self.state
            .lock()
            .map(|state| (state.published, state.coalesced))
            .unwrap_or_default()
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.pending = None;
            state.wake_outstanding = false;
        }
    }
}

struct FramebufferDisplayUpdate {
    frame: agent_client::FramebufferStreamFrame,
    pixels: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    received_at: Instant,
}

struct FramebufferDisplayState {
    recent_dirty_rects: VecDeque<DirtyRectOverlayState>,
    geometry: Option<mister_magik_framebuffer_stream::FrameGeometry>,
    applied: u64,
    chrome_enabled: bool,
    last_received_at: Option<Instant>,
    last_source_sequence: u64,
    last_source_timestamp_us: u64,
    dirty_summary: String,
}

impl Default for FramebufferDisplayState {
    fn default() -> Self {
        Self::new(true)
    }
}

impl FramebufferDisplayState {
    fn new(chrome_enabled: bool) -> Self {
        Self {
            recent_dirty_rects: VecDeque::new(),
            geometry: None,
            applied: 0,
            chrome_enabled,
            last_received_at: None,
            last_source_sequence: 0,
            last_source_timestamp_us: 0,
            dirty_summary: String::new(),
        }
    }
}

#[allow(dead_code)] // Owns the active display-link and timer for Drop/lifetime behavior.
struct FramebufferDisplayController {
    clock: FramebufferDisplayClock,
    chrome_timer: Option<slint::Timer>,
}

type SharedFramebufferDisplayMailbox = Arc<LatestMailbox<FramebufferDisplayUpdate>>;
type SharedFramebufferDisplayState = Arc<Mutex<FramebufferDisplayState>>;

#[cfg_attr(not(feature = "compiled-ui"), allow(dead_code))]
struct FramebufferRenderMetrics {
    supported: AtomicBool,
    observer_ready: AtomicBool,
    winit_observer_ready: AtomicBool,
    rendering_notifier_ready: AtomicBool,
    focused: AtomicBool,
    occluded: AtomicBool,
    lost_focus_during_measurement: AtomicBool,
    occluded_during_measurement: AtomicBool,
    monitor_refresh_millihertz: AtomicU64,
    winit_events: AtomicU64,
    winit_redraws: AtomicU64,
    foreground_redraws: AtomicU64,
    rendering_setup: AtomicU64,
    rendering_before: AtomicU64,
    rendering_after: AtomicU64,
    rendering_teardown: AtomicU64,
    clock_kind: AtomicU8,
    display_ticks: AtomicU64,
    render_callbacks: AtomicU64,
    received: AtomicU64,
    applied: AtomicU64,
    rendered: AtomicU64,
    coalesced: AtomicU64,
    applied_serial: AtomicU64,
    cadence: Arc<FramebufferCadenceTrace>,
    state: Mutex<FramebufferRenderMetricsState>,
}

struct FramebufferRenderMetricsState {
    started: Instant,
    latest_applied: Option<(u64, Instant)>,
    last_rendered_serial: u64,
    rendered_at: VecDeque<Instant>,
    latencies: VecDeque<Duration>,
}

#[cfg_attr(not(feature = "compiled-ui"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Default)]
struct FramebufferRenderSnapshot {
    supported: bool,
    render_callbacks: u64,
    fps: f64,
    received_fps: f64,
    applied_fps: f64,
    rendered_fps: f64,
    received: u64,
    applied: u64,
    rendered: u64,
    coalesced: u64,
    latency_p95_ms: f64,
}

impl Default for FramebufferRenderMetrics {
    fn default() -> Self {
        Self {
            supported: AtomicBool::new(false),
            observer_ready: AtomicBool::new(false),
            winit_observer_ready: AtomicBool::new(false),
            rendering_notifier_ready: AtomicBool::new(false),
            focused: AtomicBool::new(false),
            occluded: AtomicBool::new(false),
            lost_focus_during_measurement: AtomicBool::new(false),
            occluded_during_measurement: AtomicBool::new(false),
            monitor_refresh_millihertz: AtomicU64::new(0),
            winit_events: AtomicU64::new(0),
            winit_redraws: AtomicU64::new(0),
            foreground_redraws: AtomicU64::new(0),
            rendering_setup: AtomicU64::new(0),
            rendering_before: AtomicU64::new(0),
            rendering_after: AtomicU64::new(0),
            rendering_teardown: AtomicU64::new(0),
            clock_kind: AtomicU8::new(FramebufferDisplayClockKind::Unknown as u8),
            display_ticks: AtomicU64::new(0),
            render_callbacks: AtomicU64::new(0),
            received: AtomicU64::new(0),
            applied: AtomicU64::new(0),
            rendered: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
            applied_serial: AtomicU64::new(0),
            cadence: Arc::new(FramebufferCadenceTrace::default()),
            state: Mutex::new(FramebufferRenderMetricsState {
                started: Instant::now(),
                latest_applied: None,
                last_rendered_serial: 0,
                rendered_at: VecDeque::new(),
                latencies: VecDeque::new(),
            }),
        }
    }
}

impl FramebufferRenderMetrics {
    fn reset(&self) {
        self.cadence.reset();
        self.winit_events.store(0, Ordering::Release);
        self.winit_redraws.store(0, Ordering::Release);
        self.foreground_redraws.store(0, Ordering::Release);
        self.lost_focus_during_measurement
            .store(false, Ordering::Release);
        self.occluded_during_measurement
            .store(false, Ordering::Release);
        self.rendering_setup.store(0, Ordering::Release);
        self.rendering_before.store(0, Ordering::Release);
        self.rendering_after.store(0, Ordering::Release);
        self.rendering_teardown.store(0, Ordering::Release);
        self.display_ticks.store(0, Ordering::Release);
        self.render_callbacks.store(0, Ordering::Release);
        self.applied_serial.store(0, Ordering::Release);
        self.received.store(0, Ordering::Release);
        self.applied.store(0, Ordering::Release);
        self.rendered.store(0, Ordering::Release);
        self.coalesced.store(0, Ordering::Release);
        if let Ok(mut state) = self.state.lock() {
            state.started = Instant::now();
            state.latest_applied = None;
            state.last_rendered_serial = 0;
            state.rendered_at.clear();
            state.latencies.clear();
        }
    }

    fn mark_applied(&self, received_at: Instant, source_sequence: u64, source_timestamp_us: u64) {
        self.applied.fetch_add(1, Ordering::Relaxed);
        let serial = self.applied_serial.fetch_add(1, Ordering::AcqRel) + 1;
        self.cadence.record(
            CadenceEventKind::UiApplied,
            source_sequence,
            source_timestamp_us,
            serial,
            received_at.elapsed().as_micros() as u64,
            0,
        );
        if let Ok(mut state) = self.state.lock() {
            state.latest_applied = Some((serial, received_at));
        }
    }

    fn mark_received(&self, coalesced: u64) {
        self.received.fetch_add(1, Ordering::Relaxed);
        self.coalesced.store(coalesced, Ordering::Relaxed);
    }

    fn mark_display_tick(
        &self,
        kind: FramebufferDisplayClockKind,
        tick: FramebufferDisplayClockTick,
    ) {
        self.clock_kind.store(kind as u8, Ordering::Release);
        self.display_ticks.fetch_add(1, Ordering::Relaxed);
        self.cadence.record(
            CadenceEventKind::DisplayLinkTick,
            0,
            tick.timestamp_us,
            self.applied_serial.load(Ordering::Acquire),
            tick.duration_us,
            i64::try_from(tick.target_timestamp_us).unwrap_or(i64::MAX),
        );
    }

    fn clock_kind(&self) -> FramebufferDisplayClockKind {
        FramebufferDisplayClockKind::from_u8(self.clock_kind.load(Ordering::Acquire))
    }

    fn set_clock_kind(&self, kind: FramebufferDisplayClockKind) {
        self.clock_kind.store(kind as u8, Ordering::Release);
    }

    fn mark_rendered(&self, now: Instant, kind: CadenceEventKind) {
        self.render_callbacks.fetch_add(1, Ordering::Relaxed);
        let serial = self.applied_serial.load(Ordering::Acquire);
        self.cadence.record(kind, 0, 0, serial, 0, 0);
        if serial == 0 {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.last_rendered_serial == serial {
            return;
        }
        state.last_rendered_serial = serial;
        self.rendered.fetch_add(1, Ordering::Relaxed);
        state.rendered_at.push_back(now);
        if let Some((applied_serial, applied_at)) = state.latest_applied {
            if applied_serial == serial {
                state
                    .latencies
                    .push_back(now.saturating_duration_since(applied_at));
            }
        }
        prune_render_metrics(&mut state, now);
    }

    #[cfg_attr(not(feature = "compiled-ui"), allow(dead_code))]
    fn benchmark_ready(&self) -> bool {
        self.observer_ready.load(Ordering::Acquire)
            && self.formal_display_clock_ready()
            && self.received.load(Ordering::Acquire) > 0
            && self.applied.load(Ordering::Acquire) > 0
            && self.rendered.load(Ordering::Acquire) > 0
            && self.focused.load(Ordering::Acquire)
            && !self.occluded.load(Ordering::Acquire)
    }

    #[cfg_attr(not(feature = "compiled-ui"), allow(dead_code))]
    fn benchmark_invalid_reason(&self) -> Option<&'static str> {
        if let Some(reason) = self.benchmark_readiness_reason() {
            return Some(reason);
        }
        if self.lost_focus_during_measurement.load(Ordering::Acquire) {
            Some("window_lost_focus")
        } else if self.occluded_during_measurement.load(Ordering::Acquire) {
            Some("window_was_occluded")
        } else {
            None
        }
    }

    fn benchmark_readiness_reason(&self) -> Option<&'static str> {
        if !self.observer_ready.load(Ordering::Acquire) {
            Some("rendering_notifier_not_ready")
        } else if !self.formal_display_clock_ready() {
            Some("macos_display_link_not_ready")
        } else if self.received.load(Ordering::Acquire) == 0 {
            Some("no_stream_frames")
        } else if self.applied.load(Ordering::Acquire) == 0 {
            Some("no_applied_frames")
        } else if !self.focused.load(Ordering::Acquire) {
            Some("window_unfocused")
        } else if self.occluded.load(Ordering::Acquire) {
            Some("window_occluded")
        } else if self.rendered.load(Ordering::Acquire) == 0 {
            Some("zero_after_rendering")
        } else {
            None
        }
    }

    fn formal_display_clock_ready(&self) -> bool {
        if cfg!(target_os = "macos") {
            self.clock_kind() == FramebufferDisplayClockKind::MacosDisplayLink
        } else {
            self.clock_kind() != FramebufferDisplayClockKind::Unknown
        }
    }

    fn snapshot(&self, now: Instant) -> FramebufferRenderSnapshot {
        let Ok(mut state) = self.state.lock() else {
            return FramebufferRenderSnapshot::default();
        };
        prune_render_metrics(&mut state, now);
        let elapsed = now
            .saturating_duration_since(state.started)
            .min(Duration::from_secs(2))
            .as_secs_f64()
            .max(f64::EPSILON);
        let total_elapsed = now
            .saturating_duration_since(state.started)
            .as_secs_f64()
            .max(f64::EPSILON);
        let received = self.received.load(Ordering::Relaxed);
        let applied = self.applied.load(Ordering::Relaxed);
        let rendered = self.rendered.load(Ordering::Relaxed);
        let mut latencies = state.latencies.iter().copied().collect::<Vec<_>>();
        latencies.sort();
        FramebufferRenderSnapshot {
            supported: self.supported.load(Ordering::Acquire),
            render_callbacks: self.render_callbacks.load(Ordering::Relaxed),
            fps: state.rendered_at.len() as f64 / elapsed,
            received_fps: received as f64 / total_elapsed,
            applied_fps: applied as f64 / total_elapsed,
            rendered_fps: rendered as f64 / total_elapsed,
            received,
            applied,
            rendered,
            coalesced: self.coalesced.load(Ordering::Relaxed),
            latency_p95_ms: latency_percentile_ms(&latencies, 0.95),
        }
    }
}

fn prune_render_metrics(state: &mut FramebufferRenderMetricsState, now: Instant) {
    let cutoff = now.checked_sub(Duration::from_secs(2)).unwrap_or(now);
    while state
        .rendered_at
        .front()
        .is_some_and(|value| *value < cutoff)
    {
        state.rendered_at.pop_front();
        state.latencies.pop_front();
    }
}

fn install_framebuffer_render_notifier(
    window: &slint::Window,
    metrics: Arc<FramebufferRenderMetrics>,
) {
    use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

    let redraw_metrics = Arc::clone(&metrics);
    window.on_winit_window_event(move |_window, event| {
        redraw_metrics.winit_events.fetch_add(1, Ordering::Relaxed);
        match event {
            winit::event::WindowEvent::RedrawRequested => {
                redraw_metrics.winit_redraws.fetch_add(1, Ordering::Relaxed);
                if redraw_metrics.focused.load(Ordering::Acquire)
                    && !redraw_metrics.occluded.load(Ordering::Acquire)
                {
                    redraw_metrics
                        .foreground_redraws
                        .fetch_add(1, Ordering::Relaxed);
                }
                redraw_metrics.cadence.record(
                    CadenceEventKind::RedrawRequested,
                    0,
                    0,
                    redraw_metrics.applied_serial.load(Ordering::Acquire),
                    0,
                    0,
                );
            }
            winit::event::WindowEvent::Focused(focused) => {
                redraw_metrics.focused.store(*focused, Ordering::Release);
                if !*focused {
                    redraw_metrics
                        .lost_focus_during_measurement
                        .store(true, Ordering::Release);
                }
                redraw_metrics.cadence.record(
                    CadenceEventKind::WindowFocused,
                    0,
                    0,
                    0,
                    0,
                    i64::from(*focused),
                );
            }
            winit::event::WindowEvent::Occluded(occluded) => {
                redraw_metrics.occluded.store(*occluded, Ordering::Release);
                if *occluded {
                    redraw_metrics
                        .occluded_during_measurement
                        .store(true, Ordering::Release);
                }
                redraw_metrics.cadence.record(
                    CadenceEventKind::WindowOccluded,
                    0,
                    0,
                    0,
                    0,
                    i64::from(*occluded),
                );
            }
            _ => {}
        }
        EventResult::Propagate
    });
    metrics.winit_observer_ready.store(true, Ordering::Release);

    let callback_metrics = Arc::clone(&metrics);
    let rendering_notifier_ready = window
        .set_rendering_notifier(move |state, _graphics_api| match state {
            slint::RenderingState::RenderingSetup => {
                callback_metrics
                    .rendering_setup
                    .fetch_add(1, Ordering::Relaxed);
                // Treat notifier observation as ready only after Slint has
                // entered the renderer lifecycle. A successful registration
                // alone does not prove that this window will deliver frames.
                callback_metrics
                    .observer_ready
                    .store(true, Ordering::Release);
            }
            slint::RenderingState::BeforeRendering => {
                callback_metrics
                    .rendering_before
                    .fetch_add(1, Ordering::Relaxed);
                callback_metrics.cadence.record(
                    CadenceEventKind::BeforeRendering,
                    0,
                    0,
                    callback_metrics.applied_serial.load(Ordering::Acquire),
                    0,
                    0,
                );
            }
            slint::RenderingState::AfterRendering => {
                callback_metrics
                    .rendering_after
                    .fetch_add(1, Ordering::Relaxed);
                callback_metrics.mark_rendered(Instant::now(), CadenceEventKind::AfterRendering)
            }
            slint::RenderingState::RenderingTeardown => {
                callback_metrics
                    .rendering_teardown
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        })
        .is_ok();
    metrics
        .rendering_notifier_ready
        .store(rendering_notifier_ready, Ordering::Release);
    metrics
        .supported
        .store(rendering_notifier_ready, Ordering::Release);
}

#[derive(Clone, Debug, Default)]
struct RealtimeHistory {
    samples: VecDeque<DeviceTelemetrySample>,
}

impl RealtimeHistory {
    fn push(&mut self, sample: DeviceTelemetrySample) {
        self.samples.push_back(sample);
        while self.samples.len() > REALTIME_HISTORY_CAPACITY {
            self.samples.pop_front();
        }
    }
}

#[derive(Clone, Debug)]
struct RealtimeViewState {
    status: String,
    last_error: String,
    fps_summary: String,
    cpu_summary: String,
    memory_total_label: String,
    memory_magik_label: String,
    memory_other_label: String,
    memory_available_label: String,
    frame_summary: String,
    storage_total_label: String,
    storage_used_label: String,
    storage_empty_label: String,
    storage_activity_summary: String,
    frame_hover: String,
    streaming: bool,
    combined_cpu_pct: f64,
    magik_memory_pct: f64,
    other_memory_pct: f64,
    available_memory_pct: f64,
    storage_used_pct: f64,
    frame_budget_pct: f64,
    ui_thread_cpu: Option<u64>,
    cores: Vec<agent_client::CpuCoreTelemetry>,
    cpu_history: Vec<RealtimeChartPoint>,
    cpu0_path: String,
    cpu1_path: String,
    storage_read_path: String,
    storage_write_path: String,
    frame_history: Vec<RealtimeChartPoint>,
    phases: Vec<RealtimeFramePhaseView>,
    frame_samples: Vec<FrameSample>,
    health_tiles: Vec<RealtimeHealthTileView>,
}

#[derive(Clone, Debug)]
struct RealtimeChartPoint {
    value: f64,
    alert: bool,
}

#[derive(Clone, Debug)]
struct RealtimeFramePhaseView {
    label: String,
    us: u64,
    start_us: u64,
    color_index: i32,
}

#[derive(Clone, Debug)]
struct RealtimeHealthTileView {
    title: String,
    value: String,
    detail: String,
    state: String,
}

fn scanout_health_tile(history: &RealtimeHistory) -> RealtimeHealthTileView {
    let unavailable = |value: &str, detail: String, state: &str| RealtimeHealthTileView {
        title: "Scanout".to_string(),
        value: value.to_string(),
        detail,
        state: state.to_string(),
    };
    let Some(latest) = history.samples.back().map(|sample| &sample.presentation) else {
        return unavailable("Unavailable", "No FPGA cadence sample.".to_string(), "warn");
    };
    if !latest.available {
        return unavailable(
            "Unavailable",
            if latest.error.is_empty() {
                "FPGA cadence telemetry is unavailable.".to_string()
            } else {
                latest.error.clone()
            },
            "warn",
        );
    }
    if !latest.magik_ownership {
        return unavailable("Ownership lost", "MagiK does not own scanout.".to_string(), "bad");
    }
    if latest.pending {
        return unavailable("Pending", "FPGA endpoint is not settled.".to_string(), "warn");
    }
    if !latest.lifetime_invariant_valid {
        return unavailable("Invalid", "FPGA cadence invariant failed.".to_string(), "bad");
    }
    let previous = history
        .samples
        .iter()
        .rev()
        .skip(1)
        .map(|sample| &sample.presentation)
        .find(|sample| {
            sample.available
                && sample.magik_ownership
                && !sample.pending
                && sample.lifetime_invariant_valid
        });
    let Some(previous) = previous else {
        return unavailable("Settling", "Waiting for a second FPGA sample.".to_string(), "warn");
    };
    let Some((owned, presented, repeated, losses)) = latest
        .owned_vblank_count
        .zip(previous.owned_vblank_count)
        .map(|(end, start)| end.wrapping_sub(start))
        .zip(
            latest
                .presented_vblank_count
                .zip(previous.presented_vblank_count)
                .map(|(end, start)| end.wrapping_sub(start)),
        )
        .zip(
            latest
                .repeated_vblank_count
                .zip(previous.repeated_vblank_count)
                .map(|(end, start)| end.wrapping_sub(start)),
        )
        .zip(
            latest
                .ownership_loss_count
                .zip(previous.ownership_loss_count)
                .map(|(end, start)| end.wrapping_sub(start)),
        )
        .map(|(((owned, presented), repeated), losses)| (owned, presented, repeated, losses))
    else {
        return unavailable("Unavailable", "FPGA counters are incomplete.".to_string(), "warn");
    };
    if owned != presented.wrapping_add(repeated) {
        return unavailable("Invalid", "FPGA cadence delta invariant failed.".to_string(), "bad");
    }
    if losses > 0 {
        return unavailable(
            "Ownership lost",
            format!("{losses} ownership transition(s) in the latest window."),
            "bad",
        );
    }
    if repeated > 0 {
        unavailable(
            &format!("{repeated} repeated"),
            format!("{presented} new frames across {owned} owned vblanks."),
            "bad",
        )
    } else {
        unavailable(
            "0 repeated",
            format!("{presented} new frames; ownership remained settled."),
            "good",
        )
    }
}

#[cfg(feature = "compiled-ui")]
slint::include_modules!();

#[derive(Clone, Debug)]
struct LibraryBrowser {
    catalog: Option<library::LibraryCatalog>,
    query: library::LibraryQuery,
    selected_game_id: String,
    status: String,
    warning: String,
    last_error: String,
    loading: bool,
}

impl LibraryBrowser {
    fn new() -> Self {
        Self {
            catalog: None,
            query: library::LibraryQuery::default(),
            selected_game_id: String::new(),
            status: "Sync the MiSTer library database to browse games.".to_string(),
            warning: String::new(),
            last_error: String::new(),
            loading: false,
        }
    }

    fn start_sync(&mut self) {
        self.loading = true;
        self.status = "Copying the MagiK library database from the MiSTer...".to_string();
        self.warning.clear();
        self.last_error.clear();
    }

    fn apply_sync_result(&mut self, result: Result<library::LibrarySyncResult, String>) {
        self.loading = false;
        match result {
            Ok(result) => {
                self.catalog = Some(result.catalog);
                self.query.page = 1;
                self.selected_game_id = self
                    .current_view()
                    .and_then(|view| view.rows.first().map(|game| game.id.clone()))
                    .unwrap_or_default();
                self.status = result.status;
                self.warning = result.warning;
                self.last_error.clear();
            }
            Err(err) => {
                self.last_error = err;
                self.status = "Library sync failed.".to_string();
                self.warning.clear();
            }
        }
    }

    fn set_sort(&mut self, column_id: &str, direction_id: &str) {
        let Some(column) = library_sort_column(column_id) else {
            return;
        };
        let direction = library_sort_direction(direction_id);
        self.query.sort_column = column;
        self.query.sort_direction = direction;
        self.query.page = 1;
        self.normalize_selection();
    }

    fn set_query(&mut self, search: &str) {
        self.query.search = search.to_string();
        self.query.page = 1;
        self.normalize_selection();
    }

    fn set_filter(&mut self, filter: &str, value: &str) {
        match filter {
            "system" => self.query.system = value.to_string(),
            "category" => self.query.category = value.to_string(),
            "region" => self.query.region = value.to_string(),
            "manufacturer" => self.query.manufacturer = value.to_string(),
            "preview" => self.query.preview = value.to_string(),
            "confidence" => self.query.confidence = value.to_string(),
            _ => return,
        }
        self.query.page = 1;
        self.normalize_selection();
    }

    fn set_page(&mut self, page: i32) {
        self.query.page = usize::try_from(page).unwrap_or(1).max(1);
        self.normalize_selection();
    }

    fn select_row(&mut self, id: &str) {
        if let Some(catalog) = &self.catalog {
            if library::selected_game(catalog, id).is_some() {
                self.selected_game_id = id.to_string();
            }
        }
    }

    fn selected_game(&self) -> Option<&library::LibraryGame> {
        self.catalog
            .as_ref()
            .and_then(|catalog| library::selected_game(catalog, &self.selected_game_id))
    }

    fn current_view(&self) -> Option<library::LibraryView> {
        self.catalog
            .as_ref()
            .map(|catalog| library::apply_library_query(catalog, &self.query))
    }

    fn normalize_selection(&mut self) {
        let Some(view) = self.current_view() else {
            self.selected_game_id.clear();
            return;
        };
        if view
            .rows
            .iter()
            .any(|game| game.id == self.selected_game_id)
        {
            return;
        }
        self.selected_game_id = view
            .rows
            .first()
            .map(|game| game.id.clone())
            .unwrap_or_default();
    }

    fn result_summary(&self) -> String {
        match (&self.catalog, self.current_view()) {
            (Some(catalog), Some(view)) => {
                format!(
                    "{} of {} games in the local library snapshot.",
                    view.total_count,
                    catalog.games.len()
                )
            }
            _ => "No library loaded.".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
struct LibrarySelectOptionItem {
    value: String,
    label: String,
    enabled: bool,
}

#[derive(Clone, Debug, Default)]
struct LibraryDetailSections {
    overview: Vec<LibraryDetailRow>,
    system: Vec<LibraryDetailRow>,
    media: Vec<LibraryDetailRow>,
    launch: Vec<LibraryDetailRow>,
    identity: Vec<LibraryDetailRow>,
    paths: Vec<LibraryDetailRow>,
}

#[derive(Clone, Debug)]
struct LibraryDetailRow {
    field: String,
    value: String,
}

fn library_sort_column(column_id: &str) -> Option<library::LibrarySortColumn> {
    match column_id {
        "title" => Some(library::LibrarySortColumn::Title),
        "system" => Some(library::LibrarySortColumn::System),
        "year" => Some(library::LibrarySortColumn::Year),
        "manufacturer" => Some(library::LibrarySortColumn::Manufacturer),
        "category" => Some(library::LibrarySortColumn::Category),
        "preview" => Some(library::LibrarySortColumn::Preview),
        "discovered" => Some(library::LibrarySortColumn::Discovered),
        _ => None,
    }
}

fn library_select_options(all_label: &str, values: &[String]) -> Vec<LibrarySelectOptionItem> {
    let mut options = Vec::with_capacity(values.len() + 1);
    options.push(LibrarySelectOptionItem {
        value: String::new(),
        label: all_label.to_string(),
        enabled: true,
    });
    options.extend(values.iter().map(|value| LibrarySelectOptionItem {
        value: value.clone(),
        label: value.clone(),
        enabled: true,
    }));
    options
}

fn library_preview_options() -> Vec<LibrarySelectOptionItem> {
    [
        ("", "All previews"),
        ("with-preview", "With preview"),
        ("missing-preview", "Missing preview"),
    ]
    .into_iter()
    .map(|(value, label)| LibrarySelectOptionItem {
        value: value.to_string(),
        label: label.to_string(),
        enabled: true,
    })
    .collect()
}

fn library_option_index(options: &[LibrarySelectOptionItem], value: &str) -> i32 {
    options
        .iter()
        .position(|option| option.value == value)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0)
}

fn library_discovered_label(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn library_preview_label(has_preview: bool) -> &'static str {
    if has_preview { "Preview" } else { "Missing" }
}

fn realtime_view_from_history(
    history: &RealtimeHistory,
    streaming: bool,
    last_error: &str,
) -> RealtimeViewState {
    let latest = history.samples.back();
    let cpu_history = history
        .samples
        .iter()
        .map(|sample| RealtimeChartPoint {
            value: sample.combined_cpu_pct.clamp(0.0, 100.0),
            alert: sample.combined_cpu_pct >= 85.0,
        })
        .collect::<Vec<_>>();
    let cpu0_history = realtime_core_history(history, 0);
    let cpu1_history = realtime_core_history(history, 1);
    let cpu0_path = realtime_chart_path(&cpu0_history, REALTIME_HISTORY_CAPACITY);
    let cpu1_path = realtime_chart_path(&cpu1_history, REALTIME_HISTORY_CAPACITY);
    let storage_read_history = history
        .samples
        .iter()
        .filter(|sample| sample.storage.activity_valid)
        .map(|sample| sample.storage.read_pct)
        .collect::<Vec<_>>();
    let storage_write_history = history
        .samples
        .iter()
        .filter(|sample| sample.storage.activity_valid)
        .map(|sample| sample.storage.write_pct)
        .collect::<Vec<_>>();
    let storage_read_path = realtime_chart_path(&storage_read_history, REALTIME_HISTORY_CAPACITY);
    let storage_write_path = realtime_chart_path(&storage_write_history, REALTIME_HISTORY_CAPACITY);
    let frame_history = history
        .samples
        .iter()
        .map(|sample| {
            let budget = sample.frame_budget.budget_us.max(1) as f64;
            let value = sample.frame_budget.window_max_wall_us as f64 * 100.0 / budget;
            RealtimeChartPoint {
                value: value.clamp(0.0, 100.0),
                alert: sample.frame_budget.window_over_budget > 0,
            }
        })
        .collect::<Vec<_>>();
    let mut seen_frame_samples = HashSet::new();
    let mut frame_samples = Vec::new();
    for sample in &history.samples {
        let launcher_pid = sample.magik.pids.first().copied().unwrap_or(0);
        for frame in realtime_frame_samples_from_telemetry(sample) {
            if frame.idle || seen_frame_samples.insert((launcher_pid, frame.frame)) {
                frame_samples.push(frame);
            }
        }
    }
    if frame_samples.len() > REALTIME_FRAME_SAMPLE_CAPACITY {
        frame_samples =
            frame_samples.split_off(frame_samples.len() - REALTIME_FRAME_SAMPLE_CAPACITY);
    }

    let Some(sample) = latest else {
        return RealtimeViewState {
            status: if streaming {
                "Real Time stream starting...".to_string()
            } else if !last_error.is_empty() {
                "Real Time stream unavailable.".to_string()
            } else {
                "Real Time stream off.".to_string()
            },
            last_error: last_error.to_string(),
            fps_summary: "-".to_string(),
            cpu_summary: "-".to_string(),
            memory_total_label: "-".to_string(),
            memory_magik_label: "MagiK: 0 KiB".to_string(),
            memory_other_label: "Other: 0 KiB".to_string(),
            memory_available_label: "Available: 0 KiB".to_string(),
            frame_summary: "-".to_string(),
            storage_total_label: "-".to_string(),
            storage_used_label: "Used: 0GB".to_string(),
            storage_empty_label: "Free: 0GB".to_string(),
            storage_activity_summary: "-".to_string(),
            frame_hover: String::new(),
            streaming,
            combined_cpu_pct: 0.0,
            magik_memory_pct: 0.0,
            other_memory_pct: 0.0,
            available_memory_pct: 0.0,
            storage_used_pct: 0.0,
            frame_budget_pct: 0.0,
            ui_thread_cpu: None,
            cores: Vec::new(),
            cpu_history,
            cpu0_path,
            cpu1_path,
            storage_read_path,
            storage_write_path,
            frame_history,
            phases: Vec::new(),
            frame_samples,
            health_tiles: Vec::new(),
        };
    };

    let frame_budget = sample.frame_budget.budget_us.max(1);
    let frame_budget_pct = (sample.frame_budget.window_max_wall_us as f64 * 100.0
        / frame_budget as f64)
        .clamp(0.0, 100.0);
    let phases = realtime_frame_phases(&sample.frame_budget);
    let memory_total_label = format_kib(sample.memory.total_kb);
    let memory_magik_label = format!("MagiK: {}", format_kib(sample.memory.magik_kb));
    let memory_other_label = format!("Other: {}", format_kib(sample.memory.other_used_kb));
    let memory_available_label = format!("Available: {}", format_kib(sample.memory.available_kb));
    let storage_empty_pct = sample.storage.available_pct.clamp(0.0, 100.0);
    let storage_used_pct = (100.0 - storage_empty_pct).clamp(0.0, 100.0);
    let storage_used_bytes = sample
        .storage
        .total_bytes
        .saturating_sub(sample.storage.available_bytes);
    let storage_total_label = format_storage_gb(sample.storage.total_bytes);
    let storage_used_label = format!("Used: {}", format_storage_gb(storage_used_bytes));
    let storage_empty_label = format!(
        "Free: {}",
        format_storage_gb(sample.storage.available_bytes)
    );
    let storage_activity_summary = if sample.storage.activity_valid {
        storage_activity_summary(&sample.storage)
    } else {
        "Waiting for SD activity samples.".to_string()
    };
    let frame_summary = format!(
        "{} frames, {} over budget, max {}",
        sample.frame_budget.window_frames,
        sample.frame_budget.window_over_budget,
        format_us(sample.frame_budget.window_max_wall_us)
    );
    let health_tiles = vec![
        RealtimeHealthTileView {
            title: "MagiK".to_string(),
            value: process_tile_value(&sample.magik),
            detail: format!(
                "{} RSS, {} threads",
                format_kib(sample.magik.rss_kb),
                sample.magik.threads
            ),
            state: if sample.magik.pids.is_empty() {
                "bad"
            } else {
                "good"
            }
            .to_string(),
        },
        RealtimeHealthTileView {
            title: "Main".to_string(),
            value: process_tile_value(&sample.main),
            detail: format!(
                "{} RSS, {} threads",
                format_kib(sample.main.rss_kb),
                sample.main.threads
            ),
            state: if sample.main.pids.is_empty() {
                "warn"
            } else {
                "good"
            }
            .to_string(),
        },
        scanout_health_tile(history),
        RealtimeHealthTileView {
            title: "Network".to_string(),
            value: format!(
                "{} down / {} up",
                format_byte_rate(sample.network.rx_bytes_per_sec),
                format_byte_rate(sample.network.tx_bytes_per_sec)
            ),
            detail: "eth0 agent link".to_string(),
            state: "good".to_string(),
        },
    ];

    RealtimeViewState {
        status: if streaming {
            "Streaming lightweight telemetry at 1 Hz.".to_string()
        } else {
            "Real Time stream off.".to_string()
        },
        last_error: last_error.to_string(),
        fps_summary: if sample.launcher.idle {
            "60fps idle".to_string()
        } else {
            sample.launcher.fps.clone()
        },
        cpu_summary: format!("Combined {:.1}%", sample.combined_cpu_pct),
        memory_total_label,
        memory_magik_label,
        memory_other_label,
        memory_available_label,
        frame_summary,
        storage_total_label,
        storage_used_label,
        storage_empty_label,
        storage_activity_summary,
        frame_hover: String::new(),
        streaming,
        combined_cpu_pct: sample.combined_cpu_pct,
        magik_memory_pct: sample.memory.magik_pct,
        other_memory_pct: sample.memory.other_used_pct,
        available_memory_pct: sample.memory.available_pct,
        storage_used_pct,
        frame_budget_pct,
        ui_thread_cpu: sample.launcher.ui_thread_cpu,
        cores: sample.cores.clone(),
        cpu_history,
        cpu0_path,
        cpu1_path,
        storage_read_path,
        storage_write_path,
        frame_history,
        phases,
        frame_samples,
        health_tiles,
    }
}

fn realtime_frame_samples_from_telemetry(
    sample: &DeviceTelemetrySample,
) -> Vec<FrameSample> {
    let budget_us = sample.frame_budget.budget_us.max(1);
    let frames = sample
        .frame_budget
        .recent_frames
        .iter()
        .map(|frame| FrameSample {
            frame: frame.frame,
            wall_us: frame.wall_us,
            prepare_us: frame.prepare_us,
            render_us: frame.render_us,
            custom_draw_us: frame.custom_draw_us,
            vsync_us: frame.vsync_us,
            present_us: frame.present_us,
            cpu_prepare_us: frame.cpu_prepare_us,
            cpu_render_us: frame.cpu_render_us,
            cpu_custom_draw_us: frame.cpu_custom_draw_us,
            cpu_vsync_us: frame.cpu_vsync_us,
            cpu_present_us: frame.cpu_present_us,
            process_cpu_us: frame.process_cpu_us,
            over_budget: frame.wall_us > budget_us,
            idle: false,
        })
        .collect::<Vec<_>>();
    if !frames.is_empty() || !sample.launcher.idle {
        return frames;
    }

    (0..REALTIME_IDLE_FRAME_COLUMNS_PER_SAMPLE)
        .map(|ix| FrameSample {
            frame: sample
                .seq
                .saturating_mul(REALTIME_IDLE_FRAME_COLUMNS_PER_SAMPLE)
                .saturating_add(ix),
            wall_us: 0,
            prepare_us: 0,
            render_us: 0,
            custom_draw_us: 0,
            vsync_us: 0,
            present_us: 0,
            cpu_prepare_us: 0,
            cpu_render_us: 0,
            cpu_custom_draw_us: 0,
            cpu_vsync_us: 0,
            cpu_present_us: 0,
            process_cpu_us: 0,
            over_budget: false,
            idle: true,
        })
        .collect()
}

fn realtime_core_history(history: &RealtimeHistory, core_index: usize) -> Vec<f64> {
    history
        .samples
        .iter()
        .filter_map(|sample| sample.cores.get(core_index).map(|core| core.busy_pct))
        .collect()
}

fn storage_activity_summary(storage: &agent_client::StorageTelemetry) -> String {
    format!(
        "Read {:.1}% ({}) / Write {:.1}% ({})",
        storage.read_pct.clamp(0.0, 100.0),
        format_decimal_byte_rate(storage.read_bytes_per_sec),
        storage.write_pct.clamp(0.0, 100.0),
        format_decimal_byte_rate(storage.write_bytes_per_sec),
    )
}

fn realtime_chart_path(values: &[f64], capacity: usize) -> String {
    if values.is_empty() || capacity == 0 {
        return String::new();
    }

    let step = if capacity > 1 {
        100.0 / (capacity - 1) as f64
    } else {
        0.0
    };
    let start_index = capacity.saturating_sub(values.len());
    let mut path = String::with_capacity(values.len() * 12);
    for (ix, value) in values.iter().enumerate() {
        let x = ((start_index + ix) as f64 * step).clamp(0.0, 100.0);
        let y = 100.0 - value.clamp(0.0, 100.0);
        if ix == 0 {
            path.push('M');
        } else {
            path.push('L');
        }
        path.push(' ');
        push_path_number(&mut path, x);
        path.push(' ');
        push_path_number(&mut path, y);
        path.push(' ');
    }
    path.trim_end().to_string()
}

fn push_path_number(path: &mut String, value: f64) {
    use std::fmt::Write;
    let _ = write!(path, "{value:.2}");
}

fn realtime_frame_phases(
    frame: &agent_client::FrameBudgetTelemetry,
) -> Vec<RealtimeFramePhaseView> {
    let phases = [
        ("Prepare", frame.window_prepare_us, 0),
        ("Render", frame.window_render_us, 1),
        ("Custom", frame.window_custom_draw_us, 2),
        ("Vsync", frame.window_vsync_us, 3),
        ("Present", frame.window_present_us, 4),
    ];
    let mut start = 0_u64;
    phases
        .into_iter()
        .map(|(label, us, color_index)| {
            let item = RealtimeFramePhaseView {
                label: label.to_string(),
                us,
                start_us: start,
                color_index,
            };
            start = start.saturating_add(us);
            item
        })
        .collect()
}

fn process_tile_value(process: &agent_client::ProcessTelemetry) -> String {
    if process.pids.is_empty() {
        "not running".to_string()
    } else {
        format!(
            "pid {}",
            process
                .pids
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

fn format_kib(kb: u64) -> String {
    if kb >= 1024 * 1024 {
        format!("{:.1} GiB", kb as f64 / 1024.0 / 1024.0)
    } else if kb >= 1024 {
        format!("{:.1} MiB", kb as f64 / 1024.0)
    } else {
        format!("{kb} KiB")
    }
}

fn format_us(us: u64) -> String {
    if us >= 1000 {
        format!("{:.1}ms", us as f64 / 1000.0)
    } else {
        format!("{us}us")
    }
}

fn format_byte_rate(bytes: u64) -> String {
    format!("{}/s", format_byte_size(bytes))
}

fn format_decimal_byte_rate(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB/s", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB/s", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} B/s")
    }
}

fn format_storage_gb(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    format!("{:.0}GB", bytes as f64 / GB)
}

fn library_detail_sections(game: Option<&library::LibraryGame>) -> LibraryDetailSections {
    let Some(game) = game else {
        return LibraryDetailSections::default();
    };
    let mut sections = LibraryDetailSections::default();
    push_detail(&mut sections.overview, "Title", &game.title);
    push_detail(&mut sections.overview, "Category", &game.category);
    push_detail(&mut sections.overview, "Manufacturer", &game.manufacturer);
    push_detail(&mut sections.overview, "Year", &game.year);
    push_detail(&mut sections.overview, "Region", &game.region);
    push_detail(
        &mut sections.overview,
        "Region confidence",
        &game.region_confidence,
    );
    push_detail(&mut sections.overview, "Confidence", &game.confidence);
    push_detail(
        &mut sections.overview,
        "Discovered",
        &library_discovered_label(&game.discovered_at_unix),
    );

    push_detail(&mut sections.system, "System", &game.system_title);
    push_detail(&mut sections.system, "System ID", &game.system_id);
    push_detail(&mut sections.system, "Core ID", &game.core_id);
    push_detail(&mut sections.system, "Hardware ID", &game.hardware_id);

    push_detail(
        &mut sections.media,
        "Preview",
        if game.has_preview {
            "Available"
        } else {
            "Missing"
        },
    );
    push_detail(&mut sections.media, "Preview key", &game.preview_asset_key);

    push_detail(&mut sections.launch, "Launch kind", &game.launch_kind);
    push_detail(&mut sections.launch, "Launch ref", &game.launch_ref);
    push_detail(
        &mut sections.launch,
        "Launch ID",
        &game.launch_id.to_string(),
    );

    push_detail(&mut sections.identity, "Setname", &game.setname);
    push_detail(&mut sections.identity, "Parent", &game.parent);
    for (index, identity) in game.identities.iter().enumerate() {
        let mut parts = Vec::new();
        push_part(&mut parts, &identity.identity_id);
        push_part(&mut parts, &identity.metadata_title);
        push_part(&mut parts, &identity.family_id);
        push_part(&mut parts, &identity.year);
        push_part(&mut parts, &identity.manufacturer);
        push_part(&mut parts, &identity.category);
        push_part(&mut parts, &identity.source);
        if !parts.is_empty() {
            let label = if identity.namespace.is_empty() {
                format!("Identity {}", index + 1)
            } else {
                identity.namespace.clone()
            };
            push_detail(&mut sections.identity, &label, &parts.join(" / "));
        }
    }

    push_detail(&mut sections.paths, "Source path", &game.source_path);
    push_detail(&mut sections.paths, "Payload path", &game.payload_path);
    sections
}

fn push_detail(rows: &mut Vec<LibraryDetailRow>, field: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() || value == "-" {
        return;
    }
    rows.push(LibraryDetailRow {
        field: field.to_string(),
        value: value.to_string(),
    });
}

fn push_part(parts: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        parts.push(value.to_string());
    }
}

fn library_sort_direction(direction_id: &str) -> library::LibrarySortDirection {
    match direction_id {
        "descending" | "Descending" => library::LibrarySortDirection::Descending,
        _ => library::LibrarySortDirection::Ascending,
    }
}

fn library_sort_column_id(column: library::LibrarySortColumn) -> &'static str {
    match column {
        library::LibrarySortColumn::Title => "title",
        library::LibrarySortColumn::System => "system",
        library::LibrarySortColumn::Year => "year",
        library::LibrarySortColumn::Manufacturer => "manufacturer",
        library::LibrarySortColumn::Category => "category",
        library::LibrarySortColumn::Preview => "preview",
        library::LibrarySortColumn::Discovered => "discovered",
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    if let Some(args) = framebuffer_display_bench_args()? {
        if cfg!(debug_assertions) {
            return Err("desktop display benchmarks require --release".into());
        }
        if !cfg!(feature = "compiled-ui") || !cfg!(feature = "skia-renderer") {
            return Err(
                "desktop display benchmarks require --no-default-features --features compiled-ui,skia-renderer"
                    .into(),
            );
        }
        if let Some(backend) = std::env::var_os("SLINT_BACKEND") {
            if backend != "winit-skia" {
                return Err(format!(
                    "desktop display benchmarks require SLINT_BACKEND=winit-skia, got {}",
                    backend.to_string_lossy()
                )
                .into());
            }
        }
        // Formal display results have one renderer identity. Do not allow an
        // ambient desktop backend to turn the benchmark into another test.
        // SAFETY: main has not initialized Slint or spawned application
        // threads; this selects the backend before either can happen.
        unsafe { std::env::set_var("SLINT_BACKEND", "winit-skia") };
        select_backend()?;
        #[cfg(feature = "compiled-ui")]
        return match args.source {
            FramebufferDisplayBenchSource::RealStream => {
                run_compiled_framebuffer_display_bench(args)
            }
            FramebufferDisplayBenchSource::Synthetic => run_compiled_synthetic_display_bench(args),
        };
        #[cfg(not(feature = "compiled-ui"))]
        {
            let _ = args;
            return Err("desktop display benchmarks require compiled-ui".into());
        }
    }
    if let Some((mode, limit)) = framebuffer_stream_bench_args()? {
        run_framebuffer_stream_bench(mode, limit)?;
        return Ok(());
    }
    let frame_chart_fixture = realtime_frame_chart_fixture_args()?;

    if std::env::var_os("SLINT_BACKEND").is_none() {
        // SAFETY: this is still single-threaded process initialization before
        // select_backend creates any UI or worker state.
        unsafe { std::env::set_var("SLINT_BACKEND", default_slint_backend()) };
    }
    select_backend()?;

    #[cfg(feature = "live-ui")]
    {
        run_live_ui(frame_chart_fixture)
    }

    #[cfg(all(not(feature = "live-ui"), feature = "compiled-ui"))]
    {
        run_compiled_ui(frame_chart_fixture)
    }

    #[cfg(all(not(feature = "live-ui"), not(feature = "compiled-ui")))]
    {
        compile_error!("enable either live-ui or compiled-ui");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RealtimeFrameChartFixture {
    Small,
    Large,
}

fn realtime_frame_chart_fixture_args(
) -> Result<Option<RealtimeFrameChartFixture>, Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    parse_realtime_frame_chart_fixture_args(&args)
}

fn parse_realtime_frame_chart_fixture_args(
    args: &[String],
) -> Result<Option<RealtimeFrameChartFixture>, Box<dyn Error>> {
    if args.first().map(String::as_str) != Some("--realtime-frame-chart-fixture") {
        return Ok(None);
    }
    if args.len() != 2 {
        return Err("--realtime-frame-chart-fixture requires exactly small or large".into());
    }
    match args[1].as_str() {
        "small" => Ok(Some(RealtimeFrameChartFixture::Small)),
        "large" => Ok(Some(RealtimeFrameChartFixture::Large)),
        value => Err(format!("invalid realtime frame chart fixture: {value}").into()),
    }
}

fn realtime_frame_chart_fixture_view(fixture: RealtimeFrameChartFixture) -> RealtimeViewState {
    let count = match fixture {
        RealtimeFrameChartFixture::Small => 12,
        RealtimeFrameChartFixture::Large => 12_000,
    };
    let mut frame_samples = (0..count)
        .map(|frame| {
            let pulse = 800 + (frame % 17) as u64 * 70;
            FrameSample {
                frame: frame as u64,
                wall_us: 12_000 + pulse,
                prepare_us: 700 + pulse / 4,
                render_us: 2_800 + pulse,
                custom_draw_us: 1_100 + pulse / 2,
                vsync_us: 5_500 + pulse,
                present_us: 800 + pulse / 3,
                cpu_prepare_us: 260 + pulse / 8,
                cpu_render_us: 1_200 + pulse / 3,
                cpu_custom_draw_us: 500 + pulse / 5,
                cpu_vsync_us: 120 + pulse / 16,
                cpu_present_us: 300 + pulse / 10,
                process_cpu_us: 4_200 + pulse,
                over_budget: false,
                idle: false,
            }
        })
        .collect::<Vec<_>>();
    let spike = frame_samples.len() / 2;
    frame_samples[spike].wall_us = 31_500;
    frame_samples[spike].custom_draw_us = 19_500;
    frame_samples[spike].process_cpu_us = 24_000;
    frame_samples[spike].over_budget = true;
    let last = frame_samples.len() - 1;
    frame_samples[last].idle = true;

    let mut view = realtime_view_from_history(&RealtimeHistory::default(), false, "");
    view.status = "Deterministic frame chart inspection fixture.".to_string();
    view.frame_summary = format!("{count} fixture frames with spike, CPU, and idle markers");
    view.frame_samples = frame_samples;
    view
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FramebufferDisplayBenchSource {
    RealStream,
    Synthetic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FramebufferDisplayBenchArgs {
    source: FramebufferDisplayBenchSource,
    seconds: u64,
    chrome: bool,
    cadence_out: Option<PathBuf>,
}

fn framebuffer_display_bench_args() -> Result<Option<FramebufferDisplayBenchArgs>, Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    parse_framebuffer_display_bench_args(&args)
}

fn parse_framebuffer_display_bench_args(
    args: &[String],
) -> Result<Option<FramebufferDisplayBenchArgs>, Box<dyn Error>> {
    let source = match args.first().map(String::as_str) {
        Some("--framebuffer-stream-display-bench-secs") => {
            FramebufferDisplayBenchSource::RealStream
        }
        Some("--framebuffer-synthetic-display-bench-secs") => {
            FramebufferDisplayBenchSource::Synthetic
        }
        _ => return Ok(None),
    };
    let seconds = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("30")
        .parse::<u64>()?
        .max(1);
    let mut chrome = true;
    let mut cadence_out = None;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--chrome" => {
                let value = args.get(index + 1).ok_or("--chrome needs on or off")?;
                chrome = match value.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err("--chrome needs on or off".into()),
                };
                index += 2;
            }
            "--cadence-out" => {
                let value = args.get(index + 1).ok_or("--cadence-out needs a path")?;
                cadence_out = Some(PathBuf::from(value));
                index += 2;
            }
            other => {
                return Err(format!("unknown desktop display benchmark option: {other}").into());
            }
        }
    }
    Ok(Some(FramebufferDisplayBenchArgs {
        source,
        seconds,
        chrome,
        cadence_out,
    }))
}

fn default_slint_backend() -> &'static str {
    if cfg!(feature = "skia-renderer") {
        "winit-skia"
    } else {
        "winit-software"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FramebufferBenchMode {
    Poll,
    Stream,
    Drain,
    Dump(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FramebufferBenchLimit {
    Frames(u64),
    Duration(Duration),
}

impl FramebufferBenchMode {
    fn label(&self) -> &'static str {
        match self {
            Self::Poll => "poll",
            Self::Stream => "stream",
            Self::Drain => "drain",
            Self::Dump(_) => "dump",
        }
    }
}

fn framebuffer_stream_bench_args()
-> Result<Option<(FramebufferBenchMode, FramebufferBenchLimit)>, Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    parse_framebuffer_stream_bench_args(&args)
}

fn parse_framebuffer_stream_bench_args(
    args: &[String],
) -> Result<Option<(FramebufferBenchMode, FramebufferBenchLimit)>, Box<dyn Error>> {
    let Some(first) = args.first() else {
        return Ok(None);
    };
    let (mode, duration_mode) = match first.as_str() {
        "--framebuffer-stream-bench" => (FramebufferBenchMode::Stream, false),
        "--framebuffer-stream-bench-secs" => (FramebufferBenchMode::Stream, true),
        "--framebuffer-poll-bench" => (FramebufferBenchMode::Poll, false),
        "--framebuffer-stream-drain-bench" => (FramebufferBenchMode::Drain, false),
        "--framebuffer-stream-drain-bench-secs" => (FramebufferBenchMode::Drain, true),
        "--framebuffer-stream-dump" => {
            let dir = args
                .get(1)
                .ok_or("--framebuffer-stream-dump needs OUT_DIR [FRAMES]")?;
            (FramebufferBenchMode::Dump(PathBuf::from(dir)), false)
        }
        _ => return Ok(None),
    };
    let frame_arg_index = if matches!(mode, FramebufferBenchMode::Dump(_)) {
        2
    } else {
        1
    };
    let value = match args.get(frame_arg_index) {
        Some(value) => value.parse::<u64>()?,
        None => 120,
    };
    let limit = if duration_mode {
        FramebufferBenchLimit::Duration(Duration::from_secs(value.max(1)))
    } else {
        FramebufferBenchLimit::Frames(value.max(1))
    };
    Ok(Some((mode, limit)))
}

fn run_framebuffer_stream_bench(
    mode: FramebufferBenchMode,
    limit: FramebufferBenchLimit,
) -> Result<(), Box<dyn Error>> {
    let host = std::env::var("MISTER_IP").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let started = Instant::now();
    let mut latencies = Vec::new();
    let mut payload_bytes = 0_u64;
    let mut raw_bytes = 0_u64;
    match mode {
        FramebufferBenchMode::Stream => {
            let mut stream = connect_framebuffer_stream(&host)?;
            while !bench_limit_reached(limit, latencies.len() as u64, started.elapsed()) {
                let frame_started = Instant::now();
                let capture = stream.next_capture()?;
                let _image = framebuffer_capture_image(&capture);
                latencies.push(frame_started.elapsed());
                payload_bytes += capture.payload_bytes;
                raw_bytes += capture.raw_bytes;
            }
        }
        FramebufferBenchMode::Drain => {
            let stats = match limit {
                FramebufferBenchLimit::Frames(frames) => drain_framebuffer_stream(&host, frames)?,
                FramebufferBenchLimit::Duration(duration) => {
                    drain_framebuffer_stream_for(&host, duration)?
                }
            };
            latencies = stats.latencies;
            payload_bytes = stats.payload_bytes;
            raw_bytes = stats.raw_bytes;
        }
        FramebufferBenchMode::Poll => {
            while !bench_limit_reached(limit, latencies.len() as u64, started.elapsed()) {
                let frame_started = Instant::now();
                let capture = fetch_framebuffer_capture(&host)?;
                let _image = framebuffer_capture_image(&capture);
                latencies.push(frame_started.elapsed());
                payload_bytes += capture.payload_bytes;
                raw_bytes += capture.raw_bytes;
            }
        }
        FramebufferBenchMode::Dump(ref dir) => {
            std::fs::create_dir_all(dir)?;
            let seed_capture = fetch_framebuffer_capture(&host).ok();
            if let Some(capture) = seed_capture.as_ref() {
                let png = framebuffer_capture_png_bytes(capture)?;
                std::fs::write(dir.join("frame-0000-seed.png"), png)?;
            }
            let mut stream = connect_framebuffer_stream_seeded(&host, seed_capture.as_ref())?;
            let frames = match limit {
                FramebufferBenchLimit::Frames(frames) => frames,
                FramebufferBenchLimit::Duration(_) => unreachable!("dump is frame-count only"),
            };
            for idx in 0..frames {
                let frame_started = Instant::now();
                let capture = stream.next_capture()?;
                let png = framebuffer_capture_png_bytes(&capture)?;
                std::fs::write(dir.join(format!("frame-{idx:04}.png")), png)?;
                latencies.push(frame_started.elapsed());
                payload_bytes += capture.payload_bytes;
                raw_bytes += capture.raw_bytes;
            }
        }
    }
    latencies.sort();
    let elapsed = started.elapsed();
    let frames = latencies.len() as u64;
    if frames == 0 {
        return Err("framebuffer stream benchmark received no frames".into());
    }
    let fps = frames as f64 / elapsed.as_secs_f64();
    let p50 = latency_percentile_ms(&latencies, 0.50);
    let p95 = latency_percentile_ms(&latencies, 0.95);
    let payload_avg = payload_bytes / frames;
    let raw_avg = raw_bytes / frames;
    println!(
        "framebuffer_stream_bench_tsv\tmode={}\tframes={frames}\tfps={fps:.2}\telapsed_ms={:.0}\tp50_ms={p50:.1}\tp95_ms={p95:.1}\tavg_payload_bytes={payload_avg}\tavg_raw_bytes={raw_avg}",
        mode.label(),
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn bench_limit_reached(limit: FramebufferBenchLimit, frames: u64, elapsed: Duration) -> bool {
    match limit {
        FramebufferBenchLimit::Frames(target) => frames >= target,
        FramebufferBenchLimit::Duration(target) => elapsed >= target,
    }
}

fn latency_percentile_ms(latencies: &[Duration], percentile: f64) -> f64 {
    if latencies.is_empty() {
        return 0.0;
    }
    let rank = ((latencies.len() - 1) as f64 * percentile).round() as usize;
    latencies[rank].as_secs_f64() * 1000.0
}

fn select_backend() -> Result<(), slint::PlatformError> {
    let selector = slint::BackendSelector::new();
    #[cfg(target_os = "macos")]
    let selector =
        selector.with_winit_window_attributes_hook(macos_titlebar::apply_unified_titlebar);
    selector.select()
}

#[cfg(feature = "live-ui")]
fn run_live_ui(
    frame_chart_fixture: Option<RealtimeFrameChartFixture>,
) -> Result<(), Box<dyn Error>> {
    let ui_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/main.slint");
    let host = std::env::var("MISTER_IP").unwrap_or_else(|_| DEFAULT_HOST.to_string());

    loop {
        let reload_requested = Arc::new(AtomicBool::new(false));
        let stop_watcher = Arc::new(AtomicBool::new(false));
        let (instance, _render_metrics) =
            create_live_instance(&ui_path, &host, frame_chart_fixture)?;
        start_reload_watcher(
            &ui_path,
            Arc::clone(&reload_requested),
            Arc::clone(&stop_watcher),
        );
        instance.run()?;
        stop_watcher.store(true, Ordering::Relaxed);
        if !reload_requested.load(Ordering::Relaxed) {
            break;
        }
    }

    Ok(())
}

#[cfg(feature = "compiled-ui")]
fn prepare_compiled_framebuffer_benchmark_window(
    ui: &AppWindow,
    metrics: Arc<FramebufferRenderMetrics>,
) -> Result<(), Box<dyn Error>> {
    use slint::ComponentHandle;
    use slint::winit_030::WinitWindowAccessor;

    let ui_weak = ui.as_weak();
    slint::spawn_local(async move {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let Ok(winit_window) = ui.window().winit_window().await else {
            return;
        };
        metrics
            .focused
            .store(winit_window.has_focus(), Ordering::Release);
        if let Some(refresh_millihertz) = winit_window
            .current_monitor()
            .and_then(|monitor| monitor.refresh_rate_millihertz())
        {
            metrics
                .monitor_refresh_millihertz
                .store(u64::from(refresh_millihertz), Ordering::Release);
        }
        winit_window.set_visible(true);
        #[cfg(target_os = "macos")]
        let _ = macos_titlebar::activate_benchmark_window(&winit_window);
        winit_window.focus_window();
        winit_window.request_redraw();
    })
    .map_err(|err| format!("schedule framebuffer benchmark window setup: {err}"))?;
    Ok(())
}

#[cfg(feature = "compiled-ui")]
fn wait_for_framebuffer_benchmark_ready(
    metrics: &FramebufferRenderMetrics,
    timeout: Duration,
) -> Result<(), &'static str> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if metrics.benchmark_ready() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(metrics
        .benchmark_readiness_reason()
        .unwrap_or("benchmark_window_not_ready"))
}

#[cfg(feature = "compiled-ui")]
fn run_compiled_framebuffer_display_bench(
    args: FramebufferDisplayBenchArgs,
) -> Result<(), Box<dyn Error>> {
    use slint::ComponentHandle;

    let seconds = args.seconds;
    let host = std::env::var("MISTER_IP").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let ui = AppWindow::new()?;
    let capture = Arc::new(Mutex::new(None));
    let metrics = Arc::new(FramebufferRenderMetrics::default());
    let generation = Arc::new(AtomicU64::new(0));
    let control = Arc::new(Mutex::new(None));
    let invalid_reason = Arc::new(Mutex::new(None::<String>));
    let display_clock = Rc::new(RefCell::new(None::<FramebufferDisplayController>));

    let stream_ui = ui.as_weak();
    let stream_capture = Arc::clone(&capture);
    let stream_generation = Arc::clone(&generation);
    let stream_control = Arc::clone(&control);
    let stream_metrics = Arc::clone(&metrics);
    let chrome_enabled = args.chrome;
    let stream_display_clock = Rc::clone(&display_clock);
    ui.global::<Actions>()
        .on_live_stream_changed(move |enabled| {
            let next_generation = stream_generation.fetch_add(1, Ordering::SeqCst) + 1;
            cancel_framebuffer_stream(&stream_control);
            stream_display_clock.borrow_mut().take();
            if enabled {
                let mailbox = Arc::new(LatestMailbox::default());
                let display_state =
                    Arc::new(Mutex::new(FramebufferDisplayState::new(chrome_enabled)));
                let stream_start = Instant::now();
                *stream_display_clock.borrow_mut() = start_compiled_framebuffer_display_clock(
                    stream_ui.clone(),
                    Arc::clone(&stream_capture),
                    Arc::clone(&stream_generation),
                    Arc::clone(&mailbox),
                    display_state,
                    Arc::clone(&stream_metrics),
                    stream_start,
                    next_generation,
                );
                spawn_compiled_framebuffer_stream(
                    stream_ui.clone(),
                    Arc::clone(&stream_capture),
                    Arc::clone(&stream_generation),
                    Arc::clone(&stream_control),
                    Arc::clone(&stream_metrics),
                    mailbox,
                    host.clone(),
                    next_generation,
                );
            }
        });

    ui.global::<AppState>()
        .set_selected_page("analytics".into());
    ui.global::<AnalyticsState>().set_live_stream(true);
    ui.show()?;
    // Install after the native window exists. Installing before show() can
    // report notifier readiness without ever delivering AfterRendering.
    install_framebuffer_render_notifier(ui.window(), Arc::clone(&metrics));
    ui.window().request_redraw();
    #[cfg(target_os = "macos")]
    setup_macos_titlebar_for_compiled_ui(&ui);
    prepare_compiled_framebuffer_benchmark_window(&ui, Arc::clone(&metrics))?;
    let start_ui = ui.as_weak();
    slint::spawn_local(async move {
        use slint::winit_030::WinitWindowAccessor;
        let Some(ui) = start_ui.upgrade() else {
            return;
        };
        if ui.window().winit_window().await.is_ok() {
            ui.global::<Actions>().invoke_live_stream_changed(true);
        }
    })?;

    let timer_metrics = Arc::clone(&metrics);
    let timer_ui = ui.as_weak();
    let timer_invalid_reason = Arc::clone(&invalid_reason);
    std::thread::spawn(move || {
        match wait_for_framebuffer_benchmark_ready(&timer_metrics, Duration::from_secs(10)) {
            Ok(()) => {
                std::thread::sleep(Duration::from_secs(3));
                timer_metrics.reset();
                std::thread::sleep(Duration::from_secs(seconds));
                if let Some(reason) = timer_metrics.benchmark_invalid_reason() {
                    if let Ok(mut invalid) = timer_invalid_reason.lock() {
                        *invalid = Some(reason.to_string());
                    }
                }
            }
            Err(reason) => {
                if let Ok(mut invalid) = timer_invalid_reason.lock() {
                    *invalid = Some(reason.to_string());
                }
            }
        }
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = timer_ui.upgrade() {
                ui.global::<Actions>().invoke_live_stream_changed(false);
            }
            let _ = slint::quit_event_loop();
        });
    });

    slint::run_event_loop()?;
    ui.hide()?;
    let invalid_reason = invalid_reason.lock().ok().and_then(|reason| reason.clone());
    print_framebuffer_display_bench(
        seconds,
        "real-stream",
        args.chrome,
        args.cadence_out.as_deref(),
        &metrics,
        invalid_reason.as_deref(),
    );
    Ok(())
}

#[cfg(feature = "compiled-ui")]
fn run_compiled_synthetic_display_bench(
    args: FramebufferDisplayBenchArgs,
) -> Result<(), Box<dyn Error>> {
    use slint::ComponentHandle;

    let seconds = args.seconds;
    let ui = AppWindow::new()?;
    let capture = Arc::new(Mutex::new(None));
    let metrics = Arc::new(FramebufferRenderMetrics::default());
    let generation = Arc::new(AtomicU64::new(1));
    let invalid_reason = Arc::new(Mutex::new(None::<String>));
    let frames = Arc::new(build_synthetic_display_frames());
    let mailbox = Arc::new(LatestMailbox::default());
    let display_state = Arc::new(Mutex::new(FramebufferDisplayState::new(args.chrome)));
    let stream_start = Instant::now();
    let display_clock = Rc::new(RefCell::new(None::<FramebufferDisplayController>));

    ui.global::<AppState>()
        .set_selected_page("analytics".into());
    ui.global::<AnalyticsState>().set_live_stream(true);
    ui.global::<AnalyticsState>()
        .set_live_stream_summary("Synthetic 60fps SharedPixelBuffer source.".into());
    ui.show()?;
    install_framebuffer_render_notifier(ui.window(), Arc::clone(&metrics));
    ui.window().request_redraw();
    #[cfg(target_os = "macos")]
    setup_macos_titlebar_for_compiled_ui(&ui);
    prepare_compiled_framebuffer_benchmark_window(&ui, Arc::clone(&metrics))?;
    let clock_ui = ui.as_weak();
    let clock_capture = Arc::clone(&capture);
    let clock_generation = Arc::clone(&generation);
    let clock_mailbox = Arc::clone(&mailbox);
    let clock_display_state = Arc::clone(&display_state);
    let clock_metrics = Arc::clone(&metrics);
    let clock_holder = Rc::clone(&display_clock);
    slint::spawn_local(async move {
        use slint::winit_030::WinitWindowAccessor;
        let Some(ui) = clock_ui.upgrade() else {
            return;
        };
        if ui.window().winit_window().await.is_err() {
            return;
        }
        *clock_holder.borrow_mut() = start_compiled_framebuffer_display_clock(
            ui.as_weak(),
            clock_capture,
            clock_generation,
            clock_mailbox,
            clock_display_state,
            clock_metrics,
            stream_start,
            1,
        );
    })?;
    spawn_compiled_synthetic_display_source(
        Arc::clone(&generation),
        Arc::clone(&metrics),
        frames,
        mailbox,
        stream_start,
        1,
    );

    let timer_metrics = Arc::clone(&metrics);
    let timer_generation = Arc::clone(&generation);
    let timer_invalid_reason = Arc::clone(&invalid_reason);
    std::thread::spawn(move || {
        match wait_for_framebuffer_benchmark_ready(&timer_metrics, Duration::from_secs(10)) {
            Ok(()) => {
                std::thread::sleep(Duration::from_secs(3));
                timer_metrics.reset();
                std::thread::sleep(Duration::from_secs(seconds));
                if let Some(reason) = timer_metrics.benchmark_invalid_reason() {
                    if let Ok(mut invalid) = timer_invalid_reason.lock() {
                        *invalid = Some(reason.to_string());
                    }
                }
            }
            Err(reason) => {
                if let Ok(mut invalid) = timer_invalid_reason.lock() {
                    *invalid = Some(reason.to_string());
                }
            }
        }
        timer_generation.fetch_add(1, Ordering::SeqCst);
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });

    slint::run_event_loop()?;
    ui.hide()?;
    let invalid_reason = invalid_reason.lock().ok().and_then(|reason| reason.clone());
    print_framebuffer_display_bench(
        seconds,
        "synthetic",
        args.chrome,
        args.cadence_out.as_deref(),
        &metrics,
        invalid_reason.as_deref(),
    );
    Ok(())
}

#[cfg(feature = "compiled-ui")]
fn build_synthetic_display_frames() -> Vec<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    const WIDTH: u32 = 480;
    const HEIGHT: u32 = 270;
    const FRAME_COUNT: usize = 60;
    let mut frames = Vec::with_capacity(FRAME_COUNT);
    for sequence in 0..FRAME_COUNT {
        let mut frame = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(WIDTH, HEIGHT);
        let bytes = frame.make_mut_bytes();
        for (index, pixel) in bytes.chunks_exact_mut(4).enumerate() {
            let x = (index % WIDTH as usize) as u8;
            let y = (index / WIDTH as usize) as u8;
            pixel.copy_from_slice(&[12 + x / 8, 18 + y / 8, 32 + x / 12, 255]);
        }

        let marker_x = (sequence * 7) % (WIDTH as usize - 16);
        fill_synthetic_rect(
            bytes,
            WIDTH,
            marker_x,
            28,
            16,
            HEIGHT as usize - 56,
            [45, 212, 191, 255],
        );
        for bit in 0..6 {
            let color = if sequence & (1 << bit) == 0 {
                [42, 49, 66, 255]
            } else {
                [248, 196, 79, 255]
            };
            fill_synthetic_rect(bytes, WIDTH, 16 + bit * 18, 8, 12, 12, color);
        }
        frames.push(frame);
    }
    frames
}

#[cfg(feature = "compiled-ui")]
fn fill_synthetic_rect(
    rgba: &mut [u8],
    stride_pixels: u32,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: [u8; 4],
) {
    let stride = stride_pixels as usize * 4;
    for row in y..y + height {
        let start = row * stride + x * 4;
        for pixel in rgba[start..start + width * 4].chunks_exact_mut(4) {
            pixel.copy_from_slice(&color);
        }
    }
}

#[cfg(feature = "compiled-ui")]
fn synthetic_stream_frame(
    sequence: u64,
    timestamp_us: u64,
) -> agent_client::FramebufferStreamFrame {
    let geometry = mister_magik_framebuffer_stream::FrameGeometry {
        width: 480,
        height: 270,
        stride_pixels: 480,
    };
    let now = Instant::now();
    agent_client::FramebufferStreamFrame {
        capture: agent_client::FramebufferCapture {
            png_path: PathBuf::new(),
            rgba_pixels: Vec::new(),
            raw_pixels: Vec::new(),
            raw_stride_bytes: 0,
            width: u64::from(geometry.width),
            height: u64::from(geometry.height),
            bpp: 16,
            raw_bytes: u64::from(geometry.width) * u64::from(geometry.height) * 2,
            payload_bytes: 0,
            encoding: "synthetic-shared-pixel-buffer".to_string(),
            png_bytes: 0,
            png_hex_bytes: 0,
            timing: agent_client::FramebufferCaptureTiming::default(),
        },
        kind: if sequence == 0 {
            mister_magik_framebuffer_stream::FrameKind::Keyframe
        } else {
            mister_magik_framebuffer_stream::FrameKind::RectDelta
        },
        sequence,
        timestamp_us,
        geometry,
        rect: mister_magik_framebuffer_stream::FrameRect::full(geometry),
        raw_bytes: u64::from(geometry.width) * u64::from(geometry.height) * 2,
        payload_bytes: 0,
        timing: agent_client::FramebufferStreamTiming {
            read_started: now,
            read_complete: now,
            decompress_complete: now,
            rgba_complete: now,
        },
    }
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_synthetic_display_source(
    stream_generation: SharedLiveStreamGeneration,
    render_metrics: Arc<FramebufferRenderMetrics>,
    frames: Arc<Vec<slint::SharedPixelBuffer<slint::Rgba8Pixel>>>,
    mailbox: SharedFramebufferDisplayMailbox,
    stream_start: Instant,
    generation: u64,
) {
    std::thread::spawn(move || {
        let frame_interval = Duration::from_nanos(16_666_667);
        let mut next_frame_at = Instant::now();
        let mut sequence = 0_u64;
        while analytics_ui_state::generation_is_current(
            stream_generation.load(Ordering::SeqCst),
            generation,
        ) {
            let received_at = Instant::now();
            let timestamp_us = stream_start.elapsed().as_micros() as u64;
            let frame = synthetic_stream_frame(sequence, timestamp_us);
            let pixels = frames[sequence as usize % frames.len()].clone();
            render_metrics.cadence.record(
                CadenceEventKind::SourceReceived,
                sequence,
                timestamp_us,
                0,
                0,
                0,
            );
            render_metrics.cadence.record(
                CadenceEventKind::DecodeComplete,
                sequence,
                timestamp_us,
                0,
                0,
                0,
            );
            render_metrics.cadence.record(
                CadenceEventKind::PixelBufferReady,
                sequence,
                timestamp_us,
                0,
                0,
                0,
            );
            let (_, coalesced_before) = mailbox.stats();
            let _ = mailbox.publish(FramebufferDisplayUpdate {
                frame,
                pixels: Some(pixels),
                received_at,
            });
            let (_, coalesced) = mailbox.stats();
            render_metrics.cadence.record(
                if coalesced > coalesced_before {
                    CadenceEventKind::MailboxReplace
                } else {
                    CadenceEventKind::MailboxPublish
                },
                sequence,
                timestamp_us,
                0,
                0,
                0,
            );
            render_metrics.mark_received(coalesced);
            sequence = sequence.wrapping_add(1);
            next_frame_at += frame_interval;
            if let Some(remaining) = next_frame_at.checked_duration_since(Instant::now()) {
                std::thread::sleep(remaining);
            } else {
                next_frame_at = Instant::now();
            }
        }
        mailbox.close();
    });
}

#[cfg(feature = "compiled-ui")]
fn print_framebuffer_display_bench(
    seconds: u64,
    source: &str,
    chrome: bool,
    cadence_out: Option<&Path>,
    render_metrics: &FramebufferRenderMetrics,
    invalid_reason: Option<&str>,
) {
    let snapshot = render_metrics.snapshot(Instant::now());
    let invalid_reason = invalid_reason.unwrap_or("none");
    println!(
        "framebuffer_display_bench_tsv\tsource={source}\tchrome={}\tclock={}\tseconds={seconds}\tbuild_profile={}\tcompleted={}\tinvalid_reason={invalid_reason}\treceived={}\tapplied={}\trendered={}\trender_callbacks={}\treceived_fps={:.2}\tapplied_fps={:.2}\trendered_fps={:.2}\tcoalesced={}\trender_p95_ms={:.1}\tnotifier_supported={}\tobserver_ready={}\twinit_observer_ready={}\trendering_notifier_ready={}\tfocused={}\toccluded={}\tlost_focus={}\twas_occluded={}\tmonitor_refresh_millihertz={}\tdisplay_ticks={}\twinit_events={}\twinit_redraws={}\tforeground_redraws={}\trendering_setup={}\trendering_before={}\trendering_after={}\trendering_teardown={}",
        if chrome { "on" } else { "off" },
        render_metrics.clock_kind().label(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        u8::from(invalid_reason == "none"),
        snapshot.received,
        snapshot.applied,
        snapshot.rendered,
        snapshot.render_callbacks,
        snapshot.received_fps,
        snapshot.applied_fps,
        snapshot.rendered_fps,
        snapshot.coalesced,
        snapshot.latency_p95_ms,
        u8::from(snapshot.supported),
        u8::from(render_metrics.observer_ready.load(Ordering::Acquire)),
        u8::from(render_metrics.winit_observer_ready.load(Ordering::Acquire),),
        u8::from(
            render_metrics
                .rendering_notifier_ready
                .load(Ordering::Acquire),
        ),
        u8::from(render_metrics.focused.load(Ordering::Acquire)),
        u8::from(render_metrics.occluded.load(Ordering::Acquire)),
        u8::from(
            render_metrics
                .lost_focus_during_measurement
                .load(Ordering::Acquire),
        ),
        u8::from(
            render_metrics
                .occluded_during_measurement
                .load(Ordering::Acquire),
        ),
        render_metrics
            .monitor_refresh_millihertz
            .load(Ordering::Relaxed),
        render_metrics.display_ticks.load(Ordering::Relaxed),
        render_metrics.winit_events.load(Ordering::Relaxed),
        render_metrics.winit_redraws.load(Ordering::Relaxed),
        render_metrics.foreground_redraws.load(Ordering::Relaxed),
        render_metrics.rendering_setup.load(Ordering::Relaxed),
        render_metrics.rendering_before.load(Ordering::Relaxed),
        render_metrics.rendering_after.load(Ordering::Relaxed),
        render_metrics.rendering_teardown.load(Ordering::Relaxed),
    );
    let cadence = render_metrics
        .cadence
        .summary(CadenceEventKind::AfterRendering);
    println!(
        "framebuffer_cadence_summary_tsv\tobserver=after-rendering\tsamples={}\tinterval_p50_us={}\tinterval_p95_us={}\tinterval_p99_us={}\tinterval_max_us={}\tgaps_over_20ms={}\tgaps_over_34ms={}\tmax_consecutive_over_20ms={}\tbucket_500ms_min={}\tbucket_500ms_max={}",
        cadence.samples,
        cadence.interval_p50_us,
        cadence.interval_p95_us,
        cadence.interval_p99_us,
        cadence.interval_max_us,
        cadence.gaps_over_20ms,
        cadence.gaps_over_34ms,
        cadence.max_consecutive_over_20ms,
        cadence.bucket_500ms_min,
        cadence.bucket_500ms_max,
    );
    let cadence_path = cadence_out
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("MISTER_FRAMEBUFFER_CADENCE_OUT").map(PathBuf::from));
    if let Some(path) = cadence_path {
        match render_metrics.cadence.write_tsv(&path) {
            Ok(()) => println!(
                "framebuffer_cadence_artifact_tsv\tpath={}\tevents={}",
                path.display(),
                render_metrics.cadence.events().len()
            ),
            Err(err) => eprintln!(
                "write framebuffer cadence artifact {}: {err}",
                path.display()
            ),
        }
    }
}

#[cfg(feature = "live-ui")]
fn create_live_instance(
    ui_path: &Path,
    host: &str,
    frame_chart_fixture: Option<RealtimeFrameChartFixture>,
) -> Result<
    (
        slint_interpreter::ComponentInstance,
        Arc<FramebufferRenderMetrics>,
    ),
    Box<dyn Error>,
> {
    use slint::ComponentHandle;
    use slint_interpreter::{Compiler, Value};

    let compiler = Compiler::default();
    let result = spin_on::spin_on(compiler.build_from_path(ui_path));
    result.print_diagnostics();
    if result.has_errors() {
        return Err("Slint UI has compile errors".into());
    }
    let definition = result
        .component("AppWindow")
        .ok_or("ui/main.slint must export AppWindow")?;
    let instance = definition.create()?;
    let sd_browser = Arc::new(Mutex::new(SdCardBrowser::new()));
    let library_browser = Arc::new(Mutex::new(LibraryBrowser::new()));
    let framebuffer_capture = Arc::new(Mutex::new(None));
    let framebuffer_render_metrics = Arc::new(FramebufferRenderMetrics::default());
    install_framebuffer_render_notifier(instance.window(), Arc::clone(&framebuffer_render_metrics));
    let live_stream_generation = Arc::new(AtomicU64::new(0));
    let live_stream_control = Arc::new(Mutex::new(None));
    let live_display_clock = Rc::new(RefCell::new(None::<FramebufferDisplayController>));
    let realtime_stream_generation = Arc::new(AtomicU64::new(0));
    let realtime_stream_control = Arc::new(Mutex::new(None));
    let realtime_frame_chart = Arc::new(Mutex::new(FrameChartState::default()));
    let realtime_debug_page_active = Arc::new(AtomicBool::new(true));
    let realtime_debug_tab_index = Arc::new(AtomicI32::new(0));

    let refresh_instance = instance.as_weak();
    let refresh_host = host.to_string();
    instance.set_global_callback("Actions", "refresh-status", move |_| {
        if let Some(instance) = refresh_instance.upgrade() {
            let snapshot = fetch_dashboard(&refresh_host);
            apply_live_snapshot(&instance, &snapshot);
        }
        Value::Void
    })?;

    let select_instance = instance.as_weak();
    let select_realtime_generation = Arc::clone(&realtime_stream_generation);
    let select_realtime_control = Arc::clone(&realtime_stream_control);
    let select_frame_chart = Arc::clone(&realtime_frame_chart);
    let select_realtime_page_active = Arc::clone(&realtime_debug_page_active);
    let select_realtime_tab_index = Arc::clone(&realtime_debug_tab_index);
    let select_realtime_host = host.to_string();
    instance.set_global_callback("Actions", "select-page", move |args| {
        if let Some(instance) = select_instance.upgrade() {
            if let Some(Value::String(page)) = args.first() {
                let _ = instance.set_global_property(
                    "AppState",
                    "selected-page",
                    Value::String(page.clone()),
                );
                let debug_active = page.as_str() == "debug";
                select_realtime_page_active.store(debug_active, Ordering::SeqCst);
                start_or_stop_live_realtime(
                    select_instance.clone(),
                    Arc::clone(&select_realtime_generation),
                    Arc::clone(&select_realtime_control),
                    Arc::clone(&select_frame_chart),
                    frame_chart_fixture,
                    select_realtime_host.clone(),
                    debug_active && select_realtime_tab_index.load(Ordering::SeqCst) == 1,
                );
            }
        }
        Value::Void
    })?;

    let debug_tab_instance = instance.as_weak();
    let debug_tab_host = host.to_string();
    let debug_tab_generation = Arc::clone(&realtime_stream_generation);
    let debug_tab_control = Arc::clone(&realtime_stream_control);
    let debug_tab_frame_chart = Arc::clone(&realtime_frame_chart);
    let debug_tab_page_active = Arc::clone(&realtime_debug_page_active);
    let debug_tab_index_state = Arc::clone(&realtime_debug_tab_index);
    instance.set_global_callback("Actions", "debug-tab-changed", move |args| {
        let Some(Value::Number(index)) = args.first() else {
            return Value::Void;
        };
        let index = *index as i32;
        debug_tab_index_state.store(index, Ordering::SeqCst);
        let active = debug_tab_page_active.load(Ordering::SeqCst) && index == 1;
        start_or_stop_live_realtime(
            debug_tab_instance.clone(),
            Arc::clone(&debug_tab_generation),
            Arc::clone(&debug_tab_control),
            Arc::clone(&debug_tab_frame_chart),
            frame_chart_fixture,
            debug_tab_host.clone(),
            active,
        );
        Value::Void
    })?;

    let realtime_instance = instance.as_weak();
    let realtime_host = host.to_string();
    let realtime_generation = Arc::clone(&realtime_stream_generation);
    let realtime_control = Arc::clone(&realtime_stream_control);
    let realtime_frame_chart_for_stream = Arc::clone(&realtime_frame_chart);
    instance.set_global_callback("Actions", "realtime-stream-changed", move |args| {
        let Some(Value::Bool(active)) = args.first() else {
            return Value::Void;
        };
        start_or_stop_live_realtime(
            realtime_instance.clone(),
            Arc::clone(&realtime_generation),
            Arc::clone(&realtime_control),
            Arc::clone(&realtime_frame_chart_for_stream),
            frame_chart_fixture,
            realtime_host.clone(),
            *active,
        );
        Value::Void
    })?;

    let resize_instance = instance.as_weak();
    let resize_frame_chart_state = Arc::clone(&realtime_frame_chart);
    instance.set_global_callback("Actions", "realtime-frame-chart-resized", move |args| {
        let (Some(Value::Number(width)), Some(Value::Number(height))) =
            (args.first(), args.get(1))
        else {
            return Value::Void;
        };
        if let (Some(instance), Some(rendered)) = (
            resize_instance.upgrade(),
            resize_frame_chart(
                &resize_frame_chart_state,
                *width as i32,
                *height as i32,
            ),
        ) {
            apply_live_frame_chart(&instance, rendered);
        }
        Value::Void
    })?;

    let capture_instance = instance.as_weak();
    let capture_host = host.to_string();
    let capture_state = Arc::clone(&framebuffer_capture);
    instance.set_global_callback("Actions", "capture-framebuffer", move |_| {
        if let Some(instance) = capture_instance.upgrade() {
            set_live_analytics_loading(&instance);
        }
        spawn_live_framebuffer_capture(
            capture_instance.clone(),
            Arc::clone(&capture_state),
            capture_host.clone(),
        );
        Value::Void
    })?;

    let save_instance = instance.as_weak();
    let save_capture = Arc::clone(&framebuffer_capture);
    instance.set_global_callback("Actions", "save-framebuffer-image", move |_| {
        if let Some(instance) = save_instance.upgrade() {
            apply_live_save_status(&instance, "Saving framebuffer PNG...", "");
        }
        spawn_live_save_framebuffer_capture(save_instance.clone(), Arc::clone(&save_capture));
        Value::Void
    })?;

    let stream_instance = instance.as_weak();
    let stream_host = host.to_string();
    let stream_capture = Arc::clone(&framebuffer_capture);
    let stream_generation = Arc::clone(&live_stream_generation);
    let stream_control = Arc::clone(&live_stream_control);
    let stream_render_metrics = Arc::clone(&framebuffer_render_metrics);
    let stream_display_clock = Rc::clone(&live_display_clock);
    instance.set_global_callback("Actions", "live-stream-changed", move |args| {
        let Some(Value::Bool(enabled)) = args.first() else {
            return Value::Void;
        };
        let generation = stream_generation.fetch_add(1, Ordering::SeqCst) + 1;
        cancel_framebuffer_stream(&stream_control);
        stream_display_clock.borrow_mut().take();
        if let Some(instance) = stream_instance.upgrade() {
            apply_live_stream_summary(
                &instance,
                if *enabled {
                    "Live stream starting..."
                } else {
                    "Live stream off."
                },
            );
        }
        if *enabled {
            let mailbox = Arc::new(LatestMailbox::default());
            let display_state = Arc::new(Mutex::new(FramebufferDisplayState::default()));
            let stream_start = Instant::now();
            *stream_display_clock.borrow_mut() = start_live_framebuffer_display_clock(
                stream_instance.clone(),
                Arc::clone(&stream_capture),
                Arc::clone(&stream_generation),
                Arc::clone(&mailbox),
                display_state,
                Arc::clone(&stream_render_metrics),
                stream_start,
                generation,
            );
            spawn_live_framebuffer_stream(
                stream_instance.clone(),
                Arc::clone(&stream_capture),
                Arc::clone(&stream_generation),
                Arc::clone(&stream_control),
                Arc::clone(&stream_render_metrics),
                mailbox,
                stream_host.clone(),
                generation,
            );
        }
        Value::Void
    })?;

    let profile_instance = instance.as_weak();
    instance.set_global_callback("Actions", "load-profile-artifact", move |args| {
        let Some(Value::String(path)) = args.first() else {
            return Value::Void;
        };
        if let Some(instance) = profile_instance.upgrade() {
            set_live_profile_loading(&instance, path);
        }
        spawn_live_profile_load(profile_instance.clone(), path.to_string());
        Value::Void
    })?;

    let sd_toggle_instance = instance.as_weak();
    let sd_toggle_host = host.to_string();
    let sd_toggle_browser = Arc::clone(&sd_browser);
    instance.set_global_callback("Actions", "sd-row-toggle", move |args| {
        let Some(Value::String(path)) = args.first() else {
            return Value::Void;
        };
        if let Some(fetch_path) = sd_toggle_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.toggle_directory(path.as_str()))
        {
            let show_hidden = sd_toggle_browser
                .lock()
                .map(|browser| browser.show_hidden())
                .unwrap_or(false);
            spawn_live_sd_fetch(
                sd_toggle_instance.clone(),
                Arc::clone(&sd_toggle_browser),
                sd_toggle_host.clone(),
                fetch_path,
                show_hidden,
            );
        }
        if let Some(instance) = sd_toggle_instance.upgrade() {
            apply_live_sd_state(&instance, &sd_toggle_browser);
        }
        Value::Void
    })?;

    let sd_current_instance = instance.as_weak();
    let sd_current_host = host.to_string();
    let sd_current_browser = Arc::clone(&sd_browser);
    instance.set_global_callback("Actions", "sd-row-current", move |args| {
        if let Some(Value::String(path)) = args.first() {
            let detail_request = match sd_current_browser.lock() {
                Ok(mut browser) => {
                    browser.select_path(path.as_str());
                    browser.begin_detail_fetch_current(false)
                }
                Err(_) => None,
            };
            if let Some(instance) = sd_current_instance.upgrade() {
                apply_live_sd_state(&instance, &sd_current_browser);
            }
            if let Some(detail_request) = detail_request {
                spawn_live_sd_detail_fetch(
                    sd_current_instance.clone(),
                    Arc::clone(&sd_current_browser),
                    sd_current_host.clone(),
                    detail_request,
                );
            }
        }
        Value::Void
    })?;

    let sd_refresh_instance = instance.as_weak();
    let sd_refresh_host = host.to_string();
    let sd_refresh_browser = Arc::clone(&sd_browser);
    instance.set_global_callback("Actions", "sd-refresh-folder", move |_| {
        if let Some(fetch_path) = sd_refresh_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.refresh_current_folder())
        {
            let show_hidden = sd_refresh_browser
                .lock()
                .map(|browser| browser.show_hidden())
                .unwrap_or(false);
            spawn_live_sd_fetch(
                sd_refresh_instance.clone(),
                Arc::clone(&sd_refresh_browser),
                sd_refresh_host.clone(),
                fetch_path,
                show_hidden,
            );
        }
        if let Some(instance) = sd_refresh_instance.upgrade() {
            apply_live_sd_state(&instance, &sd_refresh_browser);
        }
        Value::Void
    })?;

    let sd_detail_refresh_instance = instance.as_weak();
    let sd_detail_refresh_host = host.to_string();
    let sd_detail_refresh_browser = Arc::clone(&sd_browser);
    instance.set_global_callback("Actions", "sd-refresh-details", move |_| {
        let detail_request = sd_detail_refresh_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.begin_detail_fetch_current(true));
        if let Some(instance) = sd_detail_refresh_instance.upgrade() {
            apply_live_sd_state(&instance, &sd_detail_refresh_browser);
        }
        if let Some(detail_request) = detail_request {
            spawn_live_sd_detail_fetch(
                sd_detail_refresh_instance.clone(),
                Arc::clone(&sd_detail_refresh_browser),
                sd_detail_refresh_host.clone(),
                detail_request,
            );
        }
        Value::Void
    })?;

    let sd_hidden_instance = instance.as_weak();
    let sd_hidden_host = host.to_string();
    let sd_hidden_browser = Arc::clone(&sd_browser);
    instance.set_global_callback("Actions", "sd-show-hidden-changed", move |args| {
        let Some(Value::Bool(show_hidden)) = args.first() else {
            return Value::Void;
        };
        if let Some(fetch_path) = sd_hidden_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.set_show_hidden(*show_hidden))
        {
            spawn_live_sd_fetch(
                sd_hidden_instance.clone(),
                Arc::clone(&sd_hidden_browser),
                sd_hidden_host.clone(),
                fetch_path,
                *show_hidden,
            );
        }
        if let Some(instance) = sd_hidden_instance.upgrade() {
            apply_live_sd_state(&instance, &sd_hidden_browser);
        }
        Value::Void
    })?;

    let library_sync_instance = instance.as_weak();
    let library_sync_host = host.to_string();
    let library_sync_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "sync-library", move |_| {
        if let Ok(mut browser) = library_sync_browser.lock() {
            if browser.loading {
                return Value::Void;
            }
            browser.start_sync();
        }
        if let Some(instance) = library_sync_instance.upgrade() {
            apply_live_library_state(&instance, &library_sync_browser);
        }
        spawn_live_library_sync(
            library_sync_instance.clone(),
            Arc::clone(&library_sync_browser),
            library_sync_host.clone(),
        );
        Value::Void
    })?;

    let library_query_instance = instance.as_weak();
    let library_query_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-query-changed", move |args| {
        if let Some(Value::String(query)) = args.first() {
            if let Ok(mut browser) = library_query_browser.lock() {
                browser.set_query(query.as_str());
            }
            if let Some(instance) = library_query_instance.upgrade() {
                apply_live_library_state(&instance, &library_query_browser);
            }
        }
        Value::Void
    })?;
    let library_filter_instance = instance.as_weak();
    let library_filter_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-filter-changed", move |args| {
        let (Some(Value::String(filter)), Some(Value::String(value))) = (args.first(), args.get(1))
        else {
            return Value::Void;
        };
        if let Ok(mut browser) = library_filter_browser.lock() {
            browser.set_filter(filter.as_str(), value.as_str());
        }
        if let Some(instance) = library_filter_instance.upgrade() {
            apply_live_library_state(&instance, &library_filter_browser);
        }
        Value::Void
    })?;
    let library_sort_instance = instance.as_weak();
    let library_sort_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-sort-toggled", move |args| {
        let Some(Value::String(column)) = args.first() else {
            return Value::Void;
        };
        let direction = match args.get(1) {
            Some(Value::EnumerationValue(_, direction)) => direction.as_str(),
            _ => "ascending",
        };
        if let Ok(mut browser) = library_sort_browser.lock() {
            browser.set_sort(column.as_str(), direction);
        }
        if let Some(instance) = library_sort_instance.upgrade() {
            apply_live_library_state(&instance, &library_sort_browser);
        }
        Value::Void
    })?;
    let library_page_instance = instance.as_weak();
    let library_page_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-page-changed", move |args| {
        if let Some(Value::Number(page)) = args.first() {
            if let Ok(mut browser) = library_page_browser.lock() {
                browser.set_page(*page as i32);
            }
            if let Some(instance) = library_page_instance.upgrade() {
                apply_live_library_state(&instance, &library_page_browser);
            }
        }
        Value::Void
    })?;
    let library_row_instance = instance.as_weak();
    let library_row_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-row-selected", move |args| {
        if let Some(Value::String(id)) = args.first() {
            if let Ok(mut browser) = library_row_browser.lock() {
                browser.select_row(id.as_str());
            }
            if let Some(instance) = library_row_instance.upgrade() {
                apply_live_library_state(&instance, &library_row_browser);
            }
        }
        Value::Void
    })?;

    let drag_instance = instance.as_weak();
    instance.set_global_callback("WindowActions", "start-window-drag", move |_| {
        if let Some(instance) = drag_instance.upgrade() {
            start_window_drag(instance.window());
        }
        Value::Void
    })?;

    let snapshot = if frame_chart_fixture.is_some() {
        DashboardSnapshot::initial(host)
    } else {
        fetch_dashboard(host)
    };
    apply_live_snapshot(&instance, &snapshot);
    apply_live_sd_state(&instance, &sd_browser);
    apply_live_library_state(&instance, &library_browser);
    if let Some(fixture) = frame_chart_fixture {
        let _ = instance.set_global_property("AppState", "debug-tab-index", Value::Number(1.0));
        apply_live_realtime_view(
            &instance,
            &realtime_frame_chart_fixture_view(fixture),
            &realtime_frame_chart,
        );
    }
    #[cfg(target_os = "macos")]
    setup_macos_titlebar_for_live_instance(&instance);
    Ok((instance, framebuffer_render_metrics))
}

#[cfg(feature = "live-ui")]
fn apply_live_snapshot(
    instance: &slint_interpreter::ComponentInstance,
    snapshot: &DashboardSnapshot,
) {
    use slint::SharedString;
    use slint_interpreter::Value;

    fn set(instance: &slint_interpreter::ComponentInstance, name: &str, value: &str) {
        let _ = instance.set_global_property(
            "DeviceState",
            name,
            Value::String(SharedString::from(value)),
        );
    }

    set(instance, "host", &snapshot.host);
    set(instance, "connection-state", &snapshot.connection_state);
    set(instance, "agent-status", &snapshot.agent_status);
    set(instance, "token-source", &snapshot.token_source);
    set(instance, "agent-version", &snapshot.agent_version);
    set(instance, "agent-uptime", &snapshot.agent_uptime);
    set(instance, "network-summary", &snapshot.network_summary);
    set(instance, "mac-address", &snapshot.mac_address);
    set(instance, "main-process", &snapshot.main_process);
    set(instance, "launcher-process", &snapshot.launcher_process);
    set(instance, "launcher-state", &snapshot.launcher_state);
    set(instance, "visible-owner", &snapshot.visible_owner);
    set(
        instance,
        "slint-status-freshness",
        &snapshot.slint_status_freshness,
    );
    set(
        instance,
        "scanout-slots-summary",
        &snapshot.scanout_slots_summary,
    );
    set(
        instance,
        "scanout-slots-detail",
        &snapshot.scanout_slots_detail,
    );
    set(instance, "catalog-summary", &snapshot.catalog_summary);
    set(instance, "screen-summary", &snapshot.screen_summary);
    set(instance, "input-summary", &snapshot.input_summary);
    set(instance, "last-error", &snapshot.last_error);
}

#[cfg(feature = "live-ui")]
fn apply_live_sd_state(instance: &slint_interpreter::ComponentInstance, browser: &SharedSdBrowser) {
    use slint::{Image, ModelRc, SharedString, VecModel};
    use slint_interpreter::Value;

    let Ok(browser) = browser.lock() else {
        return;
    };

    fn set(instance: &slint_interpreter::ComponentInstance, name: &str, value: Value) {
        let _ = instance.set_global_property("SdCardState", name, value);
    }

    set(
        instance,
        "current-path",
        Value::String(SharedString::from(browser.current_path())),
    );
    set(
        instance,
        "status",
        Value::String(SharedString::from(browser.status())),
    );
    set(
        instance,
        "last-error",
        Value::String(SharedString::from(browser.last_error())),
    );
    set(instance, "loading", Value::Bool(browser.loading()));
    set(instance, "show-hidden", Value::Bool(browser.show_hidden()));
    let detail = browser.selected_detail();
    set(
        instance,
        "detail-title",
        Value::String(SharedString::from(detail.title.as_str())),
    );
    set(
        instance,
        "detail-subtitle",
        Value::String(SharedString::from(detail.subtitle.as_str())),
    );
    set(
        instance,
        "detail-kind",
        Value::String(SharedString::from(detail.kind.as_str())),
    );
    set(
        instance,
        "detail-icon-key",
        Value::String(SharedString::from(detail.icon_key.as_str())),
    );
    set(
        instance,
        "detail-size",
        Value::String(SharedString::from(detail.size_label.as_str())),
    );
    set(
        instance,
        "detail-modified",
        Value::String(SharedString::from(detail.modified_label.as_str())),
    );
    set(
        instance,
        "detail-flags",
        Value::String(SharedString::from(detail.flags_label.as_str())),
    );
    set(instance, "detail-loading", Value::Bool(detail.loading));
    set(
        instance,
        "detail-error",
        Value::String(SharedString::from(detail.error.as_str())),
    );
    set(
        instance,
        "detail-has-image",
        Value::Bool(detail.has_image && !detail.image_path.is_empty()),
    );
    set(
        instance,
        "detail-image",
        Value::Image(if detail.image_path.is_empty() {
            Image::default()
        } else {
            Image::load_from_path(Path::new(&detail.image_path)).unwrap_or_default()
        }),
    );
    set(
        instance,
        "detail-image-summary",
        Value::String(SharedString::from(detail.image_summary.as_str())),
    );
    set(instance, "detail-is-mra", Value::Bool(detail.is_mra));
    set(
        instance,
        "detail-overview-rows",
        Value::Model(ModelRc::new(VecModel::from(live_sd_metadata_rows(
            &detail.overview_rows,
        )))),
    );
    set(
        instance,
        "detail-mra-summary-rows",
        Value::Model(ModelRc::new(VecModel::from(live_sd_metadata_rows(
            &detail.mra_summary_rows,
        )))),
    );
    set(
        instance,
        "detail-mra-xml-rows",
        Value::Model(ModelRc::new(VecModel::from(live_sd_metadata_rows(
            &detail.mra_xml_rows,
        )))),
    );
    set(
        instance,
        "detail-mra-path-rows",
        Value::Model(ModelRc::new(VecModel::from(live_sd_metadata_rows(
            &detail.mra_path_rows,
        )))),
    );
    set(
        instance,
        "detail-mra-warnings",
        Value::Model(ModelRc::new(VecModel::from(live_sd_metadata_rows(
            &detail.mra_warnings,
        )))),
    );
    set(
        instance,
        "detail-raw-xml",
        Value::String(SharedString::from(detail.raw_xml.as_str())),
    );
    set(
        instance,
        "detail-raw-xml-truncated",
        Value::Bool(detail.raw_xml_truncated),
    );

    let rows = browser
        .rows()
        .iter()
        .map(|row| Value::Struct(live_tree_row_struct(row)))
        .collect::<Vec<_>>();
    set(
        instance,
        "rows",
        Value::Model(ModelRc::new(VecModel::from(rows))),
    );
}

#[cfg(feature = "live-ui")]
fn apply_live_library_state(
    instance: &slint_interpreter::ComponentInstance,
    browser: &SharedLibraryBrowser,
) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::Value;

    let Ok(browser) = browser.lock() else {
        return;
    };

    fn set(instance: &slint_interpreter::ComponentInstance, name: &str, value: Value) {
        let _ = instance.set_global_property("LibraryState", name, value);
    }

    set(instance, "updating_controls", Value::Bool(true));
    set(
        instance,
        "status",
        Value::String(SharedString::from(browser.status.as_str())),
    );
    set(
        instance,
        "warning",
        Value::String(SharedString::from(browser.warning.as_str())),
    );
    set(
        instance,
        "last-error",
        Value::String(SharedString::from(browser.last_error.as_str())),
    );
    set(instance, "loading", Value::Bool(browser.loading));
    set(
        instance,
        "query",
        Value::String(SharedString::from(browser.query.search.as_str())),
    );
    set(
        instance,
        "system-filter",
        Value::String(SharedString::from(browser.query.system.as_str())),
    );
    set(
        instance,
        "category-filter",
        Value::String(SharedString::from(browser.query.category.as_str())),
    );
    set(
        instance,
        "region-filter",
        Value::String(SharedString::from(browser.query.region.as_str())),
    );
    set(
        instance,
        "manufacturer-filter",
        Value::String(SharedString::from(browser.query.manufacturer.as_str())),
    );
    set(
        instance,
        "preview-filter",
        Value::String(SharedString::from(browser.query.preview.as_str())),
    );
    set(
        instance,
        "confidence-filter",
        Value::String(SharedString::from(browser.query.confidence.as_str())),
    );
    set(
        instance,
        "sort-column",
        Value::String(SharedString::from(library_sort_column_id(
            browser.query.sort_column,
        ))),
    );
    set(
        instance,
        "sort-direction",
        Value::EnumerationValue(
            "DataTableSortDirection".to_string(),
            match browser.query.sort_direction {
                library::LibrarySortDirection::Ascending => "ascending".to_string(),
                library::LibrarySortDirection::Descending => "descending".to_string(),
            },
        ),
    );
    set(
        instance,
        "result-summary",
        Value::String(SharedString::from(browser.result_summary().as_str())),
    );
    let view = browser.current_view().unwrap_or_default();
    set(instance, "page", Value::Number(view.page as f64));
    set(
        instance,
        "page-count",
        Value::Number(view.page_count as f64),
    );
    set(
        instance,
        "selected-game-id",
        Value::String(SharedString::from(browser.selected_game_id.as_str())),
    );
    if let Some(game) = browser.selected_game() {
        set(
            instance,
            "detail-title",
            Value::String(SharedString::from(game.title.as_str())),
        );
        set(
            instance,
            "detail-subtitle",
            Value::String(SharedString::from(
                format!("{} · {}", game.system_title, game.id).as_str(),
            )),
        );
    } else {
        set(
            instance,
            "detail-title",
            Value::String(SharedString::from("No game selected")),
        );
        set(
            instance,
            "detail-subtitle",
            Value::String(SharedString::from("Select a row to inspect catalog facts.")),
        );
    }
    let detail_sections = library_detail_sections(browser.selected_game());
    set_live_library_detail_rows(instance, "detail-overview-rows", &detail_sections.overview);
    set_live_library_detail_rows(instance, "detail-system-rows", &detail_sections.system);
    set_live_library_detail_rows(instance, "detail-media-rows", &detail_sections.media);
    set_live_library_detail_rows(instance, "detail-launch-rows", &detail_sections.launch);
    set_live_library_detail_rows(instance, "detail-identity-rows", &detail_sections.identity);
    set_live_library_detail_rows(instance, "detail-path-rows", &detail_sections.paths);
    set(
        instance,
        "rows",
        Value::Model(ModelRc::new(VecModel::from(
            view.rows
                .iter()
                .map(|game| live_library_row_struct(game, false))
                .collect::<Vec<_>>(),
        ))),
    );
    set(
        instance,
        "compact-rows",
        Value::Model(ModelRc::new(VecModel::from(
            view.rows
                .iter()
                .map(|game| live_library_row_struct(game, true))
                .collect::<Vec<_>>(),
        ))),
    );
    let (
        system_options,
        category_options,
        region_options,
        manufacturer_options,
        confidence_options,
    ) = match &browser.catalog {
        Some(catalog) => (
            library_select_options("All systems", &catalog.systems),
            library_select_options("All categories", &catalog.categories),
            library_select_options("All regions", &catalog.regions),
            library_select_options("All manufacturers", &catalog.manufacturers),
            library_select_options("All confidence", &catalog.confidences),
        ),
        None => (
            library_select_options("All systems", &[]),
            library_select_options("All categories", &[]),
            library_select_options("All regions", &[]),
            library_select_options("All manufacturers", &[]),
            library_select_options("All confidence", &[]),
        ),
    };
    let preview_options = library_preview_options();
    set_live_library_options(instance, "system-options", &system_options);
    set_live_library_options(instance, "category-options", &category_options);
    set_live_library_options(instance, "region-options", &region_options);
    set_live_library_options(instance, "manufacturer-options", &manufacturer_options);
    set_live_library_options(instance, "preview-options", &preview_options);
    set_live_library_options(instance, "confidence-options", &confidence_options);
    set(
        instance,
        "system-index",
        Value::Number(library_option_index(&system_options, &browser.query.system) as f64),
    );
    set(
        instance,
        "category-index",
        Value::Number(library_option_index(&category_options, &browser.query.category) as f64),
    );
    set(
        instance,
        "region-index",
        Value::Number(library_option_index(&region_options, &browser.query.region) as f64),
    );
    set(
        instance,
        "manufacturer-index",
        Value::Number(
            library_option_index(&manufacturer_options, &browser.query.manufacturer) as f64,
        ),
    );
    set(
        instance,
        "preview-index",
        Value::Number(library_option_index(&preview_options, &browser.query.preview) as f64),
    );
    set(
        instance,
        "confidence-index",
        Value::Number(library_option_index(&confidence_options, &browser.query.confidence) as f64),
    );
    set(instance, "updating_controls", Value::Bool(false));
}

#[cfg(feature = "live-ui")]
fn set_live_library_detail_rows(
    instance: &slint_interpreter::ComponentInstance,
    name: &str,
    rows: &[LibraryDetailRow],
) {
    use slint::{ModelRc, VecModel};
    use slint_interpreter::Value;

    let values = rows
        .iter()
        .enumerate()
        .map(|(index, row)| live_library_detail_row_struct(index, row))
        .collect::<Vec<_>>();
    let _ = instance.set_global_property(
        "LibraryState",
        name,
        Value::Model(ModelRc::new(VecModel::from(values))),
    );
}

#[cfg(feature = "live-ui")]
fn live_library_detail_row_struct(
    index: usize,
    row: &LibraryDetailRow,
) -> slint_interpreter::Value {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::{Struct, Value};

    Value::Struct(Struct::from_iter([
        (
            "id".to_string(),
            Value::String(SharedString::from(format!("detail-{index}").as_str())),
        ),
        (
            "cells".to_string(),
            Value::Model(ModelRc::new(VecModel::from(vec![
                live_library_text_cell(&row.field),
                live_library_text_cell(&row.value),
            ]))),
        ),
    ]))
}

#[cfg(feature = "live-ui")]
fn set_live_library_options(
    instance: &slint_interpreter::ComponentInstance,
    name: &str,
    options: &[LibrarySelectOptionItem],
) {
    use slint::{ModelRc, VecModel};
    use slint_interpreter::Value;

    let values = options
        .iter()
        .map(live_select_option_struct)
        .collect::<Vec<_>>();
    let _ = instance.set_global_property(
        "LibraryState",
        name,
        Value::Model(ModelRc::new(VecModel::from(values))),
    );
}

#[cfg(feature = "live-ui")]
fn live_select_option_struct(option: &LibrarySelectOptionItem) -> slint_interpreter::Value {
    use slint::SharedString;
    use slint_interpreter::{Struct, Value};

    Value::Struct(Struct::from_iter([
        (
            "value".to_string(),
            Value::String(SharedString::from(option.value.as_str())),
        ),
        (
            "label".to_string(),
            Value::String(SharedString::from(option.label.as_str())),
        ),
        ("enabled".to_string(), Value::Bool(option.enabled)),
    ]))
}

#[cfg(feature = "live-ui")]
fn live_library_row_struct(game: &library::LibraryGame, compact: bool) -> slint_interpreter::Value {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::{Struct, Value};

    let cells = if compact {
        vec![
            live_library_text_cell(&game.title),
            live_library_text_cell(&game.system_title),
            live_library_preview_cell(game.has_preview),
        ]
    } else {
        vec![
            live_library_text_cell(&game.title),
            live_library_text_cell(&game.system_title),
            live_library_text_cell(&game.year),
            live_library_text_cell(&game.manufacturer),
            live_library_text_cell(&game.category),
            live_library_preview_cell(game.has_preview),
            live_library_text_cell(&library_discovered_label(&game.discovered_at_unix)),
        ]
    };

    Value::Struct(Struct::from_iter([
        (
            "id".to_string(),
            Value::String(SharedString::from(game.id.as_str())),
        ),
        (
            "cells".to_string(),
            Value::Model(ModelRc::new(VecModel::from(cells))),
        ),
    ]))
}

#[cfg(feature = "live-ui")]
fn live_library_text_cell(text: &str) -> slint_interpreter::Value {
    live_library_cell("text", text, "default")
}

#[cfg(feature = "live-ui")]
fn live_library_preview_cell(has_preview: bool) -> slint_interpreter::Value {
    live_library_cell(
        "label",
        library_preview_label(has_preview),
        if has_preview { "success" } else { "secondary" },
    )
}

#[cfg(feature = "live-ui")]
fn live_library_cell(kind: &str, text: &str, label_variant: &str) -> slint_interpreter::Value {
    use slint::{Image, SharedString};
    use slint_interpreter::{Struct, Value};

    Value::Struct(Struct::from_iter([
        (
            "kind".to_string(),
            Value::EnumerationValue("DataTableCellKind".to_string(), kind.to_string()),
        ),
        (
            "text".to_string(),
            Value::String(SharedString::from(if text.is_empty() { "-" } else { text })),
        ),
        (
            "label-variant".to_string(),
            Value::EnumerationValue("LabelVariant".to_string(), label_variant.to_string()),
        ),
        (
            "label-size".to_string(),
            Value::EnumerationValue("LabelSize".to_string(), "small".to_string()),
        ),
        ("icon".to_string(), Value::Image(Image::default())),
        (
            "icon-tint".to_string(),
            Value::EnumerationValue("DataTableIconTint".to_string(), "default".to_string()),
        ),
    ]))
}

#[cfg(feature = "live-ui")]
fn live_tree_row_struct(row: &sd_card::SdTreeRow) -> slint_interpreter::Struct {
    use slint::{Image, SharedString};
    use slint_interpreter::{Struct, Value};

    Struct::from_iter([
        (
            "id".to_string(),
            Value::String(SharedString::from(row.id.as_str())),
        ),
        (
            "label".to_string(),
            Value::String(SharedString::from(row.label.as_str())),
        ),
        ("level".to_string(), Value::Number(f64::from(row.level))),
        ("has-children".to_string(), Value::Bool(row.has_children)),
        ("expanded".to_string(), Value::Bool(row.expanded)),
        ("current".to_string(), Value::Bool(row.current)),
        (
            "leading-is-directory".to_string(),
            Value::Bool(row.leading_is_directory),
        ),
        ("has-leading-visual".to_string(), Value::Bool(true)),
        ("preserve-leading-icon-color".to_string(), Value::Bool(true)),
        (
            "trailing".to_string(),
            Value::EnumerationValue("TreeViewTrailingVisual".to_string(), "none".to_string()),
        ),
        ("has-leading-action".to_string(), Value::Bool(false)),
        ("show-leading-action-icon".to_string(), Value::Bool(false)),
        (
            "leading-action-icon".to_string(),
            Value::Image(Image::default()),
        ),
        (
            "leading-file-icon".to_string(),
            Value::Image(file_icons::material_icon(row.icon_key.as_str())),
        ),
        ("interactive".to_string(), Value::Bool(row.interactive)),
        ("is-skeleton".to_string(), Value::Bool(row.is_skeleton)),
        ("has-secondary-actions".to_string(), Value::Bool(false)),
        (
            "secondary-actions-badge".to_string(),
            Value::String(SharedString::from("")),
        ),
        (
            "loading-children-badge".to_string(),
            Value::String(SharedString::from(row.loading_children_badge.as_str())),
        ),
    ])
}

#[cfg(feature = "live-ui")]
fn live_sd_metadata_rows(rows: &[sd_card::SdMetadataRow]) -> Vec<slint_interpreter::Value> {
    use slint::SharedString;
    use slint_interpreter::{Struct, Value};

    rows.iter()
        .map(|row| {
            Value::Struct(Struct::from_iter([
                (
                    "label".to_string(),
                    Value::String(SharedString::from(row.label.as_str())),
                ),
                (
                    "value".to_string(),
                    Value::String(SharedString::from(row.value.as_str())),
                ),
                (
                    "kind".to_string(),
                    Value::String(SharedString::from(row.kind.as_str())),
                ),
            ]))
        })
        .collect()
}

#[cfg(feature = "compiled-ui")]
fn run_compiled_ui(
    frame_chart_fixture: Option<RealtimeFrameChartFixture>,
) -> Result<(), Box<dyn Error>> {
    use slint::ComponentHandle;

    let host = std::env::var("MISTER_IP").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let ui = AppWindow::new()?;
    let sd_browser = Arc::new(Mutex::new(SdCardBrowser::new()));
    let library_browser = Arc::new(Mutex::new(LibraryBrowser::new()));
    let framebuffer_capture = Arc::new(Mutex::new(None));
    let framebuffer_render_metrics = Arc::new(FramebufferRenderMetrics::default());
    install_framebuffer_render_notifier(ui.window(), Arc::clone(&framebuffer_render_metrics));
    let live_stream_generation = Arc::new(AtomicU64::new(0));
    let live_stream_control = Arc::new(Mutex::new(None));
    let live_display_clock = Rc::new(RefCell::new(None::<FramebufferDisplayController>));
    let realtime_stream_generation = Arc::new(AtomicU64::new(0));
    let realtime_stream_control = Arc::new(Mutex::new(None));
    let realtime_frame_chart = Arc::new(Mutex::new(FrameChartState::default()));
    let realtime_debug_page_active = Arc::new(AtomicBool::new(true));
    let realtime_debug_tab_index = Arc::new(AtomicI32::new(0));
    let refresh_ui = ui.as_weak();
    let refresh_host = host.clone();
    ui.global::<Actions>().on_refresh_status(move || {
        if let Some(ui) = refresh_ui.upgrade() {
            let snapshot = fetch_dashboard(&refresh_host);
            apply_compiled_snapshot(&ui, &snapshot);
        }
    });

    let select_ui = ui.as_weak();
    let select_realtime_generation = Arc::clone(&realtime_stream_generation);
    let select_realtime_control = Arc::clone(&realtime_stream_control);
    let select_frame_chart = Arc::clone(&realtime_frame_chart);
    let select_realtime_page_active = Arc::clone(&realtime_debug_page_active);
    let select_realtime_tab_index = Arc::clone(&realtime_debug_tab_index);
    let select_realtime_host = host.clone();
    ui.global::<Actions>().on_select_page(move |page| {
        if let Some(ui) = select_ui.upgrade() {
            let debug_active = page.as_str() == "debug";
            select_realtime_page_active.store(debug_active, Ordering::SeqCst);
            ui.global::<AppState>().set_selected_page(page);
            start_or_stop_compiled_realtime(
                select_ui.clone(),
                Arc::clone(&select_realtime_generation),
                Arc::clone(&select_realtime_control),
                Arc::clone(&select_frame_chart),
                frame_chart_fixture,
                select_realtime_host.clone(),
                debug_active && select_realtime_tab_index.load(Ordering::SeqCst) == 1,
            );
        }
    });

    let debug_tab_ui = ui.as_weak();
    let debug_tab_host = host.clone();
    let debug_tab_generation = Arc::clone(&realtime_stream_generation);
    let debug_tab_control = Arc::clone(&realtime_stream_control);
    let debug_tab_frame_chart = Arc::clone(&realtime_frame_chart);
    let debug_tab_page_active = Arc::clone(&realtime_debug_page_active);
    let debug_tab_index_state = Arc::clone(&realtime_debug_tab_index);
    ui.global::<Actions>().on_debug_tab_changed(move |index| {
        debug_tab_index_state.store(index, Ordering::SeqCst);
        start_or_stop_compiled_realtime(
            debug_tab_ui.clone(),
            Arc::clone(&debug_tab_generation),
            Arc::clone(&debug_tab_control),
            Arc::clone(&debug_tab_frame_chart),
            frame_chart_fixture,
            debug_tab_host.clone(),
            debug_tab_page_active.load(Ordering::SeqCst) && index == 1,
        );
    });

    let realtime_ui = ui.as_weak();
    let realtime_host = host.clone();
    let realtime_generation = Arc::clone(&realtime_stream_generation);
    let realtime_control = Arc::clone(&realtime_stream_control);
    let realtime_frame_chart_for_stream = Arc::clone(&realtime_frame_chart);
    ui.global::<Actions>()
        .on_realtime_stream_changed(move |active| {
            start_or_stop_compiled_realtime(
                realtime_ui.clone(),
                Arc::clone(&realtime_generation),
                Arc::clone(&realtime_control),
                Arc::clone(&realtime_frame_chart_for_stream),
                frame_chart_fixture,
                realtime_host.clone(),
                active,
            );
        });

    let resize_ui = ui.as_weak();
    let resize_frame_chart_state = Arc::clone(&realtime_frame_chart);
    ui.global::<Actions>()
        .on_realtime_frame_chart_resized(move |width, height| {
            if let (Some(ui), Some(rendered)) = (
                resize_ui.upgrade(),
                resize_frame_chart(&resize_frame_chart_state, width, height),
            ) {
                apply_compiled_frame_chart(&ui, rendered);
            }
        });

    let capture_ui = ui.as_weak();
    let capture_host = host.clone();
    let capture_state = Arc::clone(&framebuffer_capture);
    ui.global::<Actions>().on_capture_framebuffer(move || {
        if let Some(ui) = capture_ui.upgrade() {
            set_compiled_analytics_loading(&ui);
        }
        spawn_compiled_framebuffer_capture(
            capture_ui.clone(),
            Arc::clone(&capture_state),
            capture_host.clone(),
        );
    });

    let save_ui = ui.as_weak();
    let save_capture = Arc::clone(&framebuffer_capture);
    ui.global::<Actions>().on_save_framebuffer_image(move || {
        if let Some(ui) = save_ui.upgrade() {
            apply_compiled_save_status(&ui, "Saving framebuffer PNG...", "");
        }
        spawn_compiled_save_framebuffer_capture(save_ui.clone(), Arc::clone(&save_capture));
    });

    let stream_ui = ui.as_weak();
    let stream_host = host.clone();
    let stream_capture = Arc::clone(&framebuffer_capture);
    let stream_generation = Arc::clone(&live_stream_generation);
    let stream_control = Arc::clone(&live_stream_control);
    let stream_render_metrics = Arc::clone(&framebuffer_render_metrics);
    let stream_display_clock = Rc::clone(&live_display_clock);
    ui.global::<Actions>()
        .on_live_stream_changed(move |enabled| {
            let generation = stream_generation.fetch_add(1, Ordering::SeqCst) + 1;
            cancel_framebuffer_stream(&stream_control);
            stream_display_clock.borrow_mut().take();
            if let Some(ui) = stream_ui.upgrade() {
                apply_compiled_stream_summary(
                    &ui,
                    if enabled {
                        "Live stream starting..."
                    } else {
                        "Live stream off."
                    },
                );
            }
            if enabled {
                let mailbox = Arc::new(LatestMailbox::default());
                let display_state = Arc::new(Mutex::new(FramebufferDisplayState::default()));
                let stream_start = Instant::now();
                *stream_display_clock.borrow_mut() = start_compiled_framebuffer_display_clock(
                    stream_ui.clone(),
                    Arc::clone(&stream_capture),
                    Arc::clone(&stream_generation),
                    Arc::clone(&mailbox),
                    display_state,
                    Arc::clone(&stream_render_metrics),
                    stream_start,
                    generation,
                );
                spawn_compiled_framebuffer_stream(
                    stream_ui.clone(),
                    Arc::clone(&stream_capture),
                    Arc::clone(&stream_generation),
                    Arc::clone(&stream_control),
                    Arc::clone(&stream_render_metrics),
                    mailbox,
                    stream_host.clone(),
                    generation,
                );
            }
        });

    let profile_ui = ui.as_weak();
    ui.global::<Actions>()
        .on_load_profile_artifact(move |path| {
            if let Some(ui) = profile_ui.upgrade() {
                set_compiled_profile_loading(&ui, path.as_str());
            }
            spawn_compiled_profile_load(profile_ui.clone(), path.to_string());
        });

    let sd_toggle_ui = ui.as_weak();
    let sd_toggle_host = host.clone();
    let sd_toggle_browser = Arc::clone(&sd_browser);
    ui.global::<Actions>().on_sd_row_toggle(move |path| {
        if let Some(fetch_path) = sd_toggle_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.toggle_directory(path.as_str()))
        {
            let show_hidden = sd_toggle_browser
                .lock()
                .map(|browser| browser.show_hidden())
                .unwrap_or(false);
            spawn_compiled_sd_fetch(
                sd_toggle_ui.clone(),
                Arc::clone(&sd_toggle_browser),
                sd_toggle_host.clone(),
                fetch_path,
                show_hidden,
            );
        }
        if let Some(ui) = sd_toggle_ui.upgrade() {
            apply_compiled_sd_state(&ui, &sd_toggle_browser);
        }
    });

    let sd_current_ui = ui.as_weak();
    let sd_current_host = host.clone();
    let sd_current_browser = Arc::clone(&sd_browser);
    ui.global::<Actions>().on_sd_row_current(move |path| {
        let detail_request = if let Ok(mut browser) = sd_current_browser.lock() {
            browser.select_path(path.as_str());
            browser.begin_detail_fetch_current(false)
        } else {
            None
        };
        if let Some(ui) = sd_current_ui.upgrade() {
            apply_compiled_sd_state(&ui, &sd_current_browser);
        }
        if let Some(detail_request) = detail_request {
            spawn_compiled_sd_detail_fetch(
                sd_current_ui.clone(),
                Arc::clone(&sd_current_browser),
                sd_current_host.clone(),
                detail_request,
            );
        }
    });

    let sd_refresh_ui = ui.as_weak();
    let sd_refresh_host = host.clone();
    let sd_refresh_browser = Arc::clone(&sd_browser);
    ui.global::<Actions>().on_sd_refresh_folder(move || {
        if let Some(fetch_path) = sd_refresh_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.refresh_current_folder())
        {
            let show_hidden = sd_refresh_browser
                .lock()
                .map(|browser| browser.show_hidden())
                .unwrap_or(false);
            spawn_compiled_sd_fetch(
                sd_refresh_ui.clone(),
                Arc::clone(&sd_refresh_browser),
                sd_refresh_host.clone(),
                fetch_path,
                show_hidden,
            );
        }
        if let Some(ui) = sd_refresh_ui.upgrade() {
            apply_compiled_sd_state(&ui, &sd_refresh_browser);
        }
    });

    let sd_detail_refresh_ui = ui.as_weak();
    let sd_detail_refresh_host = host.clone();
    let sd_detail_refresh_browser = Arc::clone(&sd_browser);
    ui.global::<Actions>().on_sd_refresh_details(move || {
        let detail_request = sd_detail_refresh_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.begin_detail_fetch_current(true));
        if let Some(ui) = sd_detail_refresh_ui.upgrade() {
            apply_compiled_sd_state(&ui, &sd_detail_refresh_browser);
        }
        if let Some(detail_request) = detail_request {
            spawn_compiled_sd_detail_fetch(
                sd_detail_refresh_ui.clone(),
                Arc::clone(&sd_detail_refresh_browser),
                sd_detail_refresh_host.clone(),
                detail_request,
            );
        }
    });

    let sd_hidden_ui = ui.as_weak();
    let sd_hidden_host = host.clone();
    let sd_hidden_browser = Arc::clone(&sd_browser);
    ui.global::<Actions>()
        .on_sd_show_hidden_changed(move |show_hidden| {
            if let Some(fetch_path) = sd_hidden_browser
                .lock()
                .ok()
                .and_then(|mut browser| browser.set_show_hidden(show_hidden))
            {
                spawn_compiled_sd_fetch(
                    sd_hidden_ui.clone(),
                    Arc::clone(&sd_hidden_browser),
                    sd_hidden_host.clone(),
                    fetch_path,
                    show_hidden,
                );
            }
            if let Some(ui) = sd_hidden_ui.upgrade() {
                apply_compiled_sd_state(&ui, &sd_hidden_browser);
            }
        });

    let library_sync_ui = ui.as_weak();
    let library_sync_host = host.clone();
    let library_sync_browser = Arc::clone(&library_browser);
    ui.global::<Actions>().on_sync_library(move || {
        if let Ok(mut browser) = library_sync_browser.lock() {
            if browser.loading {
                return;
            }
            browser.start_sync();
        }
        if let Some(ui) = library_sync_ui.upgrade() {
            apply_compiled_library_state(&ui, &library_sync_browser);
        }
        spawn_compiled_library_sync(
            library_sync_ui.clone(),
            Arc::clone(&library_sync_browser),
            library_sync_host.clone(),
        );
    });
    let library_query_ui = ui.as_weak();
    let library_query_browser = Arc::clone(&library_browser);
    ui.global::<Actions>()
        .on_library_query_changed(move |query| {
            if let Ok(mut browser) = library_query_browser.lock() {
                browser.set_query(query.as_str());
            }
            if let Some(ui) = library_query_ui.upgrade() {
                apply_compiled_library_state(&ui, &library_query_browser);
            }
        });
    let library_filter_ui = ui.as_weak();
    let library_filter_browser = Arc::clone(&library_browser);
    ui.global::<Actions>()
        .on_library_filter_changed(move |filter, value| {
            if let Ok(mut browser) = library_filter_browser.lock() {
                browser.set_filter(filter.as_str(), value.as_str());
            }
            if let Some(ui) = library_filter_ui.upgrade() {
                apply_compiled_library_state(&ui, &library_filter_browser);
            }
        });
    let library_sort_ui = ui.as_weak();
    let library_sort_browser = Arc::clone(&library_browser);
    ui.global::<Actions>()
        .on_library_sort_toggled(move |column, direction| {
            let direction_id = if direction == DataTableSortDirection::Descending {
                "descending"
            } else {
                "ascending"
            };
            if let Ok(mut browser) = library_sort_browser.lock() {
                browser.set_sort(column.as_str(), direction_id);
            }
            if let Some(ui) = library_sort_ui.upgrade() {
                apply_compiled_library_state(&ui, &library_sort_browser);
            }
        });
    let library_page_ui = ui.as_weak();
    let library_page_browser = Arc::clone(&library_browser);
    ui.global::<Actions>().on_library_page_changed(move |page| {
        if let Ok(mut browser) = library_page_browser.lock() {
            browser.set_page(page);
        }
        if let Some(ui) = library_page_ui.upgrade() {
            apply_compiled_library_state(&ui, &library_page_browser);
        }
    });
    let library_row_ui = ui.as_weak();
    let library_row_browser = Arc::clone(&library_browser);
    ui.global::<Actions>().on_library_row_selected(move |id| {
        if let Ok(mut browser) = library_row_browser.lock() {
            browser.select_row(id.as_str());
        }
        if let Some(ui) = library_row_ui.upgrade() {
            apply_compiled_library_state(&ui, &library_row_browser);
        }
    });

    let drag_ui = ui.as_weak();
    ui.global::<WindowActions>().on_start_window_drag(move || {
        if let Some(ui) = drag_ui.upgrade() {
            start_window_drag(ui.window());
        }
    });

    let snapshot = if frame_chart_fixture.is_some() {
        DashboardSnapshot::initial(&host)
    } else {
        fetch_dashboard(&host)
    };
    apply_compiled_snapshot(&ui, &snapshot);
    apply_compiled_sd_state(&ui, &sd_browser);
    apply_compiled_library_state(&ui, &library_browser);
    if let Some(fixture) = frame_chart_fixture {
        ui.global::<AppState>().set_debug_tab_index(1);
        apply_compiled_realtime_view(
            &ui,
            &realtime_frame_chart_fixture_view(fixture),
            &realtime_frame_chart,
        );
    }
    #[cfg(target_os = "macos")]
    setup_macos_titlebar_for_compiled_ui(&ui);
    ui.run()?;
    Ok(())
}

#[cfg(target_os = "macos")]
#[cfg(feature = "live-ui")]
fn setup_macos_titlebar_for_live_instance(instance: &slint_interpreter::ComponentInstance) {
    let instance_weak = instance.as_weak();
    slint::spawn_local(async move {
        let Some(instance) = instance_weak.upgrade() else {
            return;
        };
        let _ = macos_titlebar::setup_window(instance.window()).await;
    })
    .ok();
}

#[cfg(target_os = "macos")]
#[cfg(feature = "compiled-ui")]
fn setup_macos_titlebar_for_compiled_ui(ui: &AppWindow) {
    let ui_weak = ui.as_weak();
    slint::spawn_local(async move {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let _ = macos_titlebar::setup_window(ui.window()).await;
    })
    .ok();
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_snapshot(ui: &AppWindow, snapshot: &DashboardSnapshot) {
    let state = ui.global::<DeviceState>();
    state.set_host(snapshot.host.as_str().into());
    state.set_connection_state(snapshot.connection_state.as_str().into());
    state.set_agent_status(snapshot.agent_status.as_str().into());
    state.set_token_source(snapshot.token_source.as_str().into());
    state.set_agent_version(snapshot.agent_version.as_str().into());
    state.set_agent_uptime(snapshot.agent_uptime.as_str().into());
    state.set_network_summary(snapshot.network_summary.as_str().into());
    state.set_mac_address(snapshot.mac_address.as_str().into());
    state.set_main_process(snapshot.main_process.as_str().into());
    state.set_launcher_process(snapshot.launcher_process.as_str().into());
    state.set_launcher_state(snapshot.launcher_state.as_str().into());
    state.set_visible_owner(snapshot.visible_owner.as_str().into());
    state.set_slint_status_freshness(snapshot.slint_status_freshness.as_str().into());
    state.set_scanout_slots_summary(snapshot.scanout_slots_summary.as_str().into());
    state.set_scanout_slots_detail(snapshot.scanout_slots_detail.as_str().into());
    state.set_catalog_summary(snapshot.catalog_summary.as_str().into());
    state.set_screen_summary(snapshot.screen_summary.as_str().into());
    state.set_input_summary(snapshot.input_summary.as_str().into());
    state.set_last_error(snapshot.last_error.as_str().into());
}

#[cfg(feature = "live-ui")]
fn set_live_analytics_loading(instance: &slint_interpreter::ComponentInstance) {
    use slint_interpreter::Value;
    let state = analytics_ui_state::loading_capture_state();

    let _ = instance.set_global_property("AnalyticsState", "loading", Value::Bool(state.loading));
    let _ = instance.set_global_property(
        "AnalyticsState",
        "status",
        Value::String(state.status.into()),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "last-error",
        Value::String(state.last_error.into()),
    );
}

#[cfg(feature = "live-ui")]
fn apply_live_framebuffer_stream_capture(
    instance: &slint_interpreter::ComponentInstance,
    capture: &agent_client::FramebufferCapture,
    pixels: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    geometry_changed: bool,
) {
    use slint_interpreter::Value;

    let image = pixels.map(slint::Image::from_rgba8).unwrap_or_default();
    let _ =
        instance.set_global_property("AnalyticsState", "framebuffer-image", Value::Image(image));
    if let Some(state) = analytics_ui_state::stream_capture_state(capture, geometry_changed) {
        let _ =
            instance.set_global_property("AnalyticsState", "loading", Value::Bool(state.loading));
        let _ = instance.set_global_property(
            "AnalyticsState",
            "has-image",
            Value::Bool(state.has_image),
        );
        let _ = instance.set_global_property(
            "AnalyticsState",
            "can-save-image",
            Value::Bool(state.can_save_image),
        );
        let _ = instance.set_global_property(
            "AnalyticsState",
            "framebuffer-width",
            Value::Number(state.width as f64),
        );
        let _ = instance.set_global_property(
            "AnalyticsState",
            "framebuffer-height",
            Value::Number(state.height as f64),
        );
    }
}

#[cfg(feature = "live-ui")]
fn apply_live_framebuffer_capture_result(
    instance: &slint_interpreter::ComponentInstance,
    result: Result<agent_client::FramebufferCapture, String>,
) {
    use slint_interpreter::Value;

    let state = analytics_ui_state::capture_result_state(result.as_ref().map_err(String::as_str));
    let _ = instance.set_global_property("AnalyticsState", "loading", Value::Bool(state.loading));
    match result {
        Ok(capture) => {
            let image = framebuffer_capture_image(&capture);
            let _ = instance.set_global_property(
                "AnalyticsState",
                "framebuffer-image",
                Value::Image(image),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "has-image",
                Value::Bool(state.has_image),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "can-save-image",
                Value::Bool(state.can_save_image),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "framebuffer-width",
                Value::Number(state.width as f64),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "framebuffer-height",
                Value::Number(state.height as f64),
            );
            clear_live_dirty_rects(instance);
            let _ = instance.set_global_property(
                "AnalyticsState",
                "status",
                Value::String(state.status.clone().into()),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "last-error",
                Value::String(state.last_error.clone().into()),
            );
        }
        Err(_) => {
            let _ = instance.set_global_property(
                "AnalyticsState",
                "status",
                Value::String(state.status.into()),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "last-error",
                Value::String(state.last_error.into()),
            );
        }
    }
}

#[cfg(feature = "compiled-ui")]
fn set_compiled_analytics_loading(ui: &AppWindow) {
    let next = analytics_ui_state::loading_capture_state();
    let state = ui.global::<AnalyticsState>();
    state.set_loading(next.loading);
    state.set_status(next.status.into());
    state.set_last_error(next.last_error.into());
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_framebuffer_stream_capture(
    ui: &AppWindow,
    capture: &agent_client::FramebufferCapture,
    pixels: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    geometry_changed: bool,
) {
    let state = ui.global::<AnalyticsState>();
    state.set_framebuffer_image(pixels.map(slint::Image::from_rgba8).unwrap_or_default());
    if let Some(next) = analytics_ui_state::stream_capture_state(capture, geometry_changed) {
        state.set_loading(next.loading);
        state.set_has_image(next.has_image);
        state.set_can_save_image(next.can_save_image);
        state.set_framebuffer_width(next.width as i32);
        state.set_framebuffer_height(next.height as i32);
    }
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_framebuffer_capture_result(
    ui: &AppWindow,
    result: Result<agent_client::FramebufferCapture, String>,
) {
    let next = analytics_ui_state::capture_result_state(result.as_ref().map_err(String::as_str));
    let state = ui.global::<AnalyticsState>();
    state.set_loading(next.loading);
    match result {
        Ok(capture) => {
            state.set_framebuffer_image(framebuffer_capture_image(&capture));
            state.set_has_image(next.has_image);
            state.set_can_save_image(next.can_save_image);
            state.set_framebuffer_width(next.width as i32);
            state.set_framebuffer_height(next.height as i32);
            clear_compiled_dirty_rects(ui);
            state.set_status(next.status.into());
            state.set_last_error(next.last_error.into());
        }
        Err(_) => {
            state.set_status(next.status.into());
            state.set_last_error(next.last_error.into());
        }
    }
}

#[cfg(test)]
fn framebuffer_capture_status(capture: &agent_client::FramebufferCapture) -> String {
    format!(
        "Captured {}x{} {}bpp framebuffer ({} payload; {} raw; {}).",
        capture.width,
        capture.height,
        capture.bpp,
        format_byte_size(capture.payload_bytes),
        format_byte_size(capture.raw_bytes),
        capture.encoding
    )
}

fn framebuffer_capture_image(capture: &agent_client::FramebufferCapture) -> slint::Image {
    if capture.rgba_pixels.is_empty() && !capture.png_path.as_os_str().is_empty() {
        return slint::Image::load_from_path(&capture.png_path).unwrap_or_default();
    }
    framebuffer_capture_pixel_buffer(capture)
        .map(slint::Image::from_rgba8)
        .unwrap_or_default()
}

fn framebuffer_capture_pixel_buffer(
    capture: &agent_client::FramebufferCapture,
) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let width = u32::try_from(capture.width).ok()?;
    let height = u32::try_from(capture.height).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    if capture.rgba_pixels.len()
        != (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4)
    {
        return None;
    }
    Some(
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &capture.rgba_pixels,
            width,
            height,
        ),
    )
}

fn framebuffer_capture_png_bytes(
    capture: &agent_client::FramebufferCapture,
) -> Result<Vec<u8>, String> {
    if capture.rgba_pixels.is_empty() {
        if !capture.png_path.as_os_str().is_empty() {
            return std::fs::read(&capture.png_path).map_err(|err| {
                format!("read framebuffer PNG {}: {err}", capture.png_path.display())
            });
        }
        return Err("No framebuffer image is available to save.".to_string());
    }

    let width = u32::try_from(capture.width).map_err(|_| "framebuffer width too large")?;
    let height = u32::try_from(capture.height).map_err(|_| "framebuffer height too large")?;
    if width == 0 || height == 0 {
        return Err("No framebuffer image is available to save.".to_string());
    }
    let expected_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "framebuffer image dimensions are too large".to_string())?;
    if capture.rgba_pixels.len() != expected_len {
        return Err(format!(
            "framebuffer RGBA size mismatch expected={expected_len} actual={}",
            capture.rgba_pixels.len()
        ));
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|err| format!("write PNG header: {err}"))?;
        writer
            .write_image_data(&capture.rgba_pixels)
            .map_err(|err| format!("write PNG pixels: {err}"))?;
        writer
            .finish()
            .map_err(|err| format!("finish PNG: {err}"))?;
    }
    Ok(png_bytes)
}

fn save_framebuffer_capture_png(
    capture: &agent_client::FramebufferCapture,
) -> Result<PathBuf, String> {
    let png_bytes = framebuffer_capture_png_bytes(capture)?;
    let desktop = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Desktop"))
        .ok_or_else(|| "HOME is not set; cannot find the Desktop folder.".to_string())?;
    if !desktop.is_dir() {
        return Err(format!(
            "Desktop folder does not exist: {}",
            desktop.display()
        ));
    }
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|err| format!("system clock before Unix epoch: {err}"))?
        .as_millis();
    let path = desktop.join(format!("mister-magik-framebuffer-{millis}.png"));
    std::fs::write(&path, png_bytes)
        .map_err(|err| format!("write framebuffer PNG {}: {err}", path.display()))?;
    Ok(path)
}

#[cfg(feature = "live-ui")]
fn apply_live_save_status(
    instance: &slint_interpreter::ComponentInstance,
    status: &str,
    last_error: &str,
) {
    use slint::SharedString;
    use slint_interpreter::Value;

    let _ = instance.set_global_property(
        "AnalyticsState",
        "status",
        Value::String(SharedString::from(status)),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "last-error",
        Value::String(SharedString::from(last_error)),
    );
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_save_status(ui: &AppWindow, status: &str, last_error: &str) {
    let state = ui.global::<AnalyticsState>();
    state.set_status(status.into());
    state.set_last_error(last_error.into());
}

#[cfg(feature = "live-ui")]
fn apply_live_stream_summary(instance: &slint_interpreter::ComponentInstance, summary: &str) {
    use slint::SharedString;
    use slint_interpreter::Value;

    let _ = instance.set_global_property(
        "AnalyticsState",
        "live-stream-summary",
        Value::String(SharedString::from(summary)),
    );
}

#[cfg(feature = "live-ui")]
fn apply_live_stream_disconnected(instance: &slint_interpreter::ComponentInstance, err: &str) {
    use slint::SharedString;
    use slint_interpreter::Value;

    let _ = instance.set_global_property("AnalyticsState", "live-stream", Value::Bool(false));
    let _ = instance.set_global_property(
        "AnalyticsState",
        "live-stream-summary",
        Value::String(SharedString::from("Live stream disconnected.")),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "last-error",
        Value::String(SharedString::from(err)),
    );
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_stream_summary(ui: &AppWindow, summary: &str) {
    ui.global::<AnalyticsState>()
        .set_live_stream_summary(summary.into());
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_stream_disconnected(ui: &AppWindow, err: &str) {
    let state = ui.global::<AnalyticsState>();
    state.set_live_stream(false);
    state.set_live_stream_summary("Live stream disconnected.".into());
    state.set_last_error(err.into());
}

fn cancel_framebuffer_stream(stream_control: &SharedFramebufferStreamControl) {
    stream_lifecycle::cancel(stream_control, FramebufferStreamControl::shutdown);
}

fn register_framebuffer_stream(
    stream_control: &SharedFramebufferStreamControl,
    generation: u64,
    control: FramebufferStreamControl,
) {
    stream_lifecycle::replace(
        stream_control,
        generation,
        control,
        FramebufferStreamControl::shutdown,
    );
}

fn unregister_framebuffer_stream(stream_control: &SharedFramebufferStreamControl, generation: u64) {
    stream_lifecycle::unregister(stream_control, generation);
}

fn cancel_realtime_stream(stream_control: &SharedRealtimeStreamControl) {
    stream_lifecycle::cancel(stream_control, DeviceTelemetryStreamControl::shutdown);
}

fn register_realtime_stream(
    stream_control: &SharedRealtimeStreamControl,
    generation: u64,
    control: DeviceTelemetryStreamControl,
) {
    stream_lifecycle::replace(
        stream_control,
        generation,
        control,
        DeviceTelemetryStreamControl::shutdown,
    );
}

fn unregister_realtime_stream(stream_control: &SharedRealtimeStreamControl, generation: u64) {
    stream_lifecycle::unregister(stream_control, generation);
}

#[cfg(feature = "live-ui")]
fn start_or_stop_live_realtime(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    stream_generation: SharedRealtimeStreamGeneration,
    stream_control: SharedRealtimeStreamControl,
    frame_chart: SharedRealtimeFrameChart,
    fixture: Option<RealtimeFrameChartFixture>,
    host: String,
    active: bool,
) {
    let generation = stream_generation.fetch_add(1, Ordering::SeqCst) + 1;
    cancel_realtime_stream(&stream_control);
    if let Some(fixture) = fixture {
        if let Some(instance) = instance.upgrade() {
            if active {
                apply_live_realtime_view(
                    &instance,
                    &realtime_frame_chart_fixture_view(fixture),
                    &frame_chart,
                );
            } else {
                apply_live_realtime_off(&instance, &frame_chart);
            }
        }
        return;
    }
    if !active {
        if let Some(instance) = instance.upgrade() {
            apply_live_realtime_off(&instance, &frame_chart);
        }
        return;
    }
    if let Some(instance) = instance.upgrade() {
        apply_live_realtime_view(
            &instance,
            &realtime_view_from_history(&RealtimeHistory::default(), true, ""),
            &frame_chart,
        );
    }
    spawn_live_realtime_stream(
        instance,
        stream_generation,
        stream_control,
        frame_chart,
        host,
        generation,
    );
}

#[cfg(feature = "compiled-ui")]
fn start_or_stop_compiled_realtime(
    ui: slint::Weak<AppWindow>,
    stream_generation: SharedRealtimeStreamGeneration,
    stream_control: SharedRealtimeStreamControl,
    frame_chart: SharedRealtimeFrameChart,
    fixture: Option<RealtimeFrameChartFixture>,
    host: String,
    active: bool,
) {
    let generation = stream_generation.fetch_add(1, Ordering::SeqCst) + 1;
    cancel_realtime_stream(&stream_control);
    if let Some(fixture) = fixture {
        if let Some(ui) = ui.upgrade() {
            if active {
                apply_compiled_realtime_view(
                    &ui,
                    &realtime_frame_chart_fixture_view(fixture),
                    &frame_chart,
                );
            } else {
                apply_compiled_realtime_off(&ui, &frame_chart);
            }
        }
        return;
    }
    if !active {
        if let Some(ui) = ui.upgrade() {
            apply_compiled_realtime_off(&ui, &frame_chart);
        }
        return;
    }
    if let Some(ui) = ui.upgrade() {
        apply_compiled_realtime_view(
            &ui,
            &realtime_view_from_history(&RealtimeHistory::default(), true, ""),
            &frame_chart,
        );
    }
    spawn_compiled_realtime_stream(
        ui,
        stream_generation,
        stream_control,
        frame_chart,
        host,
        generation,
    );
}

fn record_applied_frame(
    state: &mut FramebufferDisplayState,
    frame: &agent_client::FramebufferStreamFrame,
) -> bool {
    let geometry_changed = state.geometry != Some(frame.geometry);
    if geometry_changed {
        state.recent_dirty_rects.clear();
        state.geometry = Some(frame.geometry);
    }
    push_recent_dirty_rect(&mut state.recent_dirty_rects, frame);
    state.applied = state.applied.saturating_add(1);
    geometry_changed
}

fn framebuffer_display_summary(
    mailbox: &LatestMailbox<FramebufferDisplayUpdate>,
    state: &FramebufferDisplayState,
    render_metrics: &FramebufferRenderMetrics,
    stream_start: Instant,
    received_at: Instant,
) -> String {
    let elapsed = stream_start.elapsed().as_secs_f64().max(f64::EPSILON);
    let (received, coalesced) = mailbox.stats();
    let receive_fps = received as f64 / elapsed;
    let applied_fps = state.applied as f64 / elapsed;
    let geometry = state
        .geometry
        .map(|value| format!("{}x{}", value.width, value.height))
        .unwrap_or_else(|| "unknown".to_string());
    let rendered = render_metrics.snapshot(Instant::now());
    let rendered_label = if rendered.supported {
        format!(
            "rendered {:.1} fps · render p95 {:.0} ms",
            rendered.fps, rendered.latency_p95_ms
        )
    } else {
        "rendered n/a".to_string()
    };
    format!(
        "rx {receive_fps:.1} fps · applied {applied_fps:.1} fps · {rendered_label} · {geometry} · coalesced {coalesced} · queue {:.0} ms",
        received_at.elapsed().as_secs_f64() * 1000.0
    )
}

fn dirty_rect_from_stream_frame(
    frame: &agent_client::FramebufferStreamFrame,
) -> DirtyRectOverlayState {
    DirtyRectOverlayState {
        x: frame.rect.x.min(i32::MAX as u32) as i32,
        y: frame.rect.y.min(i32::MAX as u32) as i32,
        width: frame.rect.width.min(i32::MAX as u32) as i32,
        height: frame.rect.height.min(i32::MAX as u32) as i32,
        kind: match frame.kind {
            mister_magik_framebuffer_stream::FrameKind::Keyframe => "keyframe",
            mister_magik_framebuffer_stream::FrameKind::RectDelta => "delta",
            _ => "frame",
        }
        .to_string(),
    }
}

fn push_recent_dirty_rect(
    recent: &mut VecDeque<DirtyRectOverlayState>,
    frame: &agent_client::FramebufferStreamFrame,
) {
    recent.push_back(dirty_rect_from_stream_frame(frame));
    while recent.len() > DIRTY_RECT_LINGER_FRAMES || recent.len() > MAX_DIRTY_RECT_OVERLAYS {
        recent.pop_front();
    }
}

fn dirty_rect_summary(
    frame: &agent_client::FramebufferStreamFrame,
    visible_count: usize,
) -> String {
    let kind = match frame.kind {
        mister_magik_framebuffer_stream::FrameKind::Keyframe => "keyframe",
        mister_magik_framebuffer_stream::FrameKind::RectDelta => "delta",
        _ => "frame",
    };
    format!(
        "{kind} #{} rect {}x{}+{},{}; showing {} recent.",
        frame.sequence,
        frame.rect.width,
        frame.rect.height,
        frame.rect.x,
        frame.rect.y,
        visible_count
    )
}

#[cfg(feature = "live-ui")]
fn apply_live_dirty_rects(
    instance: &slint_interpreter::ComponentInstance,
    rects: &VecDeque<DirtyRectOverlayState>,
    summary: &str,
) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::{Struct, Value};

    let values = rects
        .iter()
        .map(|rect| {
            Value::Struct(Struct::from_iter([
                ("x".to_string(), Value::Number(rect.x as f64)),
                ("y".to_string(), Value::Number(rect.y as f64)),
                ("width".to_string(), Value::Number(rect.width as f64)),
                ("height".to_string(), Value::Number(rect.height as f64)),
                (
                    "kind".to_string(),
                    Value::String(SharedString::from(rect.kind.as_str())),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    let _ = instance.set_global_property(
        "AnalyticsState",
        "dirty-rects",
        Value::Model(ModelRc::new(VecModel::from(values))),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "dirty-rect-summary",
        Value::String(SharedString::from(summary)),
    );
}

#[cfg(feature = "live-ui")]
fn clear_live_dirty_rects(instance: &slint_interpreter::ComponentInstance) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::Value;

    let _ = instance.set_global_property(
        "AnalyticsState",
        "dirty-rects",
        Value::Model(ModelRc::new(VecModel::<Value>::from(Vec::new()))),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "dirty-rect-summary",
        Value::String(SharedString::from("Dirty overlay idle.")),
    );
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_dirty_rects(
    ui: &AppWindow,
    rects: &VecDeque<DirtyRectOverlayState>,
    summary: &str,
) {
    use slint::{ModelRc, SharedString, VecModel};

    let values = rects
        .iter()
        .map(|rect| DirtyRectOverlay {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            kind: SharedString::from(rect.kind.as_str()),
        })
        .collect::<Vec<_>>();
    let state = ui.global::<AnalyticsState>();
    state.set_dirty_rects(ModelRc::new(VecModel::from(values)));
    state.set_dirty_rect_summary(summary.into());
}

#[cfg(feature = "compiled-ui")]
fn clear_compiled_dirty_rects(ui: &AppWindow) {
    use slint::{ModelRc, VecModel};

    let state = ui.global::<AnalyticsState>();
    state.set_dirty_rects(ModelRc::new(VecModel::<DirtyRectOverlay>::from(Vec::new())));
    state.set_dirty_rect_summary("Dirty overlay idle.".into());
}

#[cfg(feature = "live-ui")]
fn apply_live_frame_chart(
    instance: &slint_interpreter::ComponentInstance,
    rendered: RenderedFrameChart,
) {
    use slint_interpreter::Value;

    let _ = instance.set_global_property(
        "RealtimeState",
        "frame-chart-image",
        Value::Image(rendered.image),
    );
    let _ = instance.set_global_property(
        "RealtimeState",
        "frame-chart-has-data",
        Value::Bool(rendered.has_data),
    );
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_frame_chart(ui: &AppWindow, rendered: RenderedFrameChart) {
    let state = ui.global::<RealtimeState>();
    state.set_frame_chart_image(rendered.image);
    state.set_frame_chart_has_data(rendered.has_data);
}

fn set_frame_chart_samples(
    frame_chart: &SharedRealtimeFrameChart,
    samples: &[FrameSample],
) -> RenderedFrameChart {
    frame_chart
        .lock()
        .expect("realtime frame chart state")
        .set_samples(samples)
}

fn resize_frame_chart(
    frame_chart: &SharedRealtimeFrameChart,
    width: i32,
    height: i32,
) -> Option<RenderedFrameChart> {
    frame_chart
        .lock()
        .expect("realtime frame chart state")
        .resize(width, height)
}

#[cfg(feature = "live-ui")]
fn apply_live_realtime_view(
    instance: &slint_interpreter::ComponentInstance,
    view: &RealtimeViewState,
    frame_chart: &SharedRealtimeFrameChart,
) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::{Struct, Value};

    fn set(instance: &slint_interpreter::ComponentInstance, name: &str, value: Value) {
        let _ = instance.set_global_property("RealtimeState", name, value);
    }

    set(instance, "streaming", Value::Bool(view.streaming));
    set(
        instance,
        "status",
        Value::String(SharedString::from(view.status.as_str())),
    );
    set(
        instance,
        "last-error",
        Value::String(SharedString::from(view.last_error.as_str())),
    );
    set(
        instance,
        "fps-summary",
        Value::String(SharedString::from(view.fps_summary.as_str())),
    );
    set(
        instance,
        "cpu-summary",
        Value::String(SharedString::from(view.cpu_summary.as_str())),
    );
    set(
        instance,
        "memory-total-label",
        Value::String(SharedString::from(view.memory_total_label.as_str())),
    );
    set(
        instance,
        "memory-magik-label",
        Value::String(SharedString::from(view.memory_magik_label.as_str())),
    );
    set(
        instance,
        "memory-other-label",
        Value::String(SharedString::from(view.memory_other_label.as_str())),
    );
    set(
        instance,
        "memory-available-label",
        Value::String(SharedString::from(view.memory_available_label.as_str())),
    );
    set(
        instance,
        "frame-summary",
        Value::String(SharedString::from(view.frame_summary.as_str())),
    );
    set(
        instance,
        "storage-total-label",
        Value::String(SharedString::from(view.storage_total_label.as_str())),
    );
    set(
        instance,
        "storage-used-label",
        Value::String(SharedString::from(view.storage_used_label.as_str())),
    );
    set(
        instance,
        "storage-empty-label",
        Value::String(SharedString::from(view.storage_empty_label.as_str())),
    );
    set(
        instance,
        "storage-activity-summary",
        Value::String(SharedString::from(view.storage_activity_summary.as_str())),
    );
    set(
        instance,
        "frame-hover",
        Value::String(SharedString::from(view.frame_hover.as_str())),
    );
    set(
        instance,
        "combined-cpu-pct",
        Value::Number(view.combined_cpu_pct),
    );
    set(
        instance,
        "magik-memory-pct",
        Value::Number(view.magik_memory_pct),
    );
    set(
        instance,
        "other-memory-pct",
        Value::Number(view.other_memory_pct),
    );
    set(
        instance,
        "available-memory-pct",
        Value::Number(view.available_memory_pct),
    );
    set(
        instance,
        "storage-used-pct",
        Value::Number(view.storage_used_pct),
    );
    set(
        instance,
        "frame-budget-pct",
        Value::Number(view.frame_budget_pct),
    );
    set(
        instance,
        "ui-thread-cpu",
        Value::Number(view.ui_thread_cpu.map_or(-1.0, |cpu| cpu as f64)),
    );

    set(
        instance,
        "cpu-cores",
        Value::Model(ModelRc::new(VecModel::from(
            view.cores
                .iter()
                .map(|core| {
                    Value::Struct(Struct::from_iter([
                        (
                            "label".to_string(),
                            Value::String(SharedString::from(core.label.as_str())),
                        ),
                        ("busy-pct".to_string(), Value::Number(core.busy_pct)),
                    ]))
                })
                .collect::<Vec<_>>(),
        ))),
    );
    set(
        instance,
        "cpu-history",
        Value::Model(ModelRc::new(VecModel::from(live_realtime_points(
            &view.cpu_history,
        )))),
    );
    set(
        instance,
        "cpu0-path",
        Value::String(SharedString::from(view.cpu0_path.as_str())),
    );
    set(
        instance,
        "cpu1-path",
        Value::String(SharedString::from(view.cpu1_path.as_str())),
    );
    set(
        instance,
        "storage-read-path",
        Value::String(SharedString::from(view.storage_read_path.as_str())),
    );
    set(
        instance,
        "storage-write-path",
        Value::String(SharedString::from(view.storage_write_path.as_str())),
    );
    set(
        instance,
        "frame-history",
        Value::Model(ModelRc::new(VecModel::from(live_realtime_points(
            &view.frame_history,
        )))),
    );
    set(
        instance,
        "frame-phases",
        Value::Model(ModelRc::new(VecModel::from(
            view.phases
                .iter()
                .map(|phase| {
                    Value::Struct(Struct::from_iter([
                        (
                            "label".to_string(),
                            Value::String(SharedString::from(phase.label.as_str())),
                        ),
                        ("us".to_string(), Value::Number(phase.us as f64)),
                        ("start-us".to_string(), Value::Number(phase.start_us as f64)),
                        (
                            "color-index".to_string(),
                            Value::Number(f64::from(phase.color_index)),
                        ),
                    ]))
                })
                .collect::<Vec<_>>(),
        ))),
    );
    apply_live_frame_chart(
        instance,
        set_frame_chart_samples(frame_chart, &view.frame_samples),
    );
    set(
        instance,
        "health-tiles",
        Value::Model(ModelRc::new(VecModel::from(
            view.health_tiles
                .iter()
                .map(|tile| {
                    Value::Struct(Struct::from_iter([
                        (
                            "title".to_string(),
                            Value::String(SharedString::from(tile.title.as_str())),
                        ),
                        (
                            "value".to_string(),
                            Value::String(SharedString::from(tile.value.as_str())),
                        ),
                        (
                            "detail".to_string(),
                            Value::String(SharedString::from(tile.detail.as_str())),
                        ),
                        (
                            "state".to_string(),
                            Value::String(SharedString::from(tile.state.as_str())),
                        ),
                    ]))
                })
                .collect::<Vec<_>>(),
        ))),
    );
}

#[cfg(feature = "live-ui")]
fn live_realtime_points(points: &[RealtimeChartPoint]) -> Vec<slint_interpreter::Value> {
    use slint_interpreter::{Struct, Value};

    points
        .iter()
        .map(|point| {
            Value::Struct(Struct::from_iter([
                ("value".to_string(), Value::Number(point.value)),
                ("alert".to_string(), Value::Bool(point.alert)),
            ]))
        })
        .collect()
}

#[cfg(feature = "live-ui")]
fn apply_live_realtime_off(
    instance: &slint_interpreter::ComponentInstance,
    frame_chart: &SharedRealtimeFrameChart,
) {
    apply_live_realtime_view(
        instance,
        &realtime_view_from_history(&RealtimeHistory::default(), false, ""),
        frame_chart,
    );
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_realtime_view(
    ui: &AppWindow,
    view: &RealtimeViewState,
    frame_chart: &SharedRealtimeFrameChart,
) {
    use slint::{ModelRc, SharedString, VecModel};

    let state = ui.global::<RealtimeState>();
    state.set_streaming(view.streaming);
    state.set_status(view.status.as_str().into());
    state.set_last_error(view.last_error.as_str().into());
    state.set_fps_summary(view.fps_summary.as_str().into());
    state.set_cpu_summary(view.cpu_summary.as_str().into());
    state.set_memory_total_label(view.memory_total_label.as_str().into());
    state.set_memory_magik_label(view.memory_magik_label.as_str().into());
    state.set_memory_other_label(view.memory_other_label.as_str().into());
    state.set_memory_available_label(view.memory_available_label.as_str().into());
    state.set_frame_summary(view.frame_summary.as_str().into());
    state.set_storage_total_label(view.storage_total_label.as_str().into());
    state.set_storage_used_label(view.storage_used_label.as_str().into());
    state.set_storage_empty_label(view.storage_empty_label.as_str().into());
    state.set_storage_activity_summary(view.storage_activity_summary.as_str().into());
    state.set_frame_hover(view.frame_hover.as_str().into());
    state.set_combined_cpu_pct(view.combined_cpu_pct as f32);
    state.set_magik_memory_pct(view.magik_memory_pct as f32);
    state.set_other_memory_pct(view.other_memory_pct as f32);
    state.set_available_memory_pct(view.available_memory_pct as f32);
    state.set_storage_used_pct(view.storage_used_pct as f32);
    state.set_frame_budget_pct(view.frame_budget_pct as f32);
    state.set_ui_thread_cpu(
        view.ui_thread_cpu
            .and_then(|cpu| i32::try_from(cpu).ok())
            .unwrap_or(-1),
    );
    state.set_cpu_cores(ModelRc::new(VecModel::from(
        view.cores
            .iter()
            .map(|core| RealtimeCpuCore {
                label: SharedString::from(core.label.as_str()),
                busy_pct: core.busy_pct as f32,
            })
            .collect::<Vec<_>>(),
    )));
    state.set_cpu_history(compiled_realtime_points(&view.cpu_history));
    state.set_cpu0_path(view.cpu0_path.as_str().into());
    state.set_cpu1_path(view.cpu1_path.as_str().into());
    state.set_storage_read_path(view.storage_read_path.as_str().into());
    state.set_storage_write_path(view.storage_write_path.as_str().into());
    state.set_frame_history(compiled_realtime_points(&view.frame_history));
    state.set_frame_phases(ModelRc::new(VecModel::from(
        view.phases
            .iter()
            .map(|phase| RealtimeFramePhase {
                label: SharedString::from(phase.label.as_str()),
                us: phase.us as i32,
                start_us: phase.start_us as i32,
                color_index: phase.color_index,
            })
            .collect::<Vec<_>>(),
    )));
    apply_compiled_frame_chart(
        ui,
        set_frame_chart_samples(frame_chart, &view.frame_samples),
    );
    state.set_health_tiles(ModelRc::new(VecModel::from(
        view.health_tiles
            .iter()
            .map(|tile| RealtimeHealthTile {
                title: SharedString::from(tile.title.as_str()),
                value: SharedString::from(tile.value.as_str()),
                detail: SharedString::from(tile.detail.as_str()),
                state: SharedString::from(tile.state.as_str()),
            })
            .collect::<Vec<_>>(),
    )));
}

#[cfg(feature = "compiled-ui")]
fn compiled_realtime_points(points: &[RealtimeChartPoint]) -> slint::ModelRc<RealtimePoint> {
    use slint::{ModelRc, VecModel};

    ModelRc::new(VecModel::from(
        points
            .iter()
            .map(|point| RealtimePoint {
                value: point.value as f32,
                alert: point.alert,
            })
            .collect::<Vec<_>>(),
    ))
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_realtime_off(ui: &AppWindow, frame_chart: &SharedRealtimeFrameChart) {
    apply_compiled_realtime_view(
        ui,
        &realtime_view_from_history(&RealtimeHistory::default(), false, ""),
        frame_chart,
    );
}

#[derive(Clone, Debug)]
struct ProfileArtifactView {
    path: String,
    summary: String,
    bars: Vec<ProfileBarView>,
    heatmap: Vec<ProfileHeatmapCellView>,
    stats_rows: Vec<Vec<String>>,
    slow_rows: Vec<Vec<String>>,
    histogram_rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug)]
struct ProfileBarView {
    frame: i32,
    wall_us: i32,
    over_budget: bool,
    segments: Vec<ProfileSegmentView>,
}

#[derive(Clone, Debug)]
struct ProfileSegmentView {
    phase: String,
    us: i32,
    start_us: i32,
}

#[derive(Clone, Debug)]
struct ProfileHeatmapCellView {
    x: i32,
    y: i32,
    hits: i32,
    intensity: f32,
}

fn load_profile_artifact(path: &str) -> Result<ProfileArtifactView, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("Enter a frame-profile TSV path.".to_string());
    }
    let text = std::fs::read_to_string(path).map_err(|err| format!("read {path}: {err}"))?;
    let profile = frame_profile::FrameProfile::parse_tsv(&text)?;
    Ok(profile_artifact_view(path, &profile))
}

fn profile_artifact_view(path: &str, profile: &frame_profile::FrameProfile) -> ProfileArtifactView {
    let slow_count = profile
        .rows
        .iter()
        .filter(|row| row.wall_us >= frame_profile::FRAME_BUDGET_US)
        .count();
    let summary = format!(
        "{} frames loaded from {path}; {} frames at or over 16.667ms.",
        profile.rows.len(),
        slow_count
    );
    let bars = profile
        .frame_bars(120)
        .into_iter()
        .map(|bar| {
            let mut start_us = 0_i32;
            let segments = bar
                .segments
                .into_iter()
                .map(|segment| {
                    let us = clamp_u64_i32(segment.us);
                    let view = ProfileSegmentView {
                        phase: segment.label,
                        us,
                        start_us,
                    };
                    start_us = start_us.saturating_add(us);
                    view
                })
                .collect();
            ProfileBarView {
                frame: clamp_u64_i32(bar.frame),
                wall_us: clamp_u64_i32(bar.wall_us),
                over_budget: bar.over_budget,
                segments,
            }
        })
        .collect();
    let heatmap_cells = profile.heatmap(96, 54);
    let max_hits = heatmap_cells
        .iter()
        .map(|cell| cell.hits)
        .max()
        .unwrap_or(1);
    let heatmap = heatmap_cells
        .into_iter()
        .map(|cell| ProfileHeatmapCellView {
            x: cell.x as i32,
            y: cell.y as i32,
            hits: clamp_u64_i32(cell.hits),
            intensity: (cell.hits as f32 / max_hits as f32).clamp(0.0, 1.0),
        })
        .collect();
    let stats_rows = profile
        .phase_stats()
        .into_iter()
        .take(16)
        .map(|stat| {
            vec![
                stat.label,
                stat.avg.to_string(),
                stat.p50.to_string(),
                stat.p95.to_string(),
                stat.p99.to_string(),
                stat.max.to_string(),
            ]
        })
        .collect();
    let slow_rows = profile
        .slow_frames(10, frame_profile::FRAME_BUDGET_US)
        .into_iter()
        .map(|row| {
            vec![
                row.frame.to_string(),
                format!("{}us", row.wall_us),
                row.dominant,
                row.rect
                    .map(|rect| format!("{},{}..{},{}", rect.x0, rect.y0, rect.x1, rect.y1))
                    .unwrap_or_else(|| "none".to_string()),
            ]
        })
        .collect();
    let histogram_rows = profile
        .histogram("wall_us")
        .into_iter()
        .map(|bucket| vec![bucket.label, bucket.count.to_string()])
        .collect();
    ProfileArtifactView {
        path: path.to_string(),
        summary,
        bars,
        heatmap,
        stats_rows,
        slow_rows,
        histogram_rows,
    }
}

fn clamp_u64_i32(value: u64) -> i32 {
    value.min(i32::MAX as u64) as i32
}

#[cfg(feature = "live-ui")]
fn set_live_profile_loading(instance: &slint_interpreter::ComponentInstance, path: &str) {
    use slint::SharedString;
    use slint_interpreter::Value;

    let _ = instance.set_global_property("AnalyticsState", "profile-loading", Value::Bool(true));
    let _ = instance.set_global_property(
        "AnalyticsState",
        "profile-status",
        Value::String(SharedString::from(format!("Loading {path}..."))),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "profile-last-error",
        Value::String(SharedString::from("")),
    );
}

#[cfg(feature = "live-ui")]
fn apply_live_profile_result(
    instance: &slint_interpreter::ComponentInstance,
    result: Result<ProfileArtifactView, String>,
) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::{Struct, Value};

    let _ = instance.set_global_property("AnalyticsState", "profile-loading", Value::Bool(false));
    match result {
        Ok(view) => {
            let bars = view
                .bars
                .iter()
                .map(|bar| {
                    let segments = bar
                        .segments
                        .iter()
                        .map(|segment| {
                            Value::Struct(Struct::from_iter([
                                (
                                    "phase".to_string(),
                                    Value::String(SharedString::from(segment.phase.as_str())),
                                ),
                                ("us".to_string(), Value::Number(segment.us as f64)),
                                (
                                    "start-us".to_string(),
                                    Value::Number(segment.start_us as f64),
                                ),
                            ]))
                        })
                        .collect::<Vec<_>>();
                    Value::Struct(Struct::from_iter([
                        ("frame".to_string(), Value::Number(bar.frame as f64)),
                        ("wall-us".to_string(), Value::Number(bar.wall_us as f64)),
                        ("over-budget".to_string(), Value::Bool(bar.over_budget)),
                        (
                            "segments".to_string(),
                            Value::Model(ModelRc::new(VecModel::from(segments))),
                        ),
                    ]))
                })
                .collect::<Vec<_>>();
            let heatmap = view
                .heatmap
                .iter()
                .map(|cell| {
                    Value::Struct(Struct::from_iter([
                        ("x".to_string(), Value::Number(cell.x as f64)),
                        ("y".to_string(), Value::Number(cell.y as f64)),
                        ("hits".to_string(), Value::Number(cell.hits as f64)),
                        (
                            "intensity".to_string(),
                            Value::Number(cell.intensity as f64),
                        ),
                    ]))
                })
                .collect::<Vec<_>>();
            let _ = instance.set_global_property(
                "AnalyticsState",
                "profile-path",
                Value::String(SharedString::from(view.path.as_str())),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "profile-status",
                Value::String(SharedString::from("Profile loaded.")),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "profile-last-error",
                Value::String(SharedString::from("")),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "profile-has-data",
                Value::Bool(true),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "profile-summary",
                Value::String(SharedString::from(view.summary.as_str())),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "profile-bars",
                Value::Model(ModelRc::new(VecModel::from(bars))),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "profile-heatmap",
                Value::Model(ModelRc::new(VecModel::from(heatmap))),
            );
            set_live_table_rows(instance, "profile-stats-rows", &view.stats_rows);
            set_live_table_rows(instance, "profile-slow-rows", &view.slow_rows);
            set_live_table_rows(instance, "profile-histogram-rows", &view.histogram_rows);
        }
        Err(err) => {
            let _ = instance.set_global_property(
                "AnalyticsState",
                "profile-status",
                Value::String(SharedString::from("Profile load failed.")),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "profile-last-error",
                Value::String(SharedString::from(err)),
            );
        }
    }
}

#[cfg(feature = "live-ui")]
fn set_live_table_rows(
    instance: &slint_interpreter::ComponentInstance,
    property: &str,
    rows: &[Vec<String>],
) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::{Struct, Value};

    let values = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let cells = row
                .iter()
                .map(|cell| live_library_text_cell(cell))
                .collect::<Vec<_>>();
            Value::Struct(Struct::from_iter([
                (
                    "id".to_string(),
                    Value::String(SharedString::from(format!("{property}-{index}").as_str())),
                ),
                (
                    "cells".to_string(),
                    Value::Model(ModelRc::new(VecModel::from(cells))),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    let _ = instance.set_global_property(
        "AnalyticsState",
        property,
        Value::Model(ModelRc::new(VecModel::from(values))),
    );
}

#[cfg(feature = "compiled-ui")]
fn set_compiled_profile_loading(ui: &AppWindow, path: &str) {
    let state = ui.global::<AnalyticsState>();
    state.set_profile_loading(true);
    state.set_profile_status(format!("Loading {path}...").into());
    state.set_profile_last_error("".into());
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_profile_result(ui: &AppWindow, result: Result<ProfileArtifactView, String>) {
    use slint::{ModelRc, SharedString, VecModel};

    let state = ui.global::<AnalyticsState>();
    state.set_profile_loading(false);
    match result {
        Ok(view) => {
            let bars = view
                .bars
                .iter()
                .map(|bar| ProfileFrameBar {
                    frame: bar.frame,
                    wall_us: bar.wall_us,
                    over_budget: bar.over_budget,
                    segments: ModelRc::new(VecModel::from(
                        bar.segments
                            .iter()
                            .map(|segment| ProfileFrameSegment {
                                phase: SharedString::from(segment.phase.as_str()),
                                us: segment.us,
                                start_us: segment.start_us,
                            })
                            .collect::<Vec<_>>(),
                    )),
                })
                .collect::<Vec<_>>();
            let heatmap = view
                .heatmap
                .iter()
                .map(|cell| ProfileHeatmapCell {
                    x: cell.x,
                    y: cell.y,
                    hits: cell.hits,
                    intensity: cell.intensity,
                })
                .collect::<Vec<_>>();
            state.set_profile_path(view.path.as_str().into());
            state.set_profile_status("Profile loaded.".into());
            state.set_profile_last_error("".into());
            state.set_profile_has_data(true);
            state.set_profile_summary(view.summary.as_str().into());
            state.set_profile_bars(ModelRc::new(VecModel::from(bars)));
            state.set_profile_heatmap(ModelRc::new(VecModel::from(heatmap)));
            state.set_profile_stats_rows(compiled_profile_table_rows(
                "profile-stats",
                &view.stats_rows,
            ));
            state.set_profile_slow_rows(compiled_profile_table_rows(
                "profile-slow",
                &view.slow_rows,
            ));
            state.set_profile_histogram_rows(compiled_profile_table_rows(
                "profile-histogram",
                &view.histogram_rows,
            ));
        }
        Err(err) => {
            state.set_profile_status("Profile load failed.".into());
            state.set_profile_last_error(err.into());
        }
    }
}

#[cfg(feature = "compiled-ui")]
fn compiled_profile_table_rows(prefix: &str, rows: &[Vec<String>]) -> slint::ModelRc<DataTableRow> {
    use slint::{ModelRc, VecModel};

    ModelRc::new(VecModel::from(
        rows.iter()
            .enumerate()
            .map(|(index, row)| DataTableRow {
                id: format!("{prefix}-{index}").into(),
                cells: ModelRc::new(VecModel::from(
                    row.iter()
                        .map(|cell| compiled_library_text_cell(cell))
                        .collect::<Vec<_>>(),
                )),
            })
            .collect::<Vec<_>>(),
    ))
}

fn format_byte_size(bytes: u64) -> String {
    analytics_ui_state::format_byte_size(bytes)
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_sd_state(ui: &AppWindow, browser: &SharedSdBrowser) {
    use slint::{Image, ModelRc, VecModel};

    let Ok(browser) = browser.lock() else {
        return;
    };
    let state = ui.global::<SdCardState>();
    state.set_current_path(browser.current_path().into());
    state.set_status(browser.status().into());
    state.set_last_error(browser.last_error().into());
    state.set_loading(browser.loading());
    state.set_show_hidden(browser.show_hidden());
    let detail = browser.selected_detail();
    state.set_detail_title(detail.title.as_str().into());
    state.set_detail_subtitle(detail.subtitle.as_str().into());
    state.set_detail_kind(detail.kind.as_str().into());
    state.set_detail_icon_key(detail.icon_key.as_str().into());
    state.set_detail_size(detail.size_label.as_str().into());
    state.set_detail_modified(detail.modified_label.as_str().into());
    state.set_detail_flags(detail.flags_label.as_str().into());
    state.set_detail_loading(detail.loading);
    state.set_detail_error(detail.error.as_str().into());
    state.set_detail_has_image(detail.has_image && !detail.image_path.is_empty());
    state.set_detail_image(if detail.image_path.is_empty() {
        Image::default()
    } else {
        Image::load_from_path(Path::new(&detail.image_path)).unwrap_or_default()
    });
    state.set_detail_image_summary(detail.image_summary.as_str().into());
    state.set_detail_is_mra(detail.is_mra);
    state.set_detail_overview_rows(ModelRc::new(VecModel::from(compiled_sd_metadata_rows(
        &detail.overview_rows,
    ))));
    state.set_detail_mra_summary_rows(ModelRc::new(VecModel::from(compiled_sd_metadata_rows(
        &detail.mra_summary_rows,
    ))));
    state.set_detail_mra_xml_rows(ModelRc::new(VecModel::from(compiled_sd_metadata_rows(
        &detail.mra_xml_rows,
    ))));
    state.set_detail_mra_path_rows(ModelRc::new(VecModel::from(compiled_sd_metadata_rows(
        &detail.mra_path_rows,
    ))));
    state.set_detail_mra_warnings(ModelRc::new(VecModel::from(compiled_sd_metadata_rows(
        &detail.mra_warnings,
    ))));
    state.set_detail_raw_xml(detail.raw_xml.as_str().into());
    state.set_detail_raw_xml_truncated(detail.raw_xml_truncated);
    state.set_rows(ModelRc::new(VecModel::from(
        browser
            .rows()
            .iter()
            .map(compiled_tree_row)
            .collect::<Vec<_>>(),
    )));
}

#[cfg(feature = "compiled-ui")]
fn compiled_sd_metadata_rows(rows: &[sd_card::SdMetadataRow]) -> Vec<SdMetadataRow> {
    rows.iter()
        .map(|row| SdMetadataRow {
            label: row.label.as_str().into(),
            value: row.value.as_str().into(),
            kind: row.kind.as_str().into(),
        })
        .collect()
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_library_state(ui: &AppWindow, browser: &SharedLibraryBrowser) {
    use slint::{ModelRc, VecModel};

    let Ok(browser) = browser.lock() else {
        return;
    };
    let state = ui.global::<LibraryState>();
    state.set_updating_controls(true);
    state.set_status(browser.status.as_str().into());
    state.set_warning(browser.warning.as_str().into());
    state.set_last_error(browser.last_error.as_str().into());
    state.set_loading(browser.loading);
    state.set_query(browser.query.search.as_str().into());
    state.set_system_filter(browser.query.system.as_str().into());
    state.set_category_filter(browser.query.category.as_str().into());
    state.set_region_filter(browser.query.region.as_str().into());
    state.set_manufacturer_filter(browser.query.manufacturer.as_str().into());
    state.set_preview_filter(browser.query.preview.as_str().into());
    state.set_confidence_filter(browser.query.confidence.as_str().into());
    state.set_sort_column(library_sort_column_id(browser.query.sort_column).into());
    state.set_sort_direction(match browser.query.sort_direction {
        library::LibrarySortDirection::Ascending => DataTableSortDirection::Ascending,
        library::LibrarySortDirection::Descending => DataTableSortDirection::Descending,
    });
    state.set_result_summary(browser.result_summary().as_str().into());
    let view = browser.current_view().unwrap_or_default();
    state.set_page(i32::try_from(view.page).unwrap_or(1));
    state.set_page_count(i32::try_from(view.page_count).unwrap_or(1));
    state.set_selected_game_id(browser.selected_game_id.as_str().into());
    if let Some(game) = browser.selected_game() {
        state.set_detail_title(game.title.as_str().into());
        state.set_detail_subtitle(format!("{} · {}", game.system_title, game.id).into());
    } else {
        state.set_detail_title("No game selected".into());
        state.set_detail_subtitle("Select a row to inspect catalog facts.".into());
    }
    let detail_sections = library_detail_sections(browser.selected_game());
    state.set_detail_overview_rows(compiled_library_detail_rows(&detail_sections.overview));
    state.set_detail_system_rows(compiled_library_detail_rows(&detail_sections.system));
    state.set_detail_media_rows(compiled_library_detail_rows(&detail_sections.media));
    state.set_detail_launch_rows(compiled_library_detail_rows(&detail_sections.launch));
    state.set_detail_identity_rows(compiled_library_detail_rows(&detail_sections.identity));
    state.set_detail_path_rows(compiled_library_detail_rows(&detail_sections.paths));
    state.set_rows(ModelRc::new(VecModel::from(
        view.rows
            .iter()
            .map(|game| compiled_library_row(game, false))
            .collect::<Vec<_>>(),
    )));
    state.set_compact_rows(ModelRc::new(VecModel::from(
        view.rows
            .iter()
            .map(|game| compiled_library_row(game, true))
            .collect::<Vec<_>>(),
    )));
    let (
        system_options,
        category_options,
        region_options,
        manufacturer_options,
        confidence_options,
    ) = match &browser.catalog {
        Some(catalog) => (
            library_select_options("All systems", &catalog.systems),
            library_select_options("All categories", &catalog.categories),
            library_select_options("All regions", &catalog.regions),
            library_select_options("All manufacturers", &catalog.manufacturers),
            library_select_options("All confidence", &catalog.confidences),
        ),
        None => (
            library_select_options("All systems", &[]),
            library_select_options("All categories", &[]),
            library_select_options("All regions", &[]),
            library_select_options("All manufacturers", &[]),
            library_select_options("All confidence", &[]),
        ),
    };
    let preview_options = library_preview_options();
    state.set_system_options(compiled_select_options(&system_options));
    state.set_category_options(compiled_select_options(&category_options));
    state.set_region_options(compiled_select_options(&region_options));
    state.set_manufacturer_options(compiled_select_options(&manufacturer_options));
    state.set_preview_options(compiled_select_options(&preview_options));
    state.set_confidence_options(compiled_select_options(&confidence_options));
    state.set_system_index(library_option_index(&system_options, &browser.query.system));
    state.set_category_index(library_option_index(
        &category_options,
        &browser.query.category,
    ));
    state.set_region_index(library_option_index(&region_options, &browser.query.region));
    state.set_manufacturer_index(library_option_index(
        &manufacturer_options,
        &browser.query.manufacturer,
    ));
    state.set_preview_index(library_option_index(
        &preview_options,
        &browser.query.preview,
    ));
    state.set_confidence_index(library_option_index(
        &confidence_options,
        &browser.query.confidence,
    ));
    state.set_updating_controls(false);
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_detail_rows(rows: &[LibraryDetailRow]) -> slint::ModelRc<DataTableRow> {
    use slint::{ModelRc, VecModel};

    ModelRc::new(VecModel::from(
        rows.iter()
            .enumerate()
            .map(|(index, row)| DataTableRow {
                id: format!("detail-{index}").into(),
                cells: ModelRc::new(VecModel::from(vec![
                    compiled_library_text_cell(&row.field),
                    compiled_library_text_cell(&row.value),
                ])),
            })
            .collect::<Vec<_>>(),
    ))
}

#[cfg(feature = "compiled-ui")]
fn compiled_select_options(options: &[LibrarySelectOptionItem]) -> slint::ModelRc<SelectOption> {
    use slint::{ModelRc, VecModel};

    ModelRc::new(VecModel::from(
        options
            .iter()
            .map(|option| SelectOption {
                value: option.value.as_str().into(),
                label: option.label.as_str().into(),
                enabled: option.enabled,
            })
            .collect::<Vec<_>>(),
    ))
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_row(game: &library::LibraryGame, compact: bool) -> DataTableRow {
    use slint::{ModelRc, VecModel};

    let cells = if compact {
        vec![
            compiled_library_text_cell(&game.title),
            compiled_library_text_cell(&game.system_title),
            compiled_library_preview_cell(game.has_preview),
        ]
    } else {
        vec![
            compiled_library_text_cell(&game.title),
            compiled_library_text_cell(&game.system_title),
            compiled_library_text_cell(&game.year),
            compiled_library_text_cell(&game.manufacturer),
            compiled_library_text_cell(&game.category),
            compiled_library_preview_cell(game.has_preview),
            compiled_library_text_cell(&library_discovered_label(&game.discovered_at_unix)),
        ]
    };

    DataTableRow {
        id: game.id.as_str().into(),
        cells: ModelRc::new(VecModel::from(cells)),
    }
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_text_cell(text: &str) -> DataTableCell {
    compiled_library_cell(DataTableCellKind::Text, text, LabelVariant::Default)
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_preview_cell(has_preview: bool) -> DataTableCell {
    compiled_library_cell(
        DataTableCellKind::Label,
        library_preview_label(has_preview),
        if has_preview {
            LabelVariant::Success
        } else {
            LabelVariant::Secondary
        },
    )
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_cell(
    kind: DataTableCellKind,
    text: &str,
    label_variant: LabelVariant,
) -> DataTableCell {
    DataTableCell {
        kind,
        text: if text.is_empty() { "-" } else { text }.into(),
        label_variant,
        label_size: LabelSize::Small,
        icon: slint::Image::default(),
        icon_tint: DataTableIconTint::Default,
    }
}

#[cfg(feature = "compiled-ui")]
fn compiled_tree_row(row: &SdTreeRow) -> TreeViewRow {
    TreeViewRow {
        id: row.id.as_str().into(),
        label: row.label.as_str().into(),
        level: row.level,
        has_children: row.has_children,
        expanded: row.expanded,
        current: row.current,
        leading_is_directory: row.leading_is_directory,
        has_leading_visual: true,
        preserve_leading_icon_color: true,
        trailing: TreeViewTrailingVisual::None,
        has_leading_action: false,
        show_leading_action_icon: false,
        leading_action_icon: slint::Image::default(),
        leading_file_icon: file_icons::material_icon(row.icon_key.as_str()),
        interactive: row.interactive,
        is_skeleton: row.is_skeleton,
        has_secondary_actions: false,
        secondary_actions_badge: "".into(),
        loading_children_badge: row.loading_children_badge.as_str().into(),
    }
}

#[cfg(feature = "live-ui")]
fn spawn_live_library_sync(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    browser: SharedLibraryBrowser,
    host: String,
) {
    std::thread::spawn(move || {
        let result = library::sync_library_catalog(&host);
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_sync_result(result);
            }
            if let Some(instance) = instance.upgrade() {
                apply_live_library_state(&instance, &browser);
            }
        });
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_sd_fetch(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    browser: SharedSdBrowser,
    host: String,
    path: String,
    show_hidden: bool,
) {
    let loading_instance = instance.clone();
    let loading_browser = Arc::clone(&browser);
    let loading_path = path.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SD_FOLDER_LOADING_DELAY);
        let _ = slint::invoke_from_event_loop(move || {
            let revealed = loading_browser.lock().is_ok_and(|mut browser| {
                browser.reveal_loading_after(&loading_path, SD_FOLDER_LOADING_DELAY)
            });
            if revealed {
                if let Some(instance) = loading_instance.upgrade() {
                    apply_live_sd_state(&instance, &loading_browser);
                }
            }
        });
    });

    std::thread::spawn(move || {
        let result = fetch_sd_directory(&host, &path, show_hidden).map_err(|err| err.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_listing_if_current_policy(&path, show_hidden, result);
            }
            if let Some(instance) = instance.upgrade() {
                apply_live_sd_state(&instance, &browser);
            }
        });
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_sd_detail_fetch(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    browser: SharedSdBrowser,
    host: String,
    request: sd_card::SdDetailRequest,
) {
    std::thread::spawn(move || {
        let result = fetch_sd_item_detail(&host, &request.path).map_err(|err| err.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_detail_result(&request.path, request.generation, result);
            }
            if let Some(instance) = instance.upgrade() {
                apply_live_sd_state(&instance, &browser);
            }
        });
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_framebuffer_capture(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    capture_state: SharedFramebufferCapture,
    host: String,
) {
    std::thread::spawn(move || {
        let result = fetch_framebuffer_capture(&host).map_err(|err| err.to_string());
        let capture = result.as_ref().ok().cloned();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(capture) = capture {
                if let Ok(mut state) = capture_state.lock() {
                    *state = Some(capture);
                }
            }
            if let Some(instance) = instance.upgrade() {
                apply_live_framebuffer_capture_result(&instance, result);
            }
        });
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_save_framebuffer_capture(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    capture_state: SharedFramebufferCapture,
) {
    std::thread::spawn(move || {
        let result = capture_state
            .lock()
            .ok()
            .and_then(|state| state.clone())
            .ok_or_else(|| "Capture a framebuffer before saving.".to_string())
            .and_then(|capture| save_framebuffer_capture_png(&capture));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(instance) = instance.upgrade() {
                match result {
                    Ok(path) => apply_live_save_status(
                        &instance,
                        &format!("Saved framebuffer PNG to {}.", path.display()),
                        "",
                    ),
                    Err(err) => {
                        apply_live_save_status(&instance, "Framebuffer PNG save failed.", &err)
                    }
                }
            }
        });
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_profile_load(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    path: String,
) {
    std::thread::spawn(move || {
        let result = load_profile_artifact(&path);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(instance) = instance.upgrade() {
                apply_live_profile_result(&instance, result);
            }
        });
    });
}

#[cfg(feature = "live-ui")]
#[allow(clippy::too_many_arguments)]
fn start_live_framebuffer_display_clock(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    capture_state: SharedFramebufferCapture,
    stream_generation: SharedLiveStreamGeneration,
    mailbox: SharedFramebufferDisplayMailbox,
    display_state: SharedFramebufferDisplayState,
    render_metrics: Arc<FramebufferRenderMetrics>,
    stream_start: Instant,
    generation: u64,
) -> Option<FramebufferDisplayController> {
    let strong = instance.upgrade()?;
    let window = strong.window();
    let chrome_enabled = display_state
        .lock()
        .map(|state| state.chrome_enabled)
        .unwrap_or(false);
    let chrome_instance = instance.clone();
    let chrome_generation = Arc::clone(&stream_generation);
    let chrome_mailbox = Arc::clone(&mailbox);
    let chrome_state = Arc::clone(&display_state);
    let chrome_metrics = Arc::clone(&render_metrics);
    let tick_metrics = Arc::clone(&render_metrics);
    let clock = start_framebuffer_display_clock(window, move |tick| {
        tick_metrics.mark_display_tick(tick_metrics.clock_kind(), tick);
        consume_live_framebuffer_display(
            instance.clone(),
            Arc::clone(&capture_state),
            Arc::clone(&stream_generation),
            Arc::clone(&mailbox),
            Arc::clone(&display_state),
            Arc::clone(&tick_metrics),
            generation,
        );
    });
    render_metrics.set_clock_kind(clock.kind());
    let chrome_timer = chrome_enabled.then(|| {
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(1),
            move || {
                if chrome_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                let chrome = chrome_state.lock().ok().and_then(|state| {
                    state.last_received_at.map(|received_at| {
                        (
                            state.last_source_sequence,
                            state.last_source_timestamp_us,
                            state.recent_dirty_rects.clone(),
                            state.dirty_summary.clone(),
                            framebuffer_display_summary(
                                &chrome_mailbox,
                                &state,
                                &chrome_metrics,
                                stream_start,
                                received_at,
                            ),
                        )
                    })
                });
                let Some((sequence, timestamp_us, rects, dirty_summary, stream_summary)) = chrome
                else {
                    return;
                };
                chrome_metrics.cadence.record(
                    CadenceEventKind::ChromeRefresh,
                    sequence,
                    timestamp_us,
                    chrome_metrics.applied_serial.load(Ordering::Acquire),
                    0,
                    0,
                );
                if let Some(instance) = chrome_instance.upgrade() {
                    apply_live_dirty_rects(&instance, &rects, &dirty_summary);
                    apply_live_stream_summary(&instance, &stream_summary);
                }
            },
        );
        timer
    });
    Some(FramebufferDisplayController {
        clock,
        chrome_timer,
    })
}

#[cfg(feature = "live-ui")]
fn consume_live_framebuffer_display(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    capture_state: SharedFramebufferCapture,
    stream_generation: SharedLiveStreamGeneration,
    mailbox: SharedFramebufferDisplayMailbox,
    display_state: SharedFramebufferDisplayState,
    render_metrics: Arc<FramebufferRenderMetrics>,
    generation: u64,
) {
    if stream_generation.load(Ordering::SeqCst) != generation {
        mailbox.close();
        return;
    }
    let Some(update) = mailbox.take() else {
        let _ = mailbox.complete_apply();
        return;
    };
    let Some(instance) = instance.upgrade() else {
        mailbox.close();
        return;
    };
    render_metrics.cadence.record(
        CadenceEventKind::MailboxTake,
        update.frame.sequence,
        update.frame.timestamp_us,
        render_metrics.applied_serial.load(Ordering::Acquire),
        update.received_at.elapsed().as_micros() as u64,
        0,
    );
    let geometry_changed = display_state
        .lock()
        .map(|mut state| {
            let geometry_changed = record_applied_frame(&mut state, &update.frame);
            state.last_received_at = Some(update.received_at);
            state.last_source_sequence = update.frame.sequence;
            state.last_source_timestamp_us = update.frame.timestamp_us;
            state.dirty_summary = dirty_rect_summary(&update.frame, state.recent_dirty_rects.len());
            geometry_changed
        })
        .unwrap_or_default();
    apply_live_framebuffer_stream_capture(
        &instance,
        &update.frame.capture,
        update.pixels,
        geometry_changed,
    );
    render_metrics.mark_applied(
        update.received_at,
        update.frame.sequence,
        update.frame.timestamp_us,
    );
    instance.window().request_redraw();
    render_metrics.cadence.record(
        CadenceEventKind::RedrawSubmit,
        update.frame.sequence,
        update.frame.timestamp_us,
        render_metrics.applied_serial.load(Ordering::Acquire),
        0,
        0,
    );
    if let Ok(mut state) = capture_state.lock() {
        *state = Some(update.frame.capture);
    }
    let _ = mailbox.complete_apply();
}

#[cfg(feature = "live-ui")]
#[allow(clippy::too_many_arguments)]
fn spawn_live_framebuffer_stream(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    capture_state: SharedFramebufferCapture,
    stream_generation: SharedLiveStreamGeneration,
    stream_control: SharedFramebufferStreamControl,
    render_metrics: Arc<FramebufferRenderMetrics>,
    mailbox: SharedFramebufferDisplayMailbox,
    host: String,
    generation: u64,
) {
    std::thread::spawn(move || {
        render_metrics.reset();
        let seed_capture = fetch_framebuffer_capture(&host).ok();
        if let Some(capture) = seed_capture.clone() {
            let event_generation = Arc::clone(&stream_generation);
            let event_capture_state = Arc::clone(&capture_state);
            let event_instance = instance.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if event_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Ok(mut state) = event_capture_state.lock() {
                    *state = Some(capture.clone());
                }
                if let Some(instance) = event_instance.upgrade() {
                    apply_live_framebuffer_capture_result(&instance, Ok(capture));
                }
            });
        }
        let mut stream = match connect_framebuffer_stream_seeded(&host, seed_capture.as_ref()) {
            Ok(stream) => stream,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(instance) = instance.upgrade() {
                        apply_live_stream_disconnected(&instance, &err);
                    }
                });
                return;
            }
        };
        let control = match stream.control() {
            Ok(control) => control,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(instance) = instance.upgrade() {
                        apply_live_stream_disconnected(&instance, &err);
                    }
                });
                return;
            }
        };
        if !analytics_ui_state::generation_is_current(
            stream_generation.load(Ordering::SeqCst),
            generation,
        ) {
            control.shutdown();
            return;
        }
        register_framebuffer_stream(&stream_control, generation, control);
        while analytics_ui_state::generation_is_current(
            stream_generation.load(Ordering::SeqCst),
            generation,
        ) {
            match stream.next_frame() {
                Ok(frame) => {
                    let received_at = Instant::now();
                    render_metrics.cadence.record_at(
                        CadenceEventKind::SourceReceived,
                        frame.timing.read_complete,
                        frame.sequence,
                        frame.timestamp_us,
                        0,
                        0,
                        0,
                    );
                    render_metrics.cadence.record_at(
                        CadenceEventKind::DecodeComplete,
                        frame.timing.decompress_complete,
                        frame.sequence,
                        frame.timestamp_us,
                        0,
                        0,
                        0,
                    );
                    let pixels = framebuffer_capture_pixel_buffer(&frame.capture);
                    render_metrics.cadence.record(
                        CadenceEventKind::PixelBufferReady,
                        frame.sequence,
                        frame.timestamp_us,
                        0,
                        received_at.elapsed().as_micros() as u64,
                        0,
                    );
                    let (_, coalesced_before) = mailbox.stats();
                    let source_sequence = frame.sequence;
                    let source_timestamp_us = frame.timestamp_us;
                    let _ = mailbox.publish(FramebufferDisplayUpdate {
                        frame,
                        pixels,
                        received_at,
                    });
                    let (_, coalesced) = mailbox.stats();
                    render_metrics.cadence.record(
                        if coalesced > coalesced_before {
                            CadenceEventKind::MailboxReplace
                        } else {
                            CadenceEventKind::MailboxPublish
                        },
                        source_sequence,
                        source_timestamp_us,
                        0,
                        received_at.elapsed().as_micros() as u64,
                        0,
                    );
                    render_metrics.mark_received(coalesced);
                }
                Err(err) => {
                    mailbox.close();
                    let err = err.to_string();
                    let event_generation = Arc::clone(&stream_generation);
                    let event_instance = instance.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if event_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        if let Some(instance) = event_instance.upgrade() {
                            apply_live_stream_disconnected(&instance, &err);
                        }
                    });
                    break;
                }
            }
        }
        mailbox.close();
        unregister_framebuffer_stream(&stream_control, generation);
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_realtime_stream(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    stream_generation: SharedRealtimeStreamGeneration,
    stream_control: SharedRealtimeStreamControl,
    frame_chart: SharedRealtimeFrameChart,
    host: String,
    generation: u64,
) {
    std::thread::spawn(move || {
        let mut history = RealtimeHistory::default();
        let mut stream = match connect_device_telemetry_stream(&host) {
            Ok(stream) => stream,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(instance) = instance.upgrade() {
                        apply_live_realtime_view(
                            &instance,
                            &realtime_view_from_history(&history, false, &err),
                            &frame_chart,
                        );
                    }
                });
                return;
            }
        };
        let control = match stream.control() {
            Ok(control) => control,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(instance) = instance.upgrade() {
                        apply_live_realtime_view(
                            &instance,
                            &realtime_view_from_history(&history, false, &err),
                            &frame_chart,
                        );
                    }
                });
                return;
            }
        };
        if stream_generation.load(Ordering::SeqCst) != generation {
            control.shutdown();
            return;
        }
        register_realtime_stream(&stream_control, generation, control);
        while stream_generation.load(Ordering::SeqCst) == generation {
            match stream.next_sample() {
                Ok(sample) => {
                    history.push(sample);
                    let view = realtime_view_from_history(&history, true, "");
                    let event_generation = Arc::clone(&stream_generation);
                    let event_instance = instance.clone();
                    let event_frame_chart = Arc::clone(&frame_chart);
                    let _ = slint::invoke_from_event_loop(move || {
                        if event_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        if let Some(instance) = event_instance.upgrade() {
                            apply_live_realtime_view(&instance, &view, &event_frame_chart);
                        }
                    });
                }
                Err(err) => {
                    let err = err.to_string();
                    let view = realtime_view_from_history(&history, false, &err);
                    let event_generation = Arc::clone(&stream_generation);
                    let event_instance = instance.clone();
                    let event_frame_chart = Arc::clone(&frame_chart);
                    let _ = slint::invoke_from_event_loop(move || {
                        if event_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        if let Some(instance) = event_instance.upgrade() {
                            apply_live_realtime_view(&instance, &view, &event_frame_chart);
                        }
                    });
                    break;
                }
            }
        }
        unregister_realtime_stream(&stream_control, generation);
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_library_sync(
    ui: slint::Weak<AppWindow>,
    browser: SharedLibraryBrowser,
    host: String,
) {
    std::thread::spawn(move || {
        let result = library::sync_library_catalog(&host);
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_sync_result(result);
            }
            if let Some(ui) = ui.upgrade() {
                apply_compiled_library_state(&ui, &browser);
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_sd_fetch(
    ui: slint::Weak<AppWindow>,
    browser: SharedSdBrowser,
    host: String,
    path: String,
    show_hidden: bool,
) {
    let loading_ui = ui.clone();
    let loading_browser = Arc::clone(&browser);
    let loading_path = path.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SD_FOLDER_LOADING_DELAY);
        let _ = slint::invoke_from_event_loop(move || {
            let revealed = loading_browser.lock().is_ok_and(|mut browser| {
                browser.reveal_loading_after(&loading_path, SD_FOLDER_LOADING_DELAY)
            });
            if revealed {
                if let Some(ui) = loading_ui.upgrade() {
                    apply_compiled_sd_state(&ui, &loading_browser);
                }
            }
        });
    });

    std::thread::spawn(move || {
        let result = fetch_sd_directory(&host, &path, show_hidden).map_err(|err| err.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_listing_if_current_policy(&path, show_hidden, result);
            }
            if let Some(ui) = ui.upgrade() {
                apply_compiled_sd_state(&ui, &browser);
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_sd_detail_fetch(
    ui: slint::Weak<AppWindow>,
    browser: SharedSdBrowser,
    host: String,
    request: sd_card::SdDetailRequest,
) {
    std::thread::spawn(move || {
        let result = fetch_sd_item_detail(&host, &request.path).map_err(|err| err.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_detail_result(&request.path, request.generation, result);
            }
            if let Some(ui) = ui.upgrade() {
                apply_compiled_sd_state(&ui, &browser);
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_framebuffer_capture(
    ui: slint::Weak<AppWindow>,
    capture_state: SharedFramebufferCapture,
    host: String,
) {
    std::thread::spawn(move || {
        let result = fetch_framebuffer_capture(&host).map_err(|err| err.to_string());
        let capture = result.as_ref().ok().cloned();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(capture) = capture {
                if let Ok(mut state) = capture_state.lock() {
                    *state = Some(capture);
                }
            }
            if let Some(ui) = ui.upgrade() {
                apply_compiled_framebuffer_capture_result(&ui, result);
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_save_framebuffer_capture(
    ui: slint::Weak<AppWindow>,
    capture_state: SharedFramebufferCapture,
) {
    std::thread::spawn(move || {
        let result = capture_state
            .lock()
            .ok()
            .and_then(|state| state.clone())
            .ok_or_else(|| "Capture a framebuffer before saving.".to_string())
            .and_then(|capture| save_framebuffer_capture_png(&capture));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                match result {
                    Ok(path) => apply_compiled_save_status(
                        &ui,
                        &format!("Saved framebuffer PNG to {}.", path.display()),
                        "",
                    ),
                    Err(err) => {
                        apply_compiled_save_status(&ui, "Framebuffer PNG save failed.", &err)
                    }
                }
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_profile_load(ui: slint::Weak<AppWindow>, path: String) {
    std::thread::spawn(move || {
        let result = load_profile_artifact(&path);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                apply_compiled_profile_result(&ui, result);
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn start_compiled_framebuffer_display_clock(
    ui: slint::Weak<AppWindow>,
    capture_state: SharedFramebufferCapture,
    stream_generation: SharedLiveStreamGeneration,
    mailbox: SharedFramebufferDisplayMailbox,
    display_state: SharedFramebufferDisplayState,
    render_metrics: Arc<FramebufferRenderMetrics>,
    stream_start: Instant,
    generation: u64,
) -> Option<FramebufferDisplayController> {
    let strong = ui.upgrade()?;
    let window = strong.window();
    let chrome_enabled = display_state
        .lock()
        .map(|state| state.chrome_enabled)
        .unwrap_or(false);
    let chrome_ui = ui.clone();
    let chrome_generation = Arc::clone(&stream_generation);
    let chrome_mailbox = Arc::clone(&mailbox);
    let chrome_state = Arc::clone(&display_state);
    let chrome_metrics = Arc::clone(&render_metrics);
    let tick_metrics = Arc::clone(&render_metrics);
    let clock = start_framebuffer_display_clock(window, move |tick| {
        tick_metrics.mark_display_tick(tick_metrics.clock_kind(), tick);
        consume_compiled_framebuffer_display(
            ui.clone(),
            Arc::clone(&capture_state),
            Arc::clone(&stream_generation),
            Arc::clone(&mailbox),
            Arc::clone(&display_state),
            Arc::clone(&tick_metrics),
            generation,
        );
    });
    render_metrics.set_clock_kind(clock.kind());
    let chrome_timer = chrome_enabled.then(|| {
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(1),
            move || {
                if chrome_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                let chrome = chrome_state.lock().ok().and_then(|state| {
                    state.last_received_at.map(|received_at| {
                        (
                            state.last_source_sequence,
                            state.last_source_timestamp_us,
                            state.recent_dirty_rects.clone(),
                            state.dirty_summary.clone(),
                            framebuffer_display_summary(
                                &chrome_mailbox,
                                &state,
                                &chrome_metrics,
                                stream_start,
                                received_at,
                            ),
                        )
                    })
                });
                let Some((sequence, timestamp_us, rects, dirty_summary, stream_summary)) = chrome
                else {
                    return;
                };
                chrome_metrics.cadence.record(
                    CadenceEventKind::ChromeRefresh,
                    sequence,
                    timestamp_us,
                    chrome_metrics.applied_serial.load(Ordering::Acquire),
                    0,
                    0,
                );
                if let Some(ui) = chrome_ui.upgrade() {
                    apply_compiled_dirty_rects(&ui, &rects, &dirty_summary);
                    apply_compiled_stream_summary(&ui, &stream_summary);
                }
            },
        );
        timer
    });
    Some(FramebufferDisplayController {
        clock,
        chrome_timer,
    })
}

#[cfg(feature = "compiled-ui")]
fn consume_compiled_framebuffer_display(
    ui: slint::Weak<AppWindow>,
    capture_state: SharedFramebufferCapture,
    stream_generation: SharedLiveStreamGeneration,
    mailbox: SharedFramebufferDisplayMailbox,
    display_state: SharedFramebufferDisplayState,
    render_metrics: Arc<FramebufferRenderMetrics>,
    generation: u64,
) {
    if stream_generation.load(Ordering::SeqCst) != generation {
        mailbox.close();
        return;
    }
    let Some(update) = mailbox.take() else {
        let _ = mailbox.complete_apply();
        return;
    };
    let Some(ui) = ui.upgrade() else {
        mailbox.close();
        return;
    };
    render_metrics.cadence.record(
        CadenceEventKind::MailboxTake,
        update.frame.sequence,
        update.frame.timestamp_us,
        render_metrics.applied_serial.load(Ordering::Acquire),
        update.received_at.elapsed().as_micros() as u64,
        0,
    );
    let geometry_changed = display_state
        .lock()
        .map(|mut state| {
            let geometry_changed = record_applied_frame(&mut state, &update.frame);
            state.last_received_at = Some(update.received_at);
            state.last_source_sequence = update.frame.sequence;
            state.last_source_timestamp_us = update.frame.timestamp_us;
            state.dirty_summary = dirty_rect_summary(&update.frame, state.recent_dirty_rects.len());
            geometry_changed
        })
        .unwrap_or_default();
    apply_compiled_framebuffer_stream_capture(
        &ui,
        &update.frame.capture,
        update.pixels,
        geometry_changed,
    );
    render_metrics.mark_applied(
        update.received_at,
        update.frame.sequence,
        update.frame.timestamp_us,
    );
    ui.window().request_redraw();
    render_metrics.cadence.record(
        CadenceEventKind::RedrawSubmit,
        update.frame.sequence,
        update.frame.timestamp_us,
        render_metrics.applied_serial.load(Ordering::Acquire),
        0,
        0,
    );
    if let Ok(mut state) = capture_state.lock() {
        *state = Some(update.frame.capture);
    }
    let _ = mailbox.complete_apply();
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_framebuffer_stream(
    ui: slint::Weak<AppWindow>,
    capture_state: SharedFramebufferCapture,
    stream_generation: SharedLiveStreamGeneration,
    stream_control: SharedFramebufferStreamControl,
    render_metrics: Arc<FramebufferRenderMetrics>,
    mailbox: SharedFramebufferDisplayMailbox,
    host: String,
    generation: u64,
) {
    std::thread::spawn(move || {
        render_metrics.reset();
        let seed_capture = fetch_framebuffer_capture(&host).ok();
        if let Some(capture) = seed_capture.clone() {
            let event_generation = Arc::clone(&stream_generation);
            let event_capture_state = Arc::clone(&capture_state);
            let event_ui = ui.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if event_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Ok(mut state) = event_capture_state.lock() {
                    *state = Some(capture.clone());
                }
                if let Some(ui) = event_ui.upgrade() {
                    apply_compiled_framebuffer_capture_result(&ui, Ok(capture));
                }
            });
        }
        let mut stream = match connect_framebuffer_stream_seeded(&host, seed_capture.as_ref()) {
            Ok(stream) => stream,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(ui) = ui.upgrade() {
                        apply_compiled_stream_disconnected(&ui, &err);
                    }
                });
                return;
            }
        };
        let control = match stream.control() {
            Ok(control) => control,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(ui) = ui.upgrade() {
                        apply_compiled_stream_disconnected(&ui, &err);
                    }
                });
                return;
            }
        };
        if stream_generation.load(Ordering::SeqCst) != generation {
            control.shutdown();
            return;
        }
        register_framebuffer_stream(&stream_control, generation, control);
        while stream_generation.load(Ordering::SeqCst) == generation {
            match stream.next_frame() {
                Ok(frame) => {
                    let received_at = Instant::now();
                    render_metrics.cadence.record_at(
                        CadenceEventKind::SourceReceived,
                        frame.timing.read_complete,
                        frame.sequence,
                        frame.timestamp_us,
                        0,
                        0,
                        0,
                    );
                    render_metrics.cadence.record_at(
                        CadenceEventKind::DecodeComplete,
                        frame.timing.decompress_complete,
                        frame.sequence,
                        frame.timestamp_us,
                        0,
                        0,
                        0,
                    );
                    let pixels = framebuffer_capture_pixel_buffer(&frame.capture);
                    render_metrics.cadence.record(
                        CadenceEventKind::PixelBufferReady,
                        frame.sequence,
                        frame.timestamp_us,
                        0,
                        received_at.elapsed().as_micros() as u64,
                        0,
                    );
                    let (_, coalesced_before) = mailbox.stats();
                    let source_sequence = frame.sequence;
                    let source_timestamp_us = frame.timestamp_us;
                    let _ = mailbox.publish(FramebufferDisplayUpdate {
                        frame,
                        pixels,
                        received_at,
                    });
                    let (_, coalesced) = mailbox.stats();
                    render_metrics.cadence.record(
                        if coalesced > coalesced_before {
                            CadenceEventKind::MailboxReplace
                        } else {
                            CadenceEventKind::MailboxPublish
                        },
                        source_sequence,
                        source_timestamp_us,
                        0,
                        received_at.elapsed().as_micros() as u64,
                        0,
                    );
                    render_metrics.mark_received(coalesced);
                }
                Err(err) => {
                    mailbox.close();
                    let err = err.to_string();
                    let event_generation = Arc::clone(&stream_generation);
                    let event_ui = ui.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if event_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        if let Some(ui) = event_ui.upgrade() {
                            apply_compiled_stream_disconnected(&ui, &err);
                        }
                    });
                    break;
                }
            }
        }
        mailbox.close();
        unregister_framebuffer_stream(&stream_control, generation);
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_realtime_stream(
    ui: slint::Weak<AppWindow>,
    stream_generation: SharedRealtimeStreamGeneration,
    stream_control: SharedRealtimeStreamControl,
    frame_chart: SharedRealtimeFrameChart,
    host: String,
    generation: u64,
) {
    std::thread::spawn(move || {
        let mut history = RealtimeHistory::default();
        let mut stream = match connect_device_telemetry_stream(&host) {
            Ok(stream) => stream,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(ui) = ui.upgrade() {
                        apply_compiled_realtime_view(
                            &ui,
                            &realtime_view_from_history(&history, false, &err),
                            &frame_chart,
                        );
                    }
                });
                return;
            }
        };
        let control = match stream.control() {
            Ok(control) => control,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(ui) = ui.upgrade() {
                        apply_compiled_realtime_view(
                            &ui,
                            &realtime_view_from_history(&history, false, &err),
                            &frame_chart,
                        );
                    }
                });
                return;
            }
        };
        if stream_generation.load(Ordering::SeqCst) != generation {
            control.shutdown();
            return;
        }
        register_realtime_stream(&stream_control, generation, control);
        while stream_generation.load(Ordering::SeqCst) == generation {
            match stream.next_sample() {
                Ok(sample) => {
                    history.push(sample);
                    let view = realtime_view_from_history(&history, true, "");
                    let event_generation = Arc::clone(&stream_generation);
                    let event_ui = ui.clone();
                    let event_frame_chart = Arc::clone(&frame_chart);
                    let _ = slint::invoke_from_event_loop(move || {
                        if event_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        if let Some(ui) = event_ui.upgrade() {
                            apply_compiled_realtime_view(&ui, &view, &event_frame_chart);
                        }
                    });
                }
                Err(err) => {
                    let err = err.to_string();
                    let view = realtime_view_from_history(&history, false, &err);
                    let event_generation = Arc::clone(&stream_generation);
                    let event_ui = ui.clone();
                    let event_frame_chart = Arc::clone(&frame_chart);
                    let _ = slint::invoke_from_event_loop(move || {
                        if event_generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        if let Some(ui) = event_ui.upgrade() {
                            apply_compiled_realtime_view(&ui, &view, &event_frame_chart);
                        }
                    });
                    break;
                }
            }
        }
        unregister_realtime_stream(&stream_control, generation);
    });
}

fn start_window_drag(window: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    window.with_winit_window(|winit_window| {
        let _ = winit_window.drag_window();
    });
}

#[cfg(feature = "live-ui")]
fn start_reload_watcher(path: &Path, reload_requested: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    let path = path.to_path_buf();
    let initial_mtime = modified_time(&path);
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(500));
            if modified_time(&path) != initial_mtime {
                reload_requested.store(true, Ordering::Relaxed);
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
                break;
            }
        }
    });
}

#[cfg(feature = "live-ui")]
fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory_row() -> sd_card::SdTreeRow {
        sd_card::SdTreeRow {
            id: "/games".to_string(),
            label: "games".to_string(),
            icon_key: "folder-base".to_string(),
            level: 2,
            has_children: true,
            expanded: true,
            current: false,
            leading_is_directory: true,
            interactive: true,
            is_skeleton: false,
            loading_children_badge: "loading".to_string(),
        }
    }

    #[test]
    fn framebuffer_stream_bench_cli_parses_modes_limits_and_defaults() {
        assert_eq!(parse_framebuffer_stream_bench_args(&[]).unwrap(), None);
        assert_eq!(
            parse_framebuffer_stream_bench_args(&["--unrelated".to_string()]).unwrap(),
            None
        );
        assert_eq!(
            parse_framebuffer_stream_bench_args(&[
                "--framebuffer-stream-bench".to_string(),
                "0".to_string(),
            ])
            .unwrap(),
            Some((
                FramebufferBenchMode::Stream,
                FramebufferBenchLimit::Frames(1)
            ))
        );
        assert_eq!(
            parse_framebuffer_stream_bench_args(&[
                "--framebuffer-stream-drain-bench-secs".to_string(),
                "5".to_string(),
            ])
            .unwrap(),
            Some((
                FramebufferBenchMode::Drain,
                FramebufferBenchLimit::Duration(Duration::from_secs(5))
            ))
        );
        assert_eq!(
            parse_framebuffer_stream_bench_args(&[
                "--framebuffer-stream-dump".to_string(),
                "/tmp/frames".to_string(),
                "7".to_string(),
            ])
            .unwrap(),
            Some((
                FramebufferBenchMode::Dump(PathBuf::from("/tmp/frames")),
                FramebufferBenchLimit::Frames(7)
            ))
        );
    }

    #[test]
    fn framebuffer_stream_bench_cli_rejects_missing_or_invalid_values() {
        assert!(
            parse_framebuffer_stream_bench_args(&["--framebuffer-stream-dump".to_string()])
                .is_err()
        );
        assert!(
            parse_framebuffer_stream_bench_args(&[
                "--framebuffer-poll-bench".to_string(),
                "not-a-count".to_string(),
            ])
            .is_err()
        );
    }

    #[test]
    fn library_sync_failure_reducer_clears_transient_state_and_can_retry() {
        let mut browser = LibraryBrowser::new();
        browser.warning = "stale warning".to_string();

        browser.start_sync();
        assert!(browser.loading);
        assert!(browser.warning.is_empty());
        assert!(browser.last_error.is_empty());

        browser.apply_sync_result(Err("copy failed".to_string()));
        assert!(!browser.loading);
        assert_eq!(browser.status, "Library sync failed.");
        assert_eq!(browser.last_error, "copy failed");
        assert!(browser.warning.is_empty());

        browser.start_sync();
        assert!(browser.loading);
        assert!(browser.last_error.is_empty());
    }

    #[test]
    fn empty_library_query_reducer_handles_invalid_actions_without_selection() {
        let mut browser = LibraryBrowser::new();

        browser.set_sort("unknown", "descending");
        browser.set_filter("unknown", "value");
        browser.set_page(-10);
        browser.select_row("missing");

        assert_eq!(browser.query.page, 1);
        assert!(browser.selected_game().is_none());
        assert_eq!(browser.result_summary(), "No library loaded.");
    }

    fn telemetry_sample(seq: u64) -> DeviceTelemetrySample {
        DeviceTelemetrySample {
            seq,
            combined_cpu_pct: 12.5,
            presentation: agent_client::PresentationTelemetrySample {
                available: true,
                captured_monotonic_us: seq.saturating_mul(1_000_000),
                owned_vblank_count: Some(seq as u32),
                presented_vblank_count: Some(seq as u32),
                repeated_vblank_count: Some(0),
                ownership_loss_count: Some(0),
                active_sequence: Some(seq as u16),
                magik_ownership: true,
                pending: false,
                lifetime_invariant_valid: true,
                error: String::new(),
            },
            cores: vec![
                agent_client::CpuCoreTelemetry {
                    label: "CPU0".to_string(),
                    busy_pct: 12.5,
                },
                agent_client::CpuCoreTelemetry {
                    label: "CPU1".to_string(),
                    busy_pct: 25.0,
                },
            ],
            memory: agent_client::MemoryTelemetry {
                total_kb: 1_000_000,
                magik_kb: 100_000,
                main_kb: 20_000,
                other_used_kb: 500_000,
                available_kb: 400_000,
                magik_pct: 10.0,
                other_used_pct: 50.0,
                available_pct: 40.0,
            },
            frame_budget: agent_client::FrameBudgetTelemetry {
                budget_us: 16_667,
                frames_total: seq,
                window_frames: 60,
                window_over_budget: 1,
                window_over_20ms: 1,
                window_over_33ms: 0,
                window_max_wall_us: 21_000,
                max_wall_us: 21_000,
                max_vsync_miss_streak: 1,
                window_prepare_us: 100,
                window_render_us: 200,
                window_custom_draw_us: 300,
                window_vsync_us: 400,
                window_present_us: 500,
                recent_frames: vec![agent_client::FrameBudgetFrameTelemetry {
                    frame: seq,
                    wall_us: 21_000,
                    prepare_us: 100,
                    render_us: 200,
                    custom_draw_us: 300,
                    vsync_us: 400,
                    present_us: 500,
                    cpu_prepare_us: 10,
                    cpu_render_us: 20,
                    cpu_custom_draw_us: 30,
                    cpu_vsync_us: 1,
                    cpu_present_us: 5,
                    process_cpu_us: 80,
                    vsync_source: "vsync".to_string(),
                    vsync_miss_streak: 1,
                }],
            },
            launcher: agent_client::LauncherTelemetry {
                status_current: true,
                idle: false,
                fps: "59.9 fps".to_string(),
                preview_cache_state: "exact".to_string(),
                ui_thread_cpu: Some(0),
            },
            magik: agent_client::ProcessTelemetry {
                pids: vec![42],
                rss_kb: 100_000,
                threads: 7,
            },
            main: agent_client::ProcessTelemetry {
                pids: vec![9],
                rss_kb: 20_000,
                threads: 1,
            },
            network: agent_client::NetworkTelemetry {
                rx_bytes_per_sec: 1024,
                tx_bytes_per_sec: 2048,
            },
            storage: agent_client::StorageTelemetry {
                available_bytes: 137_000_000_000,
                total_bytes: 512_000_000_000,
                available_pct: 26.8,
                device: "mmcblk0".to_string(),
                activity_valid: true,
                read_bytes_per_sec: 12_500_000,
                write_bytes_per_sec: 2_500_000,
                read_pct: 25.0,
                write_pct: 10.0,
            },
        }
    }

    #[test]
    fn realtime_history_caps_at_five_minutes() {
        let mut history = RealtimeHistory::default();
        for seq in 0..(REALTIME_HISTORY_CAPACITY as u64 + 5) {
            history.push(telemetry_sample(seq));
        }

        assert_eq!(history.samples.len(), REALTIME_HISTORY_CAPACITY);
        assert_eq!(history.samples.front().unwrap().seq, 5);
        assert_eq!(
            history.samples.back().unwrap().seq,
            REALTIME_HISTORY_CAPACITY as u64 + 4
        );
    }

    #[test]
    fn realtime_view_summarizes_latest_sample() {
        let mut history = RealtimeHistory::default();
        history.push(telemetry_sample(1));

        let view = realtime_view_from_history(&history, true, "");

        assert!(view.streaming);
        assert_eq!(view.cpu_history.len(), 1);
        assert_eq!(view.cpu0_path, "M 100.00 87.50");
        assert_eq!(view.cpu1_path, "M 100.00 75.00");
        assert_eq!(view.cpu_summary, "Combined 12.5%");
        assert_eq!(view.ui_thread_cpu, Some(0));
        assert_eq!(view.memory_total_label, "976.6 MiB");
        assert_eq!(view.memory_magik_label, "MagiK: 97.7 MiB");
        assert_eq!(view.memory_other_label, "Other: 488.3 MiB");
        assert_eq!(view.memory_available_label, "Available: 390.6 MiB");
        assert!(view.frame_history[0].alert);
        assert_eq!(view.frame_samples.len(), 1);
        assert_eq!(view.frame_samples[0].process_cpu_us, 80);
        assert!(!view.frame_samples[0].idle);
        assert_eq!(view.phases.len(), 5);
        assert_eq!(view.health_tiles.len(), 4);
        assert_eq!(view.health_tiles[0].title, "MagiK");
        assert_eq!(view.health_tiles[1].title, "Main");
        assert_eq!(view.health_tiles[2].title, "Scanout");
        assert_eq!(view.health_tiles[2].value, "Settling");
        assert_eq!(view.health_tiles[3].title, "Network");
        assert_eq!(view.storage_total_label, "512GB");
        assert_eq!(view.storage_used_label, "Used: 375GB");
        assert_eq!(view.storage_empty_label, "Free: 137GB");
        assert_eq!(view.storage_used_pct, 73.2);
        assert_eq!(
            view.storage_activity_summary,
            "Read 25.0% (12.5 MB/s) / Write 10.0% (2.5 MB/s)"
        );
        assert!(!view.storage_read_path.is_empty());
        assert!(!view.storage_write_path.is_empty());
    }

    #[test]
    fn scanout_health_uses_fpga_repeat_and_ownership_deltas() {
        let mut history = RealtimeHistory::default();
        history.push(telemetry_sample(10));
        history.push(telemetry_sample(11));
        let healthy = scanout_health_tile(&history);
        assert_eq!(healthy.value, "0 repeated");
        assert_eq!(healthy.state, "good");

        let mut repeated = telemetry_sample(12);
        repeated.presentation.owned_vblank_count = Some(12);
        repeated.presentation.presented_vblank_count = Some(11);
        repeated.presentation.repeated_vblank_count = Some(1);
        history.push(repeated);
        let dropped = scanout_health_tile(&history);
        assert_eq!(dropped.value, "1 repeated");
        assert_eq!(dropped.state, "bad");

        let mut lost = telemetry_sample(13);
        lost.presentation.presented_vblank_count = Some(12);
        lost.presentation.repeated_vblank_count = Some(1);
        lost.presentation.ownership_loss_count = Some(1);
        history.push(lost);
        let ownership = scanout_health_tile(&history);
        assert_eq!(ownership.value, "Ownership lost");
        assert_eq!(ownership.state, "bad");

        let mut unavailable = telemetry_sample(14);
        unavailable.presentation.available = false;
        unavailable.presentation.error = "missing capability".to_string();
        history.push(unavailable);
        let missing = scanout_health_tile(&history);
        assert_eq!(missing.value, "Unavailable");
        assert_eq!(missing.state, "warn");
    }

    #[test]
    fn realtime_view_waits_for_a_valid_storage_delta() {
        let mut sample = telemetry_sample(1);
        sample.storage.activity_valid = false;
        let mut history = RealtimeHistory::default();
        history.push(sample);

        let view = realtime_view_from_history(&history, true, "");
        assert_eq!(
            view.storage_activity_summary,
            "Waiting for SD activity samples."
        );
        assert!(view.storage_read_path.is_empty());
        assert!(view.storage_write_path.is_empty());
    }

    #[test]
    fn realtime_view_does_not_plot_aggregate_frame_budget_as_live_samples() {
        let mut sample = telemetry_sample(1);
        sample.frame_budget.recent_frames.clear();
        sample.frame_budget.window_frames = 60;
        sample.frame_budget.window_render_us = 12_935;

        let mut history = RealtimeHistory::default();
        history.push(sample);

        let view = realtime_view_from_history(&history, true, "");

        assert_eq!(view.frame_samples.len(), 0);
        assert_eq!(view.phases[1].label, "Render");
        assert_eq!(view.phases[1].us, 12_935);
    }

    #[test]
    fn realtime_view_plots_idle_frame_budget_as_time_markers() {
        let mut sample = telemetry_sample(1);
        sample.frame_budget.recent_frames.clear();
        sample.launcher.idle = true;

        let mut history = RealtimeHistory::default();
        history.push(sample);

        let view = realtime_view_from_history(&history, true, "");

        assert_eq!(
            view.frame_samples.len(),
            REALTIME_IDLE_FRAME_COLUMNS_PER_SAMPLE as usize
        );
        assert!(view.frame_samples.iter().all(|sample| sample.idle));
        assert!(view.frame_samples.iter().all(|sample| sample.wall_us == 0));
    }

    #[test]
    fn realtime_view_deduplicates_overlapping_frame_sample_batches() {
        let mut first = telemetry_sample(1);
        let mut second = telemetry_sample(2);
        first.frame_budget.recent_frames[0].frame = 42;
        second.frame_budget.recent_frames[0].frame = 42;

        let mut history = RealtimeHistory::default();
        history.push(first);
        history.push(second);

        let view = realtime_view_from_history(&history, true, "");

        assert_eq!(view.frame_samples.len(), 1);
        assert_eq!(view.frame_samples[0].frame, 42);
    }

    #[test]
    fn realtime_view_reports_nominal_sixty_fps_when_idle() {
        let mut history = RealtimeHistory::default();
        let mut sample = telemetry_sample(1);
        sample.launcher.idle = true;
        sample.launcher.fps = "0.0 fps".to_string();
        history.push(sample);

        let view = realtime_view_from_history(&history, true, "");

        assert_eq!(view.fps_summary, "60fps idle");
    }

    #[test]
    fn realtime_chart_path_right_aligns_and_clamps_values() {
        assert_eq!(realtime_chart_path(&[], 4), "");
        assert_eq!(
            realtime_chart_path(&[-10.0, 50.0, 125.0], 4),
            "M 33.33 100.00 L 66.67 50.00 L 100.00 0.00"
        );
    }

    #[cfg(feature = "live-ui")]
    #[test]
    fn live_tree_row_struct_preserves_directory_flags() {
        let value = live_tree_row_struct(&directory_row());

        assert!(matches!(
            value.get_field("leading-is-directory"),
            Some(slint_interpreter::Value::Bool(true))
        ));
        assert!(matches!(
            value.get_field("has-children"),
            Some(slint_interpreter::Value::Bool(true))
        ));
        assert!(matches!(
            value.get_field("loading-children-badge"),
            Some(slint_interpreter::Value::String(text)) if text.as_str() == "loading"
        ));
    }

    #[cfg(feature = "compiled-ui")]
    #[test]
    fn compiled_tree_row_preserves_directory_flags() {
        let value = compiled_tree_row(&directory_row());

        assert!(value.leading_is_directory);
        assert!(value.has_children);
        assert_eq!(value.loading_children_badge.as_str(), "loading");
    }

    #[test]
    fn byte_size_labels_use_kb_and_mb() {
        assert_eq!(format_byte_size(512), "512 B");
        assert_eq!(format_byte_size(1536), "2 KB");
        assert_eq!(format_byte_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn storage_labels_use_decimal_gb() {
        assert_eq!(format_storage_gb(512_000_000_000), "512GB");
        assert_eq!(format_storage_gb(137_400_000_000), "137GB");
    }

    #[test]
    fn storage_activity_summary_formats_idle_read_write_and_combined() {
        let mut storage = agent_client::StorageTelemetry::default();
        assert_eq!(
            storage_activity_summary(&storage),
            "Read 0.0% (0 B/s) / Write 0.0% (0 B/s)"
        );
        storage.read_bytes_per_sec = 1_048_576;
        storage.activity_valid = true;
        storage.read_pct = 2.1;
        assert_eq!(
            storage_activity_summary(&storage),
            "Read 2.1% (1.0 MB/s) / Write 0.0% (0 B/s)"
        );
        storage.read_bytes_per_sec = 0;
        storage.read_pct = 0.0;
        storage.write_bytes_per_sec = 1_048_576;
        storage.write_pct = 4.2;
        assert_eq!(
            storage_activity_summary(&storage),
            "Read 0.0% (0 B/s) / Write 4.2% (1.0 MB/s)"
        );
        storage.read_bytes_per_sec = 2_097_152;
        storage.read_pct = 4.2;
        assert_eq!(
            storage_activity_summary(&storage),
            "Read 4.2% (2.1 MB/s) / Write 4.2% (1.0 MB/s)"
        );
    }

    #[test]
    fn framebuffer_capture_status_includes_payload_and_raw_sizes() {
        let capture = agent_client::FramebufferCapture {
            png_path: std::path::PathBuf::from("/tmp/fb.png"),
            rgba_pixels: Vec::new(),
            raw_pixels: Vec::new(),
            raw_stride_bytes: 0,
            width: 960,
            height: 540,
            bpp: 16,
            raw_bytes: 1_036_800,
            payload_bytes: 10_212,
            encoding: "lz4-block-size-prepended".to_string(),
            png_bytes: 0,
            png_hex_bytes: 0,
            timing: agent_client::FramebufferCaptureTiming::default(),
        };

        assert_eq!(
            framebuffer_capture_status(&capture),
            "Captured 960x540 16bpp framebuffer (10 KB payload; 1012 KB raw; lz4-block-size-prepended)."
        );
    }

    #[test]
    fn framebuffer_capture_png_bytes_encodes_rgba_pixels() {
        let capture = agent_client::FramebufferCapture {
            png_path: std::path::PathBuf::new(),
            rgba_pixels: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
            raw_pixels: Vec::new(),
            raw_stride_bytes: 0,
            width: 2,
            height: 2,
            bpp: 16,
            raw_bytes: 8,
            payload_bytes: 8,
            encoding: "lz4-block-size-prepended".to_string(),
            png_bytes: 0,
            png_hex_bytes: 0,
            timing: agent_client::FramebufferCaptureTiming::default(),
        };

        let png_bytes =
            framebuffer_capture_png_bytes(&capture).expect("RGBA pixels should encode as PNG");

        assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn framebuffer_stream_helpers_report_latency() {
        let latencies = [
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
        ];

        assert_eq!(latency_percentile_ms(&latencies, 0.50), 20.0);
    }

    #[test]
    fn framebuffer_display_bench_cli_parses_source_chrome_and_cadence_path() {
        let real = parse_framebuffer_display_bench_args(&[
            "--framebuffer-stream-display-bench-secs".to_string(),
            "12".to_string(),
            "--chrome".to_string(),
            "off".to_string(),
            "--cadence-out".to_string(),
            "/tmp/real.tsv".to_string(),
        ])
        .expect("real display args")
        .expect("real display benchmark");
        assert_eq!(real.source, FramebufferDisplayBenchSource::RealStream);
        assert_eq!(real.seconds, 12);
        assert!(!real.chrome);
        assert_eq!(real.cadence_out, Some(PathBuf::from("/tmp/real.tsv")));

        let synthetic = parse_framebuffer_display_bench_args(&[
            "--framebuffer-synthetic-display-bench-secs".to_string(),
            "3".to_string(),
        ])
        .expect("synthetic display args")
        .expect("synthetic display benchmark");
        assert_eq!(synthetic.source, FramebufferDisplayBenchSource::Synthetic);
        assert!(synthetic.chrome);
        assert_eq!(synthetic.cadence_out, None);
    }

    #[test]
    fn framebuffer_display_bench_cli_rejects_bad_chrome_value() {
        let error = parse_framebuffer_display_bench_args(&[
            "--framebuffer-synthetic-display-bench-secs".to_string(),
            "3".to_string(),
            "--chrome".to_string(),
            "sometimes".to_string(),
        ])
        .expect_err("bad chrome value should fail");

        assert!(error.to_string().contains("on or off"));
    }

    #[test]
    fn realtime_frame_chart_fixture_cli_accepts_only_small_or_large() {
        assert_eq!(
            parse_realtime_frame_chart_fixture_args(&[
                "--realtime-frame-chart-fixture".to_string(),
                "small".to_string(),
            ])
            .expect("small fixture"),
            Some(RealtimeFrameChartFixture::Small)
        );
        assert_eq!(
            parse_realtime_frame_chart_fixture_args(&[
                "--realtime-frame-chart-fixture".to_string(),
                "large".to_string(),
            ])
            .expect("large fixture"),
            Some(RealtimeFrameChartFixture::Large)
        );
        assert!(
            parse_realtime_frame_chart_fixture_args(&[
                "--realtime-frame-chart-fixture".to_string(),
                "unknown".to_string(),
            ])
            .is_err()
        );
    }

    #[cfg(feature = "compiled-ui")]
    #[test]
    fn synthetic_display_prebuilds_sixty_distinct_shared_pixel_buffers() {
        let frames = build_synthetic_display_frames();

        assert_eq!(frames.len(), 60);
        assert!(
            frames
                .iter()
                .all(|frame| frame.width() == 480 && frame.height() == 270)
        );
        assert_ne!(frames[0].as_bytes(), frames[1].as_bytes());
    }

    #[test]
    fn latest_mailbox_keeps_newest_value_and_only_arms_one_wake() {
        let mailbox = LatestMailbox::default();

        assert!(mailbox.publish(1));
        assert!(!mailbox.publish(2));
        assert_eq!(mailbox.stats(), (2, 1));
        assert_eq!(mailbox.take(), Some(2));
        assert!(!mailbox.complete_apply());
    }

    #[test]
    fn latest_mailbox_rearms_when_publish_arrives_during_apply() {
        let mailbox = LatestMailbox::default();

        assert!(mailbox.publish(1));
        assert_eq!(mailbox.take(), Some(1));
        assert!(!mailbox.publish(2));
        assert!(mailbox.complete_apply());
        assert_eq!(mailbox.take(), Some(2));
        assert!(!mailbox.complete_apply());
        assert!(mailbox.publish(3));
    }

    #[test]
    fn render_metrics_count_each_applied_serial_once() {
        let metrics = FramebufferRenderMetrics::default();
        metrics.supported.store(true, Ordering::Release);
        let applied_at = Instant::now();

        metrics.mark_applied(applied_at, 1, 100);
        metrics.mark_rendered(
            applied_at + Duration::from_millis(10),
            CadenceEventKind::AfterRendering,
        );
        metrics.mark_rendered(
            applied_at + Duration::from_millis(20),
            CadenceEventKind::AfterRendering,
        );

        let state = metrics.state.lock().expect("render metrics state");
        assert_eq!(state.last_rendered_serial, 1);
        assert_eq!(state.rendered_at.len(), 1);
        assert_eq!(state.latencies, [Duration::from_millis(10)]);
    }

    #[test]
    fn display_clock_tick_records_clock_timing_and_kind() {
        let metrics = FramebufferRenderMetrics::default();
        metrics.mark_display_tick(
            FramebufferDisplayClockKind::MacosDisplayLink,
            FramebufferDisplayClockTick {
                timestamp_us: 1_000,
                target_timestamp_us: 9_333,
                duration_us: 8_333,
            },
        );

        assert_eq!(
            metrics.clock_kind(),
            FramebufferDisplayClockKind::MacosDisplayLink
        );
        assert_eq!(metrics.display_ticks.load(Ordering::Relaxed), 1);
        let events = metrics.cadence.events();
        let event = events.last().expect("display tick event");
        assert_eq!(event.kind, CadenceEventKind::DisplayLinkTick);
        assert_eq!(event.source_timestamp_us, 1_000);
        assert_eq!(event.queue_age_us, 8_333);
        assert_eq!(event.value, 9_333);
    }

    #[test]
    fn display_clock_kind_labels_are_stable() {
        assert_eq!(
            FramebufferDisplayClockKind::from_u8(1).label(),
            "macos-cadisplaylink"
        );
        assert_eq!(
            FramebufferDisplayClockKind::from_u8(2).label(),
            "slint-timer"
        );
        assert_eq!(FramebufferDisplayClockKind::from_u8(99).label(), "unknown");
    }

    #[test]
    fn framebuffer_benchmark_readiness_requires_visible_after_rendering() {
        let metrics = FramebufferRenderMetrics::default();
        assert_eq!(
            metrics.benchmark_invalid_reason(),
            Some("rendering_notifier_not_ready")
        );

        metrics.observer_ready.store(true, Ordering::Release);
        metrics.set_clock_kind(if cfg!(target_os = "macos") {
            FramebufferDisplayClockKind::MacosDisplayLink
        } else {
            FramebufferDisplayClockKind::SlintTimer
        });
        assert_eq!(metrics.benchmark_invalid_reason(), Some("no_stream_frames"));
        metrics.received.store(1, Ordering::Release);
        assert_eq!(
            metrics.benchmark_invalid_reason(),
            Some("no_applied_frames")
        );
        metrics.applied.store(1, Ordering::Release);
        assert_eq!(metrics.benchmark_invalid_reason(), Some("window_unfocused"));
        metrics.focused.store(true, Ordering::Release);
        assert_eq!(
            metrics.benchmark_invalid_reason(),
            Some("zero_after_rendering")
        );
        metrics.rendered.store(1, Ordering::Release);

        assert!(metrics.benchmark_ready());
        assert_eq!(metrics.benchmark_invalid_reason(), None);
    }

    #[test]
    fn framebuffer_benchmark_remembers_focus_and_occlusion_failures() {
        let metrics = FramebufferRenderMetrics::default();
        metrics.observer_ready.store(true, Ordering::Release);
        metrics.set_clock_kind(if cfg!(target_os = "macos") {
            FramebufferDisplayClockKind::MacosDisplayLink
        } else {
            FramebufferDisplayClockKind::SlintTimer
        });
        metrics.received.store(1, Ordering::Release);
        metrics.applied.store(1, Ordering::Release);
        metrics.rendered.store(1, Ordering::Release);
        metrics.focused.store(true, Ordering::Release);
        metrics
            .lost_focus_during_measurement
            .store(true, Ordering::Release);

        assert_eq!(
            metrics.benchmark_invalid_reason(),
            Some("window_lost_focus")
        );

        metrics
            .lost_focus_during_measurement
            .store(false, Ordering::Release);
        metrics
            .occluded_during_measurement
            .store(true, Ordering::Release);
        assert_eq!(
            metrics.benchmark_invalid_reason(),
            Some("window_was_occluded")
        );
    }
}
