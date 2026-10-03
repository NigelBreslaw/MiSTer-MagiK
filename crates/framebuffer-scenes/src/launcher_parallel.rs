// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! One current pose, one matching helper band, one coherent cached output.
use crate::{
    Rgb565Pixel,
    launcher::{LauncherFramePreparer, LauncherFrameRequest, PreparedLauncherFrame},
};
use std::{
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    thread::JoinHandle,
    time::Instant,
};

pub const CAROUSEL_SPLIT: usize = 629;
#[cfg(test)]
const CAROUSEL_LEFT: usize = 296;
const CAROUSEL_RIGHT: usize = 934;
/// Neither band may shrink below this; a pose change cannot strand one core.
const MINIMUM_BAND: usize = 96;
const SPLIT_ALIGNMENT: usize = 8;

/// Move the band boundary a quarter of the way to where both bands would
/// finish together. Each band's measured cost is spread evenly across its
/// columns; carousel poses change little between consecutive frames.
/// The partial step is load-bearing: level-deal cost is concentrated in a few
/// columns, and a full step crossed it every frame on device, swinging the
/// boundary between both clamps and adding about six drops per route.
fn balanced_split(left: usize, split: usize, primary_us: u64, secondary_us: u64) -> usize {
    if primary_us == 0 || secondary_us == 0 {
        return split;
    }
    let primary_rate = primary_us as f64 / (split - left) as f64;
    let secondary_rate = secondary_us as f64 / (CAROUSEL_RIGHT - split) as f64;
    let ideal = (primary_rate * left as f64 + secondary_rate * CAROUSEL_RIGHT as f64)
        / (primary_rate + secondary_rate);
    let error = ideal - split as f64;
    if error.abs() < SPLIT_ALIGNMENT as f64 {
        return split;
    }
    // A quarter step, but never less than one aligned step, so rounding cannot
    // stall the boundary short of balance.
    let step = (error / 4.0)
        .abs()
        .max(SPLIT_ALIGNMENT as f64)
        .copysign(error);
    let aligned =
        ((split as f64 + step) / SPLIT_ALIGNMENT as f64).round() as usize * SPLIT_ALIGNMENT;
    aligned.clamp(left + MINIMUM_BAND, CAROUSEL_RIGHT - MINIMUM_BAND)
}
/// Optional cumulative per-thread clocks in microseconds, supplied by the
/// application that owns the OS.
#[derive(Clone, Copy)]
pub struct ThreadClocks {
    pub cpu_us: fn() -> Option<u64>,
    /// Time the thread was runnable while another task held its CPU.
    pub run_delay_us: fn() -> Option<u64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct ThreadSample {
    cpu_us: Option<u64>,
    run_delay_us: Option<u64>,
}

impl ThreadSample {
    fn now(clocks: Option<ThreadClocks>) -> Self {
        clocks.map_or_else(Self::default, |clocks| Self {
            cpu_us: (clocks.cpu_us)(),
            run_delay_us: (clocks.run_delay_us)(),
        })
    }

