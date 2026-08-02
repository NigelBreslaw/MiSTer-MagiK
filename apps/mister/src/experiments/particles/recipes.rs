// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Embedded, validated data contracts for the archived particle experiments.

use serde::Deserialize;
use serde_json::Value;
use slint::platform::software_renderer::Rgb565Pixel;
use std::collections::BTreeMap;
use std::sync::LazyLock;

use super::showcase::ParticleDemoKind;

const FIREWORKS_FAMILY_SCHEMA: &str = "mister-magik-particle-fireworks-family-v1";
const FIREWORKS_FAMILY_JSON: &str =
    include_str!("../../../assets/experiments/particles/fireworks.json");
const PROCEDURAL_FAMILY_SCHEMA: &str = "mister-magik-particle-procedural-family-v1";
const PROCEDURAL_FAMILY_JSON: &str =
    include_str!("../../../assets/experiments/particles/procedural.json");
const FORM_FAMILY_SCHEMA: &str = "mister-magik-particle-form-family-v1";
const FORM_FAMILY_JSON: &str = include_str!("../../../assets/experiments/particles/form.json");

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BeatSpec {
    pub repeat_ms: Option<u64>,
    pub phases: Vec<BeatPhase>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BeatPhase {
    pub until_ms: u64,
    pub label: String,
}

impl BeatSpec {
    fn validate(&self, duration_ms: u64) -> Result<(), String> {
        let period = self.repeat_ms.unwrap_or(duration_ms);
        if period == 0 || period > duration_ms || self.phases.is_empty() {
            return Err("particle recipe beat period and phases must be non-zero".into());
        }
        let mut previous = 0;
        for phase in &self.phases {
            if phase.until_ms <= previous
                || phase.until_ms > period
                || phase.label.trim().is_empty()
            {
                return Err("particle recipe beat phases must be ordered and labelled".into());
            }
            previous = phase.until_ms;
        }
        if previous != period {
            return Err("particle recipe final beat must end at its period".into());
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecipeFamily {
    schema: String,
    recipes: Vec<RawRecipe>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecipe {
    id: String,
    label: String,
    duration_ms: u64,
    beats: BeatSpec,
    particle_count: usize,
    palette: Vec<String>,
    params: BTreeMap<String, f32>,
}

#[derive(Clone)]
pub(super) struct CompiledRecipe {
    pub id: String,
    pub label: String,
    pub duration_ms: u64,
    pub beats: BeatSpec,
    pub particle_count: usize,
    pub palette: [Rgb565Pixel; 8],
    params: BTreeMap<String, f32>,
}

impl CompiledRecipe {
    pub fn param(&self, name: &str) -> f32 {
        self.params[name]
    }

    pub fn beat(&self, elapsed_ms: u64) -> &str {
        &self.beats.phases[self.beat_phase(elapsed_ms)].label
    }

    pub fn beat_phase(&self, elapsed_ms: u64) -> usize {
        let period = self.beats.repeat_ms.unwrap_or(self.duration_ms);
        let logical = elapsed_ms % period;
        self.beats
            .phases
            .iter()
            .position(|phase| logical < phase.until_ms)
            .unwrap_or(self.beats.phases.len() - 1)
    }
}

#[derive(Clone)]
struct CompiledFamily {
    recipes: Vec<CompiledRecipe>,
}

static PROCEDURAL_FAMILY: LazyLock<Result<CompiledFamily, String>> = LazyLock::new(|| {
    compile_recipe_family(
        PROCEDURAL_FAMILY_JSON,
        PROCEDURAL_FAMILY_SCHEMA,
        19,
        procedural_keys,
    )
});
static FORM_FAMILY: LazyLock<Result<CompiledFamily, String>> =
    LazyLock::new(|| compile_recipe_family(FORM_FAMILY_JSON, FORM_FAMILY_SCHEMA, 5, form_keys));

#[derive(Clone)]
pub struct ParticleRecipeFamily {
    family: RecipeFamily,
}

#[derive(Clone)]
enum RecipeFamily {
    Fireworks(FireworkFamily),
    Procedural(CompiledFamily),
    Form(CompiledFamily),
}

impl ParticleRecipeFamily {
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|error| format!("parse particle recipe family: {error}"))?;
        let schema = value
            .get("schema")
            .and_then(Value::as_str)
            .ok_or("particle recipe family schema must be a string")?;
        let json = std::str::from_utf8(bytes)
            .map_err(|error| format!("particle recipe family must be UTF-8: {error}"))?;
        match schema {
            FIREWORKS_FAMILY_SCHEMA => {
                let family: FireworkFamily = serde_json::from_str(json)
                    .map_err(|error| format!("parse fireworks family: {error}"))?;
                family.validate()?;
                Ok(Self {
                    family: RecipeFamily::Fireworks(family),
                })
            }
            PROCEDURAL_FAMILY_SCHEMA => Ok(Self {
                family: RecipeFamily::Procedural(compile_recipe_family(
                    json,
                    PROCEDURAL_FAMILY_SCHEMA,
                    19,
                    procedural_keys,
                )?),
            }),
            FORM_FAMILY_SCHEMA => Ok(Self {
                family: RecipeFamily::Form(compile_recipe_family(
                    json,
                    FORM_FAMILY_SCHEMA,
                    5,
                    form_keys,
                )?),
            }),
            _ => Err(format!("unsupported particle recipe family {schema:?}")),
        }
    }

    pub fn contains(&self, demo: ParticleDemoKind) -> bool {
        let id = demo.telemetry_label();
        match &self.family {
            RecipeFamily::Fireworks(family) => family.has_show(id),
            RecipeFamily::Procedural(family) | RecipeFamily::Form(family) => {
                family.find(id).is_some()
            }
        }
    }

    pub(super) fn recipe(&self, id: &str) -> Option<&CompiledRecipe> {
        match &self.family {
            RecipeFamily::Procedural(family) | RecipeFamily::Form(family) => family.find(id),
            RecipeFamily::Fireworks(_) => None,
        }
    }

    pub(super) fn firework_show(&self, id: &str, schema: &str) -> Option<String> {
        match &self.family {
            RecipeFamily::Fireworks(family) => family.show_json(id, schema),
            RecipeFamily::Procedural(_) | RecipeFamily::Form(_) => None,
        }
    }

    pub(super) fn duration_ms(&self, demo: ParticleDemoKind) -> Option<u64> {
        let id = demo.telemetry_label();
        match &self.family {
            RecipeFamily::Fireworks(family) => family.duration_ms(id),
            RecipeFamily::Procedural(family) | RecipeFamily::Form(family) => {
                family.find(id).map(|recipe| recipe.duration_ms)
            }
        }
    }

    pub(super) fn category(&self) -> ParticleRecipeCategory {
        match &self.family {
            RecipeFamily::Fireworks(_) => ParticleRecipeCategory::Fireworks,
            RecipeFamily::Procedural(_) => ParticleRecipeCategory::Procedural,
            RecipeFamily::Form(_) => ParticleRecipeCategory::Form,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ParticleRecipeCategory {
    Fireworks,
    Procedural,
    Form,
}

impl CompiledFamily {
    fn find(&self, id: &str) -> Option<&CompiledRecipe> {
        self.recipes.iter().find(|recipe| recipe.id == id)
    }
}

fn compile_recipe_family(
    json: &str,
    expected_schema: &str,
    expected_count: usize,
    known_keys: fn(&str) -> Option<&'static [&'static str]>,
) -> Result<CompiledFamily, String> {
    let raw: RawRecipeFamily = serde_json::from_str(json)
        .map_err(|error| format!("parse particle recipe family: {error}"))?;
    if raw.schema != expected_schema {
        return Err(format!(
            "unsupported particle recipe family {:?}",
            raw.schema
        ));
    }
    if raw.recipes.len() != expected_count {
        return Err(format!(
            "particle recipe family {:?} must contain exactly {expected_count} recipes",
            raw.schema
        ));
    }
    let mut recipes = Vec::with_capacity(raw.recipes.len());
    for recipe in raw.recipes {
        if recipes
            .iter()
            .any(|existing: &CompiledRecipe| existing.id == recipe.id)
        {
            return Err(format!("duplicate particle recipe {:?}", recipe.id));
        }
        let keys = known_keys(&recipe.id)
            .ok_or_else(|| format!("unknown particle recipe {:?}", recipe.id))?;
        if recipe.params.len() != keys.len()
            || keys.iter().any(|key| !recipe.params.contains_key(*key))
        {
            return Err(format!(
                "particle recipe {:?} has missing or unknown parameters",
                recipe.id
            ));
        }
        if recipe
            .params
            .values()
            .any(|value| !value.is_finite() || value.abs() > 4096.0)
        {
            return Err(format!(
                "particle recipe {:?} parameter is out of bounds",
                recipe.id
            ));
        }
        if recipe.label.trim().is_empty()
            || !(100..=120_000).contains(&recipe.duration_ms)
            || !(1..=98_304).contains(&recipe.particle_count)
        {
            return Err(format!(
                "particle recipe {:?} envelope is invalid",
                recipe.id
            ));
        }
        validate_param_relationships(
            &recipe.id,
            &recipe.params,
            recipe.particle_count,
            recipe.duration_ms,
        )?;
        recipe.beats.validate(recipe.duration_ms)?;
        let palette: [Rgb565Pixel; 8] = recipe
            .palette
            .iter()
            .map(|color| parse_rgb565(color).map(Rgb565Pixel))
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| format!("particle recipe {:?} palette must have 8 colors", recipe.id))?;
        recipes.push(CompiledRecipe {
            id: recipe.id,
            label: recipe.label,
            duration_ms: recipe.duration_ms,
            beats: recipe.beats,
            particle_count: recipe.particle_count,
            palette,
            params: recipe.params,
        });
    }
    Ok(CompiledFamily { recipes })
}

fn validate_param_relationships(
    id: &str,
    params: &BTreeMap<String, f32>,
    particle_count: usize,
    duration_ms: u64,
) -> Result<(), String> {
    let p = |name: &str| params[name];
    let duration = duration_ms as f32 / 1000.0;
    let valid = match id {
        "warp-speed" => {
            0.0 < p("calm_end")
                && p("calm_end") < p("accelerate_end")
                && p("accelerate_end") < p("cruise_end")
                && p("cruise_end") < duration
                && 0.0 < p("min_speed")
                && p("min_speed") <= p("max_speed")
                && 0.0 < p("projection_numerator")
                && 0.0 < p("projection_bias")
        }
        "weather" => {
            0.0 < p("rain_end") && p("rain_end") < p("snow_end") && p("snow_end") < duration
        }
        "fountain-waterfall" => {
            0.0 < p("fountain_end")
                && p("fountain_end") < p("morph_end")
                && p("morph_end") < duration
        }
        "arcade-cabinet" => {
            0.0 < p("formation_end")
                && p("formation_end") < p("orbit_end")
                && p("orbit_end") < p("return_end")
                && p("return_end") < duration
        }
        "variable-width-ribbons" => {
            let ribbons = p("ribbon_count");
            let samples = p("ribbon_samples");
            let streaks = p("streak_count");
            ribbons >= 1.0
                && samples >= 2.0
                && streaks >= 0.0
                && ribbons.fract() == 0.0
                && samples.fract() == 0.0
                && streaks.fract() == 0.0
                && ribbons + streaks <= particle_count as f32
        }
        "curl-noise-flow-field" => {
            0.0 < p("normal_x")
                && 0.0 < p("normal_y")
                && 0.0 < p("softening")
                && 0.0 < p("integration_scale")
        }
        "layered-child-systems" => {
            0.0 < p("cycle") && p("trail_length") >= 2.0 && p("trail_length").fract() == 0.0
        }
        "source-morph" => {
            0.0 <= p("morph_start")
                && p("morph_start") < p("morph_end")
                && p("morph_end") < p("return_start")
                && p("return_start") < duration
        }
        "layer-mapped-hologram" => {
            0.0 < p("reveal_end") && p("reveal_end") < p("fade_start") && p("fade_start") < duration
        }
        "point-cloud-morph-passage" => {
            0.0 <= p("morph_start")
                && p("morph_start") < p("morph_end")
                && p("morph_end") < p("return_start")
                && p("return_start") < p("return_end")
                && p("return_end") < duration
        }
        _ => true,
    };
    if params
        .iter()
        .any(|(name, value)| (name == "camera_z" || name == "focal") && *value <= 32.0)
    {
        return Err(format!(
            "particle recipe {id:?} camera or focal length is invalid"
        ));
    }
    if !valid {
        return Err(format!(
            "particle recipe {id:?} parameter relationships are invalid"
        ));
    }
    Ok(())
}

fn parse_rgb565(value: &str) -> Result<u16, String> {
    let hex = value
        .strip_prefix('#')
        .filter(|hex| hex.len() == 6)
        .ok_or_else(|| format!("invalid particle recipe color {value:?}"))?;
    let rgb = u32::from_str_radix(hex, 16)
        .map_err(|_| format!("invalid particle recipe color {value:?}"))?;
    let red = ((rgb >> 16) & 0xff) as u16 >> 3;
    let green = ((rgb >> 8) & 0xff) as u16 >> 2;
    let blue = (rgb & 0xff) as u16 >> 3;
    Ok((red << 11) | (green << 5) | blue)
}

pub(super) fn procedural_recipe(id: &str) -> &'static CompiledRecipe {
    recipe(&PROCEDURAL_FAMILY, id)
}

pub(super) fn form_recipe(id: &str) -> &'static CompiledRecipe {
    recipe(&FORM_FAMILY, id)
}

fn recipe(
    family: &'static LazyLock<Result<CompiledFamily, String>>,
    id: &str,
) -> &'static CompiledRecipe {
    family
        .as_ref()
        .expect("embedded particle recipe family must be valid")
        .recipes
        .iter()
        .find(|recipe| recipe.id == id)
        .expect("registered particle recipe must be embedded")
}

fn procedural_keys(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        "fire-embers" => &[
            "camera_z",
            "ember_life",
            "ember_x_span",
            "wind_amplitude_fast",
            "wind_amplitude_slow",
            "wind_rate_fast",
            "wind_rate_slow",
        ],
        "spiral-galaxy" => &[
            "arm_inner_radius",
            "arm_radial_span",
            "arm_winding",
            "bulge_fraction",
            "bulge_radius",
            "camera_z",
            "core_pulse_rate",
            "tilt",
            "yaw_rate",
        ],
        "warp-speed" => &[
            "accelerate_end",
            "calm_end",
            "cruise_end",
            "max_speed",
            "min_speed",
            "projection_bias",
            "projection_numerator",
            "spawn_x_span",
            "spawn_y_span",
        ],
        "meteor-shower" => &[
            "depth_speed",
            "focal",
            "head_depth",
            "radiant_x",
            "radiant_y",
            "star_camera_z",
            "star_drift_rate",
            "star_x_span",
            "star_y_span",
            "track_cycle",
        ],
        "weather" => &[
            "ash_wind",
            "rain_end",
            "rain_wind",
            "rain_wind_amplitude",
            "snow_end",
            "snow_gust",
            "spawn_x_span",
        ],
        "particle-portal" => &[
            "camera_z",
            "forward_rate",
            "major_radius",
            "minor_radius_min",
            "minor_radius_span",
            "pulse_rate",
            "reverse_rate",
            "tilt",
        ],
        "electric-storm" => &[
            "bolt_x_jitter",
            "branch_start",
            "bright_start",
            "charge_power",
            "charge_rate",
            "cloud_x_span",
            "cloud_y_span",
            "leader_start",
        ],
        "fountain-waterfall" => &[
            "camera_z",
            "fountain_end",
            "gravity",
            "morph_end",
            "radial_speed_min",
            "radial_speed_span",
            "vertical_speed",
            "vertical_speed_span",
        ],
        "arcade-cabinet" => &[
            "center_y_offset",
            "focal",
            "formation_end",
            "orbit_end",
            "return_end",
            "source_x_span",
            "source_y_span",
            "source_z_span",
        ],
        "procedural-sprite-materials" => &[
            "angle_rate",
            "angle_span",
            "gravity",
            "phase_rate",
            "spawn_x_span",
            "spawn_y_span",
            "speed_min",
            "speed_span",
        ],
        "variable-width-ribbons" => &[
            "depth_x",
            "depth_y",
            "motion_rate",
            "path_x",
            "path_y",
            "ribbon_count",
            "ribbon_samples",
            "streak_count",
        ],
        "curl-noise-flow-field" => &[
            "integration_scale",
            "normal_x",
            "normal_y",
            "phase_rate",
            "softening",
            "trail_scale",
            "velocity_scale",
            "vortex_offset",
        ],
        "density-bloom" => &[
            "cavity_radius",
            "pulse_amplitude",
            "pulse_base",
            "pulse_rate",
            "radius_min",
            "radius_span",
            "x_offset",
            "y_scale",
        ],
        "layered-child-systems" => &[
            "child_gravity",
            "cycle",
            "head_travel",
            "head_x_amplitude",
            "parent_spacing",
            "ring_growth",
            "ring_radius",
            "trail_length",
        ],
        "spatial-field-stack" => &[
            "cavity_min",
            "cavity_span",
            "left_center",
            "middle_center",
            "phase_rate",
            "right_center",
            "spawn_x_span",
            "spawn_y_span",
        ],
        "depth-aware-material-lod" => &[
            "corridor_min",
            "corridor_span",
            "depth_rate",
            "drift_min",
            "drift_span",
            "parallax_min",
            "parallax_span",
            "spawn_x_span",
            "spawn_y_span",
        ],
        "source-morph" => &[
            "arc_min",
            "arc_span",
            "morph_end",
            "morph_start",
            "return_start",
            "source_body_x",
            "source_body_y",
            "target_radius",
            "target_radius_jitter",
        ],
        "sdf-collision" => &[
            "bowl_curve",
            "bowl_y",
            "cool_rate",
            "spawn_x_span",
            "spawn_y_span",
            "sphere_radius",
            "sphere_x",
            "sphere_y",
            "warm_rate",
        ],
        "grid-flocking" => &[
            "alignment",
            "cavity_force",
            "cavity_radius",
            "chaser_force",
            "chaser_radius",
            "max_speed",
            "separation",
            "spawn_inner",
            "spawn_span",
            "spawn_y_span",
        ],
        _ => return None,
    })
}

