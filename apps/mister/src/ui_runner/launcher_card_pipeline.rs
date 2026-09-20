// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded newest-wins render-ahead for the native root-card launcher.

use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::launcher::{
    LauncherFramePreparer, LauncherFrameRequest, PreparedLauncherFrame,
};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

const WIDTH: usize = 960;
const HEIGHT: usize = 540;
const CAROUSEL_TOP: usize = 120;
const CAROUSEL_BOTTOM: usize = 495;
const CAROUSEL_LEFT: usize = 296;
const CAROUSEL_SPLIT: usize = 615;
const CAROUSEL_RIGHT: usize = 934;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CardFrameRequest {
    pub(super) render: LauncherFrameRequest,
    pub(super) content_generation: u64,
    pub(super) navigation_generation: u64,
}

#[derive(Debug)]
pub(super) struct RenderedCardFrame {
    request: CardFrameRequest,
    pixels: Vec<Rgb565Pixel>,
}

impl RenderedCardFrame {
    pub(super) const fn request(&self) -> CardFrameRequest {
        self.request
    }

    pub(super) fn pixels(&self) -> &[Rgb565Pixel] {
        &self.pixels
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct CardRenderAheadStats {
    pub(super) submitted: u64,
    pub(super) coalesced: u64,
    pub(super) completed: u64,
    pub(super) ready_replaced: u64,
    pub(super) stale_discarded: u64,
}

struct PipelineState {
    pending: Option<CardFrameRequest>,
    ready: Option<RenderedCardFrame>,
    free: Vec<Vec<Rgb565Pixel>>,
    shutdown: bool,
    stats: CardRenderAheadStats,
}

struct SharedPipeline {
    state: Mutex<PipelineState>,
    wake: Condvar,
}

enum TileRequest {
    Render(LauncherFrameRequest, PreparedLauncherFrame),
    Stop,
}

pub(super) struct LauncherCardRenderAhead {
    shared: Arc<SharedPipeline>,
    coordinator: Option<JoinHandle<()>>,
}

impl LauncherCardRenderAhead {
    pub(super) fn start(preparer: LauncherFramePreparer, static_frame: &[Rgb565Pixel]) -> Self {
        assert_eq!(static_frame.len(), WIDTH * HEIGHT);
        let shared = Arc::new(SharedPipeline {
            state: Mutex::new(PipelineState {
                pending: None,
                ready: None,
                free: vec![static_frame.to_vec(), static_frame.to_vec()],
                shutdown: false,
                stats: CardRenderAheadStats::default(),
            }),
            wake: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let coordinator = std::thread::Builder::new()
            .name("launcher-card-ahead".into())
            .spawn(move || run_coordinator(worker_shared, preparer))
            .expect("failed to start launcher card render-ahead worker");
        Self {
            shared,
            coordinator: Some(coordinator),
        }
    }

    pub(super) fn submit(&self, request: CardFrameRequest) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.stats.submitted = state.stats.submitted.saturating_add(1);
        if state.pending.replace(request).is_some() {
            state.stats.coalesced = state.stats.coalesced.saturating_add(1);
        }
        self.shared.wake.notify_one();
    }

    pub(super) fn try_take(
        &self,
        content_generation: u64,
        navigation_generation: u64,
        now_us: u64,
        maximum_age_us: u64,
    ) -> Option<RenderedCardFrame> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let frame = state.ready.take()?;
        let request = frame.request;
        let stale = request.content_generation != content_generation
            || request.navigation_generation != navigation_generation
            || now_us.saturating_sub(request.render.timestamp_us) > maximum_age_us;
        if stale {
            state.stats.stale_discarded = state.stats.stale_discarded.saturating_add(1);
            state.free.push(frame.pixels);
            self.shared.wake.notify_one();
            None
        } else {
            Some(frame)
        }
    }

    pub(super) fn recycle(&self, frame: RenderedCardFrame) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.free.push(frame.pixels);
        self.shared.wake.notify_one();
    }

    pub(super) fn stats(&self) -> CardRenderAheadStats {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stats
    }
}

impl Drop for LauncherCardRenderAhead {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.shutdown = true;
            state.pending = None;
        }
        self.shared.wake.notify_all();
        if let Some(coordinator) = self.coordinator.take() {
            let _ = coordinator.join();
        }
    }
}

fn run_coordinator(shared: Arc<SharedPipeline>, preparer: LauncherFramePreparer) {
    apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardRenderer);
    let (tile_request_tx, tile_request_rx) = sync_channel(1);
    let (tile_completed_tx, tile_completed_rx) = sync_channel(1);
    let secondary_preparer = preparer.clone();
    let secondary = std::thread::Builder::new()
        .name("launcher-card-tile".into())
        .spawn(move || {
            run_secondary_tile_worker(secondary_preparer, tile_request_rx, tile_completed_tx)
        })
        .expect("failed to start secondary launcher card renderer");
    let mut left = preparer.new_tile_buffer();
    let mut right = Some(preparer.new_tile_buffer());

    loop {
        let (request, mut output) = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while !state.shutdown && (state.pending.is_none() || state.free.is_empty()) {
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            if state.shutdown {
                break;
            }
            (
                state.pending.take().expect("pending request checked"),
                state.free.pop().expect("free frame checked"),
            )
        };

        let Some(right_buffer) = right.take() else {
            break;
        };
        if tile_request_tx
            .send(TileRequest::Render(request.render, right_buffer))
            .is_err()
        {
            break;
        }
        preparer.render_tile(request.render, &mut left, (CAROUSEL_LEFT, CAROUSEL_SPLIT));
        let Ok(right_buffer) = tile_completed_rx.recv() else {
            break;
        };
        compose_tiles(&mut output, left.pixels(), right_buffer.pixels());
        right = Some(right_buffer);

        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.stats.completed = state.stats.completed.saturating_add(1);
        if let Some(old) = state.ready.replace(RenderedCardFrame {
            request,
            pixels: output,
        }) {
            state.stats.ready_replaced = state.stats.ready_replaced.saturating_add(1);
            state.free.push(old.pixels);
        }
        shared.wake.notify_one();
    }

    let _ = tile_request_tx.send(TileRequest::Stop);
    let _ = secondary.join();
}

