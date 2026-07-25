// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_screensaver::{LauncherScreensaver, ScreensaverFrameTrace};
use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel,
};
use std::thread::JoinHandle;

const RENDER_AHEAD_BUFFER_COUNT: usize = 3;
const RENDER_AHEAD_READY_CAPACITY: usize = 2;
const RENDER_AHEAD_IDLE_WAIT: Duration = Duration::from_millis(2);
const RENDER_AHEAD_FULL_WAIT: Duration = Duration::from_micros(250);

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
}

pub(crate) struct ScreensaverRenderAhead {
    ready_rx: Receiver<RenderedScreensaverFrame>,
    free_tx: SyncSender<Vec<Rgb565Pixel>>,
    period_us: Arc<AtomicU64>,
    presentation_tick: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    ready_depth: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ScreensaverRenderAhead {
    pub(crate) fn start(
        renderer: LauncherScreensaver,
        width: usize,
        height: usize,
        period_us: u64,
    ) -> Self {
        let (free_tx, free_rx) = sync_channel(RENDER_AHEAD_BUFFER_COUNT);
        let (ready_tx, ready_rx) = sync_channel(RENDER_AHEAD_READY_CAPACITY);
        let period_us = Arc::new(AtomicU64::new(period_us.max(1)));
        let presentation_tick = Arc::new(AtomicU64::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let ready_depth = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_period_us = Arc::clone(&period_us);
        let worker_presentation_tick = Arc::clone(&presentation_tick);
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_ready_depth = Arc::clone(&ready_depth);
        let worker_stopped = Arc::clone(&stopped);
        let worker_free_tx = free_tx.clone();
        let join = std::thread::Builder::new()
            .name("screensaver-render".into())
            .spawn(move || {
                let _completion = RenderAheadCompletionGuard(worker_stopped);
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverRenderer,
                );
                for _ in 0..RENDER_AHEAD_BUFFER_COUNT {
                    if worker_cancelled.load(Ordering::Acquire)
                        || worker_free_tx
                            .send(vec![Rgb565Pixel(0); width.saturating_mul(height)])
                            .is_err()
                    {
                        return;
                    }
                }
                run_render_ahead_worker(
                    renderer,
                    width,
                    height,
                    free_rx,
                    ready_tx,
                    &worker_period_us,
                    &worker_presentation_tick,
                    &worker_cancelled,
                    &worker_ready_depth,
                );
            })
            .expect("spawn screensaver render-ahead worker");
        Self {
            ready_rx,
            free_tx,
            period_us,
            presentation_tick,
            cancelled,
            ready_depth,
            stopped,
            join: Some(join),
        }
    }

    pub(crate) fn update_period_us(&self, period_us: u64) {
        self.period_us.store(period_us.max(1), Ordering::Relaxed);
    }

    pub(crate) fn note_presented_period(&self) {
        self.presentation_tick.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn try_next(&self) -> RenderAheadPoll {
        match self.ready_rx.try_recv() {
            Ok(frame) => {
                self.ready_depth.fetch_sub(1, Ordering::AcqRel);
                RenderAheadPoll::Frame(frame)
            }
            Err(TryRecvError::Empty) => RenderAheadPoll::Empty,
            Err(TryRecvError::Disconnected) => RenderAheadPoll::Disconnected,
        }
    }

    pub(crate) fn recycle(&self, pixels: Vec<Rgb565Pixel>) -> bool {
        self.free_tx.try_send(pixels).is_ok()
    }

    pub(crate) fn ready_depth(&self) -> usize {
        self.ready_depth.load(Ordering::Acquire)
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
}

struct RenderAheadCompletionGuard(Arc<AtomicBool>);

impl Drop for RenderAheadCompletionGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Drop for ScreensaverRenderAhead {
    fn drop(&mut self) {
        self.cancel();
        if self.stopped.load(Ordering::Acquire) {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }
}

fn run_render_ahead_worker(
    mut renderer: LauncherScreensaver,
    width: usize,
    height: usize,
    free_rx: Receiver<Vec<Rgb565Pixel>>,
    ready_tx: SyncSender<RenderedScreensaverFrame>,
    period_us: &AtomicU64,
    presentation_tick: &AtomicU64,
    cancelled: &AtomicBool,
    ready_depth: &AtomicUsize,
) {
    let mut sequence = 0u64;
    let mut elapsed_us = 0u64;
    let mut motion_tick = 0u64;
    let mut superseded_frames = 0u64;
    while !cancelled.load(Ordering::Acquire) {
        let mut pixels = match free_rx.recv_timeout(RENDER_AHEAD_IDLE_WAIT) {
            Ok(pixels) => pixels,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        sequence = sequence.wrapping_add(1);
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
        let wall_started = Instant::now();
        let cpu_started = thread_cpu_us();
        let trace = renderer.render_at(
            &mut pixels,
            width,
            height,
            Duration::from_micros(elapsed_us),
        );
        let frame = RenderedScreensaverFrame {
            pixels,
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
        if !send_ready_frame(frame, &ready_tx, cancelled, ready_depth) {
            break;
        }
    }
}

fn send_ready_frame(
    mut frame: RenderedScreensaverFrame,
    ready_tx: &SyncSender<RenderedScreensaverFrame>,
    cancelled: &AtomicBool,
    ready_depth: &AtomicUsize,
) -> bool {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        ready_depth.fetch_add(1, Ordering::AcqRel);
        match ready_tx.try_send(frame) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                ready_depth.fetch_sub(1, Ordering::AcqRel);
                frame = returned;
                std::thread::park_timeout(RENDER_AHEAD_FULL_WAIT);
            }
            Err(TrySendError::Disconnected(_)) => {
                ready_depth.fetch_sub(1, Ordering::AcqRel);
                return false;
            }
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
    use crate::ui_runner::launcher_screensaver::LauncherScreensaverLoader;

    #[test]
    fn render_ahead_sequences_recycle_and_cancel_without_blocking() {
        let loader = LauncherScreensaverLoader::start(64, 48, None, false);
        let renderer = loader
            .try_ready()
            .expect("renderer is handed off immediately");
        let mut pipeline = ScreensaverRenderAhead::start(renderer, 64, 48, 20_000);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut sequences = Vec::new();
        while sequences.len() < 4 && Instant::now() < deadline {
            match pipeline.try_next() {
                RenderAheadPoll::Frame(frame) => {
                    sequences.push(frame.sequence);
                    assert_eq!(frame.pixels.len(), 64 * 48);
                    assert!(pipeline.ready_depth() <= RENDER_AHEAD_READY_CAPACITY);
                    pipeline.note_presented_period();
                    assert!(pipeline.recycle(frame.pixels));
                }
                RenderAheadPoll::Empty => std::thread::yield_now(),
                RenderAheadPoll::Disconnected => panic!("worker disconnected before cancellation"),
            }
        }

        assert_eq!(sequences, vec![1, 2, 3, 4]);
        pipeline.update_period_us(16_667);
        pipeline.cancel();
        while !pipeline.poll_stopped() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(pipeline.poll_stopped());
    }

    #[test]
    fn render_motion_tick_skips_obsolete_display_periods() {
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
}
