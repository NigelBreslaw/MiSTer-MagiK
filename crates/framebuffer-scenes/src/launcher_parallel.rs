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

#[derive(Clone, Copy, Debug, Default)]
pub struct ParallelFrameTiming {
    pub total_us: u64,
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
struct Completion {
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
    primary: PreparedLauncherFrame,
    helper: Option<PreparedLauncherFrame>,
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
        Ok(Self {
            primary,
            helper: Some(helper),
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
        let split = self
            .split
            .clamp(left + MINIMUM_BAND, CAROUSEL_RIGHT - MINIMUM_BAND);
        self.requests
            .as_ref()
            .ok_or("card renderer stopped")?
            .send(Job {
                preparer: preparer.clone(),
                request,
                buffer: self.helper.take().ok_or("helper output unavailable")?,
                dispatched_at: started,
                split,
            })
            .map_err(|e| e.to_string())?;
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
        self.helper = Some(completed.buffer);
        // Balance what each band adds to the critical path, including the
        // helper's wake-up; the primary band runs on the presenting thread.
        self.split = balanced_split(
            left,
            split,
            primary_us,
            completed.wall_us + completed.start_delay_us,
        );
        Ok(ParallelFrameTiming {
            total_us: micros(started),
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
