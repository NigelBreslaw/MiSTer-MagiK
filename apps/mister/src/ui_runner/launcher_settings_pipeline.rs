// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! One-frame-ahead producer for the fixed 960x540 Home <-> Settings animation.

use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::settings_cog::render_settings_cog_transition_into;
use std::collections::BTreeSet;
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::thread::JoinHandle;
use std::time::Instant;

const FRAME_PIXELS: usize = 960 * 540;
const WORKER_COUNT: usize = 1;
const BUFFER_COUNT: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SettingsFrameRequest {
    pub(super) sequence: u64,
    pub(super) target_vblank: u64,
    pub(super) t_ms: u32,
}

pub(super) struct PreparedSettingsFrame {
    request: SettingsFrameRequest,
    pixels: Vec<Rgb565Pixel>,
    render_us: u64,
    completed_at: Instant,
}

impl PreparedSettingsFrame {
    pub(super) const fn request(&self) -> SettingsFrameRequest {
        self.request
    }

    pub(super) fn pixels(&self) -> &[Rgb565Pixel] {
        &self.pixels
    }

    pub(super) const fn render_us(&self) -> u64 {
        self.render_us
    }

    pub(super) const fn completed_at(&self) -> Instant {
        self.completed_at
    }
}

pub(super) struct SettingsCogRenderAhead {
    request_txs: Vec<SyncSender<(SettingsFrameRequest, Vec<Rgb565Pixel>)>>,
    completed_rx: Receiver<PreparedSettingsFrame>,
    free_tx: SyncSender<Vec<Rgb565Pixel>>,
    free_rx: Receiver<Vec<Rgb565Pixel>>,
    workers: Vec<JoinHandle<()>>,
    ready: Vec<PreparedSettingsFrame>,
    pending_targets: BTreeSet<u64>,
    next_worker: usize,
}

impl SettingsCogRenderAhead {
    pub(super) fn start(
        launcher: Vec<Rgb565Pixel>,
        settings: Vec<Rgb565Pixel>,
        cog: &'static [Rgb565Pixel],
    ) -> Option<Self> {
        if launcher.len() != FRAME_PIXELS
            || settings.len() != FRAME_PIXELS
            || cog.len()
                != mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_WIDTH
                    * mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_HEIGHT
        {
            return None;
        }
        let (completed_tx, completed_rx) = channel();
        let (free_tx, free_rx) = sync_channel(BUFFER_COUNT);
        for _ in 0..BUFFER_COUNT {
            free_tx.send(vec![Rgb565Pixel(0); FRAME_PIXELS]).ok()?;
        }
        let mut request_txs = Vec::with_capacity(WORKER_COUNT);
        let mut workers = Vec::with_capacity(WORKER_COUNT);
        for worker_index in 0..WORKER_COUNT {
            let (request_tx, request_rx) = sync_channel(1);
            let worker_launcher = launcher.clone();
            let worker_settings = settings.clone();
            let worker_completed = completed_tx.clone();
            let role = if worker_index == 0 {
                RuntimeThreadRole::LauncherCardRenderer
            } else {
                RuntimeThreadRole::LauncherCardRendererSecondary
            };
            let worker = std::thread::Builder::new()
                .name(format!("settings-cog-ahead-{worker_index}"))
                .spawn(move || {
                    run_worker(
                        request_rx,
                        worker_completed,
                        worker_launcher,
                        worker_settings,
                        cog,
                        role,
                    )
                })
                .ok()?;
            request_txs.push(request_tx);
            workers.push(worker);
        }
        Some(Self {
            request_txs,
            completed_rx,
            free_tx,
            free_rx,
            workers,
            ready: Vec::with_capacity(BUFFER_COUNT),
            pending_targets: BTreeSet::new(),
            next_worker: 0,
        })
    }

    pub(super) fn submit(&mut self, request: SettingsFrameRequest) {
        self.drain_completed();
        if self.pending_targets.contains(&request.target_vblank) {
            return;
        }
        let Ok(buffer) = self.free_rx.try_recv() else {
            return;
        };
        let mut work = Some((request, buffer));
        for offset in 0..self.request_txs.len() {
            let worker_index = (self.next_worker + offset) % self.request_txs.len();
            match self.request_txs[worker_index].try_send(work.take().unwrap()) {
                Ok(()) => {
                    self.pending_targets.insert(request.target_vblank);
                    self.next_worker = (worker_index + 1) % self.request_txs.len();
                    return;
                }
                Err(std::sync::mpsc::TrySendError::Full(returned))
                | Err(std::sync::mpsc::TrySendError::Disconnected(returned)) => {
                    work = Some(returned);
                }
            }
        }
        let (_, buffer) = work.unwrap();
        let _ = self.free_tx.try_send(buffer);
    }

