// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Focused, Slint-free host for portable RGB565 framebuffer scenes.

use mister_magik_framebuffer_scenes::navigation::{
    NavigationTransitionBuffers, NavigationTransitionDirection, NavigationTransitionEdge,
    NavigationTransitionFrameInput, hdmi_navigation_geometry, render_navigation_transition_input,
};
use mister_magik_framebuffer_scenes::{
    FramebufferScene, Rgb565Pixel, SceneBufferId, SceneClock, SceneGeometry, SceneTarget,
};
use mister_magik_particles::cabinet::{CabinetRenderOptions, CabinetScene, CabinetStageTimings};
use mister_magik_particles::engine::ParticlePreset;
use mister_magik_particles::intro::{IntroScene, IntroStageTimings};
use mister_magik_particles::intro_recipe::{
    INTRO_RECIPE_SCHEMA_V1, IntroRecipe, embedded_intro_recipe, parse_intro_recipe,
};
use mister_magik_particles::magik::{MagikScene, MagikSceneOptions, MagikSceneStats};
use mister_magik_particles::recipes::{
    CABINET_RECIPE_SCHEMA_V1, CabinetRecipe, MAGIK_RECIPE_SCHEMA_V1, MagikRecipe,
    embedded_cabinet_recipe, embedded_magik_recipe, parse_cabinet_recipe, parse_magik_recipe,
};
use mister_magik_particles::reload::{
    LastGoodRecipeFile, MAX_RECIPE_FILE_BYTES, ReloadAction, StartupParticleRecipe,
    StartupParticleStatus, StartupParticleStatusState, publish_startup_particle_status,
};
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectKind {
    Magik,
    Cabinet,
    Intro,
    NavigationTransition,
    CardFlip,
    ScreenshotScreensaver,
}

