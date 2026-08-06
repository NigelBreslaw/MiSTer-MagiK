// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared live screenshot runtime for production and measurement consumers.

use crate::reservoir::{
    STRICT_READY_CAPACITY, StrictFrameConsumer, StrictFramePoll, StrictFrameProducer,
    StrictFreeBufferPoll, StrictReadyFrame, strict_render_reservoir,
};
use crate::schedule::{
    ScreenshotParade, ScreenshotParadeConfig, ScreenshotParadeStats, WorkerStartCallback,
};
use crate::slack::{PreparationSlack, RenderPauseReceipt};
use mister_magik_catalog::preview_worker::ResidentPreviewArchive;
use mister_magik_framebuffer_scenes::{Rgb565Pixel, SceneGeometry};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const FREE_BUFFER_WAIT: Duration = Duration::from_millis(2);
const PREFILL_POLL_WAIT: Duration = Duration::from_millis(1);
const PREPARATION_PAUSE_LIMIT: Duration = Duration::from_millis(2);

pub trait ScreenshotBuffer: Send + 'static {
    fn pixels_mut(&mut self) -> &mut [Rgb565Pixel];
}

impl ScreenshotBuffer for Vec<Rgb565Pixel> {
    fn pixels_mut(&mut self) -> &mut [Rgb565Pixel] {
        self
    }
}

#[derive(Clone)]
pub struct LiveScreenshotConfig {
    pub geometry: SceneGeometry,
    pub seed: u64,
    pub scale_worker_start: Option<WorkerStartCallback>,
    pub render_worker_start: Option<WorkerStartCallback>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScreenshotRenderTiming {
    pub wall_us: u64,
}

#[derive(Debug)]
pub struct ReadyScreenshotFrame<B> {
    pub tick: u64,
    pub buffer: B,
    pub render_started: Instant,
    pub telemetry: ScreenshotFrameTelemetry,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScreenshotFrameTelemetry {
    pub tick: u64,
    pub fifo_ready_depth: usize,
    pub fifo_starvations: u64,
    pub fifo_sequence_failures: u64,
    pub timing: ScreenshotRenderTiming,
    pub pause: RenderPauseReceipt,
    pub stats: ScreenshotParadeStats,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenshotSequenceFailure {
    pub expected_tick: u64,
    pub actual_tick: u64,
}

#[derive(Debug)]
// The frame variant intentionally carries complete telemetry inline. Boxing it
// would add a heap allocation to every 60 Hz presentation tick.
#[allow(clippy::large_enum_variant)]
pub enum LiveScreenshotPoll<B> {
    Prefilling,
    Frame(ReadyScreenshotFrame<B>),
    Starved,
    SequenceFailure(ScreenshotSequenceFailure),
    Stopped,
}

struct RenderedScreenshotFrame<B> {
    buffer: B,
    render_started: Instant,
    timing: ScreenshotRenderTiming,
    pause: RenderPauseReceipt,
    stats: ScreenshotParadeStats,
}

pub struct LiveScreenshotParade<B> {
    reservoir: StrictFrameConsumer<B, RenderedScreenshotFrame<B>>,
    preparation_slack: Arc<PreparationSlack>,
    expected_tick: u64,
    leased_tick: Option<u64>,
    prefilling: bool,
    stopped: Arc<AtomicBool>,
    render_worker: Option<JoinHandle<()>>,
}

impl<B: ScreenshotBuffer> LiveScreenshotParade<B> {
    pub fn start(
        archive: ResidentPreviewArchive,
        config: LiveScreenshotConfig,
        buffers: [B; 3],
    ) -> Result<Self, String> {
        let preparation_slack = Arc::new(PreparationSlack::new());
        let parade = ScreenshotParade::new(
            archive,
            ScreenshotParadeConfig {
                geometry: config.geometry,
                seed: config.seed,
                worker_start: config.scale_worker_start,
                preparation_slack: Some(Arc::clone(&preparation_slack)),
            },
        )?;
        let (producer, reservoir) = strict_render_reservoir(buffers, 0);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker_slack = Arc::clone(&preparation_slack);
        let render_worker = std::thread::Builder::new()
            .name("screenshot-parade-render".to_owned())
            .spawn(move || {
                let _completion = RenderCompletionGuard(worker_stopped);
                if let Some(callback) = config.render_worker_start {
                    callback();
                }
                run_render_worker(parade, producer, &worker_slack);
            })
            .map_err(|error| format!("spawn screenshot parade render worker: {error}"))?;
        Ok(Self {
            reservoir,
            preparation_slack,
            expected_tick: 0,
            leased_tick: None,
            prefilling: true,
            stopped,
            render_worker: Some(render_worker),
        })
    }

    pub fn wait_until_prefilled(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        while self.reservoir.ready_depth() < STRICT_READY_CAPACITY {
            if self.stopped.load(Ordering::Acquire) {
                return Err("screenshot runtime stopped before prefill".to_owned());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "screenshot runtime prefill timed out depth={}",
                    self.reservoir.ready_depth()
                ));
            }
            std::thread::sleep(PREFILL_POLL_WAIT);
        }
        self.preparation_slack
            .set_ready_depth(self.reservoir.ready_depth());
        Ok(())
    }

