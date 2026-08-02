// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Particle sequences retained for the future startup animation.
//!
//! The `particle_renderer` module owns the CRT-noise-to-3D `MagiK` sequence.
//! This module preserves the matching arcade-cabinet formation without pulling
//! the archived particle showcase into the production application.

use crate::recipes::{
    CabinetModel, CabinetRecipe, RecipeEasing, RecipeRgb565, embedded_cabinet_recipe,
};
use std::time::Duration;

const ARCADE_CLOUD_POINT_COUNT: usize = 12_288;
const ARCADE_CLOUD: &[u8] = include_bytes!("../assets/cabinet/arcade-cabinet.pcloud");
const PARTICLE_CLOUD_MAGIC: &[u8; 8] = b"PCLOUD1\0";
const PARTICLE_CLOUD_HEADER_BYTES: usize = 28;
const PARTICLE_CLOUD_RECORD_BYTES: usize = 8;
const ARCADE_DEMO_NUMBER: u64 = 21;

pub use mister_magik_framebuffer_scenes::Rgb565Pixel;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArcadeCabinetFrameStats {
    pub particles: usize,
    pub visible: usize,
    pub pixel_writes: usize,
}

/// Exact extraction of the approved arcade-cabinet particle formation.
pub struct ArcadeCabinetFormation {
    width: usize,
    height: usize,
    recipe: CabinetRecipe,
    target_x: Vec<f32>,
    target_y: Vec<f32>,
    target_z: Vec<f32>,
    source_x: Vec<f32>,
    source_y: Vec<f32>,
    source_z: Vec<f32>,
    random: Vec<u32>,
    life: Vec<f32>,
    style: Vec<u8>,
    flags: Vec<u8>,
}

impl ArcadeCabinetFormation {
    pub fn from_embedded(width: usize, height: usize) -> Result<Self, String> {
        Self::new(width, height, embedded_cabinet_recipe()?)
    }

