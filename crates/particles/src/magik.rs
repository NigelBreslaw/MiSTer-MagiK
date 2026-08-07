// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(test)]
use crate::commands::pack_visual_command;
use crate::commands::{
    COMMAND_NEIGHBOR, COMMAND_OFFSET_BITS, COMMAND_OFFSET_MASK, COMMAND_PALETTE_SHIFT,
    raster_packed_visual_commands_recording,
};
use crate::engine::{
    PARTICLE_NOT_VISIBLE_OFFSET, ParticleConfig, ParticleEngine, ParticleFrameStats, ParticlePhase,
    ParticlePreset, ParticleSimulationUpdate, TargetMask, magik_target_mask,
};
use crate::recipes::MagikRecipe;
use mister_magik_framebuffer_scenes::{
    FramebufferScene, Rgb565Pixel, SceneBufferId, SceneClock, SceneError, SceneGeometry,
    SceneTarget,
};
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const CAPACITY_COLOR: Rgb565Pixel = Rgb565Pixel(0xbdf7);
#[cfg(test)]
const VISUAL_PALETTE: [Rgb565Pixel; 4] = [
    Rgb565Pixel(0x2104),
    Rgb565Pixel(0x5aeb),
    Rgb565Pixel(0xbdf7),
    Rgb565Pixel(0xffff),
];
const FULL_CLEAR_DIRTY_DIVISOR: usize = 4;
const COMMAND_BIN_SHIFT: u32 = 11;
const COMMAND_BIN_COUNT: usize = (1 << (COMMAND_OFFSET_BITS - COMMAND_BIN_SHIFT)) + 1;
const COMMAND_INVISIBLE_BIN: usize = COMMAND_BIN_COUNT - 1;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum PreparationMode {
    Synchronous,
    #[default]
    Lookahead,
}

#[derive(Clone, Copy, Debug)]
pub struct MagikSceneOptions {
    pub order_commands: bool,
    pub reusable_buffers: u8,
    pub worker_start: Option<fn()>,
}

