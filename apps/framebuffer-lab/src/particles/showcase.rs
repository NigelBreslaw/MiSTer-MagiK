// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Archived data model for the interactive ARM particle showcase.

use super::fireworks::{FireworkRenderer, embedded_firework_json};
use super::fireworks_v2::{FireworkV2Renderer, embedded_firework_v2_json};
use super::form::{FormSceneKind, FormSceneRenderer};
use super::live_reload::{
    LIVE_PARTICLE_MAX_FILE_BYTES, LastGoodFile, LiveParticleStatus, LiveParticleStatusState,
    publish_live_particle_status,
};
use super::material::{
    MaterialRasterStats, MaterialShape, MaterialStamp, MaterialStroke, raster_stamp,
    raster_tapered_segment,
};
use super::recipes::{
    CompiledRecipe, ParticleRecipeCategory, ParticleRecipeFamily, embedded_firework_duration_ms,
    form_recipe, procedural_recipe,
};
use crate::Rgb565Pixel;
use std::path::{Path, PathBuf};
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
const MAX_SEGMENT_PIXELS: i32 = 12;
const SEGMENT_CAPACITY: usize = 32_768;
const REFERENCE_WIDTH: usize = 960;
const REFERENCE_HEIGHT: usize = 540;
const FIRE_HEAT_REFERENCE_HEIGHT: usize = 72;
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
const FLOCK_CELL_PX: f32 = 16.0;
const DENSITY_SCALE: usize = 4;
const ARCADE_CLOUD: &[u8] = include_bytes!("../../assets/particles/arcade-cabinet.pcloud");
const PARTICLE_CLOUD_MAGIC: &[u8; 8] = b"PCLOUD1\0";
const PARTICLE_CLOUD_HEADER_BYTES: usize = 28;
const PARTICLE_CLOUD_RECORD_BYTES: usize = 8;
const DEVICE_LIVE_FAMILY_PATH: &str = "/tmp/mister-magik/live-particles/family.json";
const DEVICE_LIVE_STATUS_PATH: &str = "/tmp/mister-magik/live-particles/status.json";
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
    FractalGridTerrain,
    LayerMappedHologram,
    SphericalFieldObservatory,
    TwistedMultiFormCathedral,
    PointCloudMorphPassage,
}

impl ParticleDemoKind {
    pub const ALL: [Self; 36] = [
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
        Self::FractalGridTerrain,
        Self::LayerMappedHologram,
        Self::SphericalFieldObservatory,
        Self::TwistedMultiFormCathedral,
        Self::PointCloudMorphPassage,
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
            Self::FractalGridTerrain => "FRACTAL GRID TERRAIN",
            Self::LayerMappedHologram => "LAYER-MAPPED HOLOGRAM",
            Self::SphericalFieldObservatory => "SPHERICAL FIELD OBSERVATORY",
            Self::TwistedMultiFormCathedral => "TWISTED MULTI-FORM CATHEDRAL",
            Self::PointCloudMorphPassage => "POINT-CLOUD MORPH PASSAGE",
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
            Self::FractalGridTerrain => "fractal-grid-terrain",
            Self::LayerMappedHologram => "layer-mapped-hologram",
            Self::SphericalFieldObservatory => "spherical-field-observatory",
            Self::TwistedMultiFormCathedral => "twisted-multi-form-cathedral",
            Self::PointCloudMorphPassage => "point-cloud-morph-passage",
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
    pub fn starting_count(self) -> usize {
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
            demo if demo.form_scene().is_some() => {
                form_recipe(demo.telemetry_label()).particle_count
            }
            demo => procedural_recipe(demo.telemetry_label()).particle_count,
        }
    }