    pub(super) fn take_for_vblank(&mut self, target_vblank: u64) -> Option<PreparedSettingsFrame> {
        self.drain_completed();
        let mut index = 0;
        while index < self.ready.len() {
            if self.ready[index].request.target_vblank < target_vblank {
                let stale = self.ready.swap_remove(index);
                self.pending_targets.remove(&stale.request.target_vblank);
                self.recycle(stale);
            } else {
                index += 1;
            }
        }
        let index = self
            .ready
            .iter()
            .position(|frame| frame.request.target_vblank == target_vblank)?;
        let ready = self.ready.swap_remove(index);
        self.pending_targets.remove(&target_vblank);
        Some(ready)
    }

    pub(super) fn recycle(&mut self, frame: PreparedSettingsFrame) {
        let _ = self.free_tx.try_send(frame.pixels);
    }

    fn drain_completed(&mut self) {
        while let Ok(completed) = self.completed_rx.try_recv() {
            self.ready.push(completed);
        }
    }
}

impl Drop for SettingsCogRenderAhead {
    fn drop(&mut self) {
        self.request_txs.clear();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn run_worker(
    requests: Receiver<(SettingsFrameRequest, Vec<Rgb565Pixel>)>,
    completed: Sender<PreparedSettingsFrame>,
    launcher: Vec<Rgb565Pixel>,
    settings: Vec<Rgb565Pixel>,
    cog: &'static [Rgb565Pixel],
    role: RuntimeThreadRole,
) {
    apply_runtime_thread_policy(role);
    while let Ok((request, mut pixels)) = requests.recv() {
        let started = Instant::now();
        if !render_settings_cog_transition_into(
            &launcher,
            &settings,
            cog,
            request.t_ms,
            &mut pixels,
        ) {
            continue;
        }
        let render_us = started.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
        if completed
            .send(PreparedSettingsFrame {
                request,
                pixels,
                render_us,
                completed_at: Instant::now(),
            })
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn take(pipeline: &mut SettingsCogRenderAhead, target_vblank: u64) -> PreparedSettingsFrame {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(frame) = pipeline.take_for_vblank(target_vblank) {
                return frame;
            }
            assert!(Instant::now() < deadline, "Settings producer timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn prepared_frames_preserve_target_vblank_and_exact_endpoints() {
        let launcher = vec![Rgb565Pixel(0x1234); FRAME_PIXELS];
        let settings = vec![Rgb565Pixel(0x4321); FRAME_PIXELS];
        let cog = Box::leak(
            vec![
                Rgb565Pixel(0);
                mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_WIDTH
                    * mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_HEIGHT
            ]
            .into_boxed_slice(),
        );
        let mut pipeline =
            SettingsCogRenderAhead::start(launcher, settings, cog).expect("valid pipeline");

        pipeline.submit(SettingsFrameRequest {
            sequence: 7,
            target_vblank: 12,
            t_ms: 0,
        });
        let first = take(&mut pipeline, 12);
        assert_eq!(first.request().sequence, 7);
        assert!(first.pixels().iter().all(|pixel| pixel.0 == 0x1234));
        pipeline.recycle(first);

        pipeline.submit(SettingsFrameRequest {
            sequence: 8,
            target_vblank: 13,
            t_ms: 1_000,
        });
        let last = take(&mut pipeline, 13);
        assert!(last.pixels().iter().all(|pixel| pixel.0 == 0x4321));
    }

    #[test]
    fn duplicate_target_is_only_queued_once() {
        let launcher = vec![Rgb565Pixel(0x1234); FRAME_PIXELS];
        let settings = vec![Rgb565Pixel(0x4321); FRAME_PIXELS];
        let cog = Box::leak(
            vec![
                Rgb565Pixel(0);
                mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_WIDTH
                    * mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_HEIGHT
            ]
            .into_boxed_slice(),
        );
        let mut pipeline =
            SettingsCogRenderAhead::start(launcher, settings, cog).expect("valid pipeline");

        pipeline.submit(SettingsFrameRequest {
            sequence: 1,
            target_vblank: 21,
            t_ms: 400,
        });
        pipeline.submit(SettingsFrameRequest {
            sequence: 2,
            target_vblank: 21,
            t_ms: 417,
        });
        let first = take(&mut pipeline, 21);
        assert_eq!(first.request().sequence, 1);
        assert!(pipeline.take_for_vblank(21).is_none());
    }
}
