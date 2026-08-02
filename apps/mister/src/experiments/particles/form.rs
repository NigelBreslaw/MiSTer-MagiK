// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Archived runtime for Trapcode Form-style particle compositions.
//!
//! Form owns its sources and modifier semantics, while the showcase owns the
//! RGB565 command buffers, rasterizer, dirty slots, and presentation contract.

use std::f32::consts::{PI, TAU};
use std::time::Duration;

use super::recipes::{CompiledRecipe, form_recipe};

pub const FORM_MAX_PARTICLES: usize = 98_304;
pub const FORM_MAX_SEGMENTS: usize = 32_768;
const HOLOGRAM_BASE_END: usize = 20_480;
const HOLOGRAM_SHAFT_END: usize = 26_624;
const HOLOGRAM_BALL_END: usize = 32_768;
const HOLOGRAM_COLLAR_END: usize = 36_864;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FormSceneKind {
    FractalGridTerrain,
    LayerMappedHologram,
    SphericalFieldObservatory,
    TwistedMultiFormCathedral,
    PointCloudMorphPassage,
}

impl FormSceneKind {
    pub const ALL: [Self; 5] = [
        Self::FractalGridTerrain,
        Self::LayerMappedHologram,
        Self::SphericalFieldObservatory,
        Self::TwistedMultiFormCathedral,
        Self::PointCloudMorphPassage,
    ];

    #[must_use]
    pub fn count(self) -> usize {
        form_recipe(self.id()).particle_count
    }

