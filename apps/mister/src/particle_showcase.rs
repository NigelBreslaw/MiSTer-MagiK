// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared data model for the interactive ARM particle showcase.

use crate::bitmap_text::{ConsoleFont, ConsoleTypeface};
use crate::framebuffer::mapped::Pixel;
use slint::platform::software_renderer::Rgb565Pixel;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

pub const PARTICLE_DEMO_DURATION: Duration = Duration::from_secs(30);
pub const PARTICLE_DEMO_MAX_COUNT: usize = 98_304;
pub const PARTICLE_DEMO_TRANSITION_COUNT: usize = 4_096;
pub const PARTICLE_DEMO_TRANSITION_DURATION: Duration = Duration::from_millis(600);
const HIDDEN_SLOT_COUNT: usize = 2;
const FULL_CLEAR_DIRTY_DIVISOR: usize = 4;
const COMMAND_OFFSET_BITS: u32 = 20;
const COMMAND_OFFSET_MASK: u32 = (1 << COMMAND_OFFSET_BITS) - 1;
const COMMAND_STYLE_SHIFT: u32 = COMMAND_OFFSET_BITS;
const COMMAND_NEIGHBOR: u32 = 1 << 23;
const HUD_FONT_PX: f32 = 8.0;
const HUD_X: isize = 8;
const HUD_BASELINE_Y: isize = 14;
const HUD_W: usize = 232;
const HUD_H: usize = 18;
const MAX_SEGMENT_PIXELS: i32 = 12;
const SEGMENT_CAPACITY: usize = 32_768;
const FIRE_HEAT_W: usize = 320;
const FIRE_HEAT_H: usize = 72;
const FIRE_HEAT_SCALE: usize = 3;
const METEOR_TRAIL_SAMPLES: usize = 64;
const METEOR_TRACK_COUNT: usize = 64;
const METEOR_PARTICLE_COUNT: usize = METEOR_TRAIL_SAMPLES * METEOR_TRACK_COUNT;
const FIREWORKS_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x2805),
    Rgb565Pixel(0x600a),
    Rgb565Pixel(0xa80f),
    Rgb565Pixel(0xf813),
    Rgb565Pixel(0xfd20),
    Rgb565Pixel(0xff40),
    Rgb565Pixel(0xffdb),
    Rgb565Pixel(0xffff),
];
const FIRE_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0000),
    Rgb565Pixel(0x3000),
    Rgb565Pixel(0x7800),
    Rgb565Pixel(0xb800),
    Rgb565Pixel(0xf940),
    Rgb565Pixel(0xfca0),
    Rgb565Pixel(0xff40),
    Rgb565Pixel(0xffff),
];
const GALAXY_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x080f),
    Rgb565Pixel(0x201f),
    Rgb565Pixel(0x42bf),
    Rgb565Pixel(0x8d7f),
    Rgb565Pixel(0xc65f),
    Rgb565Pixel(0xfdb5),
    Rgb565Pixel(0xff59),
    Rgb565Pixel(0xffff),
];
const WARP_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0806),
    Rgb565Pixel(0x100c),
    Rgb565Pixel(0x1814),
    Rgb565Pixel(0x211f),
    Rgb565Pixel(0x4adf),
    Rgb565Pixel(0x8d7f),
    Rgb565Pixel(0xdfff),
    Rgb565Pixel(0xffff),
];
const METEOR_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0808),
    Rgb565Pixel(0x1010),
    Rgb565Pixel(0x295f),
    Rgb565Pixel(0x631f),
    Rgb565Pixel(0xa514),
    Rgb565Pixel(0xfd80),
    Rgb565Pixel(0xffb5),
    Rgb565Pixel(0xffff),
];
const WEATHER_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x080a),
    Rgb565Pixel(0x18d2),
    Rgb565Pixel(0x7bef),
    Rgb565Pixel(0xe71c),
    Rgb565Pixel(0x2945),
    Rgb565Pixel(0x8208),
    Rgb565Pixel(0xfd20),
    Rgb565Pixel(0xffb5),
];
const PORTAL_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x1008),
    Rgb565Pixel(0x2812),
    Rgb565Pixel(0x501f),
    Rgb565Pixel(0x801f),
    Rgb565Pixel(0x42df),
    Rgb565Pixel(0x07ff),
    Rgb565Pixel(0xb7ff),
    Rgb565Pixel(0xffff),
];
const ELECTRIC_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0808),
    Rgb565Pixel(0x180f),
    Rgb565Pixel(0x301f),
    Rgb565Pixel(0x801f),
    Rgb565Pixel(0x42df),
    Rgb565Pixel(0x8d7f),
    Rgb565Pixel(0xdfff),
    Rgb565Pixel(0xffff),
];
const WATER_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0809),
    Rgb565Pixel(0x1012),
    Rgb565Pixel(0x19da),
    Rgb565Pixel(0x2b5f),
    Rgb565Pixel(0x65ff),
    Rgb565Pixel(0x9eff),
    Rgb565Pixel(0xd7ff),
    Rgb565Pixel(0xffff),
];
const ARCADE_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x18d3),
    Rgb565Pixel(0x31d7),
    Rgb565Pixel(0x02d3),
    Rgb565Pixel(0x05bf),
    Rgb565Pixel(0xb80c),
    Rgb565Pixel(0xfaa5),
    Rgb565Pixel(0xfec8),
    Rgb565Pixel(0xffff),
];
const ARCADE_CLOUD: &[u8] = include_bytes!("../assets/particles/arcade-cabinet.pcloud");
const PARTICLE_CLOUD_MAGIC: &[u8; 8] = b"PCLOUD1\0";
const PARTICLE_CLOUD_HEADER_BYTES: usize = 28;
const PARTICLE_CLOUD_RECORD_BYTES: usize = 8;
static PARTICLE_DEMO_NAVIGATION: AtomicI32 = AtomicI32::new(0);

