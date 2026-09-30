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
#[derive(Clone, Copy, Debug, Default)]
pub struct ParallelFrameTiming {
    pub total_us: u64,
    pub primary_us: u64,
    pub secondary_us: u64,
    pub wait_us: u64,
    pub helper_start_delay_us: u64,
    pub completion_wake_us: u64,
    pub merge_us: u64,
    pub primary_cpu_us: Option<u64>,
    pub secondary_cpu_us: Option<u64>,
}
struct Job {
    preparer: LauncherFramePreparer,
    request: LauncherFrameRequest,
    buffer: PreparedLauncherFrame,
    dispatched_at: Instant,
}
struct Completion {
    buffer: PreparedLauncherFrame,
    wall_us: u64,
    cpu_us: Option<u64>,
    start_delay_us: u64,
    finished_at: Instant,
}
pub struct ParallelLauncherRenderer {
    primary: PreparedLauncherFrame,
    helper: Option<PreparedLauncherFrame>,
    requests: Option<SyncSender<Job>>,
    completions: Receiver<Completion>,
    worker: Option<JoinHandle<()>>,
    cpu_clock: Option<fn() -> Option<u64>>,
    storage_bytes: usize,
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
        cpu_clock: Option<fn() -> Option<u64>>,
    ) -> Result<Self, String> {
        let primary = preparer.new_tile_buffer();
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
                    let cpu_start = cpu_clock.and_then(|clock| clock());
                    job.preparer
                        .render_tile(job.request, &mut job.buffer, (CAROUSEL_SPLIT, 934));
                    let cpu_us = cpu_delta(cpu_start, cpu_clock.and_then(|clock| clock()));
                    let wall_us = micros(started_at);
                    let finished_at = Instant::now();
                    if completed
                        .send(Completion {
                            buffer: job.buffer,
                            wall_us,
                            cpu_us,
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
            cpu_clock,
            storage_bytes,
        })
    }
    pub fn render(
        &mut self,
        preparer: &LauncherFramePreparer,
        request: LauncherFrameRequest,
        destination: &mut [Rgb565Pixel],
    ) -> Result<ParallelFrameTiming, String> {
        let started = Instant::now();
        self.requests
            .as_ref()
            .ok_or("card renderer stopped")?
            .send(Job {
                preparer: preparer.clone(),
                request,
                buffer: self.helper.take().ok_or("helper output unavailable")?,
                dispatched_at: started,
            })
            .map_err(|e| e.to_string())?;
        let primary_started = Instant::now();
        let cpu_start = self.cpu_clock.and_then(|clock| clock());
        preparer.render_tile_into(
            request,
            &mut self.primary,
            destination,
            (296, CAROUSEL_SPLIT),
            false,
        );
        let primary_cpu_us = cpu_delta(cpu_start, self.cpu_clock.and_then(|clock| clock()));
        let primary_us = micros(primary_started);
        let waiting = Instant::now();
        let completed = self.completions.recv().map_err(|e| e.to_string())?;
        let received_at = Instant::now();
        let wait_us = micros(waiting);
        if completed.buffer.request() != Some(request) {
            return Err("mismatched current card pose".into());
        }
        let merge_started = Instant::now();
        for y in 120..495 {
            destination[y * 960 + CAROUSEL_SPLIT..y * 960 + 934].copy_from_slice(
                &completed.buffer.pixels()[y * 960 + CAROUSEL_SPLIT..y * 960 + 934],
            );
        }
        let merge_us = micros(merge_started);
        self.helper = Some(completed.buffer);
        Ok(ParallelFrameTiming {
            total_us: micros(started),
            primary_us,
            secondary_us: completed.wall_us,
            wait_us,
            helper_start_delay_us: completed.start_delay_us,
            completion_wake_us: received_at
                .saturating_duration_since(completed.finished_at)
                .as_micros() as u64,
            merge_us,
            primary_cpu_us,
            secondary_cpu_us: completed.cpu_us,
        })
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
