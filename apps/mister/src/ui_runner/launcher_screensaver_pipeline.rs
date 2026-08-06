// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_screensaver::{
    LauncherScreensaver, LauncherScreenshotBuffer, LauncherScreenshotRuntime,
    ScreensaverFrameTrace, shared_parade_trace,
};
use super::*;
use mister_magik_screenshot_parade::LiveScreenshotPoll;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel,
};
use std::thread::JoinHandle;

const RENDER_AHEAD_IDLE_WAIT: Duration = Duration::from_millis(2);
const RENDER_AHEAD_FULL_WAIT: Duration = Duration::from_micros(250);

fn allocate_render_ahead_buffer(width: usize, height: usize) -> Vec<Rgb565Pixel> {
    vec![Rgb565Pixel(0); width.saturating_mul(height)]
}

trait DirectRenderTarget {
    fn pixels_mut(&mut self) -> &mut [Rgb565Pixel];
    fn publish_writes(&mut self);
}

impl DirectRenderTarget for ScanoutSlotsRgb565Framebuffer {
    fn pixels_mut(&mut self) -> &mut [Rgb565Pixel] {
        ScanoutSlotsRgb565Framebuffer::pixels_mut(self)
    }

    fn publish_writes(&mut self) {
        ScanoutSlotsRgb565Framebuffer::publish_writes(self);
    }
}

fn render_and_publish_direct_target<T>(
    target: &mut T,
    render: impl FnOnce(&mut [Rgb565Pixel]) -> ScreensaverFrameTrace,
) -> ScreensaverFrameTrace
where
    T: DirectRenderTarget,
{
    let trace = render(target.pixels_mut());
    target.publish_writes();
    trace
}

pub(crate) struct RenderedScreensaverFrame {
    pub(crate) pixels: Vec<Rgb565Pixel>,
    pub(crate) sequence: u64,
    pub(crate) completed_at: Instant,
    pub(crate) render_wall_us: u64,
    pub(crate) render_cpu_us: u64,
    pub(crate) active_cards: usize,
    pub(crate) archive_loading: bool,
    pub(crate) has_rendered_card: bool,
    pub(crate) superseded_frames: u64,
    pub(crate) trace: ScreensaverFrameTrace,
}

pub(crate) enum RenderAheadPoll {
    Frame(RenderedScreensaverFrame),
    Empty,
    Disconnected,
    SequenceFailure {
        expected_tick: u64,
        actual_tick: u64,
        frame: RenderedScreensaverFrame,
    },
}

pub(crate) struct RenderedDirectScreensaverFrame {
    pub(crate) completed: CompletedHiddenFrame,
    pub(crate) sequence: u64,
    pub(crate) completed_at: Instant,
    pub(crate) render_wall_us: u64,
    pub(crate) render_cpu_us: u64,
    pub(crate) active_cards: usize,
    pub(crate) archive_loading: bool,
    pub(crate) has_rendered_card: bool,
    pub(crate) superseded_frames: u64,
    pub(crate) trace: ScreensaverFrameTrace,
}

pub(crate) enum DirectRenderAheadPoll {
    Frame(RenderedDirectScreensaverFrame),
    Empty,
    Disconnected,
}

pub(crate) struct ScreensaverRenderAhead {
    runtime: LauncherScreenshotRuntime,
    live: bool,
}

impl ScreensaverRenderAhead {
    pub(crate) const fn start(runtime: LauncherScreenshotRuntime) -> Self {
        Self {
            runtime,
            live: false,
        }
    }

    pub(crate) fn update_period_us(&self, _period_us: u64) {}

