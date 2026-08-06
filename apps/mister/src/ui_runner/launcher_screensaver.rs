// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#![cfg_attr(target_os = "macos", allow(dead_code))]

#[cfg(not(target_os = "macos"))]
use super::*;
use crate::preview_worker;
use mister_magik_catalog::device_layout::DeviceLayout;
use mister_magik_fb::particle_engine::{ParticleConfig, ParticlePreset};
use mister_magik_framebuffer_scenes::{Rgb565Pixel as SharedRgb565Pixel, SceneGeometry};
use mister_magik_particles::recipes::{embedded_magik_recipe, parse_magik_recipe};
use mister_magik_particles::reload::{
    LastGoodRecipeFile, ReloadAction, StartupParticleRecipe, StartupParticleStatus,
    publish_startup_particle_status,
};
use mister_magik_screenshot_parade::{
    LiveScreenshotConfig, LiveScreenshotParade, ScreenshotBuffer, ScreenshotParade,
    ScreenshotParadeConfig, ScreenshotParadeStats,
};
#[cfg(target_os = "macos")]
use slint::platform::software_renderer::Rgb565Pixel;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use super::particle_renderer::ParticleRenderer;

const PARTICLE_RENDERER_LABEL: &str = "particle-magik";
const DEV_MAGIK_RECIPE_PATH: &str = "/tmp/mister-magik/startup-particles/magik.json";
const DEV_MAGIK_STATUS_PATH: &str = "/tmp/mister-magik/startup-particles/status.json";

pub struct LauncherScreensaver {
    parade: Option<ScreenshotParade>,
    particle: Option<ParticleRenderer>,
    particle_reload: Option<MagikRecipeReload>,
    startup_started_at: Option<Instant>,
    frame: u64,
    motion_started_at: Instant,
}

struct MagikRecipeReload {
    watcher: LastGoodRecipeFile<ParticleRenderer>,
    embedded_spare: Option<ParticleRenderer>,
    retire_tx: Option<Sender<ParticleRenderer>>,
    retire_worker: Option<std::thread::JoinHandle<()>>,
    current_embedded: bool,
    logical_origin: Duration,
}

impl MagikRecipeReload {
    fn for_layout(
        layout: DeviceLayout,
        width: usize,
        height: usize,
        preset: ParticlePreset,
    ) -> Result<Option<Self>, String> {
        if layout != DeviceLayout::Dev {
            return Ok(None);
        }
        publish_startup_particle_status(
            Path::new(DEV_MAGIK_STATUS_PATH),
            &StartupParticleStatus::embedded(0, StartupParticleRecipe::Magik),
        )?;
        let watcher =
            LastGoodRecipeFile::spawn(PathBuf::from(DEV_MAGIK_RECIPE_PATH), move |bytes| {
                ParticleRenderer::from_magik_recipe(
                    width,
                    height,
                    preset,
                    parse_magik_recipe(bytes)?,
                )
            })?;
        let (retire_tx, retire_rx) = mpsc::channel::<ParticleRenderer>();
        let retire_worker = std::thread::Builder::new()
            .name("particle-retire".into())
            .spawn(move || {
                while let Ok(renderer) = retire_rx.recv() {
                    drop(renderer);
                }
            })
            .map_err(|error| format!("spawn particle retirement worker: {error}"))?;
        Ok(Some(Self {
            watcher,
            embedded_spare: None,
            retire_tx: Some(retire_tx),
            retire_worker: Some(retire_worker),
            current_embedded: true,
            logical_origin: Duration::ZERO,
        }))
    }

