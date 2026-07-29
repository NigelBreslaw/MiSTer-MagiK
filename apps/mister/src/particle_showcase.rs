// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared data model for the interactive ARM particle showcase.

use crate::bitmap_text::{ConsoleFont, ConsoleTypeface};
use crate::fireworks::{FireworkRenderer, embedded_firework_json};
use crate::fireworks_v2::{FireworkV2Renderer, embedded_firework_v2_json};
use crate::framebuffer::mapped::Pixel;
use crate::particle_material::{
    MaterialRasterStats, MaterialShape, MaterialStamp, MaterialStroke, raster_stamp,
    raster_tapered_segment,
};
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
const MATERIAL_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x1000),
    Rgb565Pixel(0x3800),
    Rgb565Pixel(0x7800),
    Rgb565Pixel(0xb900),
    Rgb565Pixel(0xfac0),
    Rgb565Pixel(0xfe20),
    Rgb565Pixel(0xff75),
    Rgb565Pixel(0xffff),
];
const RIBBON_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x1008),
    Rgb565Pixel(0x2012),
    Rgb565Pixel(0x381f),
    Rgb565Pixel(0x801f),
    Rgb565Pixel(0xf81f),
    Rgb565Pixel(0x05ff),
    Rgb565Pixel(0x8fff),
    Rgb565Pixel(0xff75),
];
const FLOW_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0808),
    Rgb565Pixel(0x1012),
    Rgb565Pixel(0x201f),
    Rgb565Pixel(0x03ff),
    Rgb565Pixel(0x67ff),
    Rgb565Pixel(0xafea),
    Rgb565Pixel(0xffa0),
    Rgb565Pixel(0xffff),
];
const DENSITY_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0000),
    Rgb565Pixel(0x100b),
    Rgb565Pixel(0x3015),
    Rgb565Pixel(0x681f),
    Rgb565Pixel(0xd81f),
    Rgb565Pixel(0xfa9f),
    Rgb565Pixel(0xff59),
    Rgb565Pixel(0xffff),
];
const CHILD_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x1808),
    Rgb565Pixel(0x400f),
    Rgb565Pixel(0x8014),
    Rgb565Pixel(0xf81f),
    Rgb565Pixel(0x029f),
    Rgb565Pixel(0x05ff),
    Rgb565Pixel(0x9ff5),
    Rgb565Pixel(0xffff),
];
const FIELD_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0808),
    Rgb565Pixel(0x1014),
    Rgb565Pixel(0x301f),
    Rgb565Pixel(0x801f),
    Rgb565Pixel(0x05ff),
    Rgb565Pixel(0x77ea),
    Rgb565Pixel(0xfec0),
    Rgb565Pixel(0xffff),
];
const DEPTH_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0808),
    Rgb565Pixel(0x1010),
    Rgb565Pixel(0x18d4),
    Rgb565Pixel(0x2b5f),
    Rgb565Pixel(0x67ff),
    Rgb565Pixel(0xafff),
    Rgb565Pixel(0xff59),
    Rgb565Pixel(0xffff),
];
const MORPH_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x1010),
    Rgb565Pixel(0x201f),
    Rgb565Pixel(0x03ff),
    Rgb565Pixel(0x07e0),
    Rgb565Pixel(0xf800),
    Rgb565Pixel(0xfd20),
    Rgb565Pixel(0xffdf),
    Rgb565Pixel(0xffff),
];
const COLLISION_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0808),
    Rgb565Pixel(0x1014),
    Rgb565Pixel(0x029f),
    Rgb565Pixel(0x05ff),
    Rgb565Pixel(0x7800),
    Rgb565Pixel(0xf980),
    Rgb565Pixel(0xff75),
    Rgb565Pixel(0xffff),
];
const FLOCK_PALETTE: [Rgb565Pixel; 8] = [
    Rgb565Pixel(0x0808),
    Rgb565Pixel(0x1014),
    Rgb565Pixel(0x201f),
    Rgb565Pixel(0x501f),
    Rgb565Pixel(0x05ff),
    Rgb565Pixel(0x9fff),
    Rgb565Pixel(0xfe80),
    Rgb565Pixel(0xffff),
];
const FLOCK_GRID_W: usize = 60;
const FLOCK_GRID_H: usize = 34;
const FLOCK_CELL_PX: f32 = 16.0;
const DENSITY_W: usize = 240;
const DENSITY_H: usize = 135;
const DENSITY_SCALE: usize = 4;
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
    SolarChrysanthemum,
    RecursiveHalo,
    CopperWillowRain,
    PhoenixComet,
    MagneticFlower,
    OledPeony,
    SolarChrysanthemumV2,
    RecursiveHaloV2,
    CopperWillowRainV2,
    PhoenixCometV2,
    MagneticFlowerV2,
    OledPeonyV2,
    FireEmbers,
    SpiralGalaxy,
    WarpSpeed,
    MeteorShower,
    Weather,
    ParticlePortal,
    ElectricStorm,
    FountainWaterfall,
    ArcadeCabinet,
    ProceduralSpriteMaterials,
    VariableWidthRibbons,
    CurlNoiseFlowField,
    LowResolutionDensityBloom,
    LayeredChildSystems,
    SpatialFieldStack,
    DepthAwareMaterialLod,
    SourceDrivenMorph,
    SdfCollisionEvents,
    GridAcceleratedFlocking,
}

