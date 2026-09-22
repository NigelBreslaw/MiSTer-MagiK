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
use std::time::Instant;

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
    timing: CardProducerTiming,
}

impl RenderedCardFrame {
    pub(super) const fn request(&self) -> CardFrameRequest {
        self.request
    }

    pub(super) fn pixels(&self) -> &[Rgb565Pixel] {
        &self.pixels
    }

    pub(super) const fn timing(&self) -> CardProducerTiming {
        self.timing
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct CardProducerTiming {
    pub(super) total_us: u64,
    pub(super) primary_tile_us: u64,
    pub(super) secondary_tile_us: u64,
    pub(super) secondary_wait_us: u64,
    pub(super) composition_us: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct CardPipelineCounters {
    pub(super) submitted: u64,
    pub(super) completed: u64,
    pub(super) superseded: u64,
    pub(super) stale: u64,
    pub(super) producer_total_us: u64,
    pub(super) primary_tile_us: u64,
    pub(super) secondary_tile_us: u64,
    pub(super) secondary_wait_us: u64,
    pub(super) composition_us: u64,
}

impl CardPipelineCounters {
    pub(super) fn add_assign(&mut self, other: Self) {
        self.submitted = self.submitted.saturating_add(other.submitted);
        self.completed = self.completed.saturating_add(other.completed);
        self.superseded = self.superseded.saturating_add(other.superseded);
        self.stale = self.stale.saturating_add(other.stale);
        self.producer_total_us = self
            .producer_total_us
            .saturating_add(other.producer_total_us);
        self.primary_tile_us = self.primary_tile_us.saturating_add(other.primary_tile_us);
        self.secondary_tile_us = self
            .secondary_tile_us
            .saturating_add(other.secondary_tile_us);
        self.secondary_wait_us = self
            .secondary_wait_us
            .saturating_add(other.secondary_wait_us);
        self.composition_us = self.composition_us.saturating_add(other.composition_us);
    }

    pub(super) fn delta(self, previous: Self) -> Self {
        Self {
            submitted: self.submitted.saturating_sub(previous.submitted),
            completed: self.completed.saturating_sub(previous.completed),
            superseded: self.superseded.saturating_sub(previous.superseded),
            stale: self.stale.saturating_sub(previous.stale),
            producer_total_us: self
                .producer_total_us
                .saturating_sub(previous.producer_total_us),
            primary_tile_us: self
                .primary_tile_us
                .saturating_sub(previous.primary_tile_us),
            secondary_tile_us: self
                .secondary_tile_us
                .saturating_sub(previous.secondary_tile_us),
            secondary_wait_us: self
                .secondary_wait_us
                .saturating_sub(previous.secondary_wait_us),
            composition_us: self.composition_us.saturating_sub(previous.composition_us),
        }
    }
}

struct PipelineState {
    pending: Option<CardFrameRequest>,
    ready: Option<RenderedCardFrame>,
    free: Vec<RecycledFrame>,
    static_frame: Vec<Rgb565Pixel>,
    content_generation: u64,
    counters: CardPipelineCounters,
    shutdown: bool,
    #[cfg(test)]
    hold_completion: bool,
}

struct RecycledFrame {
    pixels: Vec<Rgb565Pixel>,
    content_generation: u64,
}

impl RenderedCardFrame {
    fn recycle(self) -> RecycledFrame {
        RecycledFrame {
            pixels: self.pixels,
            content_generation: self.request.content_generation,
        }
    }
}

struct SharedPipeline {
    state: Mutex<PipelineState>,
    wake: Condvar,
}

enum TileRequest {
    Render(LauncherFrameRequest, PreparedLauncherFrame),
    Stop,
}

struct CompletedTile {
    buffer: PreparedLauncherFrame,
    render_us: u64,
}

pub(super) struct LauncherCardRenderAhead {
    shared: Arc<SharedPipeline>,
    coordinator: Option<JoinHandle<()>>,
    measure_metrics: bool,
}

impl LauncherCardRenderAhead {
    pub(super) fn start(
        preparer: LauncherFramePreparer,
        static_frame: &[Rgb565Pixel],
        measure_timing: bool,
    ) -> Self {
        assert_eq!(static_frame.len(), WIDTH * HEIGHT);
        let shared = Arc::new(SharedPipeline {
            state: Mutex::new(PipelineState {
                pending: None,
                ready: None,
                free: (0..2)
                    .map(|_| RecycledFrame {
                        pixels: static_frame.to_vec(),
                        content_generation: 0,
                    })
                    .collect(),
                static_frame: static_frame.to_vec(),
                content_generation: 0,
                counters: CardPipelineCounters::default(),
                shutdown: false,
                #[cfg(test)]
                hold_completion: false,
            }),
            wake: Condvar::new(),
        });
        let worker_shared = shared.clone();
        let coordinator = std::thread::Builder::new()
            .name("launcher-card-ahead".into())
            .spawn(move || run_coordinator(worker_shared, preparer, measure_timing))
            .expect("failed to start launcher card render-ahead worker");
        Self {
            shared,
            coordinator: Some(coordinator),
            measure_metrics: measure_timing,
        }
    }

    pub(super) fn submit(&self, request: CardFrameRequest) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        // The initial generation is supplied by the owning session's first request.
        if state.content_generation == 0 {
            state.content_generation = request.content_generation;
            for free in &mut state.free {
                free.content_generation = request.content_generation;
            }
        }
        if self.measure_metrics {
            state.counters.submitted = state.counters.submitted.saturating_add(1);
        }
        if state.pending.replace(request).is_some() && self.measure_metrics {
            state.counters.superseded = state.counters.superseded.saturating_add(1);
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
            if self.measure_metrics {
                state.counters.stale = state.counters.stale.saturating_add(1);
            }
            state.free.push(frame.recycle());
            self.shared.wake.notify_one();
            None
        } else {
            Some(frame)
        }
    }

    pub(super) fn recycle(&self, frame: RenderedCardFrame) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.free.push(frame.recycle());
        self.shared.wake.notify_one();
    }

    pub(super) fn return_ready(&self, frame: RenderedCardFrame) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if frame.request.content_generation != state.content_generation {
            state.free.push(frame.recycle());
            if self.measure_metrics {
                state.counters.stale += 1;
            }
            self.shared.wake.notify_one();
            return;
        }
        match state.ready.take() {
            Some(newer) if newer.request.render.generation > frame.request.render.generation => {
                state.ready = Some(newer);
                state.free.push(frame.recycle());
                if self.measure_metrics {
                    state.counters.superseded = state.counters.superseded.saturating_add(1);
                }
            }
            Some(older) => {
                state.free.push(older.recycle());
                state.ready = Some(frame);
                if self.measure_metrics {
                    state.counters.superseded = state.counters.superseded.saturating_add(1);
                }
            }
            None => state.ready = Some(frame),
        }
        self.shared.wake.notify_one();
    }