    pub(crate) fn try_next(&mut self) -> RenderAheadPoll {
        if !self.live {
            if let Err(error) = self.runtime.begin_live() {
                crate::ui_errln!("screensaver: shared runtime could not begin live: {error}");
                return RenderAheadPoll::Empty;
            }
            self.live = true;
        }
        match self.runtime.poll() {
            LiveScreenshotPoll::Frame(frame) => {
                let active_cards = frame.stats.active_cards;
                let trace = shared_parade_trace(frame.stats);
                RenderAheadPoll::Frame(RenderedScreensaverFrame {
                    pixels: frame.buffer.into_pixels(),
                    sequence: frame.tick,
                    completed_at: Instant::now(),
                    render_wall_us: frame.timing.wall_us,
                    render_cpu_us: 0,
                    active_cards,
                    archive_loading: false,
                    has_rendered_card: true,
                    superseded_frames: 0,
                    trace,
                })
            }
            LiveScreenshotPoll::Prefilling | LiveScreenshotPoll::Starved => RenderAheadPoll::Empty,
            LiveScreenshotPoll::Stopped => RenderAheadPoll::Disconnected,
            LiveScreenshotPoll::SequenceFailure(failure) => RenderAheadPoll::SequenceFailure {
                expected_tick: failure.expected_tick,
                actual_tick: failure.actual_tick,
                frame: RenderedScreensaverFrame {
                    pixels: Vec::new(),
                    sequence: failure.actual_tick,
                    completed_at: Instant::now(),
                    render_wall_us: 0,
                    render_cpu_us: 0,
                    active_cards: 0,
                    archive_loading: false,
                    has_rendered_card: false,
                    superseded_frames: 0,
                    trace: ScreensaverFrameTrace::default(),
                },
            },
        }
    }

    pub(crate) fn recycle(&self, pixels: Vec<Rgb565Pixel>) -> bool {
        self.runtime
            .recycle_buffer(LauncherScreenshotBuffer::from_pixels(pixels))
    }

    pub(crate) fn confirm_presented(&mut self, tick: u64) -> Result<(), String> {
        self.runtime.confirm_presented(tick).map_err(|failure| {
            format!(
                "expected_tick={} actual_tick={}",
                failure.expected_tick, failure.actual_tick
            )
        })
    }

    pub(crate) fn ready_depth(&self) -> usize {
        self.runtime.ready_depth()
    }

    pub(crate) fn cancel(&self) {
        self.runtime.cancel();
    }

    pub(crate) fn poll_stopped(&mut self) -> bool {
        self.runtime.poll_stopped()
    }
}

struct RenderAheadCompletionGuard(Arc<AtomicBool>);