    fn apply_latest(&mut self, renderer: &mut ParticleRenderer, elapsed: Duration) {
        let Some(attempt) = self.watcher.take_latest() else {
            return;
        };
        let generation = attempt.generation;
        let status = match attempt.action {
            ReloadAction::Apply(mut candidate) => {
                if self.current_embedded {
                    std::mem::swap(renderer, &mut candidate);
                    self.embedded_spare = Some(candidate);
                } else {
                    let retired = std::mem::replace(renderer, candidate);
                    self.retire(retired);
                }
                self.current_embedded = false;
                self.logical_origin = elapsed;
                StartupParticleStatus::applied(generation, StartupParticleRecipe::Magik)
            }
            ReloadAction::ResetToEmbedded if self.current_embedded => {
                self.logical_origin = elapsed;
                StartupParticleStatus::embedded(generation, StartupParticleRecipe::Magik)
            }
            ReloadAction::ResetToEmbedded => {
                let Some(mut embedded) = self.embedded_spare.take() else {
                    let status = StartupParticleStatus::rejected(
                        generation,
                        StartupParticleRecipe::Magik,
                        "embedded Magik renderer is unavailable",
                    );
                    Self::publish(&status);
                    return;
                };
                std::mem::swap(renderer, &mut embedded);
                self.retire(embedded);
                self.current_embedded = true;
                self.logical_origin = elapsed;
                StartupParticleStatus::embedded(generation, StartupParticleRecipe::Magik)
            }
            ReloadAction::Reject(error) => {
                StartupParticleStatus::rejected(generation, StartupParticleRecipe::Magik, &error)
            }
        };
        Self::publish(&status);
    }

    fn publish(status: &StartupParticleStatus) {
        if let Err(error) =
            publish_startup_particle_status(Path::new(DEV_MAGIK_STATUS_PATH), status)
        {
            crate::ui_errln!("particle recipe status failed: {error}");
        }
    }

    fn retire(&self, renderer: ParticleRenderer) {
        if let Some(retire_tx) = self.retire_tx.as_ref() {
            let _ = retire_tx.send(renderer);
        }
    }

    fn logical_elapsed(&self, elapsed: Duration) -> Duration {
        elapsed.saturating_sub(self.logical_origin)
    }
}

