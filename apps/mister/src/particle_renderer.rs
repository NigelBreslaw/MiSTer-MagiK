// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Production adapter for the portable MagiK RGB565 scene.

use crate::particle_engine::{ParticleConfig, ParticlePhase, ParticlePreset};
use mister_magik_catalog::runtime_thread::{RuntimeThreadRole, apply_runtime_thread_policy};
use mister_magik_framebuffer_scenes::{Rgb565Pixel as SharedRgb565Pixel, SceneBufferId};
use mister_magik_particles::magik::{MagikScene, MagikSceneOptions, MagikSceneStats};
use mister_magik_particles::recipes::MagikRecipe;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Duration;

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
    pub preparation_wait_us: u128,
    pub prepared_frame_age_us: u128,
    pub lookahead_mismatch_count: u64,
    pub preparation_queue_depth: usize,
    pub worker_wake_latency_us: u128,
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
    scene: MagikScene,
    reusable_buffers: u8,
    pmu: ParticlePmu,
}

impl ParticleRenderer {
    pub fn new_magik(config: ParticleConfig) -> Result<Self, String> {
        let options = production_options();
        Ok(Self {
            scene: MagikScene::new_magik_with_options(config, options)?,
            reusable_buffers: options.reusable_buffers,
            pmu: ParticlePmu::from_env(),
        })
    }

    pub fn from_magik_recipe(
        width: usize,
        height: usize,
        preset: ParticlePreset,
        recipe: MagikRecipe,
    ) -> Result<Self, String> {
        let options = production_options();
        Ok(Self {
            scene: MagikScene::from_magik_recipe_with_options(
                width, height, preset, recipe, options,
            )?,
            reusable_buffers: options.reusable_buffers,
            pmu: ParticlePmu::from_env(),
        })
    }

    #[must_use]
    pub fn preset(&self) -> ParticlePreset {
        self.scene.preset()
    }

    #[must_use]
    pub fn particle_count(&self) -> usize {
        self.scene.particle_count()
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
        let buffer = hardware_slot_to_scene_buffer(hidden_slot, self.reusable_buffers)?;
        self.pmu.begin();
        let execution_started = thread_execution_snapshot();
        let shared = slint_rgb565_as_shared_mut(destination);
        let stats = self
            .scene
            .render_with_lookahead(shared, buffer, elapsed, next_elapsed)?;
        let execution_finished = thread_execution_snapshot();
        let pmu = self.pmu.finish();
        Ok(merge_stats(
            stats,
            execution_started,
            execution_finished,
            pmu,
        ))
    }

    pub fn invalidate_hidden_slot(&mut self, hidden_slot: u8) {
        if let Ok(buffer) = hardware_slot_to_scene_buffer(hidden_slot, self.reusable_buffers) {
            self.scene.invalidate(buffer);
        }
    }
}

fn production_options() -> MagikSceneOptions {
    let order_commands = matches!(
        std::env::var("MISTER_PARTICLE_COMMAND_ORDER")
            .ok()
            .as_deref(),
        Some("1" | "on" | "true" | "locality")
    );
    MagikSceneOptions {
        order_commands,
        reusable_buffers: 2,
        worker_start: Some(start_particle_preparer),
    }
}

fn start_particle_preparer() {
    apply_runtime_thread_policy(RuntimeThreadRole::ParticlePreparer);
}

fn hardware_slot_to_scene_buffer(
    hidden_slot: u8,
    reusable_buffers: u8,
) -> Result<SceneBufferId, String> {
    let zero_based = hidden_slot
        .checked_sub(1)
        .ok_or_else(|| format!("particle hidden slot must be 1 or 2, got {hidden_slot}"))?;
    SceneBufferId::new(zero_based, reusable_buffers).map_err(|_| {
        format!("particle hidden slot must be 1 or {reusable_buffers}, got {hidden_slot}")
    })
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
    // SAFETY: Slint's `Rgb565Pixel` and the portable scene pixel are transparent
    // `u16` wrappers with equal size and alignment. Every `u16` value is valid,
    // and the returned mutable slice retains the input slice's exact lifetime.
    unsafe {
        std::slice::from_raw_parts_mut(
            destination.as_mut_ptr().cast::<SharedRgb565Pixel>(),
            destination.len(),
        )
    }
}

fn merge_stats(
    shared: MagikSceneStats,
    execution_started: ThreadExecutionSnapshot,
    execution_finished: ThreadExecutionSnapshot,
    pmu: ParticlePmuSample,
) -> ParticleRenderStats {
    ParticleRenderStats {
        count: shared.count,
        visible: shared.visible,
        phase: shared.phase,
        cycle: shared.cycle,
        simulation_backend: shared.simulation_backend,
        projection_backend: shared.projection_backend,
        simulation_us: shared.simulation_us,
        simulation_cpu_us: shared.simulation_cpu_us,
        projection_us: shared.projection_us,
        projection_cpu_us: shared.projection_cpu_us,
        preparation_wait_us: shared.preparation_wait_us,
        prepared_frame_age_us: shared.prepared_frame_age_us,
        lookahead_mismatch_count: shared.lookahead_mismatch_count,
        preparation_queue_depth: shared.preparation_queue_depth,
        worker_wake_latency_us: shared.worker_wake_latency_us,
        clear_us: shared.clear_us,
        clear_cpu_us: shared.clear_cpu_us,
        raster_us: shared.raster_us,
        raster_cpu_us: shared.raster_cpu_us,
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
        rotation_y_millidegrees: shared.rotation_y_millidegrees,
        simulation_bytes: shared.simulation_bytes,
        renderer_scratch_bytes: shared.renderer_scratch_bytes,
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
fn thread_execution_snapshot() -> ThreadExecutionSnapshot {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` points to writable, correctly sized storage.
    let usage_available = unsafe { libc::getrusage(libc::RUSAGE_THREAD, usage.as_mut_ptr()) } == 0;
    // SAFETY: successful `getrusage` initializes the value; the zeroed integer
    // representation is valid when the call fails.
    let usage = unsafe { usage.assume_init() };
    // SAFETY: `sched_getcpu` has no pointer arguments and does not retain state.
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

    #[test]
    fn hardware_slots_map_to_zero_based_scene_buffers() {
        assert_eq!(hardware_slot_to_scene_buffer(1, 2).unwrap().get(), 0);
        assert_eq!(hardware_slot_to_scene_buffer(2, 2).unwrap().get(), 1);
        assert!(hardware_slot_to_scene_buffer(0, 2).is_err());
        assert!(hardware_slot_to_scene_buffer(3, 2).is_err());
    }

    #[test]
    fn production_configures_the_dedicated_preparation_worker() {
        assert!(production_options().worker_start.is_some());
    }
}