impl Drop for RenderAheadCompletionGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(crate) struct ScreensaverDirectRenderAhead {
    grant_tx: SyncSender<HiddenSlotRenderGrant>,
    ready_rx: Receiver<RenderedDirectScreensaverFrame>,
    returned_buffers_rx: Receiver<PluginLatchFrameBuffers>,
    period_us: Arc<AtomicU64>,
    presentation_tick: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ScreensaverDirectRenderAhead {
    pub(crate) fn start(
        renderer: LauncherScreensaver,
        mut buffers: PluginLatchFrameBuffers,
        width: usize,
        height: usize,
        period_us: u64,
        launcher_snapshot_source: Option<&[Rgb565Pixel]>,
        fade_started: Option<Instant>,
    ) -> Self {
        let (grant_tx, grant_rx) = sync_channel(1);
        let (ready_tx, ready_rx) = sync_channel(1);
        let (returned_buffers_tx, returned_buffers_rx) = sync_channel(1);
        let (snapshot_allocated_tx, snapshot_allocated_rx) = sync_channel(1);
        let (snapshot_initialized_tx, snapshot_initialized_rx) = sync_channel(1);
        let snapshot_requested = launcher_snapshot_source.is_some();
        let period_us = Arc::new(AtomicU64::new(period_us.max(1)));
        let presentation_tick = Arc::new(AtomicU64::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_period_us = Arc::clone(&period_us);
        let worker_presentation_tick = Arc::clone(&presentation_tick);
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_stopped = Arc::clone(&stopped);
        let join = std::thread::Builder::new()
            .name("screensaver-render".into())
            .spawn(move || {
                let _completion = RenderAheadCompletionGuard(worker_stopped);
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverRenderer,
                );
                let run_result = (|| {
                    let launcher_snapshot = if snapshot_requested {
                        let snapshot = allocate_render_ahead_buffer(width, height);
                        snapshot_allocated_tx
                            .send(snapshot)
                            .map_err(|_| "direct preview snapshot receiver disconnected")?;
                        Some(
                            snapshot_initialized_rx
                                .recv()
                                .map_err(|_| "direct preview snapshot initialization failed")?,
                        )
                    } else {
                        None
                    };
                    run_direct_render_ahead_worker(
                        renderer,
                        &mut buffers,
                        width,
                        height,
                        launcher_snapshot,
                        fade_started,
                        grant_rx,
                        ready_tx,
                        &worker_period_us,
                        &worker_presentation_tick,
                        &worker_cancelled,
                    )
                })();
                let _ = returned_buffers_tx.send(buffers);
                if let Err(error) = run_result {
                    crate::ui_errln!("screensaver: direct hidden worker failed: {error}");
                }
            })
            .expect("spawn direct hidden screensaver worker");
        if let Some(source) = launcher_snapshot_source {
            match snapshot_allocated_rx.recv() {
                Ok(mut snapshot) if snapshot.len() == source.len() => {
                    snapshot.copy_from_slice(source);
                    let _ = snapshot_initialized_tx.send(snapshot);
                }
                Ok(snapshot) => {
                    crate::ui_errln!(
                        "screensaver: direct preview snapshot geometry mismatch allocated={} source={}",
                        snapshot.len(),
                        source.len()
                    );
                }
                Err(_) => {
                    crate::ui_errln!(
                        "screensaver: direct preview worker stopped before snapshot allocation"
                    );
                }
            }
        }
        Self {
            grant_tx,
            ready_rx,
            returned_buffers_rx,
            period_us,
            presentation_tick,
            cancelled,
            stopped,
            join: Some(join),
        }
    }

    pub(crate) fn update_period_us(&self, period_us: u64) {
        self.period_us.store(period_us.max(1), Ordering::Relaxed);
    }

    pub(crate) fn submit_grant(&self, grant: HiddenSlotRenderGrant) -> bool {
        self.grant_tx.try_send(grant).is_ok()
    }

    pub(crate) fn try_next_until(&self, deadline: Instant) -> DirectRenderAheadPoll {
        match self.ready_rx.try_recv() {
            Ok(frame) => return DirectRenderAheadPoll::Frame(frame),
            Err(TryRecvError::Disconnected) => return DirectRenderAheadPoll::Disconnected,
            Err(TryRecvError::Empty) => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return DirectRenderAheadPoll::Empty;
        }
        match self.ready_rx.recv_timeout(remaining) {
            Ok(frame) => DirectRenderAheadPoll::Frame(frame),
            Err(RecvTimeoutError::Timeout) => DirectRenderAheadPoll::Empty,
            Err(RecvTimeoutError::Disconnected) => DirectRenderAheadPoll::Disconnected,
        }
    }

    pub(crate) fn note_presented_period(&self) {
        self.presentation_tick.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub(crate) fn poll_stopped(&mut self) -> bool {
        if !self.stopped.load(Ordering::Acquire)
            && !self.join.as_ref().is_some_and(JoinHandle::is_finished)
        {
            return false;
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        true
    }

    pub(crate) fn take_returned_buffers(&mut self) -> Option<PluginLatchFrameBuffers> {
        self.returned_buffers_rx.try_recv().ok()
    }
}

impl Drop for ScreensaverDirectRenderAhead {
    fn drop(&mut self) {
        self.cancel();
        if self.stopped.load(Ordering::Acquire) {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_direct_render_ahead_worker(
    mut renderer: LauncherScreensaver,
    buffers: &mut PluginLatchFrameBuffers,
    width: usize,
    height: usize,
    launcher_snapshot: Option<Vec<Rgb565Pixel>>,
    fade_started: Option<Instant>,
    grant_rx: Receiver<HiddenSlotRenderGrant>,
    ready_tx: SyncSender<RenderedDirectScreensaverFrame>,
    period_us: &AtomicU64,
    presentation_tick: &AtomicU64,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let mut sequence = 0u64;
    let mut elapsed_us = 0u64;
    let mut motion_tick = 0u64;
    let mut superseded_frames = 0u64;
    while !cancelled.load(Ordering::Acquire) {
        let grant = match grant_rx.recv_timeout(RENDER_AHEAD_IDLE_WAIT) {
            Ok(grant) => grant,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        if grant.width != width
            || grant.height != height
            || grant.stride_pixels != width
            || !matches!(grant.slot_index, 1 | 2)
        {
            return Err(format!("invalid direct render grant: {grant:?}"));
        }
        let next_motion_tick =
            next_render_motion_tick(motion_tick, presentation_tick.load(Ordering::Acquire));
        superseded_frames = superseded_frames
            .saturating_add(next_motion_tick.saturating_sub(motion_tick.saturating_add(1)));
        let advanced_ticks = next_motion_tick.saturating_sub(motion_tick);
        motion_tick = next_motion_tick;
        elapsed_us = elapsed_us.saturating_add(
            period_us
                .load(Ordering::Relaxed)
                .max(1)
                .saturating_mul(advanced_ticks),
        );
        let next_elapsed = Duration::from_micros(
            elapsed_us.saturating_add(period_us.load(Ordering::Relaxed).max(1)),
        );
        sequence = sequence.wrapping_add(1);
        let target = buffers.buffer_mut(grant.slot_index);
        let wall_started = Instant::now();
        let cpu_started = thread_cpu_us();
        let trace = render_and_publish_direct_target(target, |pixels| {
            let trace = renderer.render_at_hidden_slot_presentation_tick(
                pixels,
                width,
                height,
                grant.slot_index,
                motion_tick,
                Duration::from_micros(elapsed_us),
                Some(next_elapsed),
            );
            if let (Some(snapshot), Some(started)) = (&launcher_snapshot, fade_started) {
                let alpha = (started.elapsed().as_micros().min(200_000) * 255 / 200_000) as u8;
                if alpha < 255 {
                    blend_rgb565_frame(pixels, snapshot, alpha);
                    renderer.invalidate_hidden_slot(grant.slot_index);
                }
            }
            trace
        });
        let frame = RenderedDirectScreensaverFrame {
            completed: CompletedHiddenFrame { grant },
            sequence,
            completed_at: Instant::now(),
            render_wall_us: wall_started
                .elapsed()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
            render_cpu_us: elapsed_thread_cpu_us(cpu_started),
            active_cards: renderer.active_card_count(),
            archive_loading: renderer.is_loading_archive(),
            has_rendered_card: renderer.has_rendered_card(),
            superseded_frames,
            trace,
        };
        if !send_direct_ready_frame(frame, &ready_tx, cancelled) {
            break;
        }
    }
    Ok(())
}

fn blend_rgb565_frame(target: &mut [Rgb565Pixel], launcher: &[Rgb565Pixel], alpha: u8) {
    let inverse = u32::from(255 - alpha);
    let alpha = u32::from(alpha);
    for (target, launcher) in target.iter_mut().zip(launcher) {
        let saver = u32::from(target.0);
        let base = u32::from(launcher.0);
        let red = (((base >> 11) & 0x1f) * inverse + ((saver >> 11) & 0x1f) * alpha + 127) / 255;
        let green = (((base >> 5) & 0x3f) * inverse + ((saver >> 5) & 0x3f) * alpha + 127) / 255;
        let blue = ((base & 0x1f) * inverse + (saver & 0x1f) * alpha + 127) / 255;
        target.0 = ((red << 11) | (green << 5) | blue) as u16;
    }
}

fn send_direct_ready_frame(
    mut frame: RenderedDirectScreensaverFrame,
    ready_tx: &SyncSender<RenderedDirectScreensaverFrame>,
    cancelled: &AtomicBool,
) -> bool {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        match ready_tx.try_send(frame) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                frame = returned;
                std::thread::park_timeout(RENDER_AHEAD_FULL_WAIT);
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

fn next_render_motion_tick(last_rendered_tick: u64, presented_periods: u64) -> u64 {
    last_rendered_tick
        .saturating_add(1)
        .max(presented_periods.saturating_add(1))
}

#[cfg(target_os = "linux")]
fn thread_cpu_us() -> Option<u64> {
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, time.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let time = unsafe { time.assume_init() };
    Some(
        u64::try_from(time.tv_sec)
            .unwrap_or(0)
            .saturating_mul(1_000_000)
            .saturating_add(u64::try_from(time.tv_nsec).unwrap_or(0) / 1_000),
    )
}

#[cfg(not(target_os = "linux"))]
fn thread_cpu_us() -> Option<u64> {
    None
}

fn elapsed_thread_cpu_us(start: Option<u64>) -> u64 {
    start
        .and_then(|start| thread_cpu_us().map(|end| end.saturating_sub(start)))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screenshot_runtime(width: usize, height: usize) -> LauncherScreenshotRuntime {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-render-ahead-{}-{width}x{height}.mmlz4b",
            std::process::id()
        ));
        let name = b"fixture.rgb565";
        let pixels = [0x00_u8, 0xf8, 0xe0, 0x07, 0x1f, 0x00, 0xff, 0xff];
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MMPX2B1\0");
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&pixels);
        std::fs::write(&path, bytes).expect("write render-ahead fixture");
        let archive = preview_worker::ResidentPreviewArchive::open(&path).expect("open fixture");
        let buffers = std::array::from_fn(|_| LauncherScreenshotBuffer::new(width, height));
        let mut runtime = mister_magik_screenshot_parade::LiveScreenshotParade::start(
            archive,
            mister_magik_screenshot_parade::LiveScreenshotConfig {
                geometry: SceneGeometry::new(width, height, width).unwrap(),
                seed: 0x1234,
                scale_worker_start: None,
                render_worker_start: None,
            },
            buffers,
        )
        .expect("construct screenshot runtime");
        runtime
            .wait_until_prefilled(Duration::from_secs(2))
            .expect("prefill screenshot runtime");
        runtime.finish_prefill().expect("finish prefill");
        let _ = std::fs::remove_file(path);
        runtime
    }

    fn direct_frame(sequence: u64) -> RenderedDirectScreensaverFrame {
        RenderedDirectScreensaverFrame {
            completed: CompletedHiddenFrame {
                grant: HiddenSlotRenderGrant {
                    slot_index: 1,
                    generation: sequence,
                    width: 64,
                    height: 48,
                    stride_pixels: 64,
                },
            },
            sequence,
            completed_at: Instant::now(),
            render_wall_us: 0,
            render_cpu_us: 0,
            active_cards: 0,
            archive_loading: false,
            has_rendered_card: false,
            superseded_frames: 0,
            trace: ScreensaverFrameTrace::default(),
        }
    }

    #[test]
    fn render_ahead_sequences_recycle_and_cancel_without_blocking() {
        let mut pipeline = ScreensaverRenderAhead::start(screenshot_runtime(64, 48));
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut sequences = Vec::new();
        while sequences.len() < 4 && Instant::now() < deadline {
            match pipeline.try_next() {
                RenderAheadPoll::Frame(frame) => {
                    sequences.push(frame.sequence);
                    assert_eq!(frame.pixels.len(), 64 * 48);
                    assert!(pipeline.ready_depth() <= 2);
                    pipeline
                        .confirm_presented(frame.sequence)
                        .expect("confirm frame");
                    assert!(pipeline.recycle(frame.pixels));
                }
                RenderAheadPoll::Empty => std::thread::yield_now(),
                RenderAheadPoll::Disconnected => panic!("worker disconnected before cancellation"),
                RenderAheadPoll::SequenceFailure { .. } => {
                    panic!("strict render-ahead sequence failure")
                }
            }
        }

        assert_eq!(sequences, vec![0, 1, 2, 3]);
        pipeline.update_period_us(16_667);
        pipeline.cancel();
        while !pipeline.poll_stopped() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(pipeline.poll_stopped());
    }

    #[test]
    fn render_ahead_supports_repeated_enter_and_exit() {
        for _ in 0..2 {
            let mut pipeline = ScreensaverRenderAhead::start(screenshot_runtime(32, 24));
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut saw_frame = false;
            while Instant::now() < deadline {
                match pipeline.try_next() {
                    RenderAheadPoll::Frame(_) => {
                        saw_frame = true;
                        break;
                    }
                    RenderAheadPoll::Empty => std::thread::yield_now(),
                    RenderAheadPoll::Disconnected => break,
                    RenderAheadPoll::SequenceFailure { .. } => break,
                }
            }
            assert!(saw_frame);
            pipeline.cancel();
            while !pipeline.poll_stopped() && Instant::now() < deadline {
                std::thread::yield_now();
            }
            assert!(pipeline.poll_stopped());
        }
    }

    #[test]
    fn direct_render_motion_tick_can_skip_obsolete_display_periods() {
        assert_eq!(next_render_motion_tick(0, 0), 1);
        assert_eq!(next_render_motion_tick(3, 1), 4);
        assert_eq!(next_render_motion_tick(3, 6), 7);
    }

    #[test]
    fn completion_guard_marks_worker_stopped_during_unwind() {
        let stopped = Arc::new(AtomicBool::new(false));
        drop(RenderAheadCompletionGuard(Arc::clone(&stopped)));
        assert!(stopped.load(Ordering::Acquire));
    }

    #[test]
    fn completion_guard_marks_an_actually_panicked_worker_stopped() {
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let join = std::thread::spawn(move || {
            let _completion = RenderAheadCompletionGuard(worker_stopped);
            panic!("test worker panic");
        });

        assert!(join.join().is_err());
        assert!(stopped.load(Ordering::Acquire));
    }

    #[test]
    fn worker_allocates_zeroed_exact_geometry_buffers() {
        let pixels = allocate_render_ahead_buffer(64, 48);
        assert_eq!(pixels.len(), 64 * 48);
        assert!(pixels.iter().all(|pixel| *pixel == Rgb565Pixel(0)));
    }

    #[test]
    fn direct_ready_channel_stops_on_disconnect_and_cancellation() {
        let cancelled = AtomicBool::new(false);
        let (disconnected_tx, disconnected_rx) = sync_channel(1);
        drop(disconnected_rx);
        assert!(!send_direct_ready_frame(
            direct_frame(1),
            &disconnected_tx,
            &cancelled
        ));

        let (ready_tx, _ready_rx) = sync_channel(1);
        ready_tx.send(direct_frame(1)).unwrap();
        cancelled.store(true, Ordering::Release);
        assert!(!send_direct_ready_frame(
            direct_frame(2),
            &ready_tx,
            &cancelled
        ));
    }

    #[test]
    fn direct_target_is_published_before_completion_can_be_built() {
        struct FakeDirectTarget {
            pixels: [Rgb565Pixel; 1],
            events: Vec<&'static str>,
        }

        impl DirectRenderTarget for FakeDirectTarget {
            fn pixels_mut(&mut self) -> &mut [Rgb565Pixel] {
                self.events.push("render");
                &mut self.pixels
            }

            fn publish_writes(&mut self) {
                self.events.push("publish");
            }
        }

        let mut target = FakeDirectTarget {
            pixels: [Rgb565Pixel(0)],
            events: Vec::new(),
        };
        render_and_publish_direct_target(&mut target, |pixels| {
            pixels[0] = Rgb565Pixel(0x5aa5);
            ScreensaverFrameTrace::default()
        });

        assert_eq!(target.events, ["render", "publish"]);
        assert_eq!(target.pixels, [Rgb565Pixel(0x5aa5)]);
    }

    #[test]
    fn direct_preview_blend_preserves_both_endpoints() {
        let launcher = [Rgb565Pixel(0x001f)];
        let mut target = [Rgb565Pixel(0xf800)];
        blend_rgb565_frame(&mut target, &launcher, 0);
        assert_eq!(target, launcher);

        target[0] = Rgb565Pixel(0xf800);
        blend_rgb565_frame(&mut target, &launcher, 255);
        assert_eq!(target, [Rgb565Pixel(0xf800)]);
    }
}