impl Drop for MagikRecipeReload {
    fn drop(&mut self) {
        self.retire_tx.take();
        if let Some(worker) = self.retire_worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScreensaverFrameTrace {
    pub(super) renderer: &'static str,
    pub(super) archive_poll_us: u128,
    pub(super) card_adopt_us: u128,
    pub(super) cards_adopted: usize,
    pub(super) parade_advance_us: u128,
    pub(super) background_us: u128,
    pub(super) draw_order_us: u128,
    pub(super) tile_blit_us: u128,
    pub(super) cards_drawn: usize,
    pub(super) cards_culled: usize,
    pub(super) raster_held_cards: usize,
    pub(super) raster_moved_cards: usize,
    pub(super) raster_hold_layer_mask: u8,
    pub(super) raster_visible_layer_mask: u8,
    pub(super) phase_bank_resident_bytes: usize,
    pub(super) render_ahead_sequence: u64,
    pub(super) render_ahead_queue_depth: usize,
    pub(super) render_ahead_frame_age_us: u64,
    pub(super) render_ahead_render_wall_us: u64,
    pub(super) render_ahead_render_cpu_us: u64,
    pub(super) render_ahead_starvation_count: u64,
    pub(super) render_ahead_superseded_frames: u64,
    pub(super) render_ahead_reused_frames: u64,
    pub(super) render_ahead_cancelled: bool,
    pub(super) particle_preset: &'static str,
    pub(super) particle_phase: &'static str,
    pub(super) particle_simulation_backend: &'static str,
    pub(super) particle_projection_backend: &'static str,
    pub(super) particle_count: usize,
    pub(super) particle_visible: usize,
    pub(super) particle_simulation_us: u128,
    pub(super) particle_simulation_cpu_us: u128,
    pub(super) particle_projection_us: u128,
    pub(super) particle_projection_cpu_us: u128,
    pub(super) particle_preparation_wait_us: u128,
    pub(super) particle_prepared_frame_age_us: u128,
    pub(super) particle_lookahead_mismatch_count: u64,
    pub(super) particle_preparation_queue_depth: usize,
    pub(super) particle_worker_wake_latency_us: u128,
    pub(super) particle_clear_us: u128,
    pub(super) particle_clear_cpu_us: u128,
    pub(super) particle_raster_us: u128,
    pub(super) particle_raster_cpu_us: u128,
    pub(super) particle_render_cpu_start: u64,
    pub(super) particle_render_cpu_end: u64,
    pub(super) particle_voluntary_context_switches: u64,
    pub(super) particle_involuntary_context_switches: u64,
    pub(super) particle_pmu_available: bool,
    pub(super) particle_pmu_cycles: u64,
    pub(super) particle_pmu_instructions: u64,
    pub(super) particle_pmu_cache_references: u64,
    pub(super) particle_pmu_cache_misses: u64,
    pub(super) particle_pmu_branch_instructions: u64,
    pub(super) particle_pmu_branch_misses: u64,
    pub(super) particle_rotation_y_millidegrees: u32,
    pub(super) particle_simulation_bytes: usize,
    pub(super) particle_renderer_scratch_bytes: usize,
}

pub(crate) fn shared_parade_trace(stats: ScreenshotParadeStats) -> ScreensaverFrameTrace {
    ScreensaverFrameTrace {
        card_adopt_us: stats.card_adopt_us,
        cards_adopted: stats.cards_adopted,
        parade_advance_us: stats.parade_advance_us,
        background_us: stats.background_us,
        draw_order_us: stats.draw_order_us,
        tile_blit_us: stats.tile_blit_us,
        cards_drawn: stats.cards_drawn,
        cards_culled: stats.cards_culled,
        raster_held_cards: stats.raster_held_cards,
        raster_moved_cards: stats.raster_moved_cards,
        raster_hold_layer_mask: stats.raster_hold_layer_mask,
        raster_visible_layer_mask: stats.raster_visible_layer_mask,
        phase_bank_resident_bytes: stats.phase_bank_resident_bytes,
        ..ScreensaverFrameTrace::default()
    }
}

fn log_shared_parade_stats(parade: &ScreenshotParade) {
    let stats = parade.stats();
    let scale_average_us = stats.scale_total_us / u128::from(stats.scale_count.max(1));
    let phase_average_us = stats.phase_total_us / u128::from(stats.phase_count.max(1));
    crate::ui_logln!(
        "screensaver_lanczos scales={} total_us={} average_us={} max_us={} phase_prepares={} phase_total_us={} phase_average_us={} phase_max_us={} queue_max={} queue_bound={} worker_connected=true phase_cache_bytes={}",
        stats.scale_count,
        stats.scale_total_us,
        scale_average_us,
        stats.scale_max_us,
        stats.phase_count,
        stats.phase_total_us,
        phase_average_us,
        stats.phase_max_us,
        stats.queue_max,
        parade.queue_bound(),
        stats.phase_bank_resident_bytes + stats.image_cache_resident_bytes
    );
    crate::ui_logln!(
        "screensaver_archive_runtime entries={} decodes={} failures={} unique_keys={} queue_depth={} queue_max={}",
        parade.asset_count(),
        stats.decode_successes,
        stats.decode_failures,
        stats.unique_decoded,
        stats.queue_depth,
        stats.queue_max
    );
}

fn slint_rgb565_as_shared_mut(destination: &mut [Rgb565Pixel]) -> &mut [SharedRgb565Pixel] {
    // SAFETY: both RGB565 pixel types are transparent `u16` wrappers with equal
    // size/alignment, and the mutable slice retains the input slice's lifetime.
    unsafe {
        std::slice::from_raw_parts_mut(
            destination.as_mut_ptr().cast::<SharedRgb565Pixel>(),
            destination.len(),
        )
    }
}

const _: () = {
    assert!(std::mem::size_of::<Rgb565Pixel>() == std::mem::size_of::<SharedRgb565Pixel>());
    assert!(std::mem::align_of::<Rgb565Pixel>() == std::mem::align_of::<SharedRgb565Pixel>());
};

fn shared_rgb565_into_slint(mut pixels: Vec<SharedRgb565Pixel>) -> Vec<Rgb565Pixel> {
    let length = pixels.len();
    let capacity = pixels.capacity();
    let pointer = pixels.as_mut_ptr().cast::<Rgb565Pixel>();
    std::mem::forget(pixels);
    // SAFETY: the compile-time assertions above establish identical layout.
    unsafe { Vec::from_raw_parts(pointer, length, capacity) }
}

fn slint_rgb565_into_shared(mut pixels: Vec<Rgb565Pixel>) -> Vec<SharedRgb565Pixel> {
    let length = pixels.len();
    let capacity = pixels.capacity();
    let pointer = pixels.as_mut_ptr().cast::<SharedRgb565Pixel>();
    std::mem::forget(pixels);
    // SAFETY: the compile-time assertions above establish identical layout.
    unsafe { Vec::from_raw_parts(pointer, length, capacity) }
}

pub(crate) struct LauncherScreenshotBuffer {
    pixels: Vec<SharedRgb565Pixel>,
}

impl LauncherScreenshotBuffer {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        Self {
            pixels: vec![SharedRgb565Pixel(0); width.saturating_mul(height)],
        }
    }

    pub(crate) fn into_pixels(self) -> Vec<Rgb565Pixel> {
        shared_rgb565_into_slint(self.pixels)
    }

    pub(crate) fn from_pixels(pixels: Vec<Rgb565Pixel>) -> Self {
        Self {
            pixels: slint_rgb565_into_shared(pixels),
        }
    }
}

impl ScreenshotBuffer for LauncherScreenshotBuffer {
    fn pixels_mut(&mut self) -> &mut [SharedRgb565Pixel] {
        &mut self.pixels
    }
}

pub(crate) type LauncherScreenshotRuntime = LiveScreenshotParade<LauncherScreenshotBuffer>;

impl LauncherScreensaver {
    fn particle(renderer: ParticleRenderer, particle_reload: Option<MagikRecipeReload>) -> Self {
        let now = Instant::now();
        Self {
            parade: None,
            particle: Some(renderer),
            particle_reload,
            startup_started_at: None,
            frame: 0,
            motion_started_at: now,
        }
    }

