// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Focused, Slint-free host for the production startup particle effects.

use mister_magik_particles::cabinet::{ArcadeCabinetFormation, Rgb565Pixel};
use mister_magik_particles::commands::{
    raster_packed_visual_commands, write_packed_visual_commands,
};
use mister_magik_particles::engine::{ParticleEngine, ParticlePreset, magik_target_mask};
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
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectKind {
    Magik,
    Cabinet,
}

impl EffectKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Magik => "magik",
            Self::Cabinet => "cabinet",
        }
    }

    const fn status_recipe(self) -> StartupParticleRecipe {
        match self {
            Self::Magik => StartupParticleRecipe::Magik,
            Self::Cabinet => StartupParticleRecipe::Cabinet,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectRecipe {
    Magik(MagikRecipe),
    Cabinet(CabinetRecipe),
}

impl EffectRecipe {
    #[must_use]
    pub const fn kind(&self) -> EffectKind {
        match self {
            Self::Magik(_) => EffectKind::Magik,
            Self::Cabinet(_) => EffectKind::Cabinet,
        }
    }

    fn embedded(kind: EffectKind) -> Result<Self, String> {
        match kind {
            EffectKind::Magik => embedded_magik_recipe().map(Self::Magik),
            EffectKind::Cabinet => embedded_cabinet_recipe().map(Self::Cabinet),
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
    pub visible: usize,
    pub simulation_backend: &'static str,
    pub projection_backend: &'static str,
    pub magik_stages: Option<MagikStageTimings>,
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
}

enum PreparedEffect {
    Magik(Box<MagikRenderer>),
    Cabinet(Box<ArcadeCabinetFormation>),
}

impl FocusedParticleRenderer {
    pub fn new(width: usize, height: usize, recipe: EffectRecipe) -> Result<Self, String> {
        let effect = match recipe {
            EffectRecipe::Magik(recipe) => {
                PreparedEffect::Magik(Box::new(MagikRenderer::new(width, height, recipe)?))
            }
            EffectRecipe::Cabinet(recipe) => PreparedEffect::Cabinet(Box::new(
                ArcadeCabinetFormation::new(width, height, recipe)?,
            )),
        };
        Ok(Self { effect })
    }

    #[must_use]
    pub const fn kind(&self) -> EffectKind {
        match self.effect {
            PreparedEffect::Magik(_) => EffectKind::Magik,
            PreparedEffect::Cabinet(_) => EffectKind::Cabinet,
        }
    }

    pub fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<FrameStats, String> {
        match &mut self.effect {
            PreparedEffect::Magik(renderer) => renderer.render(destination, elapsed),
            PreparedEffect::Cabinet(renderer) => {
                let stats = renderer.render(destination, elapsed)?;
                Ok(FrameStats {
                    effect: EffectKind::Cabinet,
                    particles: stats.particles,
                    visible: stats.visible,
                    simulation_backend: "cabinet-scalar",
                    projection_backend: "cabinet-scalar",
                    magik_stages: None,
                })
            }
        }
    }
}

struct MagikRenderer {
    engine: ParticleEngine,
    recipe: MagikRecipe,
    commands: Vec<u32>,
}

impl MagikRenderer {
    fn new(width: usize, height: usize, recipe: MagikRecipe) -> Result<Self, String> {
        let engine = ParticleEngine::from_recipe(
            width,
            height,
            ParticlePreset::Visual,
            recipe.clone(),
            magik_target_mask()?,
        )?;
        let commands = Vec::with_capacity(recipe.particle_count);
        Ok(Self {
            engine,
            recipe,
            commands,
        })
    }

    fn render(
        &mut self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<FrameStats, String> {
        let expected = self
            .engine
            .config()
            .width
            .saturating_mul(self.engine.config().height);
        if destination.len() != expected {
            return Err(format!(
                "MagiK destination has {} pixels, expected {expected}",
                destination.len()
            ));
        }
        let clear_started = Instant::now();
        destination.fill(Rgb565Pixel(self.recipe.appearance.background.0));
        let clear_us = clear_started.elapsed().as_micros() as u64;
        let simulation_started = Instant::now();
        let frame = self.engine.step(elapsed);
        let simulation_us = simulation_started.elapsed().as_micros() as u64;
        let projection_started = Instant::now();
        let visible = write_packed_visual_commands(&self.engine, &mut self.commands);
        let projection_us = projection_started.elapsed().as_micros() as u64;
        let raster_started = Instant::now();
        raster_packed_visual_commands(
            destination,
            &self.commands,
            self.recipe
                .appearance
                .palette
                .map(|color| Rgb565Pixel(color.0)),
            usize::from(self.recipe.appearance.neighbor_palette_index),
        );
        let raster_us = raster_started.elapsed().as_micros() as u64;
        Ok(FrameStats {
            effect: EffectKind::Magik,
            particles: frame.count,
            visible,
            simulation_backend: frame.simulation_backend,
            projection_backend: frame.projection_backend,
            magik_stages: Some(MagikStageTimings {
                clear_us,
                simulation_us,
                projection_us,
                raster_us,
            }),
        })
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
        publish_startup_particle_status(
            &status_path,
            &StartupParticleStatus::applied(0, initial.kind.status_recipe()),
        )?;
        let watcher = LastGoodRecipeFile::spawn_after_initial_content(
            recipe_path,
            &initial_bytes,
            move |bytes| {
                let recipe = parse_effect_recipe(bytes)?;
                PreparedCandidate::prepare(width, height, recipe)
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
        if let Err(error) = self.apply_latest(elapsed) {
            self.last_error = Some(error);
        }
        self.renderer
            .render(destination, elapsed.saturating_sub(self.logical_origin))
    }

    fn apply_latest(&mut self, elapsed: Duration) -> Result<(), String> {
        let Some(attempt) = self.watcher.take_latest() else {
            return Ok(());
        };
        self.generation = attempt.generation;
        match attempt.action {
            ReloadAction::Apply(candidate) => {
                self.renderer = candidate.renderer;
                self.embedded_reset = Some(candidate.embedded);
                self.logical_origin = elapsed;
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
                self.logical_origin = elapsed;
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
}
