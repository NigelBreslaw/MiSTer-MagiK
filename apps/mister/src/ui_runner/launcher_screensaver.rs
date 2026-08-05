// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#![cfg_attr(target_os = "macos", allow(dead_code))]

#[cfg(not(target_os = "macos"))]
use super::*;
#[cfg(target_os = "macos")]
use crate::framebuffer::target::{DirtyRect, blend_565, brighten_565};
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
    ScreenshotParade, ScreenshotParadeConfig, ScreenshotParadeStartup, ScreenshotParadeStats,
    ScreenshotSamplingProfile,
};
#[cfg(target_os = "macos")]
use slint::platform::software_renderer::{Rgb565Pixel, TargetPixel};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use super::particle_renderer::ParticleRenderer;
use mister_magik_fb::visual_composition::{ScreenshotTileImage as SaverImage, ScreenshotTileWall};

const PARTICLE_RENDERER_LABEL: &str = "particle-magik";
const DEV_MAGIK_RECIPE_PATH: &str = "/tmp/mister-magik/startup-particles/magik.json";
const DEV_MAGIK_STATUS_PATH: &str = "/tmp/mister-magik/startup-particles/status.json";

fn hash2_u8(x: usize, y: usize) -> u8 {
    let mut v = (x as u32).wrapping_mul(0x45d9f3b) ^ (y as u32).wrapping_mul(0x119de1f3);
    v ^= v >> 16;
    v = v.wrapping_mul(0x45d9f3b);
    (v >> 24) as u8
}

fn triangle_wave_u8(x: usize, phase: u8) -> u8 {
    let v = ((x as u32).wrapping_mul(13).wrapping_add(phase as u32)) & 0xff;
    let v = if v < 128 { v } else { 255 - v };
    (v * 2).min(255) as u8
}