    pub fn render(&mut self, dst: &mut [Rgb565Pixel], w: usize, h: usize) -> ScreensaverFrameTrace {
        let now = Instant::now();
        self.render_at(
            dst,
            w,
            h,
            now.saturating_duration_since(self.motion_started_at),
        )
    }

    pub fn render_at(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        elapsed: Duration,
    ) -> ScreensaverFrameTrace {
        self.render_at_target(dst, w, h, None, elapsed)
    }

    pub fn render_at_presentation_tick(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        presentation_tick: u64,
        fallback_elapsed: Duration,
    ) -> ScreensaverFrameTrace {
        self.render_at_target_with_lookahead(
            dst,
            w,
            h,
            None,
            fallback_elapsed,
            None,
            Some(presentation_tick),
        )
    }

    pub fn render_at_hidden_slot(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        hidden_slot: u8,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
    ) -> ScreensaverFrameTrace {
        self.render_at_target_with_lookahead(
            dst,
            w,
            h,
            Some(hidden_slot),
            elapsed,
            next_elapsed,
            None,
        )
    }

    pub fn render_at_hidden_slot_presentation_tick(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        hidden_slot: u8,
        presentation_tick: u64,
        fallback_elapsed: Duration,
        next_elapsed: Option<Duration>,
    ) -> ScreensaverFrameTrace {
        self.render_at_target_with_lookahead(
            dst,
            w,
            h,
            Some(hidden_slot),
            fallback_elapsed,
            next_elapsed,
            Some(presentation_tick),
        )
    }

    fn render_at_target(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        hidden_slot: Option<u8>,
        elapsed: Duration,
    ) -> ScreensaverFrameTrace {
        self.render_at_target_with_lookahead(dst, w, h, hidden_slot, elapsed, None, None)
    }

