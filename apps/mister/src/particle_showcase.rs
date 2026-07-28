// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared data model for the interactive ARM particle showcase.

use std::time::Duration;

pub const PARTICLE_DEMO_DURATION: Duration = Duration::from_secs(30);
pub const PARTICLE_DEMO_MAX_COUNT: usize = 98_304;
pub const PARTICLE_DEMO_TRANSITION_COUNT: usize = 4_096;
pub const PARTICLE_DEMO_TRANSITION_DURATION: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ParticleDemoKind {
    Fireworks,
    FireEmbers,
    SpiralGalaxy,
    WarpSpeed,
    MeteorShower,
    Weather,
    ParticlePortal,
    ElectricStorm,
    FountainWaterfall,
    ArcadeCabinet,
}

impl ParticleDemoKind {
    pub const ALL: [Self; 10] = [
        Self::Fireworks,
        Self::FireEmbers,
        Self::SpiralGalaxy,
        Self::WarpSpeed,
        Self::MeteorShower,
        Self::Weather,
        Self::ParticlePortal,
        Self::ElectricStorm,
        Self::FountainWaterfall,
        Self::ArcadeCabinet,
    ];

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn number(self) -> usize {
        self.index() + 1
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fireworks => "FIREWORKS",
            Self::FireEmbers => "FIRE + EMBERS",
            Self::SpiralGalaxy => "SPIRAL GALAXY",
            Self::WarpSpeed => "WARP SPEED",
            Self::MeteorShower => "METEOR SHOWER",
            Self::Weather => "WEATHER",
            Self::ParticlePortal => "PARTICLE PORTAL",
            Self::ElectricStorm => "ELECTRIC STORM",
            Self::FountainWaterfall => "FOUNTAIN / WATERFALL",
            Self::ArcadeCabinet => "ARCADE CABINET",
        }
    }

    #[must_use]
    pub const fn telemetry_label(self) -> &'static str {
        match self {
            Self::Fireworks => "fireworks",
            Self::FireEmbers => "fire-embers",
            Self::SpiralGalaxy => "spiral-galaxy",
            Self::WarpSpeed => "warp-speed",
            Self::MeteorShower => "meteor-shower",
            Self::Weather => "weather",
            Self::ParticlePortal => "particle-portal",
            Self::ElectricStorm => "electric-storm",
            Self::FountainWaterfall => "fountain-waterfall",
            Self::ArcadeCabinet => "arcade-cabinet",
        }
    }

    #[must_use]
    pub const fn starting_count(self) -> usize {
        match self {
            Self::Fireworks => 24_576,
            Self::FireEmbers | Self::MeteorShower => 20_480,
            Self::SpiralGalaxy => 98_304,
            Self::WarpSpeed | Self::Weather => 49_152,
            Self::ParticlePortal | Self::ArcadeCabinet => 65_536,
            Self::ElectricStorm => 16_384,
            Self::FountainWaterfall => 32_768,
        }
    }

    #[must_use]
    pub const fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }

    #[must_use]
    pub fn offset_wrapped(self, delta: i32) -> Self {
        let count = Self::ALL.len() as i32;
        let index = (self.index() as i32 + delta).rem_euclid(count) as usize;
        Self::ALL[index]
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        if let Ok(number) = normalized.parse::<usize>() {
            return (1..=Self::ALL.len())
                .contains(&number)
                .then(|| Self::ALL[number - 1]);
        }
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.telemetry_label() == normalized)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParticleShowcaseConfig {
    pub width: usize,
    pub height: usize,
    pub seed: u64,
    pub initial_demo: ParticleDemoKind,
}

impl ParticleShowcaseConfig {
    pub fn validate(self) -> Result<Self, String> {
        if (self.width, self.height) != (960, 540) {
            return Err(format!(
                "particle showcase requires 960x540, received {}x{}",
                self.width, self.height
            ));
        }
        Ok(self)
    }
}

#[derive(Debug)]
pub(crate) struct ParticleShowcasePool {
    active: usize,
    pub(crate) x: Vec<f32>,
    pub(crate) y: Vec<f32>,
    pub(crate) z: Vec<f32>,
    pub(crate) previous_x: Vec<f32>,
    pub(crate) previous_y: Vec<f32>,
    pub(crate) previous_z: Vec<f32>,
    pub(crate) vx: Vec<f32>,
    pub(crate) vy: Vec<f32>,
    pub(crate) vz: Vec<f32>,
    pub(crate) age: Vec<f32>,
    pub(crate) life: Vec<f32>,
    pub(crate) random: Vec<u32>,
    pub(crate) style: Vec<u8>,
    pub(crate) flags: Vec<u8>,
}

