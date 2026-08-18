// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::raw565_preview_renderer::{PreviewSurface, Raw565PreviewRenderer, preview_screen_rect};
use super::*;
use crate::preview_state::OwnedPreviewRawTransitionFrame;
use mister_magik_catalog::runtime_thread::{
    RuntimeThreadPolicyReport, RuntimeThreadRole, apply_runtime_thread_policy,
};
use mister_magik_framebuffer_scenes::{Rgb565OutputLayout, Rgb565Rect, Rgb565SurfaceMut};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PortraitPreviewWorkKey {
    pub(super) layout: Rgb565OutputLayout,
    pub(super) generation: u64,
    pub(super) token: u64,
}

pub(super) struct PortraitPreviewRequest {
    pub(super) key: PortraitPreviewWorkKey,
    pub(super) frame: OwnedPreviewRawTransitionFrame,
    pub(super) effect: PreviewTransitionEffect,
    pub(super) progress: f32,
    pub(super) active: bool,
    submitted_at: Instant,
}

impl PortraitPreviewRequest {
    pub(super) fn new(
        key: PortraitPreviewWorkKey,
        frame: OwnedPreviewRawTransitionFrame,
        effect: PreviewTransitionEffect,
        progress: f32,
        active: bool,
    ) -> Self {
        Self {
            key,
            frame,
            effect,
            progress,
            active,
            submitted_at: Instant::now(),
        }
    }
}