#[cfg(target_arch = "arm")]
unsafe extern "C" {
    fn mister_magik_showcase_neon_project_galaxy(
        count: usize,
        width: usize,
        height: usize,
        core_count: usize,
        rotation_y_sin: f32,
        rotation_y_cos: f32,
        core_scale: f32,
        x: *const f32,
        y: *const f32,
        styles: *const u8,
        flags: *const u8,
        commands: *mut u32,
    ) -> usize;
    fn mister_magik_showcase_neon_project_warp(
        count: usize,
        width: usize,
        height: usize,
        travel: f32,
        previous_step: f32,
        x: *const f32,
        y: *const f32,
        depth_phase: *const f32,
        styles: *const u8,
        commands: *mut u32,
        previous_commands: *mut u32,
    ) -> usize;
}

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
    pub const fn hud_label(self) -> &'static str {
        match self {
            Self::Fireworks => "01/10 FIREWORKS",
            Self::FireEmbers => "02/10 FIRE + EMBERS",
            Self::SpiralGalaxy => "03/10 SPIRAL GALAXY",
            Self::WarpSpeed => "04/10 WARP SPEED",
            Self::MeteorShower => "05/10 METEOR SHOWER",
            Self::Weather => "06/10 WEATHER",
            Self::ParticlePortal => "07/10 PARTICLE PORTAL",
            Self::ElectricStorm => "08/10 ELECTRIC STORM",
            Self::FountainWaterfall => "09/10 FOUNTAIN / WATERFALL",
            Self::ArcadeCabinet => "10/10 ARCADE CABINET",
        }
    }

    #[must_use]
    pub const fn starting_count(self) -> usize {
        match self {
            Self::Fireworks => 24_576,
            Self::FireEmbers | Self::MeteorShower => 20_480,
            Self::SpiralGalaxy => 81_920,
            Self::WarpSpeed => 45_056,
            Self::Weather => 49_152,
            Self::ParticlePortal => 65_536,
            Self::ElectricStorm => 16_384,
            Self::FountainWaterfall => 32_768,
            Self::ArcadeCabinet => 20_480,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParticleShowcaseRenderStats {
    pub demo: ParticleDemoKind,
    pub beat: &'static str,
    pub count: usize,
    pub visible: usize,
    pub simulation_us: u128,
    pub simulation_cpu_us: u128,
    pub projection_us: u128,
    pub projection_cpu_us: u128,
    pub geometry_us: u128,
    pub clear_us: u128,
    pub clear_cpu_us: u128,
    pub raster_us: u128,
    pub raster_cpu_us: u128,
    pub segment_count: usize,
    pub attempted_pixel_writes: usize,
    pub clipped_commands: usize,
    pub simulation_bytes: usize,
    pub renderer_scratch_bytes: usize,
}

pub struct ParticleShowcaseRenderer {
    config: ParticleShowcaseConfig,
    demo: ParticleDemoKind,
    demo_started_at: Duration,
    pool: ParticleShowcasePool,
    commands: Vec<u32>,
    previous_commands: Vec<u32>,
    segments: Vec<ParticleShowcaseSegment>,
    transition: ParticleShowcaseTransition,
    transition_started_at: Option<Duration>,
    heat: Vec<u8>,
    heat_frame: u64,
    galaxy_projected_count: usize,
    dirty_slots: [ParticleShowcaseDirtySlot; HIDDEN_SLOT_COUNT],
    hud_font: ConsoleFont,
    hud_pixels: Vec<Pixel>,
    renderer_scratch_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ParticleShowcaseSegment {
    x0: i16,
    y0: i16,
    x1: i16,
    y1: i16,
    style: u8,
}

struct ParticleShowcaseTransition {
    count: usize,
    x: Vec<f32>,
    y: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    style: Vec<u8>,
}

struct ParticleShowcaseDirtySlot {
    initialized: bool,
    offsets: Vec<u32>,
}

impl ParticleShowcaseRenderer {
    pub fn new(config: ParticleShowcaseConfig) -> Result<Self, String> {
        let config = config.validate()?;
        let pool = ParticleShowcasePool::new();
        let commands = Vec::with_capacity(PARTICLE_DEMO_MAX_COUNT + PARTICLE_DEMO_TRANSITION_COUNT);
        let previous_commands = Vec::with_capacity(PARTICLE_DEMO_MAX_COUNT);
        let segments = Vec::with_capacity(SEGMENT_CAPACITY);
        let transition = ParticleShowcaseTransition::new();
        let heat = vec![0; FIRE_HEAT_W * FIRE_HEAT_H];
        let dirty_slots = std::array::from_fn(|_| ParticleShowcaseDirtySlot {
            initialized: false,
            offsets: Vec::with_capacity(PARTICLE_DEMO_MAX_COUNT.saturating_mul(2)),
        });
        let mut hud_font =
            ConsoleFont::new_with_typeface(HUD_FONT_PX, ConsoleTypeface::PressStart2P);
        let mut hud_pixels = vec![Pixel(0); HUD_W * HUD_H];
        for kind in ParticleDemoKind::ALL {
            hud_pixels.fill(Pixel(0));
            hud_font.draw_text_clipped(
                &mut hud_pixels,
                HUD_W,
                HUD_W,
                0,
                HUD_H,
                0,
                HUD_BASELINE_Y,
                kind.hud_label(),
                Pixel(0x00bd_baff),
            );
        }
        hud_pixels.fill(Pixel(0));
        let renderer_scratch_bytes = commands
            .capacity()
            .saturating_mul(std::mem::size_of::<u32>())
            .saturating_add(
                previous_commands
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
            .saturating_add(
                dirty_slots
                    .iter()
                    .map(|slot| {
                        slot.offsets
                            .capacity()
                            .saturating_mul(std::mem::size_of::<u32>())
                    })
                    .sum::<usize>(),
            )
            .saturating_add(
                hud_pixels
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Pixel>()),
            )
            .saturating_add(
                segments
                    .capacity()
                    .saturating_mul(std::mem::size_of::<ParticleShowcaseSegment>()),
            )
            .saturating_add(transition.allocated_bytes());
        let renderer_scratch_bytes = renderer_scratch_bytes.saturating_add(heat.capacity());
        let mut renderer = Self {
            config,
            demo: config.initial_demo,
            demo_started_at: Duration::ZERO,
            pool,
            commands,
            previous_commands,
            segments,
            transition,
            transition_started_at: None,
            heat,
            heat_frame: u64::MAX,
            galaxy_projected_count: 0,
            dirty_slots,
            hud_font,
            hud_pixels,
            renderer_scratch_bytes,
        };
        renderer.reset_demo(config.initial_demo, Duration::ZERO);
        Ok(renderer)
    }

    #[must_use]
    pub const fn demo(&self) -> ParticleDemoKind {
        self.demo
    }

    #[must_use]
    pub const fn particle_count(&self) -> usize {
        self.pool.active()
    }

    pub fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        hidden_slot: u8,
        elapsed: Duration,
    ) -> Result<ParticleShowcaseRenderStats, String> {
        let frame_len = self.config.width.saturating_mul(self.config.height);
        if destination.len() != frame_len {
            return Err(format!(
                "particle showcase destination has {} pixels, expected {frame_len}",
                destination.len()
            ));
        }
        let slot = hidden_slot_offset(hidden_slot)?;
        self.apply_navigation(elapsed);

        let clear_started = Instant::now();
        let clear_cpu_started = thread_cpu_time_us();
        let mut dirty_offsets = self.prepare_hidden_slot(destination, slot);
        let clear_us = clear_started.elapsed().as_micros();
        let clear_cpu_us = elapsed_thread_cpu_us(clear_cpu_started);

        let simulation_started = Instant::now();
        let simulation_cpu_started = thread_cpu_time_us();
        self.update_effect(elapsed);
        let simulation_us = simulation_started.elapsed().as_micros();
        let simulation_cpu_us = elapsed_thread_cpu_us(simulation_cpu_started);

        let projection_started = Instant::now();
        let projection_cpu_started = thread_cpu_time_us();
        let mut clipped_commands = self.project_effect(elapsed);
        let projection_us = projection_started.elapsed().as_micros();
        let projection_cpu_us = elapsed_thread_cpu_us(projection_cpu_started);

        let geometry_started = Instant::now();
        clipped_commands =
            clipped_commands.saturating_add(self.append_transition_commands(elapsed));
        let geometry_us = geometry_started.elapsed().as_micros();

        let raster_started = Instant::now();
        let raster_cpu_started = thread_cpu_time_us();
        let mut attempted_pixel_writes = self.raster_effect_background(destination);
        let (visible, point_writes) = self.raster_points(destination, &mut dirty_offsets);
        attempted_pixel_writes = attempted_pixel_writes.saturating_add(point_writes);
        attempted_pixel_writes = attempted_pixel_writes
            .saturating_add(self.raster_segments(destination, &mut dirty_offsets));
        self.draw_hud(destination, &mut dirty_offsets);
        let raster_us = raster_started.elapsed().as_micros();
        let raster_cpu_us = elapsed_thread_cpu_us(raster_cpu_started);

        self.dirty_slots[slot].offsets = dirty_offsets;
        Ok(ParticleShowcaseRenderStats {
            demo: self.demo,
            beat: if self.transition_started_at.is_some() {
                "transition"
            } else {
                self.effect_beat(elapsed)
            },
            count: self.pool.active(),
            visible,
            simulation_us,
            simulation_cpu_us,
            projection_us,
            projection_cpu_us,
            geometry_us,
            clear_us,
            clear_cpu_us,
            raster_us,
            raster_cpu_us,
            segment_count: self.segments.len(),
            attempted_pixel_writes,
            clipped_commands,
            simulation_bytes: self.pool.allocated_bytes(),
            renderer_scratch_bytes: self.renderer_scratch_bytes,
        })
    }

    pub fn invalidate_hidden_slot(&mut self, hidden_slot: u8) {
        if let Ok(slot) = hidden_slot_offset(hidden_slot) {
            self.dirty_slots[slot].initialized = false;
            self.dirty_slots[slot].offsets.clear();
        }
    }

    fn apply_navigation(&mut self, elapsed: Duration) {
        let delta = PARTICLE_DEMO_NAVIGATION.swap(0, Ordering::AcqRel);
        if delta != 0 {
            self.begin_transition(elapsed);
            self.reset_demo(self.demo.offset_wrapped(delta), elapsed);
            return;
        }
        let demo_elapsed = elapsed.saturating_sub(self.demo_started_at);
        if demo_elapsed >= PARTICLE_DEMO_DURATION {
            let advances = (demo_elapsed.as_micros() / PARTICLE_DEMO_DURATION.as_micros()) as i32;
            self.begin_transition(elapsed);
            self.reset_demo(self.demo.offset_wrapped(advances), elapsed);
        }
    }

    fn reset_demo(&mut self, demo: ParticleDemoKind, elapsed: Duration) {
        self.demo = demo;
        self.demo_started_at = elapsed;
        self.pool.reset(demo, self.config.seed);
        self.heat.fill(0);
        self.heat_frame = u64::MAX;
        self.galaxy_projected_count = 0;
        for slot in &mut self.dirty_slots {
            slot.initialized = false;
            slot.offsets.clear();
        }
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random) * 248.0;
            self.pool.y[index] = unit_signed(random.rotate_left(11)) * 135.0;
            self.pool.z[index] = unit_signed(random.rotate_left(21)) * 88.0;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
            self.pool.flags[index] = u8::from(random & 0x1f == 0);
        }
        if demo == ParticleDemoKind::SpiralGalaxy {
            self.galaxy_projected_count = self.initialize_galaxy();
        } else if demo == ParticleDemoKind::WarpSpeed {
            self.initialize_warp_speed();
        } else if demo == ParticleDemoKind::MeteorShower {
            self.initialize_meteor_shower();
        } else if demo == ParticleDemoKind::Weather {
            self.initialize_weather();
        } else if demo == ParticleDemoKind::ParticlePortal {
            self.initialize_particle_portal();
        } else if demo == ParticleDemoKind::ElectricStorm {
            self.initialize_electric_storm();
        } else if demo == ParticleDemoKind::FountainWaterfall {
            self.initialize_fountain_waterfall();
        } else if demo == ParticleDemoKind::ArcadeCabinet {
            self.initialize_arcade_cabinet();
        }
    }

    fn prepare_hidden_slot(&mut self, destination: &mut [Rgb565Pixel], slot: usize) -> Vec<u32> {
        let dirty = &mut self.dirty_slots[slot];
        if !dirty.initialized || dirty.offsets.len() >= destination.len() / FULL_CLEAR_DIRTY_DIVISOR
        {
            destination.fill(Rgb565Pixel(0));
        } else {
            for &offset in &dirty.offsets {
                destination[offset as usize] = Rgb565Pixel(0);
            }
        }
        dirty.initialized = true;
        let mut offsets = std::mem::take(&mut dirty.offsets);
        offsets.clear();
        offsets
    }

    fn project_effect(&mut self, elapsed: Duration) -> usize {
        match self.demo {
            ParticleDemoKind::Fireworks => self.project_fireworks(elapsed),
            ParticleDemoKind::FireEmbers => self.project_fire_embers(elapsed),
            ParticleDemoKind::SpiralGalaxy => self.project_spiral_galaxy(elapsed),
            ParticleDemoKind::WarpSpeed => self.project_warp_speed(elapsed),
            ParticleDemoKind::MeteorShower => self.project_meteor_shower(elapsed),
            ParticleDemoKind::Weather => self.project_weather(elapsed),
            ParticleDemoKind::ParticlePortal => self.project_particle_portal(elapsed),
            ParticleDemoKind::ElectricStorm => self.project_electric_storm(elapsed),
            ParticleDemoKind::FountainWaterfall => self.project_fountain_waterfall(elapsed),
            ParticleDemoKind::ArcadeCabinet => self.project_arcade_cabinet(elapsed),
        }
    }

    fn update_effect(&mut self, elapsed: Duration) {
        if self.demo == ParticleDemoKind::FireEmbers {
            self.update_fire_heat(elapsed);
        }
    }

    fn effect_beat(&self, elapsed: Duration) -> &'static str {
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        match self.demo {
            ParticleDemoKind::Fireworks => match seconds.rem_euclid(4.8) {
                value if value < 1.25 => "launch",
                value if value < 2.6 => "burst",
                _ => "fall",
            },
            ParticleDemoKind::FireEmbers => match seconds.rem_euclid(10.0) {
                value if value < 3.5 => "flame",
                value if value < 7.0 => "gust",
                _ => "embers",
            },
            ParticleDemoKind::SpiralGalaxy => match seconds {
                value if value < 10.0 => "wide-orbit",
                value if value < 20.0 => "arm-pass",
                _ => "core-pulse",
            },
            ParticleDemoKind::WarpSpeed => match seconds {
                value if value < 7.0 => "calm",
                value if value < 14.0 => "accelerate",
                value if value < 23.0 => "warp",
                _ => "decelerate",
            },
            ParticleDemoKind::MeteorShower => match seconds {
                value if value < 8.0 => "quiet",
                value if value < 20.0 => "shower",
                _ => "peak",
            },
            ParticleDemoKind::Weather => match seconds {
                value if value < 10.0 => "rain",
                value if value < 20.0 => "snow",
                _ => "ash",
            },
            ParticleDemoKind::ParticlePortal => match seconds {
                value if value < 8.0 => "gather",
                value if value < 20.0 => "vortex",
                value if value < 26.0 => "pulse",
                _ => "surge",
            },
            ParticleDemoKind::ElectricStorm => match seconds {
                value if value < 8.0 => "charge",
                value if value < 16.0 => "leader",
                value if value < 22.0 => "return-stroke",
                _ => "branches",
            },
            ParticleDemoKind::FountainWaterfall => match seconds {
                value if value < 9.0 => "fountain",
                value if value < 13.0 => "morph",
                value if value < 24.0 => "waterfall",
                _ => "impact",
            },
            ParticleDemoKind::ArcadeCabinet => match seconds {
                value if value < 4.0 => "formation",
                value if value < 24.0 => "fly-around",
                value if value < 29.0 => "three-quarter",
                _ => "dispersal",
            },
        }
    }

    fn initialize_arcade_cabinet(&mut self) {
        let count = decode_particle_cloud(ARCADE_CLOUD, &mut self.pool)
            .expect("checked-in arcade particle cloud must satisfy its runtime contract");
        debug_assert_eq!(count, ParticleDemoKind::ArcadeCabinet.starting_count());
        for index in 0..count {
            let random = self.pool.random[index];
            self.pool.previous_x[index] = unit_signed(random.rotate_left(3)) * 510.0;
            self.pool.previous_y[index] = unit_signed(random.rotate_left(13)) * 300.0;
            self.pool.previous_z[index] = unit_signed(random.rotate_left(23)) * 360.0;
        }
    }

    fn project_arcade_cabinet(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let (formation, yaw, pitch, dolly, dispersal) = arcade_camera(seconds);
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5 + 16.0;
        let mut clipped = 0usize;
        for index in 0..self.pool.active() {
            let target_x = self.pool.x[index];
            let target_y = self.pool.y[index];
            let target_z = self.pool.z[index];
            let source_x = self.pool.previous_x[index];
            let source_y = self.pool.previous_y[index];
            let source_z = self.pool.previous_z[index];
            let formed_x = source_x + (target_x - source_x) * formation;
            let formed_y = source_y + (target_y - source_y) * formation;
            let formed_z = source_z + (target_z - source_z) * formation;
            let disperse_scale = 1.0 + dispersal * (0.9 + self.pool.life[index] * 1.6);
            let world_x = formed_x * disperse_scale;
            let world_y = formed_y * disperse_scale
                + dispersal * unit_signed(self.pool.random[index].rotate_left(11)) * 90.0;
            let world_z = formed_z * disperse_scale;
            let rotated_x = world_x.mul_add(cos_yaw, world_z * sin_yaw);
            let yaw_z = (-world_x).mul_add(sin_yaw, world_z * cos_yaw);
            let rotated_y = world_y.mul_add(cos_pitch, -(yaw_z * sin_pitch));
            let rotated_z = world_y.mul_add(sin_pitch, yaw_z * cos_pitch);
            let depth = dolly + rotated_z;
            if depth <= 48.0 {
                self.commands.push(u32::MAX);
                clipped = clipped.saturating_add(1);
                continue;
            }
            let scale = 610.0 / depth;
            let x = center_x + rotated_x * scale;
            let y = center_y + rotated_y * scale;
            let feature = self.pool.flags[index];
            let style = if feature & 2 != 0 {
                7
            } else if feature & 1 != 0 {
                self.pool.style[index].saturating_add(2).min(7)
            } else {
                self.pool.style[index]
            };
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                y,
                style,
                feature != 0 && index & 3 == 0,
            ) {
                clipped = clipped.saturating_add(1);
            }
        }
        clipped
    }

    fn initialize_fountain_waterfall(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let angle = std::f32::consts::TAU * unit01(random.rotate_left(5));
            let (sin_angle, cos_angle) = angle.sin_cos();
            let radial_speed = 58.0 + unit01(random.rotate_left(15)) * 118.0;
            self.pool.x[index] = unit_signed(random.rotate_left(25)) * 72.0;
            self.pool.y[index] = unit_signed(random.rotate_left(9)) * 18.0;
            self.pool.z[index] = unit_signed(random.rotate_left(19)) * 92.0;
            self.pool.vx[index] = cos_angle * radial_speed;
            self.pool.vy[index] = -285.0 - unit01(random.rotate_left(13)) * 150.0;
            self.pool.vz[index] = sin_angle * radial_speed;
            self.pool.age[index] = unit01(random.rotate_left(23)) * 2.4;
            self.pool.life[index] = 0.78 + unit01(random.rotate_left(29)) * 0.54;
            self.pool.style[index] = 2 + ((random >> 29) as u8).min(5);
            self.pool.flags[index] = ((index / 4) % 8) as u8 | (u8::from(random & 127 == 0) << 4);
        }
    }

    fn project_fountain_waterfall(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let mut clipped = 0usize;
        for index in (0..self.pool.active()).step_by(4) {
            let fountain = self.fountain_particle(index, seconds);
            let waterfall = self.waterfall_particle(index, (seconds - 9.0).max(0.0));
            let (world_x, world_y, world_z, style, neighbor) = if seconds < 9.0 {
                fountain
            } else if seconds < 13.0 {
                let blend = ease_out_cubic((seconds - 9.0) * 0.25);
                (
                    fountain.0 + (waterfall.0 - fountain.0) * blend,
                    fountain.1 + (waterfall.1 - fountain.1) * blend,
                    fountain.2 + (waterfall.2 - fountain.2) * blend,
                    if blend < 0.55 {
                        fountain.3
                    } else {
                        waterfall.3
                    },
                    fountain.4 || waterfall.4,
                )
            } else {
                self.waterfall_particle(index, seconds - 13.0)
            };
            let camera_x = if seconds < 9.0 {
                0.0
            } else {
                ease_out_cubic(((seconds - 9.0) * 0.25).min(1.0)) * 72.0
            };
            if let Some((x, y)) = project_world(
                world_x - camera_x,
                world_y,
                world_z,
                self.config.width,
                self.config.height,
                610.0,
            ) {
                if !push_screen_command(
                    &mut self.commands,
                    self.config.width,
                    self.config.height,
                    x,
                    y,
                    style,
                    neighbor,
                ) {
                    clipped = clipped.saturating_add(1);
                }
            } else {
                self.commands.push(u32::MAX);
                clipped = clipped.saturating_add(1);
            }
        }
        if seconds < 11.0 {
            self.append_fountain_basin(seconds);
        }
        if seconds >= 10.0 {
            self.append_waterfall_edges(seconds);
        }
        clipped
    }

    fn fountain_particle(&self, index: usize, seconds: f32) -> (f32, f32, f32, u8, bool) {
        let age = (seconds * self.pool.life[index] + self.pool.age[index]).rem_euclid(2.4);
        let drag = 1.0 / (1.0 + age * 0.16);
        let x = self.pool.vx[index] * age * drag;
        let y = 196.0 + self.pool.vy[index] * age * drag + 118.0 * age * age;
        let z = self.pool.vz[index] * age * drag;
        let brightness = (7.0 - age * 1.45).clamp(2.0, 7.0) as u8;
        (
            x,
            y,
            z,
            brightness,
            self.pool.flags[index] & 16 != 0 || age < 0.08,
        )
    }

    fn waterfall_particle(&self, index: usize, seconds: f32) -> (f32, f32, f32, u8, bool) {
        let class = self.pool.flags[index] & 7;
        let random_x = self.pool.x[index];
        let random_z = self.pool.z[index];
        if class <= 4 {
            let age = (seconds * (0.88 + self.pool.life[index] * 0.2) + self.pool.age[index])
                .rem_euclid(2.35);
            let y = -214.0 + age * (118.0 + age * 86.0);
            let curl = triangle_wave(age * 0.34 + self.pool.age[index]) * (5.0 + age * 7.0);
            (
                92.0 + random_x * 0.82 + curl,
                y.min(202.0),
                random_z * 0.72 + triangle_wave(age * 0.23) * 12.0,
                (3.0 + self.pool.z[index].abs() * 0.035).clamp(3.0, 7.0) as u8,
                self.pool.flags[index] & 16 != 0,
            )
        } else if class <= 6 {
            let age = (seconds * (0.72 + self.pool.life[index] * 0.25) + self.pool.age[index])
                .rem_euclid(1.55);
            let spread = 0.62 + age * 0.54;
            (
                92.0 + self.pool.vx[index] * age * spread,
                202.0 + self.pool.vy[index] * age * 0.26 + 118.0 * age * age,
                self.pool.vz[index] * age * spread,
                (7.0 - age * 2.0).clamp(3.0, 7.0) as u8,
                age < 0.18,
            )
        } else {
            let age = (seconds * (0.32 + self.pool.life[index] * 0.18) + self.pool.age[index])
                .rem_euclid(3.8);
            (
                92.0 + random_x * (0.9 + age * 0.45)
                    + triangle_wave(age * 0.19 + self.pool.life[index]) * 34.0,
                205.0 - age * (24.0 + self.pool.life[index] * 17.0),
                random_z * (0.65 + age * 0.2),
                (5.0 - age * 0.55).clamp(1.0, 5.0) as u8,
                self.pool.flags[index] & 16 != 0,
            )
        }
    }

    fn append_fountain_basin(&mut self, seconds: f32) {
        let pulse = 1.0 + triangle_wave(seconds * 0.22) * 0.035;
        for ring in 0..3 {
            let radius_x = (116.0 + ring as f32 * 34.0) * pulse;
            let radius_y = 17.0 + ring as f32 * 7.0;
            for step in 0..48 {
                let angle0 = std::f32::consts::TAU * step as f32 / 48.0;
                let angle1 = std::f32::consts::TAU * (step + 1) as f32 / 48.0;
                self.segments.push(ParticleShowcaseSegment {
                    x0: (self.config.width as f32 * 0.5 + angle0.cos() * radius_x) as i16,
                    y0: (self.config.height as f32 * 0.5 + 198.0 + angle0.sin() * radius_y) as i16,
                    x1: (self.config.width as f32 * 0.5 + angle1.cos() * radius_x) as i16,
                    y1: (self.config.height as f32 * 0.5 + 198.0 + angle1.sin() * radius_y) as i16,
                    style: 2 + ring as u8,
                });
            }
        }
    }

    fn append_waterfall_edges(&mut self, seconds: f32) {
        let blend = ease_out_cubic(((seconds - 10.0) / 3.0).clamp(0.0, 1.0));
        let center_x = self.config.width as f32 * 0.5 + 20.0;
        let top_y = self.config.height as f32 * 0.5 - 214.0;
        let bottom_y = self.config.height as f32 * 0.5 + 202.0;
        for edge in [-1.0_f32, 1.0] {
            let x = center_x + edge * 88.0 * blend;
            let steps = 36;
            for step in 0..steps {
                let y0 = top_y + (bottom_y - top_y) * step as f32 / steps as f32;
                let y1 = top_y + (bottom_y - top_y) * (step + 1) as f32 / steps as f32;
                let ripple = triangle_wave(seconds * 0.19 + step as f32 * 0.17) * 5.0;
                self.segments.push(ParticleShowcaseSegment {
                    x0: (x + ripple) as i16,
                    y0: y0 as i16,
                    x1: (x - ripple) as i16,
                    y1: y1 as i16,
                    style: 4,
                });
            }
        }
    }

    fn initialize_electric_storm(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random.rotate_left(5)) * 510.0;
            self.pool.y[index] = unit_signed(random.rotate_left(15)) * 300.0;
            self.pool.z[index] = unit01(random.rotate_left(25));
            self.pool.age[index] = unit01(random.rotate_left(9));
            self.pool.style[index] = 1 + ((random >> 30) as u8).min(3);
            self.pool.flags[index] = u8::from(random & 127 == 0);
        }
    }

    fn project_electric_storm(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let mut clipped = 0usize;
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let charge = ((seconds * 3.7).sin() * 0.5 + 0.5).powi(3);
        for index in (0..self.pool.active()).step_by(2) {
            let layer = 0.7 + self.pool.z[index] * 0.6;
            let drift = triangle_wave(seconds * 0.09 + self.pool.age[index]) * 18.0;
            let x = center_x + self.pool.x[index] * layer + drift;
            let y = center_y + self.pool.y[index] * layer;
            let spark = self.pool.flags[index] != 0 && charge > self.pool.age[index];
            let style = if spark { 5 } else { self.pool.style[index] };
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                y,
                style,
                spark,
            ) {
                clipped = clipped.saturating_add(1);
            }
        }

        if seconds >= 8.0 {
            let epoch = (seconds * if seconds < 16.0 { 1.5 } else { 4.0 }) as u32;
            let seed = xorshift32(epoch ^ fold_seed(self.config.seed, self.demo));
            let bright = seconds >= 16.0;
            let branches = seconds >= 22.0;
            self.append_lightning_bolt(seed, bright, branches);
            if seconds >= 22.0 {
                self.append_lightning_bolt(seed.rotate_left(13), false, true);
            }
        }
        clipped
    }

    fn append_lightning_bolt(&mut self, seed: u32, bright: bool, branches: bool) {
        let mut state = seed;
        let mut x = 260.0 + unit01(state) * 440.0;
        let mut y = 24.0;
        let steps = 40usize;
        for step in 0..steps {
            state = xorshift32(state);
            let next_x = (x + unit_signed(state) * 18.0).clamp(36.0, 924.0);
            let next_y = y + 12.0;
            for (offset, style) in [(-1.0, 3), (1.0, 5), (0.0, if bright { 7 } else { 6 })] {
                self.segments.push(ParticleShowcaseSegment {
                    x0: (x + offset) as i16,
                    y0: y as i16,
                    x1: (next_x + offset) as i16,
                    y1: next_y as i16,
                    style,
                });
            }
            if branches && step > 6 && step % 6 == 0 {
                let direction = if state & 1 == 0 { -1.0 } else { 1.0 };
                let mut branch_x = next_x;
                let mut branch_y = next_y;
                for _ in 0..5 {
                    state = xorshift32(state);
                    let branch_next_x = branch_x + direction * (9.0 + unit01(state) * 13.0);
                    let branch_next_y = branch_y + 7.0 + unit01(state.rotate_left(9)) * 5.0;
                    self.segments.push(ParticleShowcaseSegment {
                        x0: branch_x as i16,
                        y0: branch_y as i16,
                        x1: branch_next_x as i16,
                        y1: branch_next_y as i16,
                        style: 5,
                    });
                    branch_x = branch_next_x;
                    branch_y = branch_next_y;
                }
            }
            x = next_x;
            y = next_y;
        }
    }

    fn initialize_particle_portal(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let band = random & 1;
            let major_angle =
                std::f32::consts::TAU * (index as f32 / self.pool.active() as f32) * 19.0
                    + unit_signed(random.rotate_left(7)) * 0.08;
            let minor_angle = major_angle * (3.0 + band as f32 * 2.0)
                + std::f32::consts::TAU * unit01(random.rotate_left(17));
            let minor_radius = 28.0 + unit01(random.rotate_left(23)) * 34.0;
            let radius = 160.0 + minor_angle.cos() * minor_radius;
            self.pool.x[index] = major_angle.cos() * radius;
            self.pool.y[index] = major_angle.sin() * radius;
            self.pool.z[index] = minor_angle.sin() * minor_radius;
            self.pool.age[index] = unit01(random.rotate_left(11));
            self.pool.style[index] = 3 + ((random >> 29) as u8).min(4);
            self.pool.flags[index] =
                band as u8 | (u8::from(index & 127 == 0) << 1) | (u8::from(index & 511 == 0) << 2);
        }
    }

    fn project_particle_portal(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let forward_angle = seconds * 0.42;
        let reverse_angle = seconds * -0.31;
        let (forward_sin, forward_cos) = forward_angle.sin_cos();
        let (reverse_sin, reverse_cos) = reverse_angle.sin_cos();
        let (previous_forward_sin, previous_forward_cos) = (-0.035_f32).sin_cos();
        let (previous_reverse_sin, previous_reverse_cos) = 0.035_f32.sin_cos();
        let (tilt_sin, tilt_cos) = 0.42_f32.sin_cos();
        let pulse = 0.94 + ((seconds * 1.9).sin() * 0.5 + 0.5) * 0.12;
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let mut clipped = 0usize;

        for index in (0..self.pool.active()).step_by(8) {
            if index & 63 == 0 {
                let rim_index = index / 64;
                let rim_count = self.pool.active() / 64;
                let rim_lane = (rim_index & 1) as f32;
                let angle = std::f32::consts::TAU * ((rim_index / 2) as f32 + rim_lane * 0.5)
                    / (rim_count / 2) as f32
                    + seconds * 0.08;
                let (sin_angle, cos_angle) = angle.sin_cos();
                let radius =
                    168.0 + rim_lane * 14.0 + 3.0 * triangle_wave(rim_index as f32 * 0.381_966);
                let rim_x = cos_angle * radius;
                let rim_y = sin_angle * radius;
                let rim_z = triangle_wave(angle * 1.5 + seconds * 0.12) * 14.0;
                let y = rim_y.mul_add(tilt_cos, -(rim_z * tilt_sin));
                let depth_axis = rim_y.mul_add(tilt_sin, rim_z * tilt_cos);
                let scale = 570.0 / (570.0 + depth_axis);
                if !push_screen_command(
                    &mut self.commands,
                    self.config.width,
                    self.config.height,
                    center_x + rim_x * scale,
                    center_y + y * scale,
                    7,
                    true,
                ) {
                    clipped = clipped.saturating_add(1);
                }
                continue;
            }
            let reverse = self.pool.flags[index] & 1 != 0;
            let (sin_rotation, cos_rotation) = if reverse {
                (reverse_sin, reverse_cos)
            } else {
                (forward_sin, forward_cos)
            };
            let phase = (seconds * (0.24 + self.pool.age[index] * 0.16) + self.pool.age[index])
                .rem_euclid(1.0);
            let inward = 1.0 - phase * phase * 0.18;
            let base_x = self.pool.x[index];
            let base_y = self.pool.y[index];
            let x = base_x.mul_add(cos_rotation, -(base_y * sin_rotation)) * inward * pulse;
            let ring_y = base_x.mul_add(sin_rotation, base_y * cos_rotation) * inward * pulse;
            let tendril = if self.pool.flags[index] & 2 != 0 {
                triangle_wave(seconds * 0.22 + self.pool.age[index]) * 78.0
            } else {
                0.0
            };
            let z = self.pool.z[index] + tendril;
            let y = ring_y.mul_add(tilt_cos, -(z * tilt_sin));
            let depth_axis = ring_y.mul_add(tilt_sin, z * tilt_cos);
            let depth = 570.0 + depth_axis;
            let scale = 570.0 / depth.max(96.0);
            let screen_x = center_x + x * scale;
            let screen_y = center_y + y * scale;
            let depth_style = ((depth_axis + 230.0) * (3.0 / 460.0)).clamp(0.0, 3.0) as u8;
            let style = (self.pool.style[index].min(4) + depth_style).min(7);
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                screen_x,
                screen_y,
                style,
                self.pool.flags[index] & 4 != 0,
            ) {
                clipped = clipped.saturating_add(1);
                continue;
            }
            if self.pool.flags[index] & 2 != 0 {
                let (previous_sin, previous_cos) = if reverse {
                    (previous_reverse_sin, previous_reverse_cos)
                } else {
                    (previous_forward_sin, previous_forward_cos)
                };
                let previous_x = x.mul_add(previous_cos, -(ring_y * previous_sin));
                let previous_y = x.mul_add(previous_sin, ring_y * previous_cos);
                self.segments.push(ParticleShowcaseSegment {
                    x0: (center_x + previous_x * scale) as i16,
                    y0: (center_y + previous_y * tilt_cos * scale) as i16,
                    x1: screen_x as i16,
                    y1: screen_y as i16,
                    style,
                });
            }
        }
        clipped
    }

    fn initialize_weather(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random.rotate_left(3)) * 520.0;
            self.pool.y[index] = unit01(random.rotate_left(13)) * 620.0 - 40.0;
            self.pool.z[index] = unit01(random.rotate_left(23));
            self.pool.age[index] = unit01(random.rotate_left(7)) * 8.0;
            self.pool.life[index] = 0.65 + unit01(random.rotate_left(17)) * 0.7;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
            self.pool.flags[index] = u8::from(random & 63 == 0);
        }
    }

    fn project_weather(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        if seconds < 10.0 {
            self.project_weather_rain(seconds)
        } else if seconds < 20.0 {
            self.project_weather_snow(seconds - 10.0)
        } else {
            self.project_weather_ash(seconds - 20.0)
        }
    }

    fn project_weather_rain(&mut self, seconds: f32) -> usize {
        let mut clipped = 0usize;
        let wind = 34.0 + triangle_wave(seconds * 0.08) * 24.0;
        for index in (0..self.pool.active()).step_by(4) {
            let layer = 0.62 + self.pool.z[index] * 0.72;
            let fall = (self.pool.y[index] + seconds * (410.0 + 330.0 * self.pool.z[index]))
                .rem_euclid(620.0)
                - 40.0;
            let x = self.config.width as f32 * 0.5
                + self.pool.x[index] * layer
                + wind * (fall / self.config.height as f32);
            let style = 1 + (self.pool.z[index] * 2.8) as u8;
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                fall,
                style,
                self.pool.flags[index] != 0,
            ) {
                clipped = clipped.saturating_add(1);
                continue;
            }
            if index & 31 == 0 {
                self.segments.push(ParticleShowcaseSegment {
                    x0: (x - wind * 0.016) as i16,
                    y0: (fall - 12.0 - self.pool.z[index] * 10.0) as i16,
                    x1: x as i16,
                    y1: fall as i16,
                    style,
                });
            }
        }
        clipped
    }

    fn project_weather_snow(&mut self, seconds: f32) -> usize {
        let mut clipped = 0usize;
        let gust = triangle_wave(seconds * 0.055) * 42.0;
        for index in (0..self.pool.active()).step_by(8) {
            let layer = 0.72 + self.pool.z[index] * 0.58;
            let fall = (self.pool.y[index]
                + seconds * (34.0 + 58.0 * self.pool.z[index]) * self.pool.life[index])
                .rem_euclid(600.0)
                - 30.0;
            let flutter =
                triangle_wave(seconds * self.pool.life[index] * 0.42 + self.pool.age[index])
                    * (12.0 + self.pool.z[index] * 25.0);
            let x = self.config.width as f32 * 0.5
                + self.pool.x[index] * layer
                + flutter
                + gust * self.pool.z[index];
            let style = 2 + u8::from(self.pool.z[index] > 0.45);
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                fall,
                style,
                self.pool.flags[index] != 0 || index & 127 == 0,
            ) {
                clipped = clipped.saturating_add(1);
            }
        }
        clipped
    }

    fn project_weather_ash(&mut self, seconds: f32) -> usize {
        let mut clipped = 0usize;
        let wind = triangle_wave(seconds * 0.07) * 55.0;
        for index in (0..self.pool.active()).step_by(8) {
            let age = (seconds * self.pool.life[index] + self.pool.age[index]).rem_euclid(6.4);
            let layer = 0.68 + self.pool.z[index] * 0.7;
            let rise = 570.0 - age * (62.0 + self.pool.z[index] * 42.0);
            let curl = triangle_wave(age * 0.19 + self.pool.age[index]) * (18.0 + age * 5.0);
            let x = self.config.width as f32 * 0.5
                + self.pool.x[index] * layer
                + wind * age * 0.09
                + curl;
            let style = (7.0 - age * 0.62).clamp(4.0, 7.0) as u8;
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                rise,
                style,
                self.pool.flags[index] != 0 || index & 255 == 0,
            ) {
                clipped = clipped.saturating_add(1);
                continue;
            }
            if index & 63 == 0 {
                self.segments.push(ParticleShowcaseSegment {
                    x0: (x - curl * 0.18) as i16,
                    y0: (rise + 7.0) as i16,
                    x1: x as i16,
                    y1: rise as i16,
                    style: style.saturating_sub(1),
                });
            }
        }
        clipped
    }

    fn initialize_meteor_shower(&mut self) {
        let star_count = self.pool.active().saturating_sub(METEOR_PARTICLE_COUNT);
        for index in 0..star_count {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random.rotate_left(3)) * 760.0;
            self.pool.y[index] = unit_signed(random.rotate_left(13)) * 430.0;
            self.pool.z[index] = unit01(random.rotate_left(23)) * 760.0 - 80.0;
            self.pool.style[index] = 1 + ((random >> 30) as u8).min(2);
            self.pool.flags[index] = u8::from(random & 255 == 0);
        }
        for track in 0..METEOR_TRACK_COUNT {
            let first = star_count + track * METEOR_TRAIL_SAMPLES;
            let random = self.pool.random[first];
            let angle = -0.15 + unit01(random.rotate_left(7)) * 3.45;
            let radius = 280.0 + unit01(random.rotate_left(17)) * 1_050.0;
            let invariant_x = angle.cos() * radius;
            let invariant_y = angle.sin() * radius;
            let phase = unit01(random.rotate_left(27)) * 3.1;
            for sample in 0..METEOR_TRAIL_SAMPLES {
                let index = first + sample;
                self.pool.x[index] = invariant_x;
                self.pool.y[index] = invariant_y;
                self.pool.z[index] = phase;
                self.pool.style[index] = 4 + ((sample * 4 / METEOR_TRAIL_SAMPLES) as u8).min(3);
                self.pool.flags[index] = u8::from(sample + 1 == METEOR_TRAIL_SAMPLES);
            }
        }
    }

    fn project_meteor_shower(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let star_count = self.pool.active().saturating_sub(METEOR_PARTICLE_COUNT);
        let (sin_drift, cos_drift) = (seconds * 0.018).sin_cos();
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let mut clipped = 0usize;

        for index in 0..star_count {
            let x = self.pool.x[index];
            let z = self.pool.z[index];
            let rotated_x = x.mul_add(cos_drift, z * sin_drift);
            let rotated_z = (-x).mul_add(sin_drift, z * cos_drift);
            let depth = 760.0 + rotated_z;
            let scale = 760.0 / depth.max(96.0);
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                center_x + rotated_x * scale,
                center_y + self.pool.y[index] * scale,
                self.pool.style[index],
                self.pool.flags[index] != 0,
            ) {
                clipped = clipped.saturating_add(1);
            }
        }

        let active_tracks = if seconds < 8.0 {
            METEOR_TRACK_COUNT / 4
        } else if seconds < 20.0 {
            METEOR_TRACK_COUNT / 2
        } else {
            METEOR_TRACK_COUNT
        };
        let focal = 620.0;
        let radiant_x = center_x + 186.0;
        let radiant_y = center_y - 154.0;
        for track in 0..METEOR_TRACK_COUNT {
            let first = star_count + track * METEOR_TRAIL_SAMPLES;
            if track >= active_tracks {
                self.commands
                    .resize(self.commands.len() + METEOR_TRAIL_SAMPLES, u32::MAX);
                continue;
            }
            let random = self.pool.random[first];
            let rate = 0.82 + unit01(random.rotate_left(11)) * 0.34;
            let age = (seconds * rate + self.pool.z[first]).rem_euclid(3.1);
            let head_depth = 1_900.0 - age * 590.0;
            let mut tail = None;
            let mut head = None;
            for sample in 0..METEOR_TRAIL_SAMPLES {
                let index = first + sample;
                let depth = head_depth + (METEOR_TRAIL_SAMPLES - 1 - sample) as f32 * 9.5;
                let x = radiant_x + focal * self.pool.x[index] / depth;
                let y = radiant_y + focal * self.pool.y[index] / depth;
                if !push_screen_command(
                    &mut self.commands,
                    self.config.width,
                    self.config.height,
                    x,
                    y,
                    self.pool.style[index],
                    self.pool.flags[index] != 0,
                ) {
                    clipped = clipped.saturating_add(1);
                    continue;
                }
                tail.get_or_insert((x, y));
                head = Some((x, y));
            }
            if let (Some((tail_x, tail_y)), Some((head_x, head_y))) = (tail, head) {
                self.segments.push(ParticleShowcaseSegment {
                    x0: tail_x as i16,
                    y0: tail_y as i16,
                    x1: head_x as i16,
                    y1: head_y as i16,
                    style: 6 + u8::from(track & 3 == 0),
                });
            }
        }
        clipped
    }

    fn initialize_warp_speed(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random.rotate_left(5)) * 580.0;
            self.pool.y[index] = unit_signed(random.rotate_left(17)) * 330.0;
            self.pool.z[index] = unit01(random.rotate_left(27));
            self.pool.style[index] = 3 + ((random >> 30) as u8).min(4);
            self.pool.flags[index] = 1;
        }
    }

    fn project_warp_speed(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.previous_commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let (travel, speed) = warp_travel_and_speed(seconds);
        let previous_step = (speed * 0.05).clamp(0.002, 0.048);
        #[cfg(target_arch = "arm")]
        let visible = {
            self.commands.resize(self.pool.active(), u32::MAX);
            self.previous_commands.resize(self.pool.active(), u32::MAX);
            unsafe {
                mister_magik_showcase_neon_project_warp(
                    self.pool.active(),
                    self.config.width,
                    self.config.height,
                    travel,
                    previous_step,
                    self.pool.x.as_ptr(),
                    self.pool.y.as_ptr(),
                    self.pool.z.as_ptr(),
                    self.pool.style.as_ptr(),
                    self.commands.as_mut_ptr(),
                    self.previous_commands.as_mut_ptr(),
                )
            }
        };
        #[cfg(not(target_arch = "arm"))]
        let visible = self.project_warp_speed_scalar(travel, previous_step);

        let stride = if speed < 0.12 {
            256
        } else if speed < 0.55 {
            128
        } else {
            64
        };
        for index in (0..self.commands.len()).step_by(stride) {
            let current = self.commands[index];
            let previous = self.previous_commands[index];
            if current == u32::MAX || previous == u32::MAX {
                continue;
            }
            let current_offset = (current & COMMAND_OFFSET_MASK) as usize;
            let previous_offset = (previous & COMMAND_OFFSET_MASK) as usize;
            self.segments.push(ParticleShowcaseSegment {
                x0: (previous_offset % self.config.width) as i16,
                y0: (previous_offset / self.config.width) as i16,
                x1: (current_offset % self.config.width) as i16,
                y1: (current_offset / self.config.width) as i16,
                style: ((current >> COMMAND_STYLE_SHIFT) & 7) as u8,
            });
        }
        self.pool.active().saturating_sub(visible)
    }

    #[cfg(not(target_arch = "arm"))]
    fn project_warp_speed_scalar(&mut self, travel: f32, previous_step: f32) -> usize {
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let mut visible = 0usize;
        for index in 0..self.pool.active() {
            let depth = (self.pool.z[index] - travel).rem_euclid(1.0);
            let previous_depth = (depth + previous_step).rem_euclid(1.0);
            let scale = 0.22 / (0.14 + depth);
            let previous_scale = 0.22 / (0.14 + previous_depth);
            let x = center_x + self.pool.x[index] * scale;
            let y = center_y + self.pool.y[index] * scale;
            let previous_x = center_x + self.pool.x[index] * previous_scale;
            let previous_y = center_y + self.pool.y[index] * previous_scale;
            let emitted = push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                y,
                self.pool.style[index],
                false,
            );
            visible += usize::from(emitted);
            if previous_x >= 0.0
                && previous_y >= 0.0
                && previous_x < self.config.width as f32
                && previous_y < self.config.height as f32
            {
                self.previous_commands
                    .push((previous_y as usize * self.config.width + previous_x as usize) as u32);
            } else {
                self.previous_commands.push(u32::MAX);
            }
        }
        visible
    }

    fn initialize_galaxy(&mut self) -> usize {
        let bulge_count = self.pool.active() * 3 / 20;
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let azimuth = std::f32::consts::TAU * unit01(random);
            if index < bulge_count {
                let radius = 94.0 * unit01(random.rotate_left(7)).cbrt();
                let vertical = unit_signed(random.rotate_left(17));
                let planar = (1.0 - vertical * vertical).max(0.0).sqrt() * radius;
                self.pool.x[index] = azimuth.cos() * planar;
                self.pool.y[index] = vertical * radius * 0.68;
                self.pool.z[index] = azimuth.sin() * planar;
                self.pool.style[index] = 6 + u8::from(index & 15 == 0);
                self.pool.flags[index] = 1 | (u8::from(index & 255 == 0) << 1);
                continue;
            }

            let radius = 32.0 + unit01(random.rotate_left(5)).sqrt() * 382.0;
            let arm = (random.rotate_left(11) & 3) as f32;
            let uneven = unit_signed(random.rotate_left(19)) * (0.16 + radius * 0.0007);
            let angle = arm * std::f32::consts::FRAC_PI_2 + (radius / 32.0).ln() * 1.48 + uneven;
            let thickness = (34.0 - radius * 0.055).max(7.0) * unit_signed(random.rotate_left(23));
            self.pool.x[index] = angle.cos() * radius;
            self.pool.y[index] = thickness;
            self.pool.z[index] = angle.sin() * radius;
            let outer = ((radius - 32.0) * (4.0 / 382.0)).clamp(0.0, 3.0) as u8;
            self.pool.style[index] = 5u8.saturating_sub(outer);
            let dust_lane = random.rotate_left(3) & 31 < 10;
            self.pool.flags[index] = if dust_lane {
                0
            } else {
                1 | (u8::from(random & 511 == 0) << 1)
            };
        }
        let (sin_tilt, cos_tilt) = 0.92_f32.sin_cos();
        for index in 0..self.pool.active() {
            let y = self.pool.y[index];
            let z = self.pool.z[index];
            let tilted_y = y.mul_add(cos_tilt, -(z * sin_tilt));
            let tilted_z = y.mul_add(sin_tilt, z * cos_tilt);
            let perspective = 650.0 / (650.0 + tilted_z);
            self.pool.x[index] *= perspective;
            self.pool.y[index] = tilted_y * perspective;
            self.pool.z[index] = tilted_z;
        }
        let mut projected_count = 0usize;
        for index in 0..self.pool.active() {
            if self.pool.flags[index] == 0 {
                continue;
            }
            if projected_count != index {
                self.pool.swap_projection_entries(projected_count, index);
            }
            projected_count += 1;
        }
        projected_count
    }

    fn project_spiral_galaxy(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let (sin_yaw, cos_yaw) = (seconds * 0.042).sin_cos();
        let core_pulse = ((seconds * 1.7).sin() * 0.5 + 0.5) * 0.18 + 0.82;
        let bulge_count = self.pool.active() * 3 / 20;
        #[cfg(target_arch = "arm")]
        {
            self.commands.resize(self.galaxy_projected_count, u32::MAX);
            let visible = unsafe {
                mister_magik_showcase_neon_project_galaxy(
                    self.galaxy_projected_count,
                    self.config.width,
                    self.config.height,
                    bulge_count,
                    sin_yaw,
                    cos_yaw,
                    core_pulse,
                    self.pool.x.as_ptr(),
                    self.pool.y.as_ptr(),
                    self.pool.style.as_ptr(),
                    self.pool.flags.as_ptr(),
                    self.commands.as_mut_ptr(),
                )
            };
            return self.galaxy_projected_count.saturating_sub(visible);
        }
        #[cfg(not(target_arch = "arm"))]
        let mut clipped = 0usize;
        #[cfg(not(target_arch = "arm"))]
        for index in 0..self.galaxy_projected_count {
            let x = self.pool.x[index];
            let y = self.pool.y[index];
            let display_x = x.mul_add(cos_yaw, -(y * sin_yaw));
            let display_y = x.mul_add(sin_yaw, y * cos_yaw);
            let scale = if index < bulge_count { core_pulse } else { 1.0 };
            let screen_x = self.config.width as f32 * 0.5 + display_x * scale;
            let screen_y = self.config.height as f32 * 0.5 + display_y * scale;
            if screen_x < 0.0
                || screen_y < 0.0
                || screen_x >= self.config.width as f32
                || screen_y >= self.config.height as f32
            {
                self.commands.push(u32::MAX);
                clipped = clipped.saturating_add(1);
                continue;
            }
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                screen_x,
                screen_y,
                self.pool.style[index],
                index & 511 == 0,
            ) {
                clipped = clipped.saturating_add(1);
            }
        }
        #[cfg(not(target_arch = "arm"))]
        clipped
    }

    fn update_fire_heat(&mut self, elapsed: Duration) {
        let frame = (elapsed.saturating_sub(self.demo_started_at).as_micros() / 16_667) as u64;
        if frame == self.heat_frame {
            return;
        }
        self.heat_frame = frame;
        let bottom = (FIRE_HEAT_H - 1) * FIRE_HEAT_W;
        for x in 0..FIRE_HEAT_W {
            let centered = (x as f32 - FIRE_HEAT_W as f32 * 0.5).abs() / (FIRE_HEAT_W as f32 * 0.5);
            let envelope = ((1.0 - centered).max(0.0) * 72.0) as u8;
            let hash = xorshift32(
                (frame as u32)
                    .wrapping_mul(0x9e37_79b9)
                    .wrapping_add(x as u32 * 0x45d9_f3b),
            );
            let flicker = ((hash >> 25) & 0x7f) as u8;
            self.heat[bottom + x] = 150u8.saturating_add(envelope).saturating_add(flicker);
        }
        for y in 0..FIRE_HEAT_H - 1 {
            let row = y * FIRE_HEAT_W;
            let source_row = row + FIRE_HEAT_W;
            for x in 0..FIRE_HEAT_W {
                let hash = xorshift32(
                    (frame as u32)
                        .wrapping_add((y * FIRE_HEAT_W + x) as u32)
                        .wrapping_mul(0x85eb_ca6b),
                );
                let drift = match hash & 3 {
                    0 => -1,
                    1 => 1,
                    _ => 0,
                };
                let source_x = (x as isize + drift).clamp(0, FIRE_HEAT_W as isize - 1) as usize;
                let cooling = ((hash >> 8) & 7) as u8;
                self.heat[row + x] = self.heat[source_row + source_x].saturating_sub(cooling);
            }
        }
    }

    fn project_fire_embers(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let wind = (seconds * 0.72).sin() * 26.0 + (seconds * 0.19).sin() * 13.0;
        let mut clipped = 0usize;
        let live_embers = self.pool.active() / 4;
        for ember in 0..live_embers {
            let index = ember * 4;
            let random = self.pool.random[index];
            let age = (seconds * (0.72 + unit01(random.rotate_left(13)) * 0.38)
                + unit01(random) * 5.6)
                .rem_euclid(5.6);
            if age < 0.12 {
                continue;
            }
            let base_x = unit_signed(random.rotate_left(7)) * 390.0;
            let turbulence = unit_signed(random.rotate_left(19)) * age * age * 7.0;
            let x = base_x + wind * age * 0.12 + turbulence;
            let y = 252.0 - age * (67.0 + unit01(random.rotate_left(3)) * 31.0);
            let z = unit_signed(random.rotate_left(23)) * 72.0 + age * 5.0;
            let style = ((1.0 - age / 5.6) * 7.0).clamp(2.0, 7.0) as u8;
            let Some((screen_x, screen_y)) =
                project_world(x, y, z, self.config.width, self.config.height, 520.0)
            else {
                self.commands.push(u32::MAX);
                clipped = clipped.saturating_add(1);
                continue;
            };
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                screen_x,
                screen_y,
                style,
                ember & 31 == 0,
            ) {
                clipped = clipped.saturating_add(1);
            }
        }
        clipped
    }

    fn project_fireworks(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let width = self.config.width as f32;
        let height = self.config.height as f32;
        let particles_per_burst = 768usize;
        let burst_count = self.pool.active().div_ceil(particles_per_burst);
        let mut clipped = 0usize;
        for index in 0..self.pool.active() {
            let burst = index / particles_per_burst;
            let lane = index % particles_per_burst;
            let random = self.pool.random[index];
            let start = burst as f32 * 0.82;
            let local = (seconds - start).rem_euclid((burst_count as f32 * 0.82).max(4.8));
            let burst_x = width * (0.12 + 0.76 * unit01(random.rotate_left(5)));
            let apex_y = height * (0.16 + 0.31 * unit01(random.rotate_left(13)));
            let launch_duration = 1.25;
            if local < launch_duration {
                if lane & 31 != 0 {
                    self.commands.push(u32::MAX);
                    continue;
                }
                let trail = (lane / 32) as f32 / 24.0;
                let progress = (local / launch_duration - trail * 0.16).clamp(0.0, 1.0);
                let x = burst_x + (unit_signed(random.rotate_left(17)) * 3.0) * progress;
                let y = height - 18.0 - (height - apex_y - 18.0) * ease_out_cubic(progress);
                let style = 4 + ((lane / 32) & 3) as u8;
                if !push_screen_command(
                    &mut self.commands,
                    self.config.width,
                    self.config.height,
                    x,
                    y,
                    style,
                    lane & 63 == 0,
                ) {
                    clipped = clipped.saturating_add(1);
                }
                continue;
            }

            let age = local - launch_duration;
            if age > 3.55 {
                self.commands.push(u32::MAX);
                continue;
            }
            let angle = std::f32::consts::TAU * unit01(random);
            let vertical = unit_signed(random.rotate_left(9));
            let ring_radius = (1.0 - vertical * vertical).max(0.0).sqrt();
            let template = burst & 3;
            let base_speed = match template {
                0 => 66.0 + 54.0 * unit01(random.rotate_left(19)),
                1 => 88.0 + 18.0 * unit01(random.rotate_left(19)),
                2 => 52.0 + 76.0 * unit01(random.rotate_left(19)).powi(2),
                _ => 72.0 + 38.0 * unit01(random.rotate_left(19)),
            };
            let (mut dx, mut dy, mut dz) = match template {
                0 => (
                    angle.cos() * ring_radius,
                    vertical,
                    angle.sin() * ring_radius,
                ),
                1 => (angle.cos(), angle.sin() * 0.18, angle.sin()),
                2 => (
                    angle.cos() * (0.35 + 0.65 * ring_radius),
                    -vertical.abs() * 0.95 - 0.1,
                    angle.sin() * (0.35 + 0.65 * ring_radius),
                ),
                _ => (
                    angle.cos() * ring_radius,
                    vertical * 0.72,
                    angle.sin() * ring_radius,
                ),
            };
            if template == 3 {
                dy -= age * 0.16;
            }
            let drag = (-age * 0.31).exp();
            dx *= base_speed * drag;
            dy *= base_speed * drag;
            dz *= base_speed * drag;
            let world_x = burst_x - width * 0.5 + dx * age;
            let world_y = apex_y - height * 0.5 + dy * age + 34.0 * age * age;
            let world_z = dz * age;
            let style_base = ((burst * 3 + lane / 96) & 7) as u8;
            let fade = ((1.0 - age / 3.55) * 7.0) as u8;
            let style = style_base.min(fade).max(1);
            let Some((x, y)) = project_world(
                world_x,
                world_y,
                world_z,
                self.config.width,
                self.config.height,
                470.0,
            ) else {
                self.commands.push(u32::MAX);
                clipped = clipped.saturating_add(1);
                continue;
            };
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                y,
                style,
                lane & 127 == 0,
            ) {
                clipped = clipped.saturating_add(1);
                continue;
            }
            if lane & 31 == 0 && age > 0.05 {
                let previous_age = age - 0.05;
                let previous_drag = (-previous_age * 0.31).exp();
                let previous_world_x =
                    burst_x - width * 0.5 + dx / drag * previous_drag * previous_age;
                let previous_world_y = apex_y - height * 0.5
                    + dy / drag * previous_drag * previous_age
                    + 34.0 * previous_age * previous_age;
                let previous_world_z = dz / drag * previous_drag * previous_age;
                if let Some((previous_x, previous_y)) = project_world(
                    previous_world_x,
                    previous_world_y,
                    previous_world_z,
                    self.config.width,
                    self.config.height,
                    470.0,
                ) {
                    self.segments.push(ParticleShowcaseSegment {
                        x0: previous_x as i16,
                        y0: previous_y as i16,
                        x1: x as i16,
                        y1: y as i16,
                        style,
                    });
                }
            }
        }
        clipped
    }

    fn begin_transition(&mut self, elapsed: Duration) {
        let visible = self
            .commands
            .iter()
            .filter(|command| **command != u32::MAX)
            .count();
        if visible == 0 {
            self.transition.count = 0;
            self.transition_started_at = None;
            return;
        }
        let count = visible.min(PARTICLE_DEMO_TRANSITION_COUNT);
        let stride = visible.div_ceil(count);
        let mut source_index = 0usize;
        let mut transition_index = 0usize;
        while transition_index < count && source_index < self.commands.len() {
            let command = self.commands[source_index];
            source_index = source_index.saturating_add(stride);
            if command == u32::MAX {
                continue;
            }
            let offset = (command & COMMAND_OFFSET_MASK) as usize;
            let x = (offset % self.config.width) as f32;
            let y = (offset / self.config.width) as f32;
            let dx = x - self.config.width as f32 * 0.5;
            let dy = y - self.config.height as f32 * 0.5;
            let inverse_length = (dx.mul_add(dx, dy * dy)).sqrt().max(1.0).recip();
            let jitter =
                unit_signed(self.pool.random[transition_index % self.pool.active()].rotate_left(7));
            self.transition.x[transition_index] = x;
            self.transition.y[transition_index] = y;
            self.transition.vx[transition_index] = dx * inverse_length * (38.0 + jitter * 12.0);
            self.transition.vy[transition_index] =
                dy * inverse_length * (38.0 + jitter * 12.0) - 8.0;
            self.transition.style[transition_index] = ((command >> COMMAND_STYLE_SHIFT) & 7) as u8;
            transition_index += 1;
        }
        self.transition.count = transition_index;
        self.transition_started_at = (transition_index > 0).then_some(elapsed);
    }

    fn append_transition_commands(&mut self, elapsed: Duration) -> usize {
        let Some(started) = self.transition_started_at else {
            return 0;
        };
        let age = elapsed.saturating_sub(started);
        if age >= PARTICLE_DEMO_TRANSITION_DURATION {
            self.transition.count = 0;
            self.transition_started_at = None;
            return 0;
        }
        let t = age.as_secs_f32();
        let life = 1.0 - age.as_secs_f32() / PARTICLE_DEMO_TRANSITION_DURATION.as_secs_f32();
        let mut clipped = 0usize;
        for index in 0..self.transition.count {
            if (index & 3) as f32 > life * 4.0 {
                continue;
            }
            let x = self.transition.x[index] + self.transition.vx[index] * t;
            let y = self.transition.y[index] + self.transition.vy[index] * t + 28.0 * t * t;
            if x < 0.0 || y < 0.0 || x >= self.config.width as f32 || y >= self.config.height as f32
            {
                clipped = clipped.saturating_add(1);
                continue;
            }
            let offset = y as usize * self.config.width + x as usize;
            let style = self.transition.style[index].min((life * 7.0) as u8);
            self.commands
                .push(offset as u32 | u32::from(style) << COMMAND_STYLE_SHIFT);
        }
        clipped
    }

    fn raster_segments(
        &self,
        destination: &mut [Rgb565Pixel],
        dirty_offsets: &mut Vec<u32>,
    ) -> usize {
        let mut writes = 0usize;
        for segment in &self.segments {
            writes = writes.saturating_add(raster_bounded_segment(
                destination,
                dirty_offsets,
                self.config.width,
                self.config.height,
                *segment,
                showcase_palette(self.demo),
            ));
        }
        writes
    }

    fn raster_effect_background(&self, destination: &mut [Rgb565Pixel]) -> usize {
        if self.demo != ParticleDemoKind::FireEmbers {
            return 0;
        }
        let top = self
            .config
            .height
            .saturating_sub(FIRE_HEAT_H * FIRE_HEAT_SCALE);
        let mut writes = 0usize;
        for heat_y in 0..FIRE_HEAT_H {
            for heat_x in 0..FIRE_HEAT_W {
                let style = usize::from(self.heat[heat_y * FIRE_HEAT_W + heat_x] >> 5).min(7);
                let color = FIRE_PALETTE[style];
                let x0 = heat_x * FIRE_HEAT_SCALE;
                let y0 = top + heat_y * FIRE_HEAT_SCALE;
                for y in y0..(y0 + FIRE_HEAT_SCALE).min(self.config.height) {
                    let row = y * self.config.width;
                    for x in x0..(x0 + FIRE_HEAT_SCALE).min(self.config.width) {
                        destination[row + x] = color;
                        writes = writes.saturating_add(1);
                    }
                }
            }
        }
        writes
    }

    fn raster_points(
        &self,
        destination: &mut [Rgb565Pixel],
        dirty_offsets: &mut Vec<u32>,
    ) -> (usize, usize) {
        let mut visible = 0usize;
        let mut writes = 0usize;
        for &command in &self.commands {
            if command == u32::MAX {
                continue;
            }
            let offset = (command & COMMAND_OFFSET_MASK) as usize;
            let style = ((command >> COMMAND_STYLE_SHIFT) & 7) as usize;
            destination[offset] = showcase_palette(self.demo)[style];
            dirty_offsets.push(offset as u32);
            writes = writes.saturating_add(1);
            if command & COMMAND_NEIGHBOR != 0 {
                destination[offset + 1] = showcase_palette(self.demo)[style.saturating_sub(1)];
                dirty_offsets.push((offset + 1) as u32);
                writes = writes.saturating_add(1);
            }
            visible = visible.saturating_add(1);
        }
        (visible, writes)
    }

    fn draw_hud(&mut self, destination: &mut [Rgb565Pixel], dirty_offsets: &mut Vec<u32>) {
        self.hud_pixels.fill(Pixel(0));
        self.hud_font.draw_text_clipped(
            &mut self.hud_pixels,
            HUD_W,
            HUD_W,
            0,
            HUD_H,
            0,
            HUD_BASELINE_Y,
            self.demo.hud_label(),
            Pixel(0x00bd_baff),
        );
        let max_x = (HUD_X as usize + HUD_W).min(self.config.width);
        for y in 0..HUD_H.min(self.config.height) {
            let row = y * self.config.width;
            for x in HUD_X as usize..max_x {
                let offset = row + x;
                let source = self.hud_pixels[y * HUD_W + x - HUD_X as usize];
                if source.0 != 0 {
                    destination[offset] = pixel_to_rgb565(source);
                    dirty_offsets.push(offset as u32);
                }
            }
        }
    }
}