    pub fn finish_prefill(&mut self) -> Result<(), String> {
        if self.reservoir.ready_depth() < STRICT_READY_CAPACITY {
            return Err(format!(
                "screenshot runtime cannot begin live before two frames are ready depth={}",
                self.reservoir.ready_depth()
            ));
        }
        self.prefilling = false;
        self.preparation_slack
            .set_ready_depth(self.reservoir.ready_depth());
        Ok(())
    }

    pub fn begin_live(&self) -> Result<(), String> {
        if self.prefilling || self.reservoir.ready_depth() < STRICT_READY_CAPACITY {
            return Err(format!(
                "screenshot runtime cannot gate preparation before live readiness depth={}",
                self.reservoir.ready_depth()
            ));
        }
        self.preparation_slack.begin_live();
        Ok(())
    }

    pub fn poll(&mut self) -> LiveScreenshotPoll<B> {
        if self.prefilling {
            return LiveScreenshotPoll::Prefilling;
        }
        if let Some(actual_tick) = self.leased_tick {
            return LiveScreenshotPoll::SequenceFailure(ScreenshotSequenceFailure {
                expected_tick: self.expected_tick,
                actual_tick,
            });
        }
        let result = match self.reservoir.try_next() {
            StrictFramePoll::Frame(frame) if frame.tick == self.expected_tick => {
                self.leased_tick = Some(frame.tick);
                LiveScreenshotPoll::Frame(ReadyScreenshotFrame {
                    tick: frame.tick,
                    buffer: frame.payload.buffer,
                    render_started: frame.payload.render_started,
                    telemetry: ScreenshotFrameTelemetry {
                        tick: frame.tick,
                        fifo_ready_depth: self.reservoir.ready_depth(),
                        fifo_starvations: self.reservoir.starvations(),
                        fifo_sequence_failures: self.reservoir.sequence_failures(),
                        timing: frame.payload.timing,
                        pause: frame.payload.pause,
                        stats: frame.payload.stats,
                    },
                })
            }
            StrictFramePoll::Frame(frame) => {
                LiveScreenshotPoll::SequenceFailure(ScreenshotSequenceFailure {
                    expected_tick: self.expected_tick,
                    actual_tick: frame.tick,
                })
            }
            StrictFramePoll::Empty => {
                self.reservoir.record_starvation();
                LiveScreenshotPoll::Starved
            }
            StrictFramePoll::Disconnected => LiveScreenshotPoll::Stopped,
            StrictFramePoll::SequenceFailure { frame, .. } => {
                LiveScreenshotPoll::SequenceFailure(ScreenshotSequenceFailure {
                    expected_tick: self.expected_tick,
                    actual_tick: frame.tick,
                })
            }
        };
        self.preparation_slack
            .set_ready_depth(self.reservoir.ready_depth());
        result
    }

