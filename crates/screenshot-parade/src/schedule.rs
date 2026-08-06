// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::raster::{PreparedScreenshotCard, VisibleSpan, depth_style};
use crate::slack::PreparationSlack;
use crate::{PARADE_SUBPIXEL_ONE, ScreenshotImage};
use mister_magik_catalog::preview_worker::ResidentPreviewArchive;
use mister_magik_framebuffer_scenes::{
    FramebufferScene, Rgb565Pixel, SceneBufferId, SceneClock, SceneError, SceneGeometry,
    SceneTarget,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const WIDE_LAYER_TARGETS: [usize; 5] = [28, 21, 17, 14, 10];
const COMPACT_LAYER_TARGETS: [usize; 5] = [25, 18, 15, 12, 9];
const REFERENCE_HEIGHT: usize = 540;
// Preserve edge-to-edge travel time at every geometry, using velocities that
// are 25% slower than the former 1920x1080 reference behavior.
const REFERENCE_WIDTH: usize = 1920;
const CARD_SPEED_NUMERATOR: i64 = 3;
const CARD_SPEED_DENOMINATOR: i64 = 4;
const MAX_CARD_ADOPTIONS_PER_FRAME: usize = 1;
const REFERENCE_HZ: u64 = 60;
const TICK_ONE: i64 = 1 << 16;
const MIN_TILE_SPEED: usize = 1;
const SPEED_COUNT: usize = 5;
const REFERENCE_PLACEMENT_GAP: usize = 18;

pub type WorkerStartCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
pub struct ScreenshotParadeConfig {
    pub geometry: SceneGeometry,
    pub seed: u64,
    pub worker_start: Option<WorkerStartCallback>,
    pub preparation_slack: Option<Arc<PreparationSlack>>,
}

impl std::fmt::Debug for ScreenshotParadeConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenshotParadeConfig")
            .field("geometry", &self.geometry)
            .field("seed", &self.seed)
            .field(
                "worker_start",
                &self.worker_start.as_ref().map(|_| "callback"),
            )
            .field(
                "preparation_slack",
                &self.preparation_slack.as_ref().map(|_| "checkpoint"),
            )
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScreenshotParadeStats {
    pub card_adopt_us: u128,
    pub cards_adopted: usize,
    pub parade_advance_us: u128,
    pub background_us: u128,
    pub draw_order_us: u128,
    pub tile_blit_us: u128,
    pub coverage_composite_calls: usize,
    pub coverage_probe_sampled: bool,
    pub partial_edge_pixels: usize,
    pub exact_base_background_hits: usize,
    pub active_cards: usize,
    pub cards_drawn: usize,
    pub cards_culled: usize,
    pub opaque_pixels: usize,
    pub opaque_rows: usize,
    pub union_avoidable_opaque_pixels: usize,
    pub union_avoidable_opaque_rows: usize,
    pub union_fully_covered_opaque_rows: usize,
    pub preparation_overlapped_render: bool,
    pub preparation_decode_overlapped_render: bool,
    pub preparation_activity_transitions: u32,
    pub preparation_stage_start: u8,
    pub preparation_stage_end: u8,
    pub raster_held_cards: usize,
    pub raster_moved_cards: usize,
    pub raster_hold_layer_mask: u8,
    pub raster_visible_layer_mask: u8,
    pub phase_bank_resident_bytes: usize,
    pub image_cache_resident_bytes: usize,
    pub scale_count: u64,
    pub scale_total_us: u128,
    pub scale_max_us: u128,
    pub phase_count: u64,
    pub phase_total_us: u128,
    pub phase_max_us: u128,
    pub decode_successes: u64,
    pub decode_failures: u64,
    pub unique_decoded: usize,
    pub queue_depth: usize,
    pub queue_max: usize,
}

#[derive(Clone, Copy)]
struct LayerSchedule {
    next_spawn_frame: u64,
    interval_frames: u64,
    spawn_count: u64,
    active_sum: u64,
    sample_count: u64,
}

#[derive(Clone)]
struct PreparedCard {
    image_index: usize,
    raster: PreparedScreenshotCard,
    scale_us: u128,
    phase_us: u128,
}

struct Tile {
    x_fp: i64,
    y: isize,
    layer: usize,
    speed: usize,
    velocity_fp: i64,
    velocity_remainder: i64,
    image_index: usize,
    raster: PreparedScreenshotCard,
    active: bool,
    raster_held_this_frame: bool,
    raster_moved_this_frame: bool,
    next: Option<PreparedCard>,
    pending_image_index: Option<usize>,
}

impl Tile {
    fn x(&self) -> isize {
        self.x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize
    }
}

struct ScaleJob {
    tile_index: usize,
    image_index: usize,
    speed: usize,
    screen_height: usize,
}

struct ScaleResult {
    tile_index: usize,
    image_index: usize,
    card: Result<PreparedCard, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct UnionOcclusionStats {
    opaque_pixels: usize,
    opaque_rows: usize,
    avoidable_pixels: usize,
    avoidable_rows: usize,
    fully_covered_rows: usize,
}

impl Rect {
    fn contains(self, other: Self) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && self.x1 >= other.x1 && self.y1 >= other.y1
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VisibleCard {
    tile_index: usize,
    span_start: usize,
    span_end: usize,
    restricted: bool,
}

pub struct ScreenshotParade {
    geometry: SceneGeometry,
    tiles: Vec<Tile>,
    draw_order: Vec<usize>,
    visible_draw_order: Vec<VisibleCard>,
    visible_spans: Vec<VisibleSpan>,
    depth_coverage: Vec<Rect>,
    depth_coverage_rows: Vec<Vec<(usize, usize)>>,
    deck: Vec<usize>,
    cursor: usize,
    rng: u64,
    asset_keys: Vec<String>,
    compressed_bytes: usize,
    failed_images: HashSet<usize>,
    unique_decoded: HashSet<usize>,
    scale_count: u64,
    scale_total_us: u128,
    scale_max_us: u128,
    phase_count: u64,
    phase_total_us: u128,
    phase_max_us: u128,
    decode_successes: u64,
    decode_failures: u64,
    scale_tx: Option<Sender<ScaleJob>>,
    scale_rx: Receiver<ScaleResult>,
    scale_worker: Option<JoinHandle<()>>,
    scale_worker_connected: bool,
    preparation_epoch: Arc<AtomicU32>,
    preparation_stage: Arc<AtomicU8>,
    preparation_slack: Option<Arc<PreparationSlack>>,
    scale_queue_depth: usize,
    scale_queue_max: usize,
    layer_targets: [usize; SPEED_COUNT],
    layers: [LayerSchedule; SPEED_COUNT],
    previous_motion_ticks_fp: u64,
    last_elapsed: Duration,
    stats: ScreenshotParadeStats,
}

impl ScreenshotParade {
    pub fn preparation_slack(&self) -> Option<Arc<PreparationSlack>> {
        self.preparation_slack.clone()
    }

    pub fn new(
        archive: ResidentPreviewArchive,
        config: ScreenshotParadeConfig,
    ) -> Result<Self, String> {
        Self::construct(archive, config, false)
    }

    pub fn new_offline_prepared(
        archive: ResidentPreviewArchive,
        config: ScreenshotParadeConfig,
    ) -> Result<Self, String> {
        Self::construct(archive, config, true)
    }

    fn construct(
        archive: ResidentPreviewArchive,
        config: ScreenshotParadeConfig,
        offline_prepared: bool,
    ) -> Result<Self, String> {
        let width = config.geometry.width();
        let height = config.geometry.height();
        let asset_keys = archive.asset_keys().to_vec();
        if asset_keys.is_empty() {
            return Err("screenshot archive contains no RGB565 assets".to_owned());
        }
        let compressed_bytes = archive.compressed_bytes();
        let (scale_tx, job_rx) = mpsc::channel::<ScaleJob>();
        let (result_tx, scale_rx) = mpsc::channel::<ScaleResult>();
        let preparation_epoch = Arc::new(AtomicU32::new(0));
        let preparation_stage = Arc::new(AtomicU8::new(0));
        let worker_preparation_epoch = Arc::clone(&preparation_epoch);
        let worker_preparation_stage = Arc::clone(&preparation_stage);
        let worker_start = config.worker_start.clone();
        let worker_preparation_slack = config.preparation_slack.clone();
        let scale_worker = std::thread::Builder::new()
            .name("screenshot-parade-scale".to_owned())
            .spawn(move || {
                if let Some(callback) = worker_start {
                    callback();
                }
                run_scale_worker(
                    archive,
                    job_rx,
                    result_tx,
                    worker_preparation_epoch,
                    worker_preparation_stage,
                    worker_preparation_slack,
                );
            })
            .map_err(|error| format!("spawn screenshot parade scale worker: {error}"))?;
        let layer_targets = layer_targets(width, height);
        let mut parade = Self {
            geometry: config.geometry,
            tiles: Vec::new(),
            draw_order: Vec::with_capacity(WIDE_LAYER_TARGETS.iter().sum()),
            visible_draw_order: Vec::with_capacity(WIDE_LAYER_TARGETS.iter().sum()),
            visible_spans: Vec::with_capacity(WIDE_LAYER_TARGETS.iter().sum::<usize>() * 128),
            depth_coverage: Vec::with_capacity(WIDE_LAYER_TARGETS.iter().sum()),
            depth_coverage_rows: (0..height).map(|_| Vec::with_capacity(8)).collect(),
            deck: (0..asset_keys.len()).collect(),
            cursor: 0,
            rng: config.seed,
            asset_keys,
            compressed_bytes,
            failed_images: HashSet::new(),
            unique_decoded: HashSet::new(),
            scale_count: 0,
            scale_total_us: 0,
            scale_max_us: 0,
            phase_count: 0,
            phase_total_us: 0,
            phase_max_us: 0,
            decode_successes: 0,
            decode_failures: 0,
            scale_tx: Some(scale_tx),
            scale_rx,
            scale_worker: Some(scale_worker),
            scale_worker_connected: true,
            preparation_epoch,
            preparation_stage,
            preparation_slack: config.preparation_slack,
            scale_queue_depth: 0,
            scale_queue_max: 0,
            layer_targets,
            layers: [LayerSchedule {
                next_spawn_frame: 0,
                interval_frames: 1,
                spawn_count: 0,
                active_sum: 0,
                sample_count: 0,
            }; SPEED_COUNT],
            previous_motion_ticks_fp: 0,
            last_elapsed: Duration::ZERO,
            stats: ScreenshotParadeStats::default(),
        };
        shuffle(&mut parade.deck, &mut parade.rng);
        if offline_prepared {
            parade.prepare_initial_population()?;
        } else {
            parade.begin_streaming();
        }
        Ok(parade)
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.tiles.iter().any(|tile| tile.active)
    }

    #[must_use]
    pub fn has_pending_work(&self) -> bool {
        self.scale_queue_depth > 0
            || self
                .tiles
                .iter()
                .any(|tile| tile.pending_image_index.is_some())
    }

    #[must_use]
    pub fn asset_count(&self) -> usize {
        self.asset_keys.len()
    }

    #[must_use]
    pub fn active_card_count(&self) -> usize {
        self.tiles.iter().filter(|tile| tile.active).count()
    }

    #[must_use]
    pub fn first_ready_layer(&self) -> Option<usize> {
        self.tiles
            .iter()
            .find(|tile| tile.active)
            .map(|tile| tile.layer)
    }

    #[must_use]
    pub const fn compressed_bytes(&self) -> usize {
        self.compressed_bytes
    }

    #[must_use]
    pub fn queue_bound(&self) -> usize {
        self.layer_targets.iter().sum()
    }

    #[must_use]
    pub const fn stats(&self) -> ScreenshotParadeStats {
        self.stats
    }

    pub fn render_at(
        &mut self,
        pixels: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<ScreenshotParadeStats, String> {
        if elapsed < self.last_elapsed {
            return Err("screenshot parade elapsed time must be monotonic".to_owned());
        }
        let motion_ticks_fp = tick_delta_fp(elapsed) as u64;
        let stats = self.render_at_motion_ticks(pixels, motion_ticks_fp)?;
        self.last_elapsed = elapsed;
        Ok(stats)
    }

    pub fn render_at_presentation_tick(
        &mut self,
        pixels: &mut [Rgb565Pixel],
        presentation_tick: u64,
    ) -> Result<ScreenshotParadeStats, String> {
        let motion_ticks_fp = presentation_tick
            .checked_mul(TICK_ONE as u64)
            .ok_or_else(|| "screenshot parade presentation tick overflowed".to_owned())?;
        self.render_at_motion_ticks(pixels, motion_ticks_fp)
    }

    fn render_at_motion_ticks(
        &mut self,
        pixels: &mut [Rgb565Pixel],
        motion_ticks_fp: u64,
    ) -> Result<ScreenshotParadeStats, String> {
        if pixels.len() != self.geometry.len() {
            return Err(format!(
                "screenshot parade target has {} pixels, expected {}",
                pixels.len(),
                self.geometry.len()
            ));
        }
        if motion_ticks_fp < self.previous_motion_ticks_fp {
            return Err("screenshot parade motion clock must be monotonic".to_owned());
        }
        let delta = i64::try_from(motion_ticks_fp - self.previous_motion_ticks_fp)
            .map_err(|_| "screenshot parade motion clock step overflowed".to_owned())?;
        self.previous_motion_ticks_fp = motion_ticks_fp;
        let width = self.geometry.width();
        let height = self.geometry.height();
        let preparation_epoch_start = self.preparation_epoch.load(Ordering::Relaxed);
        let preparation_stage_start = self.preparation_stage.load(Ordering::Relaxed);
        let preparation_slack_start = self
            .preparation_slack
            .as_deref()
            .map(PreparationSlack::snapshot);

        let background_start = Instant::now();
        let background_pmu = mister_magik_perf_events::sampled_span("screensaver.background");
        render_background(pixels, width, height, motion_ticks_fp);
        drop(background_pmu);
        let background_us = background_start.elapsed().as_micros();
        let advance_pmu = mister_magik_perf_events::sampled_span("screensaver.advance");
        let (card_adopt_us, cards_adopted, parade_advance_us) =
            self.advance(motion_ticks_fp, delta);
        drop(advance_pmu);
        let draw_order_start = Instant::now();
        let draw_order_pmu = mister_magik_perf_events::sampled_span("screensaver.draw-order");
        self.prepare_draw_order();
        let (cards_culled, union_occlusion) = self.prepare_visible_draw_order();
        drop(draw_order_pmu);
        let draw_order_us = draw_order_start.elapsed().as_micros();

        let mut raster_held_cards = 0;
        let mut raster_moved_cards = 0;
        let mut raster_hold_layer_mask = 0_u8;
        let mut raster_visible_layer_mask = 0_u8;
        for visible in &self.visible_draw_order {
            let tile = &self.tiles[visible.tile_index];
            let layer_index = tile.layer.saturating_sub(MIN_TILE_SPEED);
            if layer_index < u8::BITS as usize {
                raster_visible_layer_mask |= 1_u8 << layer_index;
                if tile.raster_held_this_frame {
                    raster_hold_layer_mask |= 1_u8 << layer_index;
                }
            }
            raster_held_cards += usize::from(tile.raster_held_this_frame);
            raster_moved_cards += usize::from(tile.raster_moved_this_frame);
        }
        let tile_blit_start = Instant::now();
        let tile_blit_pmu = mister_magik_perf_events::sampled_span("screensaver.tile-blit");
        let mut coverage_composite_calls = 0;
        let mut partial_edge_pixels = 0;
        let mut exact_base_background_hits = 0;
        // Destination classification is diagnostic rather than presentation work.
        // Sample it sparsely so the probe cannot materially change the deadline
        // behavior it is intended to measure.
        let coverage_probe_sampled = (motion_ticks_fp / TICK_ONE as u64).is_multiple_of(64);
        let base_background = color565(0, 0, 10);
        if coverage_probe_sampled {
            for visible in &self.visible_draw_order {
                let tile = &self.tiles[visible.tile_index];
                let blit_stats = if visible.restricted {
                    tile.raster.blit_visible_spans_with_coverage_probe(
                        pixels,
                        width,
                        tile.x_fp,
                        tile.y,
                        &self.visible_spans[visible.span_start..visible.span_end],
                        base_background,
                    )
                } else {
                    tile.raster.blit_with_coverage_probe(
                        pixels,
                        width,
                        height,
                        tile.x_fp,
                        tile.y,
                        base_background,
                    )
                };
                coverage_composite_calls += blit_stats.composite_calls;
                partial_edge_pixels += blit_stats.partial_edge_pixels;
                exact_base_background_hits += blit_stats.exact_base_background_hits;
            }
        } else {
            for visible in &self.visible_draw_order {
                let tile = &self.tiles[visible.tile_index];
                if visible.restricted {
                    tile.raster.blit_visible_spans(
                        pixels,
                        width,
                        tile.x_fp,
                        tile.y,
                        &self.visible_spans[visible.span_start..visible.span_end],
                    );
                } else {
                    tile.raster.blit(pixels, width, height, tile.x_fp, tile.y);
                }
            }
        }
        drop(tile_blit_pmu);
        let preparation_epoch_end = self.preparation_epoch.load(Ordering::Relaxed);
        let preparation_stage_end = self.preparation_stage.load(Ordering::Relaxed);
        let preparation_slack_end = self
            .preparation_slack
            .as_deref()
            .map(PreparationSlack::snapshot);
        let raster_overlap = preparation_slack_start
            .zip(preparation_slack_end)
            .is_some_and(|(start, end)| {
                start.raster_active || end.raster_active || start.raster_epoch != end.raster_epoch
            });
        let decode_overlap = preparation_slack_start
            .zip(preparation_slack_end)
            .is_some_and(|(start, end)| {
                start.decode_active || end.decode_active || start.decode_epoch != end.decode_epoch
            });
        self.stats = ScreenshotParadeStats {
            card_adopt_us,
            cards_adopted,
            parade_advance_us,
            background_us,
            draw_order_us,
            tile_blit_us: tile_blit_start.elapsed().as_micros(),
            coverage_composite_calls,
            coverage_probe_sampled,
            partial_edge_pixels,
            exact_base_background_hits,
            active_cards: self.tiles.iter().filter(|tile| tile.active).count(),
            cards_drawn: self.visible_draw_order.len(),
            cards_culled,
            opaque_pixels: union_occlusion.opaque_pixels,
            opaque_rows: union_occlusion.opaque_rows,
            union_avoidable_opaque_pixels: union_occlusion.avoidable_pixels,
            union_avoidable_opaque_rows: union_occlusion.avoidable_rows,
            union_fully_covered_opaque_rows: union_occlusion.fully_covered_rows,
            preparation_overlapped_render: if self.preparation_slack.is_some() {
                raster_overlap
            } else {
                preparation_epoch_start & 1 != 0 || preparation_epoch_end != preparation_epoch_start
            },
            preparation_decode_overlapped_render: decode_overlap,
            preparation_activity_transitions: preparation_slack_start
                .zip(preparation_slack_end)
                .map_or_else(
                    || preparation_epoch_end.wrapping_sub(preparation_epoch_start),
                    |(start, end)| {
                        end.raster_epoch
                            .wrapping_sub(start.raster_epoch)
                            .min(u64::from(u32::MAX)) as u32
                    },
                ),
            preparation_stage_start,
            preparation_stage_end,
            raster_held_cards,
            raster_moved_cards,
            raster_hold_layer_mask,
            raster_visible_layer_mask,
            phase_bank_resident_bytes: self.phase_bank_resident_bytes(),
            image_cache_resident_bytes: self.image_cache_resident_bytes(),
            scale_count: self.scale_count,
            scale_total_us: self.scale_total_us,
            scale_max_us: self.scale_max_us,
            phase_count: self.phase_count,
            phase_total_us: self.phase_total_us,
            phase_max_us: self.phase_max_us,
            decode_successes: self.decode_successes,
            decode_failures: self.decode_failures,
            unique_decoded: self.unique_decoded.len(),
            queue_depth: self.scale_queue_depth,
            queue_max: self.scale_queue_max,
        };
        Ok(self.stats)
    }

    fn begin_streaming(&mut self) {
        let width = self.geometry.width();
        let height = self.geometry.height();
        for layer_index in (0..SPEED_COUNT).rev() {
            let speed = MIN_TILE_SPEED + layer_index;
            let velocity_fp = card_velocity_fp(layer_index, width);
            let (tile_width, _, _) = depth_style(speed, height);
            let interval_frames = layer_interval_frames(
                width,
                tile_width,
                velocity_fp,
                self.layer_targets[layer_index],
            );
            self.layers[layer_index] = LayerSchedule {
                next_spawn_frame: layer_index as u64 * 12,
                interval_frames,
                spawn_count: 0,
                active_sum: 0,
                sample_count: 0,
            };
            let tile_index = self.push_empty_tile(layer_index);
            self.queue_successor(tile_index);
        }
    }

    fn prepare_initial_population(&mut self) -> Result<(), String> {
        let width = self.geometry.width();
        let height = self.geometry.height();
        for (layer_index, target) in self.layer_targets.into_iter().enumerate() {
            let speed = MIN_TILE_SPEED + layer_index;
            let velocity_fp = card_velocity_fp(layer_index, width);
            let (tile_width, _, _) = depth_style(speed, height);
            let interval_frames = layer_interval_frames(width, tile_width, velocity_fp, target);
            let phase = self.random_below(interval_frames as usize) as u64;
            self.layers[layer_index] = LayerSchedule {
                next_spawn_frame: phase,
                interval_frames,
                spawn_count: 0,
                active_sum: 0,
                sample_count: 0,
            };
            for rank in 0..target {
                let tile_index = self.tiles.len();
                let Some(card) = self.prepare_archive_card(tile_index, speed)? else {
                    break;
                };
                let frames_until_exit = phase + rank as u64 * interval_frames;
                let x_fp =
                    width as i64 * PARADE_SUBPIXEL_ONE - frames_until_exit as i64 * velocity_fp;
                let x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
                let y = self
                    .random_tile_y(
                        x,
                        card.raster.width(),
                        card.raster.height(),
                        speed,
                        tile_index,
                    )
                    .unwrap_or(-(card.raster.height() as isize * 2 / 3));
                let active = self.placement_is_clear(
                    x,
                    y,
                    card.raster.width(),
                    card.raster.height(),
                    speed,
                    tile_index,
                );
                self.tiles.push(Tile {
                    x_fp,
                    y,
                    layer: speed,
                    speed,
                    velocity_fp,
                    velocity_remainder: 0,
                    image_index: card.image_index,
                    raster: card.raster,
                    active,
                    raster_held_this_frame: false,
                    raster_moved_this_frame: false,
                    next: None,
                    pending_image_index: None,
                });
            }
        }
        if self.tiles.is_empty() {
            return Err("screenshot archive did not yield a renderable card".to_owned());
        }
        for tile_index in 0..self.tiles.len() {
            self.queue_successor(tile_index);
        }
        Ok(())
    }

    fn prepare_archive_card(
        &mut self,
        tile_index: usize,
        speed: usize,
    ) -> Result<Option<PreparedCard>, String> {
        for _ in 0..self.asset_keys.len() {
            let Some(image_index) = self.next_image_for(tile_index) else {
                return Ok(None);
            };
            self.send_scale_job(tile_index, image_index, speed)?;
            let result = self
                .scale_rx
                .recv()
                .map_err(|_| "screenshot parade scale worker disconnected".to_owned())?;
            self.scale_queue_depth = self.scale_queue_depth.saturating_sub(1);
            match result.card {
                Ok(card) => {
                    self.record_card(&card);
                    return Ok(Some(card));
                }
                Err(_) => {
                    self.decode_failures += 1;
                    self.failed_images.insert(result.image_index);
                }
            }
        }
        Ok(None)
    }

    fn push_empty_tile(&mut self, layer_index: usize) -> usize {
        let speed = MIN_TILE_SPEED + layer_index;
        let raster = PreparedScreenshotCard::prepare(
            &ScreenshotImage::empty(),
            speed,
            self.geometry.height(),
        );
        let tile_index = self.tiles.len();
        self.tiles.push(Tile {
            x_fp: 0,
            y: 0,
            layer: speed,
            speed,
            velocity_fp: card_velocity_fp(layer_index, self.geometry.width()),
            velocity_remainder: 0,
            image_index: usize::MAX,
            raster,
            active: false,
            raster_held_this_frame: false,
            raster_moved_this_frame: false,
            next: None,
            pending_image_index: None,
        });
        tile_index
    }

    fn advance(&mut self, motion_ticks_fp: u64, tick_delta: i64) -> (u128, usize, u128) {
        let nominal_frame = motion_ticks_fp / TICK_ONE as u64;
        let adopt_start = Instant::now();
        let cards_adopted = self.collect_scaled_cards(MAX_CARD_ADOPTIONS_PER_FRAME);
        let card_adopt_us = adopt_start.elapsed().as_micros();
        let advance_start = Instant::now();
        let mut exited = Vec::new();
        for tile_index in 0..self.tiles.len() {
            if self.tiles[tile_index].active {
                let tile = &mut self.tiles[tile_index];
                tile.raster_held_this_frame = false;
                tile.raster_moved_this_frame = false;
                let previous_x_fp = tile.x_fp;
                let previous_phase = raster_phase_key(tile.x_fp);
                let motion = tile
                    .velocity_fp
                    .saturating_mul(tick_delta)
                    .saturating_add(tile.velocity_remainder);
                tile.x_fp = tile.x_fp.saturating_add(motion / TICK_ONE);
                tile.velocity_remainder = motion % TICK_ONE;
                if tile.x_fp != previous_x_fp {
                    if raster_phase_key(tile.x_fp) == previous_phase {
                        tile.raster_held_this_frame = true;
                    } else {
                        tile.raster_moved_this_frame = true;
                    }
                }
                if tile.x() >= self.geometry.width() as isize {
                    tile.active = false;
                    exited.push(tile_index);
                }
            }
        }
        for tile_index in exited {
            self.queue_successor(tile_index);
        }
        for layer_index in 0..SPEED_COUNT {
            if nominal_frame < self.layers[layer_index].next_spawn_frame {
                continue;
            }
            let speed = MIN_TILE_SPEED + layer_index;
            let Some(tile_index) = self
                .tiles
                .iter()
                .position(|tile| tile.layer == speed && !tile.active && tile.next.is_some())
            else {
                continue;
            };
            let next = self.tiles[tile_index]
                .next
                .take()
                .expect("ready card checked");
            let x = -(next.raster.width() as isize);
            let Some(y) = self.random_tile_y(
                x,
                next.raster.width(),
                next.raster.height(),
                speed,
                tile_index,
            ) else {
                self.tiles[tile_index].next = Some(next);
                self.layers[layer_index].next_spawn_frame = nominal_frame + 1;
                continue;
            };
            let tile = &mut self.tiles[tile_index];
            tile.x_fp = x as i64 * PARADE_SUBPIXEL_ONE;
            tile.y = y;
            tile.image_index = next.image_index;
            tile.raster = next.raster;
            tile.active = true;
            tile.velocity_remainder = 0;
            let interval = self.jittered_interval(self.layers[layer_index].interval_frames);
            self.layers[layer_index].next_spawn_frame = nominal_frame + interval;
            self.layers[layer_index].spawn_count += 1;
            let layer_tile_count = self.tiles.iter().filter(|tile| tile.layer == speed).count();
            let has_waiting_tile = self.tiles.iter().any(|tile| {
                tile.layer == speed
                    && !tile.active
                    && (tile.next.is_some() || tile.pending_image_index.is_some())
            });
            if layer_tile_count < self.layer_targets[layer_index] && !has_waiting_tile {
                let new_tile_index = self.push_empty_tile(layer_index);
                self.queue_successor(new_tile_index);
            }
        }
        for layer_index in 0..SPEED_COUNT {
            let speed = MIN_TILE_SPEED + layer_index;
            let active = self
                .tiles
                .iter()
                .filter(|tile| tile.active && tile.layer == speed)
                .count() as u64;
            self.layers[layer_index].active_sum += active;
            self.layers[layer_index].sample_count += 1;
        }
        (
            card_adopt_us,
            cards_adopted,
            advance_start.elapsed().as_micros(),
        )
    }

    fn queue_successor(&mut self, tile_index: usize) {
        if self.tiles[tile_index].next.is_some()
            || self.tiles[tile_index].pending_image_index.is_some()
        {
            return;
        }
        let Some(image_index) = self.next_image_for(tile_index) else {
            return;
        };
        let speed = self.tiles[tile_index].speed;
        self.tiles[tile_index].pending_image_index = Some(image_index);
        if self.send_scale_job(tile_index, image_index, speed).is_err() {
            self.scale_worker_connected = false;
            self.tiles[tile_index].pending_image_index = None;
        }
    }

    fn send_scale_job(
        &mut self,
        tile_index: usize,
        image_index: usize,
        speed: usize,
    ) -> Result<(), String> {
        self.scale_tx
            .as_ref()
            .ok_or_else(|| "screenshot parade scale worker stopped".to_owned())?
            .send(ScaleJob {
                tile_index,
                image_index,
                speed,
                screen_height: self.geometry.height(),
            })
            .map_err(|_| "screenshot parade scale worker disconnected".to_owned())?;
        self.scale_queue_depth += 1;
        self.scale_queue_max = self.scale_queue_max.max(self.scale_queue_depth);
        Ok(())
    }

    fn collect_scaled_cards(&mut self, limit: usize) -> usize {
        let mut failed_tiles = Vec::new();
        let mut collected = 0;
        while collected < limit {
            match self.scale_rx.try_recv() {
                Ok(result) => {
                    collected += 1;
                    self.scale_queue_depth = self.scale_queue_depth.saturating_sub(1);
                    match result.card {
                        Ok(card) => {
                            self.record_card(&card);
                            if let Some(tile) = self.tiles.get_mut(result.tile_index) {
                                tile.pending_image_index = None;
                                tile.next = Some(card);
                            }
                        }
                        Err(_) => {
                            self.decode_failures += 1;
                            self.failed_images.insert(result.image_index);
                            if let Some(tile) = self.tiles.get_mut(result.tile_index) {
                                tile.pending_image_index = None;
                            }
                            failed_tiles.push(result.tile_index);
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
        if !self.scale_worker_connected {
            self.scale_queue_depth = 0;
            for tile in &mut self.tiles {
                tile.pending_image_index = None;
            }
        }
        for tile_index in failed_tiles {
            self.queue_successor(tile_index);
        }
        collected
    }

    fn record_card(&mut self, card: &PreparedCard) {
        self.scale_count += 1;
        self.scale_total_us += card.scale_us;
        self.scale_max_us = self.scale_max_us.max(card.scale_us);
        self.phase_count += 1;
        self.phase_total_us += card.phase_us;
        self.phase_max_us = self.phase_max_us.max(card.phase_us);
        self.decode_successes += 1;
        self.unique_decoded.insert(card.image_index);
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
            let already_visible = self.tiles.iter().enumerate().any(|(index, tile)| {
                (index != replacing_tile && tile.active && tile.image_index == candidate)
                    || tile
                        .next
                        .as_ref()
                        .is_some_and(|next| next.image_index == candidate)
                    || tile.pending_image_index == Some(candidate)
            });
            if !already_visible {
                return Some(candidate);
            }
        }
        None
    }

    fn random_tile_y(
        &mut self,
        x: isize,
        tile_width: usize,
        tile_height: usize,
        speed: usize,
        replacing_tile: usize,
    ) -> Option<isize> {
        let min_y = -(tile_height as isize * 2 / 3);
        let max_y = self.geometry.height() as isize - tile_height as isize / 3;
        let span = (max_y - min_y + 1).max(1) as usize;
        for _ in 0..64 {
            let y = min_y + self.random_below(span) as isize;
            if self.placement_is_clear(x, y, tile_width, tile_height, speed, replacing_tile) {
                return Some(y);
            }
        }
        (min_y..=max_y).find(|y| {
            self.placement_is_clear(x, *y, tile_width, tile_height, speed, replacing_tile)
        })
    }

    fn placement_is_clear(
        &self,
        x: isize,
        y: isize,
        tile_width: usize,
        tile_height: usize,
        speed: usize,
        replacing_tile: usize,
    ) -> bool {
        let gap = scale_dimension(REFERENCE_PLACEMENT_GAP, self.geometry.height()) as isize;
        self.tiles.iter().enumerate().all(|(index, tile)| {
            if index == replacing_tile || !tile.active || tile.layer != speed {
                return true;
            }
            x + tile_width as isize + gap <= tile.x()
                || tile.x() + tile.raster.width() as isize + gap <= x
                || y + tile_height as isize + gap <= tile.y
                || tile.y + tile.raster.height() as isize + gap <= y
        })
    }

    fn prepare_draw_order(&mut self) {
        self.draw_order.clear();
        for layer_index in 0..SPEED_COUNT {
            let speed = MIN_TILE_SPEED + layer_index;
            for (tile_index, tile) in self.tiles.iter().enumerate() {
                if tile.active && tile.layer == speed {
                    self.draw_order.push(tile_index);
                }
            }
        }
    }

    fn prepare_visible_draw_order(&mut self) -> (usize, UnionOcclusionStats) {
        self.visible_draw_order.clear();
        self.visible_spans.clear();
        self.depth_coverage.clear();
        for row in &mut self.depth_coverage_rows {
            row.clear();
        }
        let mut culled = 0;
        let mut union_occlusion = UnionOcclusionStats::default();
        for &tile_index in self.draw_order.iter().rev() {
            let tile = &self.tiles[tile_index];
            let Some(draw_bounds) =
                tile_draw_bounds(tile, self.geometry.width(), self.geometry.height())
            else {
                continue;
            };
            if self
                .depth_coverage
                .iter()
                .any(|coverage| coverage.contains(draw_bounds))
            {
                culled += 1;
                continue;
            }
            let span_start = self.visible_spans.len();
            let (measured, restricted) = prepare_union_visibility(
                tile,
                self.geometry.width(),
                self.geometry.height(),
                &self.depth_coverage_rows,
                &mut self.visible_spans,
            );
            union_occlusion.opaque_pixels += measured.opaque_pixels;
            union_occlusion.opaque_rows += measured.opaque_rows;
            union_occlusion.avoidable_pixels += measured.avoidable_pixels;
            union_occlusion.avoidable_rows += measured.avoidable_rows;
            union_occlusion.fully_covered_rows += measured.fully_covered_rows;
            let mut span_end = self.visible_spans.len();
            if restricted && span_end == span_start {
                culled += 1;
                continue;
            }
            if !restricted {
                self.visible_spans.truncate(span_start);
                span_end = span_start;
            }
            self.visible_draw_order.push(VisibleCard {
                tile_index,
                span_start,
                span_end,
                restricted,
            });
            if let Some(opaque_bounds) =
                tile_opaque_bounds(tile, self.geometry.width(), self.geometry.height())
            {
                self.depth_coverage.push(opaque_bounds);
                add_rect_to_union_rows(&mut self.depth_coverage_rows, opaque_bounds);
            }
        }
        self.visible_draw_order.reverse();
        (culled, union_occlusion)
    }

    fn jittered_interval(&mut self, base: u64) -> u64 {
        let variance = (base / 8).max(1);
        let offset = self.random_below((variance * 2 + 1) as usize) as i64 - variance as i64;
        (base as i64 + offset).max(1) as u64
    }

    fn random_below(&mut self, upper: usize) -> usize {
        advance_rng(&mut self.rng) as usize % upper.max(1)
    }

    fn phase_bank_resident_bytes(&self) -> usize {
        self.tiles
            .iter()
            .map(|tile| {
                tile.raster.phase_resident_bytes()
                    + tile
                        .next
                        .as_ref()
                        .map_or(0, |next| next.raster.phase_resident_bytes())
            })
            .sum()
    }

    fn image_cache_resident_bytes(&self) -> usize {
        self.tiles
            .iter()
            .map(|tile| {
                tile.raster.image_resident_bytes()
                    + tile
                        .next
                        .as_ref()
                        .map_or(0, |next| next.raster.image_resident_bytes())
            })
            .sum()
    }
}

impl FramebufferScene for ScreenshotParade {
    type Stats = ScreenshotParadeStats;

    fn geometry(&self) -> SceneGeometry {
        self.geometry
    }

    fn render(
        &mut self,
        target: SceneTarget<'_>,
        clock: SceneClock,
    ) -> Result<Self::Stats, SceneError> {
        if target.geometry() != self.geometry {
            return Err(SceneError::Render(
                "screenshot parade target geometry changed".to_owned(),
            ));
        }
        self.render_at(target.into_pixels(), clock.elapsed)
            .map_err(SceneError::Render)
    }

    fn invalidate_buffer(&mut self, _buffer: SceneBufferId) {}
}

fn run_scale_worker(
    mut archive: ResidentPreviewArchive,
    jobs: Receiver<ScaleJob>,
    results: Sender<ScaleResult>,
    preparation_epoch: Arc<AtomicU32>,
    preparation_stage: Arc<AtomicU8>,
    preparation_slack: Option<Arc<PreparationSlack>>,
) {
    while let Ok(job) = jobs.recv() {
        preparation_stage.store(1, Ordering::Relaxed);
        preparation_epoch.fetch_add(1, Ordering::Relaxed);
        let started = Instant::now();
        let pixels = if let Some(slack) = preparation_slack.as_deref() {
            let decode = slack.begin_decode();
            let pixels = archive.load_pixels_at(job.image_index);
            drop(decode);
            pixels
        } else {
            archive.load_pixels_at(job.image_index)
        };
        let card = pixels.map(|pixels| {
            preparation_stage.store(2, Ordering::Relaxed);
            if let Some(slack) = preparation_slack.as_deref() {
                slack.checkpoint();
            }
            let source = ScreenshotImage::from_preview(pixels);
            let (raster, phase_us) = PreparedScreenshotCard::prepare_timed(
                &source,
                job.speed,
                job.screen_height,
                preparation_slack.as_deref(),
            );
            PreparedCard {
                image_index: job.image_index,
                raster,
                scale_us: started.elapsed().as_micros(),
                phase_us,
            }
        });
        if let Some(slack) = preparation_slack.as_deref() {
            slack.finish_preparation();
        }
        preparation_stage.store(0, Ordering::Relaxed);
        preparation_epoch.fetch_add(1, Ordering::Relaxed);
        if results
            .send(ScaleResult {
                tile_index: job.tile_index,
                image_index: job.image_index,
                card,
            })
            .is_err()
        {
            break;
        }
    }
}

impl Drop for ScreenshotParade {
    fn drop(&mut self) {
        if let Some(slack) = self.preparation_slack.as_deref() {
            slack.cancel();
        }
        self.scale_tx.take();
        if let Some(worker) = self.scale_worker.take() {
            let _ = worker.join();
        }
    }
}

fn layer_targets(width: usize, height: usize) -> [usize; SPEED_COUNT] {
    if width.saturating_mul(3) <= height.saturating_mul(4) {
        COMPACT_LAYER_TARGETS
    } else {
        WIDE_LAYER_TARGETS
    }
}

fn scale_dimension(reference: usize, screen_height: usize) -> usize {
    reference
        .saturating_mul(screen_height)
        .saturating_add(REFERENCE_HEIGHT / 2)
        .checked_div(REFERENCE_HEIGHT)
        .unwrap_or(1)
        .max(1)
}

fn card_velocity_fp(layer_index: usize, screen_width: usize) -> i64 {
    let reference_1080p = (layer_index as i64 + 1) * PARADE_SUBPIXEL_ONE;
    let slowed_reference = reference_1080p
        .saturating_mul(CARD_SPEED_NUMERATOR)
        .checked_div(CARD_SPEED_DENOMINATOR)
        .unwrap_or(1);
    slowed_reference
        .saturating_mul(screen_width as i64)
        .saturating_add((REFERENCE_WIDTH / 2) as i64)
        .checked_div(REFERENCE_WIDTH as i64)
        .unwrap_or(1)
        .max(1)
}

fn tick_delta_fp(elapsed: Duration) -> i64 {
    let ticks = elapsed
        .as_nanos()
        .saturating_mul(u128::from(REFERENCE_HZ))
        .saturating_mul(TICK_ONE as u128)
        / 1_000_000_000_u128;
    ticks.min(i64::MAX as u128) as i64
}

fn layer_interval_frames(
    screen_width: usize,
    tile_width: usize,
    velocity_fp: i64,
    target_count: usize,
) -> u64 {
    let travel_fp = (screen_width + tile_width) as i64 * PARADE_SUBPIXEL_ONE;
    let velocity_fp = velocity_fp.max(1);
    let travel_frames = ((travel_fp + velocity_fp - 1) / velocity_fp) as usize;
    (travel_frames / target_count.max(1)).max(1) as u64
}

fn render_background(
    pixels: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    motion_ticks_fp: u64,
) {
    pixels.fill(color565(0, 0, 10));
    for star in 0..210_usize {
        let layer = star & 3;
        let (x, fraction) = horizontal_star_position(star, width, height, motion_ticks_fp);
        let y = (star
            .wrapping_mul(83)
            .wrapping_add(star.wrapping_mul(star) * 7))
            % height;
        let brightness = [70, 110, 170, 235][layer];
        let color = color565(brightness / 2, brightness, 255);
        let row = y * width;
        pixels[row + x] = blend_565(pixels[row + x], color, 255 - fraction);
        if fraction > 0 {
            let next_x = (x + 1) % width;
            pixels[row + next_x] = blend_565(pixels[row + next_x], color, fraction);
        }
    }
}

fn horizontal_star_position(
    star: usize,
    width: usize,
    screen_height: usize,
    motion_ticks_fp: u64,
) -> (usize, u8) {
    const STAR_SPEED_DENOMINATOR: u64 = 16;
    const SUBPIXEL_ONE: u64 = 256;
    let speed_numerator = MIN_TILE_SPEED as u64 * ((star & 3) + 1) as u64;
    let start_x = (star
        .wrapping_mul(197)
        .wrapping_add(star.wrapping_mul(star) * 13))
        % width;
    let scaled_ticks_fp = motion_ticks_fp
        .saturating_mul(screen_height as u64)
        .saturating_add((REFERENCE_HEIGHT / 2) as u64)
        / REFERENCE_HEIGHT as u64;
    let travel = scaled_ticks_fp
        .saturating_mul(speed_numerator)
        .saturating_mul(SUBPIXEL_ONE)
        / (STAR_SPEED_DENOMINATOR * TICK_ONE as u64);
    let position = (start_x as u64 * SUBPIXEL_ONE + travel) % (width as u64 * SUBPIXEL_ONE);
    (
        (position / SUBPIXEL_ONE) as usize,
        (position % SUBPIXEL_ONE) as u8,
    )
}

fn raster_phase_key(x_fp: i64) -> i64 {
    let x = x_fp.div_euclid(PARADE_SUBPIXEL_ONE);
    let fraction = x_fp.rem_euclid(PARADE_SUBPIXEL_ONE);
    let mut phase = (fraction + 8) / 16;
    let mut origin = x;
    if phase == 16 {
        origin += 1;
        phase = 0;
    }
    origin * 16 + phase
}

fn clipped_rect(
    x: isize,
    y: isize,
    width: usize,
    height: usize,
    screen_width: usize,
    screen_height: usize,
) -> Option<Rect> {
    let x0 = x.clamp(0, screen_width as isize) as usize;
    let y0 = y.clamp(0, screen_height as isize) as usize;
    let x1 = x
        .saturating_add(width as isize)
        .clamp(0, screen_width as isize) as usize;
    let y1 = y
        .saturating_add(height as isize)
        .clamp(0, screen_height as isize) as usize;
    (x1 > x0 && y1 > y0).then_some(Rect { x0, y0, x1, y1 })
}

fn tile_draw_bounds(tile: &Tile, screen_width: usize, screen_height: usize) -> Option<Rect> {
    let x = tile.x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
    let width = tile.raster.width().saturating_add(1);
    clipped_rect(
        x,
        tile.y,
        width,
        tile.raster.height(),
        screen_width,
        screen_height,
    )
}

fn tile_opaque_bounds(tile: &Tile, screen_width: usize, screen_height: usize) -> Option<Rect> {
    let x = tile.x_fp.div_euclid(PARADE_SUBPIXEL_ONE) as isize;
    let inset = tile.raster.max_corner_inset().saturating_add(1);
    let width = tile.raster.width().saturating_sub(inset.saturating_mul(2));
    clipped_rect(
        x.saturating_add(inset as isize),
        tile.y,
        width,
        tile.raster.height(),
        screen_width,
        screen_height,
    )
}

fn prepare_union_visibility(
    tile: &Tile,
    screen_width: usize,
    screen_height: usize,
    coverage_rows: &[Vec<(usize, usize)>],
    spans: &mut Vec<VisibleSpan>,
) -> (UnionOcclusionStats, bool) {
    let mut stats = UnionOcclusionStats::default();
    let mut restricted = false;
    tile.raster.visit_target_rows(
        screen_width,
        screen_height,
        tile.x_fp,
        tile.y,
        |source_y, target_y, source_x0, source_x1, target_x0, target_x1, opaque| {
            let row = &coverage_rows[target_y];
            if let Some((opaque_x0, opaque_x1)) = opaque {
                let opaque_pixels = opaque_x1 - opaque_x0;
                let covered = row
                    .iter()
                    .map(|&(start, end)| end.min(opaque_x1).saturating_sub(start.max(opaque_x0)))
                    .sum::<usize>();
                stats.opaque_pixels += opaque_pixels;
                stats.opaque_rows += 1;
                stats.avoidable_pixels += covered;
                stats.avoidable_rows += usize::from(covered > 0);
                stats.fully_covered_rows += usize::from(covered == opaque_pixels);
            }
            let mut target_cursor = target_x0;
            for &(start, end) in row {
                if end <= target_x0 {
                    continue;
                }
                if start >= target_x1 {
                    break;
                }
                restricted = true;
                let covered_start = start.max(target_x0);
                let covered_end = end.min(target_x1);
                if covered_start > target_cursor {
                    push_visible_span(
                        spans,
                        source_y,
                        source_x0 + target_cursor - target_x0,
                        source_x0 + covered_start - target_x0,
                    );
                }
                target_cursor = target_cursor.max(covered_end);
                if target_cursor == target_x1 {
                    break;
                }
            }
            if target_cursor < target_x1 {
                push_visible_span(
                    spans,
                    source_y,
                    source_x0 + target_cursor - target_x0,
                    source_x1,
                );
            }
        },
    );
    (stats, restricted)
}

fn add_rect_to_union_rows(rows: &mut [Vec<(usize, usize)>], rect: Rect) {
    for row in &mut rows[rect.y0..rect.y1] {
        insert_union_interval(row, rect.x0, rect.x1);
    }
}

fn insert_union_interval(row: &mut Vec<(usize, usize)>, mut start: usize, mut end: usize) {
    let first = row.partition_point(|&(_, existing_end)| existing_end < start);
    let mut after = first;
    while after < row.len() && row[after].0 <= end {
        start = start.min(row[after].0);
        end = end.max(row[after].1);
        after += 1;
    }
    if first == after {
        row.insert(first, (start, end));
    } else {
        row[first] = (start, end);
        row.drain(first + 1..after);
    }
}

fn push_visible_span(spans: &mut Vec<VisibleSpan>, source_y: usize, start: usize, end: usize) {
    if end <= start {
        return;
    }
    debug_assert!(source_y <= u16::MAX as usize);
    debug_assert!(end <= u16::MAX as usize);
    spans.push(VisibleSpan {
        source_y: source_y as u16,
        start: start as u16,
        end: end as u16,
    });
}

#[cfg(test)]
fn union_covered_pixels(
    y: usize,
    x0: usize,
    x1: usize,
    coverage: &[Rect],
    intervals: &mut Vec<(usize, usize)>,
) -> usize {
    intervals.clear();
    intervals.extend(coverage.iter().filter_map(|rect| {
        if !(rect.y0..rect.y1).contains(&y) {
            return None;
        }
        let start = rect.x0.max(x0);
        let end = rect.x1.min(x1);
        (end > start).then_some((start, end))
    }));
    intervals.sort_unstable_by_key(|interval| interval.0);
    let mut covered = 0;
    let mut merged_end = x0;
    for &(start, end) in intervals.iter() {
        if end <= merged_end {
            continue;
        }
        let uncovered_start = start.max(merged_end);
        covered += end - uncovered_start;
        merged_end = end;
    }
    covered
}

fn color565(r: u8, g: u8, b: u8) -> Rgb565Pixel {
    Rgb565Pixel((u16::from(r) >> 3) << 11 | (u16::from(g) >> 2) << 5 | (u16::from(b) >> 3))
}

fn blend_565(from: Rgb565Pixel, to: Rgb565Pixel, alpha: u8) -> Rgb565Pixel {
    let from = u32::from(from.0);
    let to = u32::from(to.0);
    let alpha = ((u32::from(alpha) + 4) >> 3).min(32);
    if alpha == 0 {
        return Rgb565Pixel(from as u16);
    }
    if alpha >= 32 {
        return Rgb565Pixel(to as u16);
    }
    let inverse = 32 - alpha;
    let rb = (((from & 0xf81f) * inverse + (to & 0xf81f) * alpha) >> 5) & 0xf81f;
    let g = (((from & 0x07e0) * inverse + (to & 0x07e0) * alpha) >> 5) & 0x07e0;
    Rgb565Pixel((rb | g) as u16)
}

fn shuffle<T>(values: &mut [T], rng: &mut u64) {
    for index in (1..values.len()).rev() {
        let other = advance_rng(rng) as usize % (index + 1);
        values.swap(index, other);
    }
}

fn advance_rng(rng: &mut u64) -> u64 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 7;
    *rng ^= *rng << 17;
    *rng
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write_archive(path: &Path, count: usize) {
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
            let width = 4_u32;
            let height = 4_u32;
            let stride_bytes = 8_u32;
            let data_len = 32_u32;
            bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&width.to_le_bytes());
            bytes.extend_from_slice(&height.to_le_bytes());
            bytes.extend_from_slice(&stride_bytes.to_le_bytes());
            bytes.extend_from_slice(&data_len.to_le_bytes());
            bytes.push(1);
            bytes.extend_from_slice(&data_len.to_le_bytes());
            bytes.extend_from_slice(&offset.to_le_bytes());
            bytes.extend_from_slice(name.as_bytes());
            offset += u64::from(data_len);
        }
        for index in 0..count {
            for pixel in 0..16_u16 {
                let value = (index as u16).wrapping_mul(97).wrapping_add(pixel * 31);
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        std::fs::write(path, bytes).expect("write screenshot parade fixture");
    }

    fn prepared_scene(path: &Path, width: usize, height: usize) -> ScreenshotParade {
        let archive = ResidentPreviewArchive::open(path).expect("open fixture archive");
        ScreenshotParade::new_offline_prepared(
            archive,
            ScreenshotParadeConfig {
                geometry: SceneGeometry::new(width, height, width).unwrap(),
                seed: 0x4d61_6769_4b54_696c,
                worker_start: None,
                preparation_slack: None,
            },
        )
        .expect("prepare screenshot parade")
    }

    #[test]
    fn card_velocities_use_the_slowed_1080p_reference() {
        for layer_index in 0..SPEED_COUNT {
            assert_eq!(
                card_velocity_fp(layer_index, REFERENCE_WIDTH),
                (layer_index as i64 + 1) * 3 * PARADE_SUBPIXEL_ONE / 4
            );
        }
    }

    #[test]
    fn card_velocities_scale_only_with_framebuffer_width() {
        for layer_index in 0..SPEED_COUNT {
            let reference = card_velocity_fp(layer_index, REFERENCE_WIDTH);
            assert_eq!(card_velocity_fp(layer_index, 960) * 2, reference);
            assert_eq!(card_velocity_fp(layer_index, 640) * 3, reference);
        }
    }

    #[test]
    fn supported_framebuffer_velocities_land_on_the_sixteenth_phase_lattice() {
        for width in [960, 640] {
            for layer_index in 0..SPEED_COUNT {
                assert_eq!(card_velocity_fp(layer_index, width) % 16, 0);
            }
        }
    }

    #[test]
    fn prepared_frames_are_deterministic_for_supported_geometry() {
        let path = std::env::temp_dir().join(format!(
            "screenshot-parade-schedule-{}.mmlz4b",
            std::process::id()
        ));
        write_archive(&path, 220);
        for (width, height) in [(960, 540), (640, 480)] {
            let mut first = prepared_scene(&path, width, height);
            let mut second = prepared_scene(&path, width, height);
            let mut first_pixels = vec![Rgb565Pixel(0); width * height];
            let mut second_pixels = vec![Rgb565Pixel(0); width * height];
            for milliseconds in [0_u64, 17, 250, 1_000, 2_000] {
                let elapsed = Duration::from_millis(milliseconds);
                first.render_at(&mut first_pixels, elapsed).unwrap();
                second.render_at(&mut second_pixels, elapsed).unwrap();
                assert_eq!(first_pixels, second_pixels, "time={milliseconds}");
            }
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn streaming_starts_empty_and_reports_pending_work() {
        let path = std::env::temp_dir().join(format!(
            "screenshot-parade-streaming-{}.mmlz4b",
            std::process::id()
        ));
        write_archive(&path, 16);
        let archive = ResidentPreviewArchive::open(&path).unwrap();
        let scene = ScreenshotParade::new(
            archive,
            ScreenshotParadeConfig {
                geometry: SceneGeometry::new(320, 180, 320).unwrap(),
                seed: 7,
                worker_start: None,
                preparation_slack: None,
            },
        )
        .unwrap();
        assert!(!scene.is_ready());
        assert!(scene.has_pending_work());
        assert_eq!(scene.tiles.len(), SPEED_COUNT);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn elapsed_time_must_be_monotonic() {
        let path = std::env::temp_dir().join(format!(
            "screenshot-parade-clock-{}.mmlz4b",
            std::process::id()
        ));
        write_archive(&path, 220);
        let mut scene = prepared_scene(&path, 320, 180);
        let mut pixels = vec![Rgb565Pixel(0); 320 * 180];
        scene
            .render_at(&mut pixels, Duration::from_secs(1))
            .unwrap();
        assert!(
            scene
                .render_at(&mut pixels, Duration::from_millis(999))
                .is_err()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn presentation_ticks_advance_by_exact_sixtieths() {
        let path = std::env::temp_dir().join(format!(
            "screenshot-parade-presentation-clock-{}.mmlz4b",
            std::process::id()
        ));
        write_archive(&path, 220);
        let mut scene = prepared_scene(&path, 960, 540);
        let mut pixels = vec![Rgb565Pixel(0); 960 * 540];
        scene.render_at_presentation_tick(&mut pixels, 0).unwrap();
        let tile_index = scene
            .tiles
            .iter()
            .position(|tile| {
                tile.active
                    && tile.x_fp.saturating_add(tile.velocity_fp * 16) < 960 * PARADE_SUBPIXEL_ONE
            })
            .unwrap();
        let velocity_fp = scene.tiles[tile_index].velocity_fp;
        let mut previous_x = scene.tiles[tile_index].x_fp;
        for tick in 1..=16 {
            scene
                .render_at_presentation_tick(&mut pixels, tick)
                .unwrap();
            let x = scene.tiles[tile_index].x_fp;
            assert_eq!(x - previous_x, velocity_fp, "tick={tick}");
            previous_x = x;
        }
        assert_eq!(scene.previous_motion_ticks_fp, 16 * TICK_ONE as u64);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn presentation_tick_skips_advance_every_confirmed_interval() {
        let path = std::env::temp_dir().join(format!(
            "screenshot-parade-presentation-skip-{}.mmlz4b",
            std::process::id()
        ));
        write_archive(&path, 220);
        let mut scene = prepared_scene(&path, 960, 540);
        let mut pixels = vec![Rgb565Pixel(0); 960 * 540];
        scene.render_at_presentation_tick(&mut pixels, 0).unwrap();
        let tile_index = scene
            .tiles
            .iter()
            .position(|tile| {
                tile.active
                    && tile.x_fp.saturating_add(tile.velocity_fp * 16) < 960 * PARADE_SUBPIXEL_ONE
            })
            .unwrap();
        let velocity_fp = scene.tiles[tile_index].velocity_fp;
        let starting_x = scene.tiles[tile_index].x_fp;
        scene.render_at_presentation_tick(&mut pixels, 4).unwrap();
        assert_eq!(scene.tiles[tile_index].x_fp - starting_x, velocity_fp * 4);
        assert_eq!(scene.previous_motion_ticks_fp, 4 * TICK_ONE as u64);
        assert!(scene.render_at_presentation_tick(&mut pixels, 3).is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn culling_bounds_are_conservative_and_phase_independent() {
        let raster = PreparedScreenshotCard::prepare(
            &ScreenshotImage {
                pixels: vec![Rgb565Pixel(0xffff); 32 * 24],
                width: 32,
                height: 24,
                stride: 32,
            },
            5,
            135,
        );
        let mut tile = Tile {
            x_fp: 20 * PARADE_SUBPIXEL_ONE,
            y: 10,
            layer: 5,
            speed: 5,
            velocity_fp: PARADE_SUBPIXEL_ONE,
            velocity_remainder: 0,
            image_index: 0,
            raster,
            active: true,
            raster_held_this_frame: false,
            raster_moved_this_frame: false,
            next: None,
            pending_image_index: None,
        };
        let expected_draw = tile_draw_bounds(&tile, 320, 180);
        let expected_opaque = tile_opaque_bounds(&tile, 320, 180);
        for fraction in [8, 16, 128, 240, 255] {
            tile.x_fp = 20 * PARADE_SUBPIXEL_ONE + fraction;
            assert_eq!(tile_draw_bounds(&tile, 320, 180), expected_draw);
            assert_eq!(tile_opaque_bounds(&tile, 320, 180), expected_opaque);
        }
        let draw = expected_draw.unwrap();
        assert_eq!(draw.x0, 20);
        assert_eq!(draw.x1 - draw.x0, tile.raster.width() + 1);
    }

    #[test]
    fn union_coverage_merges_disjoint_overlapping_and_joint_intervals() {
        let mut intervals = Vec::new();
        let coverage = [
            Rect {
                x0: 2,
                y0: 4,
                x1: 7,
                y1: 8,
            },
            Rect {
                x0: 5,
                y0: 4,
                x1: 11,
                y1: 8,
            },
            Rect {
                x0: 13,
                y0: 4,
                x1: 18,
                y1: 8,
            },
        ];
        assert_eq!(
            union_covered_pixels(5, 0, 20, &coverage, &mut intervals),
            14
        );
        assert_eq!(union_covered_pixels(3, 0, 20, &coverage, &mut intervals), 0);
        assert_eq!(union_covered_pixels(5, 6, 14, &coverage, &mut intervals), 7);

        let joint = [
            Rect {
                x0: 0,
                y0: 0,
                x1: 5,
                y1: 1,
            },
            Rect {
                x0: 5,
                y0: 0,
                x1: 10,
                y1: 1,
            },
        ];
        assert_eq!(union_covered_pixels(0, 0, 10, &joint, &mut intervals), 10);

        let mut row = Vec::new();
        insert_union_interval(&mut row, 12, 18);
        insert_union_interval(&mut row, 2, 7);
        insert_union_interval(&mut row, 6, 14);
        insert_union_interval(&mut row, 18, 21);
        assert_eq!(row, vec![(2, 21)]);
    }

    #[test]
    fn union_diagnostic_handles_rounded_phases_and_clipped_edges() {
        let raster = PreparedScreenshotCard::prepare(
            &ScreenshotImage {
                pixels: vec![Rgb565Pixel(0xffff); 32 * 24],
                width: 32,
                height: 24,
                stride: 32,
            },
            5,
            135,
        );
        let coverage = [Rect {
            x0: 0,
            y0: 0,
            x1: 20,
            y1: 18,
        }];
        let mut intervals = Vec::new();
        for phase in 0..16 {
            let tile = Tile {
                x_fp: -8 * PARADE_SUBPIXEL_ONE + phase * 16,
                y: -3,
                layer: 5,
                speed: 5,
                velocity_fp: PARADE_SUBPIXEL_ONE,
                velocity_remainder: 0,
                image_index: 0,
                raster: raster.clone(),
                active: true,
                raster_held_this_frame: false,
                raster_moved_this_frame: false,
                next: None,
                pending_image_index: None,
            };
            let stats = measure_union_occlusion(&tile, 48, 36, &coverage, &mut intervals);
            assert!(stats.opaque_pixels > 0, "phase={phase}");
            assert!(stats.opaque_rows > 0, "phase={phase}");
            assert!(
                stats.avoidable_pixels <= stats.opaque_pixels,
                "phase={phase}"
            );
            assert!(stats.avoidable_rows <= stats.opaque_rows, "phase={phase}");
            assert!(
                stats.fully_covered_rows <= stats.avoidable_rows,
                "phase={phase}"
            );
        }
    }
}