    fn render_at_target_with_lookahead(
        &mut self,
        dst: &mut [Rgb565Pixel],
        _w: usize,
        _h: usize,
        hidden_slot: Option<u8>,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
        presentation_tick: Option<u64>,
    ) -> ScreensaverFrameTrace {
        if let Some(particle) = self.particle.as_mut() {
            let particle_elapsed = if let Some(reload) = self.particle_reload.as_mut() {
                reload.apply_latest(particle, elapsed);
                reload.logical_elapsed(elapsed)
            } else {
                elapsed
            };
            return match hidden_slot
                .ok_or_else(|| "particle renderer requires a direct hidden slot".into())
                .and_then(|hidden_slot| {
                    let next_elapsed = next_elapsed
                        .map(|next| next.saturating_sub(elapsed.saturating_sub(particle_elapsed)));
                    particle.render_with_lookahead(dst, hidden_slot, particle_elapsed, next_elapsed)
                }) {
                Ok(stats) => ScreensaverFrameTrace {
                    renderer: PARTICLE_RENDERER_LABEL,
                    particle_preset: particle.preset().label(),
                    particle_phase: stats.phase.label(),
                    particle_simulation_backend: stats.simulation_backend,
                    particle_projection_backend: stats.projection_backend,
                    particle_count: stats.count,
                    particle_visible: stats.visible,
                    particle_simulation_us: stats.simulation_us,
                    particle_simulation_cpu_us: stats.simulation_cpu_us,
                    particle_projection_us: stats.projection_us,
                    particle_projection_cpu_us: stats.projection_cpu_us,
                    particle_preparation_wait_us: stats.preparation_wait_us,
                    particle_prepared_frame_age_us: stats.prepared_frame_age_us,
                    particle_lookahead_mismatch_count: stats.lookahead_mismatch_count,
                    particle_preparation_queue_depth: stats.preparation_queue_depth,
                    particle_worker_wake_latency_us: stats.worker_wake_latency_us,
                    particle_clear_us: stats.clear_us,
                    particle_clear_cpu_us: stats.clear_cpu_us,
                    particle_raster_us: stats.raster_us,
                    particle_raster_cpu_us: stats.raster_cpu_us,
                    particle_render_cpu_start: stats.render_cpu_start,
                    particle_render_cpu_end: stats.render_cpu_end,
                    particle_voluntary_context_switches: stats.voluntary_context_switches,
                    particle_involuntary_context_switches: stats.involuntary_context_switches,
                    particle_pmu_available: stats.pmu_available,
                    particle_pmu_cycles: stats.pmu_cycles,
                    particle_pmu_instructions: stats.pmu_instructions,
                    particle_pmu_cache_references: stats.pmu_cache_references,
                    particle_pmu_cache_misses: stats.pmu_cache_misses,
                    particle_pmu_branch_instructions: stats.pmu_branch_instructions,
                    particle_pmu_branch_misses: stats.pmu_branch_misses,
                    particle_rotation_y_millidegrees: stats.rotation_y_millidegrees,
                    particle_simulation_bytes: stats.simulation_bytes,
                    particle_renderer_scratch_bytes: stats.renderer_scratch_bytes,
                    ..ScreensaverFrameTrace::default()
                },
                Err(error) => {
                    dst.fill(Rgb565Pixel(0));
                    crate::ui_errln!("particle renderer failed: {error}");
                    ScreensaverFrameTrace {
                        renderer: "particle-error",
                        particle_preset: particle.preset().label(),
                        particle_count: particle.particle_count(),
                        ..ScreensaverFrameTrace::default()
                    }
                }
            };
        }
        let mut trace = if let Some(parade) = self.parade.as_mut() {
            let render_result = match presentation_tick {
                Some(tick) => {
                    parade.render_at_presentation_tick(slint_rgb565_as_shared_mut(dst), tick)
                }
                None => parade.render_at(slint_rgb565_as_shared_mut(dst), elapsed),
            };
            match render_result {
                Ok(stats) => {
                    if parade.is_ready() {
                        if let Some(started) = self.startup_started_at.take() {
                            crate::ui_logln!(
                                "screensaver_startup_timing milestone=first_card_ready elapsed_us={} layer={}",
                                started.elapsed().as_micros(),
                                parade.first_ready_layer().unwrap_or_default()
                            );
                        }
                    }
                    shared_parade_trace(stats)
                }
                Err(error) => {
                    dst.fill(Rgb565Pixel(0));
                    crate::ui_errln!("screenshot parade render failed: {error}");
                    ScreensaverFrameTrace::default()
                }
            }
        } else {
            dst.fill(Rgb565Pixel(0));
            ScreensaverFrameTrace::default()
        };
        trace.renderer = "parade";
        if self.frame > 0 && self.frame % 600 == 0 {
            if let Some(parade) = self.parade.as_ref() {
                log_shared_parade_stats(parade);
            }
        }
        self.frame = self.frame.wrapping_add(1);
        trace
    }

