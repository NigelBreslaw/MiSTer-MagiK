// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic scalar particle simulation for software-rendered effects.

use std::time::Duration;
use std::{mem::MaybeUninit, ops::Range};

pub const PARTICLE_COUNT_MAX: usize = 524_288;
pub const PARTICLE_CYCLE: Duration = Duration::from_secs(10);
const STATIC_END_US: u64 = 3_000_000;
const FORM_END_US: u64 = 5_000_000;
const HOLD_END_US: u64 = 8_000_000;
const CYCLE_US: u64 = 10_000_000;
const MAX_STEP_SECONDS: f32 = 1.0 / 15.0;
const DEPTH_EXTENT: f32 = 64.0;
const DEPTH_FIXED_SCALE: f32 = 128.0;
const DEPTH_FIXED_SCALE_RECIP: f32 = 1.0 / DEPTH_FIXED_SCALE;
const FOCAL_LENGTH: f32 = 720.0;
const RECIPROCAL_TABLE_MIN: f32 = 192.0;
const RECIPROCAL_TABLE_MAX: f32 = 1_248.0;
const RECIPROCAL_TABLE_STEP: f32 = 4.0;
const RECIPROCAL_TABLE_STEP_RECIP: f32 = 1.0 / RECIPROCAL_TABLE_STEP;
const RECIPROCAL_TABLE_COUNT: usize =
    ((RECIPROCAL_TABLE_MAX - RECIPROCAL_TABLE_MIN) / RECIPROCAL_TABLE_STEP) as usize + 1;
const TARGET_FIXED_SCALE: f32 = 16.0;
const TARGET_FIXED_SCALE_RECIP: f32 = 1.0 / TARGET_FIXED_SCALE;
const TARGET_DEPTH_Q2_HALF_EXTENT: i8 = 40;
const TARGET_DEPTH_LEVELS: u64 = 81;
const TARGET_DEPTH_Q2_RECIP: f32 = 0.25;
const HOLD_DURATION_US: u64 = HOLD_END_US - FORM_END_US;
pub const PARTICLE_NOT_VISIBLE_OFFSET: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticlePreset {
    Capacity,
    Visual,
}

