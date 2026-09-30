// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Persistent producer for the fixed 960x540 Home <-> Settings animation.

use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::settings_cog::{
    CogArtwork, CogTexture, render_settings_cog_transition_into,
};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};
use std::sync::{Arc, Weak};
use std::time::Instant;

const FRAME_PIXELS: usize = 960 * 540;
const BUFFER_COUNT: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SettingsFrameRequest {
    pub(super) target_vblank: u64,
    pub(super) t_ms: u32,
}

pub(super) struct PreparedSettingsFrame {
    generation: u64,
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
struct Work {
    generation: u64,
    request: SettingsFrameRequest,
    launcher: Arc<Vec<Rgb565Pixel>>,
    settings: Arc<Vec<Rgb565Pixel>>,
    cog: &'static CogArtwork,
    pixels: Vec<Rgb565Pixel>,
}
struct Endpoints {
    launcher: Weak<Vec<Rgb565Pixel>>,
    settings: Weak<Vec<Rgb565Pixel>>,
    cog: &'static CogArtwork,
}

/// Created once during launcher initialization. The worker owns pool allocation;
/// transition submission only shares immutable snapshots and moves free buffers.
pub(super) struct SettingsCogSession {
    requests: Option<SyncSender<Work>>,
    completed: Receiver<PreparedSettingsFrame>,
    free_tx: SyncSender<Vec<Rgb565Pixel>>,
    free_rx: Receiver<Vec<Rgb565Pixel>>,
    ready: VecDeque<PreparedSettingsFrame>,
    generation: u64,
    endpoints: Option<Endpoints>,
    last_submitted_target: Option<u64>,
}
impl SettingsCogSession {
    pub(super) fn new() -> Self {
        Self::with_initializer(|| {})
    }
    fn with_initializer(initialize: impl FnOnce() + Send + 'static) -> Self {
        let (requests, request_rx) = sync_channel::<Work>(1);
        let (completed_tx, completed) = channel();
        let (free_tx, free_rx) = sync_channel(BUFFER_COUNT);
        let worker_free = free_tx.clone();
        // Dropping the JoinHandle detaches this bounded worker. Disconnecting the
        // request channel ends it after at most the running and queued frame.
        let spawned = std::thread::Builder::new()
            .name("settings-cog-ahead".into())
            .spawn(move || {
                apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardRenderer);
                initialize();
                for _ in 0..BUFFER_COUNT {
                    if worker_free
                        .send(vec![Rgb565Pixel(0); FRAME_PIXELS])
                        .is_err()
                    {
                        return;
                    }
                }
                let mut texture: Option<(&'static CogArtwork, CogTexture)> = None;
                while let Ok(mut work) = request_rx.recv() {
                    if !texture
                        .as_ref()
                        .is_some_and(|(artwork, _)| std::ptr::eq(*artwork, work.cog))
                    {
                        texture = Some((work.cog, CogTexture::from_artwork(work.cog)));
                    }
                    let started = Instant::now();
                    if !render_settings_cog_transition_into(
                        &work.launcher,
                        &work.settings,
                        &texture.as_ref().unwrap().1,
                        work.request.t_ms,
                        &mut work.pixels,
                    ) {
                        let _ = worker_free.try_send(work.pixels);
                        continue;
                    }
                    if completed_tx
                        .send(PreparedSettingsFrame {
                            generation: work.generation,
                            request: work.request,
                            pixels: work.pixels,
                            render_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                            completed_at: Instant::now(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .is_ok();
        Self {
            requests: spawned.then_some(requests),
            completed,
            free_tx,
            free_rx,
            ready: VecDeque::with_capacity(BUFFER_COUNT),
            generation: 0,
            endpoints: None,
            last_submitted_target: None,
        }
    }
    pub(super) fn clear(&mut self) {
        if self.endpoints.take().is_some() {
            self.generation = self.generation.wrapping_add(1);
            self.last_submitted_target = None;
        }
        while let Some(frame) = self.ready.pop_front() {
            self.recycle(frame);
        }
        self.drain_completed();
    }
    pub(super) fn submit(
        &mut self,
        launcher: Arc<Vec<Rgb565Pixel>>,
        settings: Arc<Vec<Rgb565Pixel>>,
        cog: &'static CogArtwork,
        request: SettingsFrameRequest,
    ) {
        if launcher.len() != FRAME_PIXELS || settings.len() != FRAME_PIXELS {
            return;
        }
        let same = self.endpoints.as_ref().is_some_and(|e| {
            e.launcher.as_ptr() == Arc::as_ptr(&launcher)
                && e.settings.as_ptr() == Arc::as_ptr(&settings)
                && std::ptr::eq(e.cog, cog)
        });
        if !same {
            self.clear();
            self.endpoints = Some(Endpoints {
                launcher: Arc::downgrade(&launcher),
                settings: Arc::downgrade(&settings),
                cog,
            });
            self.generation = self.generation.wrapping_add(1);
        }
        self.drain_completed();
        if self.last_submitted_target == Some(request.target_vblank) {
            return;
        }
        let Ok(pixels) = self.free_rx.try_recv() else {
            return;
        };
        let work = Work {
            generation: self.generation,
            request,
            launcher,
            settings,
            cog,
            pixels,
        };
        let Some(requests) = self.requests.as_ref() else {
            let _ = self.free_tx.try_send(work.pixels);
            return;
        };
        match requests.try_send(work) {
            Ok(()) => self.last_submitted_target = Some(request.target_vblank),
            Err(std::sync::mpsc::TrySendError::Full(work))
            | Err(std::sync::mpsc::TrySendError::Disconnected(work)) => {
                let _ = self.free_tx.try_send(work.pixels);
            }
        }
    }
    pub(super) fn take_for_vblank(&mut self, target: u64) -> Option<PreparedSettingsFrame> {
        self.drain_completed();
        while self
            .ready
            .front()
            .is_some_and(|f| f.request.target_vblank < target)
        {
            let stale = self.ready.pop_front().expect("front present");
            self.recycle(stale);
        }
        self.ready
            .front()
            .is_some_and(|f| f.request.target_vblank == target)
            .then(|| self.ready.pop_front().expect("front present"))
    }
    pub(super) fn recycle(&mut self, frame: PreparedSettingsFrame) {
        let _ = self.free_tx.try_send(frame.pixels);
    }
    fn drain_completed(&mut self) {
        while let Ok(frame) = self.completed.try_recv() {
            if self.endpoints.is_some() && frame.generation == self.generation {
                self.ready.push_back(frame);
            } else {
                self.recycle(frame);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn settings_transition_lifecycle_has_no_launcher_pixel_allocations() {
        let launcher = std::sync::Arc::new(vec![Rgb565Pixel(0x1234); FRAME_PIXELS]);
        let settings = std::sync::Arc::new(vec![Rgb565Pixel(0x4321); FRAME_PIXELS]);
        let cog = Box::leak(Box::new(
            CogArtwork::from_rgb888(&vec![
                0;
                mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_WIDTH
                    * mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_HEIGHT
                    * 3
            ])
            .unwrap(),
        ));
        let mut session = SettingsCogSession::new();
        let mut measurements = Vec::new();
        for generation in 1..=2 {
            crate::allocation_metrics::begin();
            session.submit(
                launcher.clone(),
                settings.clone(),
                cog,
                SettingsFrameRequest {
                    target_vblank: generation,
                    t_ms: 0,
                },
            );
            let allocated = crate::allocation_metrics::finish();
            let deadline = Instant::now() + Duration::from_secs(3);
            let frame = loop {
                // Submission is retried if the asynchronously initialized pool was not ready.
                session.submit(
                    launcher.clone(),
                    settings.clone(),
                    cog,
                    SettingsFrameRequest {
                        target_vblank: generation,
                        t_ms: 0,
                    },
                );
                if let Some(frame) = session.take_for_vblank(generation) {
                    break frame;
                }
                assert!(Instant::now() < deadline, "pool never became ready");
                std::thread::yield_now();
            };
            assert!(frame.pixels().iter().all(|p| p.0 == 0x1234));
            session.recycle(frame);
            session.clear();
            measurements.push(allocated.bytes);
        }
        println!("settings_transition_ui_allocated_bytes={measurements:?}");
        assert!(
            measurements
                .iter()
                .all(|bytes| *bytes < FRAME_PIXELS as u64 * 2),
            "pixel buffers allocated on launcher thread"
        );
    }

    #[test]
    fn blocked_initialization_does_not_block_transition_input_or_retirement() {
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let (go_tx, go_rx) = channel();
        let (input_tx, input_rx) = channel();
        let ui = std::thread::spawn(move || {
            let mut session = SettingsCogSession::with_initializer(move || {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv();
            });
            go_rx.recv().unwrap();
            let launcher = Arc::new(vec![Rgb565Pixel(7); FRAME_PIXELS]);
            let settings = Arc::new(vec![Rgb565Pixel(9); FRAME_PIXELS]);
            let cog = Box::leak(Box::new(
                CogArtwork::from_rgb888(&vec![
                    0;
                    mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_WIDTH
                        * mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_HEIGHT
                        * 3
                ])
                .unwrap(),
            ));
            session.submit(
                launcher,
                settings,
                cog,
                SettingsFrameRequest {
                    target_vblank: 1,
                    t_ms: 0,
                },
            );
            assert!(session.take_for_vblank(1).is_none());
            session.clear();
            drop(session);
            let mut nav = crate::launcher::LauncherNav::new();
            let catalog = crate::arcade_catalog::ArcadeCatalog::new(
                std::path::PathBuf::new(),
                vec![],
                vec![],
            );
            let pad = crate::input::PadState {
                dpad_right: true,
                ..Default::default()
            };
            nav.handle_input(&pad, Instant::now(), &catalog);
            input_tx.send(nav.selected).unwrap();
        });
        let entered = entered_rx.recv_timeout(Duration::from_secs(3));
        let _ = go_tx.send(());
        let input = input_rx.recv_timeout(Duration::from_secs(3));
        // Always release the watchdog gate before asserting or joining.
        let _ = release_tx.send(());
        ui.join().unwrap();
        assert!(entered.is_ok());
        assert_eq!(
            input.unwrap(),
            1,
            "actual Home navigation must precede worker release"
        );
    }
    #[test]
    fn latest_generation_and_duplicate_targets_preserve_exact_pixels() {
        let mut session = SettingsCogSession::new();
        let launcher = Arc::new(vec![Rgb565Pixel(0x1234); FRAME_PIXELS]);
        let settings = Arc::new(vec![Rgb565Pixel(0x4321); FRAME_PIXELS]);
        let cog = Box::leak(Box::new(
            CogArtwork::from_rgb888(&vec![
                0;
                mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_WIDTH
                    * mister_magik_framebuffer_scenes::settings_cog::COG_ASSET_HEIGHT
                    * 3
            ])
            .unwrap(),
        ));
        let request = SettingsFrameRequest {
            target_vblank: 21,
            t_ms: 0,
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        while session.last_submitted_target.is_none() {
            session.submit(launcher.clone(), settings.clone(), cog, request);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        session.clear();
        loop {
            session.submit(
                launcher.clone(),
                settings.clone(),
                cog,
                SettingsFrameRequest {
                    t_ms: 1000,
                    ..request
                },
            );
            if let Some(frame) = session.take_for_vblank(21) {
                assert!(frame.pixels().iter().all(|p| p.0 == 0x4321));
                session.submit(launcher.clone(), settings.clone(), cog, request);
                assert!(session.take_for_vblank(21).is_none());
                session.recycle(frame);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