    #[must_use]
    pub fn beat(self, seconds: f32) -> &'static str {
        form_recipe(self.id()).beat((seconds * 1000.0) as u64)
    }

    const fn id(self) -> &'static str {
        match self {
            Self::FractalGridTerrain => "fractal-grid-terrain",
            Self::LayerMappedHologram => "layer-mapped-hologram",
            Self::SphericalFieldObservatory => "spherical-field-observatory",
            Self::TwistedMultiFormCathedral => "twisted-multi-form-cathedral",
            Self::PointCloudMorphPassage => "point-cloud-morph-passage",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FormPoint {
    pub x: f32,
    pub y: f32,
    pub style: u8,
    pub neighbor: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FormSegment {
    pub x0: i16,
    pub y0: i16,
    pub x1: i16,
    pub y1: i16,
    pub style: u8,
}

pub struct FormSceneRenderer {
    scene: Option<FormSceneKind>,
    recipe: Option<CompiledRecipe>,
    seed: u64,
    rest_x: Vec<f32>,
    rest_y: Vec<f32>,
    rest_z: Vec<f32>,
    target_x: Vec<f32>,
    target_y: Vec<f32>,
    target_z: Vec<f32>,
    aux_x: Vec<f32>,
    aux_y: Vec<f32>,
    style: Vec<u8>,
    flags: Vec<u8>,
    points: Vec<FormPoint>,
    segments: Vec<FormSegment>,
}

impl FormSceneRenderer {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            scene: None,
            recipe: None,
            seed,
            rest_x: vec![0.0; FORM_MAX_PARTICLES],
            rest_y: vec![0.0; FORM_MAX_PARTICLES],
            rest_z: vec![0.0; FORM_MAX_PARTICLES],
            target_x: vec![0.0; FORM_MAX_PARTICLES],
            target_y: vec![0.0; FORM_MAX_PARTICLES],
            target_z: vec![0.0; FORM_MAX_PARTICLES],
            aux_x: vec![0.0; FORM_MAX_PARTICLES],
            aux_y: vec![0.0; FORM_MAX_PARTICLES],
            style: vec![0; FORM_MAX_PARTICLES],
            flags: vec![0; FORM_MAX_PARTICLES],
            points: Vec::with_capacity(FORM_MAX_PARTICLES),
            segments: Vec::with_capacity(FORM_MAX_SEGMENTS),
        }
    }

    pub fn reset(&mut self, scene: FormSceneKind) {
        self.reset_with_recipe(scene, form_recipe(scene.id()));
    }

    pub(super) fn reset_with_recipe(&mut self, scene: FormSceneKind, recipe: &CompiledRecipe) {
        self.scene = Some(scene);
        self.recipe = Some(recipe.clone());
        self.points.clear();
        self.segments.clear();
        match scene {
            FormSceneKind::FractalGridTerrain => self.initialize_terrain(),
            FormSceneKind::LayerMappedHologram => self.initialize_hologram(),
            FormSceneKind::SphericalFieldObservatory => self.initialize_observatory(),
            FormSceneKind::TwistedMultiFormCathedral => self.initialize_cathedral(),
            FormSceneKind::PointCloudMorphPassage => self.initialize_morph(),
        }
    }

    pub fn project(
        &mut self,
        scene: FormSceneKind,
        elapsed: Duration,
        width: usize,
        height: usize,
    ) -> (&[FormPoint], &[FormSegment]) {
        if self.scene != Some(scene) {
            self.reset(scene);
        }
        self.points.clear();
        self.segments.clear();
        let seconds = elapsed.as_secs_f32().rem_euclid(self.duration_seconds());
        match scene {
            FormSceneKind::FractalGridTerrain => {
                self.project_terrain(seconds, width, height);
            }
            FormSceneKind::LayerMappedHologram => {
                self.project_hologram(seconds, width, height);
            }
            FormSceneKind::SphericalFieldObservatory => {
                self.project_observatory(seconds, width, height);
            }
            FormSceneKind::TwistedMultiFormCathedral => {
                self.project_cathedral(seconds, width, height);
            }
            FormSceneKind::PointCloudMorphPassage => {
                self.project_morph(seconds, width, height);
            }
        }
        (&self.points, &self.segments)
    }

    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        (self.rest_x.capacity()
            + self.rest_y.capacity()
            + self.rest_z.capacity()
            + self.target_x.capacity()
            + self.target_y.capacity()
            + self.target_z.capacity()
            + self.aux_x.capacity()
            + self.aux_y.capacity())
        .saturating_mul(std::mem::size_of::<f32>())
        .saturating_add(self.style.capacity())
        .saturating_add(self.flags.capacity())
        .saturating_add(
            self.points
                .capacity()
                .saturating_mul(std::mem::size_of::<FormPoint>()),
        )
        .saturating_add(
            self.segments
                .capacity()
                .saturating_mul(std::mem::size_of::<FormSegment>()),
        )
    }

    fn recipe(&self) -> &CompiledRecipe {
        self.recipe
            .as_ref()
            .expect("Form scene must be reset before rendering")
    }

    fn count(&self) -> usize {
        self.recipe().particle_count
    }

    fn duration_seconds(&self) -> f32 {
        self.recipe().duration_ms as f32 / 1000.0
    }

    fn param(&self, name: &str) -> f32 {
        self.recipe().param(name)
    }

    fn initialize_terrain(&mut self) {
        let width = 256usize;
        let height = 192usize;
        for index in 0..width * height {
            let ix = index % width;
            let iy = index / width;
            let u = ix as f32 / (width - 1) as f32;
            let v = iy as f32 / (height - 1) as f32;
            let x = (u - 0.5) * self.param("world_x_span");
            let z = (v - 0.5) * self.param("world_z_span");
            let broad = (x * self.param("broad_x_rate")).sin() * self.param("broad_x_amplitude")
                + (z * self.param("broad_z_rate")).cos() * self.param("broad_z_amplitude");
            let detail = ((x + z) * 0.041).sin() * 11.0 + ((x * 0.73 - z) * 0.067).cos() * 6.0;
            let crest = (-((u - self.param("crest_u")).powi(2) * 24.0
                + (v - self.param("crest_v")).powi(2) * 8.0))
                .exp();
            self.rest_x[index] = x;
            self.rest_y[index] = broad + detail - crest * self.param("crest_drop");
            self.rest_z[index] = z;
            self.target_x[index] = crest * (v - 0.5) * 130.0;
            self.target_y[index] = crest * self.param("crest_rise");
            self.aux_x[index] = u;
            self.aux_y[index] = v;
            self.style[index] = ((u * 3.0 + v * 5.0 + crest * 3.0) as u8).min(7);
            self.flags[index] = u8::from((ix + iy) & 3 == 0);
        }
    }

    fn initialize_hologram(&mut self) {
        let count = self.count();
        for index in 0..count {
            let (part_start, logical, part) = if index < HOLOGRAM_BASE_END {
                (0, index / 4, 0)
            } else if index < HOLOGRAM_SHAFT_END {
                (HOLOGRAM_BASE_END, (index - HOLOGRAM_BASE_END) / 4, 1)
            } else if index < HOLOGRAM_BALL_END {
                (HOLOGRAM_SHAFT_END, (index - HOLOGRAM_SHAFT_END) / 4, 2)
            } else if index < HOLOGRAM_COLLAR_END {
                (HOLOGRAM_BALL_END, (index - HOLOGRAM_BALL_END) / 4, 3)
            } else {
                (HOLOGRAM_COLLAR_END, (index - HOLOGRAM_COLLAR_END) / 4, 4)
            };
            let state = xorshift32(
                (logical as u32)
                    .wrapping_mul(0x9e37_79b9)
                    .wrapping_add(part_start as u32)
                    .wrapping_add(self.seed as u32),
            );
            let a = unit01(state);
            let (x, y, z, style) = match part {
                0 => {
                    const TOP_SAMPLES: usize = 64 * 44;
                    const SIDE_SAMPLES: usize = 32 * 18;
                    let half_x = self.param("base_half_x");
                    let half_y = self.param("base_half_y");
                    let half_z = self.param("base_half_z");
                    let center_y = 96.0;
                    if logical < TOP_SAMPLES {
                        let ix = logical & 63;
                        let iz = logical >> 6;
                        (
                            lerp(-half_x, half_x, ix as f32 / 63.0),
                            center_y - half_y,
                            lerp(-half_z, half_z, iz as f32 / 43.0),
                            5,
                        )
                    } else {
                        let side_index = logical - TOP_SAMPLES;
                        let face = side_index / SIDE_SAMPLES;
                        let sample = side_index % SIDE_SAMPLES;
                        let across = (sample & 31) as f32 / 31.0;
                        let down = (sample >> 5) as f32 / 17.0;
                        match face {
                            0 => (
                                lerp(-half_x, half_x, across),
                                lerp(center_y - half_y, center_y + half_y, down),
                                -half_z,
                                3,
                            ),
                            1 => (
                                lerp(-half_x, half_x, across),
                                lerp(center_y - half_y, center_y + half_y, down),
                                half_z,
                                4,
                            ),
                            2 => (
                                -half_x,
                                lerp(center_y - half_y, center_y + half_y, down),
                                lerp(-half_z, half_z, across),
                                3,
                            ),
                            _ => (
                                half_x,
                                lerp(center_y - half_y, center_y + half_y, down),
                                lerp(-half_z, half_z, across),
                                4,
                            ),
                        }
                    }
                }
                1 => {
                    let angle = (logical & 63) as f32 * TAU / 64.0;
                    let height = (logical >> 6) as f32 / 23.0;
                    let radius = self.param("shaft_radius");
                    (
                        angle.cos() * radius,
                        45.0 - height * 165.0,
                        angle.sin() * radius,
                        5,
                    )
                }
                2 => {
                    let latitude = logical >> 6;
                    let longitude = logical & 63;
                    let latitude_angle = (latitude as f32 + 0.5) * PI / 24.0;
                    let sphere_y = -latitude_angle.cos();
                    let radial = latitude_angle.sin();
                    let front_longitude = longitude >> 1;
                    let angle = lerp(-PI * 0.5, PI * 0.5, front_longitude as f32 / 31.0);
                    let latitude_style = if sphere_y.abs() < 0.32 { 7 } else { 5 };
                    (
                        angle.sin() * radial * self.param("ball_radius"),
                        -169.0 + sphere_y * self.param("ball_radius"),
                        -angle.cos() * radial * self.param("ball_radius"),
                        latitude_style,
                    )
                }
                3 => {
                    // Four independently sampled annular terraces read much
                    // closer to the layered holographic collar than a single
                    // inflated torus.
                    let ring = logical & 3;
                    let ring_sample = logical >> 2;
                    let samples_per_ring = (HOLOGRAM_COLLAR_END - HOLOGRAM_BALL_END) / 16;
                    let major_angle = ring_sample as f32 * TAU / samples_per_ring as f32;
                    let radius = self.param("collar_radius") + ring as f32 * 17.0;
                    (
                        major_angle.cos() * radius,
                        41.0 + ring as f32 * 3.0,
                        major_angle.sin() * radius,
                        5,
                    )
                }
                _ => {
                    let button = logical % 3;
                    let button_sample = logical / 3;
                    let ring = button_sample % 3;
                    let ring_sample = button_sample / 3;
                    let angle = ring_sample as f32 * TAU / 114.0;
                    let radius = 24.0 + ring as f32 * 3.0;
                    let (center_x, center_z) = match button {
                        0 => (-122.0, -45.0),
                        1 => (118.0, -48.0),
                        _ => (112.0, 54.0),
                    };
                    (
                        center_x + angle.cos() * radius,
                        38.0,
                        center_z + angle.sin() * radius,
                        if button == 0 { 5 } else { 6 },
                    )
                }
            };
            let duplicate = (index & 3) as f32;
            self.rest_x[index] = x;
            self.rest_y[index] = y;
            self.rest_z[index] = z + duplicate * 0.15;
            self.aux_x[index] = if part == 4 { style as f32 } else { a };
            self.aux_y[index] = ((y + 230.0) / 400.0).clamp(0.0, 1.0);
            self.style[index] = style;
            self.flags[index] = 1 | (part << 1);
        }
    }

    fn initialize_observatory(&mut self) {
        let count = self.count();
        for index in 0..count {
            let trail = index & 127;
            let sample = index >> 7;
            let t = sample as f32 / 255.0;
            let phase = trail as f32 * (TAU / 128.0);
            let convergence = smootherstep(t);
            let wave = t * TAU * self.param("wave_turns") + phase;
            self.rest_x[index] = (t - 0.5) * self.param("world_x_span");
            self.rest_y[index] = wave.sin() * (self.param("wave_y") - convergence * 70.0)
                + phase.cos() * self.param("phase_y");
            self.rest_z[index] =
                phase.sin() * (self.param("wave_z") - convergence * 35.0) + wave.cos() * 32.0;
            self.aux_x[index] = phase.sin();
            self.aux_y[index] = phase.cos();
            self.style[index] = (2 + ((index >> 7) & 5)) as u8;
            self.flags[index] = u8::from(index & 31 == 0);
        }
    }

    fn initialize_cathedral(&mut self) {
        let count = self.count();
        for index in 0..count {
            let group = index & 3;
            let local = index / 4;
            let t = local as f32 / (count / 4) as f32;
            let angle = t * TAU * (3.0 + group as f32);
            let (x, y, z) = match group {
                0 => {
                    let side = if local & 1 == 0 { -1.0 } else { 1.0 };
                    (
                        side * (self.param("spire_x") + self.param("spire_x_span") * t),
                        (t - 0.5) * self.param("spire_y_span"),
                        0.0,
                    )
                }
                1 | 2 => {
                    let center = if group == 1 {
                        -self.param("dome_center")
                    } else {
                        self.param("dome_center")
                    };
                    let polar = (t * PI).sin();
                    (
                        center + angle.cos() * polar * self.param("dome_radius"),
                        (t * PI).cos() * self.param("dome_y"),
                        angle.sin() * polar * self.param("dome_radius"),
                    )
                }
                _ => (
                    angle.cos() * (70.0 + 150.0 * t),
                    (t - 0.5) * 360.0,
                    angle.sin() * (70.0 + 150.0 * t),
                ),
            };
            self.rest_x[index] = x;
            self.rest_y[index] = y;
            self.rest_z[index] = z;
            self.aux_x[index] = angle.sin();
            self.aux_y[index] = angle.cos();
            self.style[index] = (1 + group * 2 + (local & 1)) as u8;
            self.flags[index] = 1;
        }
    }

    fn initialize_morph(&mut self) {
        let count = self.count();
        for index in 0..count {
            let t = index as f32 / count as f32;
            let band = index % 96;
            let across = band as f32 / 95.0;
            let longitudinal = (index / 96) as f32 / ((count / 96) - 1) as f32;
            let ship_x = self.param("ship_x") + (longitudinal - 0.5) * self.param("ship_length");
            let ship_y = (across - 0.5)
                * (self.param("ship_y_min") + longitudinal * self.param("ship_y_span"));
            let ship_z = ((t * TAU * 17.0).sin()) * (18.0 + longitudinal * 35.0);
            self.rest_x[index] = ship_x;
            self.rest_y[index] = ship_y;
            self.rest_z[index] = ship_z;

            let wing = (across - 0.5) * 2.0;
            let manta_width = (1.0 - (longitudinal - 0.48).abs() * 1.6).max(0.08);
            self.target_x[index] =
                self.param("manta_x") + wing * manta_width * self.param("manta_width");
            self.target_y[index] = (longitudinal - 0.5) * 210.0 + wing.abs().powf(1.6) * 68.0;
            self.target_z[index] =
                wing.signum() * wing.abs().powi(2) * 55.0 + (longitudinal * TAU).sin() * 20.0;
            let phase = t * TAU * 7.0;
            self.aux_x[index] = phase.sin();
            self.aux_y[index] = phase.cos();
            self.style[index] = (2 + ((index >> 8) & 5)) as u8;
            self.flags[index] = u8::from(index & 255 == 0);
        }
    }

    fn project_terrain(&mut self, seconds: f32, width: usize, height: usize) {
        let count = self.count();
        let crest = smooth_envelope(seconds, 5.0, 15.0, 26.0);
        let angle = -0.22 + seconds * 0.012;
        let (sin, cos) = angle.sin_cos();
        for index in 0..count {
            let grid_x = index & 255;
            let grid_y = index >> 8;
            if grid_x & 1 != 0 || grid_y & 1 != 0 {
                continue;
            }
            let x = self.rest_x[index] + self.target_x[index] * crest;
            let y = self.rest_y[index] + self.target_y[index] * crest;
            let z = self.rest_z[index];
            let rx = x.mul_add(cos, z * sin);
            let rz = (-x).mul_add(sin, z * cos);
            let ry = y - 45.0;
            self.push_world(rx, ry, rz, self.param("camera_z"), width, height, index);
        }
    }

    fn project_hologram(&mut self, seconds: f32, width: usize, height: usize) {
        let count = self.count();
        let reveal_end = self.param("reveal_end");
        let fade_start = self.param("fade_start");
        let reveal = if seconds < reveal_end {
            smootherstep(seconds / reveal_end)
        } else if seconds < fade_start {
            1.0
        } else {
            1.0 - smootherstep(
                (seconds - fade_start) / (self.duration_seconds() - fade_start).max(0.001),
            ) * 0.65
        };
        let angle = -0.55 + seconds * self.param("yaw_rate");
        let (sin, cos) = angle.sin_cos();
        // Match the concept's high three-quarter presentation. Besides making
        // the silhouette legible, this exposes the top-plane material map and
        // separates the concentric collar terraces in screen space.
        let (pitch_sin, pitch_cos) = self.param("pitch").sin_cos();
        let scan_position = (seconds * 0.18).fract();
        for index in (0..count).step_by(4) {
            let part = self.flags[index] >> 1;
            let keep_sample = match part {
                0 => true,
                1 | 2 => ((index - HOLOGRAM_BASE_END) >> 2) & 1 == 0,
                3 => ((index - HOLOGRAM_BALL_END) >> 4) & 1 == 0,
                4 => ((index - HOLOGRAM_COLLAR_END) / 12) & 1 == 0,
                _ => false,
            };
            if !keep_sample {
                continue;
            }
            if self.aux_y[index] > reveal {
                continue;
            }
            let terrace = (self.aux_y[index] * 12.0).floor() * 1.8;
            let (x, y, z) = if part == 2 {
                let sphere_y = self.rest_y[index] + 169.0;
                let center_y: f32 = -169.0 - 18.0;
                (
                    self.rest_x[index],
                    center_y.mul_add(pitch_cos, sphere_y),
                    center_y.mul_add(pitch_sin, self.rest_z[index]),
                )
            } else {
                let x = self.rest_x[index].mul_add(cos, self.rest_z[index] * sin);
                let yaw_z = (-self.rest_x[index]).mul_add(sin, self.rest_z[index] * cos);
                let local_y = self.rest_y[index] + terrace - 18.0;
                let y = local_y.mul_add(pitch_cos, -yaw_z * pitch_sin);
                let z = local_y.mul_add(pitch_sin, yaw_z * pitch_cos);
                (x, y, z)
            };
            let original_style = self.style[index];
            self.style[index] = hologram_material_style(
                part,
                self.rest_x[index],
                self.rest_y[index],
                self.rest_z[index],
                self.aux_x[index],
                self.aux_y[index],
                scan_position,
            );
            self.push_world(x, y, z, self.param("camera_z"), width, height, index);
            self.style[index] = original_style;
        }
    }

    fn project_observatory(&mut self, seconds: f32, width: usize, height: usize) {
        let count = self.count();
        let orbit = seconds * self.param("orbit_rate");
        let (sin, cos) = orbit.sin_cos();
        let field = smooth_envelope(seconds, 2.0, 14.0, 27.0);
        for index in (0..count).step_by(2) {
            let phase_sin = self.aux_x[index].mul_add(cos, self.aux_y[index] * sin);
            let phase_cos = self.aux_y[index].mul_add(cos, -self.aux_x[index] * sin);
            let span = self.param("world_x_span");
            let convergence = ((self.rest_x[index] + span * 0.5) / span).clamp(0.0, 1.0);
            let x = self.rest_x[index] + convergence * phase_cos * field * self.param("field_x");
            let y = self.rest_y[index] + phase_sin * convergence * field * self.param("field_y");
            let z = self.rest_z[index] + phase_cos * convergence * field * self.param("field_z");
            self.push_world(x, y, z, self.param("camera_z"), width, height, index);
        }
        let visible = self.points.len();
        if visible != 0 {
            for index in (0..visible).step_by(256) {
                self.push_segment_from_points(index, (index + 7) % visible);
            }
        }
    }

    fn project_cathedral(&mut self, seconds: f32, width: usize, height: usize) {
        let count = self.count();
        let twist = self.param("twist_base") + seconds * self.param("twist_rate");
        let pulse =
            1.0 + (seconds * self.param("pulse_rate")).sin() * self.param("pulse_amplitude");
        for index in 0..count {
            if (index / 4) & 7 != 0 {
                continue;
            }
            let local_angle = twist * (self.rest_y[index] / 220.0);
            let (sin, cos) = local_angle.sin_cos();
            let x = self.rest_x[index].mul_add(cos, self.rest_z[index] * sin) * pulse;
            let z = (-self.rest_x[index]).mul_add(sin, self.rest_z[index] * cos);
            let y = self.rest_y[index] * pulse;
            self.push_world(x, y, z, self.param("camera_z"), width, height, index);
        }
        let visible = self.points.len();
        for index in (0..visible.saturating_sub(16)).step_by(16) {
            self.push_segment_from_points(index, index + 16);
        }
    }

    fn project_morph(&mut self, seconds: f32, width: usize, height: usize) {
        let count = self.count();
        let morph_start = self.param("morph_start");
        let morph_end = self.param("morph_end");
        let return_start = self.param("return_start");
        let return_end = self.param("return_end");
        let morph = if seconds < morph_start {
            0.0
        } else if seconds < morph_end {
            smootherstep((seconds - morph_start) / (morph_end - morph_start))
        } else if seconds < return_start {
            1.0
        } else if seconds < return_end {
            1.0 - smootherstep((seconds - return_start) / (return_end - return_start))
        } else {
            0.0
        };
        let breakup = (morph * PI).sin() * self.param("breakup");
        let angle = -0.28 + seconds * 0.025;
        let (sin, cos) = angle.sin_cos();
        for index in (0..count).step_by(2) {
            let assignment_rank = (index % 96) as f32 / 95.0;
            let local_morph = if morph <= 0.0 {
                0.0
            } else if morph >= 1.0 {
                1.0
            } else {
                smootherstep(((morph - assignment_rank * 0.7) / 0.3).clamp(0.0, 1.0))
            };
            let wobble = self.aux_x[index] * breakup;
            let x = lerp(self.rest_x[index], self.target_x[index], local_morph) + wobble;
            let y = lerp(self.rest_y[index], self.target_y[index], local_morph)
                + self.aux_y[index] * breakup * 0.38;
            let z = lerp(self.rest_z[index], self.target_z[index], local_morph)
                + self.aux_x[index] * breakup * 0.55;
            let rx = x.mul_add(cos, z * sin);
            let rz = (-x).mul_add(sin, z * cos);
            self.push_world(rx, y, rz, self.param("camera_z"), width, height, index);
        }
        if (0.05..0.95).contains(&morph) {
            let visible = self.points.len();
            if visible != 0 {
                for index in (0..visible).step_by(128) {
                    self.push_segment_from_points(index, (index + 512) % visible);
                }
            }
        }
    }

    fn push_world(
        &mut self,
        x: f32,
        y: f32,
        z: f32,
        camera_z: f32,
        width: usize,
        height: usize,
        source_index: usize,
    ) {
        let depth = camera_z + z;
        if depth <= 32.0 {
            return;
        }
        let scale = camera_z / depth;
        let screen_x = width as f32 * 0.5 + x * scale;
        let screen_y = height as f32 * 0.5 + y * scale;
        if screen_x < 0.0 || screen_y < 0.0 || screen_x >= width as f32 || screen_y >= height as f32
        {
            return;
        }
        self.points.push(FormPoint {
            x: screen_x,
            y: screen_y,
            style: self.style[source_index].min(7),
            neighbor: self.flags[source_index] != 0 && screen_x + 1.0 < width as f32,
        });
    }

    fn push_segment_from_points(&mut self, first: usize, second: usize) {
        if self.segments.len() >= FORM_MAX_SEGMENTS {
            return;
        }
        let Some(a) = self.points.get(first) else {
            return;
        };
        let Some(b) = self.points.get(second) else {
            return;
        };
        self.segments.push(FormSegment {
            x0: a.x as i16,
            y0: a.y as i16,
            x1: b.x as i16,
            y1: b.y as i16,
            style: a.style.max(b.style).min(7),
        });
    }
}