impl EffectKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Magik => "magik",
            Self::Cabinet => "cabinet",
            Self::Intro => "intro",
            Self::NavigationTransition => "navigation-transition",
            Self::CardFlip => "card-flip",
            Self::ScreenshotScreensaver => "screenshot-screensaver",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "magik" => Some(Self::Magik),
            "cabinet" => Some(Self::Cabinet),
            "intro" => Some(Self::Intro),
            "navigation-transition" => Some(Self::NavigationTransition),
            "card-flip" => Some(Self::CardFlip),
            "screenshot-screensaver" => Some(Self::ScreenshotScreensaver),
            _ => None,
        }
    }

    fn status_recipe(self) -> StartupParticleRecipe {
        match self {
            Self::Magik => StartupParticleRecipe::Magik,
            Self::Cabinet => StartupParticleRecipe::Cabinet,
            Self::Intro => StartupParticleRecipe::Intro,
            Self::NavigationTransition | Self::CardFlip | Self::ScreenshotScreensaver => {
                unreachable!("self-contained scenes do not publish particle recipe status")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationFixture {
    HomeArcade,
    HomeConsoles,
    ConsolesSystem,
}

impl NavigationFixture {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::HomeArcade => "home-arcade",
            Self::HomeConsoles => "home-consoles",
            Self::ConsolesSystem => "consoles-system",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "home-arcade" => Some(Self::HomeArcade),
            "home-consoles" => Some(Self::HomeConsoles),
            "consoles-system" => Some(Self::ConsolesSystem),
            _ => None,
        }
    }

    const fn edge(self) -> NavigationTransitionEdge {
        match self {
            Self::HomeArcade => NavigationTransitionEdge::HomeToArcade,
            Self::HomeConsoles => NavigationTransitionEdge::HomeToConsoles,
            Self::ConsolesSystem => NavigationTransitionEdge::ConsolesToSystem,
        }
    }

    const fn seed(self) -> u16 {
        match self {
            Self::HomeArcade => 0x1234,
            Self::HomeConsoles => 0x4567,
            Self::ConsolesSystem => 0x789a,
        }
    }
}

pub struct NavigationFixtureScene {
    fixture: NavigationFixture,
    width: usize,
    height: usize,
    source: Vec<Rgb565Pixel>,
    destination: Vec<Rgb565Pixel>,
    buffers: NavigationTransitionBuffers,
}

impl NavigationFixtureScene {
    pub fn new(fixture: NavigationFixture) -> Self {
        Self::new_with_geometry(fixture, DEFAULT_WIDTH, DEFAULT_HEIGHT)
    }

    pub fn new_with_geometry(fixture: NavigationFixture, width: usize, height: usize) -> Self {
        let source = generated_navigation_snapshot(fixture, false, width, height);
        let destination = generated_navigation_snapshot(fixture, true, width, height);
        Self {
            fixture,
            width,
            height,
            source,
            destination,
            buffers: NavigationTransitionBuffers::new(width, height),
        }
    }

    #[must_use]
    pub const fn fixture(&self) -> NavigationFixture {
        self.fixture
    }

    pub fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<FrameStats, String> {
        if destination.len() != self.width * self.height {
            return Err(format!(
                "navigation target has {} pixels, expected {}",
                destination.len(),
                self.width * self.height
            ));
        }
        let edge = self.fixture.edge();
        let duration_us = edge.duration_us().max(1);
        let cycle_us = duration_us.saturating_mul(2);
        let elapsed_us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64 % cycle_us;
        let (direction, leg_us) = if elapsed_us < duration_us {
            (NavigationTransitionDirection::Forward, elapsed_us)
        } else {
            (
                NavigationTransitionDirection::Reverse,
                elapsed_us - duration_us,
            )
        };
        let forward_progress = leg_us
            .saturating_mul(u64::from(u16::MAX))
            .saturating_div(duration_us) as u16;
        let progress_q16 = match direction {
            NavigationTransitionDirection::Forward => forward_progress,
            NavigationTransitionDirection::Reverse => u16::MAX - forward_progress,
        };
        let geometry = hdmi_navigation_geometry(
            self.width,
            self.height,
            1,
            0,
            true,
            edge,
            self.fixture.label(),
        );
        let stats = render_navigation_transition_input(
            &mut self.buffers,
            NavigationTransitionFrameInput {
                progress_q16,
                direction,
                edge,
                geometry,
                width: self.width,
                height: self.height,
                source: &self.source,
                destination: &self.destination,
            },
        )
        .map_err(|error| format!("render navigation fixture: {error:?}"))?;
        destination.copy_from_slice(self.buffers.working());
        Ok(FrameStats {
            effect: EffectKind::NavigationTransition,
            particles: 0,
            projected_particles: 0,
            projection_cohorts: 1,
            visible: stats
                .copied_pixels
                .saturating_add(stats.filled_pixels)
                .min(usize::MAX as u64) as usize,
            pixel_writes: stats
                .copied_pixels
                .saturating_add(stats.filled_pixels)
                .min(usize::MAX as u64) as usize,
            simulation_backend: direction.label(),
            projection_backend: self.fixture.label(),
            magik_stages: None,
            cabinet_stages: None,
            intro_stages: None,
            screenshot: None,
            cue_id: "navigation-transition",
            cue_index: 0,
            cue_start_ms: 0,
            previous_cue_start_ms: 0,
            cue_elapsed_ms: 0,
            cue_duration_ms: 0,
            total_ms: 0,
        })
    }
}

fn generated_navigation_snapshot(
    fixture: NavigationFixture,
    destination: bool,
    width: usize,
    height: usize,
) -> Vec<Rgb565Pixel> {
    let mut pixels = vec![Rgb565Pixel(if destination { 0x0841 } else { 0x1082 }); width * height];
    let seed = fixture.seed().wrapping_add(u16::from(destination) * 0x1111);
    let band_height = scale_reference_y(24, height).max(1);
    let pattern_width = scale_reference_x(96, width).max(1);
    let pattern_height = scale_reference_y(72, height).max(1);
    for y in 0..height {
        let band = ((y / band_height) as u16).wrapping_mul(0x0021);
        for x in 0..width {
            if (x / pattern_width + y / pattern_height + usize::from(destination)).is_multiple_of(7)
            {
                pixels[y * width + x] = Rgb565Pixel(seed.wrapping_add(band));
            }
        }
    }
    for card in 0..4 {
        let x0 = scale_reference_x(32 + card * 224, width);
        let y0 = scale_reference_y(if destination { 96 + card * 8 } else { 80 }, height);
        let card_width = scale_reference_x(184, width);
        let card_height = scale_reference_y(320, height);
        let bottom_margin = scale_reference_y(32, height);
        let color = seed.wrapping_add((card as u16 + 1) * 0x0841);
        for y in y0..(y0 + card_height).min(height.saturating_sub(bottom_margin)) {
            let start = y * width + x0.min(width);
            let end = y * width + (x0 + card_width).min(width);
            pixels[start..end].fill(Rgb565Pixel(color));
        }
    }
    pixels
}

const fn scale_reference_x(value: usize, width: usize) -> usize {
    value.saturating_mul(width) / DEFAULT_WIDTH
}

const fn scale_reference_y(value: usize, height: usize) -> usize {
    value.saturating_mul(height) / DEFAULT_HEIGHT
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectRecipe {
    Magik(MagikRecipe),
    Cabinet(CabinetRecipe),
    Intro(IntroRecipe),
}

impl EffectRecipe {
    #[must_use]
    pub const fn kind(&self) -> EffectKind {
        match self {
            Self::Magik(_) => EffectKind::Magik,
            Self::Cabinet(_) => EffectKind::Cabinet,
            Self::Intro(_) => EffectKind::Intro,
        }
    }

    fn embedded(kind: EffectKind) -> Result<Self, String> {
        match kind {
            EffectKind::Magik => embedded_magik_recipe().map(Self::Magik),
            EffectKind::Cabinet => embedded_cabinet_recipe().map(Self::Cabinet),
            EffectKind::Intro => embedded_intro_recipe().map(Self::Intro),
            EffectKind::NavigationTransition
            | EffectKind::CardFlip
            | EffectKind::ScreenshotScreensaver => {
                Err("self-contained scenes do not have embedded particle recipes".into())
            }
        }
    }
}

#[derive(Deserialize)]
struct RecipeHeader {
    schema: String,
}

/// Parses exactly one of the two production recipe schemas.
pub fn parse_effect_recipe(bytes: &[u8]) -> Result<EffectRecipe, String> {
    let header: RecipeHeader = serde_json::from_slice(bytes)
        .map_err(|error| format!("parse startup particle recipe header: {error}"))?;
    match header.schema.as_str() {
        MAGIK_RECIPE_SCHEMA_V1 => parse_magik_recipe(bytes).map(EffectRecipe::Magik),
        CABINET_RECIPE_SCHEMA_V1 => parse_cabinet_recipe(bytes).map(EffectRecipe::Cabinet),
        INTRO_RECIPE_SCHEMA_V1 => parse_intro_recipe(bytes).map(EffectRecipe::Intro),
        schema => Err(format!(
            "unsupported startup particle recipe schema {schema:?}"
        )),
    }
}

pub fn read_effect_recipe(path: &Path) -> Result<EffectRecipe, String> {
    parse_effect_recipe(&read_effect_recipe_bytes(path)?)
}

fn read_effect_recipe_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take((MAX_RECIPE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if bytes.len() > MAX_RECIPE_FILE_BYTES {
        return Err(format!(
            "{} exceeds the {MAX_RECIPE_FILE_BYTES} byte recipe limit",
            path.display()
        ));
    }
    Ok(bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameStats {
    pub effect: EffectKind,
    pub particles: usize,
    pub projected_particles: usize,
    pub projection_cohorts: u8,
    pub visible: usize,
    pub pixel_writes: usize,
    pub simulation_backend: &'static str,
    pub projection_backend: &'static str,
    pub magik_stages: Option<MagikStageTimings>,
    pub cabinet_stages: Option<CabinetStageTimings>,
    pub intro_stages: Option<IntroStageTimings>,
    pub screenshot: Option<ScreenshotFrameStats>,
    pub cue_id: &'static str,
    pub cue_index: usize,
    pub cue_start_ms: u64,
    pub previous_cue_start_ms: u64,
    pub cue_elapsed_ms: u64,
    pub cue_duration_ms: u64,
    pub total_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenshotFrameStats {
    pub raster_held_cards: usize,
    pub raster_moved_cards: usize,
    pub raster_hold_layer_mask: u8,
    pub raster_visible_layer_mask: u8,
    pub sixteenth_phase_layer_mask: u8,
    pub phase_bank_resident_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MagikStageTimings {
    pub clear_us: u64,
    pub simulation_us: u64,
    pub projection_us: u64,
    pub raster_us: u64,
}

pub struct FocusedParticleRenderer {
    effect: PreparedEffect,
    geometry: SceneGeometry,
}

pub const CABINET_LAB_MAX_PARTICLES: usize = 72_704;

enum PreparedEffect {
    Magik(Box<MagikScene>),
    Cabinet(Box<CabinetScene>),
    Intro(Box<IntroScene>),
}

impl FocusedParticleRenderer {
    pub fn new(width: usize, height: usize, recipe: EffectRecipe) -> Result<Self, String> {
        let effect = match recipe {
            EffectRecipe::Magik(recipe) => {
                PreparedEffect::Magik(Box::new(MagikScene::from_magik_recipe_with_options(
                    width,
                    height,
                    ParticlePreset::Visual,
                    recipe,
                    MagikSceneOptions {
                        order_commands: false,
                        reusable_buffers: 2,
                        worker_start: None,
                    },
                )?))
            }
            EffectRecipe::Cabinet(recipe) => {
                PreparedEffect::Cabinet(Box::new(CabinetScene::new_with_capacity(
                    width,
                    height,
                    recipe,
                    2,
                    CABINET_LAB_MAX_PARTICLES,
                )?))
            }
            EffectRecipe::Intro(recipe) => {
                PreparedEffect::Intro(Box::new(IntroScene::new(width, height, recipe)?))
            }
        };
        Self::with_effect(width, height, effect)
    }

    pub fn new_synchronous(
        width: usize,
        height: usize,
        recipe: EffectRecipe,
    ) -> Result<Self, String> {
        let effect = match recipe {
            EffectRecipe::Magik(recipe) => PreparedEffect::Magik(Box::new(
                MagikScene::from_magik_recipe_for_deterministic_capture(
                    width,
                    height,
                    ParticlePreset::Visual,
                    recipe,
                )?,
            )),
            EffectRecipe::Cabinet(recipe) => {
                PreparedEffect::Cabinet(Box::new(CabinetScene::new_with_capacity(
                    width,
                    height,
                    recipe,
                    2,
                    CABINET_LAB_MAX_PARTICLES,
                )?))
            }
            EffectRecipe::Intro(recipe) => {
                PreparedEffect::Intro(Box::new(IntroScene::new(width, height, recipe)?))
            }
        };
        Self::with_effect(width, height, effect)
    }

    fn with_effect(width: usize, height: usize, effect: PreparedEffect) -> Result<Self, String> {
        let geometry =
            SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
        Ok(Self { effect, geometry })
    }

    #[must_use]
    pub const fn kind(&self) -> EffectKind {
        match self.effect {
            PreparedEffect::Magik(_) => EffectKind::Magik,
            PreparedEffect::Cabinet(_) => EffectKind::Cabinet,
            PreparedEffect::Intro(_) => EffectKind::Intro,
        }
    }

    #[must_use]
    pub fn intro_total_ms(&self) -> Option<u64> {
        match &self.effect {
            PreparedEffect::Intro(renderer) => Some(renderer.recipe().total_ms),
            _ => None,
        }
    }

    pub fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<FrameStats, String> {
        self.render_buffer(destination, 0, elapsed, None)
    }

    pub fn render_buffer(
        &mut self,
        destination: &mut [Rgb565Pixel],
        buffer_id: u8,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
    ) -> Result<FrameStats, String> {
        match &mut self.effect {
            PreparedEffect::Magik(renderer) => {
                let buffer = SceneBufferId::new(buffer_id, 2).map_err(|error| error.to_string())?;
                let stats =
                    renderer.render_with_lookahead(destination, buffer, elapsed, next_elapsed)?;
                Ok(magik_frame_stats(stats))
            }
            PreparedEffect::Cabinet(renderer) => {
                let buffer = SceneBufferId::new(buffer_id, 2).map_err(|error| error.to_string())?;
                let target = SceneTarget::new(destination, self.geometry, buffer)
                    .map_err(|error| error.to_string())?;
                let stats = FramebufferScene::render(
                    renderer.as_mut(),
                    target,
                    SceneClock {
                        frame: 0,
                        elapsed,
                        next_elapsed,
                    },
                )
                .map_err(|error| error.to_string())?;
                Ok(FrameStats {
                    effect: EffectKind::Cabinet,
                    particles: stats.particles,
                    projected_particles: stats.projected_particles,
                    projection_cohorts: stats.projection_cohorts,
                    visible: stats.visible,
                    pixel_writes: stats.pixel_writes,
                    simulation_backend: "cabinet-scalar",
                    projection_backend: stats.projection_backend,
                    magik_stages: None,
                    cabinet_stages: Some(stats.stages),
                    intro_stages: None,
                    screenshot: None,
                    cue_id: "cabinet",
                    cue_index: 0,
                    cue_start_ms: 0,
                    previous_cue_start_ms: 0,
                    cue_elapsed_ms: 0,
                    cue_duration_ms: 0,
                    total_ms: 0,
                })
            }
            PreparedEffect::Intro(renderer) => {
                let buffer = SceneBufferId::new(buffer_id, 2).map_err(|error| error.to_string())?;
                let target = SceneTarget::new(destination, self.geometry, buffer)
                    .map_err(|error| error.to_string())?;
                let stats = FramebufferScene::render(
                    renderer.as_mut(),
                    target,
                    SceneClock {
                        frame: elapsed.as_nanos().saturating_mul(60) as u64 / 1_000_000_000,
                        elapsed,
                        next_elapsed,
                    },
                )
                .map_err(|error| error.to_string())?;
                Ok(FrameStats {
                    effect: EffectKind::Intro,
                    particles: stats.particles,
                    projected_particles: stats.projected_particles,
                    projection_cohorts: stats.projection_cohorts,
                    visible: stats.visible,
                    pixel_writes: stats.pixel_writes,
                    simulation_backend: "intro-storyboard",
                    projection_backend: stats.projection_backend,
                    magik_stages: None,
                    cabinet_stages: None,
                    intro_stages: Some(stats.stages),
                    screenshot: None,
                    cue_id: stats.cue_id,
                    cue_index: stats.cue_index,
                    cue_start_ms: stats.cue_start_ms,
                    previous_cue_start_ms: stats.previous_cue_start_ms,
                    cue_elapsed_ms: stats.cue_elapsed_ms,
                    cue_duration_ms: stats.cue_duration_ms,
                    total_ms: renderer.recipe().total_ms,
                })
            }
        }
    }

    pub fn set_cabinet_render_options(
        &mut self,
        options: CabinetRenderOptions,
    ) -> Result<(), String> {
        match &mut self.effect {
            PreparedEffect::Cabinet(renderer) => renderer.set_render_options(options),
            PreparedEffect::Magik(_) => Err("cabinet controls require the cabinet scene".into()),
            PreparedEffect::Intro(_) => Err("cabinet controls require the cabinet scene".into()),
        }
    }

    fn intro_cue_id_at(&self, elapsed: Duration) -> Option<&str> {
        let PreparedEffect::Intro(renderer) = &self.effect else {
            return None;
        };
        let (index, _) = renderer.cue_at(elapsed);
        Some(renderer.recipe().cues[index].id())
    }

    fn intro_cue_start_ms(&self, id: &str) -> Option<u64> {
        let PreparedEffect::Intro(renderer) = &self.effect else {
            return None;
        };
        let mut start = 0;
        for cue in &renderer.recipe().cues {
            if cue.id() == id {
                return Some(start);
            }
            start += cue.duration_ms();
        }
        None
    }
}

fn magik_frame_stats(stats: MagikSceneStats) -> FrameStats {
    FrameStats {
        effect: EffectKind::Magik,
        particles: stats.count,
        projected_particles: stats.count,
        projection_cohorts: 1,
        visible: stats.visible,
        pixel_writes: stats.visible,
        simulation_backend: stats.simulation_backend,
        projection_backend: stats.projection_backend,
        magik_stages: Some(MagikStageTimings {
            clear_us: stats.clear_us.min(u128::from(u64::MAX)) as u64,
            simulation_us: stats.simulation_us.min(u128::from(u64::MAX)) as u64,
            projection_us: stats.projection_us.min(u128::from(u64::MAX)) as u64,
            raster_us: stats.raster_us.min(u128::from(u64::MAX)) as u64,
        }),
        cabinet_stages: None,
        intro_stages: None,
        screenshot: None,
        cue_id: "magik",
        cue_index: 0,
        cue_start_ms: 0,
        previous_cue_start_ms: 0,
        cue_elapsed_ms: 0,
        cue_duration_ms: 0,
        total_ms: 0,
    }
}

/// Live renderer that applies one newest pending recipe at a frame boundary.
pub struct LiveParticleRenderer {
    renderer: FocusedParticleRenderer,
    embedded_reset: Option<FocusedParticleRenderer>,
    watcher: LastGoodRecipeFile<PreparedCandidate>,
    status_path: PathBuf,
    logical_origin: Duration,
    generation: u64,
    status_state: StartupParticleStatusState,
    last_error: Option<String>,
    cabinet_options: Option<CabinetRenderOptions>,
}

struct PreparedCandidate {
    kind: EffectKind,
    renderer: FocusedParticleRenderer,
    embedded: FocusedParticleRenderer,
}

impl PreparedCandidate {
    fn prepare(width: usize, height: usize, recipe: EffectRecipe) -> Result<Self, String> {
        let kind = recipe.kind();
        let renderer = FocusedParticleRenderer::new(width, height, recipe)?;
        let embedded = FocusedParticleRenderer::new(width, height, EffectRecipe::embedded(kind)?)?;
        Ok(Self {
            kind,
            renderer,
            embedded,
        })
    }

    fn prepare_for_kind(
        width: usize,
        height: usize,
        expected: EffectKind,
        recipe: EffectRecipe,
    ) -> Result<Self, String> {
        let actual = recipe.kind();
        if actual != expected {
            return Err(format!(
                "live reload for {} cannot switch to the {} scene",
                expected.label(),
                actual.label()
            ));
        }
        Self::prepare(width, height, recipe)
    }
}

impl LiveParticleRenderer {
    pub fn start(
        width: usize,
        height: usize,
        recipe_path: PathBuf,
        status_path: PathBuf,
    ) -> Result<Self, String> {
        let initial_bytes = read_effect_recipe_bytes(&recipe_path)?;
        let initial =
            PreparedCandidate::prepare(width, height, parse_effect_recipe(&initial_bytes)?)?;
        let expected_kind = initial.kind;
        publish_startup_particle_status(
            &status_path,
            &StartupParticleStatus::applied(0, initial.kind.status_recipe()),
        )?;
        let watcher = LastGoodRecipeFile::spawn_after_initial_content(
            recipe_path,
            &initial_bytes,
            move |bytes| {
                let recipe = parse_effect_recipe(bytes)?;
                PreparedCandidate::prepare_for_kind(width, height, expected_kind, recipe)
            },
        )?;
        Ok(Self {
            renderer: initial.renderer,
            embedded_reset: Some(initial.embedded),
            watcher,
            status_path,
            logical_origin: Duration::ZERO,
            generation: 0,
            status_state: StartupParticleStatusState::Applied,
            last_error: None,
            cabinet_options: None,
        })
    }

    #[must_use]
    pub const fn effect(&self) -> EffectKind {
        self.renderer.kind()
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn status_state(&self) -> StartupParticleStatusState {
        self.status_state
    }

    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<FrameStats, String> {
        self.render_buffer(destination, 0, elapsed, None)
    }

    pub fn render_buffer(
        &mut self,
        destination: &mut [Rgb565Pixel],
        buffer_id: u8,
        elapsed: Duration,
        next_elapsed: Option<Duration>,
    ) -> Result<FrameStats, String> {
        if let Err(error) = self.apply_latest(elapsed) {
            self.last_error = Some(error);
        }
        if let Some(options) = self.cabinet_options {
            self.renderer.set_cabinet_render_options(options)?;
        }
        let logical_elapsed = self.timeline_elapsed(elapsed);
        let logical_next = next_elapsed.map(|next| self.timeline_elapsed(next));
        self.renderer
            .render_buffer(destination, buffer_id, logical_elapsed, logical_next)
    }

    pub fn set_cabinet_render_options(
        &mut self,
        options: CabinetRenderOptions,
    ) -> Result<(), String> {
        self.renderer.set_cabinet_render_options(options)?;
        self.cabinet_options = Some(options);
        Ok(())
    }

    fn apply_latest(&mut self, elapsed: Duration) -> Result<(), String> {
        let Some(attempt) = self.watcher.take_latest() else {
            return Ok(());
        };
        self.generation = attempt.generation;
        let logical_elapsed = self.timeline_elapsed(elapsed);
        let resume_cue = self
            .renderer
            .intro_cue_id_at(logical_elapsed)
            .map(str::to_owned);
        match attempt.action {
            ReloadAction::Apply(candidate) => {
                self.renderer = candidate.renderer;
                self.embedded_reset = Some(candidate.embedded);
                self.logical_origin = resume_cue
                    .as_deref()
                    .and_then(|id| self.renderer.intro_cue_start_ms(id))
                    .map_or(elapsed, |start| {
                        elapsed.saturating_sub(Duration::from_millis(start))
                    });
                self.status_state = StartupParticleStatusState::Applied;
                self.last_error = None;
                publish_startup_particle_status(
                    &self.status_path,
                    &StartupParticleStatus::applied(
                        attempt.generation,
                        candidate.kind.status_recipe(),
                    ),
                )?;
            }
            ReloadAction::ResetToEmbedded => {
                let kind = self.renderer.kind();
                let Some(renderer) = self.embedded_reset.take() else {
                    return self.reject(
                        attempt.generation,
                        kind,
                        "embedded particle renderer was not prepared",
                    );
                };
                self.renderer = renderer;
                self.logical_origin = resume_cue
                    .as_deref()
                    .and_then(|id| self.renderer.intro_cue_start_ms(id))
                    .map_or(elapsed, |start| {
                        elapsed.saturating_sub(Duration::from_millis(start))
                    });
                self.status_state = StartupParticleStatusState::Embedded;
                self.last_error = None;
                publish_startup_particle_status(
                    &self.status_path,
                    &StartupParticleStatus::embedded(attempt.generation, kind.status_recipe()),
                )?;
            }
            ReloadAction::Reject(error) => {
                self.reject(attempt.generation, self.renderer.kind(), &error)?;
            }
        }
        Ok(())
    }

    fn timeline_elapsed(&self, elapsed: Duration) -> Duration {
        let logical = elapsed.saturating_sub(self.logical_origin);
        let Some(total_ms) = self.renderer.intro_total_ms() else {
            return logical;
        };
        let total = Duration::from_millis(total_ms);
        Duration::from_nanos((logical.as_nanos() % total.as_nanos()) as u64)
    }

    fn reject(&mut self, generation: u64, kind: EffectKind, error: &str) -> Result<(), String> {
        self.status_state = StartupParticleStatusState::Rejected;
        self.last_error = Some(error.to_owned());
        publish_startup_particle_status(
            &self.status_path,
            &StartupParticleStatus::rejected(generation, kind.status_recipe(), error),
        )
    }
}

pub const DEFAULT_WIDTH: usize = 960;
pub const DEFAULT_HEIGHT: usize = 540;

#[cfg(test)]
mod tests {
    use super::*;
    use mister_magik_particles::recipes::{
        EMBEDDED_CABINET_RECIPE_JSON, EMBEDDED_MAGIK_RECIPE_JSON,
    };

    #[test]
    fn schema_selects_only_the_two_focused_effects() {
        assert_eq!(
            parse_effect_recipe(EMBEDDED_MAGIK_RECIPE_JSON)
                .unwrap()
                .kind(),
            EffectKind::Magik
        );
        assert_eq!(
            parse_effect_recipe(EMBEDDED_CABINET_RECIPE_JSON)
                .unwrap()
                .kind(),
            EffectKind::Cabinet
        );
        assert!(parse_effect_recipe(br#"{"schema":"particle-family-v1"}"#).is_err());
    }

    #[test]
    fn embedded_effects_prepare_without_slint() {
        FocusedParticleRenderer::new(
            DEFAULT_WIDTH,
            DEFAULT_HEIGHT,
            EffectRecipe::Magik(embedded_magik_recipe().unwrap()),
        )
        .unwrap();
        FocusedParticleRenderer::new(
            DEFAULT_WIDTH,
            DEFAULT_HEIGHT,
            EffectRecipe::Cabinet(embedded_cabinet_recipe().unwrap()),
        )
        .unwrap();
    }

    #[test]
    fn navigation_fixtures_render_deterministic_forward_and_reverse_frames() {
        for (fixture, forward_ms, reverse_ms, forward_hash, reverse_hash) in [
            (
                NavigationFixture::HomeArcade,
                360,
                1_080,
                0x8794_49ba_b598_ce37,
                0xfcb5_2e21_9a05_abc8,
            ),
            (
                NavigationFixture::HomeConsoles,
                300,
                900,
                0xda67_dc7e_5bf8_f369,
                0xbaa6_b828_b360_780d,
            ),
            (
                NavigationFixture::ConsolesSystem,
                360,
                1_080,
                0x8794_49ba_b598_ce37,
                0x8b55_e6f6_dbc4_2464,
            ),
        ] {
            let mut scene = NavigationFixtureScene::new(fixture);
            let mut pixels = vec![Rgb565Pixel(0); DEFAULT_WIDTH * DEFAULT_HEIGHT];
            let forward = scene
                .render(&mut pixels, Duration::from_millis(forward_ms))
                .unwrap();
            assert_eq!(forward.simulation_backend, "forward");
            assert_eq!(rgb565_hash(&pixels), forward_hash);
            let reverse = scene
                .render(&mut pixels, Duration::from_millis(reverse_ms))
                .unwrap();
            assert_eq!(reverse.simulation_backend, "reverse");
            assert_eq!(rgb565_hash(&pixels), reverse_hash);
        }
    }

    #[test]
    fn navigation_fixture_rejects_malformed_target() {
        let mut scene = NavigationFixtureScene::new(NavigationFixture::HomeArcade);
        assert!(scene.render(&mut [], Duration::ZERO).is_err());
    }

    #[test]
    fn navigation_fixture_uses_the_selected_render_geometry() {
        let width = 960;
        let height = 600;
        let mut scene =
            NavigationFixtureScene::new_with_geometry(NavigationFixture::HomeArcade, width, height);
        let mut pixels = vec![Rgb565Pixel(0); width * height];
        let frame = scene
            .render(&mut pixels, Duration::from_millis(360))
            .unwrap();
        assert_eq!(frame.simulation_backend, "forward");
        assert!(pixels.iter().any(|pixel| pixel.0 != 0));
    }

    #[test]
    fn particle_reload_prepares_complete_scenes_without_switching_kind() {
        let magik = EffectRecipe::Magik(embedded_magik_recipe().unwrap());
        let prepared = PreparedCandidate::prepare_for_kind(
            DEFAULT_WIDTH,
            DEFAULT_HEIGHT,
            EffectKind::Magik,
            magik,
        )
        .unwrap();
        assert_eq!(prepared.renderer.kind(), EffectKind::Magik);
        assert_eq!(prepared.embedded.kind(), EffectKind::Magik);

        let cabinet = EffectRecipe::Cabinet(embedded_cabinet_recipe().unwrap());
        let error = PreparedCandidate::prepare_for_kind(
            DEFAULT_WIDTH,
            DEFAULT_HEIGHT,
            EffectKind::Magik,
            cabinet,
        )
        .err()
        .unwrap();
        assert!(error.contains("cannot switch"));
    }

    fn rgb565_hash(pixels: &[Rgb565Pixel]) -> u64 {
        pixels.iter().fold(0xcbf2_9ce4_8422_2325, |hash, pixel| {
            pixel.0.to_le_bytes().into_iter().fold(hash, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            })
        })
    }
}