fn plasma_gate(x: usize, y: usize, phase: u8) -> u8 {
    ((triangle_wave_u8(x / 3 + y / 7, phase) as u16
        + triangle_wave_u8(x / 9 + y / 2, phase.wrapping_mul(3)) as u16)
        / 2) as u8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScreensaverMode {
    AttractWall,
    MvsCarousel,
    SuperScalerFlyby,
    StarfieldCabinets,
    ScreenshotRain,
    TilemapMuseum,
    RasterGallery,
    KefrensScreenshotBars,
    PreviewPlasmaCollage,
    PhosphorGrid,
    WarpTunnel,
    Mode7Floor,
    ScannerContactSheet,
    SpriteMultiplexParade,
    CabinetMarquee,
    RandomAccessLoader,
    ColorClashGallery,
    RadialStarfield,
    PixelGrid,
    IdleMegademo,
}

impl ScreensaverMode {
    const ALL: [Self; 20] = [
        Self::AttractWall,
        Self::MvsCarousel,
        Self::SuperScalerFlyby,
        Self::StarfieldCabinets,
        Self::ScreenshotRain,
        Self::TilemapMuseum,
        Self::RasterGallery,
        Self::KefrensScreenshotBars,
        Self::PreviewPlasmaCollage,
        Self::PhosphorGrid,
        Self::WarpTunnel,
        Self::Mode7Floor,
        Self::ScannerContactSheet,
        Self::SpriteMultiplexParade,
        Self::CabinetMarquee,
        Self::RandomAccessLoader,
        Self::ColorClashGallery,
        Self::RadialStarfield,
        Self::PixelGrid,
        Self::IdleMegademo,
    ];

    const MEGA: [Self; 19] = [
        Self::AttractWall,
        Self::MvsCarousel,
        Self::SuperScalerFlyby,
        Self::StarfieldCabinets,
        Self::ScreenshotRain,
        Self::TilemapMuseum,
        Self::RasterGallery,
        Self::KefrensScreenshotBars,
        Self::PreviewPlasmaCollage,
        Self::PhosphorGrid,
        Self::WarpTunnel,
        Self::Mode7Floor,
        Self::ScannerContactSheet,
        Self::SpriteMultiplexParade,
        Self::CabinetMarquee,
        Self::RandomAccessLoader,
        Self::ColorClashGallery,
        Self::RadialStarfield,
        Self::IdleMegademo,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::AttractWall => "attract-wall",
            Self::MvsCarousel => "mvs-carousel",
            Self::SuperScalerFlyby => "super-scaler-flyby",
            Self::StarfieldCabinets => "starfield-cabinets",
            Self::ScreenshotRain => "screenshot-rain",
            Self::TilemapMuseum => "tilemap-museum",
            Self::RasterGallery => "raster-gallery",
            Self::KefrensScreenshotBars => "kefrens-screenshot-bars",
            Self::PreviewPlasmaCollage => "preview-plasma-collage",
            Self::PhosphorGrid => "phosphor-grid",
            Self::WarpTunnel => "warp-tunnel",
            Self::Mode7Floor => "mode7-floor",
            Self::ScannerContactSheet => "scanner-contact-sheet",
            Self::SpriteMultiplexParade => "sprite-multiplex-parade",
            Self::CabinetMarquee => "cabinet-marquee",
            Self::RandomAccessLoader => "random-access-loader",
            Self::ColorClashGallery => "color-clash-gallery",
            Self::RadialStarfield => "radial-starfield",
            Self::PixelGrid => "pixel-grid",
            Self::IdleMegademo => "idle-megademo",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        let value = value.to_ascii_lowercase().replace('_', "-");
        Self::ALL.iter().copied().find(|mode| mode.label() == value)
    }
}

struct ScreensaverConfig {
    modes: Vec<ScreensaverMode>,
    segment: Duration,
    cache_cap: usize,
    trace: Option<File>,
}

impl ScreensaverConfig {
    fn from_env() -> Self {
        let spec = std::env::var("MISTER_SCREENSAVER").unwrap_or_else(|_| "mega".into());
        let modes = if matches!(
            spec.trim().to_ascii_lowercase().as_str(),
            "" | "mega" | "all" | "demo"
        ) {
            ScreensaverMode::MEGA.to_vec()
        } else {
            let mut modes = Vec::new();
            for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(mode) = ScreensaverMode::parse(part) {
                    modes.push(mode);
                } else {
                    crate::ui_errln!("screensaver: unknown mode {part:?}");
                }
            }
            if modes.is_empty() {
                vec![ScreensaverMode::AttractWall]
            } else {
                modes
            }
        };
        let segment_secs = std::env::var("MISTER_SCREENSAVER_SEGMENT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(20)
            .max(1);
        let cache_cap = std::env::var("MISTER_SCREENSAVER_CACHE_CAP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(256)
            .clamp(1, 512);
        let trace = std::env::var("MISTER_SCREENSAVER_TRACE")
            .ok()
            .and_then(|path| {
                let mut f = File::create(&path)
                    .map_err(|e| crate::ui_errln!("screensaver trace: create {path} failed: {e}"))
                    .ok()?;
                f.write_all(b"frame\telapsed_us\tmode\timage_count\tdraw_us\tvsync_us\tfb_present_us\twall_us\tvsync_source\tvsync_period_us\tvsync_miss_streak\n")
                    .ok()?;
                crate::ui_logln!("screensaver_trace={path}");
                Some(f)
            });
        Self {
            modes,
            segment: Duration::from_secs(segment_secs),
            cache_cap,
            trace,
        }
    }

    fn mode_at(&self, elapsed: Duration) -> ScreensaverMode {
        let idx = ((elapsed.as_micros() / self.segment.as_micros().max(1)) as usize)
            % self.modes.len().max(1);
        self.modes
            .get(idx)
            .copied()
            .unwrap_or(ScreensaverMode::AttractWall)
    }
}

struct ScreensaverRenderState {
    parade: ParadeState,
    phosphor_grid: Vec<Rgb565Pixel>,
    phosphor_grid_page: usize,
    phosphor_grid_valid: bool,
    random_loader: Vec<Rgb565Pixel>,
    random_loader_page: usize,
    random_loader_valid: bool,
    tilemap_normal: Vec<Rgb565Pixel>,
    tilemap_bright: Vec<Rgb565Pixel>,
    tilemap_page: usize,
    tilemap_valid: bool,
    attract_wall: ScreenshotTileWall,
    color_clash_contact: Vec<Rgb565Pixel>,
    color_clash_contact_start: usize,
    color_clash_contact_valid: bool,
    scanner_contact: Vec<Rgb565Pixel>,
    scanner_contact_start: usize,
    scanner_contact_valid: bool,
    starfield_contact: Vec<Rgb565Pixel>,
    starfield_contact_start: usize,
    starfield_contact_valid: bool,
}

pub struct LauncherScreensaver {
    parade: Option<ScreenshotParade>,
    parade_seed: u64,
    parade_sampling_profile: ParadeSamplingProfile,
    particle: Option<ParticleRenderer>,
    particle_reload: Option<MagikRecipeReload>,
    archive_rx: Option<Receiver<ArchiveLoadResult>>,
    archive_cancelled: Arc<AtomicBool>,
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
    pub(super) sampling_profile: &'static str,
    pub(super) raster_held_cards: usize,
    pub(super) raster_moved_cards: usize,
    pub(super) raster_hold_layer_mask: u8,
    pub(super) raster_visible_layer_mask: u8,
    pub(super) sixteenth_phase_layer_mask: u8,
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

fn shared_sampling_profile(profile: ParadeSamplingProfile) -> ScreenshotSamplingProfile {
    match profile {
        ParadeSamplingProfile::LegacyHalf => ScreenshotSamplingProfile::HdmiLegacyHalf,
        ParadeSamplingProfile::CrtSixteenth => ScreenshotSamplingProfile::CrtSixteenth,
    }
}

fn shared_parade_trace(
    stats: ScreenshotParadeStats,
    profile: ParadeSamplingProfile,
) -> ScreensaverFrameTrace {
    ScreensaverFrameTrace {
        card_adopt_us: stats.card_adopt_us,
        cards_adopted: stats.cards_adopted,
        parade_advance_us: stats.parade_advance_us,
        background_us: stats.background_us,
        draw_order_us: stats.draw_order_us,
        tile_blit_us: stats.tile_blit_us,
        cards_drawn: stats.cards_drawn,
        cards_culled: stats.cards_culled,
        sampling_profile: profile.layer_evidence(),
        raster_held_cards: stats.raster_held_cards,
        raster_moved_cards: stats.raster_moved_cards,
        raster_hold_layer_mask: stats.raster_hold_layer_mask,
        raster_visible_layer_mask: stats.raster_visible_layer_mask,
        sixteenth_phase_layer_mask: stats.sixteenth_phase_layer_mask,
        phase_bank_resident_bytes: stats.phase_bank_resident_bytes,
        ..ScreensaverFrameTrace::default()
    }
}

fn log_shared_parade_stats(parade: &ScreenshotParade, profile: ParadeSamplingProfile) {
    let stats = parade.stats();
    let scale_average_us = stats.scale_total_us / u128::from(stats.scale_count.max(1));
    let phase_average_us = stats.phase_total_us / u128::from(stats.phase_count.max(1));
    crate::ui_logln!(
        "screensaver_lanczos sampling={} scales={} total_us={} average_us={} max_us={} phase_prepares={} phase_total_us={} phase_average_us={} phase_max_us={} queue_max={} queue_bound={} worker_connected=true phase_cache_bytes={}",
        profile.label(),
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
    assert_eq!(
        std::mem::size_of::<Rgb565Pixel>(),
        std::mem::size_of::<SharedRgb565Pixel>()
    );
    assert_eq!(
        std::mem::align_of::<Rgb565Pixel>(),
        std::mem::align_of::<SharedRgb565Pixel>()
    );
    // SAFETY: both RGB565 pixel types are transparent `u16` wrappers with equal
    // size/alignment, and the mutable slice retains the input slice's lifetime.
    unsafe {
        std::slice::from_raw_parts_mut(
            destination.as_mut_ptr().cast::<SharedRgb565Pixel>(),
            destination.len(),
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ParadeAdvanceTrace {
    card_adopt_us: u128,
    cards_adopted: usize,
    parade_advance_us: u128,
}

impl LauncherScreensaver {
    fn loading(
        archive_rx: Receiver<ArchiveLoadResult>,
        archive_cancelled: Arc<AtomicBool>,
        startup_started_at: Option<Instant>,
        sampling_profile: ParadeSamplingProfile,
    ) -> Self {
        let now = Instant::now();
        let parade_seed = random_seed();
        Self {
            parade: None,
            parade_seed,
            parade_sampling_profile: sampling_profile,
            particle: None,
            particle_reload: None,
            archive_rx: Some(archive_rx),
            archive_cancelled,
            startup_started_at,
            frame: 0,
            motion_started_at: now,
        }
    }

    fn particle(
        renderer: ParticleRenderer,
        particle_reload: Option<MagikRecipeReload>,
        archive_cancelled: Arc<AtomicBool>,
    ) -> Self {
        let now = Instant::now();
        Self {
            parade: None,
            parade_seed: random_seed(),
            parade_sampling_profile: ParadeSamplingProfile::LegacyHalf,
            particle: Some(renderer),
            particle_reload,
            archive_rx: None,
            archive_cancelled,
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

    pub fn render_at_hidden_slot(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        hidden_slot: u8,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
    ) -> ScreensaverFrameTrace {
        self.render_at_target_with_lookahead(dst, w, h, Some(hidden_slot), elapsed, next_elapsed)
    }

    fn render_at_target(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        hidden_slot: Option<u8>,
        elapsed: Duration,
    ) -> ScreensaverFrameTrace {
        self.render_at_target_with_lookahead(dst, w, h, hidden_slot, elapsed, None)
    }

    fn render_at_target_with_lookahead(
        &mut self,
        dst: &mut [Rgb565Pixel],
        w: usize,
        h: usize,
        hidden_slot: Option<u8>,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
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
                    sampling_profile: "particle-scalar",
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
                        sampling_profile: "particle-error",
                        particle_preset: particle.preset().label(),
                        particle_count: particle.particle_count(),
                        ..ScreensaverFrameTrace::default()
                    }
                }
            };
        }
        let archive_poll_start = Instant::now();
        self.poll_archive(w, h);
        let archive_poll_us = archive_poll_start.elapsed().as_micros();
        let mut trace = if let Some(parade) = self.parade.as_mut() {
            match parade.render_at(slint_rgb565_as_shared_mut(dst), elapsed) {
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
                    shared_parade_trace(stats, self.parade_sampling_profile)
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
        trace.archive_poll_us = archive_poll_us;
        if self.frame > 0 && self.frame % 600 == 0 {
            if let Some(parade) = self.parade.as_ref() {
                log_shared_parade_stats(parade, self.parade_sampling_profile);
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

    fn poll_archive(&mut self, w: usize, h: usize) {
        let Some(rx) = self.archive_rx.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(loaded)) => {
                crate::ui_logln!(
                    "screensaver_loader path={} pack_bytes={} entries={}",
                    loaded.path.display(),
                    loaded.archive.compressed_bytes(),
                    loaded.asset_keys.len()
                );
                crate::ui_logln!(
                    "screensaver_loader_timing archive_open_us={} initial_cards_us=0 total_us={} cards=0",
                    loaded.open_us,
                    loaded.open_us
                );
                let geometry = match SceneGeometry::new(w, h, w) {
                    Ok(geometry) => geometry,
                    Err(error) => {
                        crate::ui_errln!("screensaver geometry failed: {error}");
                        self.archive_rx = None;
                        return;
                    }
                };
                let worker_start = Arc::new(|| {
                    mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                        mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverScaler,
                    );
                });
                match ScreenshotParade::new(
                    loaded.archive,
                    ScreenshotParadeConfig {
                        geometry,
                        seed: self.parade_seed,
                        sampling_profile: shared_sampling_profile(self.parade_sampling_profile),
                        startup: ScreenshotParadeStartup::Streaming,
                        worker_start: Some(worker_start),
                    },
                ) {
                    Ok(parade) => self.parade = Some(parade),
                    Err(error) => crate::ui_errln!("screensaver initialization failed: {error}"),
                }
                self.archive_rx = None;
            }
            Ok(Err(error)) => {
                crate::ui_errln!("screensaver_loader error={error}");
                self.archive_rx = None;
            }
            Err(TryRecvError::Disconnected) => {
                self.archive_rx = None;
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    pub fn has_rendered_card(&self) -> bool {
        if self.particle.is_some() {
            return true;
        }
        self.parade.as_ref().is_some_and(ScreenshotParade::is_ready)
    }

    pub fn is_loading_archive(&self) -> bool {
        if self.particle.is_some() {
            return false;
        }
        self.archive_rx.is_some()
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

    pub fn requires_direct_hidden(&self) -> bool {
        self.particle.is_some()
    }
}

impl Drop for LauncherScreensaver {
    fn drop(&mut self) {
        self.archive_cancelled.store(true, Ordering::Relaxed);
    }
}

impl LauncherScreensaver {
    pub fn from_archive_path(
        path: &std::path::Path,
        width: usize,
        height: usize,
        seed: u64,
        crt_output: bool,
    ) -> Result<Self, String> {
        let archive = preview_worker::ResidentPreviewArchive::open(path)?;
        let sampling_profile = ParadeSamplingProfile::for_crt_output(crt_output);
        let geometry =
            SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
        let parade = ScreenshotParade::new(
            archive,
            ScreenshotParadeConfig {
                geometry,
                seed,
                sampling_profile: shared_sampling_profile(sampling_profile),
                startup: ScreenshotParadeStartup::Streaming,
                worker_start: None,
            },
        )?;
        let now = Instant::now();
        Ok(Self {
            parade: Some(parade),
            parade_seed: seed,
            parade_sampling_profile: sampling_profile,
            particle: None,
            particle_reload: None,
            archive_rx: None,
            archive_cancelled: Arc::new(AtomicBool::new(false)),
            startup_started_at: None,
            frame: 0,
            motion_started_at: now,
        })
    }
}

struct LoadedScreensaverArchive {
    path: PathBuf,
    archive: preview_worker::ResidentPreviewArchive,
    asset_keys: Vec<String>,
    open_us: u128,
}

type ArchiveLoadResult = Result<LoadedScreensaverArchive, String>;

pub struct LauncherScreensaverLoader {
    ready_rx: Receiver<LauncherScreensaver>,
}

impl LauncherScreensaverLoader {
    pub fn start(
        w: usize,
        h: usize,
        startup_started_at: Option<Instant>,
        crt_output: bool,
    ) -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        if magik_particle_renderer_requested() {
            let archive_cancelled = Arc::new(AtomicBool::new(false));
            match particle_config_from_env(w, h).and_then(|config| {
                let renderer = ParticleRenderer::new_magik(config)?;
                let reload =
                    MagikRecipeReload::for_layout(DeviceLayout::current(), w, h, config.preset)?;
                Ok((renderer, reload))
            }) {
                Ok((renderer, reload)) => {
                    let _ = ready_tx.send(LauncherScreensaver::particle(
                        renderer,
                        reload,
                        archive_cancelled,
                    ));
                }
                Err(error) => {
                    crate::ui_errln!("particle renderer initialization failed: {error}");
                }
            }
            return Self { ready_rx };
        }
        let (archive_tx, archive_rx) = mpsc::sync_channel(1);
        let archive_cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&archive_cancelled);
        let saver = LauncherScreensaver::loading(
            archive_rx,
            archive_cancelled,
            startup_started_at,
            ParadeSamplingProfile::for_crt_output(crt_output),
        );
        let _ = ready_tx.send(saver);
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
                let result = match preview_worker::ResidentPreviewArchive::open(&path) {
                    Ok(archive) if !worker_cancelled.load(Ordering::Relaxed) => {
                        Ok(LoadedScreensaverArchive {
                            asset_keys: archive.asset_keys().to_vec(),
                            archive,
                            path,
                            open_us: started.elapsed().as_micros(),
                        })
                    }
                    Ok(_) => return,
                    Err(error) => Err(format!("path={} error={error}", path.display())),
                };
                if worker_cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let _ = archive_tx.send(result);
            })
            .expect("spawn screensaver loader");
        Self { ready_rx }
    }

    pub fn try_ready(&self) -> Option<LauncherScreensaver> {
        self.ready_rx.try_recv().ok()
    }
}

pub fn particle_renderer_requested() -> bool {
    let value = std::env::var("MISTER_SCREENSAVER_RENDERER").ok();
    particle_renderer_label_requested(value.as_deref())
}

fn particle_renderer_label_requested(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case(PARTICLE_RENDERER_LABEL))
}

fn magik_particle_renderer_requested() -> bool {
    particle_renderer_label_requested(std::env::var("MISTER_SCREENSAVER_RENDERER").ok().as_deref())
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

impl ScreensaverRenderState {
    fn new(w: usize, h: usize, sampling_profile: ParadeSamplingProfile) -> Self {
        Self {
            parade: ParadeState::new_with_profile(random_seed(), sampling_profile),
            phosphor_grid: vec![Rgb565Pixel(0); w * h],
            phosphor_grid_page: usize::MAX,
            phosphor_grid_valid: false,
            random_loader: vec![Rgb565Pixel(0); w * h],
            random_loader_page: usize::MAX,
            random_loader_valid: false,
            tilemap_normal: vec![Rgb565Pixel(0); w * h],
            tilemap_bright: vec![Rgb565Pixel(0); w * h],
            tilemap_page: usize::MAX,
            tilemap_valid: false,
            attract_wall: ScreenshotTileWall::new(w, h),
            color_clash_contact: vec![Rgb565Pixel(0); w * h],
            color_clash_contact_start: usize::MAX,
            color_clash_contact_valid: false,
            scanner_contact: vec![Rgb565Pixel(0); w * h],
            scanner_contact_start: usize::MAX,
            scanner_contact_valid: false,
            starfield_contact: vec![Rgb565Pixel(0); w * h],
            starfield_contact_start: usize::MAX,
            starfield_contact_valid: false,
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(in crate::ui_runner) fn run_screensaver_loop(
    secs: u64,
    ui: &UiDisplay,
    hardware: &mut Fpga,
    display_session: &mut LauncherDisplaySession,
) {
    let startup_started = Instant::now();
    let mut cfg = ScreensaverConfig::from_env();
    let images = load_screensaver_images(cfg.cache_cap);
    let image_load_us = startup_started.elapsed().as_micros();
    let portrait_images = images.iter().filter(|image| image.h > image.w).count();
    let landscape_images = images.iter().filter(|image| image.w > image.h).count();
    let square_images = images.len() - portrait_images - landscape_images;
    let exact_four_three = images
        .iter()
        .filter(|image| image.w * 3 == image.h * 4)
        .count();
    crate::ui_logln!(
        "screensaver modes={} segment_secs={} cache_cap={} images={} portrait={} landscape={} square={} exact_four_three={}",
        cfg.modes
            .iter()
            .map(|mode| mode.label())
            .collect::<Vec<_>>()
            .join(","),
        cfg.segment.as_secs(),
        cfg.cache_cap,
        images.len(),
        portrait_images,
        landscape_images,
        square_images,
        exact_four_three
    );

    let mut backbuffer = vec![Rgb565Pixel(0); ui.render_w() * ui.render_h()];
    let mut render_state = ScreensaverRenderState::new(
        ui.render_w(),
        ui.render_h(),
        ParadeSamplingProfile::for_crt_output(ui.output_route().is_crt()),
    );
    if cfg.modes.contains(&ScreensaverMode::SpriteMultiplexParade) {
        render_state
            .parade
            .ensure_initialized(&images, ui.render_w(), ui.render_h());
    }
    crate::ui_logln!(
        "screensaver_startup image_load_us={} initialize_us={} ready_us={}",
        image_load_us,
        startup_started.elapsed().as_micros() - image_load_us,
        startup_started.elapsed().as_micros()
    );
    let mut presenter = match FpgaVblankLatchHiddenPresenter::open(ui) {
        Ok(presenter) => presenter,
        Err(failure) => {
            crate::ui_errln!(
                "screensaver_latch_failure state={} stage={} reason={} detail={}",
                failure.state.code(),
                failure.stage.code(),
                failure.reason_code(),
                failure.detail.replace(['\t', '\n', '\r'], " ")
            );
            return;
        }
    };
    let full_damage = DirtyRectList::from_one(DirtyRect {
        x0: 0,
        y0: 0,
        x1: ui.render_w(),
        y1: ui.render_h(),
    });
    let mut pacer = ui
        .output_route()
        .nominal_period_us()
        .map(VsyncPacer::from_env_with_default_period)
        .unwrap_or_else(VsyncPacer::from_env);
    let start = Instant::now();
    let mut frame = 0_u64;
    loop {
        let frame_start = Instant::now();
        let elapsed = start.elapsed();
        if secs > 0 && elapsed >= Duration::from_secs(secs) {
            break;
        }
        let mode = cfg.mode_at(elapsed);
        let draw_start = Instant::now();
        render_screensaver_frame(
            &mut backbuffer,
            &mut render_state,
            ui.render_w(),
            ui.render_h(),
            &images,
            mode,
            frame,
        );
        let draw_us = draw_start.elapsed().as_micros() as u64;
        let present_start = Instant::now();
        let frame_plan = LauncherFramePlan::new(full_damage, None, None, None, None);
        let stats = match presenter.present_cached_full_frame(
            CachedFrameView::new(&backbuffer, ui.render_w(), ui.render_h()),
            frame_plan,
            hardware,
            display_session,
            |_hidden, _plan| Ok(()),
        ) {
            Ok(stats) => stats,
            Err(failure) => {
                crate::ui_errln!(
                    "screensaver_latch_failure state={} stage={} reason={} detail={}",
                    failure.state.code(),
                    failure.stage.code(),
                    failure.reason_code(),
                    failure.detail.replace(['\t', '\n', '\r'], " ")
                );
                break;
            }
        };
        if let Some(scale) = mister_magik_fb::framebuffer::stream::configured_latch_scale(true) {
            let committed = presenter.committed_frame_view(stats.buffer_index);
            let _ = mister_magik_fb::framebuffer::stream::publish_latch_snapshot(committed, scale);
        }
        let present_us = present_start.elapsed().as_micros() as u64;
        // The frame is posted before waiting: the FPGA consumes it on the next
        // vblank while the CPU prepares no writes to the committed slot.
        let vsync = pacer.wait();
        let wall_us = frame_start.elapsed().as_micros() as u64;
        if let Some(trace) = cfg.trace.as_mut() {
            let _ = writeln!(
                trace,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                frame,
                elapsed.as_micros(),
                mode.label(),
                images.len(),
                draw_us,
                vsync.wait_us,
                present_us,
                wall_us,
                vsync.source.label(),
                vsync.period_us,
                vsync.miss_streak
            );
        }
        frame = frame.wrapping_add(1);
    }
    render_state
        .parade
        .collect_scaled_cards_with_limit(Some(&images), usize::MAX);
    render_state.parade.log_scaler_stats();
}

fn load_screensaver_images(cap: usize) -> Vec<SaverImage> {
    load_screensaver_images_cancellable(cap, None)
}

fn load_screensaver_images_cancellable(
    cap: usize,
    cancelled: Option<&AtomicBool>,
) -> Vec<SaverImage> {
    let arcade_screenshot_pack = screensaver_archive_path(
        std::env::var_os("MISTER_MEDIA_ASSET_DIR").as_deref(),
        DeviceLayout::current(),
    );
    let mut asset_keys =
        match preview_worker::preview_archive_sidecar_entry_stems(&arcade_screenshot_pack) {
            Ok(Some(sidecar)) => sidecar.entries,
            Ok(None) => match preview_worker::preview_archive_index(&arcade_screenshot_pack) {
                Ok(index) => index.entries,
                Err(error) => {
                    crate::ui_errln!("screensaver: arcade screenshot pack index failed: {error}");
                    Vec::new()
                }
            },
            Err(error) => {
                crate::ui_errln!("screensaver: arcade screenshot sidecar failed: {error}");
                Vec::new()
            }
        };
    let mut rng = random_seed();
    shuffle(&mut asset_keys, &mut rng);
    let mut images = Vec::new();
    for asset_key in asset_keys {
        if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
            break;
        }
        if images.len() >= cap {
            break;
        }
        if let Ok(image) = preview_worker::load_preview_asset_pixels(
            &arcade_screenshot_pack.display().to_string(),
            &asset_key,
        ) {
            let image = preview_pixels_to_saver_image(image);
            images.push(image);
        }
    }
    crate::ui_logln!(
        "screensaver_loader path={} images={}",
        arcade_screenshot_pack.display(),
        images.len()
    );
    images
}

fn screensaver_archive_path(asset_dir: Option<&OsStr>, layout: DeviceLayout) -> PathBuf {
    asset_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| layout.app_path("assets"))
        .join("arcade-screenshots-320x320.mmlz4b")
}

fn preview_pixels_to_saver_image(image: preview_worker::PreviewPixels) -> SaverImage {
    match image {
        preview_worker::PreviewPixels::Rgb565 {
            width,
            height,
            stride_bytes,
            words,
        } => SaverImage {
            pixels: words.iter().copied().map(Rgb565Pixel).collect(),
            w: width as usize,
            h: height as usize,
            stride: stride_bytes as usize / 2,
        },
    }
}

fn render_screensaver_frame(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    mode: ScreensaverMode,
    frame: u64,
) {
    if !matches!(
        mode,
        ScreensaverMode::AttractWall
            | ScreensaverMode::StarfieldCabinets
            | ScreensaverMode::TilemapMuseum
            | ScreensaverMode::PreviewPlasmaCollage
            | ScreensaverMode::PhosphorGrid
            | ScreensaverMode::ScannerContactSheet
            | ScreensaverMode::RandomAccessLoader
            | ScreensaverMode::ColorClashGallery
    ) {
        clear(dst, color565(2, 4, 10));
    }
    match mode {
        ScreensaverMode::AttractWall => render_attract_wall(dst, state, w, h, images, frame),
        ScreensaverMode::MvsCarousel => render_carousel(dst, w, h, images, frame),
        ScreensaverMode::SuperScalerFlyby => render_flyby(dst, w, h, images, frame),
        ScreensaverMode::StarfieldCabinets => {
            render_starfield_cabinets(dst, state, w, h, images, frame);
        }
        ScreensaverMode::ScreenshotRain => render_rain(dst, w, h, images, frame),
        ScreensaverMode::TilemapMuseum => render_tilemap(dst, state, w, h, images, frame),
        ScreensaverMode::RasterGallery => render_raster_gallery(dst, w, h, images, frame),
        ScreensaverMode::KefrensScreenshotBars => render_kefrens(dst, w, h, images, frame),
        ScreensaverMode::PreviewPlasmaCollage => {
            render_plasma_collage(dst, w, h, images, frame);
        }
        ScreensaverMode::PhosphorGrid => render_phosphor_grid(dst, state, w, h, images, frame),
        ScreensaverMode::WarpTunnel => render_warp(dst, w, h, images, frame),
        ScreensaverMode::Mode7Floor => render_mode7(dst, w, h, images, frame),
        ScreensaverMode::ScannerContactSheet => render_scanner(dst, state, w, h, images, frame),
        ScreensaverMode::SpriteMultiplexParade => {
            render_parade(dst, &mut state.parade, w, h, images, frame)
        }
        ScreensaverMode::CabinetMarquee => render_marquee(dst, w, h, images, frame),
        ScreensaverMode::RandomAccessLoader => {
            render_random_loader(dst, state, w, h, images, frame)
        }
        ScreensaverMode::ColorClashGallery => render_color_clash(dst, state, w, h, images, frame),
        ScreensaverMode::RadialStarfield => render_starfield(dst, w, h, frame),
        ScreensaverMode::PixelGrid => render_pixel_grid(dst, w, h),
        ScreensaverMode::IdleMegademo => {
            let sub =
                ScreensaverMode::MEGA[((frame / 240) as usize) % (ScreensaverMode::MEGA.len() - 1)];
            render_screensaver_frame(dst, state, w, h, images, sub, frame);
        }
    }
}

fn render_pixel_grid(dst: &mut [Rgb565Pixel], w: usize, h: usize) {
    let white = color565(255, 255, 255);
    let black = color565(0, 0, 0);
    let pattern = std::env::var("MISTER_SCALER_PATTERN")
        .unwrap_or_else(|_| "pixel-grid".into())
        .to_ascii_lowercase();
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            dst[row + x] = match pattern.as_str() {
                "vertical" => {
                    if x % 2 == 0 {
                        black
                    } else {
                        white
                    }
                }
                "horizontal" => {
                    if y % 2 == 0 {
                        black
                    } else {
                        white
                    }
                }
                "column-codes" => scaler_column_code(x, w),
                _ => {
                    if x % 2 == 0 || y % 2 == 0 {
                        black
                    } else {
                        white
                    }
                }
            };
        }
    }
}

fn scaler_column_code(x: usize, w: usize) -> Rgb565Pixel {
    let center = w / 2;
    if x == center {
        return color565(255, 255, 255);
    }
    if x + 1 == center || x == center + 1 {
        return color565(0, 0, 0);
    }
    let code = ((x as u32).wrapping_mul(0x9e37) ^ ((x as u32) << 7)) as u16;
    Rgb565Pixel(code | 0x0821)
}

fn color565(r: u8, g: u8, b: u8) -> Rgb565Pixel {
    <Rgb565Pixel as TargetPixel>::from_rgb(r, g, b)
}

fn clear(dst: &mut [Rgb565Pixel], color: Rgb565Pixel) {
    dst.fill(color);
}

fn sample_image(img: &SaverImage, x: usize, y: usize) -> Rgb565Pixel {
    img.pixels[(y.min(img.h - 1)) * img.stride + x.min(img.w - 1)]
}

fn blit_scaled(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    img: &SaverImage,
    x: isize,
    y: isize,
    out_w: usize,
    out_h: usize,
    tint: u8,
) {
    if out_w == 0 || out_h == 0 {
        return;
    }
    if tint == 255 && out_w == img.w && out_h == img.h {
        let dx0 = x.max(0) as usize;
        let dy0 = y.max(0) as usize;
        let dx1 = (x + out_w as isize).clamp(0, screen_w as isize) as usize;
        let dy1 = (y + out_h as isize).clamp(0, screen_h as isize) as usize;
        if dx1 <= dx0 || dy1 <= dy0 {
            return;
        }
        let src_x = (dx0 as isize - x) as usize;
        let copy_w = dx1 - dx0;
        for dst_y in dy0..dy1 {
            let src_y = (dst_y as isize - y) as usize;
            let dst_row = dst_y * screen_w + dx0;
            let src_row = src_y * img.stride + src_x;
            dst[dst_row..dst_row + copy_w].copy_from_slice(&img.pixels[src_row..src_row + copy_w]);
        }
        return;
    }

    let dx0 = x.max(0) as usize;
    let dy0 = y.max(0) as usize;
    let dx1 = (x + out_w as isize).clamp(0, screen_w as isize) as usize;
    let dy1 = (y + out_h as isize).clamp(0, screen_h as isize) as usize;
    if dx1 <= dx0 || dy1 <= dy0 {
        return;
    }

    let step_x_fp = ((img.w << 16) / out_w.max(1)).max(1);
    let step_y_fp = ((img.h << 16) / out_h.max(1)).max(1);
    let base_x_fp = (dx0 as isize - x).max(0) as usize * step_x_fp;
    let mut sy_fp = (dy0 as isize - y).max(0) as usize * step_y_fp;
    let dark = color565(0, 0, 18);
    for dy in dy0..dy1 {
        let sy = (sy_fp >> 16).min(img.h - 1);
        let mut sx_fp = base_x_fp;
        let dst_row = dy * screen_w;
        let src_row = sy * img.stride;
        if tint == 255 {
            for dx in dx0..dx1 {
                let sx = (sx_fp >> 16).min(img.w - 1);
                dst[dst_row + dx] = img.pixels[src_row + sx];
                sx_fp = sx_fp.saturating_add(step_x_fp);
            }
        } else {
            for dx in dx0..dx1 {
                let sx = (sx_fp >> 16).min(img.w - 1);
                dst[dst_row + dx] = blend_565(dark, img.pixels[src_row + sx], tint);
                sx_fp = sx_fp.saturating_add(step_x_fp);
            }
        }
        sy_fp = sy_fp.saturating_add(step_y_fp);
    }
}

const PARADE_SUBPIXEL_ONE: i64 = 256;

fn blit_scaled_subpixel_x(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    image: &SaverImage,
    half_shifted: &SaverImage,
    corner_insets: &[u8],
    x_fp: i64,
    y: isize,
) {
    let x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
    let fraction = x_fp.rem_euclid(PARADE_SUBPIXEL_ONE) as u8;
    if fraction == 0 {
        blit_rounded_card(dst, screen_w, screen_h, image, corner_insets, x, y);
        return;
    }
    if fraction == 128 {
        for src_y in 0..image.h {
            let dst_y = y + src_y as isize;
            if dst_y < 0 || dst_y >= screen_h as isize {
                continue;
            }
            let dst_row = dst_y as usize * screen_w;
            let src_row = src_y * image.stride;
            let shifted_row = src_y * half_shifted.stride;
            let inset = corner_insets.get(src_y).copied().unwrap_or(0) as usize;
            let source_end = image.w.saturating_sub(inset);
            if inset >= source_end {
                continue;
            }
            let left = x + inset as isize;
            if left >= 0 && left < screen_w as isize {
                dst[dst_row + left as usize] = blend_565(
                    dst[dst_row + left as usize],
                    image.pixels[src_row + inset],
                    127,
                );
            }
            let copy_x0 = (left + 1).max(0) as usize;
            let copy_x1 = (x + source_end as isize).clamp(0, screen_w as isize) as usize;
            if copy_x1 > copy_x0 {
                let source_x0 = (copy_x0 as isize - x) as usize;
                dst[dst_row + copy_x0..dst_row + copy_x1].copy_from_slice(
                    &half_shifted.pixels
                        [shifted_row + source_x0..shifted_row + source_x0 + copy_x1 - copy_x0],
                );
            }
            let right = x + source_end as isize;
            if right >= 0 && right < screen_w as isize {
                dst[dst_row + right as usize] = blend_565(
                    dst[dst_row + right as usize],
                    image.pixels[src_row + source_end - 1],
                    128,
                );
            }
        }
        return;
    }
    // Parade velocities are deliberately restricted to whole- and half-pixel
    // phases. Snap an unsupported future phase instead of silently falling
    // back to the ARM-hostile per-pixel fractional compositor.
    let snapped_x = if fraction < 128 { x } else { x + 1 };
    blit_rounded_card(dst, screen_w, screen_h, image, corner_insets, snapped_x, y);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CrtQuantizedPhase {
    x: isize,
    phase: usize,
}

fn quantize_crt_phase(x_fp: i64) -> CrtQuantizedPhase {
    let mut x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
    let fraction = x_fp.rem_euclid(PARADE_SUBPIXEL_ONE) as usize;
    let mut phase = (fraction + CRT_PHASE_STEP / 2) / CRT_PHASE_STEP;
    if phase == CRT_PHASE_COUNT {
        x += 1;
        phase = 0;
    }
    CrtQuantizedPhase { x, phase }
}

fn blit_scaled_crt_sixteenth_x(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    image: &SaverImage,
    phase_set: &ParadePhaseSet,
    corner_insets: &[u8],
    x_fp: i64,
    y: isize,
) {
    let quantized = quantize_crt_phase(x_fp);
    if quantized.phase == 0 {
        blit_rounded_card(
            dst,
            screen_w,
            screen_h,
            image,
            corner_insets,
            quantized.x,
            y,
        );
        return;
    }

    let Some(shifted) = phase_set.crt_phase(quantized.phase) else {
        debug_assert!(false, "CRT tile missing sixteenth-pixel phase bank");
        blit_scaled_subpixel_x(
            dst,
            screen_w,
            screen_h,
            image,
            phase_set.legacy_half(),
            corner_insets,
            x_fp,
            y,
        );
        return;
    };
    let phase_alpha = (quantized.phase * CRT_PHASE_STEP) as u8;
    blit_rounded_card_fractional(
        dst,
        screen_w,
        screen_h,
        image,
        shifted,
        corner_insets,
        quantized.x,
        y,
        phase_alpha,
    );
}

fn blit_rounded_card_fractional(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    image: &SaverImage,
    shifted: &SaverImage,
    corner_insets: &[u8],
    x: isize,
    y: isize,
    phase_alpha: u8,
) {
    for src_y in 0..image.h {
        let dst_y = y + src_y as isize;
        if dst_y < 0 || dst_y >= screen_h as isize {
            continue;
        }
        let dst_row = dst_y as usize * screen_w;
        let src_row = src_y * image.stride;
        let shifted_row = src_y * shifted.stride;
        let inset = corner_insets.get(src_y).copied().unwrap_or(0) as usize;
        let source_end = image.w.saturating_sub(inset);
        if inset >= source_end {
            continue;
        }
        let left = x + inset as isize;
        if left >= 0 && left < screen_w as isize {
            dst[dst_row + left as usize] = blend_565(
                dst[dst_row + left as usize],
                image.pixels[src_row + inset],
                255 - phase_alpha,
            );
        }
        let copy_x0 = (left + 1).max(0) as usize;
        let copy_x1 = (x + source_end as isize).clamp(0, screen_w as isize) as usize;
        if copy_x1 > copy_x0 {
            let source_x0 = (copy_x0 as isize - x) as usize;
            dst[dst_row + copy_x0..dst_row + copy_x1].copy_from_slice(
                &shifted.pixels
                    [shifted_row + source_x0..shifted_row + source_x0 + copy_x1 - copy_x0],
            );
        }
        let right = x + source_end as isize;
        if right >= 0 && right < screen_w as isize {
            dst[dst_row + right as usize] = blend_565(
                dst[dst_row + right as usize],
                image.pixels[src_row + source_end - 1],
                phase_alpha,
            );
        }
    }
}

fn blit_rounded_card(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    image: &SaverImage,
    corner_insets: &[u8],
    x: isize,
    y: isize,
) {
    for src_y in 0..image.h {
        let dst_y = y + src_y as isize;
        if dst_y < 0 || dst_y >= screen_h as isize {
            continue;
        }
        let inset = corner_insets.get(src_y).copied().unwrap_or(0) as usize;
        let source_end = image.w.saturating_sub(inset);
        if inset >= source_end {
            continue;
        }
        let dst_x0 = (x + inset as isize).max(0) as usize;
        let dst_x1 = (x + source_end as isize).clamp(0, screen_w as isize) as usize;
        if dst_x1 <= dst_x0 {
            continue;
        }
        let source_x0 = (dst_x0 as isize - x) as usize;
        let source_row = src_y * image.stride + source_x0;
        let target_row = dst_y as usize * screen_w + dst_x0;
        let copy_len = dst_x1 - dst_x0;
        dst[target_row..target_row + copy_len]
            .copy_from_slice(&image.pixels[source_row..source_row + copy_len]);
    }
}

fn prepare_fractional_shifted(image: &SaverImage, phase_alpha: u8) -> SaverImage {
    debug_assert!(phase_alpha > 0);
    let width = image.w + 1;
    let mut pixels = vec![Rgb565Pixel(0); width * image.h];
    for y in 0..image.h {
        let source = y * image.stride;
        let target = y * width;
        for x in 1..image.w {
            pixels[target + x] = blend_565(
                image.pixels[source + x - 1],
                image.pixels[source + x],
                255 - phase_alpha,
            );
        }
    }
    SaverImage {
        pixels,
        w: width,
        h: image.h,
        stride: width,
    }
}

fn prepare_half_shifted(image: &SaverImage) -> SaverImage {
    prepare_fractional_shifted(image, 128)
}

enum ParadePhaseSet {
    LegacyHalf(SaverImage),
    CrtSixteenth(Box<[SaverImage; CRT_SHIFTED_PHASE_COUNT]>),
}

impl ParadePhaseSet {
    fn prepare(image: &SaverImage, profile: ParadeSamplingProfile) -> Self {
        match profile {
            ParadeSamplingProfile::LegacyHalf => Self::LegacyHalf(prepare_half_shifted(image)),
            ParadeSamplingProfile::CrtSixteenth => {
                let phases = std::array::from_fn(|index| {
                    prepare_fractional_shifted(image, ((index + 1) * CRT_PHASE_STEP) as u8)
                });
                Self::CrtSixteenth(Box::new(phases))
            }
        }
    }

    fn legacy_half(&self) -> &SaverImage {
        match self {
            Self::LegacyHalf(image) => image,
            Self::CrtSixteenth(phases) => &phases[CRT_PHASE_COUNT / 2 - 1],
        }
    }

    fn crt_phase(&self, phase: usize) -> Option<&SaverImage> {
        if phase == 0 || phase >= CRT_PHASE_COUNT {
            return None;
        }
        match self {
            Self::CrtSixteenth(phases) => phases.get(phase - 1),
            Self::LegacyHalf(_) => None,
        }
    }

    fn resident_bytes(&self) -> usize {
        match self {
            Self::LegacyHalf(image) => image.pixels.len() * std::mem::size_of::<Rgb565Pixel>(),
            Self::CrtSixteenth(phases) => phases
                .iter()
                .map(|image| image.pixels.len() * std::mem::size_of::<Rgb565Pixel>())
                .sum(),
        }
    }
}

fn blit_slice_scaled(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    img: &SaverImage,
    src_x0: usize,
    src_w: usize,
    x: isize,
    y: isize,
    out_w: usize,
    out_h: usize,
    tint: u8,
) {
    if out_w == 0 || out_h == 0 || src_w == 0 {
        return;
    }
    let src_x0 = src_x0.min(img.w - 1);
    let src_w = src_w.min(img.w.saturating_sub(src_x0)).max(1);
    for yy in 0..out_h {
        let dy = y + yy as isize;
        if dy < 0 || dy >= screen_h as isize {
            continue;
        }
        let sy = yy * img.h / out_h;
        for xx in 0..out_w {
            let dx = x + xx as isize;
            if dx < 0 || dx >= screen_w as isize {
                continue;
            }
            let sx = src_x0 + (xx * src_w / out_w).min(src_w - 1);
            let mut px = sample_image(img, sx, sy);
            if tint < 255 {
                px = blend_565(color565(0, 0, 18), px, tint);
            }
            dst[dy as usize * screen_w + dx as usize] = px;
        }
    }
}

fn fill_rect(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: Rgb565Pixel,
) {
    let x1 = (x + w).min(screen_w);
    let y1 = (y + h).min(screen_h);
    for yy in y.min(screen_h)..y1 {
        dst[yy * screen_w + x.min(screen_w)..yy * screen_w + x1].fill(color);
    }
}

fn copy_rect(
    dst: &mut [Rgb565Pixel],
    src: &[Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    let x1 = (x + w).min(screen_w);
    let y1 = (y + h).min(screen_h);
    for yy in y.min(screen_h)..y1 {
        let row = yy * screen_w;
        dst[row + x.min(screen_w)..row + x1].copy_from_slice(&src[row + x.min(screen_w)..row + x1]);
    }
}

fn copy_rect_from_to(
    dst: &mut [Rgb565Pixel],
    src: &[Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    src_x: usize,
    src_y: usize,
    dst_x: isize,
    dst_y: isize,
    w: usize,
    h: usize,
) {
    let dx0 = dst_x.max(0) as usize;
    let dy0 = dst_y.max(0) as usize;
    let dx1 = (dst_x + w as isize).clamp(0, screen_w as isize) as usize;
    let dy1 = (dst_y + h as isize).clamp(0, screen_h as isize) as usize;
    if dx1 <= dx0 || dy1 <= dy0 {
        return;
    }
    let sx0 = src_x + (dx0 as isize - dst_x) as usize;
    let sy0 = src_y + (dy0 as isize - dst_y) as usize;
    for row in 0..(dy1 - dy0) {
        let src_row = (sy0 + row) * screen_w + sx0;
        let dst_row = (dy0 + row) * screen_w + dx0;
        dst[dst_row..dst_row + (dx1 - dx0)].copy_from_slice(&src[src_row..src_row + (dx1 - dx0)]);
    }
}

fn stroke_rect(
    dst: &mut [Rgb565Pixel],
    screen_w: usize,
    screen_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: Rgb565Pixel,
) {
    if w == 0 || h == 0 {
        return;
    }
    fill_rect(dst, screen_w, screen_h, x, y, w, 2, color);
    fill_rect(
        dst,
        screen_w,
        screen_h,
        x,
        y.saturating_add(h.saturating_sub(2)),
        w,
        2,
        color,
    );
    fill_rect(dst, screen_w, screen_h, x, y, 2, h, color);
    fill_rect(
        dst,
        screen_w,
        screen_h,
        x.saturating_add(w.saturating_sub(2)),
        y,
        2,
        h,
        color,
    );
}

fn image_at(images: &[SaverImage], idx: usize) -> Option<&SaverImage> {
    if images.is_empty() {
        None
    } else {
        images.get(idx % images.len())
    }
}

fn render_attract_wall(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    state.attract_wall.render(
        dst,
        w,
        h,
        images,
        Duration::from_micros(frame.saturating_mul(1_000_000) / 60),
    );
}

fn render_carousel(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    for i in 0..7 {
        if let Some(img) = image_at(images, i + frame as usize / 120) {
            let phase = ((frame as i64 * 3 + i as i64 * 91) % 640) - 320;
            let depth = 180 + ((phase.unsigned_abs() as usize * 220) / 320);
            let out_w = depth.min(360);
            let out_h = out_w * 3 / 4;
            let x = w as isize / 2 + phase as isize - out_w as isize / 2;
            let y = h as isize / 2 - out_h as isize / 2 + ((i as isize - 3).abs() * 8);
            blit_scaled(dst, w, h, img, x, y, out_w, out_h, 255);
        }
    }
}

fn render_flyby(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_starfield(dst, w, h, frame);
    for i in 0..5 {
        if let Some(img) = image_at(images, i + frame as usize / 100) {
            let scale_idx = ((frame as usize / 12 + i * 2) % 6).max(1);
            let out_w = [64, 96, 128, 160, 224, 320][scale_idx];
            let out_h = out_w * 3 / 4;
            let x = (w / 2 + (i * 173 + frame as usize * 2) % w) as isize - out_w as isize / 2;
            let y = (h / 2 + (i * 71 + frame as usize) % (h / 2)) as isize - out_h as isize / 2;
            blit_scaled(dst, w, h, img, x, y, out_w, out_h, 220);
        }
    }
}

fn render_starfield(dst: &mut [Rgb565Pixel], w: usize, h: usize, frame: u64) {
    clear(dst, color565(0, 0, 10));
    for i in 0..420 {
        let z = ((i * 17 + frame as usize * 3) % 255).max(1);
        let sx = ((i * 97) % w) as isize - w as isize / 2;
        let sy = ((i * 53) % h) as isize - h as isize / 2;
        let x = w as isize / 2 + sx * 255 / z as isize;
        let y = h as isize / 2 + sy * 255 / z as isize;
        if x >= 0 && y >= 0 && x < w as isize && y < h as isize {
            dst[y as usize * w + x as usize] = color565(80, 220, 255);
        }
    }
}

fn render_parade_background(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    motion_ticks_fp: u64,
    _motion: ParadeMotion,
) {
    clear(dst, color565(0, 0, 10));
    for i in 0..210usize {
        let layer = i & 3;
        let (x, fraction) = horizontal_star_position(i, w, h, motion_ticks_fp);
        let y = (i.wrapping_mul(83).wrapping_add(i.wrapping_mul(i) * 7)) % h;
        let brightness = [70, 110, 170, 235][layer];
        let color = color565(brightness / 2, brightness, 255);
        let row = y * w;
        dst[row + x] = blend_565(dst[row + x], color, 255 - fraction);
        if fraction > 0 {
            let next_x = (x + 1) % w;
            dst[row + next_x] = blend_565(dst[row + next_x], color, fraction);
        }
    }
}

fn horizontal_star_x(star: usize, width: usize, frame: u64) -> usize {
    horizontal_star_position(
        star,
        width,
        PARADE_REFERENCE_HEIGHT,
        frame.saturating_mul(PARADE_TICK_ONE as u64),
    )
    .0
}

fn horizontal_star_position(
    star: usize,
    width: usize,
    screen_h: usize,
    motion_ticks_fp: u64,
) -> (usize, u8) {
    const STAR_SPEED_DENOMINATOR: u64 = 16;
    const SUBPIXEL_ONE: u64 = 256;
    let speed_numerator = PARADE_MIN_TILE_SPEED as u64 * ((star & 3) + 1) as u64;
    let start_x = (star
        .wrapping_mul(197)
        .wrapping_add(star.wrapping_mul(star) * 13))
        % width;
    let scaled_ticks_fp = motion_ticks_fp
        .saturating_mul(screen_h as u64)
        .saturating_add((PARADE_REFERENCE_HEIGHT / 2) as u64)
        / PARADE_REFERENCE_HEIGHT as u64;
    let travel = scaled_ticks_fp
        .saturating_mul(speed_numerator)
        .saturating_mul(SUBPIXEL_ONE)
        / (STAR_SPEED_DENOMINATOR * PARADE_TICK_ONE as u64);
    let position = (start_x as u64 * SUBPIXEL_ONE + travel) % (width as u64 * SUBPIXEL_ONE);
    (
        (position / SUBPIXEL_ONE) as usize,
        (position % SUBPIXEL_ONE) as u8,
    )
}

fn render_starfield_cabinets(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    render_starfield(dst, w, h, frame);
    let cols = 5usize;
    let rows = 3usize;
    let cell_w = w / cols;
    let cell_h = h / rows;
    let contact_frame = frame / 2;
    let contact_start = (contact_frame / 90) as usize;
    if !state.starfield_contact_valid
        || state.starfield_contact_start != contact_start
        || state.starfield_contact.len() != dst.len()
    {
        state.starfield_contact.resize(dst.len(), Rgb565Pixel(0));
        clear(&mut state.starfield_contact, Rgb565Pixel(0));
        for row in 0..rows {
            for col in 0..cols {
                if let Some(img) = image_at(images, contact_start + row * cols + col) {
                    let x = col * cell_w + 8;
                    let y = row * cell_h + 8;
                    let out_w = cell_w.saturating_sub(16);
                    let out_h = cell_h.saturating_sub(16);
                    blit_scaled(
                        &mut state.starfield_contact,
                        w,
                        h,
                        img,
                        x as isize,
                        y as isize,
                        out_w,
                        out_h,
                        230,
                    );
                    stroke_rect(
                        &mut state.starfield_contact,
                        w,
                        h,
                        x,
                        y,
                        out_w,
                        out_h,
                        color565(40, 250, 220),
                    );
                }
            }
        }
        state.starfield_contact_start = contact_start;
        state.starfield_contact_valid = true;
    }

    for row in 0..rows {
        let ox = ((contact_frame as usize + row * 13) & 31) as isize - 16;
        for col in 0..cols {
            let x = col * cell_w + 8;
            let y = row * cell_h + 8;
            copy_rect_from_to(
                dst,
                &state.starfield_contact,
                w,
                h,
                x,
                y,
                x as isize + ox,
                y as isize,
                cell_w.saturating_sub(16),
                cell_h.saturating_sub(16),
            );
        }
    }
}

fn render_rain(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    for y in 0..h {
        let c = (y * 60 / h) as u8;
        fill_rect(dst, w, h, 0, y, w, 1, color565(0, c / 3, c));
    }
    for i in 0..28 {
        if let Some(img) = image_at(images, i + frame as usize / 75) {
            let x = ((i * 47) % (w + 120)) as isize - 60;
            let y = ((i * 83 + frame as usize * (2 + i % 4)) % (h + 96)) as isize - 72;
            let (tw, th) = if i & 1 == 0 { (80, 56) } else { (120, 84) };
            blit_scaled(dst, w, h, img, x, y, tw, th, 205);
        }
    }
}

fn render_tilemap(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let cols = 12usize;
    let rows = 8usize;
    let cell_w = w / cols;
    let cell_h = h / rows;
    let page = (frame / 180) as usize;
    if !state.tilemap_valid || state.tilemap_page != page || state.tilemap_normal.len() != dst.len()
    {
        state.tilemap_normal.resize(dst.len(), Rgb565Pixel(0));
        state.tilemap_bright.resize(dst.len(), Rgb565Pixel(0));
        clear(&mut state.tilemap_normal, color565(2, 4, 10));
        clear(&mut state.tilemap_bright, color565(2, 4, 10));
        for ty in 0..rows {
            for tx in 0..cols {
                if let Some(img) = image_at(images, page + ty * cols + tx) {
                    let x = (tx * cell_w) as isize;
                    let y = (ty * cell_h) as isize;
                    let out_w = cell_w.saturating_sub(2);
                    let out_h = cell_h.saturating_sub(2);
                    blit_scaled(
                        &mut state.tilemap_normal,
                        w,
                        h,
                        img,
                        x,
                        y,
                        out_w,
                        out_h,
                        185,
                    );
                    blit_scaled(
                        &mut state.tilemap_bright,
                        w,
                        h,
                        img,
                        x,
                        y,
                        out_w,
                        out_h,
                        255,
                    );
                }
            }
        }
        state.tilemap_page = page;
        state.tilemap_valid = true;
    }

    dst.copy_from_slice(&state.tilemap_normal);
    for ty in 0..rows {
        for tx in 0..cols {
            let flash = hash2_u8(tx + page, ty) < (frame as u8).wrapping_mul(3);
            if flash {
                copy_rect(
                    dst,
                    &state.tilemap_bright,
                    w,
                    h,
                    tx * cell_w,
                    ty * cell_h,
                    cell_w.saturating_sub(2),
                    cell_h.saturating_sub(2),
                );
            }
        }
    }
}

fn render_raster_gallery(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    for y in 0..h {
        let c = (((y + frame as usize) & 63) * 3) as u8;
        for x in 0..w {
            dst[y * w + x] = color565(c / 3, c, 80);
        }
    }
    if let Some(curr) = image_at(images, frame as usize / 240) {
        blit_scaled(dst, w, h, curr, 220, 70, 520, 390, 230);
    }
    if let Some(next) = image_at(images, frame as usize / 240 + 1) {
        let reveal_y = ((frame as usize % 180) * h) / 180;
        for y in (0..reveal_y).step_by(8) {
            blit_slice_scaled(dst, w, h, next, 0, next.w, 220, y as isize, 520, 6, 255);
        }
    }
    stroke_rect(dst, w, h, 218, 68, 524, 394, color565(255, 80, 200));
}

fn render_kefrens(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    let bar_w = 24usize;
    let bars = w / bar_w + 3;
    let mut y = 0usize;
    while y < h {
        let row = y * w;
        for bar in 0..bars {
            if let Some(img) = image_at(images, bar + frame as usize / 120) {
                let wave = triangle_wave_u8(y / 3 + bar * 5, frame as u8) as isize / 5 - 25;
                let x0 = bar as isize * bar_w as isize - bar_w as isize + wave;
                let x1 = x0 + bar_w as isize;
                if x1 <= 0 || x0 >= w as isize {
                    continue;
                }
                let dst_x0 = x0.max(0) as usize;
                let dst_x1 = x1.min(w as isize) as usize;
                let src_y = y * img.h / h;
                let src_row = src_y * img.stride;
                let src_base = (bar * 23 + frame as usize / 3) % img.w;
                for x in dst_x0..dst_x1 {
                    let local = (x as isize - x0) as usize;
                    let src_x = (src_base + local).min(img.w - 1);
                    dst[row + x] = img.pixels[src_row + src_x];
                }
            }
        }
        if y + 1 < h {
            let next = (y + 1) * w;
            let (head, tail) = dst.split_at_mut(next);
            tail[..w].copy_from_slice(&head[row..row + w]);
        }
        y += 2;
    }
}

fn render_plasma_collage(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let tile = 16usize;
    let page = frame as usize / 180;
    for y in (0..h).step_by(tile) {
        for x in (0..w).step_by(tile) {
            let selector = plasma_gate(x / tile, y / tile, frame as u8) as usize;
            if let Some(img) = image_at(images, page + selector / 32) {
                let sx = (x * img.w / w + selector) % img.w;
                let sy = (y * img.h / h + selector / 2) % img.h;
                let px = sample_image(img, sx, sy);
                fill_rect(dst, w, h, x, y, tile, tile, px);
            }
        }
    }
}

fn render_phosphor_grid(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let cols = 12usize;
    let rows = 8usize;
    let cell_w = w / cols;
    let cell_h = h / rows;
    let page = frame as usize / 240;
    if !state.phosphor_grid_valid
        || state.phosphor_grid_page != page
        || state.phosphor_grid.len() != dst.len()
    {
        state.phosphor_grid.resize(dst.len(), Rgb565Pixel(0));
        fill_rect(
            &mut state.phosphor_grid,
            w,
            h,
            0,
            0,
            w,
            h,
            color565(0, 18, 14),
        );
        for ty in 0..rows {
            for tx in 0..cols {
                if let Some(img) = image_at(images, page + ty * cols + tx) {
                    blit_scaled(
                        &mut state.phosphor_grid,
                        w,
                        h,
                        img,
                        (tx * cell_w) as isize,
                        (ty * cell_h) as isize,
                        cell_w.saturating_sub(2),
                        cell_h.saturating_sub(2),
                        105,
                    );
                }
            }
        }
        state.phosphor_grid_page = page;
        state.phosphor_grid_valid = true;
    }

    dst.copy_from_slice(&state.phosphor_grid);
    for y in (0..h).step_by(24) {
        fill_rect(dst, w, h, 0, y, w, 1, color565(30, 255, 180));
    }
    for x in (0..w).step_by(32) {
        fill_rect(dst, w, h, x, 0, 1, h, color565(20, 180, 150));
    }
    if frame % 180 < 12 {
        fill_rect(dst, w, h, 0, h / 2 - 3, w, 6, color565(180, 255, 230));
        fill_rect(dst, w, h, w / 2 - 3, 0, 6, h, color565(120, 255, 220));
    }
}

fn render_warp(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    render_starfield(dst, w, h, frame);
    for i in 0..12 {
        if let Some(img) = image_at(images, i + frame as usize / 150) {
            let inset_x = i * 34 + (frame as usize % 34);
            let inset_y = i * 18 + (frame as usize % 18);
            if inset_x * 2 >= w || inset_y * 2 >= h {
                continue;
            }
            let rw = w - inset_x * 2;
            let rh = h - inset_y * 2;
            blit_slice_scaled(
                dst,
                w,
                h,
                img,
                0,
                img.w,
                inset_x as isize,
                inset_y as isize,
                rw,
                4,
                180,
            );
            blit_slice_scaled(
                dst,
                w,
                h,
                img,
                0,
                img.w,
                inset_x as isize,
                (inset_y + rh.saturating_sub(4)) as isize,
                rw,
                4,
                180,
            );
            stroke_rect(dst, w, h, inset_x, inset_y, rw, rh, color565(60, 220, 255));
        }
    }
}

fn render_mode7(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    clear(dst, color565(4, 8, 22));
    if let Some(img) = image_at(images, frame as usize / 150) {
        let mut y = h / 2;
        while y < h {
            let depth = (y - h / 2 + 1) * 2;
            let span = (w * 80 / depth).max(1);
            let step_fp = ((span << 16) / w.max(1)).max(1);
            let mut sx_fp = 0usize;
            let base_x = (frame as usize * 2) % img.w;
            let sy = ((depth + frame as usize) / 3) % img.h;
            let row = y * w;
            for x in 0..w {
                let sx = (base_x + (sx_fp >> 16)) % img.w;
                dst[row + x] = sample_image(img, sx, sy);
                sx_fp = sx_fp.saturating_add(step_fp);
            }
            if y + 1 < h {
                let next = (y + 1) * w;
                let (head, tail) = dst.split_at_mut(next);
                tail[..w].copy_from_slice(&head[row..row + w]);
            }
            y += 2;
        }
    }
}

fn render_scanner(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let cols = 7usize;
    let rows = 4usize;
    let cell_w = w / cols;
    let cell_h = h / rows;
    let contact_frame = frame / 5;
    let contact_start = (contact_frame / 90) as usize;
    if !state.scanner_contact_valid
        || state.scanner_contact_start != contact_start
        || state.scanner_contact.len() != dst.len()
    {
        state.scanner_contact.resize(dst.len(), Rgb565Pixel(0));
        clear(&mut state.scanner_contact, color565(2, 4, 10));
        for row in 0..rows {
            for col in 0..cols {
                if let Some(img) = image_at(images, contact_start + row * cols + col) {
                    let x = col * cell_w + 8;
                    let y = row * cell_h + 8;
                    let out_w = cell_w.saturating_sub(16);
                    let out_h = cell_h.saturating_sub(16);
                    blit_scaled(
                        &mut state.scanner_contact,
                        w,
                        h,
                        img,
                        x as isize,
                        y as isize,
                        out_w,
                        out_h,
                        230,
                    );
                    stroke_rect(
                        &mut state.scanner_contact,
                        w,
                        h,
                        x,
                        y,
                        out_w,
                        out_h,
                        color565(40, 250, 220),
                    );
                }
            }
        }
        state.scanner_contact_start = contact_start;
        state.scanner_contact_valid = true;
    }

    dst.copy_from_slice(&state.scanner_contact);
    let scan_y = (frame as usize * 5) % h;
    for y in scan_y.saturating_sub(3)..(scan_y + 4).min(h) {
        for x in 0..w {
            dst[y * w + x] = brighten_565(dst[y * w + x]);
        }
    }
    let active = (scan_y * 7 / h).min(6);
    if let Some(img) = image_at(images, active + frame as usize / 120) {
        blit_scaled(dst, w, h, img, 360, 170, 240, 180, 255);
        stroke_rect(dst, w, h, 358, 168, 244, 184, color565(255, 255, 255));
    }
}

const PARADE_WIDE_LAYER_TARGETS: [usize; 5] = [33, 24, 20, 16, 12];
const PARADE_COMPACT_LAYER_TARGETS: [usize; 5] = [25, 18, 15, 12, 9];
// Whole-pixel comparison mode needs every depth layer to move each 60 Hz frame
// and leaves one slower whole-pixel speed for the star field.
const PARADE_LAYER_SPEEDS: [usize; 5] = [2, 3, 4, 5, 6];
const PARADE_REFERENCE_HEIGHT: usize = 540;
// Preserve edge-to-edge travel time at every geometry, using velocities that
// are 25% slower than the former 1920x1080 reference behavior.
const PARADE_REFERENCE_WIDTH: usize = 1920;
const PARADE_CARD_SPEED_NUMERATOR: i64 = 3;
const PARADE_CARD_SPEED_DENOMINATOR: i64 = 4;
const PARADE_MAX_CARD_ADOPTIONS_PER_FRAME: usize = 1;
const PARADE_REFERENCE_HZ: u64 = 60;
const PARADE_TICK_ONE: i64 = 1 << 16;
const PARADE_MIN_TILE_SPEED: usize = 1;
const PARADE_SPEED_COUNT: usize = 5;
const PARADE_REFERENCE_PLACEMENT_GAP: usize = 18;
const CRT_PHASE_COUNT: usize = 16;
const CRT_SHIFTED_PHASE_COUNT: usize = CRT_PHASE_COUNT - 1;
const CRT_PHASE_STEP: usize = PARADE_SUBPIXEL_ONE as usize / CRT_PHASE_COUNT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParadeSamplingProfile {
    LegacyHalf,
    CrtSixteenth,
}

impl ParadeSamplingProfile {
    const fn for_crt_output(crt_output: bool) -> Self {
        if crt_output {
            Self::CrtSixteenth
        } else {
            Self::LegacyHalf
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::LegacyHalf => "legacy-half",
            Self::CrtSixteenth => "crt-16",
        }
    }

    const fn for_layer(self, layer: usize) -> Self {
        if matches!(self, Self::CrtSixteenth) || layer == PARADE_MIN_TILE_SPEED {
            Self::CrtSixteenth
        } else {
            Self::LegacyHalf
        }
    }

    const fn sixteenth_layer_mask(self) -> u8 {
        if matches!(self, Self::CrtSixteenth) {
            (1_u8 << PARADE_SPEED_COUNT) - 1
        } else {
            1
        }
    }

    const fn layer_evidence(self) -> &'static str {
        if matches!(self, Self::CrtSixteenth) {
            "1:crt-16,2:crt-16,3:crt-16,4:crt-16,5:crt-16"
        } else {
            "1:crt-16,2:legacy-half,3:legacy-half,4:legacy-half,5:legacy-half"
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParadeMotion {
    Integer,
    Subpixel,
}

impl ParadeMotion {
    fn from_env() -> Self {
        match std::env::var("MISTER_PARADE_MOTION")
            .unwrap_or_else(|_| "subpixel".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "subpixel" => Self::Subpixel,
            _ => Self::Integer,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Subpixel => "subpixel",
        }
    }

    fn card_velocity_fp(self, layer_idx: usize, screen_w: usize) -> i64 {
        let reference_1080p = match self {
            Self::Integer => PARADE_LAYER_SPEEDS[layer_idx] as i64 * PARADE_SUBPIXEL_ONE * 2,
            Self::Subpixel => (layer_idx as i64 + 1) * PARADE_SUBPIXEL_ONE,
        };
        scale_parade_velocity(reference_1080p, screen_w)
    }
}

fn scale_parade_velocity(reference_1080p_fp: i64, screen_w: usize) -> i64 {
    let slowed_reference = reference_1080p_fp
        .saturating_mul(PARADE_CARD_SPEED_NUMERATOR)
        .checked_div(PARADE_CARD_SPEED_DENOMINATOR)
        .unwrap_or(1);
    slowed_reference
        .saturating_mul(screen_w as i64)
        .saturating_add((PARADE_REFERENCE_WIDTH / 2) as i64)
        .checked_div(PARADE_REFERENCE_WIDTH as i64)
        .unwrap_or(1)
        .max(1)
}

fn parade_tick_delta_fp(elapsed: Duration) -> i64 {
    let ticks = elapsed
        .as_nanos()
        .saturating_mul(PARADE_REFERENCE_HZ as u128)
        .saturating_mul(PARADE_TICK_ONE as u128)
        / 1_000_000_000u128;
    ticks.min(i64::MAX as u128) as i64
}

fn parade_layer_targets(screen_w: usize, screen_h: usize) -> [usize; PARADE_SPEED_COUNT] {
    if screen_w.saturating_mul(3) <= screen_h.saturating_mul(4) {
        PARADE_COMPACT_LAYER_TARGETS
    } else {
        PARADE_WIDE_LAYER_TARGETS
    }
}

fn scale_parade_dimension(reference: usize, screen_h: usize) -> usize {
    reference
        .saturating_mul(screen_h)
        .saturating_add(PARADE_REFERENCE_HEIGHT / 2)
        .checked_div(PARADE_REFERENCE_HEIGHT)
        .unwrap_or(1)
        .max(1)
}

fn parade_depth_style(speed: usize, screen_h: usize) -> (usize, usize, u8) {
    let depth = speed
        .saturating_sub(PARADE_MIN_TILE_SPEED)
        .min(PARADE_SPEED_COUNT - 1);
    let speed = depth + PARADE_MIN_TILE_SPEED;
    // Each layer occupies another fifth of the half-size 160x160 maximum.
    // The actual card is fitted inside this box without distortion.
    let reference = 160 * speed / PARADE_SPEED_COUNT;
    (
        scale_parade_dimension(reference, screen_h),
        scale_parade_dimension(reference, screen_h),
        [145, 170, 198, 226, 255][depth],
    )
}

fn parade_layer_interval_frames(
    screen_w: usize,
    tile_w: usize,
    velocity_fp: i64,
    target_count: usize,
) -> u64 {
    let travel_fp = (screen_w + tile_w) as i64 * PARADE_SUBPIXEL_ONE;
    let velocity_fp = velocity_fp.max(1);
    let travel_frames = ((travel_fp + velocity_fp - 1) / velocity_fp) as usize;
    (travel_frames / target_count.max(1)).max(1) as u64
}

fn parade_scaled_style(image: &SaverImage, speed: usize, screen_h: usize) -> (usize, usize, u8) {
    let (box_w, box_h, tint) = parade_depth_style(speed, screen_h);
    if image.w * box_h > image.h * box_w {
        (box_w, (box_w * image.h + image.w / 2) / image.w, tint)
    } else {
        ((box_h * image.w + image.h / 2) / image.h, box_h, tint)
    }
}

const LANCZOS_RADIUS: f64 = 3.0;
const LANCZOS_WEIGHT_ONE: i32 = 1 << 14;

struct LanczosFilter {
    start: usize,
    weights: Vec<i16>,
}

fn lanczos3(value: f64) -> f64 {
    let value = value.abs();
    if value < f64::EPSILON {
        return 1.0;
    }
    if value >= LANCZOS_RADIUS {
        return 0.0;
    }
    let pi_value = std::f64::consts::PI * value;
    (pi_value.sin() / pi_value) * ((pi_value / LANCZOS_RADIUS).sin() / (pi_value / LANCZOS_RADIUS))
}

fn lanczos_filters(src_len: usize, dst_len: usize) -> Vec<LanczosFilter> {
    let scale = dst_len as f64 / src_len as f64;
    let filter_scale = scale.min(1.0);
    let support = LANCZOS_RADIUS / filter_scale;
    (0..dst_len)
        .map(|dst| {
            let center = (dst as f64 + 0.5) / scale - 0.5;
            let first = (center - support).ceil() as isize;
            let last = (center + support).floor() as isize;
            let start = first.max(0) as usize;
            let end = last.min(src_len as isize - 1).max(first.max(0)) as usize;
            let float_weights = (start..=end)
                .map(|src| lanczos3((src as f64 - center) * filter_scale) * filter_scale)
                .collect::<Vec<_>>();
            let sum = float_weights.iter().sum::<f64>();
            let mut weights = float_weights
                .iter()
                .map(|weight| (weight / sum * LANCZOS_WEIGHT_ONE as f64).round() as i16)
                .collect::<Vec<_>>();
            let fixed_sum = weights.iter().map(|weight| *weight as i32).sum::<i32>();
            let center_tap = weights.len() / 2;
            weights[center_tap] =
                (weights[center_tap] as i32 + LANCZOS_WEIGHT_ONE - fixed_sum) as i16;
            LanczosFilter { start, weights }
        })
        .collect()
}

fn scale_lanczos3_rgb565_tinted(
    image: &SaverImage,
    out_w: usize,
    out_h: usize,
    tint: u8,
) -> SaverImage {
    if out_w == 0 || out_h == 0 || image.w == 0 || image.h == 0 {
        return SaverImage {
            pixels: Vec::new(),
            w: out_w,
            h: out_h,
            stride: out_w,
        };
    }
    let x_filters = lanczos_filters(image.w, out_w);
    let y_filters = lanczos_filters(image.h, out_h);
    // RGB888 packed into u32 keeps the separable intermediate compact and the
    // hot vertical pass cache-friendly. All transcendental work happened above.
    let mut horizontal = vec![0_u32; out_w * image.h];
    for src_y in 0..image.h {
        let src_row = src_y * image.stride;
        let dst_row = src_y * out_w;
        for (dst_x, filter) in x_filters.iter().enumerate() {
            let mut r = 0_i32;
            let mut g = 0_i32;
            let mut b = 0_i32;
            for (tap, weight) in filter.weights.iter().enumerate() {
                let pixel = image.pixels[src_row + filter.start + tap].0;
                let weight = *weight as i32;
                r += (((pixel >> 11) & 0x1f) as i32 * 255 / 31) * weight;
                g += (((pixel >> 5) & 0x3f) as i32 * 255 / 63) * weight;
                b += ((pixel & 0x1f) as i32 * 255 / 31) * weight;
            }
            let r = ((r + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255) as u32;
            let g = ((g + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255) as u32;
            let b = ((b + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255) as u32;
            horizontal[dst_row + dst_x] = (r << 16) | (g << 8) | b;
        }
    }

    let mut pixels = vec![Rgb565Pixel(0); out_w * out_h];
    for (dst_y, filter) in y_filters.iter().enumerate() {
        for dst_x in 0..out_w {
            let mut r = 0_i32;
            let mut g = 0_i32;
            let mut b = 0_i32;
            for (tap, weight) in filter.weights.iter().enumerate() {
                let pixel = horizontal[(filter.start + tap) * out_w + dst_x];
                let weight = *weight as i32;
                r += ((pixel >> 16) & 0xff) as i32 * weight;
                g += ((pixel >> 8) & 0xff) as i32 * weight;
                b += (pixel & 0xff) as i32 * weight;
            }
            let tint = tint as i32;
            let r = ((((r + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255) * tint + 127) / 255) as u8;
            let g = ((((g + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255) * tint + 127) / 255) as u8;
            let b = ((((b + LANCZOS_WEIGHT_ONE / 2) >> 14).clamp(0, 255) * tint + 127) / 255) as u8;
            pixels[dst_y * out_w + dst_x] = color565(r, g, b);
        }
    }
    SaverImage {
        pixels,
        w: out_w,
        h: out_h,
        stride: out_w,
    }
}

fn apply_parade_depth_cues(image: &mut SaverImage, speed: usize) {
    let depth = speed
        .saturating_sub(PARADE_MIN_TILE_SPEED)
        .min(PARADE_SPEED_COUNT - 1);
    let atmosphere = [20_u32, 14, 8, 3, 0][depth];
    let desaturation = [25_u32, 16, 8, 3, 0][depth];
    for pixel in &mut image.pixels {
        let packed = pixel.0;
        let mut r = u32::from((packed >> 11) & 0x1f) * 255 / 31;
        let mut g = u32::from((packed >> 5) & 0x3f) * 255 / 63;
        let mut b = u32::from(packed & 0x1f) * 255 / 31;
        let luminance = (77 * r + 150 * g + 29 * b + 128) >> 8;
        r = (r * (100 - desaturation) + luminance * desaturation + 50) / 100;
        g = (g * (100 - desaturation) + luminance * desaturation + 50) / 100;
        b = (b * (100 - desaturation) + luminance * desaturation + 50) / 100;
        r = (r * (100 - atmosphere) + 50) / 100;
        g = (g * (100 - atmosphere) + 50) / 100;
        b = (b * (100 - atmosphere) + 10 * atmosphere + 50) / 100;
        *pixel = color565(r as u8, g as u8, b as u8);
    }
}

fn rim_parade_card(image: &mut SaverImage, corner_insets: &[u8]) {
    if image.w == 0 || image.h == 0 {
        return;
    }
    let highlight = color565(210, 225, 255);
    let shadow = color565(0, 0, 8);
    for y in 0..image.h {
        let inset = corner_insets.get(y).copied().unwrap_or(0) as usize;
        let end = image.w.saturating_sub(inset);
        if inset >= end {
            continue;
        }
        let row = y * image.stride;
        for (offset, alpha) in [48_u8, 24].into_iter().enumerate() {
            if inset + offset < end {
                let left = row + inset + offset;
                image.pixels[left] = blend_565(image.pixels[left], highlight, alpha);
            }
            if end > inset + offset {
                let right = row + end - 1 - offset;
                image.pixels[right] = blend_565(image.pixels[right], shadow, alpha + 8);
            }
        }
        let horizontal_cue = if y < 2 {
            Some((highlight, [40_u8, 20][y]))
        } else if image.h - 1 - y < 2 {
            let edge = image.h - 1 - y;
            Some((shadow, [56_u8, 28][edge]))
        } else {
            None
        };
        if let Some((color, alpha)) = horizontal_cue {
            for pixel in &mut image.pixels[row + inset..row + end] {
                *pixel = blend_565(*pixel, color, alpha);
            }
        }
    }
}

fn prepare_parade_scaled(
    image: &SaverImage,
    speed: usize,
    screen_h: usize,
) -> (SaverImage, Vec<u8>) {
    let (w, h, tint) = parade_scaled_style(image, speed, screen_h);
    let mut scaled = scale_lanczos3_rgb565_tinted(image, w, h, tint);
    apply_parade_depth_cues(&mut scaled, speed);
    let corner_insets = prepare_parade_corner_insets(scaled.w, scaled.h);
    let depth = speed
        .saturating_sub(PARADE_MIN_TILE_SPEED)
        .min(PARADE_SPEED_COUNT - 1);
    if depth >= 3 {
        rim_parade_card(&mut scaled, &corner_insets);
    }
    (scaled, corner_insets)
}

fn prepare_parade_corner_insets(width: usize, height: usize) -> Vec<u8> {
    let radius = (width.min(height) / 10).clamp(2, 10);
    let mut insets = vec![0_u8; height];
    for y in 0..radius.min(height / 2) {
        let dy = radius.saturating_sub(y + 1) as f64;
        let inside = ((radius * radius) as f64 - dy * dy).max(0.0).sqrt() as usize;
        let inset = radius.saturating_sub(inside).min(u8::MAX as usize) as u8;
        insets[y] = inset;
        insets[height - 1 - y] = inset;
    }
    insets
}

struct ParadeTile {
    x_fp: i64,
    y: isize,
    layer: usize,
    speed: usize,
    velocity_fp: i64,
    velocity_remainder: i64,
    image_idx: usize,
    scaled: SaverImage,
    phase_set: ParadePhaseSet,
    corner_insets: Vec<u8>,
    active: bool,
    raster_held_this_frame: bool,
    raster_moved_this_frame: bool,
    next: Option<PreparedParadeCard>,
    pending_image_idx: Option<usize>,
}

impl ParadeTile {
    fn x(&self) -> isize {
        self.x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize
    }
}

#[derive(Clone, Copy)]
struct ParadeLayerSchedule {
    next_spawn_frame: u64,
    interval_frames: u64,
    spawn_count: u64,
    active_sum: u64,
    sample_count: u64,
}

struct PreparedParadeCard {
    image_idx: usize,
    speed: usize,
    scaled: SaverImage,
    phase_set: ParadePhaseSet,
    corner_insets: Vec<u8>,
    scale_us: u128,
    phase_us: u128,
}

struct ParadeScaleJob {
    tile_idx: usize,
    image_idx: usize,
    speed: usize,
    sampling_profile: ParadeSamplingProfile,
    screen_h: usize,
    source: ParadeScaleSource,
}

enum ParadeScaleSource {
    Decoded(SaverImage),
    ArchiveIndex(usize),
}

struct ParadeScaleResult {
    tile_idx: usize,
    image_idx: usize,
    card: Result<PreparedParadeCard, String>,
}

struct ParadeState {
    tiles: Vec<ParadeTile>,
    draw_order: Vec<usize>,
    visible_draw_order: Vec<usize>,
    depth_coverage: Vec<DirtyRect>,
    deck: Vec<usize>,
    cursor: usize,
    rng: u64,
    image_count: usize,
    scale_count: u64,
    scale_total_us: u128,
    scale_max_us: u128,
    phase_count: u64,
    phase_total_us: u128,
    phase_max_us: u128,
    scale_tx: Sender<ParadeScaleJob>,
    scale_rx: Receiver<ParadeScaleResult>,
    scale_worker_connected: bool,
    scale_queue_depth: usize,
    scale_queue_max: usize,
    archive_backed: bool,
    asset_keys: Vec<String>,
    decode_successes: u64,
    decode_failures: u64,
    unique_decoded: HashSet<usize>,
    failed_images: HashSet<usize>,
    sampling_profile: ParadeSamplingProfile,
    screen_w: usize,
    screen_h: usize,
    layer_targets: [usize; PARADE_SPEED_COUNT],
    layers: [ParadeLayerSchedule; PARADE_SPEED_COUNT],
    motion: ParadeMotion,
    startup_started_at: Option<Instant>,
    first_card_ready_logged: bool,
}

impl ParadeState {
    fn new(seed: u64) -> Self {
        Self::new_with_profile(seed, ParadeSamplingProfile::LegacyHalf)
    }

    fn new_with_profile(seed: u64, sampling_profile: ParadeSamplingProfile) -> Self {
        Self::new_with_motion(seed, ParadeMotion::from_env(), sampling_profile)
    }

    fn new_with_motion(
        seed: u64,
        motion: ParadeMotion,
        sampling_profile: ParadeSamplingProfile,
    ) -> Self {
        Self::new_with_source(seed, motion, sampling_profile, None)
    }

    #[cfg(test)]
    fn new_with_archive(
        seed: u64,
        archive: preview_worker::ResidentPreviewArchive,
        sampling_profile: ParadeSamplingProfile,
    ) -> Self {
        Self::new_with_source(
            seed,
            ParadeMotion::from_env(),
            sampling_profile,
            Some(archive),
        )
    }

    fn new_with_source(
        seed: u64,
        motion: ParadeMotion,
        sampling_profile: ParadeSamplingProfile,
        mut archive: Option<preview_worker::ResidentPreviewArchive>,
    ) -> Self {
        let archive_backed = archive.is_some();
        // A tile can own at most one pending successor, so the producer is
        // logically bounded by the selected population profile. Keep submission
        // non-blocking: Lanczos work must never stall the launcher frame.
        let (scale_tx, job_rx) = mpsc::channel::<ParadeScaleJob>();
        let (result_tx, scale_rx) = mpsc::channel::<ParadeScaleResult>();
        std::thread::Builder::new()
            .name("screensaver-lanczos".into())
            .spawn(move || {
                mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
                    mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverScaler,
                );
                while let Ok(job) = job_rx.recv() {
                    let started = Instant::now();
                    let source = match job.source {
                        ParadeScaleSource::Decoded(source) => Ok(source),
                        ParadeScaleSource::ArchiveIndex(index) => archive
                            .as_mut()
                            .ok_or_else(|| "screensaver archive reader unavailable".to_string())
                            .and_then(|archive| archive.load_pixels_at(index))
                            .map(preview_pixels_to_saver_image),
                    };
                    let card = source.map(|source| {
                        let (scaled, corner_insets) =
                            prepare_parade_scaled(&source, job.speed, job.screen_h);
                        let phase_started = Instant::now();
                        let phase_set = ParadePhaseSet::prepare(&scaled, job.sampling_profile);
                        let phase_us = phase_started.elapsed().as_micros();
                        PreparedParadeCard {
                            image_idx: job.image_idx,
                            speed: job.speed,
                            scaled,
                            phase_set,
                            corner_insets,
                            scale_us: started.elapsed().as_micros(),
                            phase_us,
                        }
                    });
                    if result_tx
                        .send(ParadeScaleResult {
                            tile_idx: job.tile_idx,
                            image_idx: job.image_idx,
                            card,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("spawn screensaver Lanczos worker");
        Self {
            tiles: Vec::new(),
            draw_order: Vec::with_capacity(PARADE_WIDE_LAYER_TARGETS.iter().sum()),
            visible_draw_order: Vec::with_capacity(PARADE_WIDE_LAYER_TARGETS.iter().sum()),
            depth_coverage: Vec::with_capacity(PARADE_WIDE_LAYER_TARGETS.iter().sum()),
            deck: Vec::new(),
            cursor: 0,
            rng: seed,
            image_count: 0,
            scale_count: 0,
            scale_total_us: 0,
            scale_max_us: 0,
            phase_count: 0,
            phase_total_us: 0,
            phase_max_us: 0,
            scale_tx,
            scale_rx,
            scale_worker_connected: true,
            scale_queue_depth: 0,
            scale_queue_max: 0,
            archive_backed,
            asset_keys: Vec::new(),
            decode_successes: 0,
            decode_failures: 0,
            unique_decoded: HashSet::new(),
            failed_images: HashSet::new(),
            sampling_profile,
            screen_w: 0,
            screen_h: PARADE_REFERENCE_HEIGHT,
            layer_targets: PARADE_WIDE_LAYER_TARGETS,
            layers: [ParadeLayerSchedule {
                next_spawn_frame: 0,
                interval_frames: 1,
                spawn_count: 0,
                active_sum: 0,
                sample_count: 0,
            }; PARADE_SPEED_COUNT],
            motion,
            startup_started_at: None,
            first_card_ready_logged: false,
        }
    }

    #[cfg(test)]
    fn ensure_archive_initialized_cancellable(
        &mut self,
        asset_keys: Vec<String>,
        w: usize,
        h: usize,
        cancelled: &AtomicBool,
    ) -> bool {
        self.set_geometry(w, h);
        self.tiles.clear();
        self.asset_keys = asset_keys;
        self.deck = (0..self.asset_keys.len()).collect();
        shuffle(&mut self.deck, &mut self.rng);
        self.cursor = 0;
        self.image_count = self.asset_keys.len();
        self.failed_images.clear();
        if self.image_count == 0 {
            return false;
        }
        for (layer_idx, target) in self.layer_targets.into_iter().enumerate() {
            let speed = PARADE_MIN_TILE_SPEED + layer_idx;
            let velocity_fp = self.motion.card_velocity_fp(layer_idx, w);
            let (tile_w, _, _) = parade_depth_style(speed, h);
            let interval_frames = parade_layer_interval_frames(w, tile_w, velocity_fp, target);
            let phase = self.random_below(interval_frames as usize) as u64;
            self.layers[layer_idx] = ParadeLayerSchedule {
                next_spawn_frame: phase,
                interval_frames,
                spawn_count: 0,
                active_sum: 0,
                sample_count: 0,
            };
            for rank in 0..target {
                if cancelled.load(Ordering::Relaxed) {
                    return false;
                }
                let tile_idx = self.tiles.len();
                let Some(card) = self.prepare_archive_card(tile_idx, speed, cancelled) else {
                    if cancelled.load(Ordering::Relaxed) {
                        return false;
                    }
                    break;
                };
                let frames_until_exit = phase + rank as u64 * interval_frames;
                let x_fp = w as i64 * PARADE_SUBPIXEL_ONE - frames_until_exit as i64 * velocity_fp;
                let x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
                let y = self
                    .random_tile_y(h, x, card.scaled.w, card.scaled.h, speed, tile_idx)
                    .unwrap_or(-(card.scaled.h as isize * 2 / 3));
                let active =
                    self.placement_is_clear(x, y, card.scaled.w, card.scaled.h, speed, tile_idx);
                self.tiles.push(ParadeTile {
                    x_fp,
                    y,
                    layer: speed,
                    speed,
                    velocity_fp,
                    velocity_remainder: 0,
                    image_idx: card.image_idx,
                    scaled: card.scaled,
                    phase_set: card.phase_set,
                    corner_insets: card.corner_insets,
                    active,
                    raster_held_this_frame: false,
                    raster_moved_this_frame: false,
                    next: None,
                    pending_image_idx: None,
                });
            }
        }
        if self.tiles.is_empty() {
            return false;
        }
        for tile_idx in 0..self.tiles.len() {
            self.queue_successor(tile_idx, None);
        }
        true
    }

    #[cfg(test)]
    fn begin_archive_streaming(
        &mut self,
        asset_keys: Vec<String>,
        screen_w: usize,
        screen_h: usize,
        startup_started_at: Option<Instant>,
    ) {
        self.set_geometry(screen_w, screen_h);
        self.startup_started_at = startup_started_at;
        self.first_card_ready_logged = false;
        self.tiles.clear();
        self.asset_keys = asset_keys;
        self.deck = (0..self.asset_keys.len()).collect();
        shuffle(&mut self.deck, &mut self.rng);
        self.cursor = 0;
        self.image_count = self.asset_keys.len();
        self.failed_images.clear();
        if self.image_count == 0 {
            return;
        }
        for layer_idx in (0..PARADE_SPEED_COUNT).rev() {
            let speed = PARADE_MIN_TILE_SPEED + layer_idx;
            let velocity_fp = self.motion.card_velocity_fp(layer_idx, screen_w);
            let (tile_w, _, _) = parade_depth_style(speed, screen_h);
            let interval_frames = parade_layer_interval_frames(
                screen_w,
                tile_w,
                velocity_fp,
                self.layer_targets[layer_idx],
            );
            self.layers[layer_idx] = ParadeLayerSchedule {
                next_spawn_frame: layer_idx as u64 * 12,
                interval_frames,
                spawn_count: 0,
                active_sum: 0,
                sample_count: 0,
            };
            let tile_idx = self.push_empty_streaming_tile(layer_idx);
            self.queue_successor(tile_idx, None);
        }
    }

    fn push_empty_streaming_tile(&mut self, layer_idx: usize) -> usize {
        let speed = PARADE_MIN_TILE_SPEED + layer_idx;
        let tile_idx = self.tiles.len();
        let scaled = SaverImage {
            pixels: Vec::new(),
            w: 0,
            h: 0,
            stride: 0,
        };
        let phase_set = ParadePhaseSet::prepare(&scaled, self.sampling_profile.for_layer(speed));
        self.tiles.push(ParadeTile {
            x_fp: 0,
            y: 0,
            layer: speed,
            speed,
            velocity_fp: self.motion.card_velocity_fp(layer_idx, self.screen_w),
            velocity_remainder: 0,
            image_idx: usize::MAX,
            scaled,
            phase_set,
            corner_insets: Vec::new(),
            active: false,
            raster_held_this_frame: false,
            raster_moved_this_frame: false,
            next: None,
            pending_image_idx: None,
        });
        tile_idx
    }

    fn prepare_archive_card(
        &mut self,
        tile_idx: usize,
        speed: usize,
        cancelled: &AtomicBool,
    ) -> Option<PreparedParadeCard> {
        for _ in 0..self.image_count {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            let image_idx = self.next_image_for(tile_idx)?;
            if self
                .scale_tx
                .send(ParadeScaleJob {
                    tile_idx,
                    image_idx,
                    speed,
                    sampling_profile: self.sampling_profile.for_layer(speed),
                    screen_h: self.screen_h,
                    source: ParadeScaleSource::ArchiveIndex(image_idx),
                })
                .is_err()
            {
                self.scale_worker_connected = false;
                return None;
            }
            self.scale_queue_depth += 1;
            self.scale_queue_max = self.scale_queue_max.max(self.scale_queue_depth);
            let result = self.scale_rx.recv().ok()?;
            self.scale_queue_depth = self.scale_queue_depth.saturating_sub(1);
            match result.card {
                Ok(card) => {
                    self.record_prepared_card(&card);
                    return Some(card);
                }
                Err(error) => {
                    self.decode_failures += 1;
                    self.failed_images.insert(result.image_idx);
                    crate::ui_errln!(
                        "screensaver_decode_failed key={} error={}",
                        self.asset_keys[result.image_idx],
                        error
                    );
                }
            }
        }
        None
    }

    fn ensure_initialized(&mut self, images: &[SaverImage], w: usize, h: usize) {
        let _ = self.ensure_initialized_cancellable(images, w, h, None);
    }

    fn ensure_initialized_cancellable(
        &mut self,
        images: &[SaverImage],
        w: usize,
        h: usize,
        cancelled: Option<&AtomicBool>,
    ) -> bool {
        let image_count = images.len();
        if self.image_count == image_count
            && self.screen_w == w
            && self.screen_h == h
            && !self.tiles.is_empty()
        {
            return true;
        }
        self.set_geometry(w, h);
        self.tiles.clear();
        self.deck = (0..image_count).collect();
        shuffle(&mut self.deck, &mut self.rng);
        self.cursor = 0;
        self.image_count = image_count;
        for (layer_idx, target) in self.layer_targets.into_iter().enumerate() {
            let speed = PARADE_MIN_TILE_SPEED + layer_idx;
            let velocity_fp = self.motion.card_velocity_fp(layer_idx, w);
            let (tile_w, _, _) = parade_depth_style(speed, h);
            let interval_frames = parade_layer_interval_frames(w, tile_w, velocity_fp, target);
            let phase = self.random_below(interval_frames as usize) as u64;
            self.layers[layer_idx] = ParadeLayerSchedule {
                next_spawn_frame: phase,
                interval_frames,
                spawn_count: 0,
                active_sum: 0,
                sample_count: 0,
            };
            for rank in 0..target {
                if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
                    return false;
                }
                if self.tiles.len() >= image_count {
                    break;
                }
                let tile_idx = self.tiles.len();
                let Some(image_idx) = self.next_image_for(tile_idx) else {
                    break;
                };
                let (scaled, corner_insets) = self.scale_image(&images[image_idx], speed);
                let phase_set = self.prepare_phase_set(&scaled, speed);
                let frames_until_exit = phase + rank as u64 * interval_frames;
                let x_fp = w as i64 * PARADE_SUBPIXEL_ONE - frames_until_exit as i64 * velocity_fp;
                let x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
                let y = self
                    .random_tile_y(h, x, scaled.w, scaled.h, speed, tile_idx)
                    .unwrap_or(-(scaled.h as isize * 2 / 3));
                let active = self.placement_is_clear(x, y, scaled.w, scaled.h, speed, tile_idx);
                self.tiles.push(ParadeTile {
                    x_fp,
                    y,
                    layer: speed,
                    speed,
                    velocity_fp,
                    velocity_remainder: 0,
                    image_idx,
                    scaled,
                    phase_set,
                    corner_insets,
                    active,
                    raster_held_this_frame: false,
                    raster_moved_this_frame: false,
                    next: None,
                    pending_image_idx: None,
                });
            }
        }
        // Initial cards are ready before the first frame. Successors are then
        // prepared off-thread instead of doubling synchronous startup work.
        for tile_idx in 0..self.tiles.len() {
            self.queue_successor(tile_idx, Some(images));
        }
        true
    }

    fn next_image_for(&mut self, replacing_tile: usize) -> Option<usize> {
        if self.deck.is_empty() {
            return None;
        }
        for _ in 0..self.deck.len() {
            if self.cursor == self.deck.len() {
                shuffle(&mut self.deck, &mut self.rng);
                self.cursor = 0;
            }
            let candidate = self.deck[self.cursor];
            self.cursor += 1;
            if self.failed_images.contains(&candidate) {
                continue;
            }
            let already_visible = self.tiles.iter().enumerate().any(|(idx, tile)| {
                (idx != replacing_tile && tile.active && tile.image_idx == candidate)
                    || tile
                        .next
                        .as_ref()
                        .is_some_and(|next| next.image_idx == candidate)
                    || tile.pending_image_idx == Some(candidate)
            });
            if !already_visible {
                return Some(candidate);
            }
        }
        None
    }

    fn advance(
        &mut self,
        screen_w: usize,
        screen_h: usize,
        images: Option<&[SaverImage]>,
        motion_ticks_fp: u64,
        tick_delta_fp: i64,
    ) -> ParadeAdvanceTrace {
        let nominal_frame = motion_ticks_fp / PARADE_TICK_ONE as u64;
        let adopt_start = Instant::now();
        let cards_adopted = self.collect_scaled_cards(images);
        let card_adopt_us = adopt_start.elapsed().as_micros();
        let advance_start = Instant::now();
        let mut exited = Vec::new();
        let sampling_profile = self.sampling_profile;
        for tile_idx in 0..self.tiles.len() {
            if self.tiles[tile_idx].active {
                let tile = &mut self.tiles[tile_idx];
                let tile_sampling_profile = sampling_profile.for_layer(tile.layer);
                tile.raster_held_this_frame = false;
                tile.raster_moved_this_frame = false;
                let previous_x_fp = tile.x_fp;
                let previous_raster_phase =
                    parade_raster_phase_key(tile_sampling_profile, tile.x_fp);
                let motion = tile
                    .velocity_fp
                    .saturating_mul(tick_delta_fp)
                    .saturating_add(tile.velocity_remainder);
                tile.x_fp = tile.x_fp.saturating_add(motion / PARADE_TICK_ONE);
                tile.velocity_remainder = motion % PARADE_TICK_ONE;
                if tile.x_fp != previous_x_fp {
                    let raster_phase = parade_raster_phase_key(tile_sampling_profile, tile.x_fp);
                    if raster_phase == previous_raster_phase {
                        tile.raster_held_this_frame = true;
                    } else {
                        tile.raster_moved_this_frame = true;
                    }
                }
                if self.tiles[tile_idx].x() >= screen_w as isize {
                    self.tiles[tile_idx].active = false;
                    exited.push(tile_idx);
                }
            }
        }
        for tile_idx in exited {
            self.queue_successor(tile_idx, images);
        }
        for layer_idx in 0..PARADE_SPEED_COUNT {
            if nominal_frame < self.layers[layer_idx].next_spawn_frame {
                continue;
            }
            let speed = PARADE_MIN_TILE_SPEED + layer_idx;
            let Some(tile_idx) = self
                .tiles
                .iter()
                .position(|tile| tile.layer == speed && !tile.active && tile.next.is_some())
            else {
                continue;
            };
            let next = self.tiles[tile_idx].next.take().expect("checked above");
            // Decodes arrive asynchronously during startup. Always stage the
            // card completely offscreen so readiness can never look like a
            // half-card pop as each layer receives its first result.
            let x = -(next.scaled.w as isize);
            let Some(y) =
                self.random_tile_y(screen_h, x, next.scaled.w, next.scaled.h, speed, tile_idx)
            else {
                self.tiles[tile_idx].next = Some(next);
                self.layers[layer_idx].next_spawn_frame = nominal_frame + 1;
                continue;
            };
            let tile = &mut self.tiles[tile_idx];
            tile.x_fp = x as i64 * PARADE_SUBPIXEL_ONE;
            tile.y = y;
            tile.image_idx = next.image_idx;
            tile.scaled = next.scaled;
            tile.phase_set = next.phase_set;
            tile.corner_insets = next.corner_insets;
            tile.active = true;
            tile.velocity_remainder = 0;
            let interval = self.jittered_interval(self.layers[layer_idx].interval_frames);
            self.layers[layer_idx].next_spawn_frame = nominal_frame + interval;
            self.layers[layer_idx].spawn_count += 1;
            if self.archive_backed {
                let layer_tile_count = self.tiles.iter().filter(|tile| tile.layer == speed).count();
                let has_waiting_tile = self.tiles.iter().any(|tile| {
                    tile.layer == speed
                        && !tile.active
                        && (tile.next.is_some() || tile.pending_image_idx.is_some())
                });
                if layer_tile_count < self.layer_targets[layer_idx] && !has_waiting_tile {
                    let next_tile_idx = self.push_empty_streaming_tile(layer_idx);
                    self.queue_successor(next_tile_idx, images);
                }
            } else {
                self.queue_successor(tile_idx, images);
            }
        }
        for layer_idx in 0..PARADE_SPEED_COUNT {
            let speed = PARADE_MIN_TILE_SPEED + layer_idx;
            let active = self
                .tiles
                .iter()
                .filter(|tile| tile.active && tile.layer == speed)
                .count() as u64;
            self.layers[layer_idx].active_sum += active;
            self.layers[layer_idx].sample_count += 1;
        }
        ParadeAdvanceTrace {
            card_adopt_us,
            cards_adopted,
            parade_advance_us: advance_start.elapsed().as_micros(),
        }
    }

    fn queue_successor(&mut self, tile_idx: usize, images: Option<&[SaverImage]>) {
        if self.tiles[tile_idx].next.is_some() || self.tiles[tile_idx].pending_image_idx.is_some() {
            return;
        }
        let Some(image_idx) = self.next_image_for(tile_idx) else {
            return;
        };
        self.queue_scale(tile_idx, image_idx, images);
    }

    fn queue_scale(&mut self, tile_idx: usize, image_idx: usize, images: Option<&[SaverImage]>) {
        let speed = self.tiles[tile_idx].speed;
        if !self.scale_worker_connected {
            if let Some(images) = images {
                let (scaled, corner_insets) = self.scale_image(&images[image_idx], speed);
                let phase_set = self.prepare_phase_set(&scaled, speed);
                let card = PreparedParadeCard {
                    image_idx,
                    speed,
                    scaled,
                    phase_set,
                    corner_insets,
                    scale_us: 0,
                    phase_us: 0,
                };
                self.tiles[tile_idx].next = Some(card);
            }
            return;
        }
        let source = if let Some(images) = images {
            ParadeScaleSource::Decoded(images[image_idx].clone())
        } else {
            if image_idx >= self.asset_keys.len() {
                return;
            }
            ParadeScaleSource::ArchiveIndex(image_idx)
        };
        self.tiles[tile_idx].pending_image_idx = Some(image_idx);
        if self
            .scale_tx
            .send(ParadeScaleJob {
                tile_idx,
                image_idx,
                speed,
                sampling_profile: self.sampling_profile.for_layer(speed),
                screen_h: self.screen_h,
                source,
            })
            .is_err()
        {
            self.scale_worker_connected = false;
            self.tiles[tile_idx].pending_image_idx = None;
            if images.is_some() {
                self.queue_successor(tile_idx, images);
            }
        } else {
            self.scale_queue_depth += 1;
            self.scale_queue_max = self.scale_queue_max.max(self.scale_queue_depth);
        }
    }

    fn collect_scaled_cards(&mut self, images: Option<&[SaverImage]>) -> usize {
        self.collect_scaled_cards_with_limit(images, PARADE_MAX_CARD_ADOPTIONS_PER_FRAME)
    }

    fn collect_scaled_cards_with_limit(
        &mut self,
        images: Option<&[SaverImage]>,
        limit: usize,
    ) -> usize {
        let mut failed_tiles = Vec::new();
        let mut collected = 0;
        while collected < limit {
            match self.scale_rx.try_recv() {
                Ok(result) => {
                    collected += 1;
                    self.scale_queue_depth = self.scale_queue_depth.saturating_sub(1);
                    match result.card {
                        Ok(card) => {
                            self.record_prepared_card(&card);
                            if !self.first_card_ready_logged {
                                self.first_card_ready_logged = true;
                                if let Some(started) = self.startup_started_at {
                                    crate::ui_logln!(
                                        "screensaver_startup_timing milestone=first_card_ready elapsed_us={} layer={}",
                                        started.elapsed().as_micros(),
                                        card.speed
                                    );
                                }
                            }
                            if let Some(tile) = self.tiles.get_mut(result.tile_idx) {
                                tile.pending_image_idx = None;
                                tile.next = Some(card);
                            }
                        }
                        Err(error) => {
                            self.decode_failures += 1;
                            self.failed_images.insert(result.image_idx);
                            if let Some(tile) = self.tiles.get_mut(result.tile_idx) {
                                tile.pending_image_idx = None;
                            }
                            if let Some(key) = self.asset_keys.get(result.image_idx) {
                                crate::ui_errln!(
                                    "screensaver_decode_failed key={} error={}",
                                    key,
                                    error
                                );
                            }
                            failed_tiles.push(result.tile_idx);
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.scale_worker_connected = false;
                    break;
                }
            }
        }
        if !self.scale_worker_connected && !self.archive_backed {
            let stranded = self
                .tiles
                .iter()
                .enumerate()
                .filter_map(|(tile_idx, tile)| {
                    tile.pending_image_idx
                        .map(|image_idx| (tile_idx, image_idx, tile.speed))
                })
                .collect::<Vec<_>>();
            self.scale_queue_depth = 0;
            for (tile_idx, image_idx, speed) in stranded {
                let Some(images) = images else {
                    continue;
                };
                let (scaled, corner_insets) = self.scale_image(&images[image_idx], speed);
                let phase_set = self.prepare_phase_set(&scaled, speed);
                self.tiles[tile_idx].pending_image_idx = None;
                let card = PreparedParadeCard {
                    image_idx,
                    speed,
                    scaled,
                    phase_set,
                    corner_insets,
                    scale_us: 0,
                    phase_us: 0,
                };
                self.tiles[tile_idx].next = Some(card);
            }
        } else if !self.scale_worker_connected {
            self.scale_queue_depth = 0;
            for tile in &mut self.tiles {
                tile.pending_image_idx = None;
            }
        }
        for tile_idx in failed_tiles {
            self.queue_successor(tile_idx, images);
        }
        collected
    }

    fn record_prepared_card(&mut self, card: &PreparedParadeCard) {
        self.scale_count += 1;
        self.scale_total_us += card.scale_us;
        self.scale_max_us = self.scale_max_us.max(card.scale_us);
        self.phase_count += 1;
        self.phase_total_us += card.phase_us;
        self.phase_max_us = self.phase_max_us.max(card.phase_us);
        if self.archive_backed {
            self.decode_successes += 1;
            self.unique_decoded.insert(card.image_idx);
        }
    }

    fn scale_image(&mut self, image: &SaverImage, speed: usize) -> (SaverImage, Vec<u8>) {
        let started = Instant::now();
        let prepared = prepare_parade_scaled(image, speed, self.screen_h);
        let elapsed_us = started.elapsed().as_micros();
        self.scale_count += 1;
        self.scale_total_us += elapsed_us;
        self.scale_max_us = self.scale_max_us.max(elapsed_us);
        prepared
    }

    fn prepare_phase_set(&mut self, image: &SaverImage, layer: usize) -> ParadePhaseSet {
        let started = Instant::now();
        let phases = ParadePhaseSet::prepare(image, self.sampling_profile.for_layer(layer));
        let elapsed_us = started.elapsed().as_micros();
        self.phase_count += 1;
        self.phase_total_us += elapsed_us;
        self.phase_max_us = self.phase_max_us.max(elapsed_us);
        phases
    }

    fn jittered_interval(&mut self, base: u64) -> u64 {
        let variance = (base / 8).max(1);
        let offset = self.random_below((variance * 2 + 1) as usize) as i64 - variance as i64;
        (base as i64 + offset).max(1) as u64
    }

    fn set_geometry(&mut self, screen_w: usize, screen_h: usize) {
        self.screen_w = screen_w;
        self.screen_h = screen_h;
        self.layer_targets = parade_layer_targets(screen_w, screen_h);
    }

    fn random_tile_y(
        &mut self,
        screen_h: usize,
        x: isize,
        tile_w: usize,
        tile_h: usize,
        speed: usize,
        replacing_tile: usize,
    ) -> Option<isize> {
        let min_y = -(tile_h as isize * 2 / 3);
        let max_y = screen_h as isize - tile_h as isize / 3;
        let span = (max_y - min_y + 1).max(1) as usize;
        for _ in 0..64 {
            let y = min_y + self.random_below(span) as isize;
            if self.placement_is_clear(x, y, tile_w, tile_h, speed, replacing_tile) {
                return Some(y);
            }
        }
        (min_y..=max_y)
            .find(|y| self.placement_is_clear(x, *y, tile_w, tile_h, speed, replacing_tile))
    }

    fn placement_is_clear(
        &self,
        x: isize,
        y: isize,
        tile_w: usize,
        tile_h: usize,
        speed: usize,
        replacing_tile: usize,
    ) -> bool {
        let placement_gap =
            scale_parade_dimension(PARADE_REFERENCE_PLACEMENT_GAP, self.screen_h) as isize;
        self.tiles.iter().enumerate().all(|(idx, tile)| {
            if idx == replacing_tile || !tile.active || tile.layer != speed {
                return true;
            }
            x + tile_w as isize + placement_gap <= tile.x()
                || tile.x() + tile.scaled.w as isize + placement_gap <= x
                || y + tile_h as isize + placement_gap <= tile.y
                || tile.y + tile.scaled.h as isize + placement_gap <= y
        })
    }

    fn log_scaler_stats(&self) {
        let average_us = self.scale_total_us / self.scale_count.max(1) as u128;
        let phase_average_us = self.phase_total_us / self.phase_count.max(1) as u128;
        let phase_cache_bytes = self.phase_bank_resident_bytes();
        let image_cache_bytes = self
            .tiles
            .iter()
            .map(|tile| {
                tile.scaled.pixels.len() * std::mem::size_of::<Rgb565Pixel>()
                    + tile.next.as_ref().map_or(0, |next| {
                        next.scaled.pixels.len() * std::mem::size_of::<Rgb565Pixel>()
                    })
            })
            .sum::<usize>();
        crate::ui_logln!(
            "screensaver_lanczos sampling={} scales={} total_us={} average_us={} max_us={} phase_prepares={} phase_total_us={} phase_average_us={} phase_max_us={} queue_max={} queue_bound={} worker_connected={} phase_cache_bytes={}",
            self.sampling_profile.label(),
            self.scale_count,
            self.scale_total_us,
            average_us,
            self.scale_max_us,
            self.phase_count,
            self.phase_total_us,
            phase_average_us,
            self.phase_max_us,
            self.scale_queue_max,
            self.layer_targets.iter().sum::<usize>(),
            self.scale_worker_connected,
            phase_cache_bytes + image_cache_bytes
        );
        if self.archive_backed {
            crate::ui_logln!(
                "screensaver_archive_runtime entries={} decodes={} failures={} unique_keys={} queue_depth={} queue_max={}",
                self.asset_keys.len(),
                self.decode_successes,
                self.decode_failures,
                self.unique_decoded.len(),
                self.scale_queue_depth,
                self.scale_queue_max
            );
        }
        for (layer_idx, layer) in self.layers.iter().enumerate() {
            let speed = PARADE_MIN_TILE_SPEED + layer_idx;
            let (w, h, _) = parade_depth_style(speed, self.screen_h);
            let average_active = layer.active_sum as f64 / layer.sample_count.max(1) as f64;
            crate::ui_logln!(
                "screensaver_parade_layer motion={} layer={} velocity_px={:.2} size={}x{} target={} interval_ms={} spawns={} average_active={:.2}",
                self.motion.label(),
                speed,
                self.motion.card_velocity_fp(layer_idx, self.screen_w) as f64
                    / PARADE_SUBPIXEL_ONE as f64,
                w,
                h,
                self.layer_targets[layer_idx],
                layer.interval_frames * 1_000 / 60,
                layer.spawn_count,
                average_active
            );
        }
    }

    fn phase_bank_resident_bytes(&self) -> usize {
        self.tiles
            .iter()
            .map(|tile| {
                tile.phase_set.resident_bytes()
                    + tile
                        .next
                        .as_ref()
                        .map_or(0, |next| next.phase_set.resident_bytes())
            })
            .sum()
    }

    fn random_below(&mut self, upper: usize) -> usize {
        advance_rng(&mut self.rng) as usize % upper.max(1)
    }
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

fn shuffle<T>(values: &mut [T], rng: &mut u64) {
    for i in (1..values.len()).rev() {
        let j = (advance_rng(rng) as usize) % (i + 1);
        values.swap(i, j);
    }
}

fn advance_rng(rng: &mut u64) -> u64 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 7;
    *rng ^= *rng << 17;
    *rng
}

fn prepare_parade_draw_order(state: &mut ParadeState) {
    state.draw_order.clear();
    for layer_idx in 0..PARADE_SPEED_COUNT {
        let speed = PARADE_MIN_TILE_SPEED + layer_idx;
        for (tile_idx, tile) in state.tiles.iter().enumerate() {
            if tile.active && tile.layer == speed {
                state.draw_order.push(tile_idx);
            }
        }
    }
}

fn clipped_parade_rect(
    x: isize,
    y: isize,
    width: usize,
    height: usize,
    screen_w: usize,
    screen_h: usize,
) -> Option<DirtyRect> {
    let x0 = x.clamp(0, screen_w as isize) as usize;
    let y0 = y.clamp(0, screen_h as isize) as usize;
    let x1 = x.saturating_add(width as isize).clamp(0, screen_w as isize) as usize;
    let y1 = y
        .saturating_add(height as isize)
        .clamp(0, screen_h as isize) as usize;
    (x1 > x0 && y1 > y0).then_some(DirtyRect { x0, y0, x1, y1 })
}

fn parade_tile_origin_and_fractional_width(
    sampling_profile: ParadeSamplingProfile,
    x_fp: i64,
    width: usize,
) -> (isize, usize, bool) {
    match sampling_profile {
        ParadeSamplingProfile::LegacyHalf => {
            let x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
            match x_fp.rem_euclid(PARADE_SUBPIXEL_ONE) {
                0 => (x, width, false),
                128 => (x, width.saturating_add(1), true),
                fraction if fraction < 128 => (x, width, false),
                _ => (x.saturating_add(1), width, false),
            }
        }
        ParadeSamplingProfile::CrtSixteenth => {
            let quantized = quantize_crt_phase(x_fp);
            (
                quantized.x,
                width.saturating_add(usize::from(quantized.phase != 0)),
                quantized.phase != 0,
            )
        }
    }
}

fn parade_raster_phase_key(sampling_profile: ParadeSamplingProfile, x_fp: i64) -> i64 {
    let x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE);
    let fraction = x_fp.rem_euclid(PARADE_SUBPIXEL_ONE);
    match sampling_profile {
        ParadeSamplingProfile::LegacyHalf => match fraction {
            0 => x * CRT_PHASE_COUNT as i64,
            128 => x * CRT_PHASE_COUNT as i64 + CRT_PHASE_COUNT as i64 / 2,
            fraction if fraction < 128 => x * CRT_PHASE_COUNT as i64,
            _ => (x + 1) * CRT_PHASE_COUNT as i64,
        },
        ParadeSamplingProfile::CrtSixteenth => {
            let phase = quantize_crt_phase(x_fp);
            phase.x as i64 * CRT_PHASE_COUNT as i64 + phase.phase as i64
        }
    }
}

fn parade_tile_draw_bounds(
    tile: &ParadeTile,
    sampling_profile: ParadeSamplingProfile,
    screen_w: usize,
    screen_h: usize,
) -> Option<DirtyRect> {
    let (x, width, _) =
        parade_tile_origin_and_fractional_width(sampling_profile, tile.x_fp, tile.scaled.w);
    clipped_parade_rect(x, tile.y, width, tile.scaled.h, screen_w, screen_h)
}

fn parade_tile_opaque_bounds(
    tile: &ParadeTile,
    sampling_profile: ParadeSamplingProfile,
    screen_w: usize,
    screen_h: usize,
) -> Option<DirtyRect> {
    let (x, _, fractional) =
        parade_tile_origin_and_fractional_width(sampling_profile, tile.x_fp, tile.scaled.w);
    let inset = tile.corner_insets.iter().copied().max().unwrap_or_default() as usize
        + usize::from(fractional);
    let width = tile.scaled.w.saturating_sub(inset.saturating_mul(2));
    clipped_parade_rect(
        x.saturating_add(inset as isize),
        tile.y,
        width,
        tile.scaled.h,
        screen_w,
        screen_h,
    )
}

fn prepare_parade_visible_draw_order(
    state: &mut ParadeState,
    screen_w: usize,
    screen_h: usize,
) -> usize {
    state.visible_draw_order.clear();
    state.depth_coverage.clear();
    let mut culled = 0;
    for &tile_idx in state.draw_order.iter().rev() {
        let tile = &state.tiles[tile_idx];
        let tile_sampling_profile = state.sampling_profile.for_layer(tile.layer);
        let Some(draw_bounds) =
            parade_tile_draw_bounds(tile, tile_sampling_profile, screen_w, screen_h)
        else {
            continue;
        };
        if state
            .depth_coverage
            .iter()
            .any(|coverage| coverage.contains(draw_bounds))
        {
            culled += 1;
            continue;
        }
        state.visible_draw_order.push(tile_idx);
        if let Some(opaque_bounds) =
            parade_tile_opaque_bounds(tile, tile_sampling_profile, screen_w, screen_h)
        {
            state.depth_coverage.push(opaque_bounds);
        }
    }
    state.visible_draw_order.reverse();
    culled
}

fn blit_parade_tile(
    dst: &mut [Rgb565Pixel],
    w: usize,
    h: usize,
    sampling_profile: ParadeSamplingProfile,
    tile: &ParadeTile,
) {
    match sampling_profile {
        ParadeSamplingProfile::LegacyHalf => blit_scaled_subpixel_x(
            dst,
            w,
            h,
            &tile.scaled,
            tile.phase_set.legacy_half(),
            &tile.corner_insets,
            tile.x_fp,
            tile.y,
        ),
        ParadeSamplingProfile::CrtSixteenth => blit_scaled_crt_sixteenth_x(
            dst,
            w,
            h,
            &tile.scaled,
            &tile.phase_set,
            &tile.corner_insets,
            tile.x_fp,
            tile.y,
        ),
    }
}

fn render_parade(
    dst: &mut [Rgb565Pixel],
    state: &mut ParadeState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let motion_ticks_fp = frame.saturating_mul(PARADE_TICK_ONE as u64);
    render_parade_background(dst, w, h, motion_ticks_fp, state.motion);
    state.ensure_initialized(images, w, h);
    let _ = state.advance(w, h, Some(images), motion_ticks_fp, PARADE_TICK_ONE);
    prepare_parade_draw_order(state);
    prepare_parade_visible_draw_order(state, w, h);
    for &tile_idx in &state.visible_draw_order {
        let tile = &state.tiles[tile_idx];
        blit_parade_tile(
            dst,
            w,
            h,
            state.sampling_profile.for_layer(tile.layer),
            tile,
        );
    }
}

#[cfg(test)]
fn render_archive_parade(
    dst: &mut [Rgb565Pixel],
    state: &mut ParadeState,
    w: usize,
    h: usize,
    motion_ticks_fp: u64,
    tick_delta_fp: i64,
) -> ScreensaverFrameTrace {
    let background_start = Instant::now();
    render_parade_background(dst, w, h, motion_ticks_fp, state.motion);
    let background_us = background_start.elapsed().as_micros();
    let advance = state.advance(w, h, None, motion_ticks_fp, tick_delta_fp);
    let draw_order_start = Instant::now();
    prepare_parade_draw_order(state);
    let cards_culled = prepare_parade_visible_draw_order(state, w, h);
    let draw_order_us = draw_order_start.elapsed().as_micros();
    let mut raster_held_cards = 0;
    let mut raster_moved_cards = 0;
    let mut raster_hold_layer_mask = 0_u8;
    let mut raster_visible_layer_mask = 0_u8;
    for &tile_idx in &state.visible_draw_order {
        let tile = &state.tiles[tile_idx];
        let layer_idx = tile.layer.saturating_sub(PARADE_MIN_TILE_SPEED);
        if layer_idx < u8::BITS as usize {
            raster_visible_layer_mask |= 1_u8 << layer_idx;
            if tile.raster_held_this_frame {
                raster_hold_layer_mask |= 1_u8 << layer_idx;
            }
        }
        raster_held_cards += usize::from(tile.raster_held_this_frame);
        raster_moved_cards += usize::from(tile.raster_moved_this_frame);
    }
    let tile_blit_start = Instant::now();
    for &tile_idx in &state.visible_draw_order {
        let tile = &state.tiles[tile_idx];
        blit_parade_tile(
            dst,
            w,
            h,
            state.sampling_profile.for_layer(tile.layer),
            tile,
        );
    }
    ScreensaverFrameTrace {
        card_adopt_us: advance.card_adopt_us,
        cards_adopted: advance.cards_adopted,
        parade_advance_us: advance.parade_advance_us,
        background_us,
        draw_order_us,
        tile_blit_us: tile_blit_start.elapsed().as_micros(),
        cards_drawn: state.visible_draw_order.len(),
        cards_culled,
        sampling_profile: state.sampling_profile.layer_evidence(),
        raster_held_cards,
        raster_moved_cards,
        raster_hold_layer_mask,
        raster_visible_layer_mask,
        sixteenth_phase_layer_mask: state.sampling_profile.sixteenth_layer_mask(),
        phase_bank_resident_bytes: state.phase_bank_resident_bytes(),
        ..ScreensaverFrameTrace::default()
    }
}

fn render_marquee(dst: &mut [Rgb565Pixel], w: usize, h: usize, images: &[SaverImage], frame: u64) {
    fill_rect(dst, w, h, 0, 0, w, h, color565(2, 4, 16));
    for i in 0..8 {
        if let Some(img) = image_at(images, i + frame as usize / 100) {
            blit_scaled(
                dst,
                w,
                h,
                img,
                (i * 132) as isize - (frame as isize % 132),
                52,
                124,
                92,
                255,
            );
            blit_scaled(
                dst,
                w,
                h,
                img,
                ((7 - i) * 132) as isize + (frame as isize % 132) - 80,
                394,
                124,
                92,
                210,
            );
        }
    }
    if let Some(img) = image_at(images, frame as usize / 180) {
        blit_scaled(dst, w, h, img, 280, 150, 400, 260, 245);
        stroke_rect(dst, w, h, 276, 146, 408, 268, color565(255, 60, 180));
    }
}

fn render_random_loader(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let tiles_x = 15;
    let tiles_y = 9;
    let page = frame as usize / 180;
    if !state.random_loader_valid
        || state.random_loader_page != page
        || state.random_loader.len() != dst.len()
    {
        state.random_loader.resize(dst.len(), Rgb565Pixel(0));
        clear(&mut state.random_loader, color565(0, 18, 30));
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile_idx = tx + ty * tiles_x;
                if let Some(img) = image_at(images, page + tile_idx) {
                    blit_scaled(
                        &mut state.random_loader,
                        w,
                        h,
                        img,
                        (tx * w / tiles_x) as isize,
                        (ty * h / tiles_y) as isize,
                        w / tiles_x,
                        h / tiles_y,
                        240,
                    );
                }
            }
        }
        state.random_loader_page = page;
        state.random_loader_valid = true;
    }

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let idx = tx + ty * tiles_x + frame as usize / 30;
            let color = if hash2_u8(tx + idx, ty) > (frame as u8) {
                color565(0, 18, 30)
            } else {
                color565(20, 180, 210)
            };
            fill_rect(
                dst,
                w,
                h,
                tx * w / tiles_x,
                ty * h / tiles_y,
                w / tiles_x,
                h / tiles_y,
                color,
            );
        }
    }
    let loaded_tiles = ((frame as usize * 3) % (tiles_x * tiles_y)).max(1);
    for tile_idx in 0..loaded_tiles {
        let tx = tile_idx % tiles_x;
        let ty = tile_idx / tiles_x;
        copy_rect(
            dst,
            &state.random_loader,
            w,
            h,
            tx * w / tiles_x,
            ty * h / tiles_y,
            w / tiles_x,
            h / tiles_y,
        );
    }
}

fn render_color_clash(
    dst: &mut [Rgb565Pixel],
    state: &mut ScreensaverRenderState,
    w: usize,
    h: usize,
    images: &[SaverImage],
    frame: u64,
) {
    let cell = 16usize;
    let page = frame as usize / 180;
    for y in (0..h).step_by(cell) {
        for x in (0..w).step_by(cell) {
            if let Some(img) = image_at(images, page + x / cell + y / cell) {
                let sx = (x * img.w / w) % img.w;
                let sy = (y * img.h / h) % img.h;
                let sample = sample_image(img, sx, sy);
                let bright = if ((x / cell + y / cell + frame as usize / 12) & 1) == 0 {
                    color565(255, 230, 80)
                } else {
                    color565(40, 250, 220)
                };
                let dark = color565(10, 12, 30);
                let color = if (sample.0 & 0x0421) != 0 {
                    bright
                } else {
                    dark
                };
                fill_rect(dst, w, h, x, y, cell, cell, color);
            }
        }
    }
    if frame % 240 > 180 {
        let cols = 3usize;
        let rows = 2usize;
        let cell_w = w / cols;
        let cell_h = h / rows;
        let contact_start = (frame / 90) as usize;
        if !state.color_clash_contact_valid
            || state.color_clash_contact_start != contact_start
            || state.color_clash_contact.len() != dst.len()
        {
            state.color_clash_contact.resize(dst.len(), Rgb565Pixel(0));
            clear(&mut state.color_clash_contact, Rgb565Pixel(0));
            for row in 0..rows {
                for col in 0..cols {
                    if let Some(img) = image_at(images, contact_start + row * cols + col) {
                        let x = col * cell_w + 8;
                        let y = row * cell_h + 8;
                        let out_w = cell_w.saturating_sub(16);
                        let out_h = cell_h.saturating_sub(16);
                        blit_scaled(
                            &mut state.color_clash_contact,
                            w,
                            h,
                            img,
                            x as isize,
                            y as isize,
                            out_w,
                            out_h,
                            230,
                        );
                        stroke_rect(
                            &mut state.color_clash_contact,
                            w,
                            h,
                            x,
                            y,
                            out_w,
                            out_h,
                            color565(40, 250, 220),
                        );
                    }
                }
            }
            state.color_clash_contact_start = contact_start;
            state.color_clash_contact_valid = true;
        }
        for row in 0..rows {
            for col in 0..cols {
                copy_rect(
                    dst,
                    &state.color_clash_contact,
                    w,
                    h,
                    col * cell_w + 8,
                    row * cell_h + 8,
                    cell_w.saturating_sub(16),
                    cell_h.saturating_sub(16),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

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
        let mut screensaver =
            LauncherScreensaver::from_archive_path(&path, 320, 180, 0x1234, false)
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

    fn write_parade_archive(path: &std::path::Path, count: usize) {
        let names = (0..count)
            .map(|index| format!("fixture-{index:03}.rgb565"))
            .collect::<Vec<_>>();
        let header_len = 8 + 4;
        let entry_len = |name: &str| 2 + 4 + 4 + 4 + 4 + 1 + 4 + 8 + name.len();
        let index_len = header_len + names.iter().map(|name| entry_len(name)).sum::<usize>();
        let mut offset = index_len as u64;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MMPX2B1\0");
        bytes.extend_from_slice(&(count as u32).to_le_bytes());
        for name in &names {
            bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&4_u32.to_le_bytes());
            bytes.extend_from_slice(&4_u32.to_le_bytes());
            bytes.extend_from_slice(&8_u32.to_le_bytes());
            bytes.extend_from_slice(&32_u32.to_le_bytes());
            bytes.push(1);
            bytes.extend_from_slice(&32_u32.to_le_bytes());
            bytes.extend_from_slice(&offset.to_le_bytes());
            bytes.extend_from_slice(name.as_bytes());
            offset += 32;
        }
        for image in 0..count {
            for pixel in 0..16_u16 {
                let value = (image as u16).wrapping_mul(97).wrapping_add(pixel * 31);
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        std::fs::write(path, bytes).expect("write screenshot parade archive fixture");
    }

    #[test]
    fn shared_parade_matches_private_renderer_at_fixed_seeds_and_times() {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-shared-parade-parity-{}.mmlz4b",
            std::process::id()
        ));
        write_parade_archive(&path, 220);
        let seed = 0x4d61_6769_4b54_696c;
        for (width, height, profile) in [
            (960, 540, ParadeSamplingProfile::LegacyHalf),
            (640, 480, ParadeSamplingProfile::CrtSixteenth),
        ] {
            let archive = preview_worker::ResidentPreviewArchive::open(&path).unwrap();
            let keys = archive.asset_keys().to_vec();
            let mut private = ParadeState::new_with_archive(seed, archive, profile);
            assert!(private.ensure_archive_initialized_cancellable(
                keys,
                width,
                height,
                &AtomicBool::new(false)
            ));
            let archive = preview_worker::ResidentPreviewArchive::open(&path).unwrap();
            let geometry = SceneGeometry::new(width, height, width).unwrap();
            let mut shared = ScreenshotParade::new(
                archive,
                ScreenshotParadeConfig {
                    geometry,
                    seed,
                    sampling_profile: shared_sampling_profile(profile),
                    startup: ScreenshotParadeStartup::Prepared,
                    worker_start: None,
                },
            )
            .unwrap();
            let mut private_pixels = vec![Rgb565Pixel(0); width * height];
            let mut shared_pixels = vec![SharedRgb565Pixel(0); width * height];
            let mut previous_ticks = 0_u64;
            for milliseconds in [0_u64, 17, 33, 250, 1_000] {
                let elapsed = Duration::from_millis(milliseconds);
                let ticks = parade_tick_delta_fp(elapsed) as u64;
                let _ = render_archive_parade(
                    &mut private_pixels,
                    &mut private,
                    width,
                    height,
                    ticks,
                    ticks.saturating_sub(previous_ticks) as i64,
                );
                shared.render_at(&mut shared_pixels, elapsed).unwrap();
                assert!(
                    private_pixels
                        .iter()
                        .zip(&shared_pixels)
                        .all(|(private, shared)| private.0 == shared.0),
                    "parade pixels differ at {width}x{height} time={milliseconds}ms"
                );
                previous_ticks = ticks;
            }
        }
        let _ = std::fs::remove_file(path);
    }

    fn test_images(count: usize) -> Vec<SaverImage> {
        (0..count)
            .map(|idx| SaverImage {
                pixels: vec![color565(idx as u8, 120, 220); 4],
                w: 2,
                h: 2,
                stride: 2,
            })
            .collect()
    }

    fn solid_parade_tile(
        color: Rgb565Pixel,
        width: usize,
        height: usize,
        x_fp: i64,
        y: isize,
        layer: usize,
        sampling_profile: ParadeSamplingProfile,
    ) -> ParadeTile {
        let scaled = SaverImage {
            pixels: vec![color; width * height],
            w: width,
            h: height,
            stride: width,
        };
        let phase_set = ParadePhaseSet::prepare(&scaled, sampling_profile);
        let corner_insets = prepare_parade_corner_insets(width, height);
        ParadeTile {
            x_fp,
            y,
            layer,
            speed: layer,
            velocity_fp: PARADE_SUBPIXEL_ONE,
            velocity_remainder: 0,
            image_idx: 0,
            scaled,
            phase_set,
            corner_insets,
            active: true,
            raster_held_this_frame: false,
            raster_moved_this_frame: false,
            next: None,
            pending_image_idx: None,
        }
    }

    fn render_parade_order(
        dst: &mut [Rgb565Pixel],
        state: &ParadeState,
        order: &[usize],
        width: usize,
        height: usize,
    ) {
        for &tile_idx in order {
            blit_parade_tile(
                dst,
                width,
                height,
                state
                    .sampling_profile
                    .for_layer(state.tiles[tile_idx].layer),
                &state.tiles[tile_idx],
            );
        }
    }

    fn collect_initial_successors(
        state: &mut ParadeState,
        images: &[SaverImage],
        minimum_ready: usize,
    ) {
        for _ in 0..5_000 {
            state.collect_scaled_cards(Some(images));
            if state
                .tiles
                .iter()
                .filter(|tile| tile.next.is_some())
                .count()
                >= minimum_ready
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("screensaver scale worker did not prepare initial successors");
    }

    #[test]
    fn archive_streaming_starts_with_one_empty_slot_per_depth_layer() {
        let mut state = ParadeState::new(0xfeed_beef);
        state.archive_backed = true;
        state.begin_archive_streaming(
            (0..256).map(|idx| format!("game-{idx}")).collect(),
            960,
            540,
            None,
        );

        assert_eq!(state.tiles.len(), PARADE_SPEED_COUNT);
        assert_eq!(state.tiles[0].layer, PARADE_SPEED_COUNT);
        assert!(state.tiles.iter().all(|tile| !tile.active));
        assert!(state.tiles.iter().all(|tile| tile.scaled.pixels.is_empty()));
        assert_eq!(
            state
                .tiles
                .iter()
                .filter(|tile| tile.pending_image_idx.is_some())
                .count(),
            PARADE_SPEED_COUNT
        );
        assert_eq!(state.scale_queue_depth, PARADE_SPEED_COUNT);
    }

    #[test]
    fn completed_cards_are_adopted_one_per_render_frame() {
        let mut state = ParadeState::new(0xfeed_beef);
        let (result_tx, result_rx) = mpsc::channel();
        state.scale_rx = result_rx;
        for image_idx in 0..2 {
            let tile_idx = state.push_empty_streaming_tile(image_idx);
            state.tiles[tile_idx].pending_image_idx = Some(image_idx);
            let scaled = SaverImage {
                pixels: vec![Rgb565Pixel(image_idx as u16 + 1); 16],
                w: 4,
                h: 4,
                stride: 4,
            };
            result_tx
                .send(ParadeScaleResult {
                    tile_idx,
                    image_idx,
                    card: Ok(PreparedParadeCard {
                        image_idx,
                        speed: state.tiles[tile_idx].speed,
                        phase_set: ParadePhaseSet::prepare(
                            &scaled,
                            ParadeSamplingProfile::LegacyHalf,
                        ),
                        corner_insets: prepare_parade_corner_insets(scaled.w, scaled.h),
                        scaled,
                        scale_us: 0,
                        phase_us: 0,
                    }),
                })
                .unwrap();
        }
        state.scale_queue_depth = 2;

        assert_eq!(state.collect_scaled_cards(None), 1);
        assert_eq!(state.scale_queue_depth, 1);
        assert_eq!(
            state
                .tiles
                .iter()
                .filter(|tile| tile.next.is_some())
                .count(),
            1
        );
        assert_eq!(state.collect_scaled_cards(None), 1);
        assert_eq!(state.scale_queue_depth, 0);
        assert_eq!(
            state
                .tiles
                .iter()
                .filter(|tile| tile.next.is_some())
                .count(),
            2
        );
    }

    #[test]
    fn first_streamed_card_starts_fully_offscreen() {
        let mut state = ParadeState::new(0x1234);
        state.archive_backed = true;
        state.image_count = 1;
        state.asset_keys.push("game".into());
        state.deck.push(0);
        let layer_idx = PARADE_SPEED_COUNT - 1;
        let tile_idx = state.push_empty_streaming_tile(layer_idx);
        let scaled = SaverImage {
            pixels: vec![Rgb565Pixel(1); 16],
            w: 4,
            h: 4,
            stride: 4,
        };
        state.tiles[tile_idx].next = Some(PreparedParadeCard {
            image_idx: 0,
            speed: PARADE_MIN_TILE_SPEED + layer_idx,
            phase_set: ParadePhaseSet::prepare(&scaled, ParadeSamplingProfile::LegacyHalf),
            corner_insets: prepare_parade_corner_insets(scaled.w, scaled.h),
            scaled,
            scale_us: 0,
            phase_us: 0,
        });
        state.layers[layer_idx].next_spawn_frame = 0;
        state.layers[layer_idx].interval_frames = 30;

        state.advance(960, 540, None, 0, 0);

        assert!(state.tiles[tile_idx].active);
        assert_eq!(state.tiles[tile_idx].x(), -4);
    }

    #[test]
    fn parade_background_queue_is_bounded_by_one_successor_per_tile() {
        let tile_count = PARADE_WIDE_LAYER_TARGETS.iter().sum::<usize>();
        let images = test_images(tile_count * 2);
        let mut state = ParadeState::new(0xfeed_beef);
        state.ensure_initialized(&images, 960, 540);

        assert!(state.scale_queue_depth <= tile_count);
        assert!(state.scale_queue_max <= tile_count);
        assert!(
            state
                .tiles
                .iter()
                .filter(|tile| tile.pending_image_idx.is_some())
                .count()
                <= tile_count
        );

        collect_initial_successors(&mut state, &images, tile_count);
        assert_eq!(
            state
                .tiles
                .iter()
                .filter(|tile| tile.next.is_some())
                .count(),
            tile_count
        );
        assert_eq!(state.scale_queue_depth, 0);
        assert!(state.scale_queue_max <= tile_count);
    }

    #[test]
    fn parade_keeps_visible_games_unique_and_exhausts_the_pool_before_recycling() {
        let tile_count = PARADE_WIDE_LAYER_TARGETS.iter().sum::<usize>();
        let image_count = tile_count * 2 + 64;
        let images = test_images(image_count);
        let mut state = ParadeState::new(0x1234_5678_9abc_def0);
        state.ensure_initialized(&images, 960, 540);
        collect_initial_successors(&mut state, &images, tile_count);
        assert_eq!(state.tiles.len(), tile_count);

        let mut history = state
            .tiles
            .iter()
            .flat_map(|tile| {
                std::iter::once(tile.image_idx).chain(tile.next.as_ref().map(|next| next.image_idx))
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(history.len(), tile_count * 2);

        for tile_idx in 0..(image_count - tile_count * 2) {
            let slot = tile_idx % tile_count;
            let next = state.next_image_for(slot).expect("unused game remains");
            state.tiles[slot].image_idx = next;
            assert!(history.insert(next), "game recycled before pool exhaustion");
            let visible = state
                .tiles
                .iter()
                .map(|tile| tile.image_idx)
                .collect::<BTreeSet<_>>();
            assert_eq!(visible.len(), state.tiles.len());
        }

        assert_eq!(history.len(), image_count);
    }

    #[test]
    fn parade_tile_identity_changes_only_after_it_leaves_the_screen() {
        let mut state = ParadeState::new(7);
        let images = test_images(PARADE_WIDE_LAYER_TARGETS.iter().sum::<usize>() * 2);
        state.ensure_initialized(&images, 960, 540);
        collect_initial_successors(&mut state, &images, 1);
        let original = state.tiles[0].image_idx;
        let speed = state.tiles[0].speed;
        let velocity_fp = state.tiles[0].velocity_fp;
        let width = state.tiles[0].scaled.w;
        state.tiles[0].x_fp = 960 * PARADE_SUBPIXEL_ONE - velocity_fp - 1;

        state.advance(960, 540, Some(&images), 0, PARADE_TICK_ONE);
        assert_eq!(state.tiles[0].image_idx, original);
        assert_eq!(state.tiles[0].x(), 959);

        let layer_idx = speed - PARADE_MIN_TILE_SPEED;
        state.layers[layer_idx].next_spawn_frame = 0;
        state.advance(
            960,
            540,
            Some(&images),
            PARADE_TICK_ONE as u64,
            PARADE_TICK_ONE,
        );
        assert_eq!(state.tiles[0].x(), -(state.tiles[0].scaled.w as isize));
        assert_ne!(state.tiles[0].image_idx, original);
        assert_ne!(state.tiles[0].scaled.w, 0);
        assert_ne!(width, 0);
    }

    #[test]
    fn parade_starfield_moves_horizontally_in_depth_bands() {
        let width = 960;
        for star in 0..4 {
            let x0 = horizontal_star_x(star, width, 0);
            let x1 = horizontal_star_x(star, width, 16);
            assert_eq!(
                (x1 + width - x0) % width,
                PARADE_MIN_TILE_SPEED * (star + 1)
            );
        }
    }

    #[test]
    fn fastest_star_layer_is_half_the_slowest_card_speed() {
        let width = 960;
        let x0 = horizontal_star_x(3, width, 0);
        let x1 = horizontal_star_x(3, width, 16);
        let star_travel = (x1 + width - x0) % width;
        let slowest_card_travel = PARADE_MIN_TILE_SPEED * 16 / 2;
        assert_eq!(star_travel * 2, slowest_card_travel);
    }

    #[test]
    fn slowest_star_has_a_new_subpixel_phase_every_frame() {
        let (_, fraction0) = horizontal_star_position(0, 960, PARADE_REFERENCE_HEIGHT, 0);
        for frame in 1..16 {
            let (_, fraction) = horizontal_star_position(
                0,
                960,
                PARADE_REFERENCE_HEIGHT,
                frame * PARADE_TICK_ONE as u64,
            );
            assert_ne!(fraction, fraction0);
            assert_eq!(fraction, frame as u8 * 16);
        }
    }

    #[test]
    fn sampling_profile_is_selected_only_by_the_output_route() {
        assert_eq!(
            ParadeSamplingProfile::for_crt_output(false),
            ParadeSamplingProfile::LegacyHalf
        );
        assert_eq!(
            ParadeSamplingProfile::for_crt_output(true),
            ParadeSamplingProfile::CrtSixteenth
        );
        assert_eq!(
            ParadeSamplingProfile::LegacyHalf.for_layer(1),
            ParadeSamplingProfile::CrtSixteenth
        );
        assert_eq!(
            ParadeSamplingProfile::LegacyHalf.for_layer(2),
            ParadeSamplingProfile::LegacyHalf
        );
        assert_eq!(
            ParadeSamplingProfile::CrtSixteenth.for_layer(5),
            ParadeSamplingProfile::CrtSixteenth
        );
        assert_eq!(ParadeSamplingProfile::LegacyHalf.sixteenth_layer_mask(), 1);
        assert_eq!(
            ParadeSamplingProfile::CrtSixteenth.sixteenth_layer_mask(),
            0b1_1111
        );
    }

    #[test]
    fn legacy_half_reports_the_known_720_pixel_wide_slow_layer_holds() {
        let velocity_fp = ParadeMotion::Subpixel.card_velocity_fp(0, 720);
        assert_eq!(velocity_fp, 72);
        let mut x_fp = 0;
        let mut previous = parade_raster_phase_key(ParadeSamplingProfile::LegacyHalf, x_fp);
        let mut held = 0;
        for _ in 0..60 {
            x_fp += velocity_fp;
            let current = parade_raster_phase_key(ParadeSamplingProfile::LegacyHalf, x_fp);
            held += usize::from(current == previous);
            previous = current;
        }
        assert_eq!(held, 41);
    }

    #[test]
    fn sixteenth_phases_move_the_720_pixel_wide_slow_layer_every_frame() {
        let velocity_fp = ParadeMotion::Subpixel.card_velocity_fp(0, 720);
        let profile = ParadeSamplingProfile::LegacyHalf.for_layer(PARADE_MIN_TILE_SPEED);
        let mut x_fp = 0;
        let mut previous = parade_raster_phase_key(profile, x_fp);
        for _ in 0..60 {
            x_fp += velocity_fp;
            let current = parade_raster_phase_key(profile, x_fp);
            assert_ne!(current, previous);
            previous = current;
        }
    }

    #[test]
    fn crt_phase_bank_contains_all_shifted_phases_and_preserves_half_shift() {
        let image = SaverImage {
            pixels: vec![
                color565(255, 0, 0),
                color565(0, 255, 0),
                color565(0, 0, 255),
            ],
            w: 3,
            h: 1,
            stride: 3,
        };
        let expected_half = prepare_half_shifted(&image);
        let phases = ParadePhaseSet::prepare(&image, ParadeSamplingProfile::CrtSixteenth);
        let ParadePhaseSet::CrtSixteenth(phases) = phases else {
            panic!("CRT profile did not create a sixteenth-pixel phase bank");
        };

        assert_eq!(phases.len(), CRT_SHIFTED_PHASE_COUNT);
        assert!(phases.iter().all(|phase| phase.w == image.w + 1
            && phase.h == image.h
            && phase.stride == image.w + 1));
        assert_eq!(phases[CRT_PHASE_COUNT / 2 - 1].pixels, expected_half.pixels);
        assert!(
            phases
                .windows(2)
                .all(|pair| pair[0].pixels != pair[1].pixels)
        );
    }

    #[test]
    fn crt_phase_quantization_rounds_to_nearest_and_carries() {
        for (x_fp, expected) in [
            (0, CrtQuantizedPhase { x: 0, phase: 0 }),
            (7, CrtQuantizedPhase { x: 0, phase: 0 }),
            (8, CrtQuantizedPhase { x: 0, phase: 1 }),
            (247, CrtQuantizedPhase { x: 0, phase: 15 }),
            (248, CrtQuantizedPhase { x: 1, phase: 0 }),
            (-8, CrtQuantizedPhase { x: 0, phase: 0 }),
            (-9, CrtQuantizedPhase { x: -1, phase: 15 }),
        ] {
            assert_eq!(quantize_crt_phase(x_fp), expected, "x_fp={x_fp}");
        }
    }

    #[test]
    fn crt_motion_selects_a_new_raster_phase_on_every_50_and_60_hz_frame() {
        for refresh_hz in [50_u64, 60] {
            for layer_idx in 0..PARADE_SPEED_COUNT {
                let velocity_fp = ParadeMotion::Subpixel.card_velocity_fp(layer_idx, 480);
                let mut x_fp = 0_i64;
                let mut velocity_remainder = 0_i64;
                let mut previous_ticks = 0_u64;
                let mut previous = quantize_crt_phase(x_fp);
                for frame in 1..=refresh_hz {
                    let elapsed_ns = 1_000_000_000_u64 * frame / refresh_hz;
                    let motion_ticks =
                        parade_tick_delta_fp(Duration::from_nanos(elapsed_ns)) as u64;
                    let tick_delta = motion_ticks.saturating_sub(previous_ticks) as i64;
                    let motion = velocity_fp
                        .saturating_mul(tick_delta)
                        .saturating_add(velocity_remainder);
                    x_fp = x_fp.saturating_add(motion / PARADE_TICK_ONE);
                    velocity_remainder = motion % PARADE_TICK_ONE;
                    previous_ticks = motion_ticks;
                    let current = quantize_crt_phase(x_fp);
                    assert_ne!(
                        current, previous,
                        "refresh={refresh_hz} layer={layer_idx} frame={frame}"
                    );
                    previous = current;
                }
            }
        }
    }

    #[test]
    fn legacy_profile_dispatch_is_pixel_identical_to_the_existing_blitter() {
        let scaled = SaverImage {
            pixels: vec![
                color565(255, 0, 0),
                color565(0, 255, 0),
                color565(0, 0, 255),
            ],
            w: 3,
            h: 1,
            stride: 3,
        };
        let phase_set = ParadePhaseSet::prepare(&scaled, ParadeSamplingProfile::LegacyHalf);
        let tile = ParadeTile {
            x_fp: PARADE_SUBPIXEL_ONE + PARADE_SUBPIXEL_ONE / 2,
            y: 1,
            layer: 1,
            speed: 1,
            velocity_fp: PARADE_SUBPIXEL_ONE / 2,
            velocity_remainder: 0,
            image_idx: 0,
            corner_insets: vec![0],
            scaled,
            phase_set,
            active: true,
            raster_held_this_frame: false,
            raster_moved_this_frame: false,
            next: None,
            pending_image_idx: None,
        };
        let background = color565(5, 9, 13);
        let mut expected = vec![background; 8 * 5];
        let mut actual = expected.clone();

        blit_scaled_subpixel_x(
            &mut expected,
            8,
            5,
            &tile.scaled,
            tile.phase_set.legacy_half(),
            &tile.corner_insets,
            tile.x_fp,
            tile.y,
        );
        blit_parade_tile(&mut actual, 8, 5, ParadeSamplingProfile::LegacyHalf, &tile);

        assert_eq!(actual, expected);
    }

    #[test]
    fn crt_fractional_renderer_respects_corner_insets_without_painting_outside_the_card() {
        let white = color565(255, 255, 255);
        let background = color565(180, 180, 180);
        let scaled = SaverImage {
            pixels: vec![
                color565(0, 0, 0),
                color565(255, 0, 0),
                color565(0, 255, 0),
                color565(0, 0, 255),
                white,
                color565(255, 255, 0),
                color565(0, 255, 255),
                color565(255, 0, 255),
                color565(32, 32, 32),
                color565(96, 96, 96),
                color565(160, 160, 160),
                color565(224, 224, 224),
            ],
            w: 4,
            h: 3,
            stride: 4,
        };
        let phases = ParadePhaseSet::prepare(&scaled, ParadeSamplingProfile::CrtSixteenth);
        let mut integer = vec![background; 10 * 7];
        let mut fractional = integer.clone();

        blit_scaled_crt_sixteenth_x(
            &mut integer,
            10,
            7,
            &scaled,
            &phases,
            &[1, 0, 1],
            PARADE_SUBPIXEL_ONE,
            0,
        );
        blit_scaled_crt_sixteenth_x(
            &mut fractional,
            10,
            7,
            &scaled,
            &phases,
            &[1, 0, 1],
            PARADE_SUBPIXEL_ONE + PARADE_SUBPIXEL_ONE / 2,
            0,
        );

        assert_eq!(fractional[1], background);
        assert_ne!(fractional[2], background);
        assert_ne!(fractional[2], white);
        assert_ne!(fractional[2 * 10 + 3], integer[2 * 10 + 3]);
        assert_eq!(fractional[2 * 10 + 5], background);
        assert!(
            fractional[3 * 10..]
                .iter()
                .all(|pixel| *pixel == background)
        );
    }

    #[test]
    fn hdmi_fractional_renderer_does_not_paint_an_offset_shadow() {
        let background = color565(180, 180, 180);
        let image = SaverImage {
            pixels: vec![color565(255, 255, 255); 4 * 3],
            w: 4,
            h: 3,
            stride: 4,
        };
        let half_shifted = prepare_half_shifted(&image);
        let mut dst = vec![background; 10 * 7];

        blit_scaled_subpixel_x(
            &mut dst,
            10,
            7,
            &image,
            &half_shifted,
            &[0; 3],
            PARADE_SUBPIXEL_ONE + PARADE_SUBPIXEL_ONE / 2,
            1,
        );

        assert!(dst[..10].iter().all(|pixel| *pixel == background));
        assert!(dst[4 * 10..].iter().all(|pixel| *pixel == background));
    }

    #[test]
    fn hdmi_integer_renderer_does_not_paint_an_offset_shadow() {
        let background = color565(180, 180, 180);
        let image = SaverImage {
            pixels: vec![color565(255, 255, 255); 4 * 3],
            w: 4,
            h: 3,
            stride: 4,
        };
        let half_shifted = prepare_half_shifted(&image);
        let mut dst = vec![background; 10 * 7];

        blit_scaled_subpixel_x(
            &mut dst,
            10,
            7,
            &image,
            &half_shifted,
            &[0; 3],
            PARADE_SUBPIXEL_ONE,
            1,
        );

        assert!(dst[..10].iter().all(|pixel| *pixel == background));
        assert!(dst[4 * 10..].iter().all(|pixel| *pixel == background));
    }

    #[test]
    fn parade_card_rim_keeps_the_baked_bevel() {
        let base = color565(120, 120, 120);
        let mut image = SaverImage {
            pixels: vec![base; 8 * 6],
            w: 8,
            h: 6,
            stride: 8,
        };

        rim_parade_card(&mut image, &[0; 6]);

        assert_ne!(image.pixels[0], base);
        assert_ne!(image.pixels[7], base);
        assert_ne!(image.pixels[5 * 8 + 3], base);
    }

    #[test]
    fn both_motion_modes_advance_every_card_on_every_frame() {
        for motion in [ParadeMotion::Integer, ParadeMotion::Subpixel] {
            for layer_idx in 0..PARADE_SPEED_COUNT {
                let velocity = motion.card_velocity_fp(layer_idx, PARADE_REFERENCE_WIDTH);
                assert!(velocity > 0);
                let positions = (0..4).map(|frame| frame * velocity).collect::<Vec<_>>();
                assert!(positions.windows(2).all(|pair| pair[0] != pair[1]));
            }
        }
    }

    #[test]
    fn parade_velocity_uses_the_slowed_1080p_width_ratios() {
        let reference_1080p = 4 * PARADE_SUBPIXEL_ONE;
        for (width, expected) in [
            (320, PARADE_SUBPIXEL_ONE / 2),
            (640, PARADE_SUBPIXEL_ONE),
            (960, PARADE_SUBPIXEL_ONE * 3 / 2),
            (1280, PARADE_SUBPIXEL_ONE * 2),
            (1440, PARADE_SUBPIXEL_ONE * 9 / 4),
            (1920, PARADE_SUBPIXEL_ONE * 3),
        ] {
            assert_eq!(
                scale_parade_velocity(reference_1080p, width),
                expected,
                "incorrect motion ratio at {width}px wide"
            );
        }
    }

    fn one_second_card_travel(refresh_hz: u64, screen_w: usize) -> (i64, i64, u64) {
        let velocity_fp = ParadeMotion::Subpixel.card_velocity_fp(3, screen_w);
        let mut x_fp = 0_i64;
        let mut remainder = 0_i64;
        let mut previous_ticks = 0_u64;
        for frame in 1..=refresh_hz {
            let elapsed_ns = 1_000_000_000_u64 * frame / refresh_hz;
            let motion_ticks = parade_tick_delta_fp(Duration::from_nanos(elapsed_ns)) as u64;
            let tick_delta = motion_ticks.saturating_sub(previous_ticks) as i64;
            let motion = velocity_fp
                .saturating_mul(tick_delta)
                .saturating_add(remainder);
            x_fp = x_fp.saturating_add(motion / PARADE_TICK_ONE);
            remainder = motion % PARADE_TICK_ONE;
            previous_ticks = motion_ticks;
        }
        (x_fp, remainder, previous_ticks)
    }

    #[test]
    fn parade_motion_is_identical_after_one_second_at_50_and_60_hz() {
        for width in [320, 640, 720, 960, 1280, 1440, 1920] {
            let at_50_hz = one_second_card_travel(50, width);
            let at_60_hz = one_second_card_travel(60, width);
            assert_eq!(at_50_hz, at_60_hz, "card travel differs at {width}px wide");

            let star_at_50_hz =
                horizontal_star_position(3, 4_096, PARADE_REFERENCE_HEIGHT, at_50_hz.2);
            let star_at_60_hz =
                horizontal_star_position(3, 4_096, PARADE_REFERENCE_HEIGHT, at_60_hz.2);
            assert_eq!(
                star_at_50_hz, star_at_60_hz,
                "star travel differs at {width}px wide"
            );
        }
    }

    #[test]
    fn parade_initializes_all_five_layers_at_their_target_populations() {
        let mut state = ParadeState::new(0xfeed_face_cafe_beef);
        let images = test_images(PARADE_WIDE_LAYER_TARGETS.iter().sum());
        state.ensure_initialized(&images, 960, 540);
        let mut counts = [0usize; PARADE_SPEED_COUNT];
        for tile in &state.tiles {
            counts[tile.layer - PARADE_MIN_TILE_SPEED] += 1;
        }
        assert_eq!(counts, PARADE_WIDE_LAYER_TARGETS);
        for (idx, tile) in state
            .tiles
            .iter()
            .enumerate()
            .filter(|(_, tile)| tile.active)
        {
            assert!(tile.y >= -(tile.scaled.h as isize * 2 / 3));
            assert!(tile.y <= 540 - tile.scaled.h as isize / 3);
            assert!(state.placement_is_clear(
                tile.x(),
                tile.y,
                tile.scaled.w,
                tile.scaled.h,
                tile.speed,
                idx
            ));
        }
        let mut other = ParadeState::new(0x0123_4567_89ab_cdef);
        other.ensure_initialized(&images, 960, 540);
        assert_ne!(
            state
                .tiles
                .iter()
                .map(|tile| (tile.x(), tile.y, tile.speed))
                .collect::<Vec<_>>(),
            other
                .tiles
                .iter()
                .map(|tile| (tile.x(), tile.y, tile.speed))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parade_layer_spacing_increases_as_cards_get_larger() {
        let intervals = (1..=5)
            .map(|speed| {
                let (w, _, _) = parade_depth_style(speed, 540);
                parade_layer_interval_frames(
                    960,
                    w,
                    speed as i64 * PARADE_SUBPIXEL_ONE / 2,
                    PARADE_WIDE_LAYER_TARGETS[speed - 1],
                )
            })
            .collect::<Vec<_>>();
        let populations = PARADE_WIDE_LAYER_TARGETS;
        assert!(populations.windows(2).all(|pair| pair[0] > pair[1]));
        let mut minimum_left_edge_gaps = Vec::new();
        for speed in 1..=5 {
            let minimum_left_edge_gap = intervals[speed - 1] as usize * speed / 2;
            assert!(minimum_left_edge_gap > PARADE_REFERENCE_PLACEMENT_GAP);
            minimum_left_edge_gaps.push(minimum_left_edge_gap);
        }
        assert!(
            minimum_left_edge_gaps
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
    }

    #[test]
    fn parade_scaling_preserves_landscape_and_portrait_aspect_ratios() {
        let landscape = SaverImage {
            pixels: vec![Rgb565Pixel(0); 320 * 240],
            w: 320,
            h: 240,
            stride: 320,
        };
        let portrait = SaverImage {
            pixels: vec![Rgb565Pixel(0); 240 * 320],
            w: 240,
            h: 320,
            stride: 240,
        };
        assert_eq!(parade_scaled_style(&landscape, 5, 540), (160, 120, 255));
        assert_eq!(parade_scaled_style(&portrait, 5, 540), (120, 160, 255));
    }

    #[test]
    fn spawn_rejects_same_layer_overlap_but_ignores_other_layers() {
        let mut state = ParadeState::new(23);
        let scaled = SaverImage {
            pixels: vec![Rgb565Pixel(1); 16],
            w: 4,
            h: 4,
            stride: 4,
        };
        let phase_set = ParadePhaseSet::prepare(&scaled, ParadeSamplingProfile::LegacyHalf);
        let corner_insets = prepare_parade_corner_insets(scaled.w, scaled.h);
        state.tiles.push(ParadeTile {
            x_fp: -20 * PARADE_SUBPIXEL_ONE,
            y: 100,
            layer: 3,
            speed: 3,
            velocity_fp: 3 * PARADE_SUBPIXEL_ONE / 2,
            velocity_remainder: 0,
            image_idx: 0,
            scaled,
            phase_set,
            corner_insets,
            active: true,
            raster_held_this_frame: false,
            raster_moved_this_frame: false,
            next: None,
            pending_image_idx: None,
        });
        assert!(!state.placement_is_clear(-20, 100, 4, 4, 3, usize::MAX));
        assert!(state.placement_is_clear(-20, 100, 4, 4, 2, usize::MAX));
    }

    #[test]
    fn parade_draws_faster_cards_above_slower_cards() {
        let mut state = ParadeState::new(11);
        let images = test_images(64);
        state.ensure_initialized(&images, 960, 540);
        for (tile, speed) in state.tiles.iter_mut().zip([5, 2, 4, 3].into_iter().cycle()) {
            tile.layer = speed;
            tile.speed = speed;
        }
        prepare_parade_draw_order(&mut state);
        let speeds = state
            .draw_order
            .iter()
            .copied()
            .map(|idx| state.tiles[idx].layer)
            .collect::<Vec<_>>();
        assert!(speeds.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(speeds.first(), Some(&2));
        assert_eq!(speeds.last(), Some(&5));
    }

    #[test]
    fn parade_draw_order_reuses_its_allocation() {
        let mut state = ParadeState::new(11);
        let images = test_images(64);
        state.ensure_initialized(&images, 960, 540);

        prepare_parade_draw_order(&mut state);
        let pointer = state.draw_order.as_ptr();
        let capacity = state.draw_order.capacity();
        prepare_parade_draw_order(&mut state);

        assert_eq!(state.draw_order.as_ptr(), pointer);
        assert_eq!(state.draw_order.capacity(), capacity);
    }

    #[test]
    fn parade_depth_culling_is_pixel_equivalent_for_fully_covered_card() {
        let mut state = ParadeState::new(31);
        state.tiles = vec![
            solid_parade_tile(
                color565(255, 0, 0),
                4,
                4,
                10 * PARADE_SUBPIXEL_ONE,
                8,
                2,
                state.sampling_profile,
            ),
            solid_parade_tile(
                color565(0, 255, 0),
                24,
                18,
                2 * PARADE_SUBPIXEL_ONE,
                2,
                5,
                state.sampling_profile,
            ),
        ];
        prepare_parade_draw_order(&mut state);
        let mut expected = vec![color565(0, 0, 20); 32 * 24];
        render_parade_order(&mut expected, &state, &state.draw_order, 32, 24);

        assert_eq!(prepare_parade_visible_draw_order(&mut state, 32, 24), 1);
        assert_eq!(state.visible_draw_order, vec![1]);
        let mut actual = vec![color565(0, 0, 20); 32 * 24];
        render_parade_order(&mut actual, &state, &state.visible_draw_order, 32, 24);

        assert_eq!(actual, expected);
    }

    #[test]
    fn parade_depth_culling_preserves_lower_card_at_rounded_edge() {
        let mut state = ParadeState::new(37);
        state.tiles = vec![
            solid_parade_tile(
                color565(255, 0, 0),
                12,
                10,
                3 * PARADE_SUBPIXEL_ONE,
                3,
                2,
                state.sampling_profile,
            ),
            solid_parade_tile(
                color565(0, 255, 0),
                12,
                10,
                4 * PARADE_SUBPIXEL_ONE,
                3,
                5,
                state.sampling_profile,
            ),
        ];
        prepare_parade_draw_order(&mut state);
        let mut expected = vec![color565(0, 0, 20); 24 * 18];
        render_parade_order(&mut expected, &state, &state.draw_order, 24, 18);

        assert_eq!(prepare_parade_visible_draw_order(&mut state, 24, 18), 0);
        let mut actual = vec![color565(0, 0, 20); 24 * 18];
        render_parade_order(&mut actual, &state, &state.visible_draw_order, 24, 18);

        assert_eq!(actual, expected);
        assert_eq!(actual[5 * 24 + 3], color565(255, 0, 0));
    }

    #[test]
    fn parade_depth_culling_is_pixel_equivalent_at_half_pixel_phase() {
        let mut state = ParadeState::new_with_profile(41, ParadeSamplingProfile::LegacyHalf);
        state.tiles = vec![
            solid_parade_tile(
                color565(255, 0, 0),
                4,
                4,
                10 * PARADE_SUBPIXEL_ONE,
                8,
                2,
                state.sampling_profile,
            ),
            solid_parade_tile(
                color565(0, 255, 0),
                24,
                18,
                2 * PARADE_SUBPIXEL_ONE + PARADE_SUBPIXEL_ONE / 2,
                2,
                5,
                state.sampling_profile,
            ),
        ];
        prepare_parade_draw_order(&mut state);
        let mut expected = vec![color565(0, 0, 20); 32 * 24];
        render_parade_order(&mut expected, &state, &state.draw_order, 32, 24);

        assert_eq!(prepare_parade_visible_draw_order(&mut state, 32, 24), 1);
        let mut actual = vec![color565(0, 0, 20); 32 * 24];
        render_parade_order(&mut actual, &state, &state.visible_draw_order, 32, 24);

        assert_eq!(actual, expected);
    }

    #[test]
    fn parade_render_places_faster_card_pixels_above_slower_card_pixels() {
        let slow = color565(255, 20, 20);
        let fast = color565(20, 255, 20);
        let images = vec![
            SaverImage {
                pixels: vec![slow],
                w: 1,
                h: 1,
                stride: 1,
            },
            SaverImage {
                pixels: vec![fast],
                w: 1,
                h: 1,
                stride: 1,
            },
        ];
        let mut state = ParadeState::new(13);
        state.image_count = images.len();
        state.deck = vec![0, 1];
        let slow_scaled = scale_lanczos3_rgb565_tinted(&images[0], 77, 58, 154);
        let slow_phase_set =
            ParadePhaseSet::prepare(&slow_scaled, ParadeSamplingProfile::LegacyHalf);
        let slow_corner_insets = prepare_parade_corner_insets(slow_scaled.w, slow_scaled.h);
        let fast_scaled = scale_lanczos3_rgb565_tinted(&images[1], 192, 144, 255);
        let fast_phase_set =
            ParadePhaseSet::prepare(&fast_scaled, ParadeSamplingProfile::LegacyHalf);
        let fast_corner_insets = prepare_parade_corner_insets(fast_scaled.w, fast_scaled.h);
        state.tiles = vec![
            ParadeTile {
                x_fp: 20 * PARADE_SUBPIXEL_ONE,
                y: 10,
                layer: 2,
                speed: 2,
                velocity_fp: PARADE_SUBPIXEL_ONE,
                velocity_remainder: 0,
                image_idx: 0,
                scaled: slow_scaled,
                phase_set: slow_phase_set,
                corner_insets: slow_corner_insets,
                active: true,
                raster_held_this_frame: false,
                raster_moved_this_frame: false,
                next: None,
                pending_image_idx: None,
            },
            ParadeTile {
                x_fp: 20 * PARADE_SUBPIXEL_ONE,
                y: 10,
                layer: 5,
                speed: 5,
                velocity_fp: 5 * PARADE_SUBPIXEL_ONE / 2,
                velocity_remainder: 0,
                image_idx: 1,
                scaled: fast_scaled,
                phase_set: fast_phase_set,
                corner_insets: fast_corner_insets,
                active: true,
                raster_held_this_frame: false,
                raster_moved_this_frame: false,
                next: None,
                pending_image_idx: None,
            },
        ];
        let mut dst = vec![Rgb565Pixel(0); 160 * 120];

        prepare_parade_draw_order(&mut state);
        render_parade_order(&mut dst, &state, &state.draw_order, 160, 120);

        assert_eq!(dst[20 * 160 + 23], fast);
    }

    #[test]
    fn parade_card_size_and_brightness_increase_with_speed() {
        let styles = (1..=5)
            .map(|speed| parade_depth_style(speed, 540))
            .collect::<Vec<_>>();
        assert_eq!(styles[0], (32, 32, 145));
        assert_eq!(styles[1], (64, 64, 170));
        assert_eq!(styles[2], (96, 96, 198));
        assert_eq!(styles[3], (128, 128, 226));
        assert_eq!(styles[4], (160, 160, 255));
        assert!(styles.windows(2).all(|pair| {
            pair[0].0 < pair[1].0 && pair[0].1 < pair[1].1 && pair[0].2 < pair[1].2
        }));
    }

    #[test]
    fn parade_card_dimensions_scale_from_the_540p_reference() {
        for (height, expected) in [
            (240, 71),
            (288, 85),
            (384, 114),
            (480, 142),
            (540, 160),
            (600, 178),
            (720, 213),
            (768, 228),
        ] {
            assert_eq!(parade_depth_style(5, height).0, expected);
            assert_eq!(parade_depth_style(5, height).1, expected);
        }
    }

    #[test]
    fn parade_population_uses_framebuffer_aspect_ratio() {
        for (width, height) in [(1280, 720), (683, 384), (960, 540), (960, 600)] {
            assert_eq!(
                parade_layer_targets(width, height),
                PARADE_WIDE_LAYER_TARGETS
            );
        }
        for (width, height) in [(320, 240), (384, 288), (640, 480), (1024, 768)] {
            assert_eq!(
                parade_layer_targets(width, height),
                PARADE_COMPACT_LAYER_TARGETS
            );
        }
    }

    #[test]
    fn compact_population_is_twenty_five_percent_smaller_with_integer_rounding() {
        for layer in 0..PARADE_SPEED_COUNT {
            assert_eq!(
                PARADE_COMPACT_LAYER_TARGETS[layer],
                (PARADE_WIDE_LAYER_TARGETS[layer] * 3 + 2) / 4
            );
        }
    }

    #[test]
    fn crt_phase_storage_for_active_cards_and_successors_stays_below_34_mib() {
        let bytes_per_pixel = std::mem::size_of::<Rgb565Pixel>();
        let one_population_bytes = PARADE_COMPACT_LAYER_TARGETS
            .iter()
            .enumerate()
            .map(|(layer_idx, target)| {
                let speed = PARADE_MIN_TILE_SPEED + layer_idx;
                let (width, height, _) = parade_depth_style(speed, 480);
                let base = width * height;
                let shifted = CRT_SHIFTED_PHASE_COUNT * (width + 1) * height;
                let descriptors = CRT_PHASE_COUNT * std::mem::size_of::<SaverImage>();
                target * ((base + shifted) * bytes_per_pixel + descriptors)
            })
            .sum::<usize>();
        let active_and_successors = one_population_bytes * 2;

        assert!(active_and_successors < 34 * 1024 * 1024);
        assert!(active_and_successors > 32 * 1024 * 1024);
    }

    #[test]
    fn crt_initial_cards_own_complete_phase_banks() {
        let tile_count = PARADE_COMPACT_LAYER_TARGETS.iter().sum::<usize>();
        let images = test_images(tile_count);
        let mut state =
            ParadeState::new_with_profile(0x16_16_16_16, ParadeSamplingProfile::CrtSixteenth);

        state.ensure_initialized(&images, 640, 480);

        assert_eq!(state.sampling_profile, ParadeSamplingProfile::CrtSixteenth);
        assert!(state.tiles.iter().all(|tile| {
            matches!(
                &tile.phase_set,
                ParadePhaseSet::CrtSixteenth(phases)
                    if phases.len() == CRT_SHIFTED_PHASE_COUNT
            )
        }));
    }

    #[test]
    fn lanczos_scaler_preserves_flat_color_and_applies_depth_tint() {
        let source_color = color565(200, 120, 40);
        let source = SaverImage {
            pixels: vec![source_color; 64],
            w: 8,
            h: 8,
            stride: 8,
        };
        let near = scale_lanczos3_rgb565_tinted(&source, 17, 13, 255);
        let far = scale_lanczos3_rgb565_tinted(&source, 7, 5, 160);
        assert!(near.pixels.iter().all(|pixel| *pixel == source_color));
        assert!(far.pixels.iter().all(|pixel| pixel.0 < source_color.0));
    }

    #[test]
    fn equal_size_blit_copies_clipped_rows_without_resampling() {
        let image = SaverImage {
            pixels: (0..12).map(|value| Rgb565Pixel(value + 1)).collect(),
            w: 4,
            h: 3,
            stride: 4,
        };
        let mut dst = vec![Rgb565Pixel(0); 5 * 4];
        blit_scaled(&mut dst, 5, 4, &image, -2, 1, 4, 3, 255);
        assert_eq!(dst[5..7], image.pixels[2..4]);
        assert_eq!(dst[10..12], image.pixels[6..8]);
        assert_eq!(dst[15..17], image.pixels[10..12]);
    }

    #[test]
    fn radial_starfield_is_available_as_a_standalone_mode() {
        assert_eq!(
            ScreensaverMode::parse("radial-starfield"),
            Some(ScreensaverMode::RadialStarfield)
        );
    }

    #[test]
    fn scaler_column_codes_mark_the_center_and_distinguish_neighbors() {
        let width = 960;
        assert_eq!(
            scaler_column_code(width / 2, width),
            color565(255, 255, 255)
        );
        assert_eq!(scaler_column_code(width / 2 - 1, width), color565(0, 0, 0));
        assert_eq!(scaler_column_code(width / 2 + 1, width), color565(0, 0, 0));
        assert_ne!(
            scaler_column_code(100, width),
            scaler_column_code(101, width)
        );
    }

    #[test]
    fn scaler_diagnostic_is_selectable_but_not_part_of_the_megademo() {
        assert_eq!(
            ScreensaverMode::parse("pixel-grid"),
            Some(ScreensaverMode::PixelGrid)
        );
        assert!(!ScreensaverMode::MEGA.contains(&ScreensaverMode::PixelGrid));
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
        let screensaver =
            LauncherScreensaver::particle(renderer, None, Arc::new(AtomicBool::new(false)));
        assert!(screensaver.requires_direct_hidden());
        assert!(!screensaver.is_loading_archive());
        assert!(screensaver.has_rendered_card());
        assert_eq!(screensaver.active_card_count(), 0);
    }
}