    pub fn new(width: usize, height: usize, recipe: CabinetRecipe) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("arcade cabinet formation requires a non-empty viewport".into());
        }
        let particle_count = recipe.particle_count;
        let mut renderer = Self {
            width,
            height,
            recipe,
            target_x: vec![0.0; particle_count],
            target_y: vec![0.0; particle_count],
            target_z: vec![0.0; particle_count],
            source_x: vec![0.0; particle_count],
            source_y: vec![0.0; particle_count],
            source_z: vec![0.0; particle_count],
            random: vec![0; particle_count],
            life: vec![0.0; particle_count],
            style: vec![0; particle_count],
            flags: vec![0; particle_count],
        };
        renderer.initialize()?;
        Ok(renderer)
    }

    #[must_use]
    pub const fn particle_count(&self) -> usize {
        self.recipe.particle_count
    }

    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        let float_capacity = self.target_x.capacity()
            + self.target_y.capacity()
            + self.target_z.capacity()
            + self.source_x.capacity()
            + self.source_y.capacity()
            + self.source_z.capacity()
            + self.life.capacity();
        float_capacity
            .saturating_mul(std::mem::size_of::<f32>())
            .saturating_add(
                self.random
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
            .saturating_add(self.style.capacity())
            .saturating_add(self.flags.capacity())
    }

    pub fn render(
        &self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<ArcadeCabinetFrameStats, String> {
        let expected = self.width.saturating_mul(self.height);
        if destination.len() != expected {
            return Err(format!(
                "arcade cabinet destination has {} pixels, expected {expected}",
                destination.len()
            ));
        }
        destination.fill(pixel(self.recipe.appearance.background));
        let (formation, yaw, pitch, dolly, dispersal) = arcade_camera(&self.recipe, elapsed);
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let center_x = self.width as f32 * 0.5 + self.recipe.camera.center_offset_x;
        let center_y = self.height as f32 * 0.5 + self.recipe.camera.center_offset_y;
        let mut visible = 0usize;
        let mut pixel_writes = 0usize;

        for index in 0..self.recipe.particle_count {
            let formed_x =
                self.source_x[index] + (self.target_x[index] - self.source_x[index]) * formation;
            let formed_y =
                self.source_y[index] + (self.target_y[index] - self.source_y[index]) * formation;
            let formed_z =
                self.source_z[index] + (self.target_z[index] - self.source_z[index]) * formation;
            let disperse_scale = 1.0
                + dispersal
                    * (self.recipe.dispersal.radial_base
                        + self.life[index] * self.recipe.dispersal.radial_life_gain);
            let world_x = formed_x * disperse_scale;
            let world_y = formed_y * disperse_scale
                + dispersal
                    * unit_signed(self.random[index].rotate_left(11))
                    * self.recipe.dispersal.vertical_jitter;
            let world_z = formed_z * disperse_scale;
            let rotated_x = world_x.mul_add(cos_yaw, world_z * sin_yaw);
            let yaw_z = (-world_x).mul_add(sin_yaw, world_z * cos_yaw);
            let rotated_y = world_y.mul_add(cos_pitch, -(yaw_z * sin_pitch));
            let rotated_z = world_y.mul_add(sin_pitch, yaw_z * cos_pitch);
            let depth = dolly + rotated_z;
            if depth <= self.recipe.camera.near_depth {
                continue;
            }
            let scale = self.recipe.camera.focal_length / depth;
            let x = center_x + rotated_x * scale;
            let y = center_y + rotated_y * scale;
            if x < 0.0 || y < 0.0 || x >= self.width as f32 || y >= self.height as f32 {
                continue;
            }
            let feature = self.flags[index];
            let appearance = self.recipe.appearance;
            let style = if feature & appearance.priority_feature_mask != 0 {
                appearance.priority_palette_index
            } else if feature & appearance.accent_feature_mask != 0 {
                self.style[index]
                    .saturating_add(appearance.accent_palette_add)
                    .min(7)
            } else {
                self.style[index]
            };
            let pixel_x = x as usize;
            let offset = y as usize * self.width + pixel_x;
            destination[offset] = pixel(appearance.palette[usize::from(style)]);
            pixel_writes = pixel_writes.saturating_add(1);
            if feature & appearance.neighbor_feature_mask != 0
                && index % usize::from(appearance.neighbor_every) == 0
                && pixel_x + 1 < self.width
            {
                let neighbor_style = style.saturating_sub(appearance.neighbor_palette_subtract);
                destination[offset + 1] = pixel(appearance.palette[usize::from(neighbor_style)]);
                pixel_writes = pixel_writes.saturating_add(1);
            }
            visible = visible.saturating_add(1);
        }

        Ok(ArcadeCabinetFrameStats {
            particles: self.recipe.particle_count,
            visible,
            pixel_writes,
        })
    }

    fn initialize(&mut self) -> Result<(), String> {
        let mut state = fold_seed(self.recipe.seed);
        for index in 0..self.recipe.particle_count {
            state = xorshift32(state);
            self.random[index] = state;
            self.source_x[index] =
                unit_signed(state.rotate_left(3)) * self.recipe.source_scatter.x_half_extent;
            self.source_y[index] =
                unit_signed(state.rotate_left(13)) * self.recipe.source_scatter.y_half_extent;
            self.source_z[index] =
                unit_signed(state.rotate_left(23)) * self.recipe.source_scatter.z_half_extent;
        }
        decode_particle_cloud(
            ARCADE_CLOUD,
            self.recipe.model,
            &mut self.target_x,
            &mut self.target_y,
            &mut self.target_z,
            &mut self.life,
            &self.random,
            &mut self.style,
            &mut self.flags,
        )
    }
}

const fn pixel(color: RecipeRgb565) -> Rgb565Pixel {
    Rgb565Pixel(color.0)
}