impl ParticleDemoKind {
    pub const ALL: [Self; 31] = [
        Self::SolarChrysanthemum,
        Self::RecursiveHalo,
        Self::CopperWillowRain,
        Self::PhoenixComet,
        Self::MagneticFlower,
        Self::OledPeony,
        Self::SolarChrysanthemumV2,
        Self::RecursiveHaloV2,
        Self::CopperWillowRainV2,
        Self::PhoenixCometV2,
        Self::MagneticFlowerV2,
        Self::OledPeonyV2,
        Self::FireEmbers,
        Self::SpiralGalaxy,
        Self::WarpSpeed,
        Self::MeteorShower,
        Self::Weather,
        Self::ParticlePortal,
        Self::ElectricStorm,
        Self::FountainWaterfall,
        Self::ArcadeCabinet,
        Self::ProceduralSpriteMaterials,
        Self::VariableWidthRibbons,
        Self::CurlNoiseFlowField,
        Self::LowResolutionDensityBloom,
        Self::LayeredChildSystems,
        Self::SpatialFieldStack,
        Self::DepthAwareMaterialLod,
        Self::SourceDrivenMorph,
        Self::SdfCollisionEvents,
        Self::GridAcceleratedFlocking,
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
            Self::SolarChrysanthemum => "SOLAR CHRYSANTHEMUM",
            Self::RecursiveHalo => "RECURSIVE HALO",
            Self::CopperWillowRain => "COPPER WILLOW RAIN",
            Self::PhoenixComet => "PHOENIX COMET",
            Self::MagneticFlower => "MAGNETIC FLOWER",
            Self::OledPeony => "OLED PEONY",
            Self::SolarChrysanthemumV2 => "SOLAR CHRYSANTHEMUM V2",
            Self::RecursiveHaloV2 => "RECURSIVE HALO V2",
            Self::CopperWillowRainV2 => "COPPER WILLOW RAIN V2",
            Self::PhoenixCometV2 => "PHOENIX COMET V2",
            Self::MagneticFlowerV2 => "MAGNETIC FLOWER V2",
            Self::OledPeonyV2 => "OLED PEONY V2",
            Self::FireEmbers => "FIRE + EMBERS",
            Self::SpiralGalaxy => "SPIRAL GALAXY",
            Self::WarpSpeed => "WARP SPEED",
            Self::MeteorShower => "METEOR SHOWER",
            Self::Weather => "WEATHER",
            Self::ParticlePortal => "PARTICLE PORTAL",
            Self::ElectricStorm => "ELECTRIC STORM",
            Self::FountainWaterfall => "FOUNTAIN / WATERFALL",
            Self::ArcadeCabinet => "ARCADE CABINET",
            Self::ProceduralSpriteMaterials => "PROCEDURAL SPRITE MATERIALS",
            Self::VariableWidthRibbons => "VARIABLE-WIDTH RIBBONS",
            Self::CurlNoiseFlowField => "CURL-NOISE FLOW FIELD",
            Self::LowResolutionDensityBloom => "LOW-RES DENSITY + BLOOM",
            Self::LayeredChildSystems => "LAYERED CHILD SYSTEMS",
            Self::SpatialFieldStack => "SPATIAL FIELD STACK",
            Self::DepthAwareMaterialLod => "DEPTH-AWARE MATERIAL LOD",
            Self::SourceDrivenMorph => "SOURCE-DRIVEN MORPH",
            Self::SdfCollisionEvents => "SDF COLLISION EVENTS",
            Self::GridAcceleratedFlocking => "GRID-ACCELERATED FLOCKING",
        }
    }

    #[must_use]
    pub const fn telemetry_label(self) -> &'static str {
        match self {
            Self::SolarChrysanthemum => "solar-chrysanthemum",
            Self::RecursiveHalo => "recursive-halo",
            Self::CopperWillowRain => "copper-willow-rain",
            Self::PhoenixComet => "phoenix-comet",
            Self::MagneticFlower => "magnetic-flower",
            Self::OledPeony => "oled-peony",
            Self::SolarChrysanthemumV2 => "solar-chrysanthemum-v2",
            Self::RecursiveHaloV2 => "recursive-halo-v2",
            Self::CopperWillowRainV2 => "copper-willow-rain-v2",
            Self::PhoenixCometV2 => "phoenix-comet-v2",
            Self::MagneticFlowerV2 => "magnetic-flower-v2",
            Self::OledPeonyV2 => "oled-peony-v2",
            Self::FireEmbers => "fire-embers",
            Self::SpiralGalaxy => "spiral-galaxy",
            Self::WarpSpeed => "warp-speed",
            Self::MeteorShower => "meteor-shower",
            Self::Weather => "weather",
            Self::ParticlePortal => "particle-portal",
            Self::ElectricStorm => "electric-storm",
            Self::FountainWaterfall => "fountain-waterfall",
            Self::ArcadeCabinet => "arcade-cabinet",
            Self::ProceduralSpriteMaterials => "procedural-sprite-materials",
            Self::VariableWidthRibbons => "variable-width-ribbons",
            Self::CurlNoiseFlowField => "curl-noise-flow-field",
            Self::LowResolutionDensityBloom => "density-bloom",
            Self::LayeredChildSystems => "layered-child-systems",
            Self::SpatialFieldStack => "spatial-field-stack",
            Self::DepthAwareMaterialLod => "depth-aware-material-lod",
            Self::SourceDrivenMorph => "source-morph",
            Self::SdfCollisionEvents => "sdf-collision",
            Self::GridAcceleratedFlocking => "grid-flocking",
        }
    }

    #[must_use]
    pub fn hud_label(self) -> String {
        format!(
            "{:02}/{:02} {}",
            self.number(),
            Self::ALL.len(),
            self.label()
        )
    }

    #[must_use]
    pub const fn firework_id(self) -> Option<&'static str> {
        match self {
            Self::SolarChrysanthemum => Some("solar-chrysanthemum"),
            Self::RecursiveHalo => Some("recursive-halo"),
            Self::CopperWillowRain => Some("copper-willow-rain"),
            Self::PhoenixComet => Some("phoenix-comet"),
            Self::MagneticFlower => Some("magnetic-flower"),
            Self::OledPeony => Some("oled-peony"),
            _ => None,
        }
    }

    #[must_use]
    pub const fn firework_v2_id(self) -> Option<&'static str> {
        match self {
            Self::SolarChrysanthemumV2 => Some("solar-chrysanthemum-v2"),
            Self::RecursiveHaloV2 => Some("recursive-halo-v2"),
            Self::CopperWillowRainV2 => Some("copper-willow-rain-v2"),
            Self::PhoenixCometV2 => Some("phoenix-comet-v2"),
            Self::MagneticFlowerV2 => Some("magnetic-flower-v2"),
            Self::OledPeonyV2 => Some("oled-peony-v2"),
            _ => None,
        }
    }

    #[must_use]
    pub const fn starting_count(self) -> usize {
        match self {
            Self::SolarChrysanthemum
            | Self::RecursiveHalo
            | Self::CopperWillowRain
            | Self::PhoenixComet
            | Self::MagneticFlower
            | Self::OledPeony
            | Self::SolarChrysanthemumV2
            | Self::RecursiveHaloV2
            | Self::CopperWillowRainV2
            | Self::PhoenixCometV2
            | Self::MagneticFlowerV2
            | Self::OledPeonyV2 => 0,
            Self::FireEmbers | Self::MeteorShower => 20_480,
            Self::SpiralGalaxy => 81_920,
            Self::WarpSpeed => 45_056,
            Self::Weather => 49_152,
            Self::ParticlePortal => 65_536,
            Self::ElectricStorm => 16_384,
            Self::FountainWaterfall => 32_768,
            Self::ArcadeCabinet => 12_288,
            Self::ProceduralSpriteMaterials => 16_384,
            Self::VariableWidthRibbons => 8_192,
            Self::CurlNoiseFlowField => 32_768,
            Self::LowResolutionDensityBloom => 24_576,
            Self::LayeredChildSystems => 4_096,
            Self::SpatialFieldStack => 24_576,
            Self::DepthAwareMaterialLod => 40_960,
            Self::SourceDrivenMorph => 12_288,
            Self::SdfCollisionEvents => 8_192,
            Self::GridAcceleratedFlocking => 12_288,
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
    firework_renderer: Option<ShowcaseFireworkRenderer>,
    firework_capture_time: Option<Duration>,
    hud_visible: bool,
    pool: ParticleShowcasePool,
    commands: Vec<u32>,
    previous_commands: Vec<u32>,
    segments: Vec<ParticleShowcaseSegment>,
    material_stamps: Vec<MaterialStamp>,
    material_strokes: Vec<MaterialStroke>,
    transition: ParticleShowcaseTransition,
    transition_started_at: Option<Duration>,
    heat: Vec<u8>,
    density: Vec<u16>,
    density_blur: Vec<u16>,
    flock_counts: Vec<u16>,
    flock_vx: Vec<f32>,
    flock_vy: Vec<f32>,
    flock_last_elapsed: Duration,
    flow_last_elapsed: Duration,
    heat_frame: u64,
    galaxy_projected_count: usize,
    dirty_slots: [ParticleShowcaseDirtySlot; HIDDEN_SLOT_COUNT],
    hud_font: ConsoleFont,
    hud_pixels: Vec<Pixel>,
    renderer_scratch_bytes: usize,
}

enum ShowcaseFireworkRenderer {
    V1(FireworkRenderer),
    V2(FireworkV2Renderer),
}

impl ShowcaseFireworkRenderer {
    fn duration(&self) -> Duration {
        match self {
            Self::V1(renderer) => renderer.duration(),
            Self::V2(renderer) => renderer.duration(),
        }
    }

    fn render(
        &self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<(usize, usize, usize), String> {
        match self {
            Self::V1(renderer) => renderer
                .render(destination, elapsed)
                .map(|stats| (stats.particles, stats.visible, stats.pixel_writes)),
            Self::V2(renderer) => renderer
                .render(destination, elapsed)
                .map(|stats| (stats.particles, stats.visible, stats.pixel_writes)),
        }
    }
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
        let material_stamps = Vec::with_capacity(16_384);
        let material_strokes = Vec::with_capacity(2_048);
        let transition = ParticleShowcaseTransition::new();
        let heat = vec![0; FIRE_HEAT_W * FIRE_HEAT_H];
        let density = vec![0; DENSITY_W * DENSITY_H];
        let density_blur = vec![0; DENSITY_W * DENSITY_H];
        let flock_counts = vec![0; FLOCK_GRID_W * FLOCK_GRID_H];
        let flock_vx = vec![0.0; FLOCK_GRID_W * FLOCK_GRID_H];
        let flock_vy = vec![0.0; FLOCK_GRID_W * FLOCK_GRID_H];
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
                &kind.hud_label(),
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
            .saturating_add(
                material_stamps
                    .capacity()
                    .saturating_mul(std::mem::size_of::<MaterialStamp>()),
            )
            .saturating_add(
                material_strokes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<MaterialStroke>()),
            )
            .saturating_add(transition.allocated_bytes());
        let renderer_scratch_bytes = renderer_scratch_bytes.saturating_add(heat.capacity());
        let renderer_scratch_bytes = renderer_scratch_bytes.saturating_add(
            density
                .capacity()
                .saturating_mul(std::mem::size_of::<u16>()),
        );
        let renderer_scratch_bytes = renderer_scratch_bytes.saturating_add(
            density_blur
                .capacity()
                .saturating_mul(std::mem::size_of::<u16>()),
        );
        let renderer_scratch_bytes = renderer_scratch_bytes
            .saturating_add(
                flock_counts
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u16>()),
            )
            .saturating_add(
                (flock_vx.capacity() + flock_vy.capacity())
                    .saturating_mul(std::mem::size_of::<f32>()),
            );
        let mut renderer = Self {
            config,
            demo: config.initial_demo,
            demo_started_at: Duration::ZERO,
            firework_renderer: None,
            firework_capture_time: None,
            hud_visible: true,
            pool,
            commands,
            previous_commands,
            segments,
            material_stamps,
            material_strokes,
            transition,
            transition_started_at: None,
            heat,
            density,
            density_blur,
            flock_counts,
            flock_vx,
            flock_vy,
            flock_last_elapsed: Duration::ZERO,
            flow_last_elapsed: Duration::ZERO,
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

        if self.firework_renderer.is_some() {
            return self.render_firework(
                destination,
                slot,
                elapsed,
                dirty_offsets,
                clear_us,
                clear_cpu_us,
            );
        }

        let simulation_started = Instant::now();
        let simulation_cpu_started = thread_cpu_time_us();
        self.update_effect(elapsed);
        let simulation_us = simulation_started.elapsed().as_micros();
        let simulation_cpu_us = elapsed_thread_cpu_us(simulation_cpu_started);

        let projection_started = Instant::now();
        let projection_cpu_started = thread_cpu_time_us();
        self.material_stamps.clear();
        self.material_strokes.clear();
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
        let material = self.raster_materials(destination, &mut dirty_offsets);
        attempted_pixel_writes =
            attempted_pixel_writes.saturating_add(material.attempted_pixel_writes);
        let strokes = self.raster_material_strokes(destination, &mut dirty_offsets);
        attempted_pixel_writes =
            attempted_pixel_writes.saturating_add(strokes.attempted_pixel_writes);
        attempted_pixel_writes = attempted_pixel_writes
            .saturating_add(self.raster_segments(destination, &mut dirty_offsets));
        if self.hud_visible {
            self.draw_hud(destination, &mut dirty_offsets);
        }
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

    pub fn configure_firework_capture(
        &mut self,
        capture_time: Option<Duration>,
        hud_visible: bool,
    ) {
        self.firework_capture_time = capture_time;
        self.hud_visible = hud_visible;
    }

    pub fn configure_capture_hud(&mut self, hud_visible: bool) {
        self.hud_visible = hud_visible;
    }

    #[allow(clippy::too_many_arguments)]
    fn render_firework(
        &mut self,
        destination: &mut [Rgb565Pixel],
        slot: usize,
        elapsed: Duration,
        mut dirty_offsets: Vec<u32>,
        clear_us: u128,
        clear_cpu_us: u128,
    ) -> Result<ParticleShowcaseRenderStats, String> {
        let demo_elapsed = elapsed.saturating_sub(self.demo_started_at);
        let renderer = self
            .firework_renderer
            .as_ref()
            .expect("firework renderer checked before dispatch");
        let duration_ms = renderer.duration().as_millis().max(1);
        let logical_ms = demo_elapsed.as_millis() % duration_ms;
        let logical_elapsed = self
            .firework_capture_time
            .unwrap_or_else(|| Duration::from_millis(logical_ms as u64));
        let raster_started = Instant::now();
        let raster_cpu_started = thread_cpu_time_us();
        let (particles, visible, pixel_writes) = renderer.render(destination, logical_elapsed)?;
        if self.hud_visible {
            self.draw_hud(destination, &mut dirty_offsets);
        }
        let raster_us = raster_started.elapsed().as_micros();
        let raster_cpu_us = elapsed_thread_cpu_us(raster_cpu_started);
        self.dirty_slots[slot].offsets = dirty_offsets;
        Ok(ParticleShowcaseRenderStats {
            demo: self.demo,
            beat: firework_beat(logical_elapsed),
            count: particles,
            visible,
            simulation_us: 0,
            simulation_cpu_us: 0,
            projection_us: 0,
            projection_cpu_us: 0,
            geometry_us: 0,
            clear_us,
            clear_cpu_us,
            raster_us,
            raster_cpu_us,
            segment_count: 0,
            attempted_pixel_writes: pixel_writes,
            clipped_commands: 0,
            simulation_bytes: self.pool.allocated_bytes(),
            renderer_scratch_bytes: self.renderer_scratch_bytes,
        })
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
        self.firework_renderer = demo
            .firework_id()
            .map(|id| {
                ShowcaseFireworkRenderer::V1(
                    FireworkRenderer::from_json(
                        embedded_firework_json(id).expect("registered firework must be embedded"),
                        self.config.width,
                        self.config.height,
                        self.config.seed,
                    )
                    .expect("embedded V1 firework must satisfy its runtime contract"),
                )
            })
            .or_else(|| {
                demo.firework_v2_id().map(|id| {
                    ShowcaseFireworkRenderer::V2(
                        FireworkV2Renderer::from_json(
                            embedded_firework_v2_json(id)
                                .expect("registered V2 firework must be embedded"),
                            self.config.width,
                            self.config.height,
                            self.config.seed,
                        )
                        .expect("embedded V2 firework must satisfy its runtime contract"),
                    )
                })
            });
        self.pool.reset(demo, self.config.seed);
        self.heat.fill(0);
        self.heat_frame = u64::MAX;
        self.flock_last_elapsed = elapsed;
        self.flow_last_elapsed = elapsed;
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
        } else if demo == ParticleDemoKind::ProceduralSpriteMaterials {
            self.initialize_procedural_sprite_materials();
        } else if demo == ParticleDemoKind::VariableWidthRibbons {
            self.initialize_variable_width_ribbons();
        } else if demo == ParticleDemoKind::CurlNoiseFlowField {
            self.initialize_curl_noise_flow_field();
        } else if demo == ParticleDemoKind::LowResolutionDensityBloom {
            self.initialize_density_bloom();
        } else if demo == ParticleDemoKind::LayeredChildSystems {
            self.initialize_layered_child_systems();
        } else if demo == ParticleDemoKind::SpatialFieldStack {
            self.initialize_spatial_field_stack();
        } else if demo == ParticleDemoKind::DepthAwareMaterialLod {
            self.initialize_depth_aware_material_lod();
        } else if demo == ParticleDemoKind::SourceDrivenMorph {
            self.initialize_source_driven_morph();
        } else if demo == ParticleDemoKind::SdfCollisionEvents {
            self.initialize_sdf_collision_events();
        } else if demo == ParticleDemoKind::GridAcceleratedFlocking {
            self.initialize_grid_flocking();
        }
    }

    fn prepare_hidden_slot(&mut self, destination: &mut [Rgb565Pixel], slot: usize) -> Vec<u32> {
        let force_full_clear = self.demo == ParticleDemoKind::VariableWidthRibbons;
        let dirty = &mut self.dirty_slots[slot];
        if force_full_clear
            || !dirty.initialized
            || dirty.offsets.len() >= destination.len() / FULL_CLEAR_DIRTY_DIVISOR
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
            ParticleDemoKind::SolarChrysanthemum
            | ParticleDemoKind::RecursiveHalo
            | ParticleDemoKind::CopperWillowRain
            | ParticleDemoKind::PhoenixComet
            | ParticleDemoKind::MagneticFlower
            | ParticleDemoKind::OledPeony
            | ParticleDemoKind::SolarChrysanthemumV2
            | ParticleDemoKind::RecursiveHaloV2
            | ParticleDemoKind::CopperWillowRainV2
            | ParticleDemoKind::PhoenixCometV2
            | ParticleDemoKind::MagneticFlowerV2
            | ParticleDemoKind::OledPeonyV2 => {
                self.commands.clear();
                self.segments.clear();
                0
            }
            ParticleDemoKind::FireEmbers => self.project_fire_embers(elapsed),
            ParticleDemoKind::SpiralGalaxy => self.project_spiral_galaxy(elapsed),
            ParticleDemoKind::WarpSpeed => self.project_warp_speed(elapsed),
            ParticleDemoKind::MeteorShower => self.project_meteor_shower(elapsed),
            ParticleDemoKind::Weather => self.project_weather(elapsed),
            ParticleDemoKind::ParticlePortal => self.project_particle_portal(elapsed),
            ParticleDemoKind::ElectricStorm => self.project_electric_storm(elapsed),
            ParticleDemoKind::FountainWaterfall => self.project_fountain_waterfall(elapsed),
            ParticleDemoKind::ArcadeCabinet => self.project_arcade_cabinet(elapsed),
            ParticleDemoKind::ProceduralSpriteMaterials => {
                self.project_procedural_sprite_materials(elapsed)
            }
            ParticleDemoKind::VariableWidthRibbons => self.project_variable_width_ribbons(elapsed),
            ParticleDemoKind::CurlNoiseFlowField => self.project_curl_noise_flow_field(elapsed),
            ParticleDemoKind::LowResolutionDensityBloom => self.project_density_bloom(elapsed),
            ParticleDemoKind::LayeredChildSystems => self.project_layered_child_systems(elapsed),
            ParticleDemoKind::SpatialFieldStack => self.project_spatial_field_stack(elapsed),
            ParticleDemoKind::DepthAwareMaterialLod => {
                self.project_depth_aware_material_lod(elapsed)
            }
            ParticleDemoKind::SourceDrivenMorph => self.project_source_driven_morph(elapsed),
            ParticleDemoKind::SdfCollisionEvents => self.project_sdf_collision_events(elapsed),
            ParticleDemoKind::GridAcceleratedFlocking => self.project_grid_flocking(),
        }
    }

    fn update_effect(&mut self, elapsed: Duration) {
        if self.demo == ParticleDemoKind::FireEmbers {
            self.update_fire_heat(elapsed);
        } else if self.demo == ParticleDemoKind::GridAcceleratedFlocking {
            self.update_grid_flocking(elapsed);
        } else if self.demo == ParticleDemoKind::CurlNoiseFlowField {
            self.update_curl_noise_flow_field(elapsed);
        }
    }

    fn effect_beat(&self, elapsed: Duration) -> &'static str {
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        match self.demo {
            ParticleDemoKind::SolarChrysanthemum
            | ParticleDemoKind::RecursiveHalo
            | ParticleDemoKind::CopperWillowRain
            | ParticleDemoKind::PhoenixComet
            | ParticleDemoKind::MagneticFlower
            | ParticleDemoKind::OledPeony
            | ParticleDemoKind::SolarChrysanthemumV2
            | ParticleDemoKind::RecursiveHaloV2
            | ParticleDemoKind::CopperWillowRainV2
            | ParticleDemoKind::PhoenixCometV2
            | ParticleDemoKind::MagneticFlowerV2
            | ParticleDemoKind::OledPeonyV2 => firework_beat(Duration::from_secs_f32(seconds)),
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
            ParticleDemoKind::ProceduralSpriteMaterials => match seconds {
                value if value < 6.0 => "ignite",
                value if value < 20.0 => "material-bloom",
                _ => "cooling",
            },
            ParticleDemoKind::VariableWidthRibbons => match seconds {
                value if value < 6.0 => "draw",
                value if value < 22.0 => "crossover",
                _ => "breakup",
            },
            ParticleDemoKind::CurlNoiseFlowField => match seconds {
                value if value < 8.0 => "counter-current",
                value if value < 22.0 => "curl-pair",
                _ => "eddy-shift",
            },
            ParticleDemoKind::LowResolutionDensityBloom => match seconds {
                value if value < 8.0 => "splat",
                value if value < 22.0 => "crescent-ridge",
                _ => "cavity-pulse",
            },
            ParticleDemoKind::LayeredChildSystems => match seconds {
                value if value < 7.0 => "parents",
                value if value < 18.0 => "event-rings",
                _ => "terminal-children",
            },
            ParticleDemoKind::SpatialFieldStack => match seconds {
                value if value < 9.0 => "attract-repel",
                value if value < 22.0 => "capture-orbit",
                _ => "release",
            },
            ParticleDemoKind::DepthAwareMaterialLod => match seconds {
                value if value < 8.0 => "far-field",
                value if value < 22.0 => "focal-plane",
                _ => "near-pass",
            },
            ParticleDemoKind::SourceDrivenMorph => match seconds {
                value if value < 6.0 => "joystick-hold",
                value if value < 14.0 => "assignment-morph",
                value if value < 23.0 => "controller-hold",
                _ => "return",
            },
            ParticleDemoKind::SdfCollisionEvents => match seconds {
                value if value < 8.0 => "waterfall-impact",
                value if value < 22.0 => "slide-bounce",
                _ => "splash-mist",
            },
            ParticleDemoKind::GridAcceleratedFlocking => match seconds {
                value if value < 8.0 => "wing-arcs",
                value if value < 22.0 => "split-rejoin",
                _ => "chaser-pass",
            },
        }
    }

    fn initialize_grid_flocking(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let side = if index & 1 == 0 { -1.0 } else { 1.0 };
            self.pool.x[index] = 480.0 + side * (70.0 + unit01(random) * 330.0);
            self.pool.y[index] = 270.0 + unit_signed(random.rotate_left(11)) * 185.0;
            let angle = unit_signed(random.rotate_left(21)) * 0.65
                + if side < 0.0 {
                    0.15
                } else {
                    std::f32::consts::PI - 0.15
                };
            self.pool.vx[index] = angle.cos() * (18.0 + unit01(random.rotate_left(7)) * 34.0);
            self.pool.vy[index] = angle.sin() * (18.0 + unit01(random.rotate_left(17)) * 34.0);
            self.pool.style[index] = 2 + ((random >> 30) as u8).min(3);
            self.pool.flags[index] = u8::from(index & 1023 == 0);
        }
    }

    fn update_grid_flocking(&mut self, elapsed: Duration) {
        let dt = elapsed
            .saturating_sub(self.flock_last_elapsed)
            .as_secs_f32()
            .clamp(0.0, 1.0 / 30.0);
        self.flock_last_elapsed = elapsed;
        if dt <= f32::EPSILON {
            return;
        }
        self.flock_counts.fill(0);
        self.flock_vx.fill(0.0);
        self.flock_vy.fill(0.0);
        for index in 0..self.pool.active() {
            let cell_x =
                (self.pool.x[index] / FLOCK_CELL_PX).clamp(0.0, (FLOCK_GRID_W - 1) as f32) as usize;
            let cell_y =
                (self.pool.y[index] / FLOCK_CELL_PX).clamp(0.0, (FLOCK_GRID_H - 1) as f32) as usize;
            let cell = cell_y * FLOCK_GRID_W + cell_x;
            self.flock_counts[cell] = self.flock_counts[cell].saturating_add(1);
            self.flock_vx[cell] += self.pool.vx[index];
            self.flock_vy[cell] += self.pool.vy[index];
        }
        let cavity_x = 480.0 + (elapsed.as_secs_f32() * 0.21).sin() * 58.0;
        let cavity_y = 270.0;
        let cohort = ((elapsed.as_micros() / 16_667) & 3) as usize;
        for index in 0..self.pool.active() {
            if index & 3 != cohort {
                self.pool.x[index] =
                    (self.pool.x[index] + self.pool.vx[index] * dt + 960.0).rem_euclid(960.0);
                self.pool.y[index] =
                    (self.pool.y[index] + self.pool.vy[index] * dt + 540.0).rem_euclid(540.0);
                continue;
            }
            let cell_x =
                (self.pool.x[index] / FLOCK_CELL_PX).clamp(0.0, (FLOCK_GRID_W - 1) as f32) as usize;
            let cell_y =
                (self.pool.y[index] / FLOCK_CELL_PX).clamp(0.0, (FLOCK_GRID_H - 1) as f32) as usize;
            let mut count = 0u32;
            let mut sum_vx = 0.0;
            let mut sum_vy = 0.0;
            for y in cell_y.saturating_sub(1)..=(cell_y + 1).min(FLOCK_GRID_H - 1) {
                for x in cell_x.saturating_sub(1)..=(cell_x + 1).min(FLOCK_GRID_W - 1) {
                    let cell = y * FLOCK_GRID_W + x;
                    count += u32::from(self.flock_counts[cell]);
                    sum_vx += self.flock_vx[cell];
                    sum_vy += self.flock_vy[cell];
                }
            }
            if count > 0 {
                self.pool.vx[index] += (sum_vx / count as f32 - self.pool.vx[index]) * dt * 2.6;
                self.pool.vy[index] += (sum_vy / count as f32 - self.pool.vy[index]) * dt * 2.6;
            }
            let left = self.flock_counts[cell_y * FLOCK_GRID_W + cell_x.saturating_sub(1)];
            let right =
                self.flock_counts[cell_y * FLOCK_GRID_W + (cell_x + 1).min(FLOCK_GRID_W - 1)];
            let above = self.flock_counts[cell_y.saturating_sub(1) * FLOCK_GRID_W + cell_x];
            let below =
                self.flock_counts[(cell_y + 1).min(FLOCK_GRID_H - 1) * FLOCK_GRID_W + cell_x];
            self.pool.vx[index] += (f32::from(left) - f32::from(right)) * dt * 1.8;
            self.pool.vy[index] += (f32::from(above) - f32::from(below)) * dt * 1.8;
            for chaser in (0..self.pool.active()).step_by(1_024) {
                let chaser_dx = self.pool.x[index] - self.pool.x[chaser];
                let chaser_dy = self.pool.y[index] - self.pool.y[chaser];
                let chaser_distance2 = chaser_dx * chaser_dx + chaser_dy * chaser_dy;
                if chaser_distance2 > 16.0 && chaser_distance2 < 132.0 * 132.0 {
                    let inverse = chaser_distance2.sqrt().recip();
                    self.pool.vx[index] += chaser_dx * inverse * dt * 94.0;
                    self.pool.vy[index] += chaser_dy * inverse * dt * 94.0;
                }
            }
            let dx = self.pool.x[index] - cavity_x;
            let dy = self.pool.y[index] - cavity_y;
            let distance2 = dx * dx + dy * dy;
            if distance2 < 118.0 * 118.0 {
                let inverse = distance2.max(64.0).sqrt().recip();
                self.pool.vx[index] += dx * inverse * dt * 260.0;
                self.pool.vy[index] += dy * inverse * dt * 260.0;
            }
            let side = if index & 1 == 0 { -1.0 } else { 1.0 };
            let target_y = 270.0
                + side
                    * (self.pool.x[index] - 480.0).abs().sqrt()
                    * 8.0
                    * (elapsed.as_secs_f32() * 0.09).cos();
            self.pool.vy[index] += (target_y - self.pool.y[index]) * dt * 0.75;
            let speed = (self.pool.vx[index] * self.pool.vx[index]
                + self.pool.vy[index] * self.pool.vy[index])
                .sqrt()
                .max(1.0);
            let limited = 72.0 / speed.max(72.0);
            self.pool.vx[index] *= limited;
            self.pool.vy[index] *= limited;
            self.pool.x[index] =
                (self.pool.x[index] + self.pool.vx[index] * dt + 960.0).rem_euclid(960.0);
            self.pool.y[index] =
                (self.pool.y[index] + self.pool.vy[index] * dt + 540.0).rem_euclid(540.0);
        }
    }

    fn project_grid_flocking(&mut self) -> usize {
        self.commands.clear();
        self.segments.clear();
        for index in 0..self.pool.active() {
            let chaser = self.pool.flags[index] != 0;
            let style = if chaser { 6 } else { self.pool.style[index] };
            let _ = push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                self.pool.x[index],
                self.pool.y[index],
                style,
                index & 31 == 0,
            );
            if chaser {
                self.material_strokes.push(MaterialStroke {
                    x0: self.pool.x[index] - self.pool.vx[index] * 0.22,
                    y0: self.pool.y[index] - self.pool.vy[index] * 0.22,
                    x1: self.pool.x[index],
                    y1: self.pool.y[index],
                    start_radius: 1,
                    end_radius: 2,
                    intensity: 14,
                    color: FLOCK_PALETTE[6],
                });
            }
        }
        0
    }

    fn initialize_sdf_collision_events(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random) * 96.0;
            self.pool.y[index] = unit_signed(random.rotate_left(11)) * 42.0;
            self.pool.age[index] = unit01(random.rotate_left(21)) * 4.0;
            self.pool.life[index] = 0.72 + unit01(random.rotate_left(7)) * 0.65;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
            self.pool.flags[index] = u8::from(index & 3 == 0);
        }
    }

    fn project_sdf_collision_events(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let mut clipped = 0usize;
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let warm = self.pool.flags[index] != 0;
            let (x, y, collided, style) = if warm {
                let phase = (seconds * 0.18 * self.pool.life[index] + self.pool.age[index]).fract();
                let angle = std::f32::consts::TAU * unit01(random.rotate_left(13));
                let travel = 190.0 - phase * 165.0;
                let mut local_x = 172.0 + angle.cos() * travel;
                let mut local_y = 4.0 + angle.sin() * travel;
                let dx = local_x - 172.0;
                let dy = local_y - 4.0;
                let distance = (dx * dx + dy * dy).sqrt().max(0.001);
                let collided = distance < 88.0;
                if collided {
                    let bounce = 88.0 + (phase * 19.0).sin().abs() * 22.0;
                    local_x = 172.0 + dx / distance * bounce;
                    local_y = 4.0 + dy / distance * bounce;
                }
                (local_x, local_y, collided, 4 + usize::from(collided) * 2)
            } else {
                let phase =
                    (seconds * 0.28 * self.pool.life[index] + self.pool.age[index]).rem_euclid(3.2);
                let mut local_x = -176.0 + self.pool.x[index] * 0.48;
                let mut local_y = -252.0 + phase * 172.0 + 42.0 * phase * phase;
                let bowl_y = 116.0 + (local_x + 176.0).powi(2) * 0.0026;
                let collided = local_y > bowl_y;
                if collided {
                    let slide = (phase - 1.55).max(0.0);
                    local_x += self.pool.x[index].signum() * slide * 92.0;
                    local_y = 116.0 + (local_x + 176.0).powi(2) * 0.0026
                        - (slide * 8.0).sin().abs() * 16.0;
                }
                (local_x, local_y, collided, 2 + usize::from(collided))
            };
            let screen_x = 480.0 + x;
            let screen_y = 270.0 + y;
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                screen_x,
                screen_y,
                style as u8,
                collided && index & 31 == 0,
            ) {
                clipped = clipped.saturating_add(1);
            }
            if collided && index & 63 == 0 {
                self.material_stamps.push(MaterialStamp {
                    x: screen_x as i16,
                    y: screen_y as i16,
                    radius: 2 + u8::from(warm),
                    intensity: 11,
                    color: COLLISION_PALETTE[style],
                    shape: if warm {
                        MaterialShape::Spark
                    } else {
                        MaterialShape::Smoke
                    },
                });
            }
        }
        clipped
    }

    fn initialize_source_driven_morph(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let u = unit_signed(random);
            let v = unit_signed(random.rotate_left(11));
            let group = index % 8;
            let (source_x, source_y) = if group < 5 {
                (u * 180.0, 78.0 + v * 72.0)
            } else if group < 7 {
                (u * 34.0, -12.0 + v * 110.0)
            } else {
                let angle = std::f32::consts::TAU * unit01(random.rotate_left(21));
                (angle.cos() * 58.0, -128.0 + angle.sin() * 42.0)
            };
            let (target_x, target_y) = if group < 5 {
                let angle = std::f32::consts::TAU * unit01(random.rotate_left(7));
                let radius = 118.0 + unit_signed(random.rotate_left(17)) * 46.0;
                (angle.cos() * radius, angle.sin() * radius * 0.55)
            } else if group < 7 {
                (u * 205.0, 42.0 + v.abs() * 112.0 + u.abs() * 48.0)
            } else {
                let button = (index / 8) % 4;
                (
                    92.0 + (button & 1) as f32 * 46.0,
                    -24.0 + (button >> 1) as f32 * 42.0,
                )
            };
            self.pool.previous_x[index] = source_x;
            self.pool.previous_y[index] = source_y;
            self.pool.x[index] = target_x;
            self.pool.y[index] = target_y;
            self.pool.age[index] = unit01(random.rotate_left(27));
            self.pool.style[index] = if group == 7 {
                7
            } else {
                ((random >> 29) & 7) as u8
            };
            self.pool.flags[index] = u8::from(index & 127 == 0);
        }
    }

    fn project_source_driven_morph(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let blend = if seconds < 6.0 {
            0.0
        } else if seconds < 14.0 {
            ease_out_cubic((seconds - 6.0) / 8.0)
        } else if seconds < 23.0 {
            1.0
        } else {
            1.0 - ease_out_cubic((seconds - 23.0) / 7.0)
        };
        let arc = (blend * std::f32::consts::PI).sin();
        let mut clipped = 0usize;
        for index in (0..self.pool.active()).step_by(2) {
            let source_x = self.pool.previous_x[index];
            let source_y = self.pool.previous_y[index];
            let target_x = self.pool.x[index];
            let target_y = self.pool.y[index];
            let x = 480.0 + source_x + (target_x - source_x) * blend;
            let y = 270.0 + source_y + (target_y - source_y) * blend
                - arc * (18.0 + self.pool.age[index] * 54.0);
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                y,
                self.pool.style[index],
                index & 31 == 0,
            ) {
                clipped = clipped.saturating_add(1);
            }
            if self.pool.flags[index] != 0 && blend > 0.04 && blend < 0.96 {
                self.material_strokes.push(MaterialStroke {
                    x0: x,
                    y0: y,
                    x1: 480.0 + target_x,
                    y1: 270.0 + target_y,
                    start_radius: 1,
                    end_radius: 1,
                    intensity: 6,
                    color: MORPH_PALETTE[usize::from(self.pool.style[index])],
                });
            }
        }
        clipped
    }

    fn initialize_depth_aware_material_lod(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random) * 520.0;
            self.pool.y[index] = unit_signed(random.rotate_left(11)) * 310.0;
            self.pool.z[index] = unit01(random.rotate_left(21));
            self.pool.age[index] = unit01(random.rotate_left(7));
            self.pool.life[index] = 0.6 + unit01(random.rotate_left(17)) * 0.8;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
        }
    }

    fn project_depth_aware_material_lod(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let mut clipped = 0usize;
        for index in (0..self.pool.active()).step_by(2) {
            let depth =
                (self.pool.z[index] + seconds * self.pool.life[index] * 0.025).rem_euclid(1.0);
            let parallax = 0.35 + depth * 1.05;
            let drift = seconds * (12.0 + depth * 68.0) * self.pool.life[index];
            let x = (480.0 + self.pool.x[index] * parallax + drift + 960.0).rem_euclid(960.0);
            let corridor = 34.0 + (1.0 - depth) * 42.0;
            let mut y = 270.0 + self.pool.y[index] * parallax;
            if y > 270.0 - corridor && y < 270.0 + corridor {
                y += if self.pool.y[index].is_sign_negative() {
                    -corridor
                } else {
                    corridor
                };
            }
            let style = if depth < 0.33 {
                1 + usize::from(self.pool.style[index] & 1)
            } else if depth < 0.72 {
                3 + usize::from(self.pool.style[index] & 1)
            } else {
                5 + usize::from(self.pool.style[index] & 1)
            };
            if depth > 0.72 && index & 31 == 0 {
                self.material_stamps.push(MaterialStamp {
                    x: x as i16,
                    y: y as i16,
                    radius: 3 + u8::from(depth > 0.9),
                    intensity: 8 + (depth * 7.0) as u8,
                    color: DEPTH_PALETTE[style],
                    shape: MaterialShape::Disc,
                });
                if index & 31 == 0 {
                    self.material_strokes.push(MaterialStroke {
                        x0: x - 10.0 - depth * 18.0,
                        y0: y,
                        x1: x,
                        y1: y,
                        start_radius: 1,
                        end_radius: 2,
                        intensity: 11,
                        color: DEPTH_PALETTE[style],
                    });
                }
            } else if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                y,
                style as u8,
                depth >= 0.33,
            ) {
                clipped = clipped.saturating_add(1);
            }
        }
        clipped
    }

    fn initialize_spatial_field_stack(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random) * 148.0;
            self.pool.y[index] = unit_signed(random.rotate_left(11)) * 226.0;
            self.pool.age[index] = unit01(random.rotate_left(21));
            self.pool.life[index] = 0.55 + unit01(random.rotate_left(7)) * 0.9;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
            self.pool.flags[index] = (index % 3) as u8;
        }
    }

    fn project_spatial_field_stack(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let mut clipped = 0usize;
        for index in (0..self.pool.active()).step_by(2) {
            let lane = self.pool.flags[index];
            let phase = (seconds * 0.08 * self.pool.life[index] + self.pool.age[index]).fract();
            let base_x = self.pool.x[index];
            let base_y = self.pool.y[index];
            let (center, x, y, style) = match lane {
                0 => {
                    let contraction = 0.22 + (1.0 - phase) * 0.78;
                    let angle = seconds * 0.17 + self.pool.age[index] * 4.0;
                    let rotated_x = base_x * angle.cos() - base_y * 0.22 * angle.sin();
                    let rotated_y = base_x * angle.sin() * 0.35 + base_y * angle.cos();
                    (240.0, rotated_x * contraction, rotated_y * contraction, 5)
                }
                1 => {
                    let radius = (base_x * base_x + base_y * base_y).sqrt().max(1.0);
                    let cavity = 68.0 + phase * 145.0;
                    let scale = cavity.max(radius) / radius;
                    (480.0, base_x * scale, base_y * scale, 6)
                }
                _ => {
                    let capture = if phase < 0.72 {
                        1.0 - phase * 0.58
                    } else {
                        0.58 + (phase - 0.72) * 3.2
                    };
                    let angle = seconds * (0.62 + self.pool.age[index] * 0.3);
                    let rotated_x = base_x * angle.cos() - base_y * angle.sin();
                    let rotated_y = base_x * angle.sin() + base_y * angle.cos();
                    (720.0, rotated_x * capture, rotated_y * capture, 4)
                }
            };
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                center + x,
                270.0 + y,
                style,
                index & 127 == 0,
            ) {
                clipped = clipped.saturating_add(1);
            }
        }
        clipped
    }

    fn initialize_layered_child_systems(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.age[index] = unit01(random.rotate_left(7));
            self.pool.life[index] = 0.65 + unit01(random.rotate_left(17)) * 0.7;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
            self.pool.flags[index] = (index % 3) as u8;
        }
    }

    fn project_layered_child_systems(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let cycle = seconds.rem_euclid(10.0) / 10.0;
        for parent in 0..3usize {
            let offset = parent as f32 - 1.0;
            let head_x = 480.0 + offset * 215.0 + (cycle * std::f32::consts::TAU).sin() * 58.0;
            let head_y = 420.0 - cycle * 330.0
                + (cycle * std::f32::consts::PI).sin() * (-72.0 - parent as f32 * 18.0);
            let color = CHILD_PALETTE[3 + parent];
            let mut previous = None;
            for sample in 0..14usize {
                let trail = sample as f32 / 13.0;
                let x = head_x - (cycle * 5.0 + parent as f32).cos() * trail * 92.0
                    + (trail * 17.0 + parent as f32).sin() * 7.0;
                let y = head_y + trail * (80.0 + parent as f32 * 12.0);
                if let Some((previous_x, previous_y)) = previous {
                    self.material_strokes.push(MaterialStroke {
                        x0: previous_x,
                        y0: previous_y,
                        x1: x,
                        y1: y,
                        start_radius: 2,
                        end_radius: 1,
                        intensity: (15.0 - trail * 8.0) as u8,
                        color,
                    });
                }
                previous = Some((x, y));
            }
            self.material_stamps.push(MaterialStamp {
                x: head_x as i16,
                y: head_y as i16,
                radius: 4,
                intensity: 15,
                color: Rgb565Pixel(0xffff),
                shape: MaterialShape::Star,
            });
            let ring_age = (cycle * 3.0 + parent as f32 * 0.27).fract();
            if ring_age < 0.58 {
                let ring_radius = 22.0 + ring_age * 90.0;
                for ring in 0..32usize {
                    let angle = std::f32::consts::TAU * ring as f32 / 32.0;
                    self.material_stamps.push(MaterialStamp {
                        x: (head_x + angle.cos() * ring_radius) as i16,
                        y: (head_y + angle.sin() * ring_radius) as i16,
                        radius: 1,
                        intensity: (13.0 - ring_age * 14.0) as u8,
                        color: CHILD_PALETTE[5],
                        shape: MaterialShape::Disc,
                    });
                }
            }
        }
        for index in 3..self.pool.active() {
            let parent = usize::from(self.pool.flags[index]);
            let random = self.pool.random[index];
            let release = (cycle + self.pool.age[index]).fract();
            let parent_offset = parent as f32 - 1.0;
            let origin_x = 480.0 + parent_offset * 215.0;
            let origin_y = 190.0 + parent as f32 * 18.0;
            let angle = std::f32::consts::TAU * unit01(random.rotate_left(11));
            let speed = 18.0 + unit01(random.rotate_left(21)) * 105.0;
            let x = origin_x + angle.cos() * speed * release;
            let y = origin_y + angle.sin() * speed * release + 72.0 * release * release;
            let style = if release < 0.18 {
                7
            } else {
                usize::from(self.pool.style[index]).min(6)
            };
            let _ = push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                y,
                style as u8,
                index & 63 == 0,
            );
        }
        0
    }

    fn initialize_density_bloom(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.age[index] = unit01(random.rotate_left(7));
            self.pool.life[index] = 0.75 + unit01(random.rotate_left(17)) * 0.5;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
            self.pool.flags[index] = u8::from(index & 63 == 0);
        }
    }

    fn project_density_bloom(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        self.density.fill(0);
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let pulse = 0.92 + (seconds * 0.7).sin() * 0.06;
        for index in (0..self.pool.active()).step_by(4) {
            let random = self.pool.random[index];
            let angle = std::f32::consts::TAU
                * (unit01(random) + seconds * (0.006 + self.pool.age[index] * 0.004));
            let radial = (0.45 + unit01(random.rotate_left(11)) * 0.55).sqrt();
            let radius = (54.0 + radial * 168.0) * pulse;
            let world_x = angle.cos() * radius + 42.0;
            let world_y = angle.sin() * radius * 0.82;
            let cavity_x = world_x + 58.0;
            let cavity_y = world_y;
            if cavity_x * cavity_x + cavity_y * cavity_y < 118.0 * 118.0 {
                continue;
            }
            let x = ((480.0 + world_x) * 0.25) as i32;
            let y = ((270.0 + world_y) * 0.25) as i32;
            if !(1..DENSITY_W as i32 - 1).contains(&x) || !(1..DENSITY_H as i32 - 1).contains(&y) {
                continue;
            }
            let center = y as usize * DENSITY_W + x as usize;
            let weight = 6 + u16::from(self.pool.style[index]);
            for (offset, scale) in [
                (0isize, 4u16),
                (-1, 2),
                (1, 2),
                (-(DENSITY_W as isize), 2),
                (DENSITY_W as isize, 2),
            ] {
                let cell = (center as isize + offset) as usize;
                self.density[cell] = self.density[cell].saturating_add(weight * scale);
            }
            if self.pool.flags[index] != 0 {
                let _ = push_screen_command(
                    &mut self.commands,
                    self.config.width,
                    self.config.height,
                    480.0 + world_x,
                    270.0 + world_y,
                    7,
                    true,
                );
            }
        }
        self.density_blur.fill(0);
        for y in 1..DENSITY_H - 1 {
            for x in 1..DENSITY_W - 1 {
                let offset = y * DENSITY_W + x;
                self.density_blur[offset] = (self.density[offset] * 2
                    + self.density[offset - 1]
                    + self.density[offset + 1]
                    + self.density[offset - DENSITY_W]
                    + self.density[offset + DENSITY_W])
                    / 6;
            }
        }
        std::mem::swap(&mut self.density, &mut self.density_blur);
        0
    }

    fn initialize_curl_noise_flow_field(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = 24.0 + unit01(random) * 912.0;
            self.pool.y[index] = 24.0 + unit01(random.rotate_left(11)) * 492.0;
            self.pool.age[index] = unit01(random.rotate_left(21));
            self.pool.life[index] = 0.45 + unit01(random.rotate_left(7)) * 0.85;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
            self.pool.flags[index] = u8::from(index & 127 == 0);
        }
    }

    fn update_curl_noise_flow_field(&mut self, elapsed: Duration) {
        let dt = elapsed
            .saturating_sub(self.flow_last_elapsed)
            .as_secs_f32()
            .clamp(0.0, 1.0 / 30.0);
        self.flow_last_elapsed = elapsed;
        if dt <= f32::EPSILON {
            return;
        }
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let cohort = ((elapsed.as_micros() / 16_667) & 3) as usize;
        for index in (cohort..self.pool.active()).step_by(4) {
            let normalized_x = (self.pool.x[index] - 480.0) / 260.0;
            let normalized_y = (self.pool.y[index] - 270.0) / 180.0;
            let phase = seconds * 0.19 + self.pool.age[index] * 9.0;
            let left_dx = normalized_x + 0.78;
            let right_dx = normalized_x - 0.78;
            let left_radius = (left_dx * left_dx + normalized_y * normalized_y + 0.18).recip();
            let right_radius = (right_dx * right_dx + normalized_y * normalized_y + 0.18).recip();
            let vx = -normalized_y * left_radius + normalized_y * right_radius
                - (normalized_y * 2.7 - phase * 0.29).cos() * 0.52
                + 0.32;
            let vy = left_dx * left_radius - right_dx * right_radius
                + (normalized_x * 2.1 + phase * 0.37).sin() * 0.48;
            self.pool.vx[index] = vx * 58.0;
            self.pool.vy[index] = vy * 58.0;
            self.pool.x[index] =
                (self.pool.x[index] + self.pool.vx[index] * dt * 4.0 + 960.0).rem_euclid(960.0);
            self.pool.y[index] =
                (self.pool.y[index] + self.pool.vy[index] * dt * 4.0 + 540.0).rem_euclid(540.0);
        }
    }

    fn project_curl_noise_flow_field(&mut self, _elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let mut clipped = 0usize;
        for index in (0..self.pool.active()).step_by(4) {
            let x = self.pool.x[index];
            let y = self.pool.y[index];
            let tracer = self.pool.flags[index] != 0;
            let style = if tracer {
                7
            } else {
                usize::from(self.pool.style[index]).min(5)
            };
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                y,
                style as u8,
                tracer,
            ) {
                clipped = clipped.saturating_add(1);
            }
            if tracer {
                self.material_strokes.push(MaterialStroke {
                    x0: x - self.pool.vx[index] * 0.12,
                    y0: y - self.pool.vy[index] * 0.12,
                    x1: x,
                    y1: y,
                    start_radius: 1,
                    end_radius: 2,
                    intensity: 13,
                    color: FLOW_PALETTE[style],
                });
            }
        }
        clipped
    }

    fn initialize_variable_width_ribbons(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.age[index] = unit01(random.rotate_left(7));
            self.pool.life[index] = 0.7 + unit01(random.rotate_left(17)) * 0.8;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
        }
    }

    fn project_variable_width_ribbons(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        const RIBBON_COUNT: usize = 18;
        const RIBBON_SAMPLES: usize = 16;
        for ribbon in 0..RIBBON_COUNT {
            let lane = ribbon % RIBBON_PALETTE.len();
            let random = self.pool.random[ribbon];
            let depth = unit01(random.rotate_left(9));
            let hero = ribbon % 3 == 0;
            let lane_offset =
                (ribbon as f32 - (RIBBON_COUNT as f32 - 1.0) * 0.5) * (3.2 + depth * 1.6);
            let start_t = unit01(random.rotate_left(19)) * 0.18;
            let end_t = 0.76
                + unit01(random.rotate_left(29)) * 0.24
                + (seconds * 0.11 + ribbon as f32).sin() * 0.018;
            let mut previous = None;
            for sample in 0..RIBBON_SAMPLES {
                let progress = sample as f32 / (RIBBON_SAMPLES - 1) as f32;
                let t = start_t + (end_t - start_t) * progress;
                let u = t * 2.0 - 1.0;
                let x = center_x + u * (350.0 + depth * 48.0);
                let y = center_y - (u * std::f32::consts::PI).sin() * (128.0 + depth * 52.0)
                    + lane_offset * (u * std::f32::consts::PI).cos();
                if let Some((previous_x, previous_y)) = previous {
                    let previous_radius = u8::from(hero && (4..=13).contains(&sample)) + 1;
                    let radius = u8::from(hero && (4..=12).contains(&sample)) + 1;
                    self.material_strokes.push(MaterialStroke {
                        x0: previous_x,
                        y0: previous_y,
                        x1: x,
                        y1: y,
                        start_radius: previous_radius,
                        end_radius: radius,
                        intensity: 7 + (depth * 8.0) as u8,
                        color: RIBBON_PALETTE[lane],
                    });
                    if ribbon % 6 == 0 && sample > 5 {
                        self.material_strokes.push(MaterialStroke {
                            x0: previous_x,
                            y0: previous_y,
                            x1: x,
                            y1: y,
                            start_radius: 2,
                            end_radius: 2,
                            intensity: 2,
                            color: RIBBON_PALETTE[lane.saturating_sub(1)],
                        });
                    }
                }
                previous = Some((x, y));
            }
            let head_u = end_t * 2.0 - 1.0;
            let head_x = center_x + head_u * (350.0 + depth * 48.0);
            let head_y = center_y - (head_u * std::f32::consts::PI).sin() * (128.0 + depth * 52.0)
                + lane_offset * (head_u * std::f32::consts::PI).cos();
            self.material_stamps.push(MaterialStamp {
                x: head_x as i16,
                y: head_y as i16,
                radius: if hero { 4 } else { 2 },
                intensity: 15,
                color: RIBBON_PALETTE[lane],
                shape: MaterialShape::Star,
            });
        }
        for streak in 0..144usize {
            let index = RIBBON_COUNT + streak;
            let random = self.pool.random[index];
            let t = unit01(random);
            let u = t * 2.0 - 1.0;
            let x = center_x + u * 410.0;
            let y = center_y - (u * std::f32::consts::PI).sin() * 190.0
                + unit_signed(random.rotate_left(11)) * 54.0;
            let length = 3.0 + unit01(random.rotate_left(21)) * 12.0;
            self.material_strokes.push(MaterialStroke {
                x0: x - length,
                y0: y + length * 0.32,
                x1: x,
                y1: y,
                start_radius: 1,
                end_radius: 1,
                intensity: 7,
                color: RIBBON_PALETTE[usize::from(self.pool.style[index])],
            });
        }
        0
    }

    fn initialize_procedural_sprite_materials(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random.rotate_left(3)) * 430.0;
            self.pool.y[index] = unit_signed(random.rotate_left(13)) * 230.0;
            self.pool.z[index] = unit01(random.rotate_left(23));
            self.pool.age[index] = unit01(random.rotate_left(7));
            self.pool.life[index] = 0.55 + unit01(random.rotate_left(17)) * 0.9;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
            self.pool.flags[index] = (index % 5) as u8;
        }
    }

    fn project_procedural_sprite_materials(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.94;
        let mut clipped = 0usize;
        for index in (0..self.pool.active()).step_by(4) {
            let phase =
                (seconds * self.pool.life[index] * 0.12 + self.pool.age[index]).rem_euclid(1.0);
            let random = self.pool.random[index];
            let shape_lane = self.pool.flags[index];
            let angle = unit_signed(random.rotate_left(11)) * 1.15 - std::f32::consts::FRAC_PI_2
                + (seconds * 0.17 + self.pool.age[index] * 9.0).sin() * 0.08;
            let speed = 90.0 + unit01(random.rotate_left(21)) * 360.0;
            let travel = phase * speed;
            let spread = 0.45 + f32::from(shape_lane) * 0.11;
            let x = center_x + angle.cos() * travel * spread + self.pool.x[index] * phase * 0.28;
            let y = center_y + angle.sin() * travel + 90.0 * phase * phase;
            let over_life = (1.0 - phase).clamp(0.0, 1.0);
            let radius = match shape_lane {
                0 => 3,
                1 => 4,
                2 => 5,
                3 => 5,
                _ => 4,
            };
            let shape = match shape_lane {
                0 => MaterialShape::Spark,
                1 => MaterialShape::Star,
                2 => MaterialShape::Disc,
                3 => MaterialShape::Smoke,
                _ => MaterialShape::Shard,
            };
            let style = if phase < 0.12 {
                7
            } else {
                ((over_life * 6.0) as usize + usize::from(self.pool.style[index] & 1)).min(7)
            };
            let material_color = if index & 127 == 0 {
                Rgb565Pixel(0x05ff)
            } else {
                match shape {
                    MaterialShape::Star => Rgb565Pixel(0xf81f),
                    MaterialShape::Smoke => Rgb565Pixel(0x600f),
                    MaterialShape::Shard => Rgb565Pixel(0xfe80),
                    _ => MATERIAL_PALETTE[style],
                }
            };
            if index & 63 == 0 {
                self.material_stamps.push(MaterialStamp {
                    x: x.round() as i16,
                    y: y.round() as i16,
                    radius,
                    intensity: (4.0 + over_life * 11.0) as u8,
                    color: material_color,
                    shape,
                });
            } else if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                x,
                y,
                style as u8,
                matches!(shape, MaterialShape::Spark | MaterialShape::Star),
            ) {
                clipped = clipped.saturating_add(1);
            }
        }
        clipped
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

    fn raster_materials(
        &self,
        destination: &mut [Rgb565Pixel],
        dirty_offsets: &mut Vec<u32>,
    ) -> MaterialRasterStats {
        let mut total = MaterialRasterStats::default();
        for &stamp in &self.material_stamps {
            let stats = raster_stamp(
                destination,
                dirty_offsets,
                self.config.width,
                self.config.height,
                stamp,
            );
            total.stamps = total.stamps.saturating_add(stats.stamps);
            total.attempted_pixel_writes = total
                .attempted_pixel_writes
                .saturating_add(stats.attempted_pixel_writes);
        }
        total
    }

    fn raster_material_strokes(
        &self,
        destination: &mut [Rgb565Pixel],
        dirty_offsets: &mut Vec<u32>,
    ) -> MaterialRasterStats {
        let mut total = MaterialRasterStats::default();
        let sparse_untracked = self.demo == ParticleDemoKind::VariableWidthRibbons;
        for &stroke in &self.material_strokes {
            let stats = raster_tapered_segment(
                destination,
                dirty_offsets,
                self.config.width,
                self.config.height,
                stroke,
                if sparse_untracked { 2 } else { 1 },
                !sparse_untracked,
            );
            total.stamps = total.stamps.saturating_add(stats.stamps);
            total.attempted_pixel_writes = total
                .attempted_pixel_writes
                .saturating_add(stats.attempted_pixel_writes);
        }
        total
    }

    fn raster_effect_background(&self, destination: &mut [Rgb565Pixel]) -> usize {
        if self.demo == ParticleDemoKind::LowResolutionDensityBloom {
            destination.fill(Rgb565Pixel(0));
            let mut writes = 0usize;
            for cell_y in 0..DENSITY_H {
                for cell_x in 0..DENSITY_W {
                    let density = self.density[cell_y * DENSITY_W + cell_x];
                    let style = match density {
                        0..=7 => 0,
                        8..=23 => 1,
                        24..=47 => 2,
                        48..=79 => 3,
                        80..=127 => 4,
                        128..=191 => 5,
                        192..=287 => 6,
                        _ => 7,
                    };
                    if style == 0 {
                        continue;
                    }
                    let color = DENSITY_PALETTE[style];
                    for y in cell_y * DENSITY_SCALE..(cell_y + 1) * DENSITY_SCALE {
                        let row = y * self.config.width;
                        for x in cell_x * DENSITY_SCALE..(cell_x + 1) * DENSITY_SCALE {
                            destination[row + x] = color;
                            writes = writes.saturating_add(1);
                        }
                    }
                }
            }
            return writes;
        }
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
            &self.demo.hud_label(),
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
        return (
            formation,
            std::f32::consts::FRAC_PI_2 - 0.62,
            -0.08,
            760.0,
            0.0,
        );
    }
    if seconds < 24.0 {
        let phase = (seconds - 4.0) / 20.0;
        return (
            1.0,
            std::f32::consts::FRAC_PI_2 - 0.62 + (phase * std::f32::consts::TAU).sin() * 1.15,
            triangle_wave(phase * 2.0) * 0.13,
            720.0 + triangle_wave(phase + 0.25) * 82.0,
            0.0,
        );
    }
    if seconds < 29.0 {
        let return_t = ease_out_cubic((seconds - 24.0) * 0.2);
        return (
            1.0,
            (std::f32::consts::FRAC_PI_2 - 0.62) * (1.0 - return_t) + 0.72 * return_t,
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

fn firework_beat(elapsed: Duration) -> &'static str {
    match elapsed.as_secs_f32() {
        seconds if seconds < 1.2 => "launch",
        seconds if seconds < 2.8 => "burst",
        _ => "fall",
    }
}

fn showcase_palette(demo: ParticleDemoKind) -> &'static [Rgb565Pixel; 8] {
    match demo {
        ParticleDemoKind::SolarChrysanthemum
        | ParticleDemoKind::RecursiveHalo
        | ParticleDemoKind::CopperWillowRain
        | ParticleDemoKind::PhoenixComet
        | ParticleDemoKind::MagneticFlower
        | ParticleDemoKind::OledPeony
        | ParticleDemoKind::SolarChrysanthemumV2
        | ParticleDemoKind::RecursiveHaloV2
        | ParticleDemoKind::CopperWillowRainV2
        | ParticleDemoKind::PhoenixCometV2
        | ParticleDemoKind::MagneticFlowerV2
        | ParticleDemoKind::OledPeonyV2 => &FIREWORKS_PALETTE,
        ParticleDemoKind::FireEmbers => &FIRE_PALETTE,
        ParticleDemoKind::SpiralGalaxy => &GALAXY_PALETTE,
        ParticleDemoKind::WarpSpeed => &WARP_PALETTE,
        ParticleDemoKind::MeteorShower => &METEOR_PALETTE,
        ParticleDemoKind::Weather => &WEATHER_PALETTE,
        ParticleDemoKind::ParticlePortal => &PORTAL_PALETTE,
        ParticleDemoKind::ElectricStorm => &ELECTRIC_PALETTE,
        ParticleDemoKind::FountainWaterfall => &WATER_PALETTE,
        ParticleDemoKind::ArcadeCabinet => &ARCADE_PALETTE,
        ParticleDemoKind::ProceduralSpriteMaterials => &MATERIAL_PALETTE,
        ParticleDemoKind::VariableWidthRibbons => &RIBBON_PALETTE,
        ParticleDemoKind::CurlNoiseFlowField => &FLOW_PALETTE,
        ParticleDemoKind::LowResolutionDensityBloom => &DENSITY_PALETTE,
        ParticleDemoKind::LayeredChildSystems => &CHILD_PALETTE,
        ParticleDemoKind::SpatialFieldStack => &FIELD_PALETTE,
        ParticleDemoKind::DepthAwareMaterialLod => &DEPTH_PALETTE,
        ParticleDemoKind::SourceDrivenMorph => &MORPH_PALETTE,
        ParticleDemoKind::SdfCollisionEvents => &COLLISION_PALETTE,
        ParticleDemoKind::GridAcceleratedFlocking => &FLOCK_PALETTE,
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
        assert_eq!(ParticleDemoKind::ALL.len(), 31);
        assert_eq!(
            ParticleDemoKind::SolarChrysanthemum.offset_wrapped(-1),
            ParticleDemoKind::GridAcceleratedFlocking
        );
        assert_eq!(
            ParticleDemoKind::GridAcceleratedFlocking.offset_wrapped(1),
            ParticleDemoKind::SolarChrysanthemum
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
            Some(ParticleDemoKind::SolarChrysanthemum)
        );
        assert_eq!(
            ParticleDemoKind::parse("oled-peony"),
            Some(ParticleDemoKind::OledPeony)
        );
        assert_eq!(
            ParticleDemoKind::parse("oled-peony-v2"),
            Some(ParticleDemoKind::OledPeonyV2)
        );
        assert_eq!(
            ParticleDemoKind::parse("particle-portal"),
            Some(ParticleDemoKind::ParticlePortal)
        );
        assert_eq!(
            ParticleDemoKind::parse("12"),
            Some(ParticleDemoKind::OledPeonyV2)
        );
        assert_eq!(
            ParticleDemoKind::parse("22"),
            Some(ParticleDemoKind::ProceduralSpriteMaterials)
        );
        assert_eq!(
            ParticleDemoKind::parse("23"),
            Some(ParticleDemoKind::VariableWidthRibbons)
        );
        assert_eq!(
            ParticleDemoKind::parse("24"),
            Some(ParticleDemoKind::CurlNoiseFlowField)
        );
        assert_eq!(
            ParticleDemoKind::parse("25"),
            Some(ParticleDemoKind::LowResolutionDensityBloom)
        );
        assert_eq!(
            ParticleDemoKind::parse("26"),
            Some(ParticleDemoKind::LayeredChildSystems)
        );
        assert_eq!(
            ParticleDemoKind::parse("27"),
            Some(ParticleDemoKind::SpatialFieldStack)
        );
        assert_eq!(
            ParticleDemoKind::parse("28"),
            Some(ParticleDemoKind::DepthAwareMaterialLod)
        );
        assert_eq!(
            ParticleDemoKind::parse("29"),
            Some(ParticleDemoKind::SourceDrivenMorph)
        );
        assert_eq!(
            ParticleDemoKind::parse("30"),
            Some(ParticleDemoKind::SdfCollisionEvents)
        );
        assert_eq!(
            ParticleDemoKind::parse("31"),
            Some(ParticleDemoKind::GridAcceleratedFlocking)
        );
        assert_eq!(ParticleDemoKind::parse("32"), None);
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
                initial_demo: ParticleDemoKind::SolarChrysanthemum,
            }
            .validate()
            .is_ok()
        );
        assert!(
            ParticleShowcaseConfig {
                width: 1280,
                height: 720,
                seed: 1,
                initial_demo: ParticleDemoKind::SolarChrysanthemum,
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
            initial_demo: ParticleDemoKind::SolarChrysanthemum,
        })
        .unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];
        let first = renderer
            .render(&mut destination, 1, Duration::from_millis(800))
            .unwrap();
        assert_eq!(first.demo, ParticleDemoKind::SolarChrysanthemum);
        assert!(first.visible > 0);

        request_particle_demo_navigation(-1);
        let wrapped = renderer
            .render(&mut destination, 2, Duration::from_millis(817))
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
        assert_eq!(renderer.commands.len(), 12_288);
        assert!(orbit.visible > 10_000);
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

    #[test]
    fn commercial_techniques_render_nonempty_bounded_frames() {
        for kind in ParticleDemoKind::ALL.into_iter().skip(21) {
            let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
                width: 960,
                height: 540,
                seed: 827_141_709_451,
                initial_demo: kind,
            })
            .unwrap();
            renderer.configure_capture_hud(false);
            let mut first = vec![Rgb565Pixel(0); 960 * 540];
            let mut second = vec![Rgb565Pixel(0); 960 * 540];
            renderer.render(&mut first, 1, Duration::ZERO).unwrap();
            let stats = renderer
                .render(&mut second, 2, Duration::from_secs(15))
                .unwrap();

            assert_eq!(stats.demo, kind);
            assert_eq!(stats.count, kind.starting_count());
            assert!(
                stats.attempted_pixel_writes > 0,
                "{} produced no pixel writes",
                kind.telemetry_label()
            );
            assert!(
                second.iter().any(|pixel| pixel.0 != 0),
                "{} produced an empty hero frame",
                kind.telemetry_label()
            );
            assert!(stats.clipped_commands <= stats.count);
        }
    }
}