impl ParticlePreset {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Capacity => "capacity",
            Self::Visual => "visual",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "capacity" => Some(Self::Capacity),
            "visual" => Some(Self::Visual),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParticlePhase {
    Static,
    Form,
    Hold,
    Disperse,
}

impl ParticlePhase {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Form => "form",
            Self::Hold => "hold",
            Self::Disperse => "disperse",
        }
    }

    #[must_use]
    pub const fn at_cycle_us(cycle_us: u64) -> Self {
        if cycle_us < STATIC_END_US {
            Self::Static
        } else if cycle_us < FORM_END_US {
            Self::Form
        } else if cycle_us < HOLD_END_US {
            Self::Hold
        } else {
            Self::Disperse
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleTarget {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetMask {
    width: usize,
    height: usize,
    points: Vec<ParticleTarget>,
}

impl TargetMask {
    pub fn from_alpha(
        width: usize,
        height: usize,
        stride: usize,
        alpha: &[u8],
        threshold: u8,
        sample_step: usize,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("particle target mask dimensions must be nonzero".into());
        }
        if stride < width || alpha.len() < stride.saturating_mul(height) {
            return Err("particle target alpha buffer is smaller than its geometry".into());
        }
        if sample_step == 0 {
            return Err("particle target sample step must be nonzero".into());
        }
        let mut points = Vec::new();
        for y in (0..height).step_by(sample_step) {
            for x in (0..width).step_by(sample_step) {
                if alpha[y * stride + x] >= threshold {
                    points.push(ParticleTarget {
                        x: x as f32,
                        y: y as f32,
                    });
                }
            }
        }
        if points.is_empty() {
            return Err("particle target mask contains no sampled opaque pixels".into());
        }
        Ok(Self {
            width,
            height,
            points,
        })
    }

    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    #[must_use]
    pub fn points(&self) -> &[ParticleTarget] {
        &self.points
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleConfig {
    pub count: usize,
    pub width: usize,
    pub height: usize,
    pub seed: u64,
    pub preset: ParticlePreset,
}

impl ParticleConfig {
    pub fn validate(self) -> Result<Self, String> {
        if self.count == 0 || self.count > PARTICLE_COUNT_MAX {
            return Err(format!(
                "particle count must be in 1..={PARTICLE_COUNT_MAX}"
            ));
        }
        if self.width == 0 || self.height == 0 {
            return Err("particle viewport dimensions must be nonzero".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleFrameStats {
    pub count: usize,
    pub phase: ParticlePhase,
    pub cycle: u64,
    pub rotation_y_radians: f32,
    pub simulation_backend: &'static str,
    pub projection_backend: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedParticle {
    pub x: i32,
    pub y: i32,
    pub depth: f32,
}

#[derive(Debug)]
pub struct ParticleEngine {
    config: ParticleConfig,
    projection_center_x: f32,
    projection_center_y: f32,
    projection_max_x: f32,
    projection_max_y: f32,
    reciprocal_table: [f32; RECIPROCAL_TABLE_COUNT],
    rotation_y_radians: f32,
    rotation_y_sin: f32,
    rotation_y_cos: f32,
    packed_targets: Vec<u32>,
    target_depth_q2: Vec<i8>,
    x: Vec<f32>,
    y: Vec<f32>,
    z_q7: Vec<i16>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    vz: Vec<f32>,
    random_states: Vec<u32>,
    last_elapsed: Duration,
    cohort_elapsed: [Duration; 2],
    simulation_step: u64,
    cycle: u64,
    phase: ParticlePhase,
    use_neon: bool,
    use_neon_projection: bool,
    validate_neon_projection: bool,
    use_table_projection: bool,
    use_alternating_cohorts: bool,
}

impl ParticleEngine {
    pub fn new(config: ParticleConfig, mask: TargetMask) -> Result<Self, String> {
        let config = config.validate()?;
        if mask.width > config.width || mask.height > config.height {
            return Err("particle target mask does not fit the viewport".into());
        }
        let offset_x = (config.width - mask.width) as f32 * 0.5;
        let offset_y = (config.height - mask.height) as f32 * 0.5;
        let target_points = mask
            .points
            .into_iter()
            .map(|point| ParticleTarget {
                x: point.x + offset_x,
                y: point.y + offset_y,
            })
            .collect::<Vec<_>>();
        let mut engine = Self {
            config,
            projection_center_x: config.width as f32 * 0.5,
            projection_center_y: config.height as f32 * 0.5,
            projection_max_x: config.width as f32 - 0.5,
            projection_max_y: config.height as f32 - 0.5,
            reciprocal_table: std::array::from_fn(|index| {
                1.0 / (RECIPROCAL_TABLE_MIN + index as f32 * RECIPROCAL_TABLE_STEP)
            }),
            rotation_y_radians: 0.0,
            rotation_y_sin: 0.0,
            rotation_y_cos: 1.0,
            packed_targets: Vec::with_capacity(config.count),
            target_depth_q2: Vec::with_capacity(config.count),
            x: Vec::with_capacity(config.count),
            y: Vec::with_capacity(config.count),
            z_q7: Vec::with_capacity(config.count),
            vx: Vec::with_capacity(config.count),
            vy: Vec::with_capacity(config.count),
            vz: Vec::with_capacity(config.count),
            random_states: Vec::with_capacity(config.count),
            last_elapsed: Duration::ZERO,
            cohort_elapsed: [Duration::ZERO; 2],
            simulation_step: 0,
            cycle: 0,
            phase: ParticlePhase::Static,
            use_neon: particle_neon_enabled(),
            use_neon_projection: particle_neon_projection_enabled(),
            validate_neon_projection: particle_neon_projection_validation_enabled(),
            use_table_projection: particle_table_projection_enabled(),
            use_alternating_cohorts: particle_alternating_cohorts_enabled(),
        };
        engine.initialize_particles(&target_points)?;
        Ok(engine)
    }

    #[must_use]
    pub const fn config(&self) -> ParticleConfig {
        self.config
    }

    #[must_use]
    pub const fn phase(&self) -> ParticlePhase {
        self.phase
    }

    #[must_use]
    pub fn particle_count(&self) -> usize {
        self.x.len()
    }

    #[must_use]
    pub const fn bytes_per_particle() -> usize {
        7 * std::mem::size_of::<u32>() + std::mem::size_of::<i16>() + std::mem::size_of::<i8>()
    }

    pub fn step(&mut self, elapsed: Duration) -> ParticleFrameStats {
        let elapsed_us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        let next_cycle = elapsed_us / CYCLE_US;
        if next_cycle != self.cycle {
            self.cycle = next_cycle;
        }
        let cycle_us = elapsed_us % CYCLE_US;
        let next_phase = ParticlePhase::at_cycle_us(cycle_us);
        let delta = elapsed
            .saturating_sub(self.last_elapsed)
            .as_secs_f32()
            .min(MAX_STEP_SECONDS);
        self.last_elapsed = elapsed;
        self.phase = next_phase;
        self.rotation_y_radians = rotation_y_at_cycle_us(cycle_us);
        (self.rotation_y_sin, self.rotation_y_cos) = self.rotation_y_radians.sin_cos();
        if delta > 0.0 {
            if self.use_alternating_cohorts {
                let cohort = (self.simulation_step & 1) as usize;
                let cohort_delta = elapsed
                    .saturating_sub(self.cohort_elapsed[cohort])
                    .as_secs_f32()
                    .min(MAX_STEP_SECONDS);
                self.cohort_elapsed[cohort] = elapsed;
                let midpoint = self.particle_count() / 2;
                let range = if cohort == 0 {
                    0..midpoint
                } else {
                    midpoint..self.particle_count()
                };
                self.advance_range(cohort_delta, range);
                self.simulation_step = self.simulation_step.wrapping_add(1);
            } else {
                self.advance_range(delta, 0..self.particle_count());
            }
        }
        ParticleFrameStats {
            count: self.particle_count(),
            phase: self.phase,
            cycle: self.cycle,
            rotation_y_radians: self.rotation_y_radians,
            simulation_backend: self.simulation_backend_label(),
            projection_backend: self.projection_backend_label(),
        }
    }

    #[must_use]
    #[inline(always)]
    pub fn project(&self, index: usize) -> Option<ProjectedParticle> {
        let (rotated_x, rotated_z) = rotate_xz(
            self.x[index] - self.projection_center_x,
            self.depth(index),
            self.rotation_y_sin,
            self.rotation_y_cos,
        );
        let denominator = FOCAL_LENGTH + rotated_z;
        if denominator <= 1.0 {
            return None;
        }
        let relative_y = self.y[index] - self.projection_center_y;
        let (mut scale, scale_error) = self.projection_scale(denominator);
        let mut screen_x = self.projection_center_x + rotated_x * scale;
        let mut screen_y = self.projection_center_y + relative_y * scale;
        if scale_error > 0.0
            && (projection_coordinate_needs_exact(
                screen_x,
                self.projection_max_x,
                rotated_x.abs() * scale_error,
            ) || projection_coordinate_needs_exact(
                screen_y,
                self.projection_max_y,
                relative_y.abs() * scale_error,
            ))
        {
            scale = FOCAL_LENGTH / denominator;
            screen_x = self.projection_center_x + rotated_x * scale;
            screen_y = self.projection_center_y + relative_y * scale;
        }
        if screen_x <= -0.5
            || screen_y <= -0.5
            || screen_x >= self.projection_max_x
            || screen_y >= self.projection_max_y
        {
            return None;
        }
        let x = (screen_x + 0.5) as i32;
        let y = (screen_y + 0.5) as i32;
        Some(ProjectedParticle {
            x,
            y,
            depth: rotated_z,
        })
    }

    /// Projects every particle into a packed RGB565 pixel offset. The caller
    /// supplies uninitialized storage so the renderer can reuse its dirty list
    /// without an extra fill pass.
    pub fn project_offsets(&self, output: &mut [MaybeUninit<u32>]) -> usize {
        self.project_packed_commands(output, false)
    }

    /// Projects every particle into a fixed index-ordered packed command.
    /// Invisible particles receive `PARTICLE_NOT_VISIBLE_OFFSET`.
    pub fn project_packed_commands(&self, output: &mut [MaybeUninit<u32>], visual: bool) -> usize {
        assert!(output.len() >= self.particle_count());
        #[cfg(target_arch = "arm")]
        if self.use_neon_projection {
            // SAFETY: the C kernel writes exactly one offset or sentinel for
            // every particle and never retains any supplied pointer.
            return unsafe { neon::project_commands(self, output.as_mut_ptr().cast(), visual) };
        }
        debug_assert!(
            !visual,
            "visual packed projection requires the NEON backend"
        );
        self.project_offsets_scalar(output, 0..self.particle_count())
    }

    fn project_offsets_scalar(
        &self,
        output: &mut [MaybeUninit<u32>],
        range: Range<usize>,
    ) -> usize {
        let mut visible = 0usize;
        for index in range {
            let offset = self
                .project(index)
                .map_or(PARTICLE_NOT_VISIBLE_OFFSET, |particle| {
                    visible += 1;
                    (particle.y as usize * self.config.width + particle.x as usize) as u32
                });
            output[index].write(offset);
        }
        visible
    }

    #[must_use]
    pub const fn projection_backend_label(&self) -> &'static str {
        if self.use_neon_projection {
            "armv7-neon-packed-r1"
        } else if self.use_table_projection {
            "scalar-table-corrected"
        } else {
            "scalar-exact"
        }
    }

    #[must_use]
    pub const fn uses_vector_projection(&self) -> bool {
        self.use_neon_projection
    }

    #[must_use]
    pub const fn validates_vector_projection(&self) -> bool {
        self.validate_neon_projection
    }

    #[must_use]
    #[inline(always)]
    pub fn flicker_key(&self, index: usize) -> u32 {
        self.random_states[index]
    }

    fn initialize_particles(&mut self, target_points: &[ParticleTarget]) -> Result<(), String> {
        let point_count = target_points.len();
        for index in 0..self.config.count {
            let seed = nonzero_random_state(mix32(
                self.config.seed as u32
                    ^ (self.config.seed >> 32) as u32
                    ^ index as u32
                    ^ 0x9e37_79b9,
            ));
            let target_index = if self.config.count <= point_count {
                index.saturating_mul(point_count) / self.config.count
            } else {
                index % point_count
            };
            let mut target = target_points[target_index];
            if self.config.count > point_count {
                target.x += signed_unit(mix32(seed ^ 0xbb67_ae85)) * 0.4;
                target.y += signed_unit(mix32(seed ^ 0x3c6e_f372)) * 0.4;
            }
            self.packed_targets.push(pack_target(target)?);
            self.target_depth_q2
                .push(distributed_target_depth_q2(mix32(seed ^ 0x510e_527f)));
            self.random_states.push(seed);
            self.x.push(0.0);
            self.y.push(0.0);
            self.z_q7.push(0);
            self.vx.push(0.0);
            self.vy.push(0.0);
            self.vz.push(0.0);
        }
        self.initialize_scatter();
        Ok(())
    }

    fn initialize_scatter(&mut self) {
        let width = self.config.width as f32;
        let height = self.config.height as f32;
        for index in 0..self.particle_count() {
            let seed = self.random_states[index];
            self.x[index] = unit_float(mix32(seed ^ 0xa511_e9b3)) * width;
            self.y[index] = unit_float(mix32(seed ^ 0x63d8_3595)) * height;
            self.z_q7[index] = pack_depth(signed_unit(mix32(seed ^ 0x7f4a_7c15)) * DEPTH_EXTENT);
            self.vx[index] = signed_unit(mix32(seed ^ 0x94d0_49bb)) * 42.0;
            self.vy[index] = signed_unit(mix32(seed ^ 0x2c1b_3c6d)) * 42.0;
            self.vz[index] = signed_unit(mix32(seed ^ 0x297a_2d39)) * 10.0;
        }
    }

    fn advance_range(&mut self, delta: f32, range: Range<usize>) {
        match self.phase {
            ParticlePhase::Static => self.advance_static(delta, range),
            ParticlePhase::Form => self.advance_form(delta, range),
            ParticlePhase::Hold => self.advance_hold(delta, range),
            ParticlePhase::Disperse => self.advance_disperse(delta, range),
        }
    }

    const fn simulation_backend_label(&self) -> &'static str {
        if self.use_neon && self.use_alternating_cohorts {
            "armv7-neon-cohort30"
        } else if self.use_neon {
            "armv7-neon"
        } else if self.use_alternating_cohorts {
            "scalar-cohort30"
        } else {
            "scalar"
        }
    }

    fn advance_static(&mut self, delta: f32, range: Range<usize>) {
        #[cfg(target_arch = "arm")]
        let first_scalar = if self.use_neon {
            // SAFETY: device builds target Cortex-A9 with NEON enabled. The
            // backend operates only on complete four-particle groups.
            unsafe { neon::advance_static(self, delta, range.clone()) }
        } else {
            range.start
        };
        #[cfg(not(target_arch = "arm"))]
        let first_scalar = range.start;
        self.advance_static_scalar(delta, first_scalar..range.end);
    }

    fn advance_static_scalar(&mut self, delta: f32, range: Range<usize>) {
        let width = self.config.width as f32;
        let height = self.config.height as f32;
        for index in range {
            let noise = next_random(&mut self.random_states[index]);
            let jitter_x = signed_unit(noise);
            let jitter_y = signed_unit(noise.rotate_left(11));
            let vx = (self.vx[index] + jitter_x * 75.0 * delta) * 0.985;
            let vy = (self.vy[index] + jitter_y * 75.0 * delta) * 0.985;
            let vz = (self.vz[index] + signed_unit(noise.rotate_left(21)) * 8.0 * delta) * 0.98;
            self.vx[index] = vx;
            self.vy[index] = vy;
            self.vz[index] = vz;
            self.x[index] = wrap_coordinate(self.x[index] + vx * delta, width);
            self.y[index] = wrap_coordinate(self.y[index] + vy * delta, height);
            let next_z = (self.depth(index) + vz * delta).clamp(-DEPTH_EXTENT, DEPTH_EXTENT);
            self.z_q7[index] = pack_depth(next_z);
        }
    }

    fn advance_form(&mut self, delta: f32, range: Range<usize>) {
        #[cfg(target_arch = "arm")]
        let first_scalar = if self.use_neon {
            // SAFETY: see `advance_static`.
            unsafe { neon::advance_attract(self, delta, 18.0, 0.08, 0.88, range.clone()) }
        } else {
            range.start
        };
        #[cfg(not(target_arch = "arm"))]
        let first_scalar = range.start;
        self.advance_form_scalar(delta, first_scalar..range.end);
    }

    fn advance_form_scalar(&mut self, delta: f32, range: Range<usize>) {
        for index in range {
            let noise = next_random(&mut self.random_states[index]);
            let jitter_x = signed_unit(noise);
            let jitter_y = signed_unit(noise.rotate_left(11));
            let (target_x, target_y, target_z) = self.target_components(index);
            let x = self.x[index];
            let y = self.y[index];
            let z = self.depth(index);
            let vx = (self.vx[index] + (target_x + jitter_x * 0.08 - x) * 18.0 * delta) * 0.88;
            let vy = (self.vy[index] + (target_y + jitter_y * 0.08 - y) * 18.0 * delta) * 0.88;
            let vz = (self.vz[index] + (target_z - z) * 18.0 * delta) * 0.88;
            self.vx[index] = vx;
            self.vy[index] = vy;
            self.vz[index] = vz;
            self.x[index] = x + vx * delta;
            self.y[index] = y + vy * delta;
            self.z_q7[index] = pack_depth((z + vz * delta).clamp(-DEPTH_EXTENT, DEPTH_EXTENT));
        }
    }

    fn advance_hold(&mut self, delta: f32, range: Range<usize>) {
        #[cfg(target_arch = "arm")]
        let first_scalar = if self.use_neon {
            // SAFETY: see `advance_static`.
            unsafe { neon::advance_attract(self, delta, 34.0, 0.35, 0.78, range.clone()) }
        } else {
            range.start
        };
        #[cfg(not(target_arch = "arm"))]
        let first_scalar = range.start;
        self.advance_hold_scalar(delta, first_scalar..range.end);
    }

    fn advance_hold_scalar(&mut self, delta: f32, range: Range<usize>) {
        for index in range {
            let noise = next_random(&mut self.random_states[index]);
            let jitter_x = signed_unit(noise);
            let jitter_y = signed_unit(noise.rotate_left(11));
            let (target_x, target_y, target_z) = self.target_components(index);
            let x = self.x[index];
            let y = self.y[index];
            let z = self.depth(index);
            let vx = (self.vx[index] + (target_x + jitter_x * 0.35 - x) * 34.0 * delta) * 0.78;
            let vy = (self.vy[index] + (target_y + jitter_y * 0.35 - y) * 34.0 * delta) * 0.78;
            let vz = (self.vz[index] + (target_z - z) * 34.0 * delta) * 0.78;
            self.vx[index] = vx;
            self.vy[index] = vy;
            self.vz[index] = vz;
            self.x[index] = x + vx * delta;
            self.y[index] = y + vy * delta;
            self.z_q7[index] = pack_depth((z + vz * delta).clamp(-DEPTH_EXTENT, DEPTH_EXTENT));
        }
    }

    fn advance_disperse(&mut self, delta: f32, range: Range<usize>) {
        #[cfg(target_arch = "arm")]
        let first_scalar = if self.use_neon {
            // SAFETY: see `advance_static`.
            unsafe { neon::advance_disperse(self, delta, range.clone()) }
        } else {
            range.start
        };
        #[cfg(not(target_arch = "arm"))]
        let first_scalar = range.start;
        self.advance_disperse_scalar(delta, first_scalar..range.end);
    }

    fn advance_disperse_scalar(&mut self, delta: f32, range: Range<usize>) {
        for index in range {
            let noise = next_random(&mut self.random_states[index]);
            let jitter_x = signed_unit(noise);
            let jitter_y = signed_unit(noise.rotate_left(11));
            let (target_x, target_y, _) = self.target_components(index);
            let x = self.x[index];
            let y = self.y[index];
            let z = self.depth(index);
            let vx = (self.vx[index] + ((x - target_x) * 2.2 + jitter_x * 115.0) * delta) * 0.99;
            let vy = (self.vy[index] + ((y - target_y) * 2.2 + jitter_y * 115.0) * delta) * 0.99;
            let vz = (self.vz[index] + signed_unit(noise.rotate_left(21)) * 55.0 * delta) * 0.99;
            self.vx[index] = vx;
            self.vy[index] = vy;
            self.vz[index] = vz;
            self.x[index] = x + vx * delta;
            self.y[index] = y + vy * delta;
            self.z_q7[index] = pack_depth((z + vz * delta).clamp(-DEPTH_EXTENT, DEPTH_EXTENT));
        }
    }

    #[inline(always)]
    fn target_components(&self, index: usize) -> (f32, f32, f32) {
        let target = unpack_target(self.packed_targets[index]);
        (
            target.x,
            target.y,
            f32::from(self.target_depth_q2[index]) * TARGET_DEPTH_Q2_RECIP,
        )
    }

    #[inline(always)]
    fn depth(&self, index: usize) -> f32 {
        f32::from(self.z_q7[index]) * DEPTH_FIXED_SCALE_RECIP
    }

    #[inline(always)]
    fn projection_scale(&self, denominator: f32) -> (f32, f32) {
        if !self.use_table_projection
            || !(RECIPROCAL_TABLE_MIN..RECIPROCAL_TABLE_MAX).contains(&denominator)
        {
            return (FOCAL_LENGTH / denominator, 0.0);
        }
        let table_position = (denominator - RECIPROCAL_TABLE_MIN) * RECIPROCAL_TABLE_STEP_RECIP;
        let index = table_position as usize;
        let fraction = table_position - index as f32;
        let lower_reciprocal = self.reciprocal_table[index];
        let reciprocal =
            lower_reciprocal + (self.reciprocal_table[index + 1] - lower_reciprocal) * fraction;
        let lower_denominator = RECIPROCAL_TABLE_MIN + index as f32 * RECIPROCAL_TABLE_STEP;
        let reciprocal_error_bound = RECIPROCAL_TABLE_STEP * RECIPROCAL_TABLE_STEP
            / (4.0 * lower_denominator * lower_denominator * lower_denominator);
        (
            FOCAL_LENGTH * reciprocal,
            FOCAL_LENGTH * reciprocal_error_bound,
        )
    }

    #[cfg(test)]
    #[inline(always)]
    fn target_depth(&self, index: usize) -> f32 {
        f32::from(self.target_depth_q2[index]) * TARGET_DEPTH_Q2_RECIP
    }
}

#[cfg(target_arch = "arm")]
fn particle_neon_enabled() -> bool {
    std::env::var("MISTER_PARTICLE_SIMD")
        .ok()
        .is_none_or(|value| !value.trim().eq_ignore_ascii_case("scalar"))
}

#[cfg(target_arch = "arm")]
fn particle_neon_projection_enabled() -> bool {
    std::env::var("MISTER_PARTICLE_PROJECTION")
        .ok()
        .is_none_or(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "scalar" | "exact" | "table"
            )
        })
}

#[cfg(target_arch = "arm")]
fn particle_table_projection_enabled() -> bool {
    std::env::var("MISTER_PARTICLE_PROJECTION")
        .ok()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("table"))
}

#[cfg(target_arch = "arm")]
fn particle_neon_projection_validation_enabled() -> bool {
    std::env::var("MISTER_PARTICLE_PROJECTION_VALIDATE")
        .ok()
        .is_some_and(|value| !matches!(value.as_str(), "0" | "off" | "false" | "no"))
}

#[cfg(target_arch = "arm")]
fn particle_alternating_cohorts_enabled() -> bool {
    std::env::var("MISTER_PARTICLE_COHORTS")
        .ok()
        .is_none_or(|value| !value.trim().eq_ignore_ascii_case("full"))
}

#[cfg(not(target_arch = "arm"))]
const fn particle_neon_enabled() -> bool {
    false
}

#[cfg(not(target_arch = "arm"))]
const fn particle_neon_projection_enabled() -> bool {
    false
}

#[cfg(not(target_arch = "arm"))]
const fn particle_table_projection_enabled() -> bool {
    false
}

#[cfg(not(target_arch = "arm"))]
const fn particle_neon_projection_validation_enabled() -> bool {
    false
}

#[cfg(not(target_arch = "arm"))]
const fn particle_alternating_cohorts_enabled() -> bool {
    false
}

#[cfg(target_arch = "arm")]
mod neon {
    use super::ParticleEngine;
    use std::ops::Range;

    unsafe extern "C" {
        fn mister_magik_particle_neon_static(
            count: usize,
            width: f32,
            height: f32,
            delta: f32,
            random_states: *mut u32,
            x: *mut f32,
            y: *mut f32,
            z_q7: *mut i16,
            vx: *mut f32,
            vy: *mut f32,
            vz: *mut f32,
        ) -> usize;
        fn mister_magik_particle_neon_attract(
            count: usize,
            delta: f32,
            stiffness: f32,
            jitter: f32,
            damping: f32,
            packed_targets: *const u32,
            target_depth_q2: *const i8,
            random_states: *mut u32,
            x: *mut f32,
            y: *mut f32,
            z_q7: *mut i16,
            vx: *mut f32,
            vy: *mut f32,
            vz: *mut f32,
        ) -> usize;
        fn mister_magik_particle_neon_disperse(
            count: usize,
            delta: f32,
            packed_targets: *const u32,
            random_states: *mut u32,
            x: *mut f32,
            y: *mut f32,
            z_q7: *mut i16,
            vx: *mut f32,
            vy: *mut f32,
            vz: *mut f32,
        ) -> usize;
        fn mister_magik_particle_neon_project_commands(
            count: usize,
            width: usize,
            visual: u32,
            phase: u32,
            projection_center_x: f32,
            projection_center_y: f32,
            projection_max_x: f32,
            projection_max_y: f32,
            rotation_y_sin: f32,
            rotation_y_cos: f32,
            x: *const f32,
            y: *const f32,
            z_q7: *const i16,
            random_states: *const u32,
            commands: *mut u32,
        ) -> usize;
    }

    pub(super) unsafe fn advance_static(
        engine: &mut ParticleEngine,
        delta: f32,
        range: Range<usize>,
    ) -> usize {
        let processed = unsafe {
            mister_magik_particle_neon_static(
                range.len(),
                engine.config.width as f32,
                engine.config.height as f32,
                delta,
                engine.random_states.as_mut_ptr().add(range.start),
                engine.x.as_mut_ptr().add(range.start),
                engine.y.as_mut_ptr().add(range.start),
                engine.z_q7.as_mut_ptr().add(range.start),
                engine.vx.as_mut_ptr().add(range.start),
                engine.vy.as_mut_ptr().add(range.start),
                engine.vz.as_mut_ptr().add(range.start),
            )
        };
        range.start + processed
    }

    pub(super) unsafe fn advance_attract(
        engine: &mut ParticleEngine,
        delta: f32,
        stiffness: f32,
        jitter: f32,
        damping: f32,
        range: Range<usize>,
    ) -> usize {
        let processed = unsafe {
            mister_magik_particle_neon_attract(
                range.len(),
                delta,
                stiffness,
                jitter,
                damping,
                engine.packed_targets.as_ptr().add(range.start),
                engine.target_depth_q2.as_ptr().add(range.start),
                engine.random_states.as_mut_ptr().add(range.start),
                engine.x.as_mut_ptr().add(range.start),
                engine.y.as_mut_ptr().add(range.start),
                engine.z_q7.as_mut_ptr().add(range.start),
                engine.vx.as_mut_ptr().add(range.start),
                engine.vy.as_mut_ptr().add(range.start),
                engine.vz.as_mut_ptr().add(range.start),
            )
        };
        range.start + processed
    }

    pub(super) unsafe fn advance_disperse(
        engine: &mut ParticleEngine,
        delta: f32,
        range: Range<usize>,
    ) -> usize {
        let processed = unsafe {
            mister_magik_particle_neon_disperse(
                range.len(),
                delta,
                engine.packed_targets.as_ptr().add(range.start),
                engine.random_states.as_mut_ptr().add(range.start),
                engine.x.as_mut_ptr().add(range.start),
                engine.y.as_mut_ptr().add(range.start),
                engine.z_q7.as_mut_ptr().add(range.start),
                engine.vx.as_mut_ptr().add(range.start),
                engine.vy.as_mut_ptr().add(range.start),
                engine.vz.as_mut_ptr().add(range.start),
            )
        };
        range.start + processed
    }

    pub(super) unsafe fn project_commands(
        engine: &ParticleEngine,
        commands: *mut u32,
        visual: bool,
    ) -> usize {
        let phase = match engine.phase {
            super::ParticlePhase::Static => 0,
            super::ParticlePhase::Form => 1,
            super::ParticlePhase::Hold => 2,
            super::ParticlePhase::Disperse => 3,
        };
        unsafe {
            mister_magik_particle_neon_project_commands(
                engine.particle_count(),
                engine.config.width,
                u32::from(visual),
                phase,
                engine.projection_center_x,
                engine.projection_center_y,
                engine.projection_max_x,
                engine.projection_max_y,
                engine.rotation_y_sin,
                engine.rotation_y_cos,
                engine.x.as_ptr(),
                engine.y.as_ptr(),
                engine.z_q7.as_ptr(),
                engine.random_states.as_ptr(),
                commands,
            )
        }
    }
}

fn pack_target(target: ParticleTarget) -> Result<u32, String> {
    let x = pack_target_coordinate(target.x)?;
    let y = pack_target_coordinate(target.y)?;
    Ok(u32::from(x as u16) | (u32::from(y as u16) << 16))
}

fn pack_target_coordinate(value: f32) -> Result<i16, String> {
    let fixed = (value * TARGET_FIXED_SCALE).round();
    if !fixed.is_finite() || fixed < f32::from(i16::MIN) || fixed > f32::from(i16::MAX) {
        return Err(format!(
            "particle target coordinate {value} exceeds Q12.4 range"
        ));
    }
    Ok(fixed as i16)
}

#[inline(always)]
fn unpack_target(packed: u32) -> ParticleTarget {
    ParticleTarget {
        x: f32::from(packed as u16 as i16) * TARGET_FIXED_SCALE_RECIP,
        y: f32::from((packed >> 16) as u16 as i16) * TARGET_FIXED_SCALE_RECIP,
    }
}

fn mix32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

const fn nonzero_random_state(value: u32) -> u32 {
    if value == 0 { 0x6d2b_79f5 } else { value }
}

fn next_random(state: &mut u32) -> u32 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    *state = value;
    value
}

fn distributed_target_depth_q2(value: u32) -> i8 {
    let level = (u64::from(value) * TARGET_DEPTH_LEVELS) >> 32;
    (level as i16 - i16::from(TARGET_DEPTH_Q2_HALF_EXTENT)) as i8
}

#[inline(always)]
fn pack_depth(value: f32) -> i16 {
    debug_assert!(value.is_finite());
    debug_assert!((-DEPTH_EXTENT..=DEPTH_EXTENT).contains(&value));
    (value * DEPTH_FIXED_SCALE).round() as i16
}

fn rotation_y_at_cycle_us(cycle_us: u64) -> f32 {
    if !(FORM_END_US..HOLD_END_US).contains(&cycle_us) {
        return 0.0;
    }
    let progress = (cycle_us - FORM_END_US) as f32 / HOLD_DURATION_US as f32;
    let eased = progress * progress * (3.0 - 2.0 * progress);
    std::f32::consts::TAU * eased
}

#[inline(always)]
fn rotate_xz(x: f32, z: f32, sin: f32, cos: f32) -> (f32, f32) {
    (x * cos + z * sin, -x * sin + z * cos)
}

#[inline(always)]
fn projection_coordinate_needs_exact(value: f32, maximum: f32, error: f32) -> bool {
    if error >= 0.5 {
        return true;
    }
    if (value + 0.5).abs() <= error || (value - maximum).abs() <= error {
        return true;
    }
    let shifted = value + 0.5;
    if shifted <= 0.0 || value >= maximum {
        return false;
    }
    let fraction = shifted - shifted as i32 as f32;
    fraction <= error || fraction >= 1.0 - error
}

#[inline(always)]
fn wrap_coordinate(value: f32, extent: f32) -> f32 {
    if value < 0.0 {
        let wrapped = value + extent;
        if wrapped >= 0.0 {
            return wrapped;
        }
    } else if value >= extent {
        let wrapped = value - extent;
        if wrapped < extent {
            return wrapped;
        }
    } else {
        return value;
    }
    value.rem_euclid(extent)
}

fn unit_float(value: u32) -> f32 {
    (value >> 8) as f32 / 16_777_215.0
}

fn signed_unit(value: u32) -> f32 {
    unit_float(value) * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask() -> TargetMask {
        TargetMask::from_alpha(
            4,
            3,
            4,
            &[0, 255, 0, 0, 255, 255, 0, 0, 0, 255, 0, 0],
            128,
            1,
        )
        .unwrap()
    }

    fn engine(count: usize) -> ParticleEngine {
        ParticleEngine::new(
            ParticleConfig {
                count,
                width: 32,
                height: 24,
                seed: 42,
                preset: ParticlePreset::Capacity,
            },
            mask(),
        )
        .unwrap()
    }

    #[test]
    fn alpha_mask_sampling_is_deterministic() {
        let sampled = TargetMask::from_alpha(4, 4, 4, &[255; 16], 128, 2).unwrap();
        assert_eq!(
            sampled.points(),
            &[
                ParticleTarget { x: 0.0, y: 0.0 },
                ParticleTarget { x: 2.0, y: 0.0 },
                ParticleTarget { x: 0.0, y: 2.0 },
                ParticleTarget { x: 2.0, y: 2.0 },
            ]
        );
    }

    #[test]
    fn phase_boundaries_follow_the_ten_second_cycle() {
        assert_eq!(ParticlePhase::at_cycle_us(0), ParticlePhase::Static);
        assert_eq!(ParticlePhase::at_cycle_us(2_999_999), ParticlePhase::Static);
        assert_eq!(ParticlePhase::at_cycle_us(3_000_000), ParticlePhase::Form);
        assert_eq!(ParticlePhase::at_cycle_us(5_000_000), ParticlePhase::Hold);
        assert_eq!(
            ParticlePhase::at_cycle_us(8_000_000),
            ParticlePhase::Disperse
        );
        assert_eq!(
            ParticlePhase::at_cycle_us(9_999_999),
            ParticlePhase::Disperse
        );
    }

    #[test]
    fn same_seed_and_time_produce_identical_projection() {
        let mut first = engine(32);
        let mut second = engine(32);
        first.step(Duration::from_millis(3_500));
        second.step(Duration::from_millis(3_500));
        let first = (0..32)
            .map(|index| first.project(index))
            .collect::<Vec<_>>();
        let second = (0..32)
            .map(|index| second.project(index))
            .collect::<Vec<_>>();
        assert_eq!(first, second);
    }

    #[test]
    fn alternating_cohorts_advance_each_particle_half_on_opposite_frames() {
        let mut engine = engine(8);
        engine.use_alternating_cohorts = true;
        let initial = engine.random_states.clone();
        engine.step(Duration::from_micros(16_667));
        assert_ne!(&engine.random_states[..4], &initial[..4]);
        assert_eq!(&engine.random_states[4..], &initial[4..]);
        let after_first = engine.random_states.clone();
        engine.step(Duration::from_micros(33_334));
        assert_eq!(&engine.random_states[..4], &after_first[..4]);
        assert_ne!(&engine.random_states[4..], &after_first[4..]);
    }

    #[test]
    fn y_rotation_matches_each_quarter_turn() {
        let epsilon = 1.0e-6;
        for ((sin, cos), expected) in [
            ((0.0, 1.0), (10.0, 2.0)),
            ((1.0, 0.0), (2.0, -10.0)),
            ((0.0, -1.0), (-10.0, -2.0)),
            ((-1.0, 0.0), (-2.0, 10.0)),
            ((0.0, 1.0), (10.0, 2.0)),
        ] {
            let actual = rotate_xz(10.0, 2.0, sin, cos);
            assert!((actual.0 - expected.0).abs() < epsilon);
            assert!((actual.1 - expected.1).abs() < epsilon);
        }
    }

    #[test]
    fn hold_rotation_eases_through_one_complete_turn() {
        assert_eq!(rotation_y_at_cycle_us(FORM_END_US), 0.0);
        assert!(
            (rotation_y_at_cycle_us(FORM_END_US + HOLD_DURATION_US / 2) - std::f32::consts::PI)
                .abs()
                < 1.0e-6
        );
        let final_hold_angle = rotation_y_at_cycle_us(HOLD_END_US - 1);
        assert!((final_hold_angle - std::f32::consts::TAU).abs() < 1.0e-5);
        assert_eq!(rotation_y_at_cycle_us(HOLD_END_US), 0.0);
    }

    #[test]
    fn projection_centre_is_invariant_during_rotation() {
        let mut engine = engine(1);
        engine.x[0] = engine.projection_center_x;
        engine.y[0] = engine.projection_center_y;
        engine.z_q7[0] = pack_depth(0.0);
        for angle in [
            0.0,
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
            3.0 * std::f32::consts::FRAC_PI_2,
            std::f32::consts::TAU,
        ] {
            (engine.rotation_y_sin, engine.rotation_y_cos) = angle.sin_cos();
            let projected = engine.project(0).unwrap();
            assert_eq!(projected.x, 16);
            assert_eq!(projected.y, 12);
        }
    }

    #[test]
    fn perspective_separates_near_and_far_particles() {
        let mut engine = engine(2);
        engine.x.fill(engine.projection_center_x + 8.0);
        engine.y.fill(engine.projection_center_y);
        engine.z_q7[0] = pack_depth(-DEPTH_EXTENT);
        engine.z_q7[1] = pack_depth(DEPTH_EXTENT);
        let near = engine.project(0).unwrap();
        let far = engine.project(1).unwrap();
        assert!(near.x > far.x);
        assert!(near.depth < far.depth);
    }

    #[test]
    fn projected_particles_remain_inside_the_viewport() {
        let mut engine = engine(128);
        for milliseconds in [0, 2_000, 4_000, 6_000, 9_000] {
            engine.step(Duration::from_millis(milliseconds));
            for index in 0..engine.particle_count() {
                if let Some(projected) = engine.project(index) {
                    assert!((0..32).contains(&projected.x));
                    assert!((0..24).contains(&projected.y));
                    assert!(projected.depth.is_finite());
                }
            }
        }
    }

    #[test]
    fn packed_projection_offsets_match_the_exact_scalar_projection() {
        let mut engine = engine(131);
        #[cfg(target_arch = "arm")]
        {
            engine.use_neon_projection = true;
        }
        for milliseconds in [0, 2_000, 4_000, 5_000, 6_000, 7_000, 9_000] {
            engine.step(Duration::from_millis(milliseconds));
            let mut offsets = vec![MaybeUninit::uninit(); engine.particle_count()];
            let visible = engine.project_offsets(&mut offsets);
            let mut expected_visible = 0usize;
            for (index, offset) in offsets.into_iter().enumerate() {
                let expected =
                    engine
                        .project(index)
                        .map_or(PARTICLE_NOT_VISIBLE_OFFSET, |particle| {
                            expected_visible += 1;
                            (particle.y as usize * engine.config.width + particle.x as usize) as u32
                        });
                // SAFETY: `project_offsets` initializes every supplied entry.
                assert_eq!(unsafe { offset.assume_init() }, expected);
            }
            assert_eq!(visible, expected_visible);
        }
    }

    #[test]
    fn corrected_reciprocal_table_preserves_projected_pixels() {
        let mut engine = engine(1_024);
        for milliseconds in [0, 2_000, 4_000, 5_000, 6_000, 7_000, 9_000] {
            engine.step(Duration::from_millis(milliseconds));
            engine.use_table_projection = false;
            let exact = (0..engine.particle_count())
                .map(|index| engine.project(index))
                .collect::<Vec<_>>();
            engine.use_table_projection = true;
            let corrected = (0..engine.particle_count())
                .map(|index| engine.project(index))
                .collect::<Vec<_>>();
            assert_eq!(corrected, exact, "projection differed at {milliseconds} ms");
        }
    }

    #[test]
    fn stepping_reuses_particle_storage() {
        let mut engine = engine(64);
        let capacities = (
            engine.x.capacity(),
            engine.y.capacity(),
            engine.z_q7.capacity(),
            engine.vx.capacity(),
            engine.vy.capacity(),
            engine.vz.capacity(),
            engine.packed_targets.capacity(),
            engine.target_depth_q2.capacity(),
            engine.random_states.capacity(),
        );
        engine.step(Duration::from_secs(6));
        assert_eq!(
            capacities,
            (
                engine.x.capacity(),
                engine.y.capacity(),
                engine.z_q7.capacity(),
                engine.vx.capacity(),
                engine.vy.capacity(),
                engine.vz.capacity(),
                engine.packed_targets.capacity(),
                engine.target_depth_q2.capacity(),
                engine.random_states.capacity(),
            )
        );
    }

    #[test]
    fn formation_depths_are_deterministic_balanced_and_bounded() {
        let first = engine(16_384);
        let second = engine(16_384);
        assert_eq!(first.target_depth_q2, second.target_depth_q2);
        assert_eq!(ParticleEngine::bytes_per_particle(), 31);
        let mut levels = [0usize; TARGET_DEPTH_LEVELS as usize];
        for &depth in &first.target_depth_q2 {
            assert!((-TARGET_DEPTH_Q2_HALF_EXTENT..=TARGET_DEPTH_Q2_HALF_EXTENT).contains(&depth));
            levels[usize::from(
                (i16::from(depth) + i16::from(TARGET_DEPTH_Q2_HALF_EXTENT)) as u16,
            )] += 1;
        }
        let minimum = levels.into_iter().min().unwrap();
        let maximum = levels.into_iter().max().unwrap();
        assert!(minimum > 0);
        assert!(maximum - minimum < 100);
    }

    #[test]
    fn particles_converge_on_their_formation_depths() {
        let mut engine = engine(512);
        for frame in 1..=360 {
            engine.step(Duration::from_micros(frame * 16_667));
        }
        let mean_error = (0..engine.particle_count())
            .map(|index| (engine.depth(index) - engine.target_depth(index)).abs())
            .sum::<f32>()
            / engine.particle_count() as f32;
        assert!(mean_error < 0.5, "mean depth error was {mean_error}");
    }

    #[test]
    fn packed_targets_preserve_q12_4_coordinates() {
        for target in [
            ParticleTarget { x: -0.4, y: 0.0 },
            ParticleTarget {
                x: 479.53125,
                y: 269.96875,
            },
            ParticleTarget { x: 959.4, y: 539.4 },
        ] {
            let unpacked = unpack_target(pack_target(target).unwrap());
            assert!((unpacked.x - target.x).abs() <= 1.0 / 32.0);
            assert!((unpacked.y - target.y).abs() <= 1.0 / 32.0);
        }
    }

    #[test]
    fn packed_targets_reject_coordinates_outside_q12_4() {
        assert!(pack_target(ParticleTarget { x: 2_048.0, y: 0.0 }).is_err());
    }

    #[test]
    fn packed_depth_preserves_q8_7_precision_across_the_simulation_range() {
        for depth in [-DEPTH_EXTENT, -10.123, 0.0, 10.123, DEPTH_EXTENT] {
            let unpacked = f32::from(pack_depth(depth)) * DEPTH_FIXED_SCALE_RECIP;
            assert!((unpacked - depth).abs() <= 1.0 / (DEPTH_FIXED_SCALE * 2.0));
        }
    }

    #[test]
    fn checked_projection_matches_rounding_at_viewport_edges() {
        let mut engine = engine(1);
        engine.z_q7[0] = pack_depth(0.0);
        for (x, expected) in [
            (-0.5, None),
            (-0.499, Some(0)),
            (0.499, Some(0)),
            (0.5, Some(1)),
            (31.499, Some(31)),
            (31.5, None),
        ] {
            engine.x[0] = x;
            engine.y[0] = 12.0;
            assert_eq!(engine.project(0).map(|particle| particle.x), expected);
        }
    }

    #[test]
    fn particle_randomness_is_nonzero_and_deterministic() {
        let mut first = nonzero_random_state(0);
        let mut second = nonzero_random_state(0);
        let sequence = (0..16).map(|_| next_random(&mut first)).collect::<Vec<_>>();
        assert!(sequence.iter().all(|value| *value != 0));
        assert_eq!(
            sequence,
            (0..16)
                .map(|_| next_random(&mut second))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn viewport_wrapping_fast_path_and_fallback_are_exact() {
        for (value, expected) in [
            (12.5, 12.5),
            (-0.5, 31.5),
            (32.5, 0.5),
            (-64.5, 31.5),
            (96.5, 0.5),
        ] {
            assert_eq!(wrap_coordinate(value, 32.0), expected);
        }
    }

    #[test]
    fn cycle_transition_does_not_reinitialize_scatter() {
        let mut transitioning = engine(64);
        let mut already_advanced = engine(64);
        transitioning.step(Duration::from_millis(9_999));
        already_advanced.step(Duration::from_millis(9_999));
        already_advanced.cycle = 1;
        transitioning.step(Duration::from_millis(10_000));
        already_advanced.step(Duration::from_millis(10_000));
        assert_eq!(transitioning.x, already_advanced.x);
        assert_eq!(transitioning.y, already_advanced.y);
        assert_eq!(transitioning.z_q7, already_advanced.z_q7);
    }

    #[cfg(not(target_arch = "arm"))]
    #[test]
    fn non_arm_builds_keep_the_scalar_reference_backend() {
        let engine = engine(7);
        assert_eq!(engine.simulation_backend_label(), "scalar");
        assert_eq!(engine.projection_backend_label(), "scalar-exact");
    }

    #[cfg(target_arch = "arm")]
    #[test]
    fn neon_updates_match_scalar_updates_including_the_tail() {
        for phase in [
            ParticlePhase::Static,
            ParticlePhase::Form,
            ParticlePhase::Hold,
            ParticlePhase::Disperse,
        ] {
            let mut scalar = engine(67);
            scalar.phase = phase;
            scalar.use_neon = false;
            let mut neon = engine(67);
            neon.phase = phase;
            neon.use_neon = true;
            scalar.advance(1.0 / 60.0);
            neon.advance(1.0 / 60.0);
            assert_eq!(scalar.random_states, neon.random_states);
            for (scalar_values, neon_values) in [
                (&scalar.x, &neon.x),
                (&scalar.y, &neon.y),
                (&scalar.z, &neon.z),
                (&scalar.vx, &neon.vx),
                (&scalar.vy, &neon.vy),
                (&scalar.vz, &neon.vz),
            ] {
                for (&scalar_value, &neon_value) in scalar_values.iter().zip(neon_values) {
                    assert!(
                        (scalar_value - neon_value).abs() <= 2.0e-5,
                        "{phase:?}: scalar {scalar_value} != NEON {neon_value}"
                    );
                }
            }
        }
    }
}