fn form_keys(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        "fractal-grid-terrain" => &[
            "broad_x_amplitude",
            "broad_x_rate",
            "broad_z_amplitude",
            "broad_z_rate",
            "camera_z",
            "crest_depth",
            "crest_x",
            "crest_y",
            "world_x_span",
            "world_z_span",
        ],
        "layer-mapped-hologram" => &[
            "ball_radius",
            "base_half_x",
            "base_half_y",
            "base_half_z",
            "camera_z",
            "collar_radius",
            "fade_start",
            "pitch",
            "reveal_end",
            "shaft_radius",
            "yaw_rate",
        ],
        "spherical-field-observatory" => &[
            "camera_z",
            "field_x",
            "field_y",
            "field_z",
            "orbit_rate",
            "phase_y",
            "wave_turns",
            "wave_y",
            "wave_z",
            "world_x_span",
        ],
        "twisted-multi-form-cathedral" => &[
            "camera_z",
            "dome_center",
            "dome_radius",
            "dome_y",
            "pulse_amplitude",
            "pulse_rate",
            "spire_x",
            "spire_x_span",
            "spire_y_span",
            "twist_base",
            "twist_rate",
        ],
        "point-cloud-morph-passage" => &[
            "breakup",
            "camera_z",
            "manta_width",
            "manta_x",
            "morph_end",
            "morph_start",
            "return_end",
            "return_start",
            "ship_length",
            "ship_x",
            "ship_y_min",
            "ship_y_span",
        ],
        _ => return None,
    })
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FireworkFamily {
    schema: String,
    recipes: Vec<FireworkFamilyEntry>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FireworkFamilyEntry {
    duration_ms: u64,
    beats: BeatSpec,
    show: Value,
}

static FIREWORKS_FAMILY: LazyLock<Result<FireworkFamily, String>> = LazyLock::new(|| {
    let family: FireworkFamily = serde_json::from_str(FIREWORKS_FAMILY_JSON)
        .map_err(|error| format!("parse embedded fireworks family: {error}"))?;
    family.validate()?;
    Ok(family)
});

impl FireworkFamily {
    fn validate(&self) -> Result<(), String> {
        if self.schema != FIREWORKS_FAMILY_SCHEMA || self.recipes.len() != 12 {
            return Err("embedded fireworks family schema or recipe count is invalid".into());
        }
        let mut ids = Vec::with_capacity(self.recipes.len());
        for recipe in &self.recipes {
            if !(100..=120_000).contains(&recipe.duration_ms) {
                return Err("fireworks showcase duration is out of bounds".into());
            }
            recipe.beats.validate(recipe.duration_ms)?;
            let id = recipe
                .show
                .get("id")
                .and_then(Value::as_str)
                .ok_or("fireworks family show id must be a string")?;
            let show_duration = recipe
                .show
                .get("duration_ms")
                .and_then(Value::as_u64)
                .ok_or("fireworks family show duration must be an integer")?;
            let show_schema = recipe
                .show
                .get("schema")
                .and_then(Value::as_str)
                .ok_or("fireworks family show schema must be a string")?;
            if firework_schema(id) != Some(show_schema)
                || recipe.beats.repeat_ms != Some(show_duration)
                || ids.contains(&id)
            {
                return Err(
                    "fireworks family show ids and beat periods must be unique and aligned".into(),
                );
            }
            ids.push(id);
        }
        if FIREWORK_IDS.iter().any(|id| !ids.contains(id)) {
            return Err("fireworks family must contain every registered show exactly once".into());
        }
        Ok(())
    }

    fn has_show(&self, id: &str) -> bool {
        let normalized = id.trim().to_ascii_lowercase();
        self.recipes.iter().any(|recipe| {
            recipe.show.get("id").and_then(Value::as_str) == Some(normalized.as_str())
        })
    }

    fn show_json(&self, id: &str, schema: &str) -> Option<String> {
        let normalized = id.trim().to_ascii_lowercase();
        self.recipes.iter().find_map(|recipe| {
            let show_id = recipe.show.get("id")?.as_str()?;
            let show_schema = recipe.show.get("schema")?.as_str()?;
            (show_id == normalized && show_schema == schema)
                .then(|| serde_json::to_string(&recipe.show).ok())
                .flatten()
        })
    }

    fn duration_ms(&self, id: &str) -> Option<u64> {
        let normalized = id.trim().to_ascii_lowercase();
        self.recipes.iter().find_map(|recipe| {
            (recipe.show.get("id").and_then(Value::as_str) == Some(normalized.as_str()))
                .then_some(recipe.duration_ms)
        })
    }
}

const FIREWORK_IDS: [&str; 12] = [
    "solar-chrysanthemum",
    "recursive-halo",
    "copper-willow-rain",
    "phoenix-comet",
    "magnetic-flower",
    "oled-peony",
    "solar-chrysanthemum-v2",
    "recursive-halo-v2",
    "copper-willow-rain-v2",
    "phoenix-comet-v2",
    "magnetic-flower-v2",
    "oled-peony-v2",
];

fn firework_schema(id: &str) -> Option<&'static str> {
    FIREWORK_IDS
        .iter()
        .position(|expected| *expected == id)
        .map(|index| {
            if index < 6 {
                "mister-magik-firework-v1"
            } else {
                "mister-magik-firework-v2"
            }
        })
}

