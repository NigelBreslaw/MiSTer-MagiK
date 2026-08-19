// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::raw565_preview_renderer::{
    PreviewSurface, Raw565PreviewRenderer, compose_cut_frame_oriented, preview_screen_rect,
};
use super::*;
use crate::preview_state::{
    OwnedPreviewRawTransitionFrame, PreviewRawFrame, PreviewRawTransitionFrame,
};
use mister_magik_catalog::runtime_thread::{
    RuntimeThreadPolicyReport, RuntimeThreadRole, apply_runtime_thread_policy,
};
use mister_magik_framebuffer_scenes::{
    OutputRotation, Rgb565OutputLayout, Rgb565Rect, Rgb565SurfaceMut,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PreviewCompositionWorkKey {
    pub(super) layout: Rgb565OutputLayout,
    pub(super) generation: u64,
    pub(super) token: u64,
}

pub(super) struct PreviewCompositionRequest {
    pub(super) key: PreviewCompositionWorkKey,
    pub(super) frame: OwnedPreviewRawTransitionFrame,
    pub(super) effect: PreviewTransitionEffect,
    pub(super) progress: f32,
    pub(super) active: bool,
    submitted_at: Instant,
}

impl PreviewCompositionRequest {
    pub(super) fn new(
        key: PreviewCompositionWorkKey,
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

pub(super) struct PreviewCompositionResult {
    pub(super) key: PreviewCompositionWorkKey,
    pub(super) rect: DirtyRect,
    pub(super) pixels: Vec<Rgb565Pixel>,
    pub(super) fade: PreviewFadeTrace,
    pub(super) age_us: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct PreviewCompositorTelemetry {
    pub(super) queue_replacements: u64,
    pub(super) result_replacements: u64,
    pub(super) stale_results: u64,
    pub(super) worker_age_us: u64,
    pub(super) generation_lag: u64,
    pub(super) affinity_status: &'static str,
    pub(super) worker_errors: u64,
    pub(super) adoption_failures: u64,
    pub(super) worker_alive: bool,
}

#[derive(Default)]
struct WorkerState {
    request: Option<PreviewCompositionRequest>,
    result: Option<PreviewCompositionResult>,
    recycled: Vec<Vec<Rgb565Pixel>>,
    shutdown: bool,
    queue_replacements: u64,
    result_replacements: u64,
    stale_results: u64,
    latest_age_us: u64,
    affinity: Option<RuntimeThreadPolicyReport>,
    failed_key: Option<PreviewCompositionWorkKey>,
    worker_errors: u64,
    adoption_failures: u64,
    consecutive_failures: u8,
    worker_alive: bool,
    disabled: bool,
    profile_flush_requested: u64,
    profile_flush_completed: u64,
}

struct SharedWorker {
    state: Mutex<WorkerState>,
    ready: Condvar,
}

pub(super) struct PreviewCompositor {
    shared: Arc<SharedWorker>,
    thread: Option<JoinHandle<()>>,
    last_submitted: Option<PreviewCompositionWorkKey>,
}

impl PreviewCompositor {
    pub(super) fn start() -> Result<Self, String> {
        let shared = Arc::new(SharedWorker {
            state: Mutex::new(WorkerState::default()),
            ready: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("preview-compositor".to_string())
            .spawn(move || {
                let panicked =
                    catch_unwind(AssertUnwindSafe(|| run_worker(Arc::clone(&worker_shared))))
                        .is_err();
                let mut state = worker_shared
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                state.worker_alive = false;
                if panicked {
                    state.worker_errors = state.worker_errors.saturating_add(1);
                    state.disabled = true;
                }
            })
            .map_err(|error| format!("start preview compositor: {error}"))?;
        Ok(Self {
            shared,
            thread: Some(thread),
            last_submitted: None,
        })
    }

    pub(super) fn queue(&mut self, request: PreviewCompositionRequest) -> bool {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.disabled || !state.worker_alive {
            return false;
        }
        let retry = state.failed_key == Some(request.key);
        if self.last_submitted == Some(request.key) && !retry {
            return false;
        }
        if retry {
            state.failed_key = None;
        }
        self.last_submitted = Some(request.key);
        if state.request.replace(request).is_some() {
            state.queue_replacements = state.queue_replacements.saturating_add(1);
        }
        true
    }

    pub(super) fn release_queued(&self) {
        self.shared.ready.notify_one();
    }

    pub(super) fn take_current(
        &self,
        expected: PreviewCompositionWorkKey,
    ) -> Option<PreviewCompositionResult> {
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

    pub(super) fn available(&self) -> bool {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.worker_alive && !state.disabled
    }

    pub(super) fn needs_retry(&self, key: PreviewCompositionWorkKey) -> bool {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.failed_key == Some(key)
    }

    pub(super) fn note_adoption_failed(&mut self, key: PreviewCompositionWorkKey) {
        self.last_submitted = None;
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.failed_key = Some(key);
        state.adoption_failures = state.adoption_failures.saturating_add(1);
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures >= 3 {
            state.disabled = true;
        }
    }

    pub(super) fn telemetry(&self, current_generation: u64) -> PreviewCompositorTelemetry {
        let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let pending_generation = state
            .request
            .as_ref()
            .map(|request| request.key.generation)
            .or_else(|| state.result.as_ref().map(|result| result.key.generation))
            .unwrap_or(current_generation);
        PreviewCompositorTelemetry {
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
            worker_errors: state.worker_errors,
            adoption_failures: state.adoption_failures,
            worker_alive: state.worker_alive && !state.disabled,
        }
    }

    pub(super) fn flush_pmu_profile(&self, timeout: Duration) -> bool {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.worker_alive {
            return false;
        }
        let requested = state.profile_flush_requested.saturating_add(1);
        state.profile_flush_requested = requested;
        self.shared.ready.notify_one();
        let (state, _) = self
            .shared
            .ready
            .wait_timeout_while(state, timeout, |state| {
                state.worker_alive && state.profile_flush_completed < requested
            })
            .unwrap_or_else(|error| error.into_inner());
        state.profile_flush_completed >= requested
    }
}

impl Drop for PreviewCompositor {
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
        state.worker_alive = true;
    }
    let mut logical = Vec::new();
    loop {
        let (request, mut physical) = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.request.is_none()
                && !state.shutdown
                && state.profile_flush_completed == state.profile_flush_requested
            {
                state = shared.ready.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            if state.shutdown {
                drop(state);
                mister_magik_perf_events::submit_thread_profile("preview-compositor");
                return;
            }
            if state.profile_flush_completed < state.profile_flush_requested {
                let requested = state.profile_flush_requested;
                drop(state);
                mister_magik_perf_events::submit_thread_profile("preview-compositor");
                let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                state.profile_flush_completed = requested;
                shared.ready.notify_all();
                continue;
            }
            (
                state.request.take().expect("worker request"),
                state.recycled.pop().unwrap_or_default(),
            )
        };
        let key = request.key;
        let request_pmu = mister_magik_perf_events::sampled_span("gui.worker.preview-composition");
        let result = compose_request(request, &mut logical, &mut physical);
        drop(request_pmu);
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(result) => {
                state.failed_key = None;
                state.consecutive_failures = 0;
                state.latest_age_us = result.age_us;
                if let Some(replaced) = state.result.replace(result) {
                    state.result_replacements = state.result_replacements.saturating_add(1);
                    state.recycled.push(replaced.pixels);
                }
            }
            Err(error) => {
                state.failed_key = Some(key);
                state.worker_errors = state.worker_errors.saturating_add(1);
                state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                if state.recycled.len() < 2 {
                    state.recycled.push(physical);
                }
                if state.consecutive_failures >= 3 {
                    state.disabled = true;
                }
                crate::ui_errln!("preview_compositor_failed: {error}");
            }
        }
    }
}

#[unsafe(export_name = "mister_magik_preview_worker_compose")]
#[inline(never)]
fn compose_request(
    request: PreviewCompositionRequest,
    logical: &mut Vec<Rgb565Pixel>,
    physical: &mut Vec<Rgb565Pixel>,
) -> Result<PreviewCompositionResult, String> {
    let output = request.key.layout;
    let ui = UiDisplay::for_framebuffer(output.logical_width(), output.logical_height());
    let screen = preview_screen_rect(&ui);
    let frame = request.frame.borrowed();
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
    let alpha = (request.progress.clamp(0.0, 1.0) * 255.0).round() as u8;
    let cut_frame = if !request.active {
        Some((&frame.current, 0, false))
    } else if alpha == 0 {
        frame.previous.as_ref().map(|previous| (previous, 0, true))
    } else if alpha == 255 {
        Some((&frame.current, 32, true))
    } else {
        None
    };
    if let Some((cut_frame, alpha_bucket, report_cut)) = cut_frame
        && let Some(cut_trace) = {
            let cut_pmu = mister_magik_perf_events::sampled_span("gui.worker.preview-cut");
            let result =
                compose_worker_cut(physical, &ui, screen, local_output, cut_frame, alpha_bucket);
            drop(cut_pmu);
            result
        }
    {
        return Ok(PreviewCompositionResult {
            key: request.key,
            rect,
            pixels: std::mem::take(physical),
            fade: report_cut.then_some(cut_trace).unwrap_or_default(),
            age_us: request
                .submitted_at
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64,
        });
    }
    let identity_output = output.rotation() == OutputRotation::None;
    let composed = if identity_output {
        &mut *physical
    } else {
        &mut *logical
    };
    composed.resize(
        screen.width().saturating_mul(screen.rows() as usize),
        Rgb565Pixel(0),
    );
    let blend_pmu = mister_magik_perf_events::sampled_span("gui.worker.preview-blend");
    let fade = compose_worker_blend(
        composed,
        &ui,
        &frame,
        request.effect,
        request.progress,
        request.active,
        screen,
    )?;
    drop(blend_pmu);
    if identity_output {
        return Ok(PreviewCompositionResult {
            key: request.key,
            rect,
            pixels: std::mem::take(physical),
            fade,
            age_us: request
                .submitted_at
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64,
        });
    }
    let rotation_pmu = mister_magik_perf_events::sampled_span("gui.worker.preview-rotation");
    let copied = compose_worker_rotation(physical, local_output, logical, screen);
    drop(rotation_pmu);
    if !copied {
        return Err("preview physical rotation failed".to_string());
    }
    Ok(PreviewCompositionResult {
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

#[unsafe(export_name = "mister_magik_preview_worker_cut")]
#[inline(never)]
fn compose_worker_cut(
    destination: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    screen: DirtyRect,
    output: Rgb565OutputLayout,
    frame: &PreviewRawFrame<'_>,
    alpha_bucket: u8,
) -> Option<PreviewFadeTrace> {
    compose_cut_frame_oriented(destination, ui, screen, output, frame, alpha_bucket)
}

#[unsafe(export_name = "mister_magik_preview_worker_blend")]
#[inline(never)]
fn compose_worker_blend(
    destination: &mut [Rgb565Pixel],
    ui: &UiDisplay,
    frame: &PreviewRawTransitionFrame<'_>,
    effect: PreviewTransitionEffect,
    progress: f32,
    active: bool,
    screen: DirtyRect,
) -> Result<PreviewFadeTrace, String> {
    let surface = PreviewSurface {
        x0: screen.x0,
        y0: screen.y0,
        stride: screen.width(),
    };
    if active {
        Ok(Raw565PreviewRenderer::compose_transition_strided(
            destination,
            ui,
            frame,
            effect,
            progress,
            surface,
        )
        .1)
    } else {
        Raw565PreviewRenderer::compose_frame_strided(
            destination,
            ui,
            &frame.current,
            true,
            surface,
        )
        .ok_or_else(|| "preview frame composition returned no rectangle".to_string())?;
        Ok(PreviewFadeTrace::default())
    }
}

#[unsafe(export_name = "mister_magik_preview_worker_rotation")]
#[inline(never)]
fn compose_worker_rotation(
    destination: &mut [Rgb565Pixel],
    output: Rgb565OutputLayout,
    logical: &[Rgb565Pixel],
    screen: DirtyRect,
) -> bool {
    Rgb565SurfaceMut::new(destination, output).is_ok_and(|surface| {
        surface.copy_rect_strided(
            0,
            0,
            screen.width(),
            screen.rows() as usize,
            logical,
            screen.width(),
            0,
            0,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_state::OwnedPreviewRawFrame;

    #[test]
    fn latest_request_replaces_unstarted_work() {
        let state = WorkerState {
            worker_alive: true,
            ..WorkerState::default()
        };
        let shared = Arc::new(SharedWorker {
            state: Mutex::new(state),
            ready: Condvar::new(),
        });
        let key = |generation| PreviewCompositionWorkKey {
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
        let compositor = PreviewCompositor {
            shared,
            thread: None,
            last_submitted: None,
        };
        let mut compositor = compositor;
        assert!(compositor.queue(PreviewCompositionRequest::new(
            key(1),
            frame.clone(),
            PreviewTransitionEffect::Fade,
            1.0,
            false,
        )));
        assert!(compositor.queue(PreviewCompositionRequest::new(
            key(2),
            frame,
            PreviewTransitionEffect::Fade,
            1.0,
            false,
        )));
        let state = compositor.shared.state.lock().unwrap();
        assert_eq!(state.queue_replacements, 1);
        assert_eq!(state.request.as_ref().unwrap().key.generation, 2);
    }

    #[test]
    fn failed_key_can_be_submitted_again() {
        let state = WorkerState {
            worker_alive: true,
            ..WorkerState::default()
        };
        let shared = Arc::new(SharedWorker {
            state: Mutex::new(state),
            ready: Condvar::new(),
        });
        let mut compositor = PreviewCompositor {
            shared,
            thread: None,
            last_submitted: None,
        };
        let key = PreviewCompositionWorkKey {
            layout: Rgb565OutputLayout::new(4, 3, 4, OutputRotation::None).unwrap(),
            generation: 3,
            token: 5,
        };
        let request = || {
            PreviewCompositionRequest::new(
                key,
                OwnedPreviewRawTransitionFrame {
                    previous: None,
                    current: OwnedPreviewRawFrame::empty(),
                    transition_id: 1,
                    duration_numerator: 1,
                    duration_denominator: 1,
                },
                PreviewTransitionEffect::Fade,
                1.0,
                false,
            )
        };

        assert!(compositor.queue(request()));
        {
            let mut state = compositor.shared.state.lock().unwrap();
            state.request = None;
            state.failed_key = Some(key);
            state.worker_errors = 1;
        }

        assert!(compositor.needs_retry(key));
        assert!(compositor.queue(request()));
        let state = compositor.shared.state.lock().unwrap();
        assert_eq!(state.request.as_ref().map(|request| request.key), Some(key));
        assert_eq!(state.worker_errors, 1);
    }

    #[test]
    fn identity_layout_composes_directly_into_dense_result_storage() {
        let layout = Rgb565OutputLayout::new(960, 540, 960, OutputRotation::None).unwrap();
        let key = PreviewCompositionWorkKey {
            layout,
            generation: 7,
            token: 11,
        };
        let frame = OwnedPreviewRawTransitionFrame {
            previous: None,
            current: OwnedPreviewRawFrame::empty(),
            transition_id: 1,
            duration_numerator: 1,
            duration_denominator: 1,
        };
        let mut logical = Vec::new();
        let mut physical = Vec::new();

        let result = compose_request(
            PreviewCompositionRequest::new(key, frame, PreviewTransitionEffect::Fade, 0.5, true),
            &mut logical,
            &mut physical,
        )
        .unwrap();

        assert!(logical.is_empty());
        assert_eq!(result.key, key);
        assert_eq!(
            result.pixels.len(),
            result.rect.width() * result.rect.rows() as usize
        );
        assert!(result.pixels.iter().all(|pixel| *pixel == Rgb565Pixel(0)));
    }
}