    pub fn invalidate_hidden_slot(&mut self, hidden_slot: u8) {
        if let Some(particle) = self.particle.as_mut() {
            particle.invalidate_hidden_slot(hidden_slot);
        }
    }

    pub fn has_rendered_card(&self) -> bool {
        if self.particle.is_some() {
            return true;
        }
        self.parade.as_ref().is_some_and(ScreenshotParade::is_ready)
    }

    pub fn is_loading_archive(&self) -> bool {
        false
    }

    pub fn active_card_count(&self) -> usize {
        if self.particle.is_some() {
            return 0;
        }
        self.parade
            .as_ref()
            .map_or(0, ScreenshotParade::active_card_count)
    }

    pub fn has_pending_card_work(&self) -> bool {
        self.parade
            .as_ref()
            .is_some_and(ScreenshotParade::has_pending_work)
    }

    pub fn preparation_slack(
        &self,
    ) -> Option<Arc<mister_magik_screenshot_parade::PreparationSlack>> {
        self.parade
            .as_ref()
            .and_then(ScreenshotParade::preparation_slack)
    }

    pub fn requires_direct_hidden(&self) -> bool {
        self.particle.is_some()
    }
}

impl LauncherScreensaver {
    pub fn from_archive_path(
        path: &std::path::Path,
        width: usize,
        height: usize,
        seed: u64,
    ) -> Result<Self, String> {
        let archive = preview_worker::ResidentPreviewArchive::open(path)?;
        let geometry =
            SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
        let parade = ScreenshotParade::new(
            archive,
            ScreenshotParadeConfig {
                geometry,
                seed,
                worker_start: None,
                preparation_slack: None,
            },
        )?;
        let now = Instant::now();
        Ok(Self {
            parade: Some(parade),
            particle: None,
            particle_reload: None,
            startup_started_at: None,
            frame: 0,
            motion_started_at: now,
        })
    }
}

pub struct LauncherScreensaverLoader {
    ready_rx: Receiver<LauncherScreenshotRuntime>,
    cancelled: Arc<AtomicBool>,
}

impl LauncherScreensaverLoader {
    pub fn start(w: usize, h: usize, startup_started_at: Option<Instant>) -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        std::thread::Builder::new()
            .name("screensaver-load".into())
            .spawn(move || {
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverLoader,
                );
                let started = Instant::now();
                let path = screensaver_archive_path(
                    std::env::var_os("MISTER_MEDIA_ASSET_DIR").as_deref(),
                    DeviceLayout::current(),
                );
                let result: Result<Option<LauncherScreenshotRuntime>, String> = (|| {
                    let archive = preview_worker::ResidentPreviewArchive::open(&path)
                        .map_err(|error| format!("path={} error={error}", path.display()))?;
                    if worker_cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    let open_us = started.elapsed().as_micros();
                    crate::ui_logln!(
                        "screensaver_loader path={} pack_bytes={} entries={}",
                        path.display(),
                        archive.compressed_bytes(),
                        archive.asset_keys().len()
                    );
                    let construction_started = Instant::now();
                    let seed = random_seed();
                    let buffers = std::array::from_fn(|_| LauncherScreenshotBuffer::new(w, h));
                    let mut runtime = LiveScreenshotParade::start(
                        archive,
                        LiveScreenshotConfig {
                            geometry: SceneGeometry::new(w, h, w)
                                .map_err(|error| error.to_string())?,
                            seed,
                            scale_worker_start: Some(Arc::new(|| {
                                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverScaler,
                                );
                            })),
                            render_worker_start: Some(Arc::new(|| {
                                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverRenderer,
                                );
                            })),
                        },
                        buffers,
                    )?;
                    runtime.wait_until_prefilled(Duration::from_secs(30))?;
                    runtime.finish_prefill()?;
                    let construct_us = construction_started.elapsed().as_micros();
                    crate::ui_logln!(
                        "screensaver_loader_timing archive_open_us={} runtime_prefill_us={} total_us={} cards=real",
                        open_us,
                        construct_us,
                        open_us.saturating_add(construct_us)
                    );
                    if let Some(started) = startup_started_at {
                        crate::ui_logln!(
                            "screensaver_startup_timing milestone=two_real_frames_ready elapsed_us={}",
                            started.elapsed().as_micros()
                        );
                    }
                    Ok(Some(runtime))
                })();
                match result {
                    Ok(Some(saver)) if !worker_cancelled.load(Ordering::Relaxed) => {
                        let _ = ready_tx.send(saver);
                    }
                    Ok(_) => {}
                    Err(error) => crate::ui_errln!("screensaver_loader error={error}"),
                }
            })
            .expect("spawn screensaver loader");
        Self {
            ready_rx,
            cancelled,
        }
    }

    pub(crate) fn try_ready(&self) -> Option<LauncherScreenshotRuntime> {
        self.ready_rx.try_recv().ok()
    }
}