fn run_secondary_tile_worker(
    preparer: LauncherFramePreparer,
    requests: Receiver<TileRequest>,
    completed: SyncSender<PreparedLauncherFrame>,
) {
    apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardRendererSecondary);
    while let Ok(request) = requests.recv() {
        match request {
            TileRequest::Render(request, mut buffer) => {
                preparer.render_tile(request, &mut buffer, (CAROUSEL_SPLIT, CAROUSEL_RIGHT));
                if completed.send(buffer).is_err() {
                    break;
                }
            }
            TileRequest::Stop => break,
        }
    }
}

fn compose_tiles(output: &mut [Rgb565Pixel], left: &[Rgb565Pixel], right: &[Rgb565Pixel]) {
    for y in CAROUSEL_TOP..CAROUSEL_BOTTOM {
        output[y * WIDTH + CAROUSEL_LEFT..y * WIDTH + CAROUSEL_SPLIT]
            .copy_from_slice(&left[y * WIDTH + CAROUSEL_LEFT..y * WIDTH + CAROUSEL_SPLIT]);
        output[y * WIDTH + CAROUSEL_SPLIT..y * WIDTH + CAROUSEL_RIGHT]
            .copy_from_slice(&right[y * WIDTH + CAROUSEL_SPLIT..y * WIDTH + CAROUSEL_RIGHT]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_framebuffer_scenes::launcher::{
        LauncherCard, LauncherCardId, LauncherData, LauncherScene,
    };
    use mister_magik_framebuffer_scenes::launcher_navigation::{
        BrowseDirection, BrowseFrame, BrowsePhase,
    };
    use std::time::{Duration, Instant};

    fn prepared() -> mister_magik_framebuffer_scenes::launcher::PreparedLauncher {
        let cards = [
            LauncherCard {
                id: LauncherCardId::Arcade,
                name: "Arcade",
                games: Some(12),
                colour: 0xf800,
            },
            LauncherCard {
                id: LauncherCardId::Consoles,
                name: "Consoles",
                games: Some(34),
                colour: 0x07e0,
            },
        ];
        LauncherScene::new(WIDTH, HEIGHT).prepare(LauncherData {
            cards: &cards,
            selected: 0,
            library_games: 46,
            collections: 2,
            favourites: 3,
            clock: "21:37",
        })
    }

    fn request(sequence: u64, timestamp_us: u64, progress_millis: u32) -> CardFrameRequest {
        CardFrameRequest {
            render: LauncherFrameRequest {
                frame: BrowseFrame {
                    selected: 0,
                    target: 1,
                    phase: BrowsePhase::Flipping,
                    direction: Some(BrowseDirection::Right),
                    progress_millis,
                    duration_millis: 180,
                    outgoing: None,
                },
                timestamp_us,
                generation: sequence,
            },
            content_generation: 4,
            navigation_generation: 7,
        }
    }

    fn wait_for(pipeline: &LauncherCardRenderAhead, sequence: u64) -> RenderedCardFrame {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(frame) = pipeline.try_take(4, 7, 1_000_000, 1_000_000) {
                if frame.request().render.generation == sequence {
                    return frame;
                }
                pipeline.recycle(frame);
            }
            assert!(Instant::now() < deadline, "render-ahead worker timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn completed_frame_matches_serial_renderer() {
        let mut serial = prepared();
        let pipeline = LauncherCardRenderAhead::start(serial.frame_preparer(), serial.pixels());
        let request = request(1, 900_000, 90);
        pipeline.submit(request);
        let frame = wait_for(&pipeline, 1);
        serial.render_frame(request.render.frame);
        assert_eq!(frame.pixels(), serial.pixels());
        pipeline.recycle(frame);
    }

    #[test]
    fn latest_pending_and_ready_frames_win() {
        let serial = prepared();
        let pipeline = LauncherCardRenderAhead::start(serial.frame_preparer(), serial.pixels());
        for sequence in 1..=8 {
            pipeline.submit(request(
                sequence,
                900_000 + sequence,
                u32::try_from(sequence * 10).unwrap(),
            ));
        }
        let frame = wait_for(&pipeline, 8);
        assert_eq!(frame.request().render.generation, 8);
        assert!(pipeline.stats().coalesced > 0 || pipeline.stats().ready_replaced > 0);
        pipeline.recycle(frame);
    }

    #[test]
    fn stale_generation_is_discarded_and_recycled() {
        let serial = prepared();
        let pipeline = LauncherCardRenderAhead::start(serial.frame_preparer(), serial.pixels());
        pipeline.submit(request(1, 900_000, 90));
        let deadline = Instant::now() + Duration::from_secs(2);
        while pipeline.stats().completed == 0 {
            assert!(Instant::now() < deadline, "render-ahead worker timed out");
            std::thread::yield_now();
        }
        assert!(pipeline.try_take(4, 8, 1_000_000, 1_000_000).is_none());
        assert_eq!(pipeline.stats().stale_discarded, 1);
    }
}
