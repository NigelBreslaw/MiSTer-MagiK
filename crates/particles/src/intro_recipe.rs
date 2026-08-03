// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Strict declarative contract for the standalone startup-intro storyboard.

use crate::recipes::{RecipeEasing, RecipeRgb565};
use serde::Deserialize;
use std::collections::HashSet;

pub const INTRO_RECIPE_SCHEMA_V1: &str = "mister-magik-particle-intro-v1";
pub const EMBEDDED_INTRO_RECIPE_JSON: &[u8] = include_bytes!("../assets/recipes/intro-v1.json");
const MAX_DURATION_MS: u64 = 120_000;
const MAX_PARTICLES: usize = 524_288;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IntroTarget {
    Crt,
    Mister,
    Magik,
    Cloud,
    Cabinet,
    LauncherLive,
    LauncherLiveRgb565,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IntroTrack {
    pub id: String,
    pub source: String,
    pub destination: String,
    pub preserve: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IntroCue {
    CrtStatic {
        id: String,
        duration_ms: u64,
    },
    MorphTarget {
        id: String,
        duration_ms: u64,
        from: IntroTarget,
        to: IntroTarget,
        active_at_end: usize,
        easing: RecipeEasing,
    },
    HoldTarget {
        id: String,
        duration_ms: u64,
        target: IntroTarget,
    },
    LetterMorph {
        id: String,
        duration_ms: u64,
        from: IntroTarget,
        to: IntroTarget,
        turns: f32,
        stagger_ms: u64,
        easing: RecipeEasing,
    },
    Cloud {
        id: String,
        duration_ms: u64,
        from: IntroTarget,
        to: IntroTarget,
        turns: f32,
        letter_turns: f32,
        stagger_ms: u64,
        radius: f32,
        formation_start_percent: f32,
        formation_end_percent: f32,
        easing: RecipeEasing,
    },
    TargetOrbit {
        id: String,
        duration_ms: u64,
        target: IntroTarget,
        start_turns: f32,
        turns: f32,
        formation_percent: f32,
    },
    LauncherCrossfade {
        id: String,
        duration_ms: u64,
        from: IntroTarget,
        to: IntroTarget,
        easing: RecipeEasing,
    },
}

impl IntroCue {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::CrtStatic { id, .. }
            | Self::MorphTarget { id, .. }
            | Self::HoldTarget { id, .. }
            | Self::LetterMorph { id, .. }
            | Self::Cloud { id, .. }
            | Self::TargetOrbit { id, .. }
            | Self::LauncherCrossfade { id, .. } => id,
        }
    }

    #[must_use]
    pub const fn duration_ms(&self) -> u64 {
        match self {
            Self::CrtStatic { duration_ms, .. }
            | Self::MorphTarget { duration_ms, .. }
            | Self::HoldTarget { duration_ms, .. }
            | Self::LetterMorph { duration_ms, .. }
            | Self::Cloud { duration_ms, .. }
            | Self::TargetOrbit { duration_ms, .. }
            | Self::LauncherCrossfade { duration_ms, .. } => *duration_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IntroCamera {
    pub focal_length: f32,
    pub near_depth: f32,
    pub dolly: f32,
    pub center_offset_x: f32,
    pub center_offset_y: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IntroAppearanceFileV1 {
    pub background: String,
    pub crt_palette: [String; 4],
    pub text_palette: [String; 4],
    pub palette: [String; 8],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntroAppearance {
    pub background: RecipeRgb565,
    pub crt_palette: [RecipeRgb565; 4],
    pub text_palette: [RecipeRgb565; 4],
    pub palette: [RecipeRgb565; 8],
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct IntroRecipeFileV1 {
    schema: String,
    seed: u64,
    initial_particle_count: usize,
    steady_particle_count: usize,
    camera: IntroCamera,
    appearance: IntroAppearanceFileV1,
    tracks: Vec<IntroTrack>,
    cues: Vec<IntroCue>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IntroRecipe {
    pub seed: u64,
    pub initial_particle_count: usize,
    pub steady_particle_count: usize,
    pub camera: IntroCamera,
    pub appearance: IntroAppearance,
    pub tracks: Vec<IntroTrack>,
    pub cues: Vec<IntroCue>,
    pub total_ms: u64,
}

impl IntroRecipe {
    #[must_use]
    pub fn cue_at(&self, elapsed_ms: u64) -> (usize, u64) {
        let mut start = 0;
        for (index, cue) in self.cues.iter().enumerate() {
            let end = start + cue.duration_ms();
            if elapsed_ms < end {
                return (index, elapsed_ms.saturating_sub(start));
            }
            start = end;
        }
        (self.cues.len() - 1, self.cues.last().unwrap().duration_ms())
    }
}

pub fn embedded_intro_recipe() -> Result<IntroRecipe, String> {
    let recipe = parse_intro_recipe(EMBEDDED_INTRO_RECIPE_JSON)?;
    if recipe.total_ms != 20_000 {
        return Err("embedded intro recipe must total exactly 20000ms".into());
    }
    Ok(recipe)
}

pub fn parse_intro_recipe(bytes: &[u8]) -> Result<IntroRecipe, String> {
    let file: IntroRecipeFileV1 =
        serde_json::from_slice(bytes).map_err(|error| format!("parse intro recipe: {error}"))?;
    if file.schema != INTRO_RECIPE_SCHEMA_V1 {
        return Err(format!("unsupported intro recipe schema {:?}", file.schema));
    }
    if file.initial_particle_count == 0
        || file.initial_particle_count > MAX_PARTICLES
        || file.steady_particle_count == 0
        || file.steady_particle_count > file.initial_particle_count
        || !file.steady_particle_count.is_multiple_of(4)
        || !file.initial_particle_count.is_multiple_of(4)
    {
        return Err("intro particle counts are invalid or not four-lane aligned".into());
    }
    validate_finite(
        file.camera.focal_length,
        32.0,
        4_096.0,
        "camera.focal_length",
    )?;
    validate_finite(file.camera.near_depth, 0.01, 4_096.0, "camera.near_depth")?;
    validate_finite(file.camera.dolly, 1.0, 8_192.0, "camera.dolly")?;
    validate_finite(
        file.camera.center_offset_x,
        -4_096.0,
        4_096.0,
        "camera.center_offset_x",
    )?;
    validate_finite(
        file.camera.center_offset_y,
        -4_096.0,
        4_096.0,
        "camera.center_offset_y",
    )?;
    validate_tracks(&file.tracks)?;
    if file.cues.is_empty() {
        return Err("intro recipe must contain at least one cue".into());
    }
    let mut ids = HashSet::with_capacity(file.cues.len());
    let mut total_ms = 0_u64;
    for cue in &file.cues {
        if cue.id().is_empty() || !ids.insert(cue.id()) {
            return Err("intro cue ids must be non-empty and unique".into());
        }
        if cue.duration_ms() == 0 {
            return Err(format!("intro cue {:?} has zero duration", cue.id()));
        }
        total_ms = total_ms
            .checked_add(cue.duration_ms())
            .ok_or("intro duration overflow")?;
        validate_cue(cue, file.steady_particle_count)?;
    }
    if total_ms > MAX_DURATION_MS {
        return Err(format!("intro duration exceeds {MAX_DURATION_MS}ms"));
    }
    validate_storyboard(&file.cues)?;
    Ok(IntroRecipe {
        seed: file.seed,
        initial_particle_count: file.initial_particle_count,
        steady_particle_count: file.steady_particle_count,
        camera: file.camera,
        appearance: IntroAppearance {
            background: parse_rgb565(&file.appearance.background)?,
            crt_palette: parse_palette(file.appearance.crt_palette, "appearance.crt_palette")?,
            text_palette: parse_palette(file.appearance.text_palette, "appearance.text_palette")?,
            palette: file
                .appearance
                .palette
                .map(|color| parse_rgb565(&color))
                .into_iter()
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| "intro palette must contain eight colors")?,
        },
        tracks: file.tracks,
        cues: file.cues,
        total_ms,
    })
}

fn parse_palette<const N: usize>(
    colors: [String; N],
    name: &str,
) -> Result<[RecipeRgb565; N], String> {
    colors
        .map(|color| parse_rgb565(&color))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| format!("{name} must contain {N} colors"))
}

fn validate_tracks(tracks: &[IntroTrack]) -> Result<(), String> {
    let expected = [
        ("m", "mister:M", "magik:M", true),
        ("i-a", "mister:i", "magik:a", false),
        ("s-g", "mister:S", "magik:g", false),
        ("t-i", "mister:T", "magik:i", false),
        ("e-k-a", "mister:e", "magik:K-A", false),
        ("r-k-b", "mister:r", "magik:K-B", false),
    ];
    if tracks.len() != expected.len()
        || tracks.iter().zip(expected).any(|(track, expected)| {
            (
                track.id.as_str(),
                track.source.as_str(),
                track.destination.as_str(),
                track.preserve,
            ) != expected
        })
    {
        return Err("intro letter tracks do not match the supported six-track mapping".into());
    }
    Ok(())
}

fn validate_storyboard(cues: &[IntroCue]) -> Result<(), String> {
    let kinds = cues
        .iter()
        .map(|cue| match cue {
            IntroCue::CrtStatic { .. } => "crt_static",
            IntroCue::MorphTarget { .. } => "morph_target",
            IntroCue::HoldTarget { .. } => "hold_target",
            IntroCue::LetterMorph { .. } => "letter_morph",
            IntroCue::Cloud { .. } => "cloud",
            IntroCue::TargetOrbit { .. } => "target_orbit",
            IntroCue::LauncherCrossfade { .. } => "launcher_crossfade",
        })
        .collect::<Vec<_>>();
    let expected = [
        "crt_static",
        "morph_target",
        "hold_target",
        "letter_morph",
        "hold_target",
        "cloud",
        "target_orbit",
        "morph_target",
        "hold_target",
        "launcher_crossfade",
    ];
    if kinds != expected {
        return Err("intro cues do not follow the supported v1 storyboard".into());
    }
    Ok(())
}

fn validate_cue(cue: &IntroCue, steady_count: usize) -> Result<(), String> {
    match cue {
        IntroCue::MorphTarget { active_at_end, .. } if *active_at_end != steady_count => {
            Err("intro morph active_at_end must equal steady_particle_count".into())
        }
        IntroCue::LetterMorph {
            turns, stagger_ms, ..
        } => {
            validate_finite(*turns, -16.0, 16.0, "letter_morph.turns")?;
            if *stagger_ms > 2_000 {
                return Err("letter_morph.stagger_ms exceeds 2000".into());
            }
            Ok(())
        }
        IntroCue::Cloud {
            turns,
            letter_turns,
            stagger_ms,
            radius,
            formation_start_percent,
            formation_end_percent,
            ..
        } => {
            validate_finite(*turns, -16.0, 16.0, "cloud.turns")?;
            validate_finite(*letter_turns, -16.0, 16.0, "cloud.letter_turns")?;
            validate_finite(*radius, 0.0, 1_024.0, "cloud.radius")?;
            validate_percent(*formation_start_percent, "cloud.formation_start_percent")?;
            validate_percent(*formation_end_percent, "cloud.formation_end_percent")?;
            if formation_end_percent < formation_start_percent {
                return Err("cloud formation percentage must not move backwards".into());
            }
            if *stagger_ms > 2_000 {
                return Err("cloud.stagger_ms exceeds 2000".into());
            }
            Ok(())
        }
        IntroCue::TargetOrbit {
            start_turns,
            turns,
            formation_percent,
            ..
        } => {
            validate_finite(*start_turns, -16.0, 16.0, "target_orbit.start_turns")?;
            validate_finite(*turns, -16.0, 16.0, "target_orbit.turns")?;
            validate_percent(*formation_percent, "target_orbit.formation_percent")
        }
        _ => Ok(()),
    }
}

fn validate_percent(value: f32, name: &str) -> Result<(), String> {
    validate_finite(value, 0.0, 100.0, name)
}

fn validate_finite(value: f32, minimum: f32, maximum: f32, name: &str) -> Result<(), String> {
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "{name} must be finite and in {minimum}..={maximum}"
        ));
    }
    Ok(())
}

fn parse_rgb565(value: &str) -> Result<RecipeRgb565, String> {
    let digits = value
        .strip_prefix('#')
        .filter(|digits| digits.len() == 6)
        .ok_or_else(|| format!("invalid RGB color {value:?}"))?;
    let red = u8::from_str_radix(&digits[0..2], 16)
        .map_err(|_| format!("invalid RGB color {value:?}"))?;
    let green = u8::from_str_radix(&digits[2..4], 16)
        .map_err(|_| format!("invalid RGB color {value:?}"))?;
    let blue = u8::from_str_radix(&digits[4..6], 16)
        .map_err(|_| format!("invalid RGB color {value:?}"))?;
    Ok(RecipeRgb565(
        (u16::from(red) >> 3) << 11 | (u16::from(green) >> 2) << 5 | (u16::from(blue) >> 3),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_storyboard_is_exactly_twenty_seconds() {
        let recipe = embedded_intro_recipe().unwrap();
        assert_eq!(recipe.total_ms, 20_000);
        assert_eq!(recipe.initial_particle_count, 102_400);
        assert_eq!(recipe.steady_particle_count, 40_960);
        assert_eq!(recipe.cue_at(12_000), (6, 0));
        assert_eq!(recipe.cue_at(20_100), (9, 1_000));
        let IntroCue::Cloud {
            stagger_ms,
            formation_start_percent,
            formation_end_percent,
            ..
        } = &recipe.cues[5]
        else {
            panic!("sixth intro cue must form the cabinet");
        };
        assert_eq!(*stagger_ms, 100);
        assert_eq!(*formation_start_percent, 0.0);
        assert_eq!(*formation_end_percent, 95.0);
        let IntroCue::TargetOrbit {
            start_turns,
            turns,
            formation_percent,
            ..
        } = &recipe.cues[6]
        else {
            panic!("seventh intro cue must orbit the cabinet");
        };
        assert_eq!((*start_turns, *turns, *formation_percent), (0.3, 0.4, 95.0));
    }

    #[test]
    fn storyboard_rejects_unknown_fields_and_invalid_mapping() {
        let mut value: serde_json::Value =
            serde_json::from_slice(EMBEDDED_INTRO_RECIPE_JSON).unwrap();
        value["unexpected"] = serde_json::json!(true);
        assert!(parse_intro_recipe(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut value: serde_json::Value =
            serde_json::from_slice(EMBEDDED_INTRO_RECIPE_JSON).unwrap();
        value["tracks"][1]["destination"] = serde_json::json!("magik:wrong");
        assert!(parse_intro_recipe(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
