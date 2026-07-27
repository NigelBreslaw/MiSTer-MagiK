// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic scalar particle simulation for software-rendered effects.

use std::time::Duration;

pub const PARTICLE_COUNT_MAX: usize = 524_288;
pub const PARTICLE_CYCLE: Duration = Duration::from_secs(10);
const STATIC_END_US: u64 = 3_000_000;
const FORM_END_US: u64 = 5_000_000;
const HOLD_END_US: u64 = 8_000_000;
const CYCLE_US: u64 = 10_000_000;
const MAX_STEP_SECONDS: f32 = 1.0 / 15.0;
const DEPTH_EXTENT: f32 = 64.0;
const FOCAL_LENGTH: f32 = 720.0;
const TARGET_FIXED_SCALE: f32 = 16.0;
const TARGET_FIXED_SCALE_RECIP: f32 = 1.0 / TARGET_FIXED_SCALE;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParticleFrameStats {
    pub count: usize,
    pub phase: ParticlePhase,
    pub cycle: u64,
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
    packed_targets: Vec<u32>,
    x: Vec<f32>,
    y: Vec<f32>,
    z: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    vz: Vec<f32>,
    seeds: Vec<u32>,
    last_elapsed: Duration,
    cycle: u64,
    phase: ParticlePhase,
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
            packed_targets: Vec::with_capacity(config.count),
            x: Vec::with_capacity(config.count),
            y: Vec::with_capacity(config.count),
            z: Vec::with_capacity(config.count),
            vx: Vec::with_capacity(config.count),
            vy: Vec::with_capacity(config.count),
            vz: Vec::with_capacity(config.count),
            seeds: Vec::with_capacity(config.count),
            last_elapsed: Duration::ZERO,
            cycle: 0,
            phase: ParticlePhase::Static,
        };
        engine.initialize_particles(&target_points, 0)?;
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
        7 * std::mem::size_of::<u32>() + std::mem::size_of::<u32>()
    }

    pub fn step(&mut self, elapsed: Duration) -> ParticleFrameStats {
        let elapsed_us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        let next_cycle = elapsed_us / CYCLE_US;
        if next_cycle != self.cycle {
            self.cycle = next_cycle;
            self.initialize_scatter(next_cycle);
        }
        let cycle_us = elapsed_us % CYCLE_US;
        let next_phase = ParticlePhase::at_cycle_us(cycle_us);
        let delta = elapsed
            .saturating_sub(self.last_elapsed)
            .as_secs_f32()
            .min(MAX_STEP_SECONDS);
        self.last_elapsed = elapsed;
        self.phase = next_phase;
        if delta > 0.0 {
            self.advance(delta, elapsed_us);
        }
        ParticleFrameStats {
            count: self.particle_count(),
            phase: self.phase,
            cycle: self.cycle,
        }
    }

    #[must_use]
    pub fn project(&self, index: usize) -> Option<ProjectedParticle> {
        let denominator = FOCAL_LENGTH + self.z[index];
        if denominator <= 1.0 {
            return None;
        }
        let scale = FOCAL_LENGTH / denominator;
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let screen_x = center_x + (self.x[index] - center_x) * scale;
        let screen_y = center_y + (self.y[index] - center_y) * scale;
        let x = screen_x.round() as i32;
        let y = screen_y.round() as i32;
        if x < 0 || y < 0 || x >= self.config.width as i32 || y >= self.config.height as i32 {
            return None;
        }
        Some(ProjectedParticle {
            x,
            y,
            depth: self.z[index],
        })
    }

    #[must_use]
    pub fn flicker_key(&self, index: usize) -> u32 {
        mix32(self.seeds[index] ^ self.last_elapsed.as_millis() as u32)
    }

    fn initialize_particles(
        &mut self,
        target_points: &[ParticleTarget],
        cycle: u64,
    ) -> Result<(), String> {
        let point_count = target_points.len();
        for index in 0..self.config.count {
            let seed = mix32(
                self.config.seed as u32
                    ^ (self.config.seed >> 32) as u32
                    ^ index as u32
                    ^ 0x9e37_79b9,
            );
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
            self.seeds.push(seed);
            self.x.push(0.0);
            self.y.push(0.0);
            self.z.push(0.0);
            self.vx.push(0.0);
            self.vy.push(0.0);
            self.vz.push(0.0);
        }
        self.initialize_scatter(cycle);
        Ok(())
    }

    fn initialize_scatter(&mut self, cycle: u64) {
        let width = self.config.width as f32;
        let height = self.config.height as f32;
        for index in 0..self.particle_count() {
            let seed = self.seeds[index] ^ cycle as u32;
            self.x[index] = unit_float(mix32(seed ^ 0xa511_e9b3)) * width;
            self.y[index] = unit_float(mix32(seed ^ 0x63d8_3595)) * height;
            self.z[index] = signed_unit(mix32(seed ^ 0x7f4a_7c15)) * DEPTH_EXTENT;
            self.vx[index] = signed_unit(mix32(seed ^ 0x94d0_49bb)) * 42.0;
            self.vy[index] = signed_unit(mix32(seed ^ 0x2c1b_3c6d)) * 42.0;
            self.vz[index] = signed_unit(mix32(seed ^ 0x297a_2d39)) * 10.0;
        }
    }

    fn advance(&mut self, delta: f32, elapsed_us: u64) {
        let width = self.config.width as f32;
        let height = self.config.height as f32;
        for index in 0..self.particle_count() {
            let noise = mix32(self.seeds[index] ^ (elapsed_us / 16_667) as u32);
            let jitter_x = signed_unit(noise);
            let jitter_y = signed_unit(noise.rotate_left(11));
            match self.phase {
                ParticlePhase::Static => {
                    self.vx[index] += jitter_x * 75.0 * delta;
                    self.vy[index] += jitter_y * 75.0 * delta;
                    self.vz[index] += signed_unit(noise.rotate_left(21)) * 8.0 * delta;
                    self.vx[index] *= 0.985;
                    self.vy[index] *= 0.985;
                    self.vz[index] *= 0.98;
                }
                ParticlePhase::Form | ParticlePhase::Hold => {
                    let target = self.target(index);
                    let hold = self.phase == ParticlePhase::Hold;
                    let stiffness = if hold { 34.0 } else { 18.0 };
                    let damping = if hold { 0.78 } else { 0.88 };
                    let jitter = if hold { 0.35 } else { 0.08 };
                    self.vx[index] +=
                        (target.x + jitter_x * jitter - self.x[index]) * stiffness * delta;
                    self.vy[index] +=
                        (target.y + jitter_y * jitter - self.y[index]) * stiffness * delta;
                    self.vz[index] += -self.z[index] * stiffness * delta;
                    self.vx[index] *= damping;
                    self.vy[index] *= damping;
                    self.vz[index] *= damping;
                }
                ParticlePhase::Disperse => {
                    let target = self.target(index);
                    let dx = self.x[index] - target.x;
                    let dy = self.y[index] - target.y;
                    self.vx[index] += (dx * 2.2 + jitter_x * 115.0) * delta;
                    self.vy[index] += (dy * 2.2 + jitter_y * 115.0) * delta;
                    self.vz[index] += signed_unit(noise.rotate_left(21)) * 55.0 * delta;
                    self.vx[index] *= 0.99;
                    self.vy[index] *= 0.99;
                    self.vz[index] *= 0.99;
                }
            }
            self.x[index] += self.vx[index] * delta;
            self.y[index] += self.vy[index] * delta;
            self.z[index] =
                (self.z[index] + self.vz[index] * delta).clamp(-DEPTH_EXTENT, DEPTH_EXTENT);
            if self.phase == ParticlePhase::Static {
                self.x[index] = self.x[index].rem_euclid(width);
                self.y[index] = self.y[index].rem_euclid(height);
            }
        }
    }

    fn target(&self, index: usize) -> ParticleTarget {
        unpack_target(self.packed_targets[index])
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
    fn stepping_reuses_particle_storage() {
        let mut engine = engine(64);
        let capacities = (
            engine.x.capacity(),
            engine.y.capacity(),
            engine.z.capacity(),
            engine.vx.capacity(),
            engine.vy.capacity(),
            engine.vz.capacity(),
            engine.packed_targets.capacity(),
            engine.seeds.capacity(),
        );
        engine.step(Duration::from_secs(6));
        assert_eq!(
            capacities,
            (
                engine.x.capacity(),
                engine.y.capacity(),
                engine.z.capacity(),
                engine.vx.capacity(),
                engine.vy.capacity(),
                engine.vz.capacity(),
                engine.packed_targets.capacity(),
                engine.seeds.capacity(),
            )
        );
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
}