    pub fn confirm_presented(&mut self, tick: u64) -> Result<(), ScreenshotSequenceFailure> {
        if self.leased_tick != Some(tick) || tick != self.expected_tick {
            return Err(ScreenshotSequenceFailure {
                expected_tick: self.expected_tick,
                actual_tick: tick,
            });
        }
        self.leased_tick = None;
        self.expected_tick = self.expected_tick.saturating_add(1);
        Ok(())
    }

    pub fn recycle_buffer(&self, buffer: B) -> bool {
        self.reservoir.recycle(buffer)
    }

    pub fn ready_depth(&self) -> usize {
        self.reservoir.ready_depth()
    }

    pub fn starvations(&self) -> u64 {
        self.reservoir.starvations()
    }

    pub fn sequence_failures(&self) -> u64 {
        self.reservoir.sequence_failures()
    }

    pub const fn expected_tick(&self) -> u64 {
        self.expected_tick
    }

    pub fn stop(&mut self) {
        self.cancel();
        if let Some(worker) = self.render_worker.take() {
            let _ = worker.join();
        }
    }

    pub fn cancel(&self) {
        self.reservoir.cancel();
        self.preparation_slack.cancel();
    }

    pub fn poll_stopped(&mut self) -> bool {
        if !self.stopped.load(Ordering::Acquire)
            && !self
                .render_worker
                .as_ref()
                .is_some_and(JoinHandle::is_finished)
        {
            return false;
        }
        if let Some(worker) = self.render_worker.take() {
            let _ = worker.join();
        }
        true
    }
}

impl<B> Drop for LiveScreenshotParade<B> {
    fn drop(&mut self) {
        self.reservoir.cancel();
        self.preparation_slack.cancel();
        if let Some(worker) = self.render_worker.take() {
            let _ = worker.join();
        }
    }
}

struct RenderCompletionGuard(Arc<AtomicBool>);

impl Drop for RenderCompletionGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn run_render_worker<B: ScreenshotBuffer>(
    mut parade: ScreenshotParade,
    producer: StrictFrameProducer<B, RenderedScreenshotFrame<B>>,
    preparation_slack: &PreparationSlack,
) {
    let mut tick = 0_u64;
    while !producer.is_cancelled() {
        let mut buffer = match producer.take_free_timeout(FREE_BUFFER_WAIT) {
            StrictFreeBufferPoll::Buffer(buffer) => buffer,
            StrictFreeBufferPoll::Timeout => continue,
            StrictFreeBufferPoll::Disconnected => break,
        };
        let (stats, render_started, timing, pause) = loop {
            let render_pause = preparation_slack.begin_render(PREPARATION_PAUSE_LIMIT);
            let pause = render_pause.receipt();
            let render_started = Instant::now();
            let result = parade.render_at_presentation_tick(buffer.pixels_mut(), tick);
            let timing = ScreenshotRenderTiming {
                wall_us: render_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
            };
            drop(render_pause);
            let Ok(stats) = result else {
                return;
            };
            if parade.is_ready() || producer.is_cancelled() {
                break (stats, render_started, timing, pause);
            }
            std::thread::park_timeout(FREE_BUFFER_WAIT);
        };
        if !producer.publish(StrictReadyFrame {
            tick,
            payload: RenderedScreenshotFrame {
                buffer,
                render_started,
                timing,
                pause,
                stats,
            },
        }) {
            break;
        }
        preparation_slack.set_ready_depth(producer.ready_depth());
        tick = tick.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn prefill_leases_confirmation_starvation_and_shutdown_are_explicit() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-live-screenshot-{}.mmlz4b",
            std::process::id()
        ));
        write_unique_layer_archive(&path);
        let archive = ResidentPreviewArchive::open(&path).expect("open live runtime fixture");
        let buffers = std::array::from_fn(|_| vec![Rgb565Pixel(0); 64 * 48]);
        let mut runtime = LiveScreenshotParade::start(
            archive,
            LiveScreenshotConfig {
                geometry: SceneGeometry::new(64, 48, 64).unwrap(),
                seed: 0x1234,
                scale_worker_start: None,
                render_worker_start: None,
            },
            buffers,
        )
        .expect("start live screenshot runtime");