impl Drop for LauncherScreensaverLoader {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

fn particle_renderer_label_requested(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case(PARTICLE_RENDERER_LABEL))
}

fn particle_config_from_env(width: usize, height: usize) -> Result<ParticleConfig, String> {
    if (width, height) != (960, 540) {
        return Err(format!(
            "particle experiment requires 960x540, received {width}x{height}"
        ));
    }
    let embedded = embedded_magik_recipe()
        .map_err(|error| format!("embedded Magik particle recipe is invalid: {error}"))?;
    let count = std::env::var("MISTER_PARTICLE_COUNT")
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|error| format!("invalid MISTER_PARTICLE_COUNT={value:?}: {error}"))
        })
        .transpose()?
        .unwrap_or(embedded.particle_count);
    let preset = std::env::var("MISTER_PARTICLE_PRESET")
        .ok()
        .map(|value| {
            ParticlePreset::parse(&value)
                .ok_or_else(|| format!("invalid MISTER_PARTICLE_PRESET={value:?}"))
        })
        .transpose()?
        .unwrap_or(ParticlePreset::Visual);
    let seed = std::env::var("MISTER_PARTICLE_SEED")
        .ok()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("invalid MISTER_PARTICLE_SEED={value:?}: {error}"))
        })
        .transpose()?
        .unwrap_or(embedded.seed);
    ParticleConfig {
        count,
        width,
        height,
        seed,
        preset,
    }
    .validate()
}

fn screensaver_archive_path(asset_dir: Option<&OsStr>, layout: DeviceLayout) -> PathBuf {
    asset_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| layout.app_path("assets"))
        .join("arcade-screenshots-320x320.mmlz4b")
}

fn random_seed() -> u64 {
    if let Some(seed) = std::env::var("MISTER_SCREENSAVER_SEED")
        .ok()
        .as_deref()
        .and_then(parse_screensaver_seed)
    {
        return seed;
    }
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    time ^ (std::process::id() as u64).rotate_left(17) ^ 0x9e37_79b9_7f4a_7c15
}