impl ParticleShowcasePool {
    pub(crate) fn new() -> Self {
        Self {
            active: 0,
            x: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            y: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            z: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            previous_x: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            previous_y: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            previous_z: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            vx: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            vy: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            vz: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            age: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            life: vec![0.0; PARTICLE_DEMO_MAX_COUNT],
            random: vec![0; PARTICLE_DEMO_MAX_COUNT],
            style: vec![0; PARTICLE_DEMO_MAX_COUNT],
            flags: vec![0; PARTICLE_DEMO_MAX_COUNT],
        }
    }

    pub(crate) fn reset(&mut self, kind: ParticleDemoKind, seed: u64) {
        self.active = kind.starting_count();
        let mut state = fold_seed(seed, kind);
        for index in 0..self.active {
            state = xorshift32(state);
            self.random[index] = state;
            self.x[index] = 0.0;
            self.y[index] = 0.0;
            self.z[index] = 0.0;
            self.previous_x[index] = 0.0;
            self.previous_y[index] = 0.0;
            self.previous_z[index] = 0.0;
            self.vx[index] = 0.0;
            self.vy[index] = 0.0;
            self.vz[index] = 0.0;
            self.age[index] = 0.0;
            self.life[index] = 1.0;
            self.style[index] = 0;
            self.flags[index] = 0;
        }
    }

    #[must_use]
    pub(crate) const fn active(&self) -> usize {
        self.active
    }

    #[must_use]
    pub(crate) fn allocated_bytes(&self) -> usize {
        let f32_capacity = self.x.capacity()
            + self.y.capacity()
            + self.z.capacity()
            + self.previous_x.capacity()
            + self.previous_y.capacity()
            + self.previous_z.capacity()
            + self.vx.capacity()
            + self.vy.capacity()
            + self.vz.capacity()
            + self.age.capacity()
            + self.life.capacity();
        f32_capacity
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_add(
                self.random
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
            .saturating_add(self.style.capacity())
            .saturating_add(self.flags.capacity())
    }
}

fn fold_seed(seed: u64, kind: ParticleDemoKind) -> u32 {
    let folded = seed ^ (seed >> 32) ^ ((kind.number() as u64) * 0x9e37_79b9);
    let folded = folded as u32;
    if folded == 0 { 0xa341_316c } else { folded }
}

pub(crate) fn xorshift32(mut state: u32) -> u32 {
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_order_and_wrapping_are_stable() {
        assert_eq!(ParticleDemoKind::ALL.len(), 10);
        assert_eq!(
            ParticleDemoKind::Fireworks.offset_wrapped(-1),
            ParticleDemoKind::ArcadeCabinet
        );
        assert_eq!(
            ParticleDemoKind::ArcadeCabinet.offset_wrapped(1),
            ParticleDemoKind::Fireworks
        );
        for (index, kind) in ParticleDemoKind::ALL.into_iter().enumerate() {
            assert_eq!(kind.index(), index);
            assert_eq!(kind.number(), index + 1);
            assert!(kind.starting_count() <= PARTICLE_DEMO_MAX_COUNT);
        }
    }

    #[test]
    fn demo_parser_accepts_numbers_and_stable_labels() {
        assert_eq!(
            ParticleDemoKind::parse("1"),
            Some(ParticleDemoKind::Fireworks)
        );
        assert_eq!(
            ParticleDemoKind::parse("particle-portal"),
            Some(ParticleDemoKind::ParticlePortal)
        );
        assert_eq!(ParticleDemoKind::parse("11"), None);
        assert_eq!(ParticleDemoKind::parse("unknown"), None);
    }

    #[test]
    fn reset_is_deterministic_and_clears_reused_state() {
        let mut first = ParticleShowcasePool::new();
        let mut second = ParticleShowcasePool::new();
        first.vx.fill(17.0);
        first.reset(ParticleDemoKind::Weather, 0x1234);
        second.reset(ParticleDemoKind::Weather, 0x1234);

        assert_eq!(first.active(), ParticleDemoKind::Weather.starting_count());
        assert_eq!(
            &first.random[..first.active()],
            &second.random[..second.active()]
        );
        assert!(first.vx[..first.active()].iter().all(|value| *value == 0.0));
        assert!(first.allocated_bytes() > 0);
    }

    #[test]
    fn showcase_geometry_is_fixed_to_the_direct_renderer() {
        assert!(
            ParticleShowcaseConfig {
                width: 960,
                height: 540,
                seed: 1,
                initial_demo: ParticleDemoKind::Fireworks,
            }
            .validate()
            .is_ok()
        );
        assert!(
            ParticleShowcaseConfig {
                width: 1280,
                height: 720,
                seed: 1,
                initial_demo: ParticleDemoKind::Fireworks,
            }
            .validate()
            .is_err()
        );
    }
}