impl ParticleShowcaseTransition {
    fn new() -> Self {
        Self {
            count: 0,
            x: vec![0.0; PARTICLE_DEMO_TRANSITION_COUNT],
            y: vec![0.0; PARTICLE_DEMO_TRANSITION_COUNT],
            vx: vec![0.0; PARTICLE_DEMO_TRANSITION_COUNT],
            vy: vec![0.0; PARTICLE_DEMO_TRANSITION_COUNT],
            style: vec![0; PARTICLE_DEMO_TRANSITION_COUNT],
        }
    }

    fn allocated_bytes(&self) -> usize {
        (self.x.capacity() + self.y.capacity() + self.vx.capacity() + self.vy.capacity())
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_add(self.style.capacity())
    }
}

pub fn request_particle_demo_navigation(delta: i32) {
    if delta != 0 {
        PARTICLE_DEMO_NAVIGATION.fetch_add(delta, Ordering::AcqRel);
    }
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

    fn swap_projection_entries(&mut self, first: usize, second: usize) {
        self.random.swap(first, second);
        self.x.swap(first, second);
        self.y.swap(first, second);
        self.z.swap(first, second);
        self.style.swap(first, second);
        self.flags.swap(first, second);
    }
}

fn decode_particle_cloud(bytes: &[u8], pool: &mut ParticleShowcasePool) -> Result<usize, String> {
    if bytes.len() < PARTICLE_CLOUD_HEADER_BYTES {
        return Err("particle cloud header is truncated".to_string());
    }
    if &bytes[..8] != PARTICLE_CLOUD_MAGIC {
        return Err("particle cloud magic is invalid".to_string());
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    let stride = u16::from_le_bytes([bytes[10], bytes[11]]) as usize;
    let count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    if version != 1 {
        return Err(format!("particle cloud version {version} is unsupported"));
    }
    if stride != PARTICLE_CLOUD_RECORD_BYTES {
        return Err(format!(
            "particle cloud record stride {stride} is unsupported"
        ));
    }
    if count == 0 || count > pool.active() {
        return Err(format!(
            "particle cloud count {count} exceeds active pool {}",
            pool.active()
        ));
    }
    let expected = PARTICLE_CLOUD_HEADER_BYTES.saturating_add(count.saturating_mul(stride));
    if bytes.len() != expected {
        return Err(format!(
            "particle cloud length {} does not match expected {expected}",
            bytes.len()
        ));
    }
    let mut bounds = [0i16; 6];
    for (index, value) in bounds.iter_mut().enumerate() {
        let offset = 16 + index * 2;
        *value = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    }
    if bounds[0] > bounds[1] || bounds[2] > bounds[3] || bounds[4] > bounds[5] {
        return Err("particle cloud header bounds are invalid".to_string());
    }
    for index in 0..count {
        let offset = PARTICLE_CLOUD_HEADER_BYTES + index * stride;
        let x = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let y = i16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        let z = i16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
        let palette = bytes[offset + 6];
        let flags = bytes[offset + 7];
        if x < bounds[0]
            || x > bounds[1]
            || y < bounds[2]
            || y > bounds[3]
            || z < bounds[4]
            || z > bounds[5]
            || y < 0
            || palette > 7
            || flags & !3 != 0
        {
            return Err(format!("particle cloud record {index} is out of bounds"));
        }
        pool.x[index] = f32::from(x) * (390.0 / 32_767.0);
        pool.y[index] = 220.0 - f32::from(y) * (440.0 / 32_767.0);
        pool.z[index] = f32::from(z) * (390.0 / 32_767.0);
        pool.style[index] = palette;
        pool.flags[index] = flags;
        pool.life[index] = unit01(pool.random[index].rotate_left(17));
    }
    Ok(count)
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

fn unit_signed(value: u32) -> f32 {
    ((value >> 8) as f32) * (2.0 / 16_777_215.0) - 1.0
}

fn unit01(value: u32) -> f32 {
    ((value >> 8) as f32) * (1.0 / 16_777_215.0)
}

fn ease_out_cubic(value: f32) -> f32 {
    1.0 - (1.0 - value).powi(3)
}

fn triangle_wave(value: f32) -> f32 {
    let phase = value.rem_euclid(2.0);
    if phase < 1.0 {
        phase.mul_add(2.0, -1.0)
    } else {
        3.0 - phase * 2.0
    }
}

fn arcade_camera(seconds: f32) -> (f32, f32, f32, f32, f32) {
    let formation = ease_out_cubic((seconds * 0.25).clamp(0.0, 1.0));
    if seconds < 4.0 {
        return (formation, -0.62, -0.08, 760.0, 0.0);
    }
    if seconds < 24.0 {
        let phase = (seconds - 4.0) / 20.0;
        let orbit = phase * std::f32::consts::TAU;
        return (
            1.0,
            -0.62 + orbit,
            triangle_wave(phase * 2.0) * 0.13,
            720.0 + triangle_wave(phase + 0.25) * 82.0,
            0.0,
        );
    }
    if seconds < 29.0 {
        let return_t = ease_out_cubic((seconds - 24.0) * 0.2);
        return (
            1.0,
            (-0.62 + std::f32::consts::TAU) * (1.0 - return_t) + 0.72 * return_t,
            0.11 * return_t,
            720.0 + 35.0 * return_t,
            0.0,
        );
    }
    (1.0, 0.72, 0.11, 755.0, (seconds - 29.0).clamp(0.0, 1.0))
}

fn warp_travel_and_speed(seconds: f32) -> (f32, f32) {
    let cycle = seconds.rem_euclid(30.0);
    let (distance, speed) = if cycle < 7.0 {
        (cycle * 0.03, 0.03)
    } else if cycle < 14.0 {
        let time = cycle - 7.0;
        let acceleration = 0.87 / 7.0;
        (
            0.21 + 0.03 * time + 0.5 * acceleration * time * time,
            0.03 + acceleration * time,
        )
    } else if cycle < 23.0 {
        (3.465 + (cycle - 14.0) * 0.9, 0.9)
    } else {
        let time = cycle - 23.0;
        let deceleration = 0.87 / 7.0;
        (
            11.565 + 0.9 * time - 0.5 * deceleration * time * time,
            0.9 - deceleration * time,
        )
    };
    (distance.rem_euclid(1.0), speed)
}

fn showcase_palette(demo: ParticleDemoKind) -> &'static [Rgb565Pixel; 8] {
    match demo {
        ParticleDemoKind::Fireworks => &FIREWORKS_PALETTE,
        ParticleDemoKind::FireEmbers => &FIRE_PALETTE,
        ParticleDemoKind::SpiralGalaxy => &GALAXY_PALETTE,
        ParticleDemoKind::WarpSpeed => &WARP_PALETTE,
        ParticleDemoKind::MeteorShower => &METEOR_PALETTE,
        ParticleDemoKind::Weather => &WEATHER_PALETTE,
        ParticleDemoKind::ParticlePortal => &PORTAL_PALETTE,
        ParticleDemoKind::ElectricStorm => &ELECTRIC_PALETTE,
        ParticleDemoKind::FountainWaterfall => &WATER_PALETTE,
        ParticleDemoKind::ArcadeCabinet => &ARCADE_PALETTE,
    }
}

fn project_world(
    x: f32,
    y: f32,
    z: f32,
    width: usize,
    height: usize,
    camera_z: f32,
) -> Option<(f32, f32)> {
    let depth = camera_z + z;
    if depth <= 32.0 {
        return None;
    }
    let scale = camera_z / depth;
    let screen_x = width as f32 * 0.5 + x * scale;
    let screen_y = height as f32 * 0.5 + y * scale;
    (screen_x >= 0.0 && screen_y >= 0.0 && screen_x < width as f32 && screen_y < height as f32)
        .then_some((screen_x, screen_y))
}

fn push_screen_command(
    commands: &mut Vec<u32>,
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    style: u8,
    neighbor: bool,
) -> bool {
    if x < 0.0 || y < 0.0 || x >= width as f32 || y >= height as f32 {
        commands.push(u32::MAX);
        return false;
    }
    let x = x as usize;
    let offset = y as usize * width + x;
    let neighbor = u32::from(neighbor && x + 1 < width) * COMMAND_NEIGHBOR;
    commands.push(offset as u32 | u32::from(style.min(7)) << COMMAND_STYLE_SHIFT | neighbor);
    true
}

fn hidden_slot_offset(hidden_slot: u8) -> Result<usize, String> {
    match hidden_slot {
        1 => Ok(0),
        2 => Ok(1),
        _ => Err(format!(
            "particle showcase hidden slot must be 1 or 2, received {hidden_slot}"
        )),
    }
}

fn pixel_to_rgb565(pixel: Pixel) -> Rgb565Pixel {
    let red = (pixel.0 >> 16) & 0xff;
    let green = (pixel.0 >> 8) & 0xff;
    let blue = pixel.0 & 0xff;
    Rgb565Pixel(((red >> 3) << 11 | (green >> 2) << 5 | (blue >> 3)) as u16)
}

fn raster_bounded_segment(
    destination: &mut [Rgb565Pixel],
    dirty_offsets: &mut Vec<u32>,
    width: usize,
    height: usize,
    segment: ParticleShowcaseSegment,
    palette: &[Rgb565Pixel; 8],
) -> usize {
    let mut x = i32::from(segment.x0);
    let mut y = i32::from(segment.y0);
    let end_x = i32::from(segment.x1);
    let end_y = i32::from(segment.y1);
    let dx = (end_x - x).abs();
    let sx = if x < end_x { 1 } else { -1 };
    let dy = -(end_y - y).abs();
    let sy = if y < end_y { 1 } else { -1 };
    let mut error = dx + dy;
    let mut writes = 0usize;
    for _ in 0..MAX_SEGMENT_PIXELS {
        if x >= 0 && y >= 0 && x < width as i32 && y < height as i32 {
            let offset = y as usize * width + x as usize;
            destination[offset] = palette[usize::from(segment.style.min(7))];
            dirty_offsets.push(offset as u32);
            writes = writes.saturating_add(1);
        }
        if x == end_x && y == end_y {
            break;
        }
        let doubled = error * 2;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
    writes
}

#[cfg(target_os = "linux")]
fn thread_cpu_time_us() -> Option<u64> {
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, time.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let time = unsafe { time.assume_init() };
    Some(
        u64::try_from(time.tv_sec)
            .unwrap_or(0)
            .saturating_mul(1_000_000)
            .saturating_add(u64::try_from(time.tv_nsec).unwrap_or(0) / 1_000),
    )
}

#[cfg(not(target_os = "linux"))]
fn thread_cpu_time_us() -> Option<u64> {
    None
}

fn elapsed_thread_cpu_us(start: Option<u64>) -> u128 {
    start
        .and_then(|start| thread_cpu_time_us().map(|end| end.saturating_sub(start)))
        .map(u128::from)
        .unwrap_or(0)
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

    #[test]
    fn diagnostic_renderer_uses_both_hidden_slots_and_wraps_navigation() {
        let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 7,
            initial_demo: ParticleDemoKind::Fireworks,
        })
        .unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];
        let first = renderer
            .render(&mut destination, 1, Duration::ZERO)
            .unwrap();
        assert_eq!(first.demo, ParticleDemoKind::Fireworks);
        assert!(first.visible > 0);

        request_particle_demo_navigation(-1);
        let wrapped = renderer
            .render(&mut destination, 2, Duration::from_millis(17))
            .unwrap();
        assert_eq!(wrapped.demo, ParticleDemoKind::ArcadeCabinet);
        assert!(
            renderer
                .render(&mut destination, 0, Duration::ZERO)
                .is_err()
        );
    }

    #[test]
    fn bounded_segments_clip_and_never_exceed_the_pixel_limit() {
        let mut destination = vec![Rgb565Pixel(0); 16 * 16];
        let mut dirty = Vec::new();
        let writes = raster_bounded_segment(
            &mut destination,
            &mut dirty,
            16,
            16,
            ParticleShowcaseSegment {
                x0: -4,
                y0: 8,
                x1: 30,
                y1: 8,
                style: 7,
            },
            &FIREWORKS_PALETTE,
        );
        assert!(writes <= MAX_SEGMENT_PIXELS as usize);
        assert!(dirty.iter().all(|offset| *offset < 16 * 16));
    }

    #[test]
    fn fire_heat_and_embers_are_deterministic_and_visible() {
        let config = ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0xf1ae,
            initial_demo: ParticleDemoKind::FireEmbers,
        };
        let mut first = ParticleShowcaseRenderer::new(config).unwrap();
        let mut second = ParticleShowcaseRenderer::new(config).unwrap();
        let mut first_destination = vec![Rgb565Pixel(0); 960 * 540];
        let mut second_destination = vec![Rgb565Pixel(0); 960 * 540];
        let elapsed = Duration::from_millis(1_250);

        let first_stats = first.render(&mut first_destination, 1, elapsed).unwrap();
        let second_stats = second.render(&mut second_destination, 1, elapsed).unwrap();

        assert_eq!(first_stats.demo, ParticleDemoKind::FireEmbers);
        assert_eq!(first_stats.beat, "flame");
        assert!(first_stats.visible > 0);
        assert!(first.commands.len() <= first.pool.active() / 4);
        assert!(first.heat.iter().any(|value| *value > 0));
        assert_eq!(first.heat, second.heat);
        assert_eq!(first_destination, second_destination);
        assert!(
            first_destination[(540 - FIRE_HEAT_H * FIRE_HEAT_SCALE) * 960..]
                .iter()
                .any(|pixel| *pixel != Rgb565Pixel(0))
        );
    }

    #[test]
    fn galaxy_has_four_arms_bulge_depth_and_dust_gaps() {
        let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0x6a1a_9a,
            initial_demo: ParticleDemoKind::SpiralGalaxy,
        })
        .unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];
        let stats = renderer
            .render(&mut destination, 1, Duration::from_secs(15))
            .unwrap();
        let bulge_count = renderer.pool.active() * 3 / 20;

        assert_eq!(stats.demo, ParticleDemoKind::SpiralGalaxy);
        assert_eq!(stats.beat, "arm-pass");
        assert_eq!(renderer.commands.len(), renderer.galaxy_projected_count);
        assert!(renderer.galaxy_projected_count < renderer.pool.active());
        assert!(stats.visible > renderer.pool.active() / 2);
        assert!(
            renderer.pool.style[..bulge_count]
                .iter()
                .all(|style| *style >= 6)
        );
        assert!(
            renderer.pool.y[bulge_count..]
                .iter()
                .any(|height| height.abs() > 8.0)
        );
        assert!(
            renderer.pool.flags[renderer.galaxy_projected_count..]
                .iter()
                .any(|flag| *flag == 0)
        );
    }

    #[test]
    fn warp_speed_accelerates_and_emits_bounded_streaks() {
        let (calm_travel, calm_speed) = warp_travel_and_speed(2.0);
        let (_, warp_speed) = warp_travel_and_speed(18.0);
        assert!((0.0..1.0).contains(&calm_travel));
        assert!(warp_speed > calm_speed * 20.0);

        let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0x5eed,
            initial_demo: ParticleDemoKind::WarpSpeed,
        })
        .unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];
        let stats = renderer
            .render(&mut destination, 1, Duration::from_secs(18))
            .unwrap();

        assert_eq!(stats.demo, ParticleDemoKind::WarpSpeed);
        assert_eq!(stats.beat, "warp");
        assert!(stats.visible > renderer.pool.active() / 2);
        assert!(!renderer.segments.is_empty());
        assert!(renderer.segments.len() <= renderer.pool.active().div_ceil(8));
    }

    #[test]
    fn meteor_shower_converges_on_radiant_and_scales_its_peak() {
        let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0x5eed,
            initial_demo: ParticleDemoKind::MeteorShower,
        })
        .unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];
        let stats = renderer
            .render(&mut destination, 1, Duration::from_secs(24))
            .unwrap();
        let radiant = (666_i32, 116_i32);

        assert_eq!(stats.demo, ParticleDemoKind::MeteorShower);
        assert_eq!(stats.beat, "peak");
        assert_eq!(renderer.commands.len(), renderer.pool.active());
        assert!(!renderer.segments.is_empty());
        assert!(renderer.segments.len() <= METEOR_TRACK_COUNT);
        assert!(renderer.segments.iter().all(|segment| {
            let tail_dx = i32::from(segment.x0) - radiant.0;
            let tail_dy = i32::from(segment.y0) - radiant.1;
            let head_dx = i32::from(segment.x1) - radiant.0;
            let head_dy = i32::from(segment.y1) - radiant.1;
            head_dx * head_dx + head_dy * head_dy >= tail_dx * tail_dx + tail_dy * tail_dy
        }));
    }

    #[test]
    fn weather_sections_change_motion_palette_and_geometry_deterministically() {
        let config = ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0xc10d,
            initial_demo: ParticleDemoKind::Weather,
        };
        let mut renderer = ParticleShowcaseRenderer::new(config).unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];

        let rain = renderer
            .render(&mut destination, 1, Duration::from_secs(5))
            .unwrap();
        assert_eq!(rain.beat, "rain");
        assert!(rain.segment_count > 0);
        assert_eq!(renderer.commands.len(), renderer.pool.active() / 4);

        let snow = renderer
            .render(&mut destination, 2, Duration::from_secs(15))
            .unwrap();
        assert_eq!(snow.beat, "snow");
        assert_eq!(snow.segment_count, 0);
        assert!(snow.visible > renderer.pool.active() / 16);

        let ash = renderer
            .render(&mut destination, 1, Duration::from_secs(25))
            .unwrap();
        assert_eq!(ash.beat, "ash");
        assert!(ash.segment_count > 0);
        assert!(ash.clipped_commands < renderer.pool.active() / 2);
        assert_eq!(triangle_wave(0.5), 0.0);
        assert_eq!(triangle_wave(1.5), 0.0);
    }

    #[test]
    fn portal_preserves_toroidal_volume_and_bounded_tendrils() {
        let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0x9077_a1,
            initial_demo: ParticleDemoKind::ParticlePortal,
        })
        .unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];
        let stats = renderer
            .render(&mut destination, 1, Duration::from_secs(23))
            .unwrap();
        let radial_min = renderer.pool.x[0]
            .mul_add(renderer.pool.x[0], renderer.pool.y[0] * renderer.pool.y[0])
            .sqrt();

        assert_eq!(stats.demo, ParticleDemoKind::ParticlePortal);
        assert_eq!(stats.beat, "pulse");
        assert!((90.0..230.0).contains(&radial_min));
        assert_eq!(renderer.commands.len(), renderer.pool.active() / 8);
        assert!(stats.visible > renderer.pool.active() / 16);
        assert!(stats.segment_count > 0);
        assert!(stats.segment_count <= renderer.pool.active() / 128);
    }

    #[test]
    fn electric_storm_bounds_branching_and_layers_return_stroke() {
        let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0xb017,
            initial_demo: ParticleDemoKind::ElectricStorm,
        })
        .unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];
        let stats = renderer
            .render(&mut destination, 1, Duration::from_secs(25))
            .unwrap();

        assert_eq!(stats.demo, ParticleDemoKind::ElectricStorm);
        assert_eq!(stats.beat, "branches");
        assert_eq!(renderer.commands.len(), renderer.pool.active() / 2);
        assert!((240..=360).contains(&stats.segment_count));
        assert!(renderer.segments.iter().any(|segment| segment.style == 7));
        assert!(
            renderer
                .segments
                .iter()
                .all(|segment| i32::from(segment.y1) - i32::from(segment.y0) <= MAX_SEGMENT_PIXELS)
        );
    }

    #[test]
    fn fountain_morphs_into_bounded_waterfall_with_impact_layers() {
        let config = ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0x57a7_e2,
            initial_demo: ParticleDemoKind::FountainWaterfall,
        };
        let mut fountain = ParticleShowcaseRenderer::new(config).unwrap();
        let mut waterfall = ParticleShowcaseRenderer::new(config).unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];

        let fountain_stats = fountain
            .render(&mut destination, 1, Duration::from_secs(6))
            .unwrap();
        let waterfall_stats = waterfall
            .render(&mut destination, 1, Duration::from_secs(26))
            .unwrap();

        assert_eq!(fountain_stats.beat, "fountain");
        assert_eq!(waterfall_stats.beat, "impact");
        assert_eq!(fountain.commands.len(), fountain.pool.active() / 4);
        assert_eq!(waterfall.commands.len(), waterfall.pool.active() / 4);
        assert!(fountain_stats.segment_count >= 120);
        assert!(waterfall_stats.segment_count >= 64);
        assert!(fountain_stats.visible > fountain.pool.active() / 8);
        assert!(waterfall_stats.visible > waterfall.pool.active() / 8);
        assert!(waterfall.segments.iter().all(|segment| {
            (i32::from(segment.y1) - i32::from(segment.y0)).abs() <= MAX_SEGMENT_PIXELS
        }));
    }

    #[test]
    fn arcade_cloud_validates_and_forms_feature_aware_cabinet() {
        let config = ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0x1983,
            initial_demo: ParticleDemoKind::ArcadeCabinet,
        };
        let mut renderer = ParticleShowcaseRenderer::new(config).unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];
        let formation = renderer
            .render(&mut destination, 1, Duration::from_secs(2))
            .unwrap();
        let orbit = renderer
            .render(&mut destination, 2, Duration::from_secs(16))
            .unwrap();

        assert_eq!(formation.beat, "formation");
        assert_eq!(orbit.beat, "fly-around");
        assert_eq!(renderer.commands.len(), 20_480);
        assert!(orbit.visible > 18_000);
        assert!(
            renderer.pool.flags[..renderer.pool.active()]
                .iter()
                .any(|flags| flags & 1 != 0)
        );
        assert!(
            renderer.pool.flags[..renderer.pool.active()]
                .iter()
                .any(|flags| flags & 2 != 0)
        );
    }

    #[test]
    fn arcade_cloud_rejects_malformed_headers_and_records() {
        let mut pool = ParticleShowcasePool::new();
        pool.reset(ParticleDemoKind::ArcadeCabinet, 1);
        assert!(decode_particle_cloud(b"short", &mut pool).is_err());

        let mut bad_version = ARCADE_CLOUD.to_vec();
        bad_version[8] = 2;
        assert!(decode_particle_cloud(&bad_version, &mut pool).is_err());

        let mut bad_palette = ARCADE_CLOUD.to_vec();
        bad_palette[PARTICLE_CLOUD_HEADER_BYTES + 6] = 8;
        assert!(decode_particle_cloud(&bad_palette, &mut pool).is_err());
    }
}
