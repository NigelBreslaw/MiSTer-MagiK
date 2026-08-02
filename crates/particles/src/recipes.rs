// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Versioned, validated data contracts for the production particle effects.

use serde::Deserialize;

pub const MAGIK_RECIPE_SCHEMA_V1: &str = "mister-magik-particle-magik-v1";
pub const CABINET_RECIPE_SCHEMA_V1: &str = "mister-magik-particle-cabinet-v1";
pub const MAGIK_PARTICLE_COUNT_MAX: u32 = 524_288;
pub const CABINET_PARTICLE_COUNT_MAX: u32 = 12_288;
pub const EMBEDDED_MAGIK_RECIPE_JSON: &[u8] = include_bytes!("../assets/recipes/magik-v1.json");
pub const EMBEDDED_CABINET_RECIPE_JSON: &[u8] = include_bytes!("../assets/recipes/cabinet-v1.json");

const JSON_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;
const DURATION_FIELD_MAX_MS: u64 = 120_000;
const DURATION_TOTAL_MAX_MS: u64 = 120_000;
const MOTION_VALUE_MAX: f32 = 4_096.0;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RecipeEasing {
    Linear,
    Smoothstep,
    EaseOutCubic,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikRecipeFileV1 {
    pub schema: String,
    pub particle_count: u32,
    pub seed: u64,
    pub timing: MagikTimingFileV1,
    pub initial: MagikInitialFileV1,
    pub depth: MagikDepthFileV1,
    pub projection: MagikProjectionFileV1,
    pub rotation: MagikRotationFileV1,
    pub static_motion: MagikStaticMotionFileV1,
    pub form_motion: MagikAttractionMotionFileV1,
    pub hold_motion: MagikAttractionMotionFileV1,
    pub disperse_motion: MagikDisperseMotionFileV1,
    pub appearance: MagikAppearanceFileV1,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikTimingFileV1 {
    pub static_ms: u64,
    pub form_ms: u64,
    pub hold_ms: u64,
    pub disperse_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikInitialFileV1 {
    pub duplicate_target_jitter_px: f32,
    pub velocity_xy_max: f32,
    pub velocity_z_max: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikDepthFileV1 {
    pub particle_extent: f32,
    pub target_extent: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikProjectionFileV1 {
    pub focal_length: f32,
    pub near_denominator: f32,
    pub center_offset_x: f32,
    pub center_offset_y: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikRotationFileV1 {
    pub hold_turns: f32,
    pub easing: RecipeEasing,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikStaticMotionFileV1 {
    pub acceleration_xy: f32,
    pub acceleration_z: f32,
    pub damping_xy: f32,
    pub damping_z: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikAttractionMotionFileV1 {
    pub stiffness: f32,
    pub jitter_px: f32,
    pub damping: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikDisperseMotionFileV1 {
    pub outward_acceleration: f32,
    pub jitter_xy: f32,
    pub jitter_z: f32,
    pub damping: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MagikAppearanceFileV1 {
    pub background: String,
    pub palette: [String; 4],
    pub formed_neighbor_when_depth_below: f32,
    pub unformed_palette_index: u8,
    pub neighbor_palette_index: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MagikRecipe {
    pub particle_count: usize,
    pub seed: u64,
    pub timing: MagikTiming,
    pub initial: MagikInitial,
    pub depth: MagikDepth,
    pub projection: MagikProjection,
    pub rotation: MagikRotation,
    pub static_motion: MagikStaticMotion,
    pub form_motion: MagikAttractionMotion,
    pub hold_motion: MagikAttractionMotion,
    pub disperse_motion: MagikDisperseMotion,
    pub appearance: MagikAppearance,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagikTiming {
    pub static_ms: u64,
    pub form_ms: u64,
    pub hold_ms: u64,
    pub disperse_ms: u64,
    pub cycle_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagikInitial {
    pub duplicate_target_jitter_px: f32,
    pub velocity_xy_max: f32,
    pub velocity_z_max: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagikDepth {
    pub particle_extent: f32,
    pub target_extent: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagikProjection {
    pub focal_length: f32,
    pub near_denominator: f32,
    pub center_offset_x: f32,
    pub center_offset_y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagikRotation {
    pub hold_turns: f32,
    pub easing: RecipeEasing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagikStaticMotion {
    pub acceleration_xy: f32,
    pub acceleration_z: f32,
    pub damping_xy: f32,
    pub damping_z: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagikAttractionMotion {
    pub stiffness: f32,
    pub jitter_px: f32,
    pub damping: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagikDisperseMotion {
    pub outward_acceleration: f32,
    pub jitter_xy: f32,
    pub jitter_z: f32,
    pub damping: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct RecipeRgb565(pub u16);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagikAppearance {
    pub background: RecipeRgb565,
    pub palette: [RecipeRgb565; 4],
    pub formed_neighbor_when_depth_below: f32,
    pub unformed_palette_index: u8,
    pub neighbor_palette_index: u8,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CabinetRecipeFileV1 {
    pub schema: String,
    pub particle_count: u32,
    pub seed: u64,
    pub timing: CabinetTimingFileV1,
    pub model: CabinetModelFileV1,
    pub source_scatter: CabinetSourceScatterFileV1,
    pub dispersal: CabinetDispersalFileV1,
    pub camera: CabinetCameraFileV1,
    pub appearance: CabinetAppearanceFileV1,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CabinetTimingFileV1 {
    pub formation_ms: u64,
    pub orbit_ms: u64,
    pub return_ms: u64,
    pub disperse_ms: u64,
    pub formation_easing: RecipeEasing,
    pub return_easing: RecipeEasing,
    pub disperse_easing: RecipeEasing,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CabinetModelFileV1 {
    pub x_half_extent: f32,
    pub y_extent: f32,
    pub z_half_extent: f32,
    pub y_origin: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CabinetSourceScatterFileV1 {
    pub x_half_extent: f32,
    pub y_half_extent: f32,
    pub z_half_extent: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CabinetDispersalFileV1 {
    pub radial_base: f32,
    pub radial_life_gain: f32,
    pub vertical_jitter: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CabinetCameraFileV1 {
    pub center_offset_x: f32,
    pub center_offset_y: f32,
    pub focal_length: f32,
    pub near_depth: f32,
    pub formation: CabinetPoseFileV1,
    pub orbit: CabinetOrbitFileV1,
    pub return_pose: CabinetPoseFileV1,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CabinetPoseFileV1 {
    pub yaw_radians: f32,
    pub pitch_radians: f32,
    pub dolly: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CabinetOrbitFileV1 {
    pub yaw_center_radians: f32,
    pub yaw_amplitude_radians: f32,
    pub yaw_turns: f32,
    pub pitch_triangle_rate: f32,
    pub pitch_amplitude_radians: f32,
    pub dolly_center: f32,
    pub dolly_triangle_rate: f32,
    pub dolly_triangle_phase: f32,
    pub dolly_amplitude: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CabinetAppearanceFileV1 {
    pub background: String,
    pub palette: [String; 8],
    pub priority_feature_mask: u8,
    pub priority_palette_index: u8,
    pub accent_feature_mask: u8,
    pub accent_palette_add: u8,
    pub neighbor_feature_mask: u8,
    pub neighbor_every: u16,
    pub neighbor_palette_subtract: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CabinetRecipe {
    pub particle_count: usize,
    pub seed: u64,
    pub timing: CabinetTiming,
    pub model: CabinetModel,
    pub source_scatter: CabinetSourceScatter,
    pub dispersal: CabinetDispersal,
    pub camera: CabinetCamera,
    pub appearance: CabinetAppearance,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CabinetTiming {
    pub formation_ms: u64,
    pub orbit_ms: u64,
    pub return_ms: u64,
    pub disperse_ms: u64,
    pub cycle_ms: u64,
    pub formation_easing: RecipeEasing,
    pub return_easing: RecipeEasing,
    pub disperse_easing: RecipeEasing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CabinetModel {
    pub x_half_extent: f32,
    pub y_extent: f32,
    pub z_half_extent: f32,
    pub y_origin: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CabinetSourceScatter {
    pub x_half_extent: f32,
    pub y_half_extent: f32,
    pub z_half_extent: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CabinetDispersal {
    pub radial_base: f32,
    pub radial_life_gain: f32,
    pub vertical_jitter: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CabinetCamera {
    pub center_offset_x: f32,
    pub center_offset_y: f32,
    pub focal_length: f32,
    pub near_depth: f32,
    pub formation: CabinetPose,
    pub orbit: CabinetOrbit,
    pub return_pose: CabinetPose,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CabinetPose {
    pub yaw_radians: f32,
    pub pitch_radians: f32,
    pub dolly: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CabinetOrbit {
    pub yaw_center_radians: f32,
    pub yaw_amplitude_radians: f32,
    pub yaw_turns: f32,
    pub pitch_triangle_rate: f32,
    pub pitch_amplitude_radians: f32,
    pub dolly_center: f32,
    pub dolly_triangle_rate: f32,
    pub dolly_triangle_phase: f32,
    pub dolly_amplitude: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CabinetAppearance {
    pub background: RecipeRgb565,
    pub palette: [RecipeRgb565; 8],
    pub priority_feature_mask: u8,
    pub priority_palette_index: u8,
    pub accent_feature_mask: u8,
    pub accent_palette_add: u8,
    pub neighbor_feature_mask: u8,
    pub neighbor_every: u16,
    pub neighbor_palette_subtract: u8,
}

pub fn parse_magik_recipe(bytes: &[u8]) -> Result<MagikRecipe, String> {
    let file: MagikRecipeFileV1 = serde_json::from_slice(bytes)
        .map_err(|error| format!("parse Magik particle recipe: {error}"))?;
    file.try_into()
}

pub fn parse_cabinet_recipe(bytes: &[u8]) -> Result<CabinetRecipe, String> {
    let file: CabinetRecipeFileV1 = serde_json::from_slice(bytes)
        .map_err(|error| format!("parse cabinet particle recipe: {error}"))?;
    file.try_into()
}

pub fn embedded_magik_recipe() -> Result<MagikRecipe, String> {
    parse_magik_recipe(EMBEDDED_MAGIK_RECIPE_JSON)
}

pub fn embedded_cabinet_recipe() -> Result<CabinetRecipe, String> {
    parse_cabinet_recipe(EMBEDDED_CABINET_RECIPE_JSON)
}

impl TryFrom<MagikRecipeFileV1> for MagikRecipe {
    type Error = String;

    fn try_from(file: MagikRecipeFileV1) -> Result<Self, Self::Error> {
        validate_schema(&file.schema, MAGIK_RECIPE_SCHEMA_V1, "Magik")?;
        validate_particle_count(
            file.particle_count,
            MAGIK_PARTICLE_COUNT_MAX,
            "Magik particle_count",
        )?;
        validate_seed(file.seed)?;
        let timing = validate_magik_timing(file.timing)?;
        let initial = validate_magik_initial(file.initial)?;
        let depth = validate_magik_depth(file.depth)?;
        let projection = validate_magik_projection(file.projection)?;
        let rotation = validate_magik_rotation(file.rotation)?;
        let static_motion = validate_magik_static_motion(file.static_motion)?;
        let form_motion = validate_magik_attraction(file.form_motion, "form_motion")?;
        let hold_motion = validate_magik_attraction(file.hold_motion, "hold_motion")?;
        let disperse_motion = validate_magik_disperse(file.disperse_motion)?;
        let appearance = validate_magik_appearance(file.appearance, depth.particle_extent)?;
        Ok(Self {
            particle_count: file.particle_count as usize,
            seed: file.seed,
            timing,
            initial,
            depth,
            projection,
            rotation,
            static_motion,
            form_motion,
            hold_motion,
            disperse_motion,
            appearance,
        })
    }
}

impl TryFrom<CabinetRecipeFileV1> for CabinetRecipe {
    type Error = String;

    fn try_from(file: CabinetRecipeFileV1) -> Result<Self, Self::Error> {
        validate_schema(&file.schema, CABINET_RECIPE_SCHEMA_V1, "cabinet")?;
        validate_particle_count(
            file.particle_count,
            CABINET_PARTICLE_COUNT_MAX,
            "cabinet particle_count",
        )?;
        validate_seed(file.seed)?;
        Ok(Self {
            particle_count: file.particle_count as usize,
            seed: file.seed,
            timing: validate_cabinet_timing(file.timing)?,
            model: validate_cabinet_model(file.model)?,
            source_scatter: validate_cabinet_source_scatter(file.source_scatter)?,
            dispersal: validate_cabinet_dispersal(file.dispersal)?,
            camera: validate_cabinet_camera(file.camera)?,
            appearance: validate_cabinet_appearance(file.appearance)?,
        })
    }
}

fn validate_schema(actual: &str, expected: &str, effect: &str) -> Result<(), String> {
    if actual != expected {
        return Err(format!(
            "{effect} particle recipe schema must be {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

fn validate_particle_count(value: u32, maximum: u32, name: &str) -> Result<(), String> {
    if !(1..=maximum).contains(&value) {
        return Err(format!("{name} must be in 1..={maximum}"));
    }
    Ok(())
}

fn validate_seed(seed: u64) -> Result<(), String> {
    if seed > JSON_SAFE_INTEGER_MAX {
        return Err(format!(
            "particle seed must be no greater than {JSON_SAFE_INTEGER_MAX}"
        ));
    }
    Ok(())
}

fn validate_duration(value: u64, name: &str) -> Result<(), String> {
    if !(1..=DURATION_FIELD_MAX_MS).contains(&value) {
        return Err(format!("{name} must be in 1..={DURATION_FIELD_MAX_MS}"));
    }
    Ok(())
}

fn duration_total(values: &[u64], minimum: u64, name: &str) -> Result<u64, String> {
    let total = values.iter().try_fold(0_u64, |total, value| {
        total
            .checked_add(*value)
            .ok_or_else(|| format!("{name} duration sum overflowed"))
    })?;
    if !(minimum..=DURATION_TOTAL_MAX_MS).contains(&total) {
        return Err(format!(
            "{name} total duration must be in {minimum}..={DURATION_TOTAL_MAX_MS}"
        ));
    }
    Ok(total)
}

fn validate_magik_timing(file: MagikTimingFileV1) -> Result<MagikTiming, String> {
    validate_duration(file.static_ms, "timing.static_ms")?;
    validate_duration(file.form_ms, "timing.form_ms")?;
    validate_duration(file.hold_ms, "timing.hold_ms")?;
    validate_duration(file.disperse_ms, "timing.disperse_ms")?;
    let cycle_ms = duration_total(
        &[file.static_ms, file.form_ms, file.hold_ms, file.disperse_ms],
        100,
        "Magik",
    )?;
    Ok(MagikTiming {
        static_ms: file.static_ms,
        form_ms: file.form_ms,
        hold_ms: file.hold_ms,
        disperse_ms: file.disperse_ms,
        cycle_ms,
    })
}

fn validate_magik_initial(file: MagikInitialFileV1) -> Result<MagikInitial, String> {
    finite_range(
        file.duplicate_target_jitter_px,
        0.0,
        MOTION_VALUE_MAX,
        "initial.duplicate_target_jitter_px",
    )?;
    finite_range(
        file.velocity_xy_max,
        0.0,
        MOTION_VALUE_MAX,
        "initial.velocity_xy_max",
    )?;
    finite_range(
        file.velocity_z_max,
        0.0,
        MOTION_VALUE_MAX,
        "initial.velocity_z_max",
    )?;
    Ok(MagikInitial {
        duplicate_target_jitter_px: file.duplicate_target_jitter_px,
        velocity_xy_max: file.velocity_xy_max,
        velocity_z_max: file.velocity_z_max,
    })
}

fn validate_magik_depth(file: MagikDepthFileV1) -> Result<MagikDepth, String> {
    finite_range_exclusive_min(file.particle_extent, 0.0, 255.0, "depth.particle_extent")?;
    finite_range(file.target_extent, 0.0, 31.75, "depth.target_extent")?;
    if (file.target_extent * 4.0).fract() != 0.0 {
        return Err("depth.target_extent must use 0.25 increments".into());
    }
    if file.target_extent > file.particle_extent {
        return Err("depth.target_extent must not exceed depth.particle_extent".into());
    }
    Ok(MagikDepth {
        particle_extent: file.particle_extent,
        target_extent: file.target_extent,
    })
}

fn validate_magik_projection(file: MagikProjectionFileV1) -> Result<MagikProjection, String> {
    finite_range(file.focal_length, 32.0, 4_096.0, "projection.focal_length")?;
    finite_range_exclusive_min(
        file.near_denominator,
        0.0,
        file.focal_length,
        "projection.near_denominator",
    )?;
    finite_absolute(file.center_offset_x, 4_096.0, "projection.center_offset_x")?;
    finite_absolute(file.center_offset_y, 4_096.0, "projection.center_offset_y")?;
    Ok(MagikProjection {
        focal_length: file.focal_length,
        near_denominator: file.near_denominator,
        center_offset_x: file.center_offset_x,
        center_offset_y: file.center_offset_y,
    })
}

fn validate_magik_rotation(file: MagikRotationFileV1) -> Result<MagikRotation, String> {
    finite_absolute(file.hold_turns, 16.0, "rotation.hold_turns")?;
    Ok(MagikRotation {
        hold_turns: file.hold_turns,
        easing: file.easing,
    })
}

fn validate_magik_static_motion(
    file: MagikStaticMotionFileV1,
) -> Result<MagikStaticMotion, String> {
    finite_range(
        file.acceleration_xy,
        0.0,
        MOTION_VALUE_MAX,
        "static_motion.acceleration_xy",
    )?;
    finite_range(
        file.acceleration_z,
        0.0,
        MOTION_VALUE_MAX,
        "static_motion.acceleration_z",
    )?;
    finite_range(file.damping_xy, 0.0, 1.0, "static_motion.damping_xy")?;
    finite_range(file.damping_z, 0.0, 1.0, "static_motion.damping_z")?;
    Ok(MagikStaticMotion {
        acceleration_xy: file.acceleration_xy,
        acceleration_z: file.acceleration_z,
        damping_xy: file.damping_xy,
        damping_z: file.damping_z,
    })
}

fn validate_magik_attraction(
    file: MagikAttractionMotionFileV1,
    name: &str,
) -> Result<MagikAttractionMotion, String> {
    finite_range(
        file.stiffness,
        0.0,
        MOTION_VALUE_MAX,
        &format!("{name}.stiffness"),
    )?;
    finite_range(
        file.jitter_px,
        0.0,
        MOTION_VALUE_MAX,
        &format!("{name}.jitter_px"),
    )?;
    finite_range(file.damping, 0.0, 1.0, &format!("{name}.damping"))?;
    Ok(MagikAttractionMotion {
        stiffness: file.stiffness,
        jitter_px: file.jitter_px,
        damping: file.damping,
    })
}

fn validate_magik_disperse(file: MagikDisperseMotionFileV1) -> Result<MagikDisperseMotion, String> {
    finite_range(
        file.outward_acceleration,
        0.0,
        MOTION_VALUE_MAX,
        "disperse_motion.outward_acceleration",
    )?;
    finite_range(
        file.jitter_xy,
        0.0,
        MOTION_VALUE_MAX,
        "disperse_motion.jitter_xy",
    )?;
    finite_range(
        file.jitter_z,
        0.0,
        MOTION_VALUE_MAX,
        "disperse_motion.jitter_z",
    )?;
    finite_range(file.damping, 0.0, 1.0, "disperse_motion.damping")?;
    Ok(MagikDisperseMotion {
        outward_acceleration: file.outward_acceleration,
        jitter_xy: file.jitter_xy,
        jitter_z: file.jitter_z,
        damping: file.damping,
    })
}

fn validate_magik_appearance(
    file: MagikAppearanceFileV1,
    depth_extent: f32,
) -> Result<MagikAppearance, String> {
    finite_range(
        file.formed_neighbor_when_depth_below,
        -depth_extent,
        depth_extent,
        "appearance.formed_neighbor_when_depth_below",
    )?;
    validate_palette_index(file.unformed_palette_index, 4, "unformed_palette_index")?;
    validate_palette_index(file.neighbor_palette_index, 4, "neighbor_palette_index")?;
    Ok(MagikAppearance {
        background: parse_rgb565(&file.background, "appearance.background")?,
        palette: parse_palette(file.palette, "appearance.palette")?,
        formed_neighbor_when_depth_below: file.formed_neighbor_when_depth_below,
        unformed_palette_index: file.unformed_palette_index,
        neighbor_palette_index: file.neighbor_palette_index,
    })
}

fn validate_cabinet_timing(file: CabinetTimingFileV1) -> Result<CabinetTiming, String> {
    validate_duration(file.formation_ms, "timing.formation_ms")?;
    validate_duration(file.orbit_ms, "timing.orbit_ms")?;
    validate_duration(file.return_ms, "timing.return_ms")?;
    validate_duration(file.disperse_ms, "timing.disperse_ms")?;
    let cycle_ms = duration_total(
        &[
            file.formation_ms,
            file.orbit_ms,
            file.return_ms,
            file.disperse_ms,
        ],
        1,
        "cabinet",
    )?;
    Ok(CabinetTiming {
        formation_ms: file.formation_ms,
        orbit_ms: file.orbit_ms,
        return_ms: file.return_ms,
        disperse_ms: file.disperse_ms,
        cycle_ms,
        formation_easing: file.formation_easing,
        return_easing: file.return_easing,
        disperse_easing: file.disperse_easing,
    })
}

fn validate_cabinet_model(file: CabinetModelFileV1) -> Result<CabinetModel, String> {
    finite_range(file.x_half_extent, 0.0, 4_096.0, "model.x_half_extent")?;
    finite_range(file.y_extent, 0.0, 4_096.0, "model.y_extent")?;
    finite_range(file.z_half_extent, 0.0, 4_096.0, "model.z_half_extent")?;
    finite_absolute(file.y_origin, 4_096.0, "model.y_origin")?;
    Ok(CabinetModel {
        x_half_extent: file.x_half_extent,
        y_extent: file.y_extent,
        z_half_extent: file.z_half_extent,
        y_origin: file.y_origin,
    })
}

fn validate_cabinet_source_scatter(
    file: CabinetSourceScatterFileV1,
) -> Result<CabinetSourceScatter, String> {
    finite_range(
        file.x_half_extent,
        0.0,
        4_096.0,
        "source_scatter.x_half_extent",
    )?;
    finite_range(
        file.y_half_extent,
        0.0,
        4_096.0,
        "source_scatter.y_half_extent",
    )?;
    finite_range(
        file.z_half_extent,
        0.0,
        4_096.0,
        "source_scatter.z_half_extent",
    )?;
    Ok(CabinetSourceScatter {
        x_half_extent: file.x_half_extent,
        y_half_extent: file.y_half_extent,
        z_half_extent: file.z_half_extent,
    })
}

fn validate_cabinet_dispersal(file: CabinetDispersalFileV1) -> Result<CabinetDispersal, String> {
    finite_range(file.radial_base, 0.0, 4_096.0, "dispersal.radial_base")?;
    finite_range(
        file.radial_life_gain,
        0.0,
        4_096.0,
        "dispersal.radial_life_gain",
    )?;
    finite_range(
        file.vertical_jitter,
        0.0,
        4_096.0,
        "dispersal.vertical_jitter",
    )?;
    Ok(CabinetDispersal {
        radial_base: file.radial_base,
        radial_life_gain: file.radial_life_gain,
        vertical_jitter: file.vertical_jitter,
    })
}

fn validate_cabinet_camera(file: CabinetCameraFileV1) -> Result<CabinetCamera, String> {
    finite_absolute(file.center_offset_x, 4_096.0, "camera.center_offset_x")?;
    finite_absolute(file.center_offset_y, 4_096.0, "camera.center_offset_y")?;
    finite_range(file.focal_length, 32.0, 4_096.0, "camera.focal_length")?;
    finite_range(file.near_depth, 0.0, 4_096.0, "camera.near_depth")?;
    Ok(CabinetCamera {
        center_offset_x: file.center_offset_x,
        center_offset_y: file.center_offset_y,
        focal_length: file.focal_length,
        near_depth: file.near_depth,
        formation: validate_cabinet_pose(file.formation, "camera.formation")?,
        orbit: validate_cabinet_orbit(file.orbit)?,
        return_pose: validate_cabinet_pose(file.return_pose, "camera.return_pose")?,
    })
}

fn validate_cabinet_pose(file: CabinetPoseFileV1, name: &str) -> Result<CabinetPose, String> {
    finite_absolute(file.yaw_radians, 16.0, &format!("{name}.yaw_radians"))?;
    finite_absolute(file.pitch_radians, 16.0, &format!("{name}.pitch_radians"))?;
    finite_range(file.dolly, 32.0, 4_096.0, &format!("{name}.dolly"))?;
    Ok(CabinetPose {
        yaw_radians: file.yaw_radians,
        pitch_radians: file.pitch_radians,
        dolly: file.dolly,
    })
}

fn validate_cabinet_orbit(file: CabinetOrbitFileV1) -> Result<CabinetOrbit, String> {
    finite_absolute(
        file.yaw_center_radians,
        16.0,
        "camera.orbit.yaw_center_radians",
    )?;
    finite_absolute(
        file.yaw_amplitude_radians,
        16.0,
        "camera.orbit.yaw_amplitude_radians",
    )?;
    finite_absolute(file.yaw_turns, 16.0, "camera.orbit.yaw_turns")?;
    finite_absolute(
        file.pitch_triangle_rate,
        16.0,
        "camera.orbit.pitch_triangle_rate",
    )?;
    finite_absolute(
        file.pitch_amplitude_radians,
        16.0,
        "camera.orbit.pitch_amplitude_radians",
    )?;
    finite_range(
        file.dolly_center,
        32.0,
        4_096.0,
        "camera.orbit.dolly_center",
    )?;
    finite_absolute(
        file.dolly_triangle_rate,
        16.0,
        "camera.orbit.dolly_triangle_rate",
    )?;
    finite_absolute(
        file.dolly_triangle_phase,
        16.0,
        "camera.orbit.dolly_triangle_phase",
    )?;
    finite_range(
        file.dolly_amplitude,
        0.0,
        4_096.0,
        "camera.orbit.dolly_amplitude",
    )?;
    Ok(CabinetOrbit {
        yaw_center_radians: file.yaw_center_radians,
        yaw_amplitude_radians: file.yaw_amplitude_radians,
        yaw_turns: file.yaw_turns,
        pitch_triangle_rate: file.pitch_triangle_rate,
        pitch_amplitude_radians: file.pitch_amplitude_radians,
        dolly_center: file.dolly_center,
        dolly_triangle_rate: file.dolly_triangle_rate,
        dolly_triangle_phase: file.dolly_triangle_phase,
        dolly_amplitude: file.dolly_amplitude,
    })
}

fn validate_cabinet_appearance(file: CabinetAppearanceFileV1) -> Result<CabinetAppearance, String> {
    validate_feature_mask(file.priority_feature_mask, "priority_feature_mask")?;
    validate_feature_mask(file.accent_feature_mask, "accent_feature_mask")?;
    validate_feature_mask(file.neighbor_feature_mask, "neighbor_feature_mask")?;
    validate_palette_index(file.priority_palette_index, 8, "priority_palette_index")?;
    validate_palette_index(file.accent_palette_add, 8, "accent_palette_add")?;
    validate_palette_index(
        file.neighbor_palette_subtract,
        8,
        "neighbor_palette_subtract",
    )?;
    if !(1..=256).contains(&file.neighbor_every) {
        return Err("appearance.neighbor_every must be in 1..=256".into());
    }
    Ok(CabinetAppearance {
        background: parse_rgb565(&file.background, "appearance.background")?,
        palette: parse_palette(file.palette, "appearance.palette")?,
        priority_feature_mask: file.priority_feature_mask,
        priority_palette_index: file.priority_palette_index,
        accent_feature_mask: file.accent_feature_mask,
        accent_palette_add: file.accent_palette_add,
        neighbor_feature_mask: file.neighbor_feature_mask,
        neighbor_every: file.neighbor_every,
        neighbor_palette_subtract: file.neighbor_palette_subtract,
    })
}

fn validate_feature_mask(value: u8, name: &str) -> Result<(), String> {
    if value & !3 != 0 {
        return Err(format!(
            "appearance.{name} may only use feature bits 0 and 1"
        ));
    }
    Ok(())
}

fn validate_palette_index(value: u8, length: u8, name: &str) -> Result<(), String> {
    if value >= length {
        return Err(format!("appearance.{name} must be in 0..={}", length - 1));
    }
    Ok(())
}

fn finite_range(value: f32, minimum: f32, maximum: f32, name: &str) -> Result<(), String> {
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return Err(format!(
            "{name} must be finite and in {minimum}..={maximum}"
        ));
    }
    Ok(())
}

fn finite_range_exclusive_min(
    value: f32,
    minimum: f32,
    maximum: f32,
    name: &str,
) -> Result<(), String> {
    if !value.is_finite() || value <= minimum || value > maximum {
        return Err(format!(
            "{name} must be finite, greater than {minimum}, and no greater than {maximum}"
        ));
    }
    Ok(())
}

fn finite_absolute(value: f32, maximum: f32, name: &str) -> Result<(), String> {
    if !value.is_finite() || value.abs() > maximum {
        return Err(format!(
            "{name} must be finite with absolute value no greater than {maximum}"
        ));
    }
    Ok(())
}

fn parse_palette<const N: usize>(
    values: [String; N],
    name: &str,
) -> Result<[RecipeRgb565; N], String> {
    let mut colors = [RecipeRgb565(0); N];
    for (index, value) in values.iter().enumerate() {
        colors[index] = parse_rgb565(value, &format!("{name}[{index}]"))?;
    }
    Ok(colors)
}

fn parse_rgb565(value: &str, name: &str) -> Result<RecipeRgb565, String> {
    let bytes = value.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' {
        return Err(format!("{name} must use #RRGGBB syntax"));
    }
    let parse_component = |range: std::ops::Range<usize>| {
        std::str::from_utf8(&bytes[range])
            .ok()
            .and_then(|component| u8::from_str_radix(component, 16).ok())
            .ok_or_else(|| format!("{name} must use #RRGGBB syntax"))
    };
    let red = parse_component(1..3)?;
    let green = parse_component(3..5)?;
    let blue = parse_component(5..7)?;
    Ok(RecipeRgb565(
        (u16::from(red >> 3) << 11) | (u16::from(green >> 2) << 5) | u16::from(blue >> 3),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn magik_file() -> MagikRecipeFileV1 {
        serde_json::from_slice(EMBEDDED_MAGIK_RECIPE_JSON).unwrap()
    }

    fn cabinet_file() -> CabinetRecipeFileV1 {
        serde_json::from_slice(EMBEDDED_CABINET_RECIPE_JSON).unwrap()
    }

    #[test]
    fn embedded_magik_recipe_preserves_current_defaults() {
        let recipe = embedded_magik_recipe().unwrap();
        assert_eq!(recipe.particle_count, 16_384);
        assert_eq!(recipe.seed, 0x004d_6167_694b);
        assert_eq!(recipe.timing.cycle_ms, 10_000);
        assert_eq!(recipe.depth.particle_extent, 64.0);
        assert_eq!(recipe.projection.focal_length, 720.0);
        assert_eq!(recipe.appearance.background, RecipeRgb565(0));
        assert_eq!(
            recipe.appearance.palette,
            [
                RecipeRgb565(0x2104),
                RecipeRgb565(0x5aeb),
                RecipeRgb565(0xbdf7),
                RecipeRgb565(0xffff),
            ]
        );
    }

    #[test]
    fn embedded_cabinet_recipe_preserves_current_defaults() {
        let recipe = embedded_cabinet_recipe().unwrap();
        assert_eq!(recipe.particle_count, 12_288);
        assert_eq!(recipe.seed, 0x004d_6167_694b);
        assert_eq!(recipe.timing.cycle_ms, 30_000);
        assert_eq!(recipe.camera.focal_length, 610.0);
        assert_eq!(recipe.camera.formation.dolly, 760.0);
        assert_eq!(
            recipe.camera.formation.yaw_radians,
            std::f32::consts::FRAC_PI_2 - 0.62
        );
        assert_eq!(
            recipe.appearance.palette,
            [
                RecipeRgb565(0x18d3),
                RecipeRgb565(0x31d7),
                RecipeRgb565(0x02d3),
                RecipeRgb565(0x05bf),
                RecipeRgb565(0xb80c),
                RecipeRgb565(0xfaa5),
                RecipeRgb565(0xfec8),
                RecipeRgb565(0xffff),
            ]
        );
    }

    #[test]
    fn rgb888_is_truncated_to_the_exact_rgb565_words() {
        assert_eq!(
            parse_rgb565("#FF0000", "color").unwrap(),
            RecipeRgb565(0xf800)
        );
        assert_eq!(
            parse_rgb565("#00FF00", "color").unwrap(),
            RecipeRgb565(0x07e0)
        );
        assert_eq!(
            parse_rgb565("#0000FF", "color").unwrap(),
            RecipeRgb565(0x001f)
        );
        assert!(parse_rgb565("red", "color").is_err());
    }

    #[test]
    fn unknown_top_level_and_nested_fields_are_rejected() {
        let mut magik: serde_json::Value =
            serde_json::from_slice(EMBEDDED_MAGIK_RECIPE_JSON).unwrap();
        magik["surprise"] = true.into();
        assert!(parse_magik_recipe(&serde_json::to_vec(&magik).unwrap()).is_err());

        let mut cabinet: serde_json::Value =
            serde_json::from_slice(EMBEDDED_CABINET_RECIPE_JSON).unwrap();
        cabinet["camera"]["surprise"] = true.into();
        assert!(parse_cabinet_recipe(&serde_json::to_vec(&cabinet).unwrap()).is_err());
    }

    #[test]
    fn schema_and_safe_integer_seed_are_enforced() {
        let mut magik = magik_file();
        magik.schema = CABINET_RECIPE_SCHEMA_V1.into();
        assert!(MagikRecipe::try_from(magik).is_err());

        let mut cabinet = cabinet_file();
        cabinet.seed = JSON_SAFE_INTEGER_MAX + 1;
        assert!(CabinetRecipe::try_from(cabinet).is_err());
    }

    #[test]
    fn magik_rejects_unsafe_counts_timings_depth_and_motion() {
        let mut file = magik_file();
        file.particle_count = MAGIK_PARTICLE_COUNT_MAX + 1;
        assert!(MagikRecipe::try_from(file).is_err());

        let mut file = magik_file();
        file.timing.static_ms = DURATION_FIELD_MAX_MS;
        assert!(MagikRecipe::try_from(file).is_err());

        let mut file = magik_file();
        file.depth.target_extent = 10.1;
        assert!(MagikRecipe::try_from(file).is_err());

        let mut file = magik_file();
        file.hold_motion.damping = 1.01;
        assert!(MagikRecipe::try_from(file).is_err());

        let mut file = magik_file();
        file.projection.center_offset_x = f32::NAN;
        assert!(MagikRecipe::try_from(file).is_err());
    }

    #[test]
    fn cabinet_rejects_unsafe_count_camera_features_and_color() {
        let mut file = cabinet_file();
        file.particle_count = CABINET_PARTICLE_COUNT_MAX + 1;
        assert!(CabinetRecipe::try_from(file).is_err());

        let mut file = cabinet_file();
        file.camera.orbit.yaw_turns = 16.1;
        assert!(CabinetRecipe::try_from(file).is_err());

        let mut file = cabinet_file();
        file.appearance.neighbor_feature_mask = 4;
        assert!(CabinetRecipe::try_from(file).is_err());

        let mut file = cabinet_file();
        file.appearance.neighbor_every = 0;
        assert!(CabinetRecipe::try_from(file).is_err());

        let mut file = cabinet_file();
        file.appearance.palette[0] = "#xyzxyz".into();
        assert!(CabinetRecipe::try_from(file).is_err());
    }
}