pub(super) fn embedded_firework_show(id: &str, schema: &str) -> Option<String> {
    let family = FIREWORKS_FAMILY.as_ref().ok()?;
    family.show_json(id, schema)
}

pub(super) fn embedded_firework_duration_ms(id: &str) -> Option<u64> {
    FIREWORKS_FAMILY.as_ref().ok()?.duration_ms(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fireworks_family_is_complete_and_valid() {
        let family = FIREWORKS_FAMILY.as_ref().unwrap();
        assert_eq!(family.recipes.len(), 12);
        assert!(
            embedded_firework_show("solar-chrysanthemum", "mister-magik-firework-v1").is_some()
        );
        assert!(
            embedded_firework_show("solar-chrysanthemum-v2", "mister-magik-firework-v2").is_some()
        );
    }

    #[test]
    fn live_families_require_the_complete_registered_set() {
        for embedded in [
            FIREWORKS_FAMILY_JSON,
            PROCEDURAL_FAMILY_JSON,
            FORM_FAMILY_JSON,
        ] {
            assert!(ParticleRecipeFamily::from_json(embedded.as_bytes()).is_ok());
            let mut value: Value = serde_json::from_str(embedded).unwrap();
            value["recipes"].as_array_mut().unwrap().pop();
            let incomplete = serde_json::to_vec(&value).unwrap();
            assert!(ParticleRecipeFamily::from_json(&incomplete).is_err());
        }
    }

    #[test]
    fn live_families_reject_unsafe_parameter_relationships() {
        let mut value: Value = serde_json::from_str(PROCEDURAL_FAMILY_JSON).unwrap();
        let warp = value["recipes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|recipe| recipe["id"].as_str() == Some("warp-speed"))
            .unwrap();
        warp["params"]["accelerate_end"] = warp["params"]["calm_end"].clone();
        let unsafe_family = serde_json::to_vec(&value).unwrap();
        assert!(ParticleRecipeFamily::from_json(&unsafe_family).is_err());
    }
}
