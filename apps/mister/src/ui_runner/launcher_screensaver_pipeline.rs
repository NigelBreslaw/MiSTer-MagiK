// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::launcher_screensaver::{
    LauncherScreenshotBuffer, LauncherScreenshotRuntime, ScreensaverFrameTrace, shared_parade_trace,
};
use super::*;
use mister_magik_screenshot_parade::LiveScreenshotPoll;

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
                let active_cards = frame.telemetry.stats.active_cards;
                let trace = shared_parade_trace(frame.telemetry.stats);
                RenderAheadPoll::Frame(RenderedScreensaverFrame {
                    pixels: frame.buffer.into_pixels(),
                    sequence: frame.tick,
                    completed_at: Instant::now(),
                    render_wall_us: frame.telemetry.timing.wall_us,
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
        let archive =
            crate::preview_worker::ResidentPreviewArchive::open(&path).expect("open fixture");
        let buffers = std::array::from_fn(|_| LauncherScreenshotBuffer::new(width, height));
        let mut runtime = mister_magik_screenshot_parade::LiveScreenshotParade::start(
            archive,
            mister_magik_screenshot_parade::LiveScreenshotConfig {
                geometry: mister_magik_framebuffer_scenes::SceneGeometry::new(width, height, width)
                    .unwrap(),
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
}