pub(super) struct PortraitPreviewResult {
    pub(super) key: PortraitPreviewWorkKey,
    pub(super) rect: DirtyRect,
    pub(super) pixels: Vec<Rgb565Pixel>,
    pub(super) fade: PreviewFadeTrace,
    pub(super) age_us: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct PortraitPreviewWorkerTelemetry {
    pub(super) queue_replacements: u64,
    pub(super) result_replacements: u64,
    pub(super) stale_results: u64,
    pub(super) worker_age_us: u64,
    pub(super) generation_lag: u64,
    pub(super) affinity_status: &'static str,
}

#[derive(Default)]
struct WorkerState {
    request: Option<PortraitPreviewRequest>,
    result: Option<PortraitPreviewResult>,
    recycled: Vec<Vec<Rgb565Pixel>>,
    shutdown: bool,
    queue_replacements: u64,
    result_replacements: u64,
    stale_results: u64,
    latest_age_us: u64,
    affinity: Option<RuntimeThreadPolicyReport>,
}

struct SharedWorker {
    state: Mutex<WorkerState>,
    ready: Condvar,
}

pub(super) struct PortraitPreviewCompositor {
    shared: Arc<SharedWorker>,
    thread: Option<JoinHandle<()>>,
    last_submitted: Option<PortraitPreviewWorkKey>,
}

impl PortraitPreviewCompositor {
    pub(super) fn start() -> Result<Self, String> {
        let shared = Arc::new(SharedWorker {
            state: Mutex::new(WorkerState::default()),
            ready: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("preview-compositor".to_string())
            .spawn(move || run_worker(worker_shared))
            .map_err(|error| format!("start portrait preview compositor: {error}"))?;
        Ok(Self {
            shared,
            thread: Some(thread),
            last_submitted: None,
        })
    }

    pub(super) fn submit(&mut self, request: PortraitPreviewRequest) {
        if self.last_submitted == Some(request.key) {
            return;
        }
        self.last_submitted = Some(request.key);
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.request.replace(request).is_some() {
            state.queue_replacements = state.queue_replacements.saturating_add(1);
        }
        self.shared.ready.notify_one();
    }

    pub(super) fn take_current(
        &self,
        expected: PortraitPreviewWorkKey,
    ) -> Option<PortraitPreviewResult> {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let result = state.result.take()?;
        if result.key != expected {
            state.stale_results = state.stale_results.saturating_add(1);
            state.recycled.push(result.pixels);
            return None;
        }
        Some(result)
    }

    pub(super) fn recycle(&self, pixels: Vec<Rgb565Pixel>) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.recycled.len() < 2 {
            state.recycled.push(pixels);
        }
    }

    pub(super) fn telemetry(&self, current_generation: u64) -> PortraitPreviewWorkerTelemetry {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let pending_generation = state
            .request
            .as_ref()
            .map(|request| request.key.generation)
            .or_else(|| state.result.as_ref().map(|result| result.key.generation))
            .unwrap_or(current_generation);
        PortraitPreviewWorkerTelemetry {
            queue_replacements: state.queue_replacements,
            result_replacements: state.result_replacements,
            stale_results: state.stale_results,
            worker_age_us: state.latest_age_us,
            generation_lag: current_generation.saturating_sub(pending_generation),
            affinity_status: state
                .affinity
                .as_ref()
                .map(|report| report.affinity_status)
                .unwrap_or("pending"),
        }
    }
}

impl Drop for PortraitPreviewCompositor {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            state.shutdown = true;
            self.shared.ready.notify_one();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_worker(shared: Arc<SharedWorker>) {
    let affinity = apply_runtime_thread_policy(RuntimeThreadRole::PreviewCompositor);
    {
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.affinity = Some(affinity);
    }
    let mut logical = Vec::new();
    loop {
        let (request, mut physical) = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.request.is_none() && !state.shutdown {
                state = shared.ready.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            if state.shutdown {
                return;
            }
            (
                state.request.take().expect("worker request"),
                state.recycled.pop().unwrap_or_default(),
            )
        };
        let result = compose_request(request, &mut logical, &mut physical);
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(result) => {
                state.latest_age_us = result.age_us;
                if let Some(replaced) = state.result.replace(result) {
                    state.result_replacements = state.result_replacements.saturating_add(1);
                    state.recycled.push(replaced.pixels);
                }
            }
            Err(error) => crate::ui_errln!("portrait_preview_compositor_failed: {error}"),
        }
    }
}

fn compose_request(
    request: PortraitPreviewRequest,
    logical: &mut Vec<Rgb565Pixel>,
    physical: &mut Vec<Rgb565Pixel>,
) -> Result<PortraitPreviewResult, String> {
    let output = request.key.layout;
    let ui = UiDisplay::for_framebuffer(output.logical_width(), output.logical_height());
    let screen = preview_screen_rect(&ui);
    logical.resize(
        screen.width().saturating_mul(screen.rows() as usize),
        Rgb565Pixel(0),
    );
    let frame = request.frame.borrowed();
    let fade = if request.active {
        Raw565PreviewRenderer::compose_transition_strided(
            logical,
            &ui,
            &frame,
            request.effect,
            request.progress,
            PreviewSurface {
                x0: screen.x0,
                y0: screen.y0,
                stride: screen.width(),
            },
        )
        .1
    } else {
        Raw565PreviewRenderer::compose_frame_strided(
            logical,
            &ui,
            &frame.current,
            true,
            PreviewSurface {
                x0: screen.x0,
                y0: screen.y0,
                stride: screen.width(),
            },
        )
        .ok_or_else(|| "preview frame composition returned no rectangle".to_string())?;
        PreviewFadeTrace::default()
    };
    let mapped = output.logical_rect_to_physical(Rgb565Rect {
        x0: screen.x0,
        y0: screen.y0,
        x1: screen.x1,
        y1: screen.y1,
    });
    let rect = DirtyRect {
        x0: mapped.x0,
        y0: mapped.y0,
        x1: mapped.x1,
        y1: mapped.y1,
    };
    let local_output = Rgb565OutputLayout::new(
        screen.width(),
        screen.rows() as usize,
        rect.width(),
        output.rotation(),
    )
    .map_err(|error| error.to_string())?;
    physical.resize(local_output.len(), Rgb565Pixel(0));
    let copied = Rgb565SurfaceMut::new(physical, local_output)
        .map_err(|error| error.to_string())?
        .copy_rect_strided(
            0,
            0,
            screen.width(),
            screen.rows() as usize,
            logical,
            screen.width(),
            0,
            0,
        );
    if !copied {
        return Err("preview physical rotation failed".to_string());
    }
    Ok(PortraitPreviewResult {
        key: request.key,
        rect,
        pixels: std::mem::take(physical),
        fade,
        age_us: request
            .submitted_at
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_state::OwnedPreviewRawFrame;

    #[test]
    fn latest_request_replaces_unstarted_work() {
        let shared = Arc::new(SharedWorker {
            state: Mutex::new(WorkerState::default()),
            ready: Condvar::new(),
        });
        let key = |generation| PortraitPreviewWorkKey {
            layout: Rgb565OutputLayout::new(
                3,
                4,
                4,
                mister_magik_framebuffer_scenes::OutputRotation::Clockwise90,
            )
            .unwrap(),
            generation,
            token: generation,
        };
        let frame = OwnedPreviewRawTransitionFrame {
            previous: None,
            current: OwnedPreviewRawFrame::empty(),
            transition_id: 1,
            duration_numerator: 1,
            duration_denominator: 1,
        };
        let compositor = PortraitPreviewCompositor {
            shared,
            thread: None,
            last_submitted: None,
        };
        let mut compositor = compositor;
        compositor.submit(PortraitPreviewRequest::new(
            key(1),
            frame.clone(),
            PreviewTransitionEffect::Fade,
            1.0,
            false,
        ));
        compositor.submit(PortraitPreviewRequest::new(
            key(2),
            frame,
            PreviewTransitionEffect::Fade,
            1.0,
            false,
        ));
        let state = compositor.shared.state.lock().unwrap();
        assert_eq!(state.queue_replacements, 1);
        assert_eq!(state.request.as_ref().unwrap().key.generation, 2);
    }
}