    pub(super) fn has_ready(&self) -> bool {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ready
            .is_some()
    }

    pub(super) fn refresh_chrome(&self, generation: u64, pixels: &[Rgb565Pixel]) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.static_frame.copy_from_slice(pixels);
        state.content_generation = generation;
        if let Some(old) = state.ready.take() {
            state.free.push(old.recycle());
            if self.measure_metrics {
                state.counters.stale += 1;
            }
        }
        if state.pending.take().is_some() && self.measure_metrics {
            state.counters.stale += 1;
        }
        self.shared.wake.notify_one();
    }

    pub(super) fn counters(&self) -> CardPipelineCounters {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .counters
    }

    #[cfg(test)]
    pub(super) fn worker_identity(&self) -> std::thread::ThreadId {
        self.coordinator.as_ref().unwrap().thread().id()
    }
}

impl LauncherCardRenderAhead {
    pub(super) fn stop(&mut self) {
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

impl Drop for LauncherCardRenderAhead {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_coordinator(
    shared: Arc<SharedPipeline>,
    preparer: LauncherFramePreparer,
    measure_timing: bool,
) {
    apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardRenderer);
    let (tile_request_tx, tile_request_rx) = sync_channel(1);
    let (tile_completed_tx, tile_completed_rx) = sync_channel(1);
    let secondary_preparer = preparer.clone();
    let secondary = std::thread::Builder::new()
        .name("launcher-card-tile".into())
        .spawn(move || {
            run_secondary_tile_worker(
                secondary_preparer,
                tile_request_rx,
                tile_completed_tx,
                measure_timing,
            )
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
            let request = state.pending.take().expect("pending request checked");
            let mut output = state.free.pop().expect("free frame checked");
            if output.content_generation != state.content_generation {
                output.pixels.copy_from_slice(&state.static_frame);
                output.content_generation = state.content_generation;
            }
            (request, output)
        };
        let total_started = measure_timing.then(Instant::now);

        let Some(right_buffer) = right.take() else {
            break;
        };
        if tile_request_tx
            .send(TileRequest::Render(request.render, right_buffer))
            .is_err()
        {
            break;
        }
        let primary_started = measure_timing.then(Instant::now);
        preparer.render_tile(request.render, &mut left, (CAROUSEL_LEFT, CAROUSEL_SPLIT));
        let primary_tile_us = elapsed_us(primary_started);
        let wait_started = measure_timing.then(Instant::now);
        let Ok(completed_right) = tile_completed_rx.recv() else {
            break;
        };
        let secondary_wait_us = elapsed_us(wait_started);
        let composition_started = measure_timing.then(Instant::now);
        compose_tiles(
            &mut output.pixels,
            left.pixels(),
            completed_right.buffer.pixels(),
        );
        let composition_us = elapsed_us(composition_started);
        right = Some(completed_right.buffer);
        let timing = CardProducerTiming {
            total_us: elapsed_us(total_started),
            primary_tile_us,
            secondary_tile_us: completed_right.render_us,
            secondary_wait_us,
            composition_us,
        };

        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if measure_timing {
            state.counters.completed = state.counters.completed.saturating_add(1);
            state.counters.producer_total_us += timing.total_us;
            state.counters.primary_tile_us += timing.primary_tile_us;
            state.counters.secondary_tile_us += timing.secondary_tile_us;
            state.counters.secondary_wait_us += timing.secondary_wait_us;
            state.counters.composition_us += timing.composition_us;
        }
        #[cfg(test)]
        while state.hold_completion && !state.shutdown {
            state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        if request.content_generation != state.content_generation {
            state.free.push(output);
            if measure_timing {
                state.counters.stale += 1;
            }
            shared.wake.notify_one();
            continue;
        }
        if let Some(old) = state.ready.replace(RenderedCardFrame {
            request,
            pixels: output.pixels,
            timing,
        }) {
            state.free.push(old.recycle());
            if measure_timing {
                state.counters.superseded = state.counters.superseded.saturating_add(1);
            }
        }
        shared.wake.notify_one();
    }

    let _ = tile_request_tx.send(TileRequest::Stop);
    let _ = secondary.join();
}

fn run_secondary_tile_worker(
    preparer: LauncherFramePreparer,
    requests: Receiver<TileRequest>,
    completed: SyncSender<CompletedTile>,
    measure_timing: bool,
) {
    apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardRendererSecondary);
    while let Ok(request) = requests.recv() {
        match request {
            TileRequest::Render(request, mut buffer) => {
                let started = measure_timing.then(Instant::now);
                preparer.render_tile(request, &mut buffer, (CAROUSEL_SPLIT, CAROUSEL_RIGHT));
                let render_us = elapsed_us(started);
                if completed.send(CompletedTile { buffer, render_us }).is_err() {
                    break;
                }
            }
            TileRequest::Stop => break,
        }
    }
}

fn elapsed_us(started: Option<Instant>) -> u64 {
    started.map_or(0, |started| {
        started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
    })
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
        let pipeline =
            LauncherCardRenderAhead::start(serial.frame_preparer(), serial.pixels(), true);
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
        let pipeline =
            LauncherCardRenderAhead::start(serial.frame_preparer(), serial.pixels(), true);
        for sequence in 1..=8 {
            pipeline.submit(request(
                sequence,
                900_000 + sequence,
                u32::try_from(sequence * 10).unwrap(),
            ));
        }
        let frame = wait_for(&pipeline, 8);
        assert_eq!(frame.request().render.generation, 8);
        pipeline.recycle(frame);
    }

    #[test]
    fn stale_generation_is_discarded_and_recycled() {
        let serial = prepared();
        let pipeline =
            LauncherCardRenderAhead::start(serial.frame_preparer(), serial.pixels(), true);
        pipeline.submit(request(1, 900_000, 90));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pipeline.has_ready() {
            assert!(Instant::now() < deadline, "render-ahead worker timed out");
            std::thread::yield_now();
        }
        assert!(pipeline.try_take(4, 8, 1_000_000, 1_000_000).is_none());
        let counters = pipeline.counters();
        assert_eq!(counters.submitted, 1);
        assert_eq!(counters.completed, 1);
        assert_eq!(counters.stale, 1);
    }

    #[test]
    fn unposted_frame_returns_to_the_ready_slot() {
        let serial = prepared();
        let pipeline =
            LauncherCardRenderAhead::start(serial.frame_preparer(), serial.pixels(), true);
        pipeline.submit(request(1, 900_000, 90));
        let frame = wait_for(&pipeline, 1);
        pipeline.return_ready(frame);
        let frame = pipeline
            .try_take(4, 7, 1_000_000, 1_000_000)
            .expect("returned frame remains ready");
        assert_eq!(frame.request().render.generation, 1);
        assert!(frame.timing().total_us > 0);
        pipeline.recycle(frame);
    }

    #[test]
    fn chrome_refresh_rejects_in_flight_work_and_reseeds_recycled_frames() {
        let mut serial = prepared();
        let pipeline =
            LauncherCardRenderAhead::start(serial.frame_preparer(), serial.pixels(), true);
        pipeline.shared.state.lock().unwrap().hold_completion = true;
        pipeline.submit(request(1, 900_000, 90));
        let deadline = Instant::now() + Duration::from_secs(2);
        while pipeline.counters().completed == 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        // A chrome pixel outside the carousel must survive tile composition.
        let mut chrome = serial.pixels().to_vec();
        chrome[20 * WIDTH + 880] = Rgb565Pixel(0x1234);
        pipeline.refresh_chrome(5, &chrome);
        {
            let mut state = pipeline.shared.state.lock().unwrap();
            state.hold_completion = false;
            pipeline.shared.wake.notify_one();
        }
        for sequence in 2..=4 {
            let mut req = request(sequence, 900_000, 90);
            req.content_generation = 5;
            pipeline.submit(req);
            let deadline = Instant::now() + Duration::from_secs(2);
            let frame = loop {
                if let Some(frame) = pipeline.try_take(5, 7, 1_000_000, 1_000_000) {
                    break frame;
                }
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            };
            serial.render_frame(req.render.frame);
            let mut expected = serial.pixels().to_vec();
            expected[20 * WIDTH + 880] = Rgb565Pixel(0x1234);
            assert_eq!(frame.pixels(), expected);
            assert_eq!(frame.request.content_generation, 5);
            pipeline.recycle(frame);
        }
        assert_eq!(pipeline.counters().stale, 1);
        assert_eq!(pipeline.shared.state.lock().unwrap().free.len(), 2);
    }

    #[test]
    fn counter_delta_is_saturating_and_field_specific() {
        let previous = CardPipelineCounters {
            submitted: 4,
            completed: 3,
            superseded: 2,
            stale: 1,
            ..Default::default()
        };
        let current = CardPipelineCounters {
            submitted: 7,
            completed: 5,
            superseded: 6,
            stale: 1,
            ..Default::default()
        };
        assert_eq!(
            current.delta(previous),
            CardPipelineCounters {
                submitted: 3,
                completed: 2,
                superseded: 4,
                stale: 0,
                ..Default::default()
            }
        );
    }
}
