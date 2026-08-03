// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Particle sequences retained for the future startup animation.
//!
//! The `particle_renderer` module owns the CRT-noise-to-3D `MagiK` sequence.
//! This module preserves the matching arcade-cabinet formation without pulling
//! the archived particle showcase into the production application.

use crate::recipes::{
    CabinetModel, CabinetRecipe, RecipeEasing, RecipeRgb565, embedded_cabinet_recipe,
};
use mister_magik_framebuffer_scenes::{
    FramebufferScene, SceneBufferId, SceneClock, SceneError, SceneGeometry, SceneTarget,
};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const ARCADE_CLOUD_POINT_COUNT: usize = 72_704;
const ARCADE_CLOUD: &[u8] = include_bytes!("../assets/cabinet/arcade-cabinet.pcloud");
const PARTICLE_CLOUD_MAGIC: &[u8; 8] = b"PCLOUD1\0";
const PARTICLE_CLOUD_HEADER_BYTES: usize = 28;
const PARTICLE_CLOUD_RECORD_BYTES: usize = 8;
const ARCADE_DEMO_NUMBER: u64 = 21;
const FULL_RATE_PARTICLE_LIMIT: usize = 48_128;
const TWO_WAY_PARTICLE_LIMIT: usize = 72_192;

pub use mister_magik_framebuffer_scenes::Rgb565Pixel;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArcadeCabinetFrameStats {
    pub particles: usize,
    pub projected_particles: usize,
    pub projection_cohorts: u8,
    pub visible: usize,
    pub pixel_writes: usize,
    pub projection_backend: &'static str,
    pub stages: CabinetStageTimings,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CabinetStageTimings {
    pub clear_us: u64,
    pub projection_us: u64,
    pub ordering_us: u64,
    pub raster_us: u64,
    pub worker_wait_us: u64,
    pub prepared_age_us: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CabinetCreativeMode {
    #[default]
    Baseline,
    Satellites,
    HistoryEcho,
    DepthPalette,
    MicroJitter,
    All,
}

impl CabinetCreativeMode {
    pub const ALL: [Self; 6] = [
        Self::Baseline,
        Self::Satellites,
        Self::HistoryEcho,
        Self::DepthPalette,
        Self::MicroJitter,
        Self::All,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Baseline => "BASELINE",
            Self::Satellites => "SATELLITES",
            Self::HistoryEcho => "HISTORY ECHO",
            Self::DepthPalette => "DEPTH PALETTE",
            Self::MicroJitter => "MICRO-JITTER",
            Self::All => "ALL",
        }
    }

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    const fn uses_satellites(self) -> bool {
        matches!(self, Self::Satellites | Self::All)
    }

    const fn uses_history_echo(self) -> bool {
        matches!(self, Self::HistoryEcho | Self::All)
    }

    const fn uses_depth_palette(self) -> bool {
        matches!(self, Self::DepthPalette | Self::All)
    }

    const fn uses_micro_jitter(self) -> bool {
        matches!(self, Self::MicroJitter | Self::All)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CabinetRenderOptions {
    pub active_count: usize,
    pub creative_mode: CabinetCreativeMode,
}

const PARTICLE_LANES: usize = 4;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct CabinetPositionBlock {
    target_x: [f32; PARTICLE_LANES],
    target_y: [f32; PARTICLE_LANES],
    target_z: [f32; PARTICLE_LANES],
    source_x: [f32; PARTICLE_LANES],
    source_y: [f32; PARTICLE_LANES],
    source_z: [f32; PARTICLE_LANES],
}

#[repr(C, align(16))]
struct CabinetAttributeBlock {
    random: [u32; PARTICLE_LANES],
    life: [f32; PARTICLE_LANES],
    style: [u8; PARTICLE_LANES],
    flags: [u8; PARTICLE_LANES],
}

const INVALID_PARTICLE_OFFSET: u32 = u32::MAX;
const COMMAND_OFFSET_MASK: u32 = (1 << 20) - 1;
const COMMAND_DEPTH_SHIFT: u32 = 20;

#[repr(transparent)]
#[derive(Clone, Copy)]
struct CabinetDrawCommand(u32);

impl CabinetDrawCommand {
    fn visible(offset: usize, depth: f32) -> Self {
        let depth_band =
            u32::from(depth >= 480.0) + u32::from(depth >= 640.0) + u32::from(depth >= 800.0);
        Self((offset as u32) | (depth_band << COMMAND_DEPTH_SHIFT))
    }

    fn offset(self) -> Option<usize> {
        (self.0 != INVALID_PARTICLE_OFFSET).then_some((self.0 & COMMAND_OFFSET_MASK) as usize)
    }

    fn depth_band(self) -> u8 {
        ((self.0 >> COMMAND_DEPTH_SHIFT) & 3) as u8
    }
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
unsafe extern "C" {
    fn mister_magik_cabinet_neon_project_stable(
        count: usize,
        blocks: *const CabinetPositionBlock,
        first_block: usize,
        block_step: usize,
        sin_yaw: f32,
        cos_yaw: f32,
        sin_pitch: f32,
        cos_pitch: f32,
        dolly: f32,
        near_depth: f32,
        focal_length: f32,
        center_x: f32,
        center_y: f32,
        width: u32,
        height: u32,
        offsets: *mut u32,
    ) -> usize;
}

/// Exact extraction of the approved arcade-cabinet particle formation.
pub struct ArcadeCabinetFormation {
    width: usize,
    height: usize,
    recipe: CabinetRecipe,
    capacity: usize,
    positions: Vec<CabinetPositionBlock>,
    dynamic_positions: Vec<CabinetPositionBlock>,
    attributes: Vec<CabinetAttributeBlock>,
    commands: Vec<CabinetDrawCommand>,
    previous_commands: Vec<CabinetDrawCommand>,
    dirty_offsets: [Vec<u32>; 2],
    full_clear: [bool; 2],
    projection_frame: u64,
    commands_initialized: bool,
    options: CabinetRenderOptions,
}

pub struct CabinetScene {
    pipeline: CabinetPreparationPipeline,
    geometry: SceneGeometry,
    reusable_buffers: u8,
    capacity: usize,
    options: CabinetRenderOptions,
}

struct CabinetPreparationRequest {
    tick: u64,
    elapsed: Duration,
    buffer_id: usize,
    options: CabinetRenderOptions,
    pixels: Vec<Rgb565Pixel>,
}

struct PreparedCabinetFrame {
    tick: u64,
    elapsed: Duration,
    buffer_id: usize,
    options: CabinetRenderOptions,
    completed_at: Instant,
    pixels: Vec<Rgb565Pixel>,
    stats: ArcadeCabinetFrameStats,
}

struct CabinetPreparationPipeline {
    request_tx: Option<SyncSender<CabinetPreparationRequest>>,
    ready_rx: Receiver<Result<PreparedCabinetFrame, String>>,
    worker: Option<JoinHandle<()>>,
    spare_pixels: [Option<Vec<Rgb565Pixel>>; 2],
    pending: bool,
    tick: u64,
    reusable_buffers: u8,
}

impl CabinetScene {
    pub fn new(
        width: usize,
        height: usize,
        recipe: CabinetRecipe,
        reusable_buffers: u8,
    ) -> Result<Self, String> {
        let capacity = recipe.particle_count;
        Self::new_with_capacity(width, height, recipe, reusable_buffers, capacity)
    }

    pub fn new_with_capacity(
        width: usize,
        height: usize,
        recipe: CabinetRecipe,
        reusable_buffers: u8,
        capacity: usize,
    ) -> Result<Self, String> {
        if reusable_buffers == 0 {
            return Err("cabinet scene requires at least one reusable buffer".into());
        }
        let geometry =
            SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
        let formation = ArcadeCabinetFormation::new_with_capacity(width, height, recipe, capacity)?;
        let options = formation.render_options();
        Ok(Self {
            pipeline: CabinetPreparationPipeline::start(formation, reusable_buffers)?,
            geometry,
            reusable_buffers,
            capacity,
            options,
        })
    }

    pub fn set_render_options(&mut self, options: CabinetRenderOptions) -> Result<(), String> {
        if options.active_count == 0 || options.active_count > self.capacity {
            return Err(format!(
                "cabinet active count {} is outside 1..={}",
                options.active_count, self.capacity
            ));
        }
        self.options = options;
        Ok(())
    }

    #[must_use]
    pub const fn render_options(&self) -> CabinetRenderOptions {
        self.options
    }

    pub fn from_embedded(
        width: usize,
        height: usize,
        reusable_buffers: u8,
    ) -> Result<Self, String> {
        Self::new(width, height, embedded_cabinet_recipe()?, reusable_buffers)
    }
}

impl FramebufferScene for CabinetScene {
    type Stats = ArcadeCabinetFrameStats;

    fn geometry(&self) -> SceneGeometry {
        self.geometry
    }

    fn render(
        &mut self,
        target: SceneTarget<'_>,
        clock: SceneClock,
    ) -> Result<Self::Stats, SceneError> {
        if target.geometry() != self.geometry {
            return Err(SceneError::Render(format!(
                "cabinet target geometry {:?} does not match scene {:?}",
                target.geometry(),
                self.geometry
            )));
        }
        if target.buffer_id().get() >= self.reusable_buffers {
            return Err(SceneError::InvalidBufferId {
                value: target.buffer_id().get(),
                reusable_buffers: self.reusable_buffers,
            });
        }
        let buffer_id = usize::from(target.buffer_id().get());
        self.pipeline
            .acquire(
                target.into_pixels(),
                buffer_id,
                clock.elapsed,
                clock.next_elapsed,
                self.options,
            )
            .map_err(SceneError::Render)
    }

    fn invalidate_buffer(&mut self, _buffer: SceneBufferId) {
        // Cabinet clears the complete target on every frame.
    }
}

impl CabinetPreparationPipeline {
    fn start(formation: ArcadeCabinetFormation, reusable_buffers: u8) -> Result<Self, String> {
        if reusable_buffers == 0 || reusable_buffers > 2 {
            return Err("cabinet preparation supports one or two reusable buffers".into());
        }
        let frame_len = formation.width.saturating_mul(formation.height);
        let background = pixel(formation.recipe.appearance.background);
        let (request_tx, request_rx) = sync_channel::<CabinetPreparationRequest>(1);
        let (ready_tx, ready_rx) = sync_channel::<Result<PreparedCabinetFrame, String>>(1);
        let worker = thread::Builder::new()
            .name("cabinet-prepare".into())
            .spawn(move || run_cabinet_preparation_worker(formation, request_rx, ready_tx))
            .map_err(|error| format!("start cabinet preparation worker: {error}"))?;
        Ok(Self {
            request_tx: Some(request_tx),
            ready_rx,
            worker: Some(worker),
            spare_pixels: [
                Some(vec![background; frame_len]),
                (reusable_buffers > 1).then(|| vec![background; frame_len]),
            ],
            pending: false,
            tick: 0,
            reusable_buffers,
        })
    }

    fn dispatch(
        &mut self,
        tick: u64,
        elapsed: Duration,
        buffer_id: usize,
        options: CabinetRenderOptions,
    ) -> Result<(), String> {
        let pixels = self.spare_pixels[buffer_id]
            .take()
            .ok_or_else(|| format!("cabinet buffer {buffer_id} is already in flight"))?;
        self.request_tx
            .as_ref()
            .ok_or("cabinet preparation worker has stopped")?
            .send(CabinetPreparationRequest {
                tick,
                elapsed,
                buffer_id,
                options,
                pixels,
            })
            .map_err(|_| "cabinet preparation worker disconnected".to_string())?;
        self.pending = true;
        Ok(())
    }

    fn receive(&mut self) -> Result<PreparedCabinetFrame, String> {
        let prepared = self
            .ready_rx
            .recv()
            .map_err(|_| "cabinet preparation worker disconnected".to_string())??;
        self.pending = false;
        Ok(prepared)
    }

    fn recycle(&mut self, prepared: PreparedCabinetFrame) {
        self.spare_pixels[prepared.buffer_id] = Some(prepared.pixels);
    }

    fn acquire(
        &mut self,
        destination: &mut [Rgb565Pixel],
        buffer_id: usize,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
        options: CabinetRenderOptions,
    ) -> Result<ArcadeCabinetFrameStats, String> {
        let wait_started = Instant::now();
        if !self.pending {
            self.dispatch(self.tick, elapsed, buffer_id, options)?;
        }
        let mut prepared = self.receive()?;
        if prepared.tick != self.tick
            || prepared.elapsed != elapsed
            || prepared.buffer_id != buffer_id
            || prepared.options != options
        {
            self.recycle(prepared);
            self.dispatch(self.tick, elapsed, buffer_id, options)?;
            prepared = self.receive()?;
        }
        let worker_wait_us = elapsed_us(wait_started.elapsed());
        let prepared_age_us = elapsed_us(prepared.completed_at.elapsed());
        if destination.len() != prepared.pixels.len() {
            return Err("prepared cabinet frame geometry changed".into());
        }
        destination.copy_from_slice(&prepared.pixels);
        let mut stats = prepared.stats;
        stats.stages.worker_wait_us = worker_wait_us;
        stats.stages.prepared_age_us = prepared_age_us;
        self.recycle(prepared);
        self.tick = self.tick.wrapping_add(1);
        if let Some(next_elapsed) = next_elapsed {
            let next_buffer = if self.reusable_buffers > 1 {
                1 - buffer_id
            } else {
                0
            };
            self.dispatch(self.tick, next_elapsed, next_buffer, options)?;
        }
        Ok(stats)
    }
}

impl Drop for CabinetPreparationPipeline {
    fn drop(&mut self) {
        self.request_tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_cabinet_preparation_worker(
    mut formation: ArcadeCabinetFormation,
    request_rx: Receiver<CabinetPreparationRequest>,
    ready_tx: SyncSender<Result<PreparedCabinetFrame, String>>,
) {
    while let Ok(mut request) = request_rx.recv() {
        let result = formation
            .set_render_options(request.options)
            .and_then(|()| {
                formation.render(&mut request.pixels, request.elapsed, request.buffer_id)
            })
            .map(|stats| PreparedCabinetFrame {
                tick: request.tick,
                elapsed: request.elapsed,
                buffer_id: request.buffer_id,
                options: request.options,
                completed_at: Instant::now(),
                pixels: request.pixels,
                stats,
            });
        let failed = result.is_err();
        if ready_tx.send(result).is_err() || failed {
            break;
        }
    }
}

impl ArcadeCabinetFormation {
    pub fn from_embedded(width: usize, height: usize) -> Result<Self, String> {
        Self::new(width, height, embedded_cabinet_recipe()?)
    }

    pub fn new(width: usize, height: usize, recipe: CabinetRecipe) -> Result<Self, String> {
        let capacity = recipe.particle_count;
        Self::new_with_capacity(width, height, recipe, capacity)
    }

    pub fn new_with_capacity(
        width: usize,
        height: usize,
        recipe: CabinetRecipe,
        capacity: usize,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("arcade cabinet formation requires a non-empty viewport".into());
        }
        if capacity < recipe.particle_count {
            return Err(format!(
                "arcade cabinet capacity {capacity} is below recipe count {}",
                recipe.particle_count
            ));
        }
        let active_count = recipe.particle_count;
        let mut target_x = vec![0.0; capacity];
        let mut target_y = vec![0.0; capacity];
        let mut target_z = vec![0.0; capacity];
        let mut source_x = vec![0.0; capacity];
        let mut source_y = vec![0.0; capacity];
        let mut source_z = vec![0.0; capacity];
        let mut random = vec![0; capacity];
        let mut life = vec![0.0; capacity];
        let mut style = vec![0; capacity];
        let mut flags = vec![0; capacity];
        let mut state = fold_seed(recipe.seed);
        for index in 0..capacity {
            state = xorshift32(state);
            random[index] = state;
            source_x[index] =
                unit_signed(state.rotate_left(3)) * recipe.source_scatter.x_half_extent;
            source_y[index] =
                unit_signed(state.rotate_left(13)) * recipe.source_scatter.y_half_extent;
            source_z[index] =
                unit_signed(state.rotate_left(23)) * recipe.source_scatter.z_half_extent;
        }
        decode_particle_cloud(
            ARCADE_CLOUD,
            recipe.model,
            &mut target_x,
            &mut target_y,
            &mut target_z,
            &mut life,
            &random,
            &mut style,
            &mut flags,
        )?;
        let block_count = capacity.div_ceil(PARTICLE_LANES);
        let mut positions = Vec::with_capacity(block_count);
        let mut attributes = Vec::with_capacity(block_count);
        for block_index in 0..block_count {
            let mut position = CabinetPositionBlock {
                target_x: [0.0; PARTICLE_LANES],
                target_y: [0.0; PARTICLE_LANES],
                target_z: [0.0; PARTICLE_LANES],
                source_x: [0.0; PARTICLE_LANES],
                source_y: [0.0; PARTICLE_LANES],
                source_z: [0.0; PARTICLE_LANES],
            };
            let mut attribute = CabinetAttributeBlock {
                random: [0; PARTICLE_LANES],
                life: [0.0; PARTICLE_LANES],
                style: [0; PARTICLE_LANES],
                flags: [0; PARTICLE_LANES],
            };
            for lane in 0..PARTICLE_LANES {
                let index = block_index * PARTICLE_LANES + lane;
                if index >= capacity {
                    break;
                }
                position.target_x[lane] = target_x[index];
                position.target_y[lane] = target_y[index];
                position.target_z[lane] = target_z[index];
                position.source_x[lane] = source_x[index];
                position.source_y[lane] = source_y[index];
                position.source_z[lane] = source_z[index];
                attribute.random[lane] = random[index];
                attribute.life[lane] = life[index];
                attribute.style[lane] = style[index];
                attribute.flags[lane] = flags[index];
            }
            positions.push(position);
            attributes.push(attribute);
        }
        let dynamic_positions = positions.clone();
        Ok(Self {
            width,
            height,
            recipe,
            capacity,
            positions,
            dynamic_positions,
            attributes,
            commands: vec![CabinetDrawCommand(INVALID_PARTICLE_OFFSET); capacity],
            previous_commands: vec![CabinetDrawCommand(INVALID_PARTICLE_OFFSET); capacity],
            dirty_offsets: [
                Vec::with_capacity(capacity * 2),
                Vec::with_capacity(capacity * 2),
            ],
            full_clear: [true; 2],
            projection_frame: 0,
            commands_initialized: false,
            options: CabinetRenderOptions {
                active_count,
                creative_mode: CabinetCreativeMode::Baseline,
            },
        })
    }

    #[must_use]
    pub const fn particle_count(&self) -> usize {
        self.options.active_count
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn set_render_options(&mut self, options: CabinetRenderOptions) -> Result<(), String> {
        if options.active_count == 0 || options.active_count > self.capacity() {
            return Err(format!(
                "cabinet active count {} is outside 1..={}",
                options.active_count,
                self.capacity()
            ));
        }
        if self.options != options {
            self.options = options;
            self.full_clear = [true; 2];
            self.commands_initialized = false;
            self.previous_commands
                .fill(CabinetDrawCommand(INVALID_PARTICLE_OFFSET));
        }
        Ok(())
    }

    #[must_use]
    pub const fn render_options(&self) -> CabinetRenderOptions {
        self.options
    }

    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        self.positions
            .capacity()
            .saturating_mul(std::mem::size_of::<CabinetPositionBlock>())
            .saturating_add(
                self.dynamic_positions
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CabinetPositionBlock>()),
            )
            .saturating_add(
                self.attributes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CabinetAttributeBlock>()),
            )
            .saturating_add(
                self.commands
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CabinetDrawCommand>()),
            )
            .saturating_add(
                self.previous_commands
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CabinetDrawCommand>()),
            )
            .saturating_add(
                self.dirty_offsets
                    .iter()
                    .map(Vec::capacity)
                    .sum::<usize>()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
    }

    pub fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
        buffer_id: usize,
    ) -> Result<ArcadeCabinetFrameStats, String> {
        let expected = self.width.saturating_mul(self.height);
        if destination.len() != expected {
            return Err(format!(
                "arcade cabinet destination has {} pixels, expected {expected}",
                destination.len()
            ));
        }
        if buffer_id >= self.dirty_offsets.len() {
            return Err(format!("cabinet buffer id {buffer_id} is outside 0..2"));
        }
        let clear_started = Instant::now();
        let background = pixel(self.recipe.appearance.background);
        let dirty_offsets = &mut self.dirty_offsets[buffer_id];
        if self.full_clear[buffer_id] {
            destination.fill(background);
            self.full_clear[buffer_id] = false;
        } else {
            for &offset in dirty_offsets.iter() {
                destination[offset as usize] = background;
            }
        }
        dirty_offsets.clear();
        let clear_us = elapsed_us(clear_started.elapsed());
        let (formation, yaw, pitch, dolly, dispersal) = arcade_camera(&self.recipe, elapsed);
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let center_x = self.width as f32 * 0.5 + self.recipe.camera.center_offset_x;
        let center_y = self.height as f32 * 0.5 + self.recipe.camera.center_offset_y;
        let appearance = self.recipe.appearance;
        let creative_mode = self.options.creative_mode;
        let satellite_mode = creative_mode.uses_satellites();
        let history_mode =
            creative_mode.uses_history_echo() && (formation < 1.0 || dispersal > 0.0);
        let depth_palette_mode = creative_mode.uses_depth_palette();
        let micro_jitter_mode = creative_mode.uses_micro_jitter();
        let jitter_phase = (self.projection_frame & 3) as u8;
        let width_f32 = self.width as f32;
        let height_f32 = self.height as f32;
        let projection_started = Instant::now();
        let projection_cohorts = if self.options.active_count > TWO_WAY_PARTICLE_LIMIT {
            3_u8
        } else if self.options.active_count > FULL_RATE_PARTICLE_LIMIT {
            2_u8
        } else {
            1_u8
        };
        let project_all = !self.commands_initialized || projection_cohorts == 1;
        let projection_cohort = (self.projection_frame % u64::from(projection_cohorts)) as usize;
        let projected_particles = if project_all {
            self.options.active_count
        } else {
            cohort_particle_count(
                self.options.active_count,
                usize::from(projection_cohorts),
                projection_cohort,
            )
        };
        let mut visible: usize;
        let mut pixel_writes = 0usize;
        let mut projection_backend = "cabinet-scalar";

        macro_rules! draw_offset {
            ($index:expr, $offset:expr, $pixel_x:expr, $attribute:expr, $lane:expr) => {{
                let index = $index;
                let mut offset = $offset;
                let attribute = $attribute;
                let lane = $lane;
                let feature = attribute.flags[lane];
                // All-mode composition is intentionally ordered: jitter the
                // position, choose depth color, draw history, draw satellites,
                // and finally overwrite the center with the current primary.
                if micro_jitter_mode && feature == 0 && attribute.random[lane] & 1 == 0 {
                    offset = jittered_offset(
                        offset,
                        self.width,
                        destination.len(),
                        attribute.random[lane],
                        jitter_phase,
                    );
                }
                let pixel_x = offset % self.width;
                let base_style = if feature & appearance.priority_feature_mask != 0 {
                    appearance.priority_palette_index
                } else if feature & appearance.accent_feature_mask != 0 {
                    attribute.style[lane]
                        .saturating_add(appearance.accent_palette_add)
                        .min(7)
                } else {
                    attribute.style[lane]
                };
                let style = if depth_palette_mode {
                    let adjustment = match self.commands[index].depth_band() {
                        0 => 2_i16,
                        1 => 1_i16,
                        2 => 0_i16,
                        _ => -1_i16,
                    };
                    (i16::from(base_style) + adjustment).clamp(0, 7) as u8
                } else {
                    base_style
                };
                let recipe_neighbor = feature & appearance.neighbor_feature_mask != 0
                    && index % usize::from(appearance.neighbor_every) == 0
                    && pixel_x + 1 < self.width;
                if history_mode {
                    if let Some(mut history_offset) = self.previous_commands[index].offset() {
                        if micro_jitter_mode && feature == 0 && attribute.random[lane] & 1 == 0 {
                            history_offset = jittered_offset(
                                history_offset,
                                self.width,
                                destination.len(),
                                attribute.random[lane],
                                jitter_phase.wrapping_sub(1),
                            );
                        }
                        let history_style = style.saturating_sub(2);
                        destination[history_offset] =
                            pixel(appearance.palette[usize::from(history_style)]);
                        dirty_offsets.push(history_offset as u32);
                        pixel_writes = pixel_writes.saturating_add(1);
                    }
                }
                if satellite_mode && recipe_neighbor {
                    let neighbor_style = style.saturating_sub(appearance.neighbor_palette_subtract);
                    destination[offset + 1] =
                        pixel(appearance.palette[usize::from(neighbor_style)]);
                    dirty_offsets.push((offset + 1) as u32);
                    pixel_writes = pixel_writes.saturating_add(1);
                }
                if satellite_mode && feature == 0 {
                    let satellite_offset = match attribute.random[lane] & 3 {
                        0 if pixel_x > 0 => Some(offset - 1),
                        1 if pixel_x + 1 < self.width => Some(offset + 1),
                        2 if offset >= self.width => Some(offset - self.width),
                        3 if offset + self.width < destination.len() => Some(offset + self.width),
                        _ => None,
                    };
                    if let Some(satellite_offset) = satellite_offset {
                        let satellite_style = style.saturating_sub(1);
                        destination[satellite_offset] =
                            pixel(appearance.palette[usize::from(satellite_style)]);
                        dirty_offsets.push(satellite_offset as u32);
                        pixel_writes = pixel_writes.saturating_add(1);
                    }
                }
                destination[offset] = pixel(appearance.palette[usize::from(style)]);
                dirty_offsets.push(offset as u32);
                pixel_writes = pixel_writes.saturating_add(1);
                if !satellite_mode && recipe_neighbor {
                    let neighbor_style = style.saturating_sub(appearance.neighbor_palette_subtract);
                    destination[offset + 1] =
                        pixel(appearance.palette[usize::from(neighbor_style)]);
                    dirty_offsets.push((offset + 1) as u32);
                    pixel_writes = pixel_writes.saturating_add(1);
                }
            }};
        }

        macro_rules! project_and_draw {
            ($index:expr, $world_x:expr, $world_y:expr, $world_z:expr) => {{
                let index = $index;
                let world_x = $world_x;
                let world_y = $world_y;
                let world_z = $world_z;
                self.commands[index] = CabinetDrawCommand(INVALID_PARTICLE_OFFSET);
                let rotated_x = world_x.mul_add(cos_yaw, world_z * sin_yaw);
                let yaw_z = (-world_x).mul_add(sin_yaw, world_z * cos_yaw);
                let rotated_y = world_y.mul_add(cos_pitch, -(yaw_z * sin_pitch));
                let rotated_z = world_y.mul_add(sin_pitch, yaw_z * cos_pitch);
                let depth = dolly + rotated_z;
                if depth > self.recipe.camera.near_depth {
                    let scale = self.recipe.camera.focal_length / depth;
                    let x = center_x + rotated_x * scale;
                    let y = center_y + rotated_y * scale;
                    if x >= 0.0 && y >= 0.0 && x < width_f32 && y < height_f32 {
                        let pixel_x = x as usize;
                        let offset = y as usize * self.width + pixel_x;
                        self.commands[index] = CabinetDrawCommand::visible(offset, depth);
                    }
                }
            }};
        }

        macro_rules! selected_for_projection {
            ($index:expr) => {
                project_all
                    || (($index / PARTICLE_LANES) % usize::from(projection_cohorts)
                        == projection_cohort)
            };
        }

        if history_mode && self.commands_initialized {
            for index in 0..self.options.active_count {
                if selected_for_projection!(index) {
                    self.previous_commands[index] = self.commands[index];
                }
            }
        } else if history_mode {
            self.previous_commands[..self.options.active_count]
                .fill(CabinetDrawCommand(INVALID_PARTICLE_OFFSET));
        }

        let first_projection_block = if project_all { 0 } else { projection_cohort };
        let projection_block_step = if project_all {
            1
        } else {
            usize::from(projection_cohorts)
        };
        let projection_block_count = self.options.active_count.div_ceil(PARTICLE_LANES);
        let projection_positions = if dispersal > 0.0 {
            for block_index in (first_projection_block..projection_block_count)
                .step_by(projection_block_step)
            {
                let position = &self.positions[block_index];
                let attribute = &self.attributes[block_index];
                let dynamic = &mut self.dynamic_positions[block_index];
                for lane in 0..PARTICLE_LANES {
                    let scale = 1.0
                        + dispersal
                            * (self.recipe.dispersal.radial_base
                                + attribute.life[lane]
                                    * self.recipe.dispersal.radial_life_gain);
                    dynamic.target_x[lane] = position.target_x[lane] * scale;
                    dynamic.target_y[lane] = position.target_y[lane] * scale
                        + dispersal
                            * unit_signed(attribute.random[lane].rotate_left(11))
                            * self.recipe.dispersal.vertical_jitter;
                    dynamic.target_z[lane] = position.target_z[lane] * scale;
                }
            }
            &self.dynamic_positions
        } else if formation < 1.0 {
            for block_index in (first_projection_block..projection_block_count)
                .step_by(projection_block_step)
            {
                let position = &self.positions[block_index];
                let dynamic = &mut self.dynamic_positions[block_index];
                for lane in 0..PARTICLE_LANES {
                    dynamic.target_x[lane] = position.source_x[lane]
                        + (position.target_x[lane] - position.source_x[lane]) * formation;
                    dynamic.target_y[lane] = position.source_y[lane]
                        + (position.target_y[lane] - position.source_y[lane]) * formation;
                    dynamic.target_z[lane] = position.source_z[lane]
                        + (position.target_z[lane] - position.source_z[lane]) * formation;
                }
            }
            &self.dynamic_positions
        } else {
            &self.positions
        };
        let vector_end = project_stable_neon(
            self.options.active_count,
            projection_positions,
            first_projection_block,
            projection_block_step,
            sin_yaw,
            cos_yaw,
            sin_pitch,
            cos_pitch,
            dolly,
            self.recipe.camera.near_depth,
            self.recipe.camera.focal_length,
            center_x,
            center_y,
            self.width,
            self.height,
            &mut self.commands,
        );
        if vector_end > 0 {
            projection_backend = match (project_all, projection_cohorts) {
                (true, _) => "cabinet-neon",
                (false, 2) => "cabinet-neon-cohort-2",
                (false, 3) => "cabinet-neon-cohort-3",
                (false, _) => "cabinet-neon-cohort",
            };
        }
        for index in vector_end..self.options.active_count {
            if !selected_for_projection!(index) {
                continue;
            }
            let position = &projection_positions[index / PARTICLE_LANES];
            let lane = index % PARTICLE_LANES;
            project_and_draw!(
                index,
                position.target_x[lane],
                position.target_y[lane],
                position.target_z[lane]
            );
        }
        self.commands_initialized = true;
        self.projection_frame = self.projection_frame.wrapping_add(1);

        let projection_us = elapsed_us(projection_started.elapsed());
        let ordering_us = 0;
        let raster_started = Instant::now();
        visible = 0;
        if creative_mode == CabinetCreativeMode::Baseline {
            for index in 0..self.options.active_count {
                let Some(offset) = self.commands[index].offset() else {
                    continue;
                };
                visible = visible.saturating_add(1);
                let attribute = &self.attributes[index / PARTICLE_LANES];
                let lane = index % PARTICLE_LANES;
                let feature = attribute.flags[lane];
                let style = if feature & appearance.priority_feature_mask != 0 {
                    appearance.priority_palette_index
                } else if feature & appearance.accent_feature_mask != 0 {
                    attribute.style[lane]
                        .saturating_add(appearance.accent_palette_add)
                        .min(7)
                } else {
                    attribute.style[lane]
                };
                destination[offset] = pixel(appearance.palette[usize::from(style)]);
                dirty_offsets.push(offset as u32);
                pixel_writes = pixel_writes.saturating_add(1);
                if feature & appearance.neighbor_feature_mask != 0
                    && index % usize::from(appearance.neighbor_every) == 0
                {
                    let pixel_x = offset % self.width;
                    if pixel_x + 1 < self.width {
                        let neighbor_style =
                            style.saturating_sub(appearance.neighbor_palette_subtract);
                        destination[offset + 1] =
                            pixel(appearance.palette[usize::from(neighbor_style)]);
                        dirty_offsets.push((offset + 1) as u32);
                        pixel_writes = pixel_writes.saturating_add(1);
                    }
                }
            }
        } else {
            for index in 0..self.options.active_count {
                let Some(offset) = self.commands[index].offset() else {
                    continue;
                };
                visible = visible.saturating_add(1);
                let attribute = &self.attributes[index / PARTICLE_LANES];
                let lane = index % PARTICLE_LANES;
                draw_offset!(index, offset, offset % self.width, attribute, lane);
            }
        }
        let raster_us = elapsed_us(raster_started.elapsed());

        Ok(ArcadeCabinetFrameStats {
            particles: self.options.active_count,
            projected_particles,
            projection_cohorts,
            visible,
            pixel_writes,
            projection_backend,
            stages: CabinetStageTimings {
                clear_us,
                projection_us,
                ordering_us,
                raster_us,
                worker_wait_us: 0,
                prepared_age_us: 0,
            },
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn project_stable_neon(
    count: usize,
    positions: &[CabinetPositionBlock],
    first_block: usize,
    block_step: usize,
    sin_yaw: f32,
    cos_yaw: f32,
    sin_pitch: f32,
    cos_pitch: f32,
    dolly: f32,
    near_depth: f32,
    focal_length: f32,
    center_x: f32,
    center_y: f32,
    width: usize,
    height: usize,
    offsets: &mut [CabinetDrawCommand],
) -> usize {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        if count < PARTICLE_LANES
            || block_step == 0
            || positions.len() * PARTICLE_LANES < count
            || offsets.len() < count
        {
            return 0;
        }
        let Ok(width) = u32::try_from(width) else {
            return 0;
        };
        let Ok(height) = u32::try_from(height) else {
            return 0;
        };
        // SAFETY: position blocks have the C layout declared above, the input
        // covers count rounded down to four lanes, and offsets has count words.
        unsafe {
            mister_magik_cabinet_neon_project_stable(
                count,
                positions.as_ptr(),
                first_block,
                block_step,
                sin_yaw,
                cos_yaw,
                sin_pitch,
                cos_pitch,
                dolly,
                near_depth,
                focal_length,
                center_x,
                center_y,
                width,
                height,
                offsets.as_mut_ptr().cast::<u32>(),
            )
        }
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        let _ = (
            count,
            positions,
            first_block,
            block_step,
            sin_yaw,
            cos_yaw,
            sin_pitch,
            cos_pitch,
            dolly,
            near_depth,
            focal_length,
            center_x,
            center_y,
            width,
            height,
            offsets,
        );
        0
    }
}

fn elapsed_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn cohort_particle_count(count: usize, cohorts: usize, cohort: usize) -> usize {
    let full_blocks = count / PARTICLE_LANES;
    let complete = full_blocks / cohorts;
    let extra_block = usize::from(cohort < full_blocks % cohorts);
    let tail = if full_blocks % cohorts == cohort {
        count % PARTICLE_LANES
    } else {
        0
    };
    (complete + extra_block) * PARTICLE_LANES + tail
}

fn jittered_offset(offset: usize, width: usize, frame_len: usize, random: u32, phase: u8) -> usize {
    let x = offset % width;
    let y = offset / width;
    if x == 0 || x + 1 >= width || y == 0 || offset + width >= frame_len {
        return offset;
    }
    match (u32::from(phase) + ((random >> 3) & 3)) & 3 {
        0 => offset - 1,
        1 => offset - width,
        2 => offset + 1,
        _ => offset + width,
    }
}

const fn pixel(color: RecipeRgb565) -> Rgb565Pixel {
    Rgb565Pixel(color.0)
}

/// Decodes the canonical little-endian `PCLOUD1` representation.
///
/// The 28-byte header is the eight-byte `PCLOUD1\0` magic, `u16` version,
/// `u16` record stride, `u32` point count, then six `i16` bounds in
/// x-min/x-max/y-min/y-max/z-min/z-max order. Each eight-byte point record is
/// `i16 x`, `i16 y`, `i16 z`, `u8 material`, and `u8 feature flags`.
#[allow(clippy::too_many_arguments)]
fn decode_particle_cloud(
    bytes: &[u8],
    model: CabinetModel,
    target_x: &mut [f32],
    target_y: &mut [f32],
    target_z: &mut [f32],
    life: &mut [f32],
    random: &[u32],
    style: &mut [u8],
    flags: &mut [u8],
) -> Result<(), String> {
    if bytes.len() < PARTICLE_CLOUD_HEADER_BYTES || &bytes[..8] != PARTICLE_CLOUD_MAGIC {
        return Err("arcade particle cloud header is invalid".into());
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    let stride = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
    let count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    if version != 1 || stride != PARTICLE_CLOUD_RECORD_BYTES {
        return Err(format!(
            "arcade particle cloud contract mismatch: version={version} stride={stride} count={count}"
        ));
    }
    if count == 0 || count > ARCADE_CLOUD_POINT_COUNT {
        return Err(format!(
            "arcade particle cloud count {count} is outside 1..={ARCADE_CLOUD_POINT_COUNT}"
        ));
    }
    if count != ARCADE_CLOUD_POINT_COUNT {
        return Err(format!(
            "arcade particle cloud has {count} points, expected {ARCADE_CLOUD_POINT_COUNT}"
        ));
    }
    let expected = PARTICLE_CLOUD_HEADER_BYTES.saturating_add(count.saturating_mul(stride));
    if bytes.len() != expected {
        return Err(format!(
            "arcade particle cloud length {} does not match expected {expected}",
            bytes.len()
        ));
    }
    let output_count = target_x.len();
    if target_y.len() != output_count
        || target_z.len() != output_count
        || life.len() != output_count
        || random.len() != output_count
        || style.len() != output_count
        || flags.len() != output_count
    {
        return Err("arcade particle cloud output lengths are inconsistent".into());
    }
    let mut bounds = [0i16; 6];
    for (index, value) in bounds.iter_mut().enumerate() {
        let offset = 16 + index * 2;
        *value = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    }
    if bounds[0] > bounds[1] || bounds[2] > bounds[3] || bounds[4] > bounds[5] {
        return Err("arcade particle cloud bounds are invalid".into());
    }
    for index in 0..count {
        let offset = PARTICLE_CLOUD_HEADER_BYTES + index * stride;
        let x = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let y = i16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        let z = i16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
        let palette = bytes[offset + 6];
        let feature_flags = bytes[offset + 7];
        if x < bounds[0]
            || x > bounds[1]
            || y < bounds[2]
            || y > bounds[3]
            || z < bounds[4]
            || z > bounds[5]
            || y < 0
            || palette > 7
            || feature_flags & !3 != 0
        {
            return Err(format!("arcade particle cloud record {index} is invalid"));
        }
        if index < output_count {
            target_x[index] = f32::from(x) * (model.x_half_extent / 32_767.0);
            target_y[index] = model.y_origin - f32::from(y) * (model.y_extent / 32_767.0);
            target_z[index] = f32::from(z) * (model.z_half_extent / 32_767.0);
            style[index] = palette;
            flags[index] = feature_flags;
            life[index] = unit01(random[index].rotate_left(17));
        }
    }
    if output_count > count {
        return Err(format!(
            "arcade particle cloud capacity {output_count} exceeds its {count} unique points"
        ));
    }
    Ok(())
}

fn fold_seed(seed: u64) -> u32 {
    let folded = seed ^ (seed >> 32) ^ (ARCADE_DEMO_NUMBER * 0x9e37_79b9);
    let folded = folded as u32;
    if folded == 0 { 0xa341_316c } else { folded }
}

fn xorshift32(mut state: u32) -> u32 {
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
}

fn unit_signed(value: u32) -> f32 {
    ((value >> 8) as f32) * (2.0 / 16_777_215.0) - 1.0
}

fn unit01(value: u32) -> f32 {
    ((value >> 8) as f32) * (1.0 / 16_777_215.0)
}

fn ease(value: f32, easing: RecipeEasing) -> f32 {
    match easing {
        RecipeEasing::Linear => value,
        RecipeEasing::Smoothstep => value * value * (3.0 - 2.0 * value),
        RecipeEasing::EaseOutCubic => 1.0 - (1.0 - value).powi(3),
    }
}

fn triangle_wave(value: f32) -> f32 {
    let phase = value.rem_euclid(2.0);
    if phase < 1.0 {
        phase.mul_add(2.0, -1.0)
    } else {
        3.0 - phase * 2.0
    }
}

fn arcade_camera(recipe: &CabinetRecipe, elapsed: Duration) -> (f32, f32, f32, f32, f32) {
    let timing = recipe.timing;
    let seconds = elapsed
        .as_secs_f32()
        .rem_euclid(timing.cycle_ms as f32 / 1_000.0);
    let formation_seconds = timing.formation_ms as f32 / 1_000.0;
    let orbit_seconds = timing.orbit_ms as f32 / 1_000.0;
    let return_seconds = timing.return_ms as f32 / 1_000.0;
    let disperse_seconds = timing.disperse_ms as f32 / 1_000.0;
    let formation = ease(
        (seconds / formation_seconds).clamp(0.0, 1.0),
        timing.formation_easing,
    );
    if seconds < formation_seconds {
        let pose = recipe.camera.formation;
        return (
            formation,
            pose.yaw_radians,
            pose.pitch_radians,
            pose.dolly,
            0.0,
        );
    }
    let orbit_end = formation_seconds + orbit_seconds;
    if seconds < orbit_end {
        let phase = (seconds - formation_seconds) / orbit_seconds;
        let orbit = recipe.camera.orbit;
        return (
            1.0,
            orbit.yaw_center_radians
                + (phase * orbit.yaw_turns * std::f32::consts::TAU).sin()
                    * orbit.yaw_amplitude_radians,
            triangle_wave(phase * orbit.pitch_triangle_rate) * orbit.pitch_amplitude_radians,
            orbit.dolly_center
                + triangle_wave(phase * orbit.dolly_triangle_rate + orbit.dolly_triangle_phase)
                    * orbit.dolly_amplitude,
            0.0,
        );
    }
    let return_end = orbit_end + return_seconds;
    if seconds < return_end {
        let return_t = ease(
            ((seconds - orbit_end) / return_seconds).clamp(0.0, 1.0),
            timing.return_easing,
        );
        let orbit = recipe.camera.orbit;
        let target = recipe.camera.return_pose;
        return (
            1.0,
            orbit.yaw_center_radians * (1.0 - return_t) + target.yaw_radians * return_t,
            target.pitch_radians * return_t,
            orbit.dolly_center * (1.0 - return_t) + target.dolly * return_t,
            0.0,
        );
    }
    let target = recipe.camera.return_pose;
    let dispersal = ease(
        ((seconds - return_end) / disperse_seconds).clamp(0.0, 1.0),
        timing.disperse_easing,
    );
    (
        1.0,
        target.yaw_radians,
        target.pitch_radians,
        target.dolly,
        dispersal,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_for_test(bytes: &[u8], model: CabinetModel) -> Result<(), String> {
        let count = ARCADE_CLOUD_POINT_COUNT;
        let mut target_x = vec![0.0; count];
        let mut target_y = vec![0.0; count];
        let mut target_z = vec![0.0; count];
        let mut life = vec![0.0; count];
        let random = vec![0; count];
        let mut style = vec![0; count];
        let mut flags = vec![0; count];
        decode_particle_cloud(
            bytes,
            model,
            &mut target_x,
            &mut target_y,
            &mut target_z,
            &mut life,
            &random,
            &mut style,
            &mut flags,
        )
    }

    #[test]
    fn checked_in_arcade_cloud_supports_the_48k_embedded_prefix() {
        let renderer = ArcadeCabinetFormation::from_embedded(960, 540).unwrap();
        assert_eq!(renderer.particle_count(), 48_128);
        assert_eq!(renderer.capacity(), 48_128);
        assert_eq!(
            u32::from_le_bytes(ARCADE_CLOUD[12..16].try_into().unwrap()) as usize,
            ARCADE_CLOUD_POINT_COUNT
        );
        assert_eq!(renderer.positions.as_ptr().align_offset(16), 0);
        assert_eq!(renderer.attributes.as_ptr().align_offset(16), 0);
    }

    #[test]
    fn checked_in_cloud_prefixes_have_unique_quantized_targets() {
        use std::collections::HashSet;

        for count in [1_024, 24_064, 48_128, 72_192, 72_704] {
            let unique = (0..count)
                .map(|index| {
                    let offset = PARTICLE_CLOUD_HEADER_BYTES + index * PARTICLE_CLOUD_RECORD_BYTES;
                    &ARCADE_CLOUD[offset..offset + 6]
                })
                .collect::<HashSet<_>>();
            assert_eq!(unique.len(), count);
        }
    }

    #[test]
    fn projection_cohorts_cover_every_particle_once() {
        for cohorts in [2, 3] {
            for count in [48_129, 72_192, 72_704, 72_703] {
                let covered = (0..cohorts)
                    .map(|cohort| cohort_particle_count(count, cohorts, cohort))
                    .sum::<usize>();
                assert_eq!(covered, count);
            }
        }
    }

    #[test]
    fn micro_jitter_sequence_is_bounded_and_keeps_edges_fixed() {
        let center = 10 * 32 + 10;
        let offsets = (0..4)
            .map(|phase| jittered_offset(center, 32, 32 * 24, 0, phase))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(offsets.len(), 4);
        assert!(offsets.iter().all(|offset| {
            let delta = offset.abs_diff(center);
            delta == 1 || delta == 32
        }));
        assert_eq!(jittered_offset(0, 32, 32 * 24, 0, 2), 0);
    }

    #[test]
    fn all_mode_enables_every_creative_pass() {
        assert!(CabinetCreativeMode::All.uses_micro_jitter());
        assert!(CabinetCreativeMode::All.uses_depth_palette());
        assert!(CabinetCreativeMode::All.uses_history_echo());
        assert!(CabinetCreativeMode::All.uses_satellites());
        assert!(!CabinetCreativeMode::Baseline.uses_micro_jitter());
        assert!(!CabinetCreativeMode::Baseline.uses_depth_palette());
        assert!(!CabinetCreativeMode::Baseline.uses_history_echo());
        assert!(!CabinetCreativeMode::Baseline.uses_satellites());
    }

    #[test]
    fn lookahead_scene_matches_direct_render_across_alternating_buffers() {
        let recipe = embedded_cabinet_recipe().unwrap();
        let mut direct = ArcadeCabinetFormation::new(320, 180, recipe.clone()).unwrap();
        let mut scene = CabinetScene::new(320, 180, recipe, 2).unwrap();
        let options = CabinetRenderOptions {
            active_count: 1_024,
            creative_mode: CabinetCreativeMode::All,
        };
        direct.set_render_options(options).unwrap();
        scene.set_render_options(options).unwrap();
        let mut direct_pixels = [
            vec![Rgb565Pixel(0); 320 * 180],
            vec![Rgb565Pixel(0); 320 * 180],
        ];
        let mut scene_pixels = direct_pixels.clone();
        for frame in 0..6_u64 {
            let slot = (frame & 1) as usize;
            let elapsed = Duration::from_micros(frame * 16_667);
            direct
                .render(&mut direct_pixels[slot], elapsed, slot)
                .unwrap();
            let buffer = SceneBufferId::new(slot as u8, 2).unwrap();
            let target =
                SceneTarget::new(&mut scene_pixels[slot], scene.geometry(), buffer).unwrap();
            FramebufferScene::render(
                &mut scene,
                target,
                SceneClock {
                    frame,
                    elapsed,
                    next_elapsed: Some(Duration::from_micros((frame + 1) * 16_667)),
                },
            )
            .unwrap();
            assert_eq!(scene_pixels[slot], direct_pixels[slot]);
        }
    }

    #[test]
    fn arcade_formation_is_deterministic() {
        let recipe = embedded_cabinet_recipe().unwrap();
        let mut first = ArcadeCabinetFormation::new(960, 540, recipe.clone()).unwrap();
        let mut second = ArcadeCabinetFormation::new(960, 540, recipe).unwrap();
        let mut first_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let mut second_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let elapsed = Duration::from_secs(12);

        let first_stats = first.render(&mut first_pixels, elapsed, 0).unwrap();
        let second_stats = second.render(&mut second_pixels, elapsed, 0).unwrap();

        assert_eq!(first_stats, second_stats);
        assert_eq!(first_pixels, second_pixels);
        assert!(first_stats.visible > 10_000);
    }

    #[test]
    fn satellite_mode_adds_writes_without_changing_primary_count() {
        let recipe = embedded_cabinet_recipe().unwrap();
        let mut baseline = ArcadeCabinetFormation::new(960, 540, recipe.clone()).unwrap();
        let mut satellites = ArcadeCabinetFormation::new(960, 540, recipe).unwrap();
        satellites
            .set_render_options(CabinetRenderOptions {
                active_count: 48_128,
                creative_mode: CabinetCreativeMode::Satellites,
            })
            .unwrap();
        let mut baseline_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let mut satellite_pixels = baseline_pixels.clone();
        let elapsed = Duration::from_secs(12);
        let baseline_stats = baseline.render(&mut baseline_pixels, elapsed, 0).unwrap();
        let satellite_stats = satellites
            .render(&mut satellite_pixels, elapsed, 0)
            .unwrap();
        assert_eq!(satellite_stats.particles, baseline_stats.particles);
        assert_eq!(satellite_stats.visible, baseline_stats.visible);
        assert!(satellite_stats.pixel_writes > baseline_stats.pixel_writes);
        assert_ne!(satellite_pixels, baseline_pixels);
    }

    #[test]
    fn history_echo_is_limited_to_formation_and_dispersal() {
        let recipe = embedded_cabinet_recipe().unwrap();
        let mut history = ArcadeCabinetFormation::new(960, 540, recipe.clone()).unwrap();
        let mut baseline = ArcadeCabinetFormation::new(960, 540, recipe).unwrap();
        history
            .set_render_options(CabinetRenderOptions {
                active_count: 48_128,
                creative_mode: CabinetCreativeMode::HistoryEcho,
            })
            .unwrap();
        let mut pixels = vec![Rgb565Pixel(0); 960 * 540];
        history
            .render(&mut pixels, Duration::from_millis(900), 0)
            .unwrap();
        let formation = history
            .render(&mut pixels, Duration::from_millis(916), 1)
            .unwrap();
        let orbit = history
            .render(&mut pixels, Duration::from_secs(12), 0)
            .unwrap();
        let mut baseline_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let baseline_orbit = baseline
            .render(&mut baseline_pixels, Duration::from_secs(12), 0)
            .unwrap();
        assert!(formation.pixel_writes > formation.visible);
        assert_eq!(orbit.pixel_writes, baseline_orbit.pixel_writes);
        assert_eq!(pixels, baseline_pixels);
    }

    #[test]
    fn depth_palette_changes_color_without_adding_writes() {
        let recipe = embedded_cabinet_recipe().unwrap();
        let mut baseline = ArcadeCabinetFormation::new(960, 540, recipe.clone()).unwrap();
        let mut depth = ArcadeCabinetFormation::new(960, 540, recipe).unwrap();
        depth
            .set_render_options(CabinetRenderOptions {
                active_count: 48_128,
                creative_mode: CabinetCreativeMode::DepthPalette,
            })
            .unwrap();
        let mut baseline_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let mut depth_pixels = baseline_pixels.clone();
        let baseline_stats = baseline
            .render(&mut baseline_pixels, Duration::from_secs(12), 0)
            .unwrap();
        let depth_stats = depth
            .render(&mut depth_pixels, Duration::from_secs(12), 0)
            .unwrap();
        assert_eq!(depth_stats.pixel_writes, baseline_stats.pixel_writes);
        assert_eq!(depth_stats.visible, baseline_stats.visible);
        assert_ne!(depth_pixels, baseline_pixels);
    }

    #[test]
    fn micro_jitter_is_deterministic_without_adding_writes() {
        let recipe = embedded_cabinet_recipe().unwrap();
        let mut first = ArcadeCabinetFormation::new(960, 540, recipe.clone()).unwrap();
        let mut second = ArcadeCabinetFormation::new(960, 540, recipe).unwrap();
        for renderer in [&mut first, &mut second] {
            renderer
                .set_render_options(CabinetRenderOptions {
                    active_count: 48_128,
                    creative_mode: CabinetCreativeMode::MicroJitter,
                })
                .unwrap();
        }
        let mut first_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let mut second_pixels = first_pixels.clone();
        let first_stats = first
            .render(&mut first_pixels, Duration::from_secs(12), 0)
            .unwrap();
        let second_stats = second
            .render(&mut second_pixels, Duration::from_secs(12), 0)
            .unwrap();
        assert_eq!(first_stats.pixel_writes, second_stats.pixel_writes);
        assert_eq!(first_pixels, second_pixels);
    }

    #[test]
    fn cloud_rejects_trailing_data_before_rendering() {
        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes.push(0);
        let recipe = embedded_cabinet_recipe().unwrap();
        assert!(decode_for_test(&bytes, recipe.model).is_err());
    }

    #[test]
    fn cloud_rejects_unsafe_count_coordinates_material_and_flags() {
        let recipe = embedded_cabinet_recipe().unwrap();

        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes[12..16].copy_from_slice(&((ARCADE_CLOUD_POINT_COUNT as u32) + 1).to_le_bytes());
        assert!(decode_for_test(&bytes, recipe.model).is_err());

        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes[PARTICLE_CLOUD_HEADER_BYTES..PARTICLE_CLOUD_HEADER_BYTES + 2]
            .copy_from_slice(&i16::MAX.to_le_bytes());
        assert!(decode_for_test(&bytes, recipe.model).is_err());

        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes[PARTICLE_CLOUD_HEADER_BYTES + 6] = 8;
        assert!(decode_for_test(&bytes, recipe.model).is_err());

        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes[PARTICLE_CLOUD_HEADER_BYTES + 7] = 4;
        assert!(decode_for_test(&bytes, recipe.model).is_err());
    }
}