/// Decodes the canonical little-endian `PCLOUD1` representation.
///
/// The 28-byte header is the eight-byte `PCLOUD1\0` magic, `u16` version,
/// `u16` record stride, `u32` point count, then six `i16` bounds in
/// x-min/x-max/y-min/y-max/z-min/z-max order. Each eight-byte point record is
/// `i16 x`, `i16 y`, `i16 z`, `u8 material`, and `u8 feature flags`.
#[allow(clippy::too_many_arguments)]
fn decode_particle_cloud(
    bytes: &[u8],
    model: CabinetModel,
    target_x: &mut [f32],
    target_y: &mut [f32],
    target_z: &mut [f32],
    life: &mut [f32],
    random: &[u32],
    style: &mut [u8],
    flags: &mut [u8],
) -> Result<(), String> {
    if bytes.len() < PARTICLE_CLOUD_HEADER_BYTES || &bytes[..8] != PARTICLE_CLOUD_MAGIC {
        return Err("arcade particle cloud header is invalid".into());
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    let stride = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
    let count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    if version != 1 || stride != PARTICLE_CLOUD_RECORD_BYTES {
        return Err(format!(
            "arcade particle cloud contract mismatch: version={version} stride={stride} count={count}"
        ));
    }
    if count == 0 || count > ARCADE_CLOUD_POINT_COUNT {
        return Err(format!(
            "arcade particle cloud count {count} is outside 1..={ARCADE_CLOUD_POINT_COUNT}"
        ));
    }
    if count != ARCADE_CLOUD_POINT_COUNT {
        return Err(format!(
            "arcade particle cloud has {count} points, expected {ARCADE_CLOUD_POINT_COUNT}"
        ));
    }
    let expected = PARTICLE_CLOUD_HEADER_BYTES.saturating_add(count.saturating_mul(stride));
    if bytes.len() != expected {
        return Err(format!(
            "arcade particle cloud length {} does not match expected {expected}",
            bytes.len()
        ));
    }
    let output_count = target_x.len();
    if output_count > count
        || target_y.len() != output_count
        || target_z.len() != output_count
        || life.len() != output_count
        || random.len() != output_count
        || style.len() != output_count
        || flags.len() != output_count
    {
        return Err("arcade particle cloud output lengths are inconsistent".into());
    }
    let mut bounds = [0i16; 6];
    for (index, value) in bounds.iter_mut().enumerate() {
        let offset = 16 + index * 2;
        *value = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    }
    if bounds[0] > bounds[1] || bounds[2] > bounds[3] || bounds[4] > bounds[5] {
        return Err("arcade particle cloud bounds are invalid".into());
    }
    for index in 0..count {
        let offset = PARTICLE_CLOUD_HEADER_BYTES + index * stride;
        let x = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let y = i16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        let z = i16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
        let palette = bytes[offset + 6];
        let feature_flags = bytes[offset + 7];
        if x < bounds[0]
            || x > bounds[1]
            || y < bounds[2]
            || y > bounds[3]
            || z < bounds[4]
            || z > bounds[5]
            || y < 0
            || palette > 7
            || feature_flags & !3 != 0
        {
            return Err(format!("arcade particle cloud record {index} is invalid"));
        }
        if index < output_count {
            target_x[index] = f32::from(x) * (model.x_half_extent / 32_767.0);
            target_y[index] = model.y_origin - f32::from(y) * (model.y_extent / 32_767.0);
            target_z[index] = f32::from(z) * (model.z_half_extent / 32_767.0);
            style[index] = palette;
            flags[index] = feature_flags;
            life[index] = unit01(random[index].rotate_left(17));
        }
    }
    Ok(())
}

fn fold_seed(seed: u64) -> u32 {
    let folded = seed ^ (seed >> 32) ^ (ARCADE_DEMO_NUMBER * 0x9e37_79b9);
    let folded = folded as u32;
    if folded == 0 { 0xa341_316c } else { folded }
}

fn xorshift32(mut state: u32) -> u32 {
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

fn ease(value: f32, easing: RecipeEasing) -> f32 {
    match easing {
        RecipeEasing::Linear => value,
        RecipeEasing::Smoothstep => value * value * (3.0 - 2.0 * value),
        RecipeEasing::EaseOutCubic => 1.0 - (1.0 - value).powi(3),
    }
}

fn triangle_wave(value: f32) -> f32 {
    let phase = value.rem_euclid(2.0);
    if phase < 1.0 {
        phase.mul_add(2.0, -1.0)
    } else {
        3.0 - phase * 2.0
    }
}

fn arcade_camera(recipe: &CabinetRecipe, elapsed: Duration) -> (f32, f32, f32, f32, f32) {
    let timing = recipe.timing;
    let seconds = elapsed
        .as_secs_f32()
        .rem_euclid(timing.cycle_ms as f32 / 1_000.0);
    let formation_seconds = timing.formation_ms as f32 / 1_000.0;
    let orbit_seconds = timing.orbit_ms as f32 / 1_000.0;
    let return_seconds = timing.return_ms as f32 / 1_000.0;
    let disperse_seconds = timing.disperse_ms as f32 / 1_000.0;
    let formation = ease(
        (seconds / formation_seconds).clamp(0.0, 1.0),
        timing.formation_easing,
    );
    if seconds < formation_seconds {
        let pose = recipe.camera.formation;
        return (
            formation,
            pose.yaw_radians,
            pose.pitch_radians,
            pose.dolly,
            0.0,
        );
    }
    let orbit_end = formation_seconds + orbit_seconds;
    if seconds < orbit_end {
        let phase = (seconds - formation_seconds) / orbit_seconds;
        let orbit = recipe.camera.orbit;
        return (
            1.0,
            orbit.yaw_center_radians
                + (phase * orbit.yaw_turns * std::f32::consts::TAU).sin()
                    * orbit.yaw_amplitude_radians,
            triangle_wave(phase * orbit.pitch_triangle_rate) * orbit.pitch_amplitude_radians,
            orbit.dolly_center
                + triangle_wave(phase * orbit.dolly_triangle_rate + orbit.dolly_triangle_phase)
                    * orbit.dolly_amplitude,
            0.0,
        );
    }
    let return_end = orbit_end + return_seconds;
    if seconds < return_end {
        let return_t = ease(
            ((seconds - orbit_end) / return_seconds).clamp(0.0, 1.0),
            timing.return_easing,
        );
        let orbit = recipe.camera.orbit;
        let target = recipe.camera.return_pose;
        return (
            1.0,
            orbit.yaw_center_radians * (1.0 - return_t) + target.yaw_radians * return_t,
            target.pitch_radians * return_t,
            orbit.dolly_center * (1.0 - return_t) + target.dolly * return_t,
            0.0,
        );
    }
    let target = recipe.camera.return_pose;
    let dispersal = ease(
        ((seconds - return_end) / disperse_seconds).clamp(0.0, 1.0),
        timing.disperse_easing,
    );
    (
        1.0,
        target.yaw_radians,
        target.pitch_radians,
        target.dolly,
        dispersal,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_for_test(bytes: &[u8], model: CabinetModel) -> Result<(), String> {
        let count = ARCADE_CLOUD_POINT_COUNT;
        let mut target_x = vec![0.0; count];
        let mut target_y = vec![0.0; count];
        let mut target_z = vec![0.0; count];
        let mut life = vec![0.0; count];
        let random = vec![0; count];
        let mut style = vec![0; count];
        let mut flags = vec![0; count];
        decode_particle_cloud(
            bytes,
            model,
            &mut target_x,
            &mut target_y,
            &mut target_z,
            &mut life,
            &random,
            &mut style,
            &mut flags,
        )
    }

    #[test]
    fn checked_in_arcade_cloud_preserves_the_approved_particle_count() {
        let renderer = ArcadeCabinetFormation::from_embedded(960, 540).unwrap();
        assert_eq!(renderer.particle_count(), ARCADE_CLOUD_POINT_COUNT);
    }

    #[test]
    fn arcade_formation_is_deterministic() {
        let recipe = embedded_cabinet_recipe().unwrap();
        let first = ArcadeCabinetFormation::new(960, 540, recipe.clone()).unwrap();
        let second = ArcadeCabinetFormation::new(960, 540, recipe).unwrap();
        let mut first_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let mut second_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let elapsed = Duration::from_secs(12);

        let first_stats = first.render(&mut first_pixels, elapsed).unwrap();
        let second_stats = second.render(&mut second_pixels, elapsed).unwrap();

        assert_eq!(first_stats, second_stats);
        assert_eq!(first_pixels, second_pixels);
        assert!(first_stats.visible > 10_000);
    }

    #[test]
    fn cloud_rejects_trailing_data_before_rendering() {
        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes.push(0);
        let recipe = embedded_cabinet_recipe().unwrap();
        assert!(decode_for_test(&bytes, recipe.model).is_err());
    }

    #[test]
    fn cloud_rejects_unsafe_count_coordinates_material_and_flags() {
        let recipe = embedded_cabinet_recipe().unwrap();

        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes[12..16].copy_from_slice(&((ARCADE_CLOUD_POINT_COUNT as u32) + 1).to_le_bytes());
        assert!(decode_for_test(&bytes, recipe.model).is_err());

        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes[PARTICLE_CLOUD_HEADER_BYTES..PARTICLE_CLOUD_HEADER_BYTES + 2]
            .copy_from_slice(&i16::MAX.to_le_bytes());
        assert!(decode_for_test(&bytes, recipe.model).is_err());

        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes[PARTICLE_CLOUD_HEADER_BYTES + 6] = 8;
        assert!(decode_for_test(&bytes, recipe.model).is_err());

        let mut bytes = ARCADE_CLOUD.to_vec();
        bytes[PARTICLE_CLOUD_HEADER_BYTES + 7] = 4;
        assert!(decode_for_test(&bytes, recipe.model).is_err());
    }
}