impl Default for MagikSceneOptions {
    fn default() -> Self {
        Self {
            order_commands: false,
            reusable_buffers: 2,
            worker_start: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MagikSceneStats {
    pub count: usize,
    pub visible: usize,
    pub phase: ParticlePhase,
    pub cycle: u64,
    pub simulation_backend: &'static str,
    pub projection_backend: &'static str,
    pub simulation_us: u128,
    pub simulation_cpu_us: u128,
    pub projection_us: u128,
    pub projection_cpu_us: u128,
    pub preparation_wait_us: u128,
    pub prepared_frame_age_us: u128,
    pub lookahead_mismatch_count: u64,
    pub preparation_queue_depth: usize,
    pub worker_wake_latency_us: u128,
    pub clear_us: u128,
    pub clear_cpu_us: u128,
    pub raster_us: u128,
    pub raster_cpu_us: u128,
    pub rotation_y_millidegrees: u32,
    pub simulation_bytes: usize,
    pub renderer_scratch_bytes: usize,
}

pub struct MagikScene {
    config: ParticleConfig,
    background: Rgb565Pixel,
    visual_palette: [Rgb565Pixel; 4],
    neighbor_palette_index: usize,
    engine: Option<ParticleEngine>,
    preparation_pipeline: Option<ParticlePreparationPipeline>,
    command_ordering_scratch: Option<Vec<u32>>,
    dirty_slots: Vec<ParticleDirtySlot>,
    simulation_bytes: usize,
    renderer_scratch_bytes: usize,
    commands: Vec<u32>,
}

struct ParticleDirtySlot {
    initialized: bool,
    offsets: Vec<u32>,
}

struct ParticlePreparationRequest {
    tick: u64,
    elapsed: Duration,
    sent_at: Instant,
    commands: Vec<u32>,
}

struct PreparedParticleFrame {
    tick: u64,
    frame: ParticleFrameStats,
    visible: usize,
    simulation_us: u128,
    simulation_cpu_us: u128,
    projection_us: u128,
    projection_cpu_us: u128,
    preparation_wait_us: u128,
    prepared_frame_age_us: u128,
    lookahead_mismatch_count: u64,
    preparation_queue_depth: usize,
    worker_wake_latency_us: u128,
    completed_at: Instant,
    commands: Vec<u32>,
}

struct ParticlePreparationPipeline {
    request_tx: Option<SyncSender<ParticlePreparationRequest>>,
    ready_rx: Receiver<Result<PreparedParticleFrame, String>>,
    presentation_tick: u64,
    in_flight: VecDeque<u64>,
    spare_commands: Vec<Vec<u32>>,
    worker: Option<JoinHandle<()>>,
}

struct ParticleProjectionCache {
    commands: Vec<u32>,
    cohort_visible: [usize; 2],
    phase: Option<ParticlePhase>,
    initialized: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectionCacheUpdate {
    None,
    Cohort(u8),
    All,
}

impl MagikScene {
    pub fn new_magik(config: ParticleConfig) -> Result<Self, String> {
        Self::new_magik_with_options(config, MagikSceneOptions::default())
    }

    pub fn new_magik_with_options(
        config: ParticleConfig,
        options: MagikSceneOptions,
    ) -> Result<Self, String> {
        let mask = magik_target_mask()?.fit_within(config.width, config.height)?;
        Self::new(config, mask, options)
    }

    fn new(
        config: ParticleConfig,
        mask: TargetMask,
        options: MagikSceneOptions,
    ) -> Result<Self, String> {
        Self::new_with_preparation(config, mask, options, PreparationMode::Lookahead)
    }

    fn new_with_preparation(
        config: ParticleConfig,
        mask: TargetMask,
        options: MagikSceneOptions,
        preparation: PreparationMode,
    ) -> Result<Self, String> {
        Self::from_engine(ParticleEngine::new(config, mask)?, options, preparation)
    }

    pub fn from_magik_recipe(
        width: usize,
        height: usize,
        preset: ParticlePreset,
        recipe: MagikRecipe,
    ) -> Result<Self, String> {
        Self::from_magik_recipe_with_options(
            width,
            height,
            preset,
            recipe,
            MagikSceneOptions::default(),
        )
    }

    pub fn from_magik_recipe_with_options(
        width: usize,
        height: usize,
        preset: ParticlePreset,
        recipe: MagikRecipe,
        options: MagikSceneOptions,
    ) -> Result<Self, String> {
        let mask = magik_target_mask()?.fit_within(width, height)?;
        Self::from_engine(
            ParticleEngine::from_recipe(width, height, preset, recipe, mask)?,
            options,
            PreparationMode::Lookahead,
        )
    }

    /// Constructs the synchronous reference renderer used only for deterministic
    /// headless captures. Realtime hosts use the asynchronous constructors.
    pub fn from_magik_recipe_for_deterministic_capture(
        width: usize,
        height: usize,
        preset: ParticlePreset,
        recipe: MagikRecipe,
    ) -> Result<Self, String> {
        let mask = magik_target_mask()?.fit_within(width, height)?;
        Self::from_engine(
            ParticleEngine::from_recipe(width, height, preset, recipe, mask)?,
            MagikSceneOptions::default(),
            PreparationMode::Synchronous,
        )
    }

    fn from_engine(
        engine: ParticleEngine,
        options: MagikSceneOptions,
        preparation: PreparationMode,
    ) -> Result<Self, String> {
        if options.reusable_buffers == 0 {
            return Err("MagiK scene requires at least one reusable buffer".into());
        }
        let config = engine.config();
        let appearance = engine.recipe().appearance;
        let write_capacity = match config.preset {
            ParticlePreset::Capacity => config.count,
            ParticlePreset::Visual => config.count.saturating_mul(2),
        };
        let simulation_bytes = config
            .count
            .saturating_mul(ParticleEngine::bytes_per_particle());
        let dirty_slots = (0..options.reusable_buffers)
            .map(|_| ParticleDirtySlot {
                initialized: false,
                offsets: Vec::with_capacity(write_capacity),
            })
            .collect::<Vec<_>>();
        let mut engine = Some(engine);
        let commands = Vec::with_capacity(config.count);
        let order_commands = options.order_commands;
        let preparation_pipeline = if preparation == PreparationMode::Lookahead {
            Some(ParticlePreparationPipeline::start(
                engine
                    .take()
                    .expect("particle preparation pipeline must receive its engine"),
                [
                    Vec::with_capacity(config.count),
                    Vec::with_capacity(config.count),
                ],
                Vec::with_capacity(config.count),
                order_commands.then(|| Vec::with_capacity(config.count)),
                options.worker_start,
            )?)
        } else {
            None
        };
        let command_ordering_scratch = (preparation_pipeline.is_none() && order_commands)
            .then(|| Vec::with_capacity(config.count));
        let command_buffer_count =
            if preparation_pipeline.is_some() { 4 } else { 1 } + usize::from(order_commands);
        let renderer_scratch_bytes = dirty_slots.iter().fold(0usize, |total, slot| {
            total.saturating_add(
                slot.offsets
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
        }) + commands
            .capacity()
            .saturating_mul(std::mem::size_of::<u32>())
            .saturating_mul(command_buffer_count);
        Ok(Self {
            config,
            background: Rgb565Pixel(appearance.background.0),
            visual_palette: appearance.palette.map(|color| Rgb565Pixel(color.0)),
            neighbor_palette_index: usize::from(appearance.neighbor_palette_index),
            engine,
            preparation_pipeline,
            command_ordering_scratch,
            dirty_slots,
            simulation_bytes,
            renderer_scratch_bytes,
            commands,
        })
    }

    pub fn preset(&self) -> ParticlePreset {
        self.config.preset
    }

    pub fn particle_count(&self) -> usize {
        self.config.count
    }

    pub fn render_frame(
        &mut self,
        destination: &mut [Rgb565Pixel],
        buffer_id: SceneBufferId,
        elapsed: Duration,
    ) -> Result<MagikSceneStats, String> {
        self.render_with_lookahead(destination, buffer_id, elapsed, None)
    }

    pub fn render_with_lookahead(
        &mut self,
        destination: &mut [Rgb565Pixel],
        buffer_id: SceneBufferId,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
    ) -> Result<MagikSceneStats, String> {
        let frame_len = self.config.width.saturating_mul(self.config.height);
        if destination.len() != frame_len {
            return Err(format!(
                "particle destination has {} pixels, expected {frame_len}",
                destination.len()
            ));
        }
        let slot_offset = usize::from(buffer_id.get());
        if slot_offset >= self.dirty_slots.len() {
            return Err(format!(
                "MagiK scene buffer ID {slot_offset} exceeds configured count {}",
                self.dirty_slots.len()
            ));
        }
        let prepared = if let Some(pipeline) = self.preparation_pipeline.as_mut() {
            pipeline.acquire(elapsed, next_elapsed, &mut self.commands)?
        } else {
            prepare_particle_frame(
                self.engine
                    .as_mut()
                    .expect("same-thread particle renderer must own its engine"),
                0,
                elapsed,
                &mut self.commands,
                self.command_ordering_scratch.as_mut(),
                None,
            )?
        };
        let clear_started = Instant::now();
        let clear_cpu_started = thread_cpu_time_us();
        let mut dirty_offsets = self.prepare_hidden_slot(destination, slot_offset);
        let clear_us = clear_started.elapsed().as_micros();
        let clear_cpu_us = elapsed_thread_cpu_us(clear_cpu_started);
        let raster_started = Instant::now();
        let raster_cpu_started = thread_cpu_time_us();
        self.raster(destination, &mut dirty_offsets);
        let raster_us = raster_started.elapsed().as_micros();
        let raster_cpu_us = elapsed_thread_cpu_us(raster_cpu_started);
        self.dirty_slots[slot_offset].offsets = dirty_offsets;
        Ok(stats(
            prepared.frame,
            prepared.visible,
            prepared.simulation_us,
            prepared.simulation_cpu_us,
            prepared.projection_us,
            prepared.projection_cpu_us,
            prepared.preparation_wait_us,
            prepared.prepared_frame_age_us,
            prepared.lookahead_mismatch_count,
            prepared.preparation_queue_depth,
            prepared.worker_wake_latency_us,
            clear_us,
            clear_cpu_us,
            raster_us,
            raster_cpu_us,
            self.simulation_bytes,
            self.renderer_scratch_bytes,
        ))
    }

    pub fn invalidate(&mut self, buffer_id: SceneBufferId) {
        let slot_offset = usize::from(buffer_id.get());
        if slot_offset < self.dirty_slots.len() {
            let slot = &mut self.dirty_slots[slot_offset];
            slot.initialized = false;
            slot.offsets.clear();
        }
    }

    fn prepare_hidden_slot(
        &mut self,
        destination: &mut [Rgb565Pixel],
        slot_offset: usize,
    ) -> Vec<u32> {
        let slot = &mut self.dirty_slots[slot_offset];
        if !slot.initialized || slot.offsets.len() >= destination.len() / FULL_CLEAR_DIRTY_DIVISOR {
            destination.fill(self.background);
        } else {
            for &offset in &slot.offsets {
                if offset != PARTICLE_NOT_VISIBLE_OFFSET {
                    destination[offset as usize] = self.background;
                }
            }
        }
        slot.initialized = true;
        let mut offsets = std::mem::take(&mut slot.offsets);
        offsets.clear();
        offsets
    }

    fn raster(&self, destination: &mut [Rgb565Pixel], dirty_offsets: &mut Vec<u32>) {
        match self.config.preset {
            ParticlePreset::Capacity => self.raster_capacity(destination, dirty_offsets),
            ParticlePreset::Visual => self.raster_visual(destination, dirty_offsets),
        }
    }

    fn raster_capacity(&self, destination: &mut [Rgb565Pixel], dirty_offsets: &mut Vec<u32>) {
        for &offset in &self.commands {
            if offset != PARTICLE_NOT_VISIBLE_OFFSET {
                destination[offset as usize] = CAPACITY_COLOR;
                dirty_offsets.push(offset);
            }
        }
    }

    fn raster_visual(&self, destination: &mut [Rgb565Pixel], dirty_offsets: &mut Vec<u32>) {
        raster_packed_visual_commands_recording(
            destination,
            &self.commands,
            self.visual_palette,
            self.neighbor_palette_index,
            dirty_offsets,
        );
    }
}

impl FramebufferScene for MagikScene {
    type Stats = MagikSceneStats;

    fn geometry(&self) -> SceneGeometry {
        SceneGeometry::new(self.config.width, self.config.height, self.config.width)
            .expect("validated particle geometry must remain valid")
    }

    fn render(
        &mut self,
        target: SceneTarget<'_>,
        clock: SceneClock,
    ) -> Result<Self::Stats, SceneError> {
        if target.geometry() != self.geometry() {
            return Err(SceneError::Render(format!(
                "MagiK target geometry {:?} does not match scene {:?}",
                target.geometry(),
                self.geometry()
            )));
        }
        let buffer_id = target.buffer_id();
        self.render_with_lookahead(
            target.into_pixels(),
            buffer_id,
            clock.elapsed,
            clock.next_elapsed,
        )
        .map_err(SceneError::Render)
    }

    fn invalidate_buffer(&mut self, buffer: SceneBufferId) {
        self.invalidate(buffer);
    }
}

impl ParticlePreparationPipeline {
    fn start(
        engine: ParticleEngine,
        spare_commands: [Vec<u32>; 2],
        cached_commands: Vec<u32>,
        ordering_scratch: Option<Vec<u32>>,
        worker_start: Option<fn()>,
    ) -> Result<Self, String> {
        let (request_tx, request_rx) = mpsc::sync_channel::<ParticlePreparationRequest>(2);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<PreparedParticleFrame, String>>(2);
        let worker = std::thread::Builder::new()
            .name("particle-prepare".into())
            .spawn(move || {
                if let Some(worker_start) = worker_start {
                    worker_start();
                }
                run_particle_preparation_worker(
                    engine,
                    cached_commands,
                    ordering_scratch,
                    request_rx,
                    ready_tx,
                );
            })
            .map_err(|error| format!("spawn particle preparation worker: {error}"))?;
        Ok(Self {
            request_tx: Some(request_tx),
            ready_rx,
            presentation_tick: 0,
            in_flight: VecDeque::new(),
            spare_commands: spare_commands.into(),
            worker: Some(worker),
        })
    }

    fn acquire(
        &mut self,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
        commands: &mut Vec<u32>,
    ) -> Result<PreparedParticleFrame, String> {
        let tick = self.presentation_tick;
        self.presentation_tick = self.presentation_tick.wrapping_add(1);
        let preparation_queue_depth = self.in_flight.len();
        let wait_started = Instant::now();
        let mut prepared = if let Some(in_flight_tick) = self.in_flight.pop_front() {
            debug_assert_eq!(in_flight_tick, tick);
            self.receive()?
        } else {
            self.send(tick, elapsed)?;
            self.receive()?
        };
        if prepared.tick != tick {
            self.spare_commands
                .push(std::mem::take(&mut prepared.commands));
            return Err(format!(
                "particle preparation queue returned tick {}, expected {tick}",
                prepared.tick
            ));
        }
        debug_assert_eq!(prepared.tick, tick);
        prepared.preparation_wait_us = wait_started.elapsed().as_micros();
        prepared.prepared_frame_age_us = prepared.completed_at.elapsed().as_micros();
        prepared.lookahead_mismatch_count = 0;
        prepared.preparation_queue_depth = preparation_queue_depth;
        std::mem::swap(commands, &mut prepared.commands);
        self.spare_commands
            .push(std::mem::take(&mut prepared.commands));
        if let Some(next_elapsed) = next_elapsed.filter(|next| *next > elapsed) {
            let period = next_elapsed.saturating_sub(elapsed);
            while self.in_flight.len() < 2 {
                let frames_ahead = self.in_flight.len() + 1;
                let next_tick = tick.wrapping_add(frames_ahead as u64);
                let prepared_elapsed =
                    next_elapsed.saturating_add(period.saturating_mul((frames_ahead - 1) as u32));
                self.send(next_tick, prepared_elapsed)?;
                self.in_flight.push_back(next_tick);
            }
        }
        Ok(prepared)
    }

    fn send(&mut self, tick: u64, elapsed: Duration) -> Result<(), String> {
        let commands = self
            .spare_commands
            .pop()
            .ok_or("particle preparation pipeline has no spare command buffer")?;
        self.request_tx
            .as_ref()
            .ok_or("particle preparation worker has stopped")?
            .send(ParticlePreparationRequest {
                tick,
                elapsed,
                sent_at: Instant::now(),
                commands,
            })
            .map_err(|_| "particle preparation worker disconnected".to_string())
    }

    fn receive(&self) -> Result<PreparedParticleFrame, String> {
        self.ready_rx
            .recv()
            .map_err(|_| "particle preparation worker disconnected".to_string())?
    }
}

impl Drop for ParticlePreparationPipeline {
    fn drop(&mut self) {
        self.request_tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_particle_preparation_worker(
    mut engine: ParticleEngine,
    cached_commands: Vec<u32>,
    mut ordering_scratch: Option<Vec<u32>>,
    request_rx: Receiver<ParticlePreparationRequest>,
    ready_tx: SyncSender<Result<PreparedParticleFrame, String>>,
) {
    let mut projection_cache = ParticleProjectionCache {
        commands: cached_commands,
        cohort_visible: [0; 2],
        phase: None,
        initialized: false,
    };
    while let Ok(request) = request_rx.recv() {
        let worker_wake_latency_us = request.sent_at.elapsed().as_micros();
        let mut commands = request.commands;
        let prepared = prepare_particle_frame(
            &mut engine,
            request.tick,
            request.elapsed,
            &mut commands,
            ordering_scratch.as_mut(),
            Some(&mut projection_cache),
        );
        let prepared = prepared.map(|mut prepared| {
            prepared.worker_wake_latency_us = worker_wake_latency_us;
            prepared.completed_at = Instant::now();
            prepared.commands = commands;
            prepared
        });
        let failed = prepared.is_err();
        if ready_tx.send(prepared).is_err() || failed {
            break;
        }
    }
}

fn prepare_particle_frame(
    engine: &mut ParticleEngine,
    tick: u64,
    elapsed: Duration,
    commands: &mut Vec<u32>,
    ordering_scratch: Option<&mut Vec<u32>>,
    projection_cache: Option<&mut ParticleProjectionCache>,
) -> Result<PreparedParticleFrame, String> {
    let simulation_started = Instant::now();
    let simulation_cpu_started = thread_cpu_time_us();
    let frame = engine.step(elapsed);
    let simulation_us = simulation_started.elapsed().as_micros();
    let simulation_cpu_us = elapsed_thread_cpu_us(simulation_cpu_started);
    let projection_started = Instant::now();
    let projection_cpu_started = thread_cpu_time_us();
    let visible = if let Some(projection_cache) = projection_cache {
        prepare_cached_particle_commands(engine, frame, commands, projection_cache)?
    } else {
        prepare_particle_commands(engine, commands)?
    };
    if let Some(ordering_scratch) = ordering_scratch {
        order_particle_commands(engine.config().preset, commands, ordering_scratch);
    }
    let projection_us = projection_started.elapsed().as_micros();
    let projection_cpu_us = elapsed_thread_cpu_us(projection_cpu_started);
    Ok(PreparedParticleFrame {
        tick,
        frame,
        visible,
        simulation_us,
        simulation_cpu_us,
        projection_us,
        projection_cpu_us,
        preparation_wait_us: 0,
        prepared_frame_age_us: 0,
        lookahead_mismatch_count: 0,
        preparation_queue_depth: 0,
        worker_wake_latency_us: 0,
        completed_at: Instant::now(),
        commands: Vec::new(),
    })
}

fn order_particle_commands(
    preset: ParticlePreset,
    commands: &mut Vec<u32>,
    scratch: &mut Vec<u32>,
) {
    let mut counts = [0usize; COMMAND_BIN_COUNT];
    for &command in commands.iter() {
        counts[particle_command_bin(preset, command)] += 1;
    }
    let mut positions = [0usize; COMMAND_BIN_COUNT];
    let mut next = 0usize;
    for (position, count) in positions.iter_mut().zip(counts) {
        *position = next;
        next += count;
    }
    scratch.clear();
    assert!(scratch.capacity() >= commands.len());
    let len = commands.len();
    let output = &mut scratch.spare_capacity_mut()[..len];
    for &command in commands.iter() {
        let bin = particle_command_bin(preset, command);
        output[positions[bin]].write(command);
        positions[bin] += 1;
    }
    // SAFETY: the stable counting pass initialized exactly `len` entries.
    unsafe {
        scratch.set_len(len);
    }
    std::mem::swap(commands, scratch);
}

fn particle_command_bin(preset: ParticlePreset, command: u32) -> usize {
    if command == PARTICLE_NOT_VISIBLE_OFFSET {
        return COMMAND_INVISIBLE_BIN;
    }
    let offset = match preset {
        ParticlePreset::Capacity => command,
        ParticlePreset::Visual => command & COMMAND_OFFSET_MASK,
    };
    ((offset >> COMMAND_BIN_SHIFT) as usize).min(COMMAND_INVISIBLE_BIN - 1)
}

fn prepare_cached_particle_commands(
    engine: &ParticleEngine,
    frame: ParticleFrameStats,
    commands: &mut Vec<u32>,
    cache: &mut ParticleProjectionCache,
) -> Result<usize, String> {
    if !engine.uses_vector_projection() {
        return prepare_particle_commands(engine, commands);
    }
    if frame.phase == ParticlePhase::Hold {
        cache.initialized = false;
        cache.phase = Some(frame.phase);
        return prepare_particle_commands(engine, commands);
    }
    let count = engine.particle_count();
    if cache.commands.len() != count {
        cache.commands.resize(count, PARTICLE_NOT_VISIBLE_OFFSET);
        cache.initialized = false;
    }
    let update = projection_cache_update(
        cache.initialized,
        cache.phase,
        frame.phase,
        frame.simulation_update,
    );
    let visual = engine.config().preset == ParticlePreset::Visual;
    match update {
        ProjectionCacheUpdate::None => {}
        ProjectionCacheUpdate::Cohort(cohort) => {
            let cohort = usize::from(cohort);
            let range = particle_cohort_range(count, cohort);
            cache.cohort_visible[cohort] =
                engine.project_packed_commands_range(&mut cache.commands, range, visual);
        }
        ProjectionCacheUpdate::All => {
            for cohort in 0..2 {
                cache.cohort_visible[cohort] = engine.project_packed_commands_range(
                    &mut cache.commands,
                    particle_cohort_range(count, cohort),
                    visual,
                );
            }
        }
    }
    cache.initialized = true;
    cache.phase = Some(frame.phase);
    if engine.validates_vector_projection() {
        validate_sampled_packed_commands(engine, &cache.commands)?;
    }
    commands.clear();
    commands.extend_from_slice(&cache.commands);
    Ok(cache.cohort_visible.iter().sum())
}

fn projection_cache_update(
    initialized: bool,
    cached_phase: Option<ParticlePhase>,
    phase: ParticlePhase,
    simulation_update: ParticleSimulationUpdate,
) -> ProjectionCacheUpdate {
    if !initialized || cached_phase != Some(phase) || phase == ParticlePhase::Hold {
        return ProjectionCacheUpdate::All;
    }
    match simulation_update {
        ParticleSimulationUpdate::None => ProjectionCacheUpdate::None,
        ParticleSimulationUpdate::Cohort(cohort @ 0..=1) => ProjectionCacheUpdate::Cohort(cohort),
        ParticleSimulationUpdate::Cohort(_) | ParticleSimulationUpdate::All => {
            ProjectionCacheUpdate::All
        }
    }
}

fn particle_cohort_range(count: usize, cohort: usize) -> std::ops::Range<usize> {
    let midpoint = count / 2;
    match cohort {
        0 => 0..midpoint,
        1 => midpoint..count,
        _ => 0..count,
    }
}

fn prepare_particle_commands(
    engine: &ParticleEngine,
    commands: &mut Vec<u32>,
) -> Result<usize, String> {
    commands.clear();
    let count = engine.particle_count();
    assert!(commands.capacity() >= count);
    let visible = engine.project_packed_commands(
        &mut commands.spare_capacity_mut()[..count],
        engine.config().preset == ParticlePreset::Visual,
    );
    // SAFETY: `project_packed_commands` initializes exactly `count` entries.
    unsafe {
        commands.set_len(count);
    }
    if engine.validates_vector_projection() {
        validate_sampled_packed_commands(engine, commands)?;
    }
    Ok(visible)
}

fn recipe_particle_has_neighbor(
    engine: &ParticleEngine,
    camera_depth: f32,
    palette_index: usize,
) -> bool {
    match engine.phase() {
        ParticlePhase::Form | ParticlePhase::Hold => {
            camera_depth < engine.recipe().appearance.formed_neighbor_when_depth_below
        }
        ParticlePhase::Static | ParticlePhase::Disperse => {
            palette_index == usize::from(engine.recipe().appearance.unformed_palette_index)
        }
    }
}

fn validate_sampled_packed_commands(
    engine: &ParticleEngine,
    commands: &[u32],
) -> Result<(), String> {
    const SAMPLE_COUNT: usize = 64;
    let width = engine.config().width;
    let height = engine.config().height;
    let visual = engine.config().preset == ParticlePreset::Visual;
    let stride = engine.particle_count().div_ceil(SAMPLE_COUNT).max(1);
    for index in (0..engine.particle_count()).step_by(stride) {
        let command = commands[index];
        let exact = engine.project(index);
        let Some(projected) = exact else {
            if command != PARTICLE_NOT_VISIBLE_OFFSET {
                return Err(format!(
                    "packed projection made invisible particle {index} visible"
                ));
            }
            continue;
        };
        if command == PARTICLE_NOT_VISIBLE_OFFSET {
            return Err(format!("packed projection hid visible particle {index}"));
        }
        let offset = (command & COMMAND_OFFSET_MASK) as usize;
        if offset >= width.saturating_mul(height) {
            return Err(format!(
                "packed projection emitted unsafe offset {offset} for particle {index}"
            ));
        }
        let approximate_x = offset % width;
        let approximate_y = offset / width;
        if approximate_x.abs_diff(projected.x as usize) > 1
            || approximate_y.abs_diff(projected.y as usize) > 1
        {
            return Err(format!(
                "packed projection exceeded one-pixel error for particle {index}"
            ));
        }
        if visual {
            let palette_index = (engine.flicker_key(index) >> 30) as usize;
            let actual_palette = ((command >> COMMAND_PALETTE_SHIFT) & 3) as usize;
            if actual_palette != palette_index {
                return Err(format!(
                    "packed projection changed palette semantics for particle {index}"
                ));
            }
            let expected_neighbor =
                recipe_particle_has_neighbor(engine, projected.depth, palette_index)
                    && projected.x + 1 < width as i32;
            let actual_neighbor = command & COMMAND_NEIGHBOR != 0;
            if actual_neighbor != expected_neighbor
                || (actual_neighbor && offset + 1 >= width.saturating_mul(height))
            {
                return Err(format!(
                    "packed projection changed neighbor semantics for particle {index}"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn visual_particle_has_neighbor(
    phase: ParticlePhase,
    camera_depth: f32,
    palette_index: usize,
) -> bool {
    match phase {
        ParticlePhase::Form | ParticlePhase::Hold => camera_depth < 0.0,
        ParticlePhase::Static | ParticlePhase::Disperse => {
            palette_index == VISUAL_PALETTE.len() - 1
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stats(
    frame: ParticleFrameStats,
    visible: usize,
    simulation_us: u128,
    simulation_cpu_us: u128,
    projection_us: u128,
    projection_cpu_us: u128,
    preparation_wait_us: u128,
    prepared_frame_age_us: u128,
    lookahead_mismatch_count: u64,
    preparation_queue_depth: usize,
    worker_wake_latency_us: u128,
    clear_us: u128,
    clear_cpu_us: u128,
    raster_us: u128,
    raster_cpu_us: u128,
    simulation_bytes: usize,
    renderer_scratch_bytes: usize,
) -> MagikSceneStats {
    MagikSceneStats {
        count: frame.count,
        visible,
        phase: frame.phase,
        cycle: frame.cycle,
        simulation_backend: frame.simulation_backend,
        projection_backend: frame.projection_backend,
        simulation_us,
        simulation_cpu_us,
        projection_us,
        projection_cpu_us,
        preparation_wait_us,
        prepared_frame_age_us,
        lookahead_mismatch_count,
        preparation_queue_depth,
        worker_wake_latency_us,
        clear_us,
        clear_cpu_us,
        raster_us,
        raster_cpu_us,
        rotation_y_millidegrees: (frame.rotation_y_radians * (180_000.0 / std::f32::consts::PI)
            + 0.5) as u32,
        simulation_bytes,
        renderer_scratch_bytes,
    }
}

#[cfg(target_os = "linux")]
fn thread_cpu_time_us() -> Option<u128> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is writable for the duration of the call.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0 {
        return None;
    }
    let seconds = u128::try_from(time.tv_sec).ok()?;
    let nanoseconds = u128::try_from(time.tv_nsec).ok()?;
    Some(
        seconds
            .saturating_mul(1_000_000)
            .saturating_add(nanoseconds / 1_000),
    )
}

#[cfg(not(target_os = "linux"))]
fn thread_cpu_time_us() -> Option<u128> {
    None
}

fn elapsed_thread_cpu_us(started: Option<u128>) -> u128 {
    started
        .zip(thread_cpu_time_us())
        .map_or(0, |(started, finished)| finished.saturating_sub(started))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask() -> TargetMask {
        TargetMask::from_alpha(4, 4, 4, &[255; 16], 128, 1).unwrap()
    }

    fn config(preset: ParticlePreset) -> ParticleConfig {
        ParticleConfig {
            count: 64,
            width: 32,
            height: 24,
            seed: 7,
            preset,
        }
    }

    fn scene(preset: ParticlePreset) -> MagikScene {
        MagikScene::new_with_preparation(
            config(preset),
            mask(),
            MagikSceneOptions::default(),
            PreparationMode::Synchronous,
        )
        .unwrap()
    }

    fn lookahead_scene(preset: ParticlePreset) -> MagikScene {
        MagikScene::new_with_preparation(
            config(preset),
            mask(),
            MagikSceneOptions::default(),
            PreparationMode::Lookahead,
        )
        .unwrap()
    }

    fn buffer(value: u8) -> SceneBufferId {
        SceneBufferId::new(value, 2).unwrap()
    }

    fn render_at_hold(renderer: &mut MagikScene) -> Vec<Rgb565Pixel> {
        let mut pixels = vec![Rgb565Pixel(0); 32 * 24];
        for frame in 0..=360 {
            renderer
                .render_frame(
                    &mut pixels,
                    buffer(0),
                    Duration::from_micros(frame * 16_667),
                )
                .unwrap();
        }
        pixels
    }

    #[test]
    fn magik_mask_fits_the_960_by_540_viewport() {
        let mask = magik_target_mask().unwrap();
        assert!(mask.width() < 960);
        assert!(mask.height() < 540);
        assert!(mask.points().len() > 1_000);
    }

    #[test]
    fn capacity_preset_draws_only_single_particle_pixels() {
        let mut renderer = scene(ParticlePreset::Capacity);
        let pixels = render_at_hold(&mut renderer);
        assert!(pixels.contains(&CAPACITY_COLOR));
        assert!(pixels.iter().all(|pixel| matches!(pixel.0, 0 | 0xbdf7)));
    }

    #[test]
    fn visual_preset_uses_the_phosphor_palette() {
        let mut renderer = scene(ParticlePreset::Visual);
        let pixels = render_at_hold(&mut renderer);
        assert!(pixels.iter().any(|pixel| VISUAL_PALETTE.contains(pixel)));
        assert!(
            pixels
                .iter()
                .all(|pixel| pixel.0 == 0 || VISUAL_PALETTE.contains(pixel))
        );
    }

    #[test]
    fn visual_footprint_uses_depth_only_while_the_word_is_formed() {
        for phase in [ParticlePhase::Form, ParticlePhase::Hold] {
            assert!(visual_particle_has_neighbor(phase, -0.25, 0));
            assert!(!visual_particle_has_neighbor(
                phase,
                0.0,
                VISUAL_PALETTE.len() - 1
            ));
            assert!(!visual_particle_has_neighbor(
                phase,
                10.0,
                VISUAL_PALETTE.len() - 1
            ));
        }
        for phase in [ParticlePhase::Static, ParticlePhase::Disperse] {
            assert!(visual_particle_has_neighbor(
                phase,
                10.0,
                VISUAL_PALETTE.len() - 1
            ));
            assert!(!visual_particle_has_neighbor(phase, -10.0, 0));
        }
    }

    #[test]
    fn destination_geometry_must_match_exactly() {
        let mut renderer = scene(ParticlePreset::Capacity);
        assert!(
            renderer
                .render_with_lookahead(&mut [Rgb565Pixel(0); 1], buffer(0), Duration::ZERO, None,)
                .unwrap_err()
                .contains("expected 768")
        );
    }

    #[test]
    fn hidden_slots_clear_only_their_own_previous_pixels() {
        let mut renderer = scene(ParticlePreset::Capacity);
        renderer.dirty_slots[0].initialized = true;
        renderer.dirty_slots[0].offsets.extend([1, 3]);
        renderer.dirty_slots[1].initialized = true;
        renderer.dirty_slots[1].offsets.push(2);
        let mut slot1 = [Rgb565Pixel(0x1234); 32 * 24];
        let mut slot2 = [Rgb565Pixel(0x5678); 32 * 24];
        let _slot1_offsets = renderer.prepare_hidden_slot(&mut slot1, 0);
        let _slot2_offsets = renderer.prepare_hidden_slot(&mut slot2, 1);
        assert_eq!(slot1[1], Rgb565Pixel(0));
        assert_eq!(slot1[3], Rgb565Pixel(0));
        assert_eq!(slot2[2], Rgb565Pixel(0));
        assert_eq!(slot1[2], Rgb565Pixel(0x1234));
        assert_eq!(slot2[1], Rgb565Pixel(0x5678));
    }

    #[test]
    fn visual_dirty_history_records_every_written_pixel_without_growth() {
        let mut renderer = scene(ParticlePreset::Visual);
        let command_capacity = renderer.commands.capacity();
        let capacities = renderer
            .dirty_slots
            .iter()
            .map(|slot| slot.offsets.capacity())
            .collect::<Vec<_>>();
        let mut pixels = vec![Rgb565Pixel(0); 32 * 24];
        renderer
            .render_frame(&mut pixels, buffer(0), Duration::from_secs(6))
            .unwrap();
        for (offset, pixel) in pixels.iter().enumerate() {
            if *pixel != Rgb565Pixel(0) {
                assert!(renderer.dirty_slots[0].offsets.contains(&(offset as u32)));
            }
        }
        renderer
            .render_frame(&mut pixels, buffer(0), Duration::from_micros(6_016_667))
            .unwrap();
        assert_eq!(
            capacities,
            renderer
                .dirty_slots
                .iter()
                .map(|slot| slot.offsets.capacity())
                .collect::<Vec<_>>()
        );
        assert_eq!(renderer.commands.capacity(), command_capacity);
    }

    #[test]
    fn renderer_memory_accounts_for_simulation_and_both_dirty_slots() {
        let capacity = scene(ParticlePreset::Capacity);
        assert_eq!(capacity.simulation_bytes, 64 * 31);
        assert_eq!(capacity.renderer_scratch_bytes, 64 * 12);
        let visual = scene(ParticlePreset::Visual);
        assert_eq!(visual.simulation_bytes, 64 * 31);
        assert_eq!(visual.renderer_scratch_bytes, 64 * 20);
    }

    #[test]
    fn invalidation_forces_a_full_clear_on_next_slot_use() {
        let mut renderer = scene(ParticlePreset::Capacity);
        let mut pixels = vec![Rgb565Pixel(0); 32 * 24];
        renderer
            .render_frame(&mut pixels, buffer(0), Duration::ZERO)
            .unwrap();
        pixels[767] = Rgb565Pixel(0xffff);
        renderer.invalidate(buffer(0));
        let _offsets = renderer.prepare_hidden_slot(&mut pixels, 0);
        assert_eq!(pixels[767], Rgb565Pixel(0));
    }

    #[test]
    fn dense_dirty_history_uses_the_adaptive_full_clear() {
        let mut renderer = scene(ParticlePreset::Capacity);
        renderer.dirty_slots[0].initialized = true;
        renderer.dirty_slots[0].offsets.extend(0..192);
        let mut pixels = vec![Rgb565Pixel(0xffff); 32 * 24];
        let _offsets = renderer.prepare_hidden_slot(&mut pixels, 0);
        assert!(pixels.iter().all(|pixel| *pixel == Rgb565Pixel(0)));
    }

    #[test]
    fn reusable_buffer_indices_are_strictly_bounded() {
        assert_eq!(SceneBufferId::new(0, 2).unwrap().get(), 0);
        assert_eq!(SceneBufferId::new(1, 2).unwrap().get(), 1);
        assert!(SceneBufferId::new(2, 2).is_err());
    }

    #[test]
    fn preparation_pipeline_returns_exact_lookahead_frames_in_order() {
        let engine = ParticleEngine::new(config(ParticlePreset::Capacity), mask()).unwrap();
        let mut pipeline = ParticlePreparationPipeline::start(
            engine,
            [Vec::with_capacity(64), Vec::with_capacity(64)],
            Vec::with_capacity(64),
            None,
            None,
        )
        .unwrap();
        let mut commands = Vec::with_capacity(64);
        let first_elapsed = Duration::from_micros(16_667);
        let second_elapsed = Duration::from_micros(33_334);
        let first = pipeline
            .acquire(first_elapsed, Some(second_elapsed), &mut commands)
            .unwrap();
        assert_eq!(first.tick, 0);
        assert_eq!(first.lookahead_mismatch_count, 0);
        assert_eq!(first.preparation_queue_depth, 0);
        assert_eq!(commands.len(), first.frame.count);
        assert_eq!(
            commands
                .iter()
                .filter(|command| **command != PARTICLE_NOT_VISIBLE_OFFSET)
                .count(),
            first.visible
        );
        let first_commands = commands.clone();
        let jittered_second_elapsed = second_elapsed + Duration::from_micros(500);
        let second = pipeline
            .acquire(jittered_second_elapsed, None, &mut commands)
            .unwrap();
        assert_eq!(second.tick, 1);
        assert_eq!(second.lookahead_mismatch_count, 0);
        assert_eq!(second.preparation_queue_depth, 2);
        assert_eq!(commands.len(), second.frame.count);
        assert_eq!(
            commands
                .iter()
                .filter(|command| **command != PARTICLE_NOT_VISIBLE_OFFSET)
                .count(),
            second.visible
        );
        assert_ne!(commands, first_commands);
        let third_elapsed = Duration::from_micros(50_001);
        let fourth_elapsed = Duration::from_micros(66_668);
        let third = pipeline
            .acquire(third_elapsed, Some(fourth_elapsed), &mut commands)
            .unwrap();
        assert_eq!(third.tick, 2);
        assert_eq!(third.lookahead_mismatch_count, 0);
        assert_eq!(third.preparation_queue_depth, 1);
        assert_eq!(pipeline.in_flight.len(), 2);
    }

    #[test]
    fn realtime_construction_always_starts_the_preparation_pipeline() {
        let renderer =
            MagikScene::new(config(ParticlePreset::Visual), mask(), Default::default()).unwrap();
        assert!(renderer.preparation_pipeline.is_some());
        assert!(renderer.engine.is_none());
    }

    #[test]
    fn synchronous_and_lookahead_frames_match_across_reusable_buffers() {
        let mut synchronous = scene(ParticlePreset::Visual);
        let mut lookahead = lookahead_scene(ParticlePreset::Visual);
        let mut synchronous_pixels = [vec![Rgb565Pixel(0); 32 * 24], vec![Rgb565Pixel(0); 32 * 24]];
        let mut lookahead_pixels = synchronous_pixels.clone();
        for frame in 0..24_u64 {
            let buffer_index = (frame % 2) as usize;
            let elapsed = Duration::from_micros(frame * 16_667);
            let next_elapsed = Duration::from_micros((frame + 1) * 16_667);
            let synchronous_stats = synchronous
                .render_with_lookahead(
                    &mut synchronous_pixels[buffer_index],
                    buffer(buffer_index as u8),
                    elapsed,
                    Some(next_elapsed),
                )
                .unwrap();
            let lookahead_stats = lookahead
                .render_with_lookahead(
                    &mut lookahead_pixels[buffer_index],
                    buffer(buffer_index as u8),
                    elapsed,
                    Some(next_elapsed),
                )
                .unwrap();
            assert_eq!(lookahead_stats.phase, synchronous_stats.phase);
            assert_eq!(lookahead_stats.visible, synchronous_stats.visible);
            assert_eq!(
                lookahead_pixels[buffer_index],
                synchronous_pixels[buffer_index]
            );
        }
    }

    #[test]
    fn preparation_ticks_roll_over_without_elapsed_matching() {
        let engine = ParticleEngine::new(config(ParticlePreset::Capacity), mask()).unwrap();
        let mut pipeline = ParticlePreparationPipeline::start(
            engine,
            [Vec::with_capacity(64), Vec::with_capacity(64)],
            Vec::with_capacity(64),
            None,
            None,
        )
        .unwrap();
        pipeline.presentation_tick = u64::MAX;
        let mut commands = Vec::with_capacity(64);
        let first_elapsed = Duration::from_micros(16_667);
        let second_elapsed = Duration::from_micros(33_334);
        let first = pipeline
            .acquire(first_elapsed, Some(second_elapsed), &mut commands)
            .unwrap();
        let second = pipeline
            .acquire(
                second_elapsed + Duration::from_micros(900),
                None,
                &mut commands,
            )
            .unwrap();
        assert_eq!(first.tick, u64::MAX);
        assert_eq!(second.tick, 0);
        assert_eq!(second.lookahead_mismatch_count, 0);
    }

    #[test]
    fn projection_cache_reprojects_phase_changes_and_rotating_hold() {
        assert_eq!(
            projection_cache_update(
                false,
                None,
                ParticlePhase::Static,
                ParticleSimulationUpdate::Cohort(0),
            ),
            ProjectionCacheUpdate::All
        );
        assert_eq!(
            projection_cache_update(
                true,
                Some(ParticlePhase::Static),
                ParticlePhase::Static,
                ParticleSimulationUpdate::Cohort(1),
            ),
            ProjectionCacheUpdate::Cohort(1)
        );
        assert_eq!(
            projection_cache_update(
                true,
                Some(ParticlePhase::Static),
                ParticlePhase::Form,
                ParticleSimulationUpdate::Cohort(0),
            ),
            ProjectionCacheUpdate::All
        );
        assert_eq!(
            projection_cache_update(
                true,
                Some(ParticlePhase::Hold),
                ParticlePhase::Hold,
                ParticleSimulationUpdate::Cohort(1),
            ),
            ProjectionCacheUpdate::All
        );
    }

    #[test]
    fn command_ordering_is_stable_within_contiguous_framebuffer_bins() {
        let mut commands = vec![4_100, 8, 4_096, 7, PARTICLE_NOT_VISIBLE_OFFSET];
        let mut scratch = Vec::with_capacity(commands.len());
        order_particle_commands(ParticlePreset::Capacity, &mut commands, &mut scratch);
        assert_eq!(
            commands,
            vec![8, 7, 4_100, 4_096, PARTICLE_NOT_VISIBLE_OFFSET]
        );
    }

    #[test]
    fn visual_command_ordering_uses_only_the_packed_pixel_offset() {
        let first = pack_visual_command(4_100, 3, true);
        let second = pack_visual_command(8, 0, false);
        let third = pack_visual_command(4_096, 1, false);
        let mut commands = vec![first, PARTICLE_NOT_VISIBLE_OFFSET, second, third];
        let mut scratch = Vec::with_capacity(commands.len());
        order_particle_commands(ParticlePreset::Visual, &mut commands, &mut scratch);
        assert_eq!(
            commands,
            vec![second, first, third, PARTICLE_NOT_VISIBLE_OFFSET]
        );
    }

    #[test]
    fn sampled_packed_visual_validation_enforces_safety_and_semantics() {
        let mut engine = ParticleEngine::new(config(ParticlePreset::Visual), mask()).unwrap();
        engine.step(Duration::from_millis(5_000));
        let width = engine.config().width;
        let mut commands = (0..engine.particle_count())
            .map(|index| {
                engine
                    .project(index)
                    .map_or(PARTICLE_NOT_VISIBLE_OFFSET, |particle| {
                        let offset = (particle.y as usize * width + particle.x as usize) as u32;
                        let palette = (engine.flicker_key(index) >> 30) as usize;
                        let neighbor =
                            visual_particle_has_neighbor(engine.phase(), particle.depth, palette)
                                && particle.x + 1 < width as i32;
                        pack_visual_command(offset, palette, neighbor)
                    })
            })
            .collect::<Vec<_>>();
        validate_sampled_packed_commands(&engine, &commands).unwrap();

        let sampled = (0..engine.particle_count())
            .step_by(engine.particle_count().div_ceil(64).max(1))
            .find(|index| commands[*index] != PARTICLE_NOT_VISIBLE_OFFSET)
            .unwrap();
        commands[sampled] = (commands[sampled] & !COMMAND_OFFSET_MASK) | COMMAND_OFFSET_MASK;
        assert!(
            validate_sampled_packed_commands(&engine, &commands)
                .unwrap_err()
                .contains("unsafe offset")
        );
    }
}
