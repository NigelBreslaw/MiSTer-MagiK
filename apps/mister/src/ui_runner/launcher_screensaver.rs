// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#![cfg_attr(target_os = "macos", allow(dead_code))]

#[cfg(not(target_os = "macos"))]
use super::*;
#[cfg(target_os = "macos")]
use crate::framebuffer::target::{blend_565, brighten_565};
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
    ScreenshotParade, ScreenshotParadeConfig, ScreenshotParadeReplacementMode,
    ScreenshotParadeStartup, ScreenshotParadeStats,
};
#[cfg(target_os = "macos")]
use slint::platform::software_renderer::{Rgb565Pixel, TargetPixel};
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
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
    CabinetMarquee,
    RandomAccessLoader,
    ColorClashGallery,
    RadialStarfield,
    PixelGrid,
    IdleMegademo,
}

impl ScreensaverMode {
    const ALL: [Self; 19] = [
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
        Self::CabinetMarquee,
        Self::RandomAccessLoader,
        Self::ColorClashGallery,
        Self::RadialStarfield,
        Self::PixelGrid,
        Self::IdleMegademo,
    ];

    const MEGA: [Self; 18] = [
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

fn shared_parade_trace(stats: ScreenshotParadeStats) -> ScreensaverFrameTrace {
    ScreensaverFrameTrace {
        card_adopt_us: stats.card_adopt_us,
        cards_adopted: stats.cards_adopted,
        parade_advance_us: stats.parade_advance_us,
        background_us: stats.background_us,
        draw_order_us: stats.draw_order_us,
        tile_blit_us: stats.tile_blit_us,
        cards_drawn: stats.cards_drawn,
        cards_culled: stats.cards_culled,
        sampling_profile: "1:sixteenth,2:sixteenth,3:sixteenth,4:sixteenth,5:sixteenth",
        raster_held_cards: stats.raster_held_cards,
        raster_moved_cards: stats.raster_moved_cards,
        raster_hold_layer_mask: stats.raster_hold_layer_mask,
        raster_visible_layer_mask: stats.raster_visible_layer_mask,
        sixteenth_phase_layer_mask: stats.sixteenth_phase_layer_mask,
        phase_bank_resident_bytes: stats.phase_bank_resident_bytes,
        ..ScreensaverFrameTrace::default()
    }
}

fn log_shared_parade_stats(parade: &ScreenshotParade) {
    let stats = parade.stats();
    let scale_average_us = stats.scale_total_us / u128::from(stats.scale_count.max(1));
    let phase_average_us = stats.phase_total_us / u128::from(stats.phase_count.max(1));
    crate::ui_logln!(
        "screensaver_lanczos sampling={} scales={} total_us={} average_us={} max_us={} phase_prepares={} phase_total_us={} phase_average_us={} phase_max_us={} queue_max={} queue_bound={} worker_connected=true phase_cache_bytes={}",
        "sixteenth",
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
        w: usize,
        h: usize,
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
                startup: ScreenshotParadeStartup::Streaming,
                replacement_mode: ScreenshotParadeReplacementMode::Prepare,
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

struct LoadedScreensaverArchive {
    path: PathBuf,
    archive: preview_worker::ResidentPreviewArchive,
    asset_keys: Vec<String>,
    open_us: u128,
}

pub struct LauncherScreensaverLoader {
    ready_rx: Receiver<LauncherScreensaver>,
    cancelled: Arc<AtomicBool>,
}

impl LauncherScreensaverLoader {
    pub fn start(w: usize, h: usize, startup_started_at: Option<Instant>) -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        if magik_particle_renderer_requested() {
            match particle_config_from_env(w, h).and_then(|config| {
                let renderer = ParticleRenderer::new_magik(config)?;
                let reload =
                    MagikRecipeReload::for_layout(DeviceLayout::current(), w, h, config.preset)?;
                Ok((renderer, reload))
            }) {
                Ok((renderer, reload)) => {
                    let _ = ready_tx.send(LauncherScreensaver::particle(renderer, reload));
                }
                Err(error) => {
                    crate::ui_errln!("particle renderer initialization failed: {error}");
                }
            }
            return Self {
                ready_rx,
                cancelled: Arc::new(AtomicBool::new(false)),
            };
        }
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
                let result = (|| {
                    let archive = preview_worker::ResidentPreviewArchive::open(&path)
                        .map_err(|error| format!("path={} error={error}", path.display()))?;
                    if worker_cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    let loaded = LoadedScreensaverArchive {
                        asset_keys: archive.asset_keys().to_vec(),
                        archive,
                        path,
                        open_us: started.elapsed().as_micros(),
                    };
                    let seed = random_seed();
                    build_loaded_screensaver(loaded, w, h, seed, startup_started_at).map(Some)
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

    pub fn try_ready(&self) -> Option<LauncherScreensaver> {
        self.ready_rx.try_recv().ok()
    }
}

impl Drop for LauncherScreensaverLoader {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

fn build_loaded_screensaver(
    loaded: LoadedScreensaverArchive,
    width: usize,
    height: usize,
    seed: u64,
    startup_started_at: Option<Instant>,
) -> Result<LauncherScreensaver, String> {
    crate::ui_logln!(
        "screensaver_loader path={} pack_bytes={} entries={}",
        loaded.path.display(),
        loaded.archive.compressed_bytes(),
        loaded.asset_keys.len()
    );
    let geometry = SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
    let preparation_slack = Arc::new(mister_magik_screenshot_parade::PreparationSlack::new());
    let worker_start = Arc::new(|| {
        mister_magik_catalog::runtime_thread::apply_runtime_thread_policy(
            mister_magik_catalog::runtime_thread::RuntimeThreadRole::ScreensaverScaler,
        );
    });
    let construction_started = Instant::now();
    let parade = ScreenshotParade::new(
        loaded.archive,
        ScreenshotParadeConfig {
            geometry,
            seed,
            startup: ScreenshotParadeStartup::Streaming,
            replacement_mode: ScreenshotParadeReplacementMode::Prepare,
            worker_start: Some(worker_start),
            preparation_slack: Some(preparation_slack),
        },
    )?;
    crate::ui_logln!(
        "screensaver_loader_timing archive_open_us={} parade_construct_us={} total_us={} cards=0",
        loaded.open_us,
        construction_started.elapsed().as_micros(),
        loaded
            .open_us
            .saturating_add(construction_started.elapsed().as_micros())
    );
    Ok(LauncherScreensaver {
        parade: Some(parade),
        particle: None,
        particle_reload: None,
        startup_started_at,
        frame: 0,
        motion_started_at: Instant::now(),
    })
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
    fn new(w: usize, h: usize) -> Self {
        Self {
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
    let mut render_state = ScreensaverRenderState::new(ui.render_w(), ui.render_h());
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
        let screensaver = LauncherScreensaver::particle(renderer, None);
        assert!(screensaver.requires_direct_hidden());
        assert!(!screensaver.is_loading_archive());
        assert!(screensaver.has_rendered_card());
        assert_eq!(screensaver.active_card_count(), 0);
    }
}