    /// CPU time and run delay elapsed since `start`.
    fn since(self, start: Self) -> (Option<u64>, Option<u64>) {
        (
            cpu_delta(start.cpu_us, self.cpu_us),
            cpu_delta(start.run_delay_us, self.run_delay_us),
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParallelFrameTiming {
    pub renderer_id: u64,
    pub request_generation: u64,
    pub request_timestamp_us: u64,
    pub helper_dispatched_at: Instant,
    pub helper_started_at: Instant,
    pub helper_finished_at: Instant,
    pub helper_received_at: Instant,
    pub discarded_generation: Option<u64>,
    pub total_us: u64,
    pub helper_ahead: bool,
    pub helper_ahead_lead_us: u64,
    pub discarded_helper_us: u64,
    pub discarded_helper_cpu_us: Option<u64>,
    pub primary_us: u64,
    pub secondary_us: u64,
    pub wait_us: u64,
    pub helper_start_delay_us: u64,
    pub completion_delivery_us: u64,
    pub merge_us: u64,
    pub primary_cpu_us: Option<u64>,
    pub secondary_cpu_us: Option<u64>,
    /// Time each band's thread was runnable while another task held its CPU.
    pub primary_run_delay_us: Option<u64>,
    pub secondary_run_delay_us: Option<u64>,
    /// First helper column for this frame.
    pub split: usize,
}
struct Job {
    preparer: LauncherFramePreparer,
    request: LauncherFrameRequest,
    buffer: PreparedLauncherFrame,
    dispatched_at: Instant,
    split: usize,
}
struct Ahead {
    preparer: LauncherFramePreparer,
    request: LauncherFrameRequest,
    split: usize,
    dispatched_at: Instant,
}
struct Completion {
    dispatched_at: Instant,
    started_at: Instant,
    #[cfg(feature = "launcher-profile")]
    profile: Option<crate::launcher_profile::Report>,
    buffer: PreparedLauncherFrame,
    wall_us: u64,
    cpu_us: Option<u64>,
    run_delay_us: Option<u64>,
    start_delay_us: u64,
    finished_at: Instant,
}
pub struct ParallelLauncherRenderer {
    instance_id: u64,
    primary: PreparedLauncherFrame,
    helper: Option<PreparedLauncherFrame>,
    spare: Option<PreparedLauncherFrame>,
    ahead: Option<Ahead>,
    requests: Option<SyncSender<Job>>,
    completions: Receiver<Completion>,
    worker: Option<JoinHandle<()>>,
    clocks: Option<ThreadClocks>,
    storage_bytes: usize,
    split: usize,
    rendered_split: usize,
    retain_bands: bool,
    helper_unmerged: bool,
}
fn copy_helper_band(destination: &mut [Rgb565Pixel], source: &[Rgb565Pixel], split: usize) -> u64 {
    let started = Instant::now();
    #[cfg(feature = "launcher-profile")]
    let _merge = crate::launcher_profile::span("frame.helper-merge");
    for y in 120..495 {
        let range = y * 960 + split..y * 960 + CAROUSEL_RIGHT;
        destination[range.clone()].copy_from_slice(&source[range]);
    }
    micros(started)
}

fn micros(start: Instant) -> u64 {
    start.elapsed().as_micros() as u64
}
fn cpu_delta(start: Option<u64>, end: Option<u64>) -> Option<u64> {
    start.zip(end).map(|(s, e)| e.saturating_sub(s))
}
impl ParallelLauncherRenderer {
    pub fn new(
        preparer: LauncherFramePreparer,
        worker_setup: Option<fn()>,
        clocks: Option<ThreadClocks>,
    ) -> Result<Self, String> {
        let primary = preparer.new_direct_tile_buffer();
        let helper = preparer.new_tile_buffer();
        let storage_bytes = primary.storage_bytes() + helper.storage_bytes();
        let (requests, received) = sync_channel::<Job>(1);
        let (completed, completions) = sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("card-tile-helper".into())
            .spawn(move || {
                if let Some(setup) = worker_setup {
                    setup();
                }
                while let Ok(mut job) = received.recv() {
                    let started_at = Instant::now();
                    let sample = ThreadSample::now(clocks);
                    job.preparer.render_tile(
                        job.request,
                        &mut job.buffer,
                        (job.split, CAROUSEL_RIGHT),
                    );
                    let (cpu_us, run_delay_us) = ThreadSample::now(clocks).since(sample);
                    let wall_us = micros(started_at);
                    let finished_at = Instant::now();
                    #[cfg(feature = "launcher-profile")]
                    let profile =
                        crate::launcher_profile::enabled().then(crate::launcher_profile::take);
                    if completed
                        .send(Completion {
                            dispatched_at: job.dispatched_at,
                            started_at,
                            #[cfg(feature = "launcher-profile")]
                            profile,
                            buffer: job.buffer,
                            wall_us,
                            cpu_us,
                            run_delay_us,
                            start_delay_us: started_at
                                .saturating_duration_since(job.dispatched_at)
                                .as_micros() as u64,
                            finished_at,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Ok(Self {
            instance_id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            primary,
            helper: Some(helper),
            spare: None,
            ahead: None,
            requests: Some(requests),
            completions,
            worker: Some(worker),
            clocks,
            storage_bytes,
            split: CAROUSEL_SPLIT,
            rendered_split: CAROUSEL_SPLIT,
            retain_bands: false,
            helper_unmerged: false,
        })
    }
    pub fn render(
        &mut self,
        preparer: &LauncherFramePreparer,
        request: LauncherFrameRequest,
        destination: &mut [Rgb565Pixel],
    ) -> Result<ParallelFrameTiming, String> {
        let started = Instant::now();
        let left = preparer.carousel_clip().0;
        let helper_ahead = self
            .ahead
            .as_ref()
            .is_some_and(|ahead| ahead.request == request && ahead.preparer.same_source(preparer));
        let discarded_generation = self
            .ahead
            .as_ref()
            .filter(|_| !helper_ahead)
            .map(|ahead| ahead.request.generation);
        let (discarded_helper_us, discarded_helper_cpu_us) =
            if self.ahead.is_some() && !helper_ahead {
                self.retire_ahead()?
            } else {
                (0, None)
            };
        let ahead = self.ahead.take();
        let helper_ahead_lead_us = ahead.as_ref().map_or(0, |ahead| {
            started
                .saturating_duration_since(ahead.dispatched_at)
                .as_micros() as u64
        });
        let split = ahead.as_ref().map_or_else(
            || {
                self.split
                    .clamp(left + MINIMUM_BAND, CAROUSEL_RIGHT - MINIMUM_BAND)
            },
            |ahead| ahead.split,
        );
        if !helper_ahead {
            self.requests
                .as_ref()
                .ok_or("card renderer stopped")?
                .send(Job {
                    preparer: preparer.clone(),
                    request,
                    buffer: self.helper.take().ok_or("helper output unavailable")?,
                    dispatched_at: Instant::now(),
                    split,
                })
                .map_err(|e| e.to_string())?;
        }
        let primary_started = Instant::now();
        let sample = ThreadSample::now(self.clocks);
        preparer.render_tile_into(request, &mut self.primary, destination, (left, split));
        let (primary_cpu_us, primary_run_delay_us) = ThreadSample::now(self.clocks).since(sample);
        let primary_us = micros(primary_started);
        let waiting = Instant::now();
        let completed = self.completions.recv().map_err(|e| e.to_string())?;
        let received_at = Instant::now();
        let wait_us = micros(waiting);
        if completed.buffer.request() != Some(request) {
            return Err("mismatched current card pose".into());
        }
        self.rendered_split = split;
        self.helper_unmerged = self.retain_bands;
        let merge_us = if self.retain_bands {
            0
        } else {
            copy_helper_band(destination, completed.buffer.pixels(), split)
        };
        #[cfg(feature = "launcher-profile")]
        if let Some(profile) = completed.profile {
            crate::launcher_profile::absorb_worker(profile);
        }
        let previous = self.helper.replace(completed.buffer);
        if helper_ahead {
            self.spare = previous;
        }
        // Balance what each band adds to the critical path, including the
        // helper's wake-up; the primary band runs on the presenting thread.
        self.split = balanced_split(
            left,
            split,
            primary_us,
            completed.wall_us + completed.start_delay_us,
        );
        Ok(ParallelFrameTiming {
            renderer_id: self.instance_id,
            request_generation: request.generation,
            request_timestamp_us: request.timestamp_us,
            helper_dispatched_at: completed.dispatched_at,
            helper_started_at: completed.started_at,
            helper_finished_at: completed.finished_at,
            helper_received_at: received_at,
            discarded_generation,
            total_us: micros(started),
            helper_ahead,
            helper_ahead_lead_us,
            discarded_helper_us,
            discarded_helper_cpu_us,
            primary_us,
            secondary_us: completed.wall_us,
            wait_us,
            helper_start_delay_us: completed.start_delay_us,
            completion_delivery_us: received_at
                .saturating_duration_since(completed.finished_at)
                .as_micros() as u64,
            merge_us,
            primary_cpu_us,
            secondary_cpu_us: completed.cpu_us,
            primary_run_delay_us,
            secondary_run_delay_us: completed.run_delay_us,
            split,
        })
    }
    /// At most one future helper band is in flight. The current immutable
    /// pixels stay available; projection scratch moves to the spare buffer.
    pub fn prepare_helper_ahead(
        &mut self,
        preparer: &LauncherFramePreparer,
        request: LauncherFrameRequest,
    ) -> Result<bool, String> {
        if self.ahead.is_some() {
            return Ok(false);
        }
        let sender = self.requests.as_ref().ok_or("card renderer stopped")?;
        let helper = self.helper.as_mut().ok_or("helper output unavailable")?;
        if helper.request().is_none() {
            return Ok(false);
        }
        let mut buffer = self.spare.take().unwrap_or_else(|| {
            let buffer = PreparedLauncherFrame::spare_pixels();
            self.storage_bytes += buffer.storage_bytes();
            buffer
        });
        buffer.swap_scratch(helper);
        let split = self.split.clamp(
            preparer.carousel_clip().0 + MINIMUM_BAND,
            CAROUSEL_RIGHT - MINIMUM_BAND,
        );
        let dispatched_at = Instant::now();
        let job = Job {
            preparer: preparer.clone(),
            request,
            buffer,
            dispatched_at,
            split,
        };
        if let Err(error) = sender.send(job) {
            let mut buffer = error.0.buffer;
            buffer.swap_scratch(helper);
            self.spare = Some(buffer);
            return Err("card helper stopped".into());
        }
        self.ahead = Some(Ahead {
            preparer: preparer.clone(),
            request,
            split,
            dispatched_at,
        });
        Ok(true)
    }

    fn retire_ahead(&mut self) -> Result<(u64, Option<u64>), String> {
        self.ahead.take();
        let mut completed = self.completions.recv().map_err(|error| error.to_string())?;
        #[cfg(feature = "launcher-profile")]
        if let Some(profile) = completed.profile {
            crate::launcher_profile::absorb_worker(profile);
        }
        completed
            .buffer
            .swap_scratch(self.helper.as_mut().ok_or("helper output unavailable")?);
        self.spare = Some(completed.buffer);
        Ok((completed.wall_us, completed.cpu_us))
    }

    /// Retain separate immutable sources for the native two-tile publisher.
    /// Full-frame consumers keep the default merged output.
    pub fn retain_bands(&mut self, retain: bool) {
        self.retain_bands = retain;
    }

    pub const fn rendered_split(&self) -> usize {
        self.rendered_split
    }

    pub fn helper_pixels(&self, request: LauncherFrameRequest) -> Option<&[Rgb565Pixel]> {
        self.helper
            .as_ref()
            .filter(|buffer| buffer.request() == Some(request))
            .map(PreparedLauncherFrame::pixels)
    }

    pub fn merge_retained_helper(&mut self, destination: &mut [Rgb565Pixel]) {
        if self.helper_unmerged {
            copy_helper_band(
                destination,
                self.helper.as_ref().expect("completed helper").pixels(),
                self.rendered_split,
            );
            self.helper_unmerged = false;
        }
    }

    pub const fn storage_bytes(&self) -> usize {
        self.storage_bytes
    }
    pub fn helper_thread_id(&self) -> std::thread::ThreadId {
        self.worker.as_ref().unwrap().thread().id()
    }
    pub fn stop(&mut self) {
        if self.ahead.is_some() {
            let _ = self.retire_ahead();
        }
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for ParallelLauncherRenderer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::{
        LauncherCard, LauncherCardId, LauncherData, LauncherLevel, LauncherScene,
    };
    use crate::launcher_navigation::{BrowseDirection, BrowseFrame, BrowsePhase};

    #[test]
    fn ahead_preserves_current_pixels_and_rejects_changed_sources() {
        static HELPER_READS: std::sync::Mutex<Vec<Instant>> = std::sync::Mutex::new(Vec::new());
        fn record_helper_clock() -> Option<u64> {
            if std::thread::current().name() == Some("card-tile-helper") {
                HELPER_READS.lock().unwrap().push(Instant::now());
            }
            None
        }
        let clocks = ThreadClocks {
            cpu_us: record_helper_clock,
            run_delay_us: || None,
        };
        let scene = LauncherScene::new(960, 540);
        let cards = ["A", "B", "C", "D", "E", "F"].map(|name| LauncherCard {
            id: LauncherCardId::Consoles,
            name,
            games: Some(12),
            colour: 0x2a7f,
        });
        let data = LauncherData {
            cards: &cards,
            selected: 0,
            library_games: 72,
            collections: 6,
            favourites: 1,
            clock: "12:00",
            level: LauncherLevel::Root,
        };
        let mut page = scene.prepare(data);
        let mut serial = scene.prepare(data);
        let preparer = page.frame_preparer();
        let mut renderer =
            ParallelLauncherRenderer::new(preparer.clone(), None, Some(clocks)).unwrap();
        renderer.retain_bands(true);
        let storage = renderer.storage_bytes();
        let worker = renderer.helper_thread_id();
        let frame = BrowseFrame {
            selected: 0,
            target: 0,
            phase: BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 180,
        };
        let first = LauncherFrameRequest {
            frame,
            timestamp_us: 0,
            generation: 1,
        };
        page.render_parallel_frame(&mut renderer, first).unwrap();
        let original = renderer.helper_pixels(first).unwrap().to_vec();
        let next = LauncherFrameRequest {
            timestamp_us: 16_000,
            generation: 2,
            ..first
        };
        assert!(renderer.prepare_helper_ahead(&preparer, next).unwrap());
        assert!(!renderer.prepare_helper_ahead(&preparer, next).unwrap());
        assert_eq!(renderer.helper_pixels(first).unwrap(), original);
        assert!(renderer.helper_pixels(next).is_none());
        page.merge_retained_helper(&mut renderer);
        serial.render_frame(frame);
        assert!(page.pixels() == serial.pixels());
        let timing = page.render_parallel_frame(&mut renderer, next).unwrap();
        assert!(timing.helper_ahead);
        assert_eq!(timing.request_generation, next.generation);
        assert_eq!(timing.request_timestamp_us, next.timestamp_us);
        assert!(timing.helper_dispatched_at <= timing.helper_started_at);
        assert!(timing.helper_started_at <= timing.helper_finished_at);
        assert!(timing.helper_finished_at <= timing.helper_received_at);
        page.merge_retained_helper(&mut renderer);
        assert!(page.pixels() == serial.pixels());
        assert_eq!(renderer.storage_bytes(), storage + 960 * 540 * 2);

        let next_request = LauncherFrameRequest {
            timestamp_us: 33_000,
            generation: 3,
            ..first
        };
        assert!(
            renderer
                .prepare_helper_ahead(&preparer, next_request)
                .unwrap()
        );
        // Same request, but independently prepared artwork: the old band must
        // be discarded rather than tagged as the new source's output.
        let mut replacement = scene.prepare(data);
        let timing = replacement
            .render_parallel_frame(&mut renderer, next_request)
            .unwrap();
        assert!(!timing.helper_ahead);
        assert!(timing.discarded_helper_us > 0);
        assert_eq!(timing.discarded_generation, Some(next_request.generation));
        {
            let samples = HELPER_READS.lock().unwrap();
            let discarded_finished = samples[samples.len() - 3];
            assert!(timing.helper_dispatched_at >= discarded_finished);
            assert_eq!(
                timing.helper_start_delay_us,
                timing
                    .helper_started_at
                    .saturating_duration_since(timing.helper_dispatched_at)
                    .as_micros() as u64
            );
        }
        replacement.merge_retained_helper(&mut renderer);
        assert!(replacement.pixels() == serial.pixels());
        assert_eq!(renderer.helper_thread_id(), worker);
        let retained = renderer.helper_pixels(next_request).unwrap().to_vec();
        let future = LauncherFrameRequest {
            timestamp_us: 50_000,
            generation: 4,
            ..first
        };
        assert!(
            renderer
                .prepare_helper_ahead(&replacement.frame_preparer(), future)
                .unwrap()
        );
        renderer.stop();
        assert_eq!(renderer.helper_pixels(next_request).unwrap(), retained);
        assert!(renderer.prepare_helper_ahead(&preparer, future).is_err());
    }

    #[test]
    fn split_moves_toward_balance_and_stays_bounded() {
        // Equal cost per column leaves a balanced boundary in place.
        assert_eq!(balanced_split(CAROUSEL_LEFT, 616, 3200, 3180), 616);
        // A slower helper band moves the boundary right, by a bounded step.
        let next = balanced_split(CAROUSEL_LEFT, CAROUSEL_SPLIT, 11_300, 13_300);
        assert!(
            next > CAROUSEL_SPLIT && next <= CAROUSEL_SPLIT + 24,
            "{next}"
        );
        assert_eq!(next % SPLIT_ALIGNMENT, 0);
        // Repeated imbalance converges near the boundary where both finish together.
        let mut split = CAROUSEL_SPLIT;
        let (left_rate, right_rate) = (34.0, 43.0);
        for _ in 0..40 {
            let primary = ((split - CAROUSEL_LEFT) as f64 * left_rate) as u64;
            let secondary = ((CAROUSEL_RIGHT - split) as f64 * right_rate) as u64;
            split = balanced_split(CAROUSEL_LEFT, split, primary, secondary);
        }
        let ideal = (left_rate * CAROUSEL_LEFT as f64 + right_rate * CAROUSEL_RIGHT as f64)
            / (left_rate + right_rate);
        assert!(
            (split as f64 - ideal).abs() <= SPLIT_ALIGNMENT as f64,
            "{split} vs {ideal}"
        );
        // Extremes cannot starve either band.
        let (mut right, mut left) = (CAROUSEL_SPLIT, CAROUSEL_SPLIT);
        for _ in 0..40 {
            right = balanced_split(CAROUSEL_LEFT, right, 1, 1_000_000);
            left = balanced_split(CAROUSEL_LEFT, left, 1_000_000, 1);
        }
        assert_eq!(right, CAROUSEL_RIGHT - MINIMUM_BAND);
        assert_eq!(left, CAROUSEL_LEFT + MINIMUM_BAND);
        assert_eq!(
            balanced_split(CAROUSEL_LEFT, CAROUSEL_SPLIT, 0, 5_000),
            CAROUSEL_SPLIT
        );
    }

    #[test]
    #[cfg(feature = "launcher-profile")]
    fn instrumented_frame_collects_and_drains_helper_stages() {
        let cards = ["A", "B", "C", "D", "E", "F"].map(|name| LauncherCard {
            id: LauncherCardId::Consoles,
            name,
            games: Some(12),
            colour: 0x2a7f,
        });
        let mut scene = LauncherScene::new(960, 540).prepare(LauncherData {
            cards: &cards,
            selected: 0,
            library_games: 72,
            collections: 6,
            favourites: 1,
            clock: "12:00",
            level: LauncherLevel::Root,
        });
        let preparer = scene.frame_preparer();
        let mut renderer = ParallelLauncherRenderer::new(preparer.clone(), None, None).unwrap();
        let _ = crate::launcher_profile::take();
        crate::launcher_profile::enable_wall_time();
        let request = LauncherFrameRequest {
            frame: BrowseFrame {
                selected: 0,
                target: 1,
                phase: BrowsePhase::Flipping,
                direction: Some(BrowseDirection::Right),
                progress_millis: 60,
                duration_millis: 180,
            },
            timestamp_us: 60_000,
            generation: 1,
        };
        scene.render_parallel_frame(&mut renderer, request).unwrap();
        crate::launcher_profile::disable();
        let report = crate::launcher_profile::take();
        assert_eq!(report.worker_frames, 1);
        for label in [
            "flip.clear",
            "flip.geometry-filter",
            "flip.compose",
            "reflection.prepare",
            "flip.reflection",
            "frame.helper-merge",
        ] {
            assert!(report.stages.contains_key(label), "missing {label}");
        }
        let drained = crate::launcher_profile::take();
        assert_eq!(drained.worker_frames, 0);
        assert!(drained.stages.is_empty());
    }

    #[test]
    fn every_split_matches_the_serial_renderer() {
        const CARDS: [LauncherCard<'static>; 5] = [
            LauncherCard {
                id: LauncherCardId::Arcade,
                name: "ARCADE",
                games: Some(1752),
                colour: 0x88a6,
            },
            LauncherCard {
                id: LauncherCardId::Consoles,
                name: "CONSOLES",
                games: Some(34),
                colour: 0x07e0,
            },
            LauncherCard {
                id: LauncherCardId::Computers,
                name: "COMPUTERS",
                games: Some(86),
                colour: 0xb926,
            },
            LauncherCard {
                id: LauncherCardId::Handhelds,
                name: "HANDHELDS",
                games: Some(12),
                colour: 0x2a5f,
            },
            LauncherCard {
                id: LauncherCardId::Settings,
                name: "SETTINGS",
                games: None,
                colour: 0x8410,
            },
        ];
        let data = LauncherData {
            cards: &CARDS,
            selected: 0,
            library_games: 6842,
            collections: 18,
            favourites: 126,
            clock: "21:37",
            level: LauncherLevel::Root,
        };
        let mut serial;
        let mut parallel = LauncherScene::new(960, 540).prepare(data);
        let preparer = parallel.frame_preparer();
        let mut renderer = ParallelLauncherRenderer::new(preparer.clone(), None, None).unwrap();
        let mut generation = 0;
        // One persistent renderer crosses root/nested boundaries in production.
        for level in [
            LauncherLevel::Root,
            LauncherLevel::Nested(crate::launcher::NestedLevel {
                path: &["CONSOLES"],
                games: 123,
                children: 5,
                children_label: "MAKERS",
                detail: None,
                accent: 0x2a7f,
            }),
            LauncherLevel::Root,
        ] {
            serial = LauncherScene::new(960, 540).prepare(LauncherData { level, ..data });
            parallel = LauncherScene::new(960, 540).prepare(LauncherData { level, ..data });
            for split in [
                CAROUSEL_LEFT + MINIMUM_BAND,
                456,
                600,
                CAROUSEL_SPLIT,
                777,
                CAROUSEL_RIGHT - MINIMUM_BAND,
            ] {
                for (direction, progress) in [
                    (BrowseDirection::Right, 1),
                    (BrowseDirection::Right, 23_000),
                    (BrowseDirection::Left, 40_000),
                    (BrowseDirection::Left, 65_535),
                ] {
                    let frame = BrowseFrame {
                        selected: 0,
                        target: 1,
                        phase: BrowsePhase::Flipping,
                        direction: Some(direction),
                        progress_millis: progress,
                        duration_millis: crate::launcher_navigation::SPRING_POSITION_UNITS,
                    };
                    generation += 1;
                    renderer.split = split;
                    let timing = parallel
                        .render_parallel_frame(
                            &mut renderer,
                            LauncherFrameRequest {
                                frame,
                                timestamp_us: 0,
                                generation,
                            },
                        )
                        .unwrap();
                    assert_eq!(timing.split, split);
                    serial.render_frame(frame);
                    assert!(
                        parallel.pixels() == serial.pixels(),
                        "split {split}, {direction:?} {progress}"
                    );
                    // A native publisher combines the two immutable sources,
                    // including when the adaptive boundary changes each frame.
                    renderer.retain_bands(true);
                    renderer.split = split;
                    generation += 1;
                    let retained_frame = BrowseFrame {
                        progress_millis: progress ^ 0x8000,
                        ..frame
                    };
                    let request = LauncherFrameRequest {
                        frame: retained_frame,
                        timestamp_us: 0,
                        generation,
                    };
                    let timing = parallel
                        .render_parallel_frame(&mut renderer, request)
                        .unwrap();
                    assert_eq!(timing.merge_us, 0);
                    assert_eq!(renderer.rendered_split(), split);
                    let mut published = parallel.pixels().to_vec();
                    let helper = renderer.helper_pixels(request).unwrap();
                    for y in 120..495 {
                        let range = y * 960 + split..y * 960 + CAROUSEL_RIGHT;
                        published[range.clone()].copy_from_slice(&helper[range]);
                    }
                    serial.render_frame(retained_frame);
                    assert!(published == serial.pixels(), "retained split {split}");
                    parallel.merge_retained_helper(&mut renderer);
                    assert!(
                        parallel.pixels() == serial.pixels(),
                        "full consumer after retained bands"
                    );
                    renderer.retain_bands(false);
                }
            }
        }
    }
}