fn parse_screensaver_seed(value: &str) -> Option<u64> {
    let value = value.trim();
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map(|digits| u64::from_str_radix(digits, 16).ok())
        .unwrap_or_else(|| value.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screensaver_archive_path_uses_public_layout_by_default() {
        assert_eq!(
            screensaver_archive_path(None, DeviceLayout::Public),
            PathBuf::from("/media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b")
        );
    }

    #[test]
    fn screensaver_archive_path_uses_development_layout_by_default() {
        assert_eq!(
            screensaver_archive_path(None, DeviceLayout::Dev),
            PathBuf::from("/media/fat/mister-magik-dev/assets/arcade-screenshots-320x320.mmlz4b")
        );
    }

    #[test]
    fn screensaver_archive_path_honors_explicit_asset_directory() {
        assert_eq!(
            screensaver_archive_path(
                Some(OsStr::new("/tmp/screensaver-assets")),
                DeviceLayout::Dev
            ),
            PathBuf::from("/tmp/screensaver-assets/arcade-screenshots-320x320.mmlz4b")
        );
    }

    #[test]
    fn public_archive_constructor_runs_the_production_parade() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-production-screensaver-{}.mmlz4b",
            std::process::id()
        ));
        write_single_image_archive(&path);
        let mut screensaver = LauncherScreensaver::from_archive_path(&path, 320, 180, 0x1234)
            .expect("open production screensaver");
        let mut frame = vec![Rgb565Pixel(0); 320 * 180];
        let trace = screensaver.render_at(&mut frame, 320, 180, Duration::from_secs(2));

        assert_eq!(trace.renderer, "parade");
        assert!(!screensaver.is_loading_archive());
        let _ = std::fs::remove_file(path);
    }

    fn write_single_image_archive(path: &std::path::Path) {
        let name = b"fixture.rgb565";
        let width = 2_u32;
        let height = 2_u32;
        let stride_bytes = 4_u32;
        let pixels = [0x00_u8, 0xf8, 0xe0, 0x07, 0x1f, 0x00, 0xff, 0xff];
        let index_len = 8 + 4 + 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MMPX2B1\0");
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&stride_bytes.to_le_bytes());
        bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(index_len as u64).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(&pixels);
        std::fs::write(path, bytes).expect("write production screensaver archive fixture");
    }

    #[test]
    fn benchmark_seed_accepts_decimal_and_hex_without_fallback_guessing() {
        assert_eq!(parse_screensaver_seed("42"), Some(42));
        assert_eq!(parse_screensaver_seed(" 0x2a "), Some(42));
        assert_eq!(parse_screensaver_seed("0X2A"), Some(42));
        assert_eq!(parse_screensaver_seed(""), None);
        assert_eq!(parse_screensaver_seed("seed"), None);
    }

    #[test]
    fn particle_renderer_is_selected_only_by_its_explicit_label() {
        assert!(particle_renderer_label_requested(Some("particle-magik")));
        assert!(particle_renderer_label_requested(Some(" PARTICLE-MAGIK ")));
        assert!(!particle_renderer_label_requested(None));
        assert!(!particle_renderer_label_requested(Some("")));
        assert!(!particle_renderer_label_requested(Some("parade")));
    }

    #[test]
    fn public_layout_never_constructs_the_mutable_recipe_watcher() {
        assert!(
            MagikRecipeReload::for_layout(DeviceLayout::Public, 960, 540, ParticlePreset::Visual)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn particle_renderer_requires_the_direct_hidden_pipeline() {
        let renderer = ParticleRenderer::new_magik(ParticleConfig {
            count: 1_024,
            width: 960,
            height: 540,
            seed: 7,
            preset: ParticlePreset::Capacity,
        })
        .unwrap();
        let screensaver = LauncherScreensaver::particle(renderer, None);
        assert!(screensaver.requires_direct_hidden());
        assert!(!screensaver.is_loading_archive());
        assert!(screensaver.has_rendered_card());
        assert_eq!(screensaver.active_card_count(), 0);
    }
}