    #[must_use]
    pub const fn form_scene(self) -> Option<FormSceneKind> {
        match self {
            Self::FractalGridTerrain => Some(FormSceneKind::FractalGridTerrain),
            Self::LayerMappedHologram => Some(FormSceneKind::LayerMappedHologram),
            Self::SphericalFieldObservatory => Some(FormSceneKind::SphericalFieldObservatory),
            Self::TwistedMultiFormCathedral => Some(FormSceneKind::TwistedMultiFormCathedral),
            Self::PointCloudMorphPassage => Some(FormSceneKind::PointCloudMorphPassage),
            _ => None,
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
    fireworks_family: Option<ParticleRecipeFamily>,
    procedural_family: Option<ParticleRecipeFamily>,
    form_family: Option<ParticleRecipeFamily>,
    live_family: Option<LiveFamilyReload>,
    firework_capture_time: Option<Duration>,
    pool: ParticleShowcasePool,
    form_renderer: FormSceneRenderer,
    commands: Vec<u32>,
    previous_commands: Vec<u32>,
    segments: Vec<ParticleShowcaseSegment>,
    material_stamps: Vec<MaterialStamp>,
    material_strokes: Vec<MaterialStroke>,
    transition: ParticleShowcaseTransition,
    transition_started_at: Option<Duration>,
    heat: Vec<u8>,
    heat_width: usize,
    heat_height: usize,
    density: Vec<u16>,
    density_blur: Vec<u16>,
    density_width: usize,
    density_height: usize,
    flock_counts: Vec<u16>,
    flock_vx: Vec<f32>,
    flock_vy: Vec<f32>,
    flock_grid_width: usize,
    flock_grid_height: usize,
    flock_last_elapsed: Duration,
    flow_last_elapsed: Duration,
    heat_frame: u64,
    galaxy_projected_count: usize,
    dirty_slots: [ParticleShowcaseDirtySlot; HIDDEN_SLOT_COUNT],
    renderer_scratch_bytes: usize,
}

struct LiveFamilyReload {
    watcher: LastGoodFile<ParticleRecipeFamily>,
    status_path: Option<PathBuf>,
    status: LiveParticleStatus,
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
    fn recipe_for(&self, demo: ParticleDemoKind) -> &CompiledRecipe {
        let id = demo.telemetry_label();
        let override_family = if demo.form_scene().is_some() {
            self.form_family.as_ref()
        } else {
            self.procedural_family.as_ref()
        };
        override_family
            .and_then(|family| family.recipe(id))
            .unwrap_or_else(|| {
                if demo.form_scene().is_some() {
                    form_recipe(id)
                } else {
                    procedural_recipe(id)
                }
            })
    }

    fn param(&self, name: &str) -> f32 {
        self.recipe_for(self.demo).param(name)
    }

    fn palette(&self) -> &[Rgb565Pixel; 8] {
        if self.firework_renderer.is_some() {
            &FIREWORKS_PALETTE
        } else {
            let recipe = self.recipe_for(self.demo);
            let has_override = if self.demo.form_scene().is_some() {
                self.form_family.is_some()
            } else {
                self.procedural_family.is_some()
            };
            if !has_override {
                debug_assert_eq!(&recipe.palette, showcase_palette(self.demo));
            }
            &recipe.palette
        }
    }

    pub fn new(config: ParticleShowcaseConfig) -> Result<Self, String> {
        let config = config.validate()?;
        let pool = ParticleShowcasePool::new();
        let form_renderer = FormSceneRenderer::new(config.seed);
        let commands = Vec::with_capacity(PARTICLE_DEMO_MAX_COUNT + PARTICLE_DEMO_TRANSITION_COUNT);
        let previous_commands = Vec::with_capacity(PARTICLE_DEMO_MAX_COUNT);
        let segments = Vec::with_capacity(SEGMENT_CAPACITY);
        let material_stamps = Vec::with_capacity(16_384);
        let material_strokes = Vec::with_capacity(2_048);
        let transition = ParticleShowcaseTransition::new();
        let heat_width = config.width.div_ceil(FIRE_HEAT_SCALE);
        let heat_height = config
            .height
            .saturating_mul(FIRE_HEAT_REFERENCE_HEIGHT)
            .div_ceil(REFERENCE_HEIGHT);
        let heat = vec![0; heat_width * heat_height];
        let density_width = config.width.div_ceil(DENSITY_SCALE);
        let density_height = config.height.div_ceil(DENSITY_SCALE);
        let density = vec![0; density_width * density_height];
        let density_blur = vec![0; density_width * density_height];
        let flock_grid_width = config.width.div_ceil(FLOCK_CELL_PX as usize);
        let flock_grid_height = config.height.div_ceil(FLOCK_CELL_PX as usize);
        let flock_counts = vec![0; flock_grid_width * flock_grid_height];
        let flock_vx = vec![0.0; flock_grid_width * flock_grid_height];
        let flock_vy = vec![0.0; flock_grid_width * flock_grid_height];
        let dirty_slots = std::array::from_fn(|_| ParticleShowcaseDirtySlot {
            initialized: false,
            offsets: Vec::with_capacity(PARTICLE_DEMO_MAX_COUNT.saturating_mul(2)),
        });
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
        let renderer_scratch_bytes =
            renderer_scratch_bytes.saturating_add(form_renderer.allocated_bytes());
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
            fireworks_family: None,
            procedural_family: None,
            form_family: None,
            live_family: None,
            firework_capture_time: None,
            pool,
            form_renderer,
            commands,
            previous_commands,
            segments,
            material_stamps,
            material_strokes,
            transition,
            transition_started_at: None,
            heat,
            heat_width,
            heat_height,
            density,
            density_blur,
            density_width,
            density_height,
            flock_counts,
            flock_vx,
            flock_vy,
            flock_grid_width,
            flock_grid_height,
            flock_last_elapsed: Duration::ZERO,
            flow_last_elapsed: Duration::ZERO,
            heat_frame: u64::MAX,
            galaxy_projected_count: 0,
            dirty_slots,
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

    /// Atomically replaces one validated recipe family and restarts the active
    /// composition only when that family owns it.
    pub fn replace_family(&mut self, family: ParticleRecipeFamily, elapsed: Duration) {
        let restart_active = family.contains(self.demo);
        match family.category() {
            ParticleRecipeCategory::Fireworks => self.fireworks_family = Some(family),
            ParticleRecipeCategory::Procedural => self.procedural_family = Some(family),
            ParticleRecipeCategory::Form => self.form_family = Some(family),
        }
        if restart_active {
            self.transition_started_at = None;
            self.transition.count = 0;
            self.reset_demo(self.demo, elapsed);
        }
    }

    /// Loads one family once without starting a background watcher.
    pub fn load_family_file(&mut self, path: &Path) -> Result<(), String> {
        let family = read_live_family_now(path)?;
        if !family.contains(self.demo) {
            return Err(format!(
                "particle family {} does not contain demo {}",
                path.display(),
                self.demo.number()
            ));
        }
        self.replace_family(family, Duration::ZERO);
        Ok(())
    }

    /// Enables one attended, whole-family live-reload session.
    ///
    /// Device sessions publish their state through the fixed volatile status
    /// path. Local sessions validate the initial file before starting the
    /// watcher; deterministic headless captures use `load_family_file`.
    pub fn enable_live_family(&mut self, path: PathBuf) -> Result<(), String> {
        if self.live_family.is_some() {
            return Err("particle showcase already has a live family".into());
        }
        let device_session = path == Path::new(DEVICE_LIVE_FAMILY_PATH);
        if !device_session {
            let family = read_live_family_now(&path)?;
            if !family.contains(self.demo) {
                return Err(format!(
                    "live particle family {} does not contain demo {}",
                    path.display(),
                    self.demo.number()
                ));
            }
        }
        let watcher = LastGoodFile::spawn(path, ParticleRecipeFamily::from_json)?;
        let status = LiveParticleStatus::embedded(self.demo.number() as u8);
        let status_path = device_session.then(|| PathBuf::from(DEVICE_LIVE_STATUS_PATH));
        if let Some(path) = status_path.as_deref() {
            publish_live_particle_status(path, &status)?;
        }
        self.live_family = Some(LiveFamilyReload {
            watcher,
            status_path,
            status,
        });
        Ok(())
    }

    #[must_use]
    pub fn live_reload_status_label(&self) -> Option<String> {
        self.live_family.as_ref().map(|live| {
            let state = match live.status.state {
                LiveParticleStatusState::Embedded => "embedded",
                LiveParticleStatusState::Applied => "applied",
                LiveParticleStatusState::Rejected => "rejected",
            };
            format!("recipes:{state}:{}", live.status.generation)
        })
    }

    #[must_use]
    pub fn live_reload_error(&self) -> Option<&str> {
        self.live_family
            .as_ref()
            .and_then(|live| live.status.error.as_deref())
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
        self.apply_live_family_reload(elapsed)?;
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
        _hud_visible: bool,
    ) {
        self.firework_capture_time = capture_time;
    }

    /// Retained for source compatibility with deterministic capture callers.
    /// The standalone lab never draws an in-frame HUD.
    pub const fn configure_capture_hud(&mut self, _hud_visible: bool) {}

    #[allow(clippy::too_many_arguments)]
    fn render_firework(
        &mut self,
        destination: &mut [Rgb565Pixel],
        slot: usize,
        elapsed: Duration,
        dirty_offsets: Vec<u32>,
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
        if self.live_family.is_some() {
            let _ = PARTICLE_DEMO_NAVIGATION.swap(0, Ordering::AcqRel);
            return;
        }
        let delta = PARTICLE_DEMO_NAVIGATION.swap(0, Ordering::AcqRel);
        if delta != 0 {
            self.begin_transition(elapsed);
            self.reset_demo(self.demo.offset_wrapped(delta), elapsed);
            return;
        }
        let demo_elapsed = elapsed.saturating_sub(self.demo_started_at);
        let duration = self.demo_duration();
        if demo_elapsed >= duration {
            let advances = (demo_elapsed.as_micros() / duration.as_micros()) as i32;
            self.begin_transition(elapsed);
            self.reset_demo(self.demo.offset_wrapped(advances), elapsed);
        }
    }

    fn apply_live_family_reload(&mut self, elapsed: Duration) -> Result<(), String> {
        let attempt = self
            .live_family
            .as_ref()
            .and_then(|live| live.watcher.take_latest());
        let Some(attempt) = attempt else {
            return Ok(());
        };
        let demo = self.demo.number() as u8;
        let status = match attempt.result {
            Ok(family) if family.contains(self.demo) => {
                self.replace_family(family, elapsed);
                LiveParticleStatus::applied(attempt.generation, demo)
            }
            Ok(_) => LiveParticleStatus::rejected(
                attempt.generation,
                demo,
                &format!(
                    "live particle family does not contain pinned demo {}",
                    self.demo.number()
                ),
            ),
            Err(error) => LiveParticleStatus::rejected(attempt.generation, demo, &error),
        };
        let status_path = self
            .live_family
            .as_ref()
            .and_then(|live| live.status_path.clone());
        self.live_family
            .as_mut()
            .expect("live family exists while applying its attempt")
            .status = status.clone();
        if let Some(path) = status_path {
            publish_live_particle_status(&path, &status)?;
        }
        Ok(())
    }

    fn demo_duration(&self) -> Duration {
        if self.demo.firework_id().is_some() || self.demo.firework_v2_id().is_some() {
            let duration_ms = self
                .fireworks_family
                .as_ref()
                .and_then(|family| family.duration_ms(self.demo))
                .or_else(|| embedded_firework_duration_ms(self.demo.telemetry_label()));
            duration_ms
                .map(Duration::from_millis)
                .unwrap_or(PARTICLE_DEMO_DURATION)
        } else {
            Duration::from_millis(self.recipe_for(self.demo).duration_ms)
        }
    }

    fn reset_demo(&mut self, demo: ParticleDemoKind, elapsed: Duration) {
        self.demo = demo;
        self.demo_started_at = elapsed;
        self.firework_renderer = demo
            .firework_id()
            .map(|id| {
                let json = self
                    .fireworks_family
                    .as_ref()
                    .and_then(|family| family.firework_show(id, "mister-magik-firework-v1"))
                    .or_else(|| embedded_firework_json(id))
                    .expect("registered firework must be embedded");
                ShowcaseFireworkRenderer::V1(
                    FireworkRenderer::from_json(
                        &json,
                        self.config.width,
                        self.config.height,
                        self.config.seed,
                    )
                    .expect("embedded V1 firework must satisfy its runtime contract"),
                )
            })
            .or_else(|| {
                demo.firework_v2_id().map(|id| {
                    let json = self
                        .fireworks_family
                        .as_ref()
                        .and_then(|family| family.firework_show(id, "mister-magik-firework-v2"))
                        .or_else(|| embedded_firework_v2_json(id))
                        .expect("registered V2 firework must be embedded");
                    ShowcaseFireworkRenderer::V2(
                        FireworkV2Renderer::from_json(
                            &json,
                            self.config.width,
                            self.config.height,
                            self.config.seed,
                        )
                        .expect("embedded V2 firework must satisfy its runtime contract"),
                    )
                })
            });
        // Transitions copy their outgoing geometry before reset. Everything
        // left here belongs to the previous renderer family and must not leak
        // into firework demos, which render directly without rebuilding the
        // shared command buffers.
        self.commands.clear();
        self.previous_commands.clear();
        self.segments.clear();
        self.material_stamps.clear();
        self.material_strokes.clear();
        let active = if self.firework_renderer.is_some() {
            0
        } else {
            self.recipe_for(demo).particle_count
        };
        self.pool.reset_with_count(demo, self.config.seed, active);
        if let Some(scene) = demo.form_scene() {
            let recipe = self.recipe_for(demo).clone();
            self.form_renderer.reset_with_recipe(scene, &recipe);
        }
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
            ParticleDemoKind::FractalGridTerrain
            | ParticleDemoKind::LayerMappedHologram
            | ParticleDemoKind::SphericalFieldObservatory
            | ParticleDemoKind::TwistedMultiFormCathedral
            | ParticleDemoKind::PointCloudMorphPassage => self.project_form_scene(elapsed),
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
        let logical = elapsed.saturating_sub(self.demo_started_at);
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
            | ParticleDemoKind::OledPeonyV2 => firework_beat(logical),
            demo => {
                let phase = self.recipe_for(demo).beat_phase(logical.as_millis() as u64);
                let embedded = if demo.form_scene().is_some() {
                    form_recipe(demo.telemetry_label())
                } else {
                    procedural_recipe(demo.telemetry_label())
                };
                embedded
                    .beats
                    .phases
                    .get(phase)
                    .or_else(|| embedded.beats.phases.last())
                    .map(|phase| phase.label.as_str())
                    .expect("validated embedded recipe has beat phases")
            }
        }
    }

    fn project_form_scene(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let scene = self
            .demo
            .form_scene()
            .expect("Form projection requires a Form demo");
        let logical_elapsed = elapsed.saturating_sub(self.demo_started_at);
        let (points, segments) = self.form_renderer.project(
            scene,
            logical_elapsed,
            self.config.width,
            self.config.height,
        );
        let mut clipped = 0usize;
        for point in points {
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                point.x,
                point.y,
                point.style,
                point.neighbor,
            ) {
                clipped = clipped.saturating_add(1);
            }
        }
        for segment in segments {
            self.segments.push(ParticleShowcaseSegment {
                x0: segment.x0,
                y0: segment.y0,
                x1: segment.x1,
                y1: segment.y1,
                style: segment.style,
            });
        }
        clipped
    }

    fn initialize_grid_flocking(&mut self) {
        let spawn_inner = self.param("spawn_inner");
        let spawn_span = self.param("spawn_span");
        let spawn_y_span = self.param("spawn_y_span");
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let side = if index & 1 == 0 { -1.0 } else { 1.0 };
            self.pool.x[index] = center_x + side * (spawn_inner + unit01(random) * spawn_span);
            self.pool.y[index] = center_y + unit_signed(random.rotate_left(11)) * spawn_y_span;
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
        let alignment = self.param("alignment");
        let separation = self.param("separation");
        let chaser_radius = self.param("chaser_radius");
        let chaser_force = self.param("chaser_force");
        let cavity_radius = self.param("cavity_radius");
        let cavity_force = self.param("cavity_force");
        let max_speed = self.param("max_speed");
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
        let width = self.config.width as f32;
        let height = self.config.height as f32;
        let center_x = width * 0.5;
        let center_y = height * 0.5;
        let grid_width = self.flock_grid_width;
        let grid_height = self.flock_grid_height;
        for index in 0..self.pool.active() {
            let cell_x =
                (self.pool.x[index] / FLOCK_CELL_PX).clamp(0.0, (grid_width - 1) as f32) as usize;
            let cell_y =
                (self.pool.y[index] / FLOCK_CELL_PX).clamp(0.0, (grid_height - 1) as f32) as usize;
            let cell = cell_y * grid_width + cell_x;
            self.flock_counts[cell] = self.flock_counts[cell].saturating_add(1);
            self.flock_vx[cell] += self.pool.vx[index];
            self.flock_vy[cell] += self.pool.vy[index];
        }
        let cavity_x = center_x + (elapsed.as_secs_f32() * 0.21).sin() * 58.0;
        let cavity_y = center_y;
        let cohort = ((elapsed.as_micros() / 16_667) & 3) as usize;
        for index in 0..self.pool.active() {
            if index & 3 != cohort {
                self.pool.x[index] =
                    (self.pool.x[index] + self.pool.vx[index] * dt + width).rem_euclid(width);
                self.pool.y[index] =
                    (self.pool.y[index] + self.pool.vy[index] * dt + height).rem_euclid(height);
                continue;
            }
            let cell_x =
                (self.pool.x[index] / FLOCK_CELL_PX).clamp(0.0, (grid_width - 1) as f32) as usize;
            let cell_y =
                (self.pool.y[index] / FLOCK_CELL_PX).clamp(0.0, (grid_height - 1) as f32) as usize;
            let mut count = 0u32;
            let mut sum_vx = 0.0;
            let mut sum_vy = 0.0;
            for y in cell_y.saturating_sub(1)..=(cell_y + 1).min(grid_height - 1) {
                for x in cell_x.saturating_sub(1)..=(cell_x + 1).min(grid_width - 1) {
                    let cell = y * grid_width + x;
                    count += u32::from(self.flock_counts[cell]);
                    sum_vx += self.flock_vx[cell];
                    sum_vy += self.flock_vy[cell];
                }
            }
            if count > 0 {
                self.pool.vx[index] +=
                    (sum_vx / count as f32 - self.pool.vx[index]) * dt * alignment;
                self.pool.vy[index] +=
                    (sum_vy / count as f32 - self.pool.vy[index]) * dt * alignment;
            }
            let left = self.flock_counts[cell_y * grid_width + cell_x.saturating_sub(1)];
            let right = self.flock_counts[cell_y * grid_width + (cell_x + 1).min(grid_width - 1)];
            let above = self.flock_counts[cell_y.saturating_sub(1) * grid_width + cell_x];
            let below = self.flock_counts[(cell_y + 1).min(grid_height - 1) * grid_width + cell_x];
            self.pool.vx[index] += (f32::from(left) - f32::from(right)) * dt * separation;
            self.pool.vy[index] += (f32::from(above) - f32::from(below)) * dt * separation;
            for chaser in (0..self.pool.active()).step_by(1_024) {
                let chaser_dx = self.pool.x[index] - self.pool.x[chaser];
                let chaser_dy = self.pool.y[index] - self.pool.y[chaser];
                let chaser_distance2 = chaser_dx * chaser_dx + chaser_dy * chaser_dy;
                if chaser_distance2 > 16.0 && chaser_distance2 < chaser_radius * chaser_radius {
                    let inverse = chaser_distance2.sqrt().recip();
                    self.pool.vx[index] += chaser_dx * inverse * dt * chaser_force;
                    self.pool.vy[index] += chaser_dy * inverse * dt * chaser_force;
                }
            }
            let dx = self.pool.x[index] - cavity_x;
            let dy = self.pool.y[index] - cavity_y;
            let distance2 = dx * dx + dy * dy;
            if distance2 < cavity_radius * cavity_radius {
                let inverse = distance2.max(64.0).sqrt().recip();
                self.pool.vx[index] += dx * inverse * dt * cavity_force;
                self.pool.vy[index] += dy * inverse * dt * cavity_force;
            }
            let side = if index & 1 == 0 { -1.0 } else { 1.0 };
            let target_y = center_y
                + side
                    * (self.pool.x[index] - center_x).abs().sqrt()
                    * 8.0
                    * (elapsed.as_secs_f32() * 0.09).cos();
            self.pool.vy[index] += (target_y - self.pool.y[index]) * dt * 0.75;
            let speed = (self.pool.vx[index] * self.pool.vx[index]
                + self.pool.vy[index] * self.pool.vy[index])
                .sqrt()
                .max(1.0);
            let limited = max_speed / speed.max(max_speed);
            self.pool.vx[index] *= limited;
            self.pool.vy[index] *= limited;
            self.pool.x[index] =
                (self.pool.x[index] + self.pool.vx[index] * dt + width).rem_euclid(width);
            self.pool.y[index] =
                (self.pool.y[index] + self.pool.vy[index] * dt + height).rem_euclid(height);
        }
    }

    fn project_grid_flocking(&mut self) -> usize {
        self.commands.clear();
        self.segments.clear();
        let palette = *self.palette();
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
                    color: palette[6],
                });
            }
        }
        0
    }

    fn initialize_sdf_collision_events(&mut self) {
        let x_span = self.param("spawn_x_span");
        let y_span = self.param("spawn_y_span");
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random) * x_span;
            self.pool.y[index] = unit_signed(random.rotate_left(11)) * y_span;
            self.pool.age[index] = unit01(random.rotate_left(21)) * 4.0;
            self.pool.life[index] = 0.72 + unit01(random.rotate_left(7)) * 0.65;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
            self.pool.flags[index] = u8::from(index & 3 == 0);
        }
    }

    fn project_sdf_collision_events(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let palette = *self.palette();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let warm_rate = self.param("warm_rate");
        let sphere_x = self.param("sphere_x");
        let sphere_y = self.param("sphere_y");
        let sphere_radius = self.param("sphere_radius");
        let cool_rate = self.param("cool_rate");
        let bowl_y_base = self.param("bowl_y");
        let bowl_curve = self.param("bowl_curve");
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let mut clipped = 0usize;
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let warm = self.pool.flags[index] != 0;
            let (x, y, collided, style) = if warm {
                let phase =
                    (seconds * warm_rate * self.pool.life[index] + self.pool.age[index]).fract();
                let angle = std::f32::consts::TAU * unit01(random.rotate_left(13));
                let travel = 190.0 - phase * 165.0;
                let mut local_x = sphere_x + angle.cos() * travel;
                let mut local_y = sphere_y + angle.sin() * travel;
                let dx = local_x - sphere_x;
                let dy = local_y - sphere_y;
                let distance = (dx * dx + dy * dy).sqrt().max(0.001);
                let collided = distance < sphere_radius;
                if collided {
                    let bounce = sphere_radius + (phase * 19.0).sin().abs() * 22.0;
                    local_x = sphere_x + dx / distance * bounce;
                    local_y = sphere_y + dy / distance * bounce;
                }
                (local_x, local_y, collided, 4 + usize::from(collided) * 2)
            } else {
                let phase = (seconds * cool_rate * self.pool.life[index] + self.pool.age[index])
                    .rem_euclid(3.2);
                let mut local_x = -176.0 + self.pool.x[index] * 0.48;
                let mut local_y = -252.0 + phase * 172.0 + 42.0 * phase * phase;
                let bowl_y = bowl_y_base + (local_x + 176.0).powi(2) * bowl_curve;
                let collided = local_y > bowl_y;
                if collided {
                    let slide = (phase - 1.55).max(0.0);
                    local_x += self.pool.x[index].signum() * slide * 92.0;
                    local_y = bowl_y_base + (local_x + 176.0).powi(2) * bowl_curve
                        - (slide * 8.0).sin().abs() * 16.0;
                }
                (local_x, local_y, collided, 2 + usize::from(collided))
            };
            let screen_x = center_x + x;
            let screen_y = center_y + y;
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
                    color: palette[style],
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
        let source_body_x = self.param("source_body_x");
        let source_body_y = self.param("source_body_y");
        let target_radius = self.param("target_radius");
        let target_radius_jitter = self.param("target_radius_jitter");
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let u = unit_signed(random);
            let v = unit_signed(random.rotate_left(11));
            let group = index % 8;
            let (source_x, source_y) = if group < 5 {
                (u * source_body_x, 78.0 + v * source_body_y)
            } else if group < 7 {
                (u * 34.0, -12.0 + v * 110.0)
            } else {
                let angle = std::f32::consts::TAU * unit01(random.rotate_left(21));
                (angle.cos() * 58.0, -128.0 + angle.sin() * 42.0)
            };
            let (target_x, target_y) = if group < 5 {
                let angle = std::f32::consts::TAU * unit01(random.rotate_left(7));
                let radius =
                    target_radius + unit_signed(random.rotate_left(17)) * target_radius_jitter;
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
        let palette = *self.palette();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let morph_start = self.param("morph_start");
        let morph_end = self.param("morph_end");
        let return_start = self.param("return_start");
        let arc_min = self.param("arc_min");
        let arc_span = self.param("arc_span");
        let blend = if seconds < morph_start {
            0.0
        } else if seconds < morph_end {
            ease_out_cubic((seconds - morph_start) / (morph_end - morph_start))
        } else if seconds < return_start {
            1.0
        } else {
            1.0 - ease_out_cubic(
                (seconds - return_start)
                    / (self.recipe_for(self.demo).duration_ms as f32 / 1000.0 - return_start),
            )
        };
        let arc = (blend * std::f32::consts::PI).sin();
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let mut clipped = 0usize;
        for index in (0..self.pool.active()).step_by(2) {
            let source_x = self.pool.previous_x[index];
            let source_y = self.pool.previous_y[index];
            let target_x = self.pool.x[index];
            let target_y = self.pool.y[index];
            let x = center_x + source_x + (target_x - source_x) * blend;
            let y = center_y + source_y + (target_y - source_y) * blend
                - arc * (arc_min + self.pool.age[index] * arc_span);
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
                    x1: center_x + target_x,
                    y1: center_y + target_y,
                    start_radius: 1,
                    end_radius: 1,
                    intensity: 6,
                    color: palette[usize::from(self.pool.style[index])],
                });
            }
        }
        clipped
    }

    fn initialize_depth_aware_material_lod(&mut self) {
        let x_span = self.param("spawn_x_span");
        let y_span = self.param("spawn_y_span");
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random) * x_span;
            self.pool.y[index] = unit_signed(random.rotate_left(11)) * y_span;
            self.pool.z[index] = unit01(random.rotate_left(21));
            self.pool.age[index] = unit01(random.rotate_left(7));
            self.pool.life[index] = 0.6 + unit01(random.rotate_left(17)) * 0.8;
            self.pool.style[index] = ((random >> 29) & 7) as u8;
        }
    }

    fn project_depth_aware_material_lod(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let palette = *self.palette();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let depth_rate = self.param("depth_rate");
        let parallax_min = self.param("parallax_min");
        let parallax_span = self.param("parallax_span");
        let drift_min = self.param("drift_min");
        let drift_span = self.param("drift_span");
        let corridor_min = self.param("corridor_min");
        let corridor_span = self.param("corridor_span");
        let width = self.config.width as f32;
        let center_x = width * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let mut clipped = 0usize;
        for index in (0..self.pool.active()).step_by(2) {
            let depth =
                (self.pool.z[index] + seconds * self.pool.life[index] * depth_rate).rem_euclid(1.0);
            let parallax = parallax_min + depth * parallax_span;
            let drift = seconds * (drift_min + depth * drift_span) * self.pool.life[index];
            let x = (center_x + self.pool.x[index] * parallax + drift + width).rem_euclid(width);
            let corridor = corridor_min + (1.0 - depth) * corridor_span;
            let mut y = center_y + self.pool.y[index] * parallax;
            if y > center_y - corridor && y < center_y + corridor {
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
                    color: palette[style],
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
                        color: palette[style],
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
            self.pool.x[index] = unit_signed(random) * self.param("spawn_x_span");
            self.pool.y[index] = unit_signed(random.rotate_left(11)) * self.param("spawn_y_span");
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
        let x_scale = self.config.width as f32 / REFERENCE_WIDTH as f32;
        let center_y = self.config.height as f32 * 0.5;
        let mut clipped = 0usize;
        for index in (0..self.pool.active()).step_by(2) {
            let lane = self.pool.flags[index];
            let phase = (seconds * self.param("phase_rate") * self.pool.life[index]
                + self.pool.age[index])
                .fract();
            let base_x = self.pool.x[index];
            let base_y = self.pool.y[index];
            let (center, x, y, style) = match lane {
                0 => {
                    let contraction = 0.22 + (1.0 - phase) * 0.78;
                    let angle = seconds * 0.17 + self.pool.age[index] * 4.0;
                    let rotated_x = base_x * angle.cos() - base_y * 0.22 * angle.sin();
                    let rotated_y = base_x * angle.sin() * 0.35 + base_y * angle.cos();
                    (
                        self.param("left_center"),
                        rotated_x * contraction,
                        rotated_y * contraction,
                        5,
                    )
                }
                1 => {
                    let radius = (base_x * base_x + base_y * base_y).sqrt().max(1.0);
                    let cavity = self.param("cavity_min") + phase * self.param("cavity_span");
                    let scale = cavity.max(radius) / radius;
                    (
                        self.param("middle_center"),
                        base_x * scale,
                        base_y * scale,
                        6,
                    )
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
                    (
                        self.param("right_center"),
                        rotated_x * capture,
                        rotated_y * capture,
                        4,
                    )
                }
            };
            if !push_screen_command(
                &mut self.commands,
                self.config.width,
                self.config.height,
                center * x_scale + x,
                center_y + y,
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
        let palette = *self.palette();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let cycle_seconds = self.param("cycle");
        let cycle = seconds.rem_euclid(cycle_seconds) / cycle_seconds;
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        for parent in 0..3usize {
            let offset = parent as f32 - 1.0;
            let head_x = center_x
                + offset * self.param("parent_spacing")
                + (cycle * std::f32::consts::TAU).sin() * self.param("head_x_amplitude");
            let head_y = self.config.height as f32 - 120.0 - cycle * self.param("head_travel")
                + (cycle * std::f32::consts::PI).sin() * (-72.0 - parent as f32 * 18.0);
            let color = palette[3 + parent];
            let mut previous = None;
            let trail_samples = self.param("trail_length") as usize;
            for sample in 0..trail_samples {
                let trail = sample as f32 / (trail_samples - 1).max(1) as f32;
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
            let ring_age = (cycle * self.param("ring_growth") + parent as f32 * 0.27).fract();
            if ring_age < 0.58 {
                let ring_radius = self.param("ring_radius") + ring_age * 90.0;
                for ring in 0..32usize {
                    let angle = std::f32::consts::TAU * ring as f32 / 32.0;
                    self.material_stamps.push(MaterialStamp {
                        x: (head_x + angle.cos() * ring_radius) as i16,
                        y: (head_y + angle.sin() * ring_radius) as i16,
                        radius: 1,
                        intensity: (13.0 - ring_age * 14.0) as u8,
                        color: palette[5],
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
            let origin_x = center_x + parent_offset * 215.0;
            let origin_y = center_y - 80.0 + parent as f32 * 18.0;
            let angle = std::f32::consts::TAU * unit01(random.rotate_left(11));
            let speed = 18.0 + unit01(random.rotate_left(21)) * 105.0;
            let x = origin_x + angle.cos() * speed * release;
            let y = origin_y
                + angle.sin() * speed * release
                + self.param("child_gravity") * release * release;
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
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let density_width = self.density_width;
        let density_height = self.density_height;
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let pulse = self.param("pulse_base")
            + (seconds * self.param("pulse_rate")).sin() * self.param("pulse_amplitude");
        for index in (0..self.pool.active()).step_by(4) {
            let random = self.pool.random[index];
            let angle = std::f32::consts::TAU
                * (unit01(random) + seconds * (0.006 + self.pool.age[index] * 0.004));
            let radial = (0.45 + unit01(random.rotate_left(11)) * 0.55).sqrt();
            let radius = (self.param("radius_min") + radial * self.param("radius_span")) * pulse;
            let world_x = angle.cos() * radius + self.param("x_offset");
            let world_y = angle.sin() * radius * self.param("y_scale");
            let cavity_x = world_x + 58.0;
            let cavity_y = world_y;
            if cavity_x * cavity_x + cavity_y * cavity_y < self.param("cavity_radius").powi(2) {
                continue;
            }
            let x = ((center_x + world_x) / DENSITY_SCALE as f32) as i32;
            let y = ((center_y + world_y) / DENSITY_SCALE as f32) as i32;
            if !(1..density_width as i32 - 1).contains(&x)
                || !(1..density_height as i32 - 1).contains(&y)
            {
                continue;
            }
            let center = y as usize * density_width + x as usize;
            let weight = 6 + u16::from(self.pool.style[index]);
            for (offset, scale) in [
                (0isize, 4u16),
                (-1, 2),
                (1, 2),
                (-(density_width as isize), 2),
                (density_width as isize, 2),
            ] {
                let cell = (center as isize + offset) as usize;
                self.density[cell] = self.density[cell].saturating_add(weight * scale);
            }
            if self.pool.flags[index] != 0 {
                let _ = push_screen_command(
                    &mut self.commands,
                    self.config.width,
                    self.config.height,
                    center_x + world_x,
                    center_y + world_y,
                    7,
                    true,
                );
            }
        }
        self.density_blur.fill(0);
        for y in 1..density_height - 1 {
            for x in 1..density_width - 1 {
                let offset = y * density_width + x;
                self.density_blur[offset] = (self.density[offset] * 2
                    + self.density[offset - 1]
                    + self.density[offset + 1]
                    + self.density[offset - density_width]
                    + self.density[offset + density_width])
                    / 6;
            }
        }
        std::mem::swap(&mut self.density, &mut self.density_blur);
        0
    }

    fn initialize_curl_noise_flow_field(&mut self) {
        let margin = 24.0;
        let spawn_width = (self.config.width as f32 - margin * 2.0).max(1.0);
        let spawn_height = (self.config.height as f32 - margin * 2.0).max(1.0);
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = margin + unit01(random) * spawn_width;
            self.pool.y[index] = margin + unit01(random.rotate_left(11)) * spawn_height;
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
        let width = self.config.width as f32;
        let height = self.config.height as f32;
        let center_x = width * 0.5;
        let center_y = height * 0.5;
        let cohort = ((elapsed.as_micros() / 16_667) & 3) as usize;
        for index in (cohort..self.pool.active()).step_by(4) {
            let normalized_x = (self.pool.x[index] - center_x) / self.param("normal_x");
            let normalized_y = (self.pool.y[index] - center_y) / self.param("normal_y");
            let phase = seconds * self.param("phase_rate") + self.pool.age[index] * 9.0;
            let left_dx = normalized_x + self.param("vortex_offset");
            let right_dx = normalized_x - self.param("vortex_offset");
            let left_radius =
                (left_dx * left_dx + normalized_y * normalized_y + self.param("softening")).recip();
            let right_radius =
                (right_dx * right_dx + normalized_y * normalized_y + self.param("softening"))
                    .recip();
            let vx = -normalized_y * left_radius + normalized_y * right_radius
                - (normalized_y * 2.7 - phase * 0.29).cos() * 0.52
                + 0.32;
            let vy = left_dx * left_radius - right_dx * right_radius
                + (normalized_x * 2.1 + phase * 0.37).sin() * 0.48;
            self.pool.vx[index] = vx * self.param("velocity_scale");
            self.pool.vy[index] = vy * self.param("velocity_scale");
            self.pool.x[index] = (self.pool.x[index]
                + self.pool.vx[index] * dt * self.param("integration_scale")
                + width)
                .rem_euclid(width);
            self.pool.y[index] = (self.pool.y[index]
                + self.pool.vy[index] * dt * self.param("integration_scale")
                + height)
                .rem_euclid(height);
        }
    }

    fn project_curl_noise_flow_field(&mut self, _elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let palette = *self.palette();
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
                    x0: x - self.pool.vx[index] * self.param("trail_scale"),
                    y0: y - self.pool.vy[index] * self.param("trail_scale"),
                    x1: x,
                    y1: y,
                    start_radius: 1,
                    end_radius: 2,
                    intensity: 13,
                    color: palette[style],
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
        let palette = *self.palette();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let ribbon_count = self.param("ribbon_count") as usize;
        let ribbon_samples = self.param("ribbon_samples") as usize;
        for ribbon in 0..ribbon_count {
            let lane = ribbon % palette.len();
            let random = self.pool.random[ribbon];
            let depth = unit01(random.rotate_left(9));
            let hero = ribbon % 4 == 0;
            let lane_offset =
                (ribbon as f32 - (ribbon_count as f32 - 1.0) * 0.5) * (3.2 + depth * 1.6);
            let start_t = unit01(random.rotate_left(19)) * 0.18;
            let end_t = 0.76
                + unit01(random.rotate_left(29)) * 0.24
                + (seconds * self.param("motion_rate") + ribbon as f32).sin() * 0.018;
            let mut previous = None;
            for sample in 0..ribbon_samples {
                let progress = sample as f32 / (ribbon_samples - 1).max(1) as f32;
                let t = start_t + (end_t - start_t) * progress;
                let u = t * 2.0 - 1.0;
                let x = center_x + u * (self.param("path_x") + depth * self.param("depth_x"));
                let y = center_y
                    - (u * std::f32::consts::PI).sin()
                        * (self.param("path_y") + depth * self.param("depth_y"))
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
                        color: palette[lane],
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
                            color: palette[lane.saturating_sub(1)],
                        });
                    }
                }
                previous = Some((x, y));
            }
            let head_u = end_t * 2.0 - 1.0;
            let head_x = center_x + head_u * (self.param("path_x") + depth * self.param("depth_x"));
            let head_y = center_y
                - (head_u * std::f32::consts::PI).sin()
                    * (self.param("path_y") + depth * self.param("depth_y"))
                + lane_offset * (head_u * std::f32::consts::PI).cos();
            self.material_stamps.push(MaterialStamp {
                x: head_x as i16,
                y: head_y as i16,
                radius: if hero { 4 } else { 2 },
                intensity: 15,
                color: palette[lane],
                shape: MaterialShape::Star,
            });
        }
        for streak in 0..self.param("streak_count") as usize {
            let index = ribbon_count + streak;
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
                color: palette[usize::from(self.pool.style[index])],
            });
        }
        0
    }

    fn initialize_procedural_sprite_materials(&mut self) {
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            self.pool.x[index] = unit_signed(random.rotate_left(3)) * self.param("spawn_x_span");
            self.pool.y[index] = unit_signed(random.rotate_left(13)) * self.param("spawn_y_span");
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
            let phase = (seconds * self.pool.life[index] * self.param("phase_rate")
                + self.pool.age[index])
                .rem_euclid(1.0);
            let random = self.pool.random[index];
            let shape_lane = self.pool.flags[index];
            let angle = unit_signed(random.rotate_left(11)) * self.param("angle_span")
                - std::f32::consts::FRAC_PI_2
                + (seconds * self.param("angle_rate") + self.pool.age[index] * 9.0).sin() * 0.08;
            let speed =
                self.param("speed_min") + unit01(random.rotate_left(21)) * self.param("speed_span");
            let travel = phase * speed;
            let spread = 0.45 + f32::from(shape_lane) * 0.11;
            let x = center_x + angle.cos() * travel * spread + self.pool.x[index] * phase * 0.28;
            let y = center_y + angle.sin() * travel + self.param("gravity") * phase * phase;
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
                self.recipe_for(self.demo).color("highlight")
            } else {
                match shape {
                    MaterialShape::Star => self.recipe_for(self.demo).color("star"),
                    MaterialShape::Smoke => self.recipe_for(self.demo).color("smoke"),
                    MaterialShape::Shard => self.recipe_for(self.demo).color("shard"),
                    _ => self.palette()[style],
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
            self.pool.previous_x[index] =
                unit_signed(random.rotate_left(3)) * self.param("source_x_span");
            self.pool.previous_y[index] =
                unit_signed(random.rotate_left(13)) * self.param("source_y_span");
            self.pool.previous_z[index] =
                unit_signed(random.rotate_left(23)) * self.param("source_z_span");
        }
    }

    fn project_arcade_cabinet(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let (formation, yaw, pitch, dolly, dispersal) = arcade_camera(
            seconds,
            self.recipe_for(self.demo).duration_ms as f32 / 1000.0,
            self.param("formation_end"),
            self.param("orbit_end"),
            self.param("return_end"),
        );
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5 + self.param("center_y_offset");
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
            let scale = self.param("focal") / depth;
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
            let radial_speed = self.param("radial_speed_min")
                + unit01(random.rotate_left(15)) * self.param("radial_speed_span");
            self.pool.x[index] = unit_signed(random.rotate_left(25)) * 72.0;
            self.pool.y[index] = unit_signed(random.rotate_left(9)) * 18.0;
            self.pool.z[index] = unit_signed(random.rotate_left(19)) * 92.0;
            self.pool.vx[index] = cos_angle * radial_speed;
            self.pool.vy[index] = self.param("vertical_speed")
                - unit01(random.rotate_left(13)) * self.param("vertical_speed_span");
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
            let fountain_end = self.param("fountain_end");
            let morph_end = self.param("morph_end");
            let waterfall = self.waterfall_particle(index, (seconds - fountain_end).max(0.0));
            let (world_x, world_y, world_z, style, neighbor) = if seconds < fountain_end {
                fountain
            } else if seconds < morph_end {
                let blend = ease_out_cubic(
                    (seconds - fountain_end) / (morph_end - fountain_end).max(0.001),
                );
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
                self.waterfall_particle(index, seconds - morph_end)
            };
            let camera_x = if seconds < fountain_end {
                0.0
            } else {
                ease_out_cubic(
                    ((seconds - fountain_end) / (morph_end - fountain_end).max(0.001)).min(1.0),
                ) * 72.0
            };
            if let Some((x, y)) = project_world(
                world_x - camera_x,
                world_y,
                world_z,
                self.config.width,
                self.config.height,
                self.param("camera_z"),
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
        let fountain_end = self.param("fountain_end");
        if seconds < fountain_end + 2.0 {
            self.append_fountain_basin(seconds);
        }
        if seconds >= fountain_end + 1.0 {
            self.append_waterfall_edges(seconds);
        }
        clipped
    }

    fn fountain_particle(&self, index: usize, seconds: f32) -> (f32, f32, f32, u8, bool) {
        let age = (seconds * self.pool.life[index] + self.pool.age[index]).rem_euclid(2.4);
        let drag = 1.0 / (1.0 + age * 0.16);
        let x = self.pool.vx[index] * age * drag;
        let y = 196.0 + self.pool.vy[index] * age * drag + self.param("gravity") * age * age;
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
            self.pool.x[index] = unit_signed(random.rotate_left(5)) * self.param("cloud_x_span");
            self.pool.y[index] = unit_signed(random.rotate_left(15)) * self.param("cloud_y_span");
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
        let charge = ((seconds * self.param("charge_rate")).sin() * 0.5 + 0.5)
            .powf(self.param("charge_power"));
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

        if seconds >= self.param("leader_start") {
            let epoch = (seconds
                * if seconds < self.param("bright_start") {
                    1.5
                } else {
                    4.0
                }) as u32;
            let seed = xorshift32(epoch ^ fold_seed(self.config.seed, self.demo));
            let bright = seconds >= self.param("bright_start");
            let branches = seconds >= self.param("branch_start");
            self.append_lightning_bolt(seed, bright, branches);
            if branches {
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
            let next_x = (x + unit_signed(state) * self.param("bolt_x_jitter")).clamp(36.0, 924.0);
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
            let minor_radius = self.param("minor_radius_min")
                + unit01(random.rotate_left(23)) * self.param("minor_radius_span");
            let radius = self.param("major_radius") + minor_angle.cos() * minor_radius;
            self.pool.x[index] = major_angle.cos() * radius;
            self.pool.y[index] = major_angle.sin() * radius;
            self.pool.z[index] = minor_angle.sin() * minor_radius;
            self.pool.age[index] = unit01(random.rotate_left(11));
            self.pool.style[index] = 3 + ((random >> 29) as u8).min(4);
            // Rim particles are handled by the `index & 63 == 0` branch in
            // projection. Keep tendril and highlight samples on the projected
            // lanes so those effects are not silently discarded as rim points.
            self.pool.flags[index] =
                band as u8 | (u8::from(index & 127 == 8) << 1) | (u8::from(index & 511 == 16) << 2);
        }
    }

    fn project_particle_portal(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let forward_angle = seconds * self.param("forward_rate");
        let reverse_angle = seconds * self.param("reverse_rate");
        let (forward_sin, forward_cos) = forward_angle.sin_cos();
        let (reverse_sin, reverse_cos) = reverse_angle.sin_cos();
        let (previous_forward_sin, previous_forward_cos) = (-0.035_f32).sin_cos();
        let (previous_reverse_sin, previous_reverse_cos) = 0.035_f32.sin_cos();
        let (tilt_sin, tilt_cos) = self.param("tilt").sin_cos();
        let pulse = 0.94 + ((seconds * self.param("pulse_rate")).sin() * 0.5 + 0.5) * 0.12;
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
                let camera_z = self.param("camera_z");
                let scale = camera_z / (camera_z + depth_axis);
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
            let camera_z = self.param("camera_z");
            let depth = camera_z + depth_axis;
            let scale = camera_z / depth.max(96.0);
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
            self.pool.x[index] = unit_signed(random.rotate_left(3)) * self.param("spawn_x_span");
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
        let rain_end = self.param("rain_end");
        let snow_end = self.param("snow_end");
        if seconds < rain_end {
            self.project_weather_rain(seconds)
        } else if seconds < snow_end {
            self.project_weather_snow(seconds - rain_end)
        } else {
            self.project_weather_ash(seconds - snow_end)
        }
    }

    fn project_weather_rain(&mut self, seconds: f32) -> usize {
        let mut clipped = 0usize;
        let wind = self.param("rain_wind")
            + triangle_wave(seconds * 0.08) * self.param("rain_wind_amplitude");
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
        let gust = triangle_wave(seconds * 0.055) * self.param("snow_gust");
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
        let wind = triangle_wave(seconds * 0.07) * self.param("ash_wind");
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
            self.pool.x[index] = unit_signed(random.rotate_left(3)) * self.param("star_x_span");
            self.pool.y[index] = unit_signed(random.rotate_left(13)) * self.param("star_y_span");
            self.pool.z[index] =
                unit01(random.rotate_left(23)) * self.param("star_camera_z") - 80.0;
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
            let phase = unit01(random.rotate_left(27)) * self.param("track_cycle");
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
        let (sin_drift, cos_drift) = (seconds * self.param("star_drift_rate")).sin_cos();
        let center_x = self.config.width as f32 * 0.5;
        let center_y = self.config.height as f32 * 0.5;
        let mut clipped = 0usize;

        for index in 0..star_count {
            let x = self.pool.x[index];
            let z = self.pool.z[index];
            let rotated_x = x.mul_add(cos_drift, z * sin_drift);
            let rotated_z = (-x).mul_add(sin_drift, z * cos_drift);
            let camera_z = self.param("star_camera_z");
            let depth = camera_z + rotated_z;
            let scale = camera_z / depth.max(96.0);
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

        let quarter_end = self.recipe_for(self.demo).phase_until_seconds(0);
        let half_end = self.recipe_for(self.demo).phase_until_seconds(1);
        let active_tracks = if seconds < quarter_end {
            METEOR_TRACK_COUNT / 4
        } else if seconds < half_end {
            METEOR_TRACK_COUNT / 2
        } else {
            METEOR_TRACK_COUNT
        };
        let focal = self.param("focal");
        let radiant_x = center_x + self.param("radiant_x");
        let radiant_y = center_y + self.param("radiant_y");
        for track in 0..METEOR_TRACK_COUNT {
            let first = star_count + track * METEOR_TRAIL_SAMPLES;
            if track >= active_tracks {
                self.commands
                    .resize(self.commands.len() + METEOR_TRAIL_SAMPLES, u32::MAX);
                continue;
            }
            let random = self.pool.random[first];
            let rate = 0.82 + unit01(random.rotate_left(11)) * 0.34;
            let age = (seconds * rate + self.pool.z[first]).rem_euclid(self.param("track_cycle"));
            let head_depth = self.param("head_depth") - age * self.param("depth_speed");
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
            self.pool.x[index] = unit_signed(random.rotate_left(5)) * self.param("spawn_x_span");
            self.pool.y[index] = unit_signed(random.rotate_left(17)) * self.param("spawn_y_span");
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
        let (travel, speed) = warp_travel_and_speed(
            seconds,
            self.recipe_for(self.demo).duration_ms as f32 / 1000.0,
            self.param("accelerate_end"),
            self.param("cruise_end"),
            self.param("calm_end"),
            self.param("min_speed"),
            self.param("max_speed"),
        );
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
            let numerator = self.param("projection_numerator");
            let bias = self.param("projection_bias");
            let scale = numerator / (bias + depth);
            let previous_scale = numerator / (bias + previous_depth);
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
        let bulge_count = (self.pool.active() as f32 * self.param("bulge_fraction")) as usize;
        for index in 0..self.pool.active() {
            let random = self.pool.random[index];
            let azimuth = std::f32::consts::TAU * unit01(random);
            if index < bulge_count {
                let radius = self.param("bulge_radius") * unit01(random.rotate_left(7)).cbrt();
                let vertical = unit_signed(random.rotate_left(17));
                let planar = (1.0 - vertical * vertical).max(0.0).sqrt() * radius;
                self.pool.x[index] = azimuth.cos() * planar;
                self.pool.y[index] = vertical * radius * 0.68;
                self.pool.z[index] = azimuth.sin() * planar;
                self.pool.style[index] = 6 + u8::from(index & 15 == 0);
                self.pool.flags[index] = 1 | (u8::from(index & 255 == 0) << 1);
                continue;
            }

            let radius = self.param("arm_inner_radius")
                + unit01(random.rotate_left(5)).sqrt() * self.param("arm_radial_span");
            let arm = (random.rotate_left(11) & 3) as f32;
            let uneven = unit_signed(random.rotate_left(19)) * (0.16 + radius * 0.0007);
            let angle = arm * std::f32::consts::FRAC_PI_2
                + (radius / self.param("arm_inner_radius")).ln() * self.param("arm_winding")
                + uneven;
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
        let (sin_tilt, cos_tilt) = self.param("tilt").sin_cos();
        for index in 0..self.pool.active() {
            let y = self.pool.y[index];
            let z = self.pool.z[index];
            let tilted_y = y.mul_add(cos_tilt, -(z * sin_tilt));
            let tilted_z = y.mul_add(sin_tilt, z * cos_tilt);
            let camera_z = self.param("camera_z");
            let perspective = camera_z / (camera_z + tilted_z);
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
        let (sin_yaw, cos_yaw) = (seconds * self.param("yaw_rate")).sin_cos();
        let core_pulse =
            ((seconds * self.param("core_pulse_rate")).sin() * 0.5 + 0.5) * 0.18 + 0.82;
        let bulge_count = (self.pool.active() as f32 * self.param("bulge_fraction")) as usize;
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
        let heat_width = self.heat_width;
        let heat_height = self.heat_height;
        let bottom = (heat_height - 1) * heat_width;
        for x in 0..heat_width {
            let centered = (x as f32 - heat_width as f32 * 0.5).abs() / (heat_width as f32 * 0.5);
            let envelope = ((1.0 - centered).max(0.0) * 72.0) as u8;
            let hash = xorshift32(
                (frame as u32)
                    .wrapping_mul(0x9e37_79b9)
                    .wrapping_add((x as u32).wrapping_mul(0x045d_9f3b)),
            );
            let flicker = ((hash >> 25) & 0x7f) as u8;
            self.heat[bottom + x] = 150u8.saturating_add(envelope).saturating_add(flicker);
        }
        for y in 0..heat_height - 1 {
            let row = y * heat_width;
            let source_row = row + heat_width;
            for x in 0..heat_width {
                let hash = xorshift32(
                    (frame as u32)
                        .wrapping_add((y * heat_width + x) as u32)
                        .wrapping_mul(0x85eb_ca6b),
                );
                let drift = match hash & 3 {
                    0 => -1,
                    1 => 1,
                    _ => 0,
                };
                let source_x = (x as isize + drift).clamp(0, heat_width as isize - 1) as usize;
                let cooling = ((hash >> 8) & 7) as u8;
                self.heat[row + x] = self.heat[source_row + source_x].saturating_sub(cooling);
            }
        }
    }

    fn project_fire_embers(&mut self, elapsed: Duration) -> usize {
        self.commands.clear();
        self.segments.clear();
        let seconds = elapsed.saturating_sub(self.demo_started_at).as_secs_f32();
        let wind = (seconds * self.param("wind_rate_fast")).sin()
            * self.param("wind_amplitude_fast")
            + (seconds * self.param("wind_rate_slow")).sin() * self.param("wind_amplitude_slow");
        let mut clipped = 0usize;
        let live_embers = self.pool.active() / 4;
        for ember in 0..live_embers {
            let index = ember * 4;
            let random = self.pool.random[index];
            let age = (seconds * (0.72 + unit01(random.rotate_left(13)) * 0.38)
                + unit01(random) * self.param("ember_life"))
            .rem_euclid(self.param("ember_life"));
            if age < 0.12 {
                continue;
            }
            let base_x = unit_signed(random.rotate_left(7)) * self.param("ember_x_span");
            let turbulence = unit_signed(random.rotate_left(19)) * age * age * 7.0;
            let x = base_x + wind * age * 0.12 + turbulence;
            let y = 252.0 - age * (67.0 + unit01(random.rotate_left(3)) * 31.0);
            let z = unit_signed(random.rotate_left(23)) * 72.0 + age * 5.0;
            let style = ((1.0 - age / self.param("ember_life")) * 7.0).clamp(2.0, 7.0) as u8;
            let Some((screen_x, screen_y)) = project_world(
                x,
                y,
                z,
                self.config.width,
                self.config.height,
                self.param("camera_z"),
            ) else {
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
            // Renderer families such as fireworks intentionally have no
            // simulation-pool particles. Derive transition jitter from the
            // rendered command instead of indexing a possibly empty pool.
            let random = xorshift32(
                fold_seed(self.config.seed, self.demo)
                    .wrapping_add((transition_index as u32).wrapping_mul(0x9e37_79b9))
                    .wrapping_add(command.rotate_left(11)),
            );
            let jitter = unit_signed(random.rotate_left(7));
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
                self.palette(),
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
            for cell_y in 0..self.density_height {
                for cell_x in 0..self.density_width {
                    let density = self.density[cell_y * self.density_width + cell_x];
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
                    let color = self.palette()[style];
                    for y in cell_y * DENSITY_SCALE
                        ..((cell_y + 1) * DENSITY_SCALE).min(self.config.height)
                    {
                        let row = y * self.config.width;
                        for x in cell_x * DENSITY_SCALE
                            ..((cell_x + 1) * DENSITY_SCALE).min(self.config.width)
                        {
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
            .saturating_sub(self.heat_height * FIRE_HEAT_SCALE);
        let mut writes = 0usize;
        for heat_y in 0..self.heat_height {
            for heat_x in 0..self.heat_width {
                let style = usize::from(self.heat[heat_y * self.heat_width + heat_x] >> 5).min(7);
                let color = self.palette()[style];
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
            let color = self.palette()[style];
            destination[offset] = color;
            dirty_offsets.push(offset as u32);
            writes = writes.saturating_add(1);
            if command & COMMAND_NEIGHBOR != 0 {
                let neighbor_color = if self.demo == ParticleDemoKind::LayerMappedHologram {
                    color
                } else {
                    self.palette()[style.saturating_sub(1)]
                };
                destination[offset + 1] = neighbor_color;
                dirty_offsets.push((offset + 1) as u32);
                writes = writes.saturating_add(1);
                if self.demo == ParticleDemoKind::LayerMappedHologram
                    && offset + self.config.width + 1 < destination.len()
                {
                    destination[offset + self.config.width] = color;
                    destination[offset + self.config.width + 1] = color;
                    dirty_offsets.push((offset + self.config.width) as u32);
                    dirty_offsets.push((offset + self.config.width + 1) as u32);
                    writes = writes.saturating_add(2);
                }
            }
            visible = visible.saturating_add(1);
        }
        (visible, writes)
    }
}

fn read_live_family_now(path: &Path) -> Result<ParticleRecipeFamily, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("inspect live particle family {}: {error}", path.display()))?;
    if metadata.len() > LIVE_PARTICLE_MAX_FILE_BYTES as u64 {
        return Err(format!(
            "{} exceeds the {} byte live-particle limit",
            path.display(),
            LIVE_PARTICLE_MAX_FILE_BYTES
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("read live particle family {}: {error}", path.display()))?;
    if bytes.len() > LIVE_PARTICLE_MAX_FILE_BYTES {
        return Err(format!(
            "{} exceeds the {} byte live-particle limit",
            path.display(),
            LIVE_PARTICLE_MAX_FILE_BYTES
        ));
    }
    ParticleRecipeFamily::from_json(&bytes)
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
        let Some(frame_len) = self.width.checked_mul(self.height) else {
            return Err(format!(
                "particle showcase geometry {}x{} overflows its frame length",
                self.width, self.height
            ));
        };
        if self.width < 16 || self.height < 16 {
            return Err(format!(
                "particle showcase geometry must be at least 16x16, received {}x{}",
                self.width, self.height
            ));
        }
        if frame_len > COMMAND_OFFSET_MASK as usize + 1 {
            return Err(format!(
                "particle showcase geometry {}x{} exceeds the packed command offset capacity",
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

    #[cfg(test)]
    pub(crate) fn reset(&mut self, kind: ParticleDemoKind, seed: u64) {
        self.reset_with_count(kind, seed, kind.starting_count());
    }

    pub(crate) fn reset_with_count(&mut self, kind: ParticleDemoKind, seed: u64, active: usize) {
        self.active = active.min(PARTICLE_DEMO_MAX_COUNT);
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

fn arcade_camera(
    seconds: f32,
    duration: f32,
    formation_end: f32,
    orbit_end: f32,
    return_end: f32,
) -> (f32, f32, f32, f32, f32) {
    let formation = ease_out_cubic((seconds / formation_end).clamp(0.0, 1.0));
    if seconds < formation_end {
        return (
            formation,
            std::f32::consts::FRAC_PI_2 - 0.62,
            -0.08,
            760.0,
            0.0,
        );
    }
    if seconds < orbit_end {
        let phase = (seconds - formation_end) / (orbit_end - formation_end);
        return (
            1.0,
            std::f32::consts::FRAC_PI_2 - 0.62 + (phase * std::f32::consts::TAU).sin() * 1.15,
            triangle_wave(phase * 2.0) * 0.13,
            720.0 + triangle_wave(phase + 0.25) * 82.0,
            0.0,
        );
    }
    if seconds < return_end {
        let return_t = ease_out_cubic((seconds - orbit_end) / (return_end - orbit_end));
        return (
            1.0,
            (std::f32::consts::FRAC_PI_2 - 0.62) * (1.0 - return_t) + 0.72 * return_t,
            0.11 * return_t,
            720.0 + 35.0 * return_t,
            0.0,
        );
    }
    (
        1.0,
        0.72,
        0.11,
        755.0,
        ((seconds - return_end) / (duration - return_end)).clamp(0.0, 1.0),
    )
}

fn warp_travel_and_speed(
    seconds: f32,
    duration: f32,
    accelerate_end: f32,
    cruise_end: f32,
    calm_end: f32,
    min_speed: f32,
    max_speed: f32,
) -> (f32, f32) {
    let cycle = seconds.rem_euclid(duration);
    let calm_distance = calm_end * min_speed;
    let acceleration_duration = accelerate_end - calm_end;
    let acceleration = (max_speed - min_speed) / acceleration_duration;
    let accelerate_distance = calm_distance
        + min_speed * acceleration_duration
        + 0.5 * acceleration * acceleration_duration * acceleration_duration;
    let cruise_distance = accelerate_distance + (cruise_end - accelerate_end) * max_speed;
    let deceleration_duration = duration - cruise_end;
    let deceleration = (max_speed - min_speed) / deceleration_duration;
    let (distance, speed) = if cycle < calm_end {
        (cycle * min_speed, min_speed)
    } else if cycle < accelerate_end {
        let time = cycle - calm_end;
        (
            calm_distance + min_speed * time + 0.5 * acceleration * time * time,
            min_speed + acceleration * time,
        )
    } else if cycle < cruise_end {
        (
            accelerate_distance + (cycle - accelerate_end) * max_speed,
            max_speed,
        )
    } else {
        let time = cycle - cruise_end;
        (
            cruise_distance + max_speed * time - 0.5 * deceleration * time * time,
            max_speed - deceleration * time,
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
        demo if demo.form_scene().is_some() => &form_recipe(demo.telemetry_label()).palette,
        demo => &procedural_recipe(demo.telemetry_label()).palette,
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

    fn frame_signature(frame: &[Rgb565Pixel]) -> u64 {
        frame.iter().fold(0xcbf2_9ce4_8422_2325, |hash, pixel| {
            pixel.0.to_le_bytes().into_iter().fold(hash, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            })
        })
    }

    #[test]
    fn demo_order_and_wrapping_are_stable() {
        assert_eq!(ParticleDemoKind::ALL.len(), 36);
        assert_eq!(
            ParticleDemoKind::SolarChrysanthemum.offset_wrapped(-1),
            ParticleDemoKind::PointCloudMorphPassage
        );
        assert_eq!(
            ParticleDemoKind::PointCloudMorphPassage.offset_wrapped(1),
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
        assert_eq!(
            ParticleDemoKind::parse("32"),
            Some(ParticleDemoKind::FractalGridTerrain)
        );
        assert_eq!(
            ParticleDemoKind::parse("36"),
            Some(ParticleDemoKind::PointCloudMorphPassage)
        );
        assert_eq!(ParticleDemoKind::parse("37"), None);
        assert_eq!(ParticleDemoKind::parse("unknown"), None);
    }

    #[test]
    fn embedded_showcase_defaults_match_golden_hero_frames() {
        let expected = [
            0x6de5_4005_6f5c_1047,
            0x4288_b811_5aa4_cc53,
            0x1cf8_4dcb_b9f9_8b25,
            0xe9a1_f13e_eff2_392f,
            0x3cb9_b01c_914c_87be,
            0xb163_f487_a847_2d45,
            0x4125_8c0b_def3_f401,
            0x1591_d78d_e78c_453a,
            0x860e_d103_2073_9183,
            0x17dc_2485_fc1e_165b,
            0x59af_52d3_eb30_48bd,
            0xdcb8_666b_b420_5b26,
            0x8594_ee01_66e1_ae68,
            0x2246_e8e4_add0_648d,
            0xc6e1_ab4d_12fa_b44a,
            0x2e38_4232_bc0d_ac30,
            0xb5e7_ba49_c685_98f1,
            0x54d3_ada9_2e92_16a7,
            0x4295_03ed_7b11_b53e,
            0xbb50_9235_7fe4_3817,
            0xac1d_5455_b6dc_5fad,
            0xae84_dbba_c3b2_ae03,
            0x365e_8cdc_e616_3153,
            0xb942_3be1_67c6_87e3,
            0x5c8b_ea52_b5c3_f9d5,
            0x37ca_6c5b_0924_a8a3,
            0x68b0_4368_7bbe_52b3,
            0x4e49_d829_56f3_3e32,
            0xabaa_b094_0768_696a,
            0x7a53_fea3_ce50_7236,
            0xf235_37e7_00a6_58d4,
            0x5f0a_4a86_d499_02cc,
            0xf610_4319_497f_a537,
            0x1b39_f651_80b2_b75b,
            0xba86_6b65_2c31_2281,
            0x57a7_8a53_616b_1622,
        ];
        for (kind, expected_signature) in ParticleDemoKind::ALL.into_iter().zip(expected) {
            let config = ParticleShowcaseConfig {
                width: 960,
                height: 540,
                seed: 827_141_709_451,
                initial_demo: kind,
            };
            let mut renderer = ParticleShowcaseRenderer::new(config).unwrap();
            renderer.configure_capture_hud(false);
            let mut frame = vec![Rgb565Pixel(0); 960 * 540];
            let elapsed = Duration::from_secs(15);

            let stats = renderer.render(&mut frame, 1, elapsed).unwrap();

            assert_eq!(stats.demo, kind);
            assert_eq!(frame_signature(&frame), expected_signature, "{kind:?}");
        }
    }

    #[test]
    fn replacing_the_active_family_restarts_with_live_recipe_values() {
        let mut value: serde_json::Value = serde_json::from_str(include_str!(
            "../../assets/experiments/particles/procedural.json"
        ))
        .unwrap();
        let fire = value["recipes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|recipe| recipe["id"].as_str() == Some("fire-embers"))
            .unwrap();
        fire["particle_count"] = 12_345.into();
        fire["beats"]["phases"][0]["until_ms"] = 4_000.into();
        let bytes = serde_json::to_vec(&value).unwrap();
        let family = ParticleRecipeFamily::from_json(&bytes).unwrap();
        let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 827_141_709_451,
            initial_demo: ParticleDemoKind::FireEmbers,
        })
        .unwrap();

        renderer.replace_family(family, Duration::from_secs(7));

        assert_eq!(renderer.demo_started_at, Duration::from_secs(7));
        assert_eq!(renderer.particle_count(), 12_345);
        assert_eq!(renderer.effect_beat(Duration::from_secs(7)), "flame");
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
    fn showcase_geometry_accepts_mister_render_sizes() {
        for (width, height) in [(960, 540), (960, 600), (640, 480), (640, 288)] {
            assert!(
                ParticleShowcaseConfig {
                    width,
                    height,
                    seed: 1,
                    initial_demo: ParticleDemoKind::SolarChrysanthemum,
                }
                .validate()
                .is_ok()
            );
        }
        assert!(
            ParticleShowcaseConfig {
                width: 8,
                height: 8,
                seed: 1,
                initial_demo: ParticleDemoKind::SolarChrysanthemum,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn every_demo_renders_at_dynamic_mister_geometries() {
        for (width, height) in [(960, 600), (640, 480)] {
            for demo in ParticleDemoKind::ALL {
                let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
                    width,
                    height,
                    seed: 827_141_709_451,
                    initial_demo: demo,
                })
                .unwrap();
                renderer.configure_capture_hud(false);
                let mut destination = vec![Rgb565Pixel(0); width * height];
                let stats = renderer
                    .render(&mut destination, 1, Duration::from_secs(15))
                    .unwrap();
                assert_eq!(stats.demo, demo);
            }
        }
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
        assert_eq!(wrapped.demo, ParticleDemoKind::PointCloudMorphPassage);
        assert!(
            renderer
                .render(&mut destination, 0, Duration::ZERO)
                .is_err()
        );
    }

    #[test]
    fn zero_pool_transition_clears_stale_geometry_and_uses_renderer_neutral_jitter() {
        let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0x33_7a,
            initial_demo: ParticleDemoKind::LayerMappedHologram,
        })
        .unwrap();
        let mut destination = vec![Rgb565Pixel(0); 960 * 540];
        renderer
            .render(&mut destination, 1, Duration::from_secs(15))
            .unwrap();
        let outgoing_command = renderer
            .commands
            .iter()
            .copied()
            .find(|command| *command != u32::MAX)
            .expect("Form demo must project visible transition geometry");

        renderer.reset_demo(
            ParticleDemoKind::SolarChrysanthemum,
            Duration::from_secs(15),
        );
        assert_eq!(renderer.pool.active(), 0);
        assert!(renderer.commands.is_empty());

        // Recreate the stale-command state from the device crash. This must
        // remain safe even though the direct firework renderer has no pool.
        renderer.commands.push(outgoing_command);
        renderer.begin_transition(Duration::from_secs(45));
        assert_eq!(renderer.transition.count, 1);
        assert!(renderer.transition_started_at.is_some());
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
        assert_eq!(second_stats.demo, first_stats.demo);
        assert_eq!(second_stats.beat, first_stats.beat);
        assert_eq!(second_stats.visible, first_stats.visible);
        assert!(first.commands.len() <= first.pool.active() / 4);
        assert!(first.heat.iter().any(|value| *value > 0));
        assert_eq!(first.heat, second.heat);
        assert_eq!(first_destination, second_destination);
        assert!(
            first_destination[(540 - first.heat_height * FIRE_HEAT_SCALE) * 960..]
                .iter()
                .any(|pixel| *pixel != Rgb565Pixel(0))
        );
    }

    #[test]
    fn galaxy_has_four_arms_bulge_depth_and_dust_gaps() {
        let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
            width: 960,
            height: 540,
            seed: 0x006a_1a9a,
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
        assert!(renderer.pool.flags[renderer.galaxy_projected_count..].contains(&0));
    }

    #[test]
    fn warp_speed_accelerates_and_emits_bounded_streaks() {
        let (calm_travel, calm_speed) =
            warp_travel_and_speed(2.0, 30.0, 14.0, 23.0, 7.0, 0.03, 0.9);
        let (_, warp_speed) = warp_travel_and_speed(18.0, 30.0, 14.0, 23.0, 7.0, 0.03, 0.9);
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
            seed: 0x0090_77a1,
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
            seed: 0x0057_a7e2,
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
                .all(|flags| flags & !3 == 0)
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