        assert!(matches!(runtime.poll(), LiveScreenshotPoll::Prefilling));
        runtime
            .wait_until_prefilled(Duration::from_secs(2))
            .expect("prefill two real frames");
        runtime.finish_prefill().expect("finish prefill");

        let first = match runtime.poll() {
            LiveScreenshotPoll::Frame(frame) => frame,
            other => panic!("expected tick zero, got {other:?}"),
        };
        assert_eq!(first.tick, 0);
        assert!(matches!(
            runtime.poll(),
            LiveScreenshotPoll::SequenceFailure(ScreenshotSequenceFailure {
                expected_tick: 0,
                actual_tick: 0
            })
        ));
        assert_eq!(
            runtime.confirm_presented(1),
            Err(ScreenshotSequenceFailure {
                expected_tick: 0,
                actual_tick: 1
            })
        );
        runtime.confirm_presented(0).expect("confirm tick zero");

        let second = match runtime.poll() {
            LiveScreenshotPoll::Frame(frame) => frame,
            other => panic!("expected tick one, got {other:?}"),
        };
        assert_eq!(second.tick, 1);
        runtime.confirm_presented(1).expect("confirm tick one");

        let third = loop {
            match runtime.poll() {
                LiveScreenshotPoll::Frame(frame) => break frame,
                LiveScreenshotPoll::Starved => std::thread::yield_now(),
                other => panic!("expected tick two, got {other:?}"),
            }
        };
        assert_eq!(third.tick, 2);
        runtime.confirm_presented(2).expect("confirm tick two");
        let starvation_before = runtime.starvations();
        assert!(matches!(runtime.poll(), LiveScreenshotPoll::Starved));
        assert_eq!(runtime.expected_tick(), 3);
        assert_eq!(runtime.sequence_failures(), 0);
        assert_eq!(runtime.starvations(), starvation_before.saturating_add(1));

        drop((first, second, third));
        runtime.stop();
        let _ = std::fs::remove_file(path);
    }

    fn write_unique_layer_archive(path: &Path) {
        const IMAGE_COUNT: usize = 5;
        let pixels = [0x00_u8, 0xf8, 0xe0, 0x07, 0x1f, 0x00, 0xff, 0xff];
        let names = (0..IMAGE_COUNT)
            .map(|index| format!("fixture-{index}.rgb565").into_bytes())
            .collect::<Vec<_>>();
        let index_len = 8
            + 4
            + names
                .iter()
                .map(|name| 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len())
                .sum::<usize>();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MMPX2B1\0");
        bytes.extend_from_slice(&(IMAGE_COUNT as u32).to_le_bytes());
        for (index, name) in names.iter().enumerate() {
            bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&2_u32.to_le_bytes());
            bytes.extend_from_slice(&2_u32.to_le_bytes());
            bytes.extend_from_slice(&4_u32.to_le_bytes());
            bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
            bytes.push(1);
            bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
            let offset = index_len + index * pixels.len();
            bytes.extend_from_slice(&(offset as u64).to_le_bytes());
            bytes.extend_from_slice(name);
        }
        for _ in 0..IMAGE_COUNT {
            bytes.extend_from_slice(&pixels);
        }
        std::fs::write(path, bytes).expect("write live runtime fixture");
    }
}
