// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::bitmap_text::{ConsoleFont, ConsoleTypeface};
use crate::particle_engine::{
    PARTICLE_NOT_VISIBLE_OFFSET, ParticleConfig, ParticleEngine, ParticleFrameStats, ParticlePhase,
    ParticlePreset, TargetMask,
};
use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use slint::platform::software_renderer::Rgb565Pixel;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAGIK_FONT_PX: f32 = 128.0;
const MAGIK_TEXT: &str = "MagiK";
const MAGIK_MASK_THRESHOLD: u8 = 128;
const MAGIK_MASK_SAMPLE_STEP: usize = 2;
const CAPACITY_COLOR: Rgb565Pixel = Rgb565Pixel(0xbdf7);
const VISUAL_PALETTE: [Rgb565Pixel; 4] = [
    Rgb565Pixel(0x2104),
    Rgb565Pixel(0x5aeb),
    Rgb565Pixel(0xbdf7),
    Rgb565Pixel(0xffff),
];
const HIDDEN_SLOT_COUNT: usize = 2;
const FULL_CLEAR_DIRTY_DIVISOR: usize = 4;
const COMMAND_OFFSET_BITS: u32 = 20;
const COMMAND_OFFSET_MASK: u32 = (1 << COMMAND_OFFSET_BITS) - 1;
const COMMAND_PALETTE_SHIFT: u32 = COMMAND_OFFSET_BITS;
const COMMAND_NEIGHBOR: u32 = 1 << (COMMAND_PALETTE_SHIFT + 2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParticleRenderStats {
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
    pub clear_us: u128,
    pub clear_cpu_us: u128,
    pub raster_us: u128,
    pub raster_cpu_us: u128,
    pub render_cpu_start: u64,
    pub render_cpu_end: u64,
    pub voluntary_context_switches: u64,
    pub involuntary_context_switches: u64,
    pub pmu_available: bool,
    pub pmu_cycles: u64,
    pub pmu_instructions: u64,
    pub pmu_cache_references: u64,
    pub pmu_cache_misses: u64,
    pub pmu_branch_instructions: u64,
    pub pmu_branch_misses: u64,
    pub rotation_y_millidegrees: u32,
    pub simulation_bytes: usize,
    pub renderer_scratch_bytes: usize,
}

pub struct ParticleRenderer {
    config: ParticleConfig,
    engine: Option<ParticleEngine>,
    preparation_pipeline: Option<ParticlePreparationPipeline>,
    dirty_slots: [ParticleDirtySlot; HIDDEN_SLOT_COUNT],
    simulation_bytes: usize,
    renderer_scratch_bytes: usize,
    commands: Vec<u32>,
    pmu: ParticlePmu,
}

struct ParticleDirtySlot {
    initialized: bool,
    offsets: Vec<u32>,
}

struct ParticlePreparationRequest {
    elapsed: Duration,
    commands: Vec<u32>,
}

struct PreparedParticleFrame {
    elapsed: Duration,
    frame: ParticleFrameStats,
    visible: usize,
    simulation_us: u128,
    simulation_cpu_us: u128,
    projection_us: u128,
    projection_cpu_us: u128,
    commands: Vec<u32>,
}

struct ParticlePreparationPipeline {
    request_tx: Option<SyncSender<ParticlePreparationRequest>>,
    ready_rx: Receiver<PreparedParticleFrame>,
    in_flight: Option<Duration>,
    spare_commands: Option<Vec<u32>>,
    worker: Option<JoinHandle<()>>,
}

impl ParticleRenderer {
    pub fn new_magik(config: ParticleConfig) -> Result<Self, String> {
        let mask = magik_target_mask()?;
        Self::new(config, mask)
    }

    fn new(config: ParticleConfig, mask: TargetMask) -> Result<Self, String> {
        let write_capacity = match config.preset {
            ParticlePreset::Capacity => config.count,
            ParticlePreset::Visual => config.count.saturating_mul(2),
        };
        let simulation_bytes = config
            .count
            .saturating_mul(ParticleEngine::bytes_per_particle());
        let dirty_slots = std::array::from_fn(|_| ParticleDirtySlot {
            initialized: false,
            offsets: Vec::with_capacity(write_capacity),
        });
        let mut engine = Some(ParticleEngine::new(config, mask)?);
        let commands = Vec::with_capacity(config.count);
        let preparation_pipeline = if particle_pipeline_requested() {
            Some(ParticlePreparationPipeline::start(
                engine
                    .take()
                    .expect("particle preparation pipeline must receive its engine"),
                Vec::with_capacity(config.count),
            )?)
        } else {
            None
        };
        let command_buffer_count = 1 + usize::from(preparation_pipeline.is_some());
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
            engine,
            preparation_pipeline,
            dirty_slots,
            simulation_bytes,
            renderer_scratch_bytes,
            commands,
            pmu: ParticlePmu::from_env(),
        })
    }

    pub fn preset(&self) -> ParticlePreset {
        self.config.preset
    }

    pub fn particle_count(&self) -> usize {
        self.config.count
    }

    pub fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        hidden_slot: u8,
        elapsed: Duration,
    ) -> Result<ParticleRenderStats, String> {
        self.render_with_lookahead(destination, hidden_slot, elapsed, None)
    }

    pub fn render_with_lookahead(
        &mut self,
        destination: &mut [Rgb565Pixel],
        hidden_slot: u8,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
    ) -> Result<ParticleRenderStats, String> {
        let frame_len = self.config.width.saturating_mul(self.config.height);
        if destination.len() != frame_len {
            return Err(format!(
                "particle destination has {} pixels, expected {frame_len}",
                destination.len()
            ));
        }
        let slot_offset = hidden_slot_offset(hidden_slot)?;
        self.pmu.begin();
        let execution_started = thread_execution_snapshot();
        let prepared = if let Some(pipeline) = self.preparation_pipeline.as_mut() {
            pipeline.acquire(elapsed, next_elapsed, &mut self.commands)?
        } else {
            prepare_particle_frame(
                self.engine
                    .as_mut()
                    .expect("same-thread particle renderer must own its engine"),
                elapsed,
                &mut self.commands,
            )
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
        let execution_finished = thread_execution_snapshot();
        let pmu = self.pmu.finish();
        self.dirty_slots[slot_offset].offsets = dirty_offsets;
        Ok(stats(
            prepared.frame,
            prepared.visible,
            prepared.simulation_us,
            prepared.simulation_cpu_us,
            prepared.projection_us,
            prepared.projection_cpu_us,
            clear_us,
            clear_cpu_us,
            raster_us,
            raster_cpu_us,
            execution_started,
            execution_finished,
            pmu,
            self.simulation_bytes,
            self.renderer_scratch_bytes,
        ))
    }

    pub fn invalidate_hidden_slot(&mut self, hidden_slot: u8) {
        if let Ok(slot_offset) = hidden_slot_offset(hidden_slot) {
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
            destination.fill(Rgb565Pixel(0));
        } else {
            for &offset in &slot.offsets {
                if offset != PARTICLE_NOT_VISIBLE_OFFSET {
                    destination[offset as usize] = Rgb565Pixel(0);
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
        for &command in &self.commands {
            let offset = (command & COMMAND_OFFSET_MASK) as usize;
            let palette_index = ((command >> COMMAND_PALETTE_SHIFT) & 3) as usize;
            destination[offset] = VISUAL_PALETTE[palette_index];
            dirty_offsets.push(offset as u32);
            if command & COMMAND_NEIGHBOR != 0 {
                destination[offset + 1] = VISUAL_PALETTE[2];
                dirty_offsets.push((offset + 1) as u32);
            }
        }
    }
}

impl ParticlePreparationPipeline {
    fn start(engine: ParticleEngine, spare_commands: Vec<u32>) -> Result<Self, String> {
        let (request_tx, request_rx) = mpsc::sync_channel::<ParticlePreparationRequest>(1);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<PreparedParticleFrame>(1);
        let worker = std::thread::Builder::new()
            .name("particle-prepare".into())
            .spawn(move || {
                apply_runtime_thread_policy(RuntimeThreadRole::ParticlePreparer);
                run_particle_preparation_worker(engine, request_rx, ready_tx);
            })
            .map_err(|error| format!("spawn particle preparation worker: {error}"))?;
        Ok(Self {
            request_tx: Some(request_tx),
            ready_rx,
            in_flight: None,
            spare_commands: Some(spare_commands),
            worker: Some(worker),
        })
    }

    fn acquire(
        &mut self,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
        commands: &mut Vec<u32>,
    ) -> Result<PreparedParticleFrame, String> {
        let mut prepared = if self.in_flight.take().is_some() {
            self.receive()?
        } else {
            self.send(elapsed)?;
            self.receive()?
        };
        if prepared.elapsed != elapsed {
            self.spare_commands = Some(std::mem::take(&mut prepared.commands));
            self.send(elapsed)?;
            prepared = self.receive()?;
        }
        debug_assert_eq!(prepared.elapsed, elapsed);
        std::mem::swap(commands, &mut prepared.commands);
        self.spare_commands = Some(std::mem::take(&mut prepared.commands));
        if let Some(next_elapsed) = next_elapsed.filter(|next| *next > elapsed) {
            self.send(next_elapsed)?;
            self.in_flight = Some(next_elapsed);
        }
        Ok(prepared)
    }

    fn send(&mut self, elapsed: Duration) -> Result<(), String> {
        let commands = self
            .spare_commands
            .take()
            .ok_or("particle preparation pipeline has no spare command buffer")?;
        self.request_tx
            .as_ref()
            .ok_or("particle preparation worker has stopped")?
            .send(ParticlePreparationRequest { elapsed, commands })
            .map_err(|_| "particle preparation worker disconnected".to_string())
    }

    fn receive(&self) -> Result<PreparedParticleFrame, String> {
        self.ready_rx
            .recv()
            .map_err(|_| "particle preparation worker disconnected".to_string())
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
    request_rx: Receiver<ParticlePreparationRequest>,
    ready_tx: SyncSender<PreparedParticleFrame>,
) {
    while let Ok(request) = request_rx.recv() {
        let mut commands = request.commands;
        let mut prepared = prepare_particle_frame(&mut engine, request.elapsed, &mut commands);
        prepared.commands = commands;
        if ready_tx.send(prepared).is_err() {
            break;
        }
    }
}

fn prepare_particle_frame(
    engine: &mut ParticleEngine,
    elapsed: Duration,
    commands: &mut Vec<u32>,
) -> PreparedParticleFrame {
    let simulation_started = Instant::now();
    let simulation_cpu_started = thread_cpu_time_us();
    let frame = engine.step(elapsed);
    let simulation_us = simulation_started.elapsed().as_micros();
    let simulation_cpu_us = elapsed_thread_cpu_us(simulation_cpu_started);
    let projection_started = Instant::now();
    let projection_cpu_started = thread_cpu_time_us();
    let visible = prepare_particle_commands(engine, commands);
    let projection_us = projection_started.elapsed().as_micros();
    let projection_cpu_us = elapsed_thread_cpu_us(projection_cpu_started);
    PreparedParticleFrame {
        elapsed,
        frame,
        visible,
        simulation_us,
        simulation_cpu_us,
        projection_us,
        projection_cpu_us,
        commands: Vec::new(),
    }
}

fn prepare_particle_commands(engine: &ParticleEngine, commands: &mut Vec<u32>) -> usize {
    commands.clear();
    match engine.config().preset {
        ParticlePreset::Capacity => {
            if engine.uses_vector_projection() {
                let count = engine.particle_count();
                assert!(commands.capacity() >= count);
                let visible = engine.project_offsets(&mut commands.spare_capacity_mut()[..count]);
                // SAFETY: `project_offsets` initialized exactly `count` entries.
                unsafe {
                    commands.set_len(count);
                }
                visible
            } else {
                let width = engine.config().width;
                for index in 0..engine.particle_count() {
                    if let Some(particle) = engine.project(index) {
                        commands.push((particle.y as usize * width + particle.x as usize) as u32);
                    }
                }
                commands.len()
            }
        }
        ParticlePreset::Visual => {
            let width = engine.config().width;
            for index in 0..engine.particle_count() {
                let Some(particle) = engine.project(index) else {
                    continue;
                };
                let offset = (particle.y as usize * width + particle.x as usize) as u32;
                let palette_index = (engine.flicker_key(index) >> 30) as usize;
                let neighbor =
                    visual_particle_has_neighbor(engine.phase(), particle.depth, palette_index)
                        && particle.x + 1 < width as i32;
                commands.push(pack_visual_command(offset, palette_index, neighbor));
            }
            commands.len()
        }
    }
}

fn particle_pipeline_requested() -> bool {
    cfg!(all(target_os = "linux", target_arch = "arm"))
        && !matches!(
            std::env::var("MISTER_PARTICLE_PIPELINE").ok().as_deref(),
            Some("0" | "off" | "false" | "no")
        )
}

fn pack_visual_command(offset: u32, palette_index: usize, neighbor: bool) -> u32 {
    debug_assert!(offset <= COMMAND_OFFSET_MASK);
    offset
        | ((palette_index as u32) << COMMAND_PALETTE_SHIFT)
        | if neighbor { COMMAND_NEIGHBOR } else { 0 }
}

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

fn hidden_slot_offset(hidden_slot: u8) -> Result<usize, String> {
    match hidden_slot {
        1 | 2 => Ok(usize::from(hidden_slot - 1)),
        _ => Err(format!(
            "particle hidden slot must be 1 or 2, got {hidden_slot}"
        )),
    }
}

fn magik_target_mask() -> Result<TargetMask, String> {
    let mut font = ConsoleFont::new_with_typeface(MAGIK_FONT_PX, ConsoleTypeface::PressStart2P);
    let alpha = font
        .rasterize_alpha_mask(MAGIK_TEXT)
        .ok_or("Press Start 2P produced no MagiK alpha mask")?;
    TargetMask::from_alpha(
        alpha.width,
        alpha.height,
        alpha.stride,
        &alpha.alpha,
        MAGIK_MASK_THRESHOLD,
        MAGIK_MASK_SAMPLE_STEP,
    )
}

fn stats(
    frame: ParticleFrameStats,
    visible: usize,
    simulation_us: u128,
    simulation_cpu_us: u128,
    projection_us: u128,
    projection_cpu_us: u128,
    clear_us: u128,
    clear_cpu_us: u128,
    raster_us: u128,
    raster_cpu_us: u128,
    execution_started: ThreadExecutionSnapshot,
    execution_finished: ThreadExecutionSnapshot,
    pmu: ParticlePmuSample,
    simulation_bytes: usize,
    renderer_scratch_bytes: usize,
) -> ParticleRenderStats {
    ParticleRenderStats {
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
        clear_us,
        clear_cpu_us,
        raster_us,
        raster_cpu_us,
        render_cpu_start: execution_started.cpu,
        render_cpu_end: execution_finished.cpu,
        voluntary_context_switches: execution_finished
            .voluntary_context_switches
            .saturating_sub(execution_started.voluntary_context_switches),
        involuntary_context_switches: execution_finished
            .involuntary_context_switches
            .saturating_sub(execution_started.involuntary_context_switches),
        pmu_available: pmu.available,
        pmu_cycles: pmu.cycles,
        pmu_instructions: pmu.instructions,
        pmu_cache_references: pmu.cache_references,
        pmu_cache_misses: pmu.cache_misses,
        pmu_branch_instructions: pmu.branch_instructions,
        pmu_branch_misses: pmu.branch_misses,
        rotation_y_millidegrees: (frame.rotation_y_radians * (180_000.0 / std::f32::consts::PI)
            + 0.5) as u32,
        simulation_bytes,
        renderer_scratch_bytes,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ParticlePmuSample {
    available: bool,
    cycles: u64,
    instructions: u64,
    cache_references: u64,
    cache_misses: u64,
    branch_instructions: u64,
    branch_misses: u64,
}

struct ParticlePmu {
    requested: bool,
    initialization_attempted: bool,
    #[cfg(target_os = "linux")]
    counters: Option<PerfCounterGroup>,
}

impl ParticlePmu {
    fn from_env() -> Self {
        Self {
            requested: std::env::var_os("MISTER_PARTICLE_PMU").is_some_and(|value| value == "1"),
            initialization_attempted: false,
            #[cfg(target_os = "linux")]
            counters: None,
        }
    }

    fn begin(&mut self) {
        if !self.requested {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            if !self.initialization_attempted {
                self.initialization_attempted = true;
                self.counters = PerfCounterGroup::open().ok();
            }
            if let Some(counters) = self.counters.as_mut()
                && counters.begin().is_err()
            {
                self.counters = None;
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.initialization_attempted = true;
        }
    }

    fn finish(&mut self) -> ParticlePmuSample {
        #[cfg(target_os = "linux")]
        {
            let Some(counters) = self.counters.as_mut() else {
                return ParticlePmuSample::default();
            };
            return counters.finish().unwrap_or_default();
        }
        #[cfg(not(target_os = "linux"))]
        ParticlePmuSample::default()
    }
}

#[cfg(target_os = "linux")]
const PERF_EVENT_CONFIGS: [u64; 6] = [0, 1, 2, 3, 4, 5];
#[cfg(target_os = "linux")]
const PERF_TYPE_HARDWARE: u32 = 0;
#[cfg(target_os = "linux")]
const PERF_FORMAT_GROUP: u64 = 1 << 3;
#[cfg(target_os = "linux")]
const PERF_ATTR_DISABLED: u64 = 1;
#[cfg(target_os = "linux")]
const PERF_ATTR_EXCLUDE_KERNEL: u64 = 1 << 5;
#[cfg(target_os = "linux")]
const PERF_ATTR_EXCLUDE_HYPERVISOR: u64 = 1 << 6;
#[cfg(target_os = "linux")]
const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 1 << 3;
#[cfg(target_os = "linux")]
const PERF_EVENT_IOC_ENABLE: libc::c_ulong = 0x2400;
#[cfg(target_os = "linux")]
const PERF_EVENT_IOC_DISABLE: libc::c_ulong = 0x2401;
#[cfg(target_os = "linux")]
const PERF_EVENT_IOC_RESET: libc::c_ulong = 0x2403;
#[cfg(target_os = "linux")]
const PERF_IOC_FLAG_GROUP: libc::c_ulong = 1;

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Default)]
struct PerfEventAttr {
    event_type: u32,
    size: u32,
    config: u64,
    sample_period: u64,
    sample_type: u64,
    read_format: u64,
    flags: u64,
    wakeup_events: u32,
    breakpoint_type: u32,
    config1: u64,
}

#[cfg(target_os = "linux")]
struct PerfCounterGroup {
    descriptors: Vec<libc::c_int>,
}

#[cfg(target_os = "linux")]
impl PerfCounterGroup {
    fn open() -> std::io::Result<Self> {
        let leader = open_perf_event(PERF_EVENT_CONFIGS[0], -1)?;
        let mut group = Self {
            descriptors: vec![leader],
        };
        for config in PERF_EVENT_CONFIGS.iter().copied().skip(1) {
            group
                .descriptors
                .push(open_perf_event(config, group.descriptors[0])?);
        }
        Ok(group)
    }

    fn begin(&mut self) -> std::io::Result<()> {
        perf_group_ioctl(self.descriptors[0], PERF_EVENT_IOC_RESET)?;
        perf_group_ioctl(self.descriptors[0], PERF_EVENT_IOC_ENABLE)
    }

    fn finish(&mut self) -> std::io::Result<ParticlePmuSample> {
        perf_group_ioctl(self.descriptors[0], PERF_EVENT_IOC_DISABLE)?;
        let mut values = [0_u64; PERF_EVENT_CONFIGS.len() + 1];
        let expected_bytes = std::mem::size_of_val(&values);
        // SAFETY: `values` is writable for `expected_bytes` and the descriptor
        // is owned by this group.
        let read_bytes = unsafe {
            libc::read(
                self.descriptors[0],
                values.as_mut_ptr().cast(),
                expected_bytes,
            )
        };
        if read_bytes != expected_bytes as isize || values[0] != PERF_EVENT_CONFIGS.len() as u64 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(ParticlePmuSample {
            available: true,
            cycles: values[1],
            instructions: values[2],
            cache_references: values[3],
            cache_misses: values[4],
            branch_instructions: values[5],
            branch_misses: values[6],
        })
    }
}

#[cfg(target_os = "linux")]
impl Drop for PerfCounterGroup {
    fn drop(&mut self) {
        for descriptor in self.descriptors.drain(..) {
            // SAFETY: every descriptor was returned by `perf_event_open` and
            // ownership remains with this group.
            unsafe {
                libc::close(descriptor);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn open_perf_event(config: u64, group_descriptor: libc::c_int) -> std::io::Result<libc::c_int> {
    let attributes = PerfEventAttr {
        event_type: PERF_TYPE_HARDWARE,
        size: u32::try_from(std::mem::size_of::<PerfEventAttr>()).unwrap_or(u32::MAX),
        config,
        read_format: PERF_FORMAT_GROUP,
        flags: PERF_ATTR_DISABLED | PERF_ATTR_EXCLUDE_KERNEL | PERF_ATTR_EXCLUDE_HYPERVISOR,
        ..PerfEventAttr::default()
    };
    // SAFETY: the syscall receives a valid attribute pointer and requests
    // counters for the calling thread on any CPU.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &attributes,
            0,
            -1,
            group_descriptor,
            PERF_FLAG_FD_CLOEXEC,
        )
    };
    let descriptor =
        libc::c_int::try_from(descriptor).map_err(|_| std::io::Error::last_os_error())?;
    if descriptor < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(descriptor)
    }
}

#[cfg(target_os = "linux")]
fn perf_group_ioctl(descriptor: libc::c_int, request: libc::c_ulong) -> std::io::Result<()> {
    // SAFETY: the descriptor is the live leader of the owned event group.
    if unsafe { libc::ioctl(descriptor, request, PERF_IOC_FLAG_GROUP) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ThreadExecutionSnapshot {
    cpu: u64,
    voluntary_context_switches: u64,
    involuntary_context_switches: u64,
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

#[cfg(target_os = "linux")]
fn thread_execution_snapshot() -> ThreadExecutionSnapshot {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` points to writable, correctly sized storage.
    let usage_available = unsafe { libc::getrusage(libc::RUSAGE_THREAD, usage.as_mut_ptr()) } == 0;
    // SAFETY: a successful `getrusage` initialized the complete value. On failure
    // the zeroed representation is valid for the integer-only C structure.
    let usage = unsafe { usage.assume_init() };
    let cpu = unsafe { libc::sched_getcpu() };
    ThreadExecutionSnapshot {
        cpu: u64::try_from(cpu).unwrap_or(u64::MAX),
        voluntary_context_switches: usage_available
            .then(|| u64::try_from(usage.ru_nvcsw).unwrap_or(0))
            .unwrap_or(0),
        involuntary_context_switches: usage_available
            .then(|| u64::try_from(usage.ru_nivcsw).unwrap_or(0))
            .unwrap_or(0),
    }
}

#[cfg(not(target_os = "linux"))]
fn thread_execution_snapshot() -> ThreadExecutionSnapshot {
    ThreadExecutionSnapshot {
        cpu: u64::MAX,
        ..ThreadExecutionSnapshot::default()
    }
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

    fn render_at_hold(renderer: &mut ParticleRenderer) -> Vec<Rgb565Pixel> {
        let mut pixels = vec![Rgb565Pixel(0); 32 * 24];
        for frame in 0..=360 {
            renderer
                .render(&mut pixels, 1, Duration::from_micros(frame * 16_667))
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
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        let pixels = render_at_hold(&mut renderer);
        assert!(pixels.iter().any(|pixel| *pixel == CAPACITY_COLOR));
        assert!(pixels.iter().all(|pixel| matches!(pixel.0, 0 | 0xbdf7)));
    }

    #[test]
    fn visual_preset_uses_the_phosphor_palette() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Visual), mask()).unwrap();
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
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        assert!(
            renderer
                .render(&mut [Rgb565Pixel(0); 1], 1, Duration::ZERO)
                .unwrap_err()
                .contains("expected 768")
        );
    }

    #[test]
    fn hidden_slots_clear_only_their_own_previous_pixels() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
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
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Visual), mask()).unwrap();
        let command_capacity = renderer.commands.capacity();
        let capacities = renderer
            .dirty_slots
            .each_ref()
            .map(|slot| slot.offsets.capacity());
        let mut pixels = vec![Rgb565Pixel(0); 32 * 24];
        renderer
            .render(&mut pixels, 1, Duration::from_secs(6))
            .unwrap();
        for (offset, pixel) in pixels.iter().enumerate() {
            if *pixel != Rgb565Pixel(0) {
                assert!(renderer.dirty_slots[0].offsets.contains(&(offset as u32)));
            }
        }
        renderer
            .render(&mut pixels, 1, Duration::from_micros(6_016_667))
            .unwrap();
        assert_eq!(
            capacities,
            renderer
                .dirty_slots
                .each_ref()
                .map(|slot| slot.offsets.capacity())
        );
        assert_eq!(renderer.commands.capacity(), command_capacity);
    }

    #[test]
    fn renderer_memory_accounts_for_simulation_and_both_dirty_slots() {
        let capacity = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        assert_eq!(capacity.simulation_bytes, 64 * 33);
        assert_eq!(capacity.renderer_scratch_bytes, 64 * 12);
        let visual = ParticleRenderer::new(config(ParticlePreset::Visual), mask()).unwrap();
        assert_eq!(visual.simulation_bytes, 64 * 33);
        assert_eq!(visual.renderer_scratch_bytes, 64 * 20);
    }

    #[test]
    fn invalidation_forces_a_full_clear_on_next_slot_use() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        let mut pixels = vec![Rgb565Pixel(0); 32 * 24];
        renderer.render(&mut pixels, 1, Duration::ZERO).unwrap();
        pixels[767] = Rgb565Pixel(0xffff);
        renderer.invalidate_hidden_slot(1);
        let _offsets = renderer.prepare_hidden_slot(&mut pixels, 0);
        assert_eq!(pixels[767], Rgb565Pixel(0));
    }

    #[test]
    fn dense_dirty_history_uses_the_adaptive_full_clear() {
        let mut renderer = ParticleRenderer::new(config(ParticlePreset::Capacity), mask()).unwrap();
        renderer.dirty_slots[0].initialized = true;
        renderer.dirty_slots[0].offsets.extend(0..192);
        let mut pixels = vec![Rgb565Pixel(0xffff); 32 * 24];
        let _offsets = renderer.prepare_hidden_slot(&mut pixels, 0);
        assert!(pixels.iter().all(|pixel| *pixel == Rgb565Pixel(0)));
    }

    #[test]
    fn hidden_slot_indices_are_strictly_bounded() {
        assert!(hidden_slot_offset(0).is_err());
        assert_eq!(hidden_slot_offset(1), Ok(0));
        assert_eq!(hidden_slot_offset(2), Ok(1));
        assert!(hidden_slot_offset(3).is_err());
    }

    #[test]
    fn preparation_pipeline_returns_exact_lookahead_frames_in_order() {
        let engine = ParticleEngine::new(config(ParticlePreset::Capacity), mask()).unwrap();
        let mut pipeline =
            ParticlePreparationPipeline::start(engine, Vec::with_capacity(64)).unwrap();
        let mut commands = Vec::with_capacity(64);
        let first_elapsed = Duration::from_micros(16_667);
        let second_elapsed = Duration::from_micros(33_334);
        let first = pipeline
            .acquire(first_elapsed, Some(second_elapsed), &mut commands)
            .unwrap();
        assert_eq!(first.elapsed, first_elapsed);
        assert_eq!(commands.len(), first.visible);
        let first_commands = commands.clone();
        let second = pipeline
            .acquire(second_elapsed, None, &mut commands)
            .unwrap();
        assert_eq!(second.elapsed, second_elapsed);
        assert_eq!(commands.len(), second.visible);
        assert_ne!(commands, first_commands);
    }
}