fn xorshift32(mut state: u32) -> u32 {
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
}

fn unit01(value: u32) -> f32 {
    ((value >> 8) as f32) * (1.0 / 16_777_215.0)
}

fn lerp(first: f32, second: f32, amount: f32) -> f32 {
    first + (second - first) * amount
}

fn smootherstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

fn smooth_envelope(seconds: f32, enter: f32, peak: f32, exit: f32) -> f32 {
    if seconds <= enter {
        0.0
    } else if seconds < peak {
        smootherstep((seconds - enter) / (peak - enter).max(f32::EPSILON))
    } else if seconds < exit {
        1.0 - smootherstep((seconds - peak) / (exit - peak).max(f32::EPSILON))
    } else {
        0.0
    }
}

fn hologram_material_style(
    part: u8,
    x: f32,
    y: f32,
    z: f32,
    material_map: f32,
    scan_coordinate: f32,
    scan_position: f32,
) -> u8 {
    if part == 4 {
        return if material_map < 5.5 { 5 } else { 6 };
    }
    let scan_delta = (scan_coordinate - scan_position).abs();
    let scan_delta = scan_delta.min(1.0 - scan_delta);
    if scan_delta < 0.012 {
        return 7;
    }
    if scan_delta < 0.024 && part == 0 {
        return 6;
    }
    match part {
        0 => {
            let on_top = y < 55.0;
            if on_top {
                let in_button_well = [(-122.0_f32, -45.0_f32), (118.0, -48.0), (112.0, 54.0)]
                    .iter()
                    .any(|&(center_x, center_z)| {
                        (x - center_x).powi(2) + (z - center_z).powi(2) < 31.0_f32.powi(2)
                    });
                if in_button_well {
                    0
                } else if [(-52.0_f32, -103.0_f32), (184.0, 31.0), (49.0, 105.0)]
                    .iter()
                    .any(|&(marker_x, marker_z)| {
                        (x - marker_x).powi(2) + (z - marker_z).powi(2) < 6.0_f32.powi(2)
                    })
                {
                    6
                } else if x.abs() > 188.0 || z.abs() > 108.0 {
                    5
                } else {
                    4
                }
            } else {
                let vertical = ((y - 48.0) / 96.0).clamp(0.0, 1.0);
                if vertical < 0.18 {
                    5
                } else if vertical < 0.52 {
                    4
                } else if vertical < 0.82 {
                    2
                } else {
                    1
                }
            }
        }
        1 => {
            if x > -4.0 {
                5
            } else {
                4
            }
        }
        2 => {
            let latitude = ((y + 169.0) / 54.0).clamp(-1.0, 1.0);
            if latitude < -0.78 {
                7
            } else if latitude < -0.18 {
                5
            } else if latitude < 0.52 {
                4
            } else {
                5
            }
        }
        3 => {
            let ring = ((y - 41.0) / 3.0).round() as i32;
            match ring {
                0 => 7,
                1 => 5,
                2 => 3,
                _ => 2,
            }
        }
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_budgets_fit_the_shared_showcase_contract() {
        for scene in FormSceneKind::ALL {
            assert!(scene.count() > 0);
            assert!(scene.count() <= FORM_MAX_PARTICLES);
        }
    }

    #[test]
    fn form_scenes_are_deterministic_nonempty_and_allocation_stable() {
        let mut first = FormSceneRenderer::new(827_141_709_451);
        let mut second = FormSceneRenderer::new(827_141_709_451);
        let allocated = first.allocated_bytes();
        for scene in FormSceneKind::ALL {
            let (first_points, first_segments) =
                first.project(scene, Duration::from_secs(15), 960, 540);
            let first_signature: Vec<_> = first_points
                .iter()
                .take(128)
                .map(|point| (point.x.to_bits(), point.y.to_bits(), point.style))
                .collect();
            let first_segment_count = first_segments.len();
            let (second_points, second_segments) =
                second.project(scene, Duration::from_secs(15), 960, 540);
            let second_signature: Vec<_> = second_points
                .iter()
                .take(128)
                .map(|point| (point.x.to_bits(), point.y.to_bits(), point.style))
                .collect();
            assert!(!first_signature.is_empty());
            assert_eq!(first_signature, second_signature);
            assert_eq!(first_segment_count, second_segments.len());
            assert!(first_segment_count <= FORM_MAX_SEGMENTS);
            assert_eq!(first.allocated_bytes(), allocated);
        }
    }

    #[test]
    fn morph_endpoints_hold_exactly() {
        assert_eq!(smootherstep(0.0), 0.0);
        assert_eq!(smootherstep(1.0), 1.0);
        assert_eq!(
            FormSceneKind::PointCloudMorphPassage.beat(4.0),
            "spacecraft-hold"
        );
        assert_eq!(
            FormSceneKind::PointCloudMorphPassage.beat(15.0),
            "manta-hold"
        );
    }

    #[test]
    fn hologram_material_map_preserves_concept_color_hierarchy() {
        assert_eq!(
            hologram_material_style(0, 0.0, 140.0, 0.0, 0.0, 0.8, 0.2),
            1
        );
        assert_eq!(
            hologram_material_style(0, 200.0, 48.0, 0.0, 0.0, 0.8, 0.2),
            5
        );
        assert_eq!(
            hologram_material_style(2, 0.0, -220.0, 0.0, 0.0, 0.8, 0.2),
            7
        );
        assert_eq!(
            hologram_material_style(2, 0.0, -169.0, 0.0, 0.0, 0.8, 0.2),
            4
        );
        assert_eq!(
            hologram_material_style(0, -122.0, 48.0, -45.0, 0.0, 0.8, 0.2),
            0
        );
        assert_eq!(hologram_material_style(4, 0.0, 35.0, 0.0, 5.0, 0.8, 0.2), 5);
        assert_eq!(hologram_material_style(4, 0.0, 35.0, 0.0, 6.0, 0.8, 0.2), 6);
        assert_eq!(
            hologram_material_style(1, 0.0, -40.0, 0.0, 0.0, 0.205, 0.2),
            7
        );
    }
}
