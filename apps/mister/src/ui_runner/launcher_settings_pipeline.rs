// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! One-frame-ahead producer for the fixed 960x540 Home <-> Settings animation.

use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::settings_cog::render_settings_cog_transition_into;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::thread::JoinHandle;
use std::time::Instant;

const FRAME_PIXELS: usize = 960 * 540;
const BUFFER_COUNT: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SettingsFrameRequest {
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
    request_tx: Option<SyncSender<(SettingsFrameRequest, Vec<Rgb565Pixel>)>>,
    completed_rx: Receiver<PreparedSettingsFrame>,
    free_tx: SyncSender<Vec<Rgb565Pixel>>,
    free_rx: Receiver<Vec<Rgb565Pixel>>,
    worker: Option<JoinHandle<()>>,
    ready: VecDeque<PreparedSettingsFrame>,
    last_submitted_target: Option<u64>,
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
        let (request_tx, request_rx) = sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("settings-cog-ahead".into())
            .spawn(move || run_worker(request_rx, completed_tx, launcher, settings, cog))
            .ok()?;
        Some(Self {
            request_tx: Some(request_tx),
            completed_rx,
            free_tx,
            free_rx,
            worker: Some(worker),
            ready: VecDeque::with_capacity(BUFFER_COUNT),
            last_submitted_target: None,
        })
    }

    pub(super) fn submit(&mut self, request: SettingsFrameRequest) {
        self.drain_completed();
        if self.last_submitted_target == Some(request.target_vblank) {
            return;
        }
        let Ok(buffer) = self.free_rx.try_recv() else {
            return;
        };
        let Some(request_tx) = self.request_tx.as_ref() else {
            let _ = self.free_tx.try_send(buffer);
            return;
        };
        match request_tx.try_send((request, buffer)) {
            Ok(()) => self.last_submitted_target = Some(request.target_vblank),
            Err(std::sync::mpsc::TrySendError::Full((_, buffer)))
            | Err(std::sync::mpsc::TrySendError::Disconnected((_, buffer))) => {
                let _ = self.free_tx.try_send(buffer);
            }
        }
    }

    pub(super) fn take_for_vblank(&mut self, target_vblank: u64) -> Option<PreparedSettingsFrame> {
        self.drain_completed();
        while self
            .ready
            .front()
            .is_some_and(|frame| frame.request.target_vblank < target_vblank)
        {
            let stale = self.ready.pop_front().expect("front was present");
            self.recycle(stale);
        }
        self.ready
            .front()
            .is_some_and(|frame| frame.request.target_vblank == target_vblank)
            .then(|| self.ready.pop_front().expect("front was present"))
    }

    pub(super) fn recycle(&mut self, frame: PreparedSettingsFrame) {
        let _ = self.free_tx.try_send(frame.pixels);
    }

    fn drain_completed(&mut self) {
        while let Ok(completed) = self.completed_rx.try_recv() {
            self.ready.push_back(completed);
        }
    }
}

impl Drop for SettingsCogRenderAhead {
    fn drop(&mut self) {
        self.request_tx.take();
        if let Some(worker) = self.worker.take() {
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
) {
    apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardRenderer);
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
            target_vblank: 12,
            t_ms: 0,
        });
        let first = take(&mut pipeline, 12);
        assert_eq!(first.request().target_vblank, 12);
        assert!(first.pixels().iter().all(|pixel| pixel.0 == 0x1234));
        pipeline.recycle(first);

        pipeline.submit(SettingsFrameRequest {
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
            target_vblank: 21,
            t_ms: 400,
        });
        pipeline.submit(SettingsFrameRequest {
            target_vblank: 21,
            t_ms: 417,
        });
        let first = take(&mut pipeline, 21);
        assert_eq!(first.request().t_ms, 400);
        assert!(pipeline.take_for_vblank(21).is_none());
    }
}
