// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Particle sequences retained for the future startup animation.
//!
//! The `particle_renderer` module owns the CRT-noise-to-3D `MagiK` sequence.
//! This module preserves the matching arcade-cabinet formation without pulling
//! the archived particle showcase into the production application.

use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Duration;

const ARCADE_PARTICLE_COUNT: usize = 12_288;
const ARCADE_CLOUD: &[u8] = include_bytes!("../assets/particles/arcade-cabinet.pcloud");
const PARTICLE_CLOUD_MAGIC: &[u8; 8] = b"PCLOUD1\0";
const PARTICLE_CLOUD_HEADER_BYTES: usize = 28;
const PARTICLE_CLOUD_RECORD_BYTES: usize = 8;
const ARCADE_DEMO_NUMBER: u64 = 21;
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
    pub fn new(width: usize, height: usize, seed: u64) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("arcade cabinet formation requires a non-empty viewport".into());
        }
        let mut renderer = Self {
            width,
            height,
            target_x: vec![0.0; ARCADE_PARTICLE_COUNT],
            target_y: vec![0.0; ARCADE_PARTICLE_COUNT],
            target_z: vec![0.0; ARCADE_PARTICLE_COUNT],
            source_x: vec![0.0; ARCADE_PARTICLE_COUNT],
            source_y: vec![0.0; ARCADE_PARTICLE_COUNT],
            source_z: vec![0.0; ARCADE_PARTICLE_COUNT],
            random: vec![0; ARCADE_PARTICLE_COUNT],
            life: vec![0.0; ARCADE_PARTICLE_COUNT],
            style: vec![0; ARCADE_PARTICLE_COUNT],
            flags: vec![0; ARCADE_PARTICLE_COUNT],
        };
        renderer.initialize(seed)?;
        Ok(renderer)
    }

    #[must_use]
    pub const fn particle_count(&self) -> usize {
        ARCADE_PARTICLE_COUNT
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
        destination.fill(Rgb565Pixel(0));
        let (formation, yaw, pitch, dolly, dispersal) =
            arcade_camera(elapsed.as_secs_f32().rem_euclid(30.0));
        let (sin_yaw, cos_yaw) = yaw.sin_cos();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let center_x = self.width as f32 * 0.5;
        let center_y = self.height as f32 * 0.5 + 16.0;
        let mut visible = 0usize;
        let mut pixel_writes = 0usize;

        for index in 0..ARCADE_PARTICLE_COUNT {
            let formed_x =
                self.source_x[index] + (self.target_x[index] - self.source_x[index]) * formation;
            let formed_y =
                self.source_y[index] + (self.target_y[index] - self.source_y[index]) * formation;
            let formed_z =
                self.source_z[index] + (self.target_z[index] - self.source_z[index]) * formation;
            let disperse_scale = 1.0 + dispersal * (0.9 + self.life[index] * 1.6);
            let world_x = formed_x * disperse_scale;
            let world_y = formed_y * disperse_scale
                + dispersal * unit_signed(self.random[index].rotate_left(11)) * 90.0;
            let world_z = formed_z * disperse_scale;
            let rotated_x = world_x.mul_add(cos_yaw, world_z * sin_yaw);
            let yaw_z = (-world_x).mul_add(sin_yaw, world_z * cos_yaw);
            let rotated_y = world_y.mul_add(cos_pitch, -(yaw_z * sin_pitch));
            let rotated_z = world_y.mul_add(sin_pitch, yaw_z * cos_pitch);
            let depth = dolly + rotated_z;
            if depth <= 48.0 {
                continue;
            }
            let scale = 610.0 / depth;
            let x = center_x + rotated_x * scale;
            let y = center_y + rotated_y * scale;
            if x < 0.0 || y < 0.0 || x >= self.width as f32 || y >= self.height as f32 {
                continue;
            }
            let feature = self.flags[index];
            let style = if feature & 2 != 0 {
                7
            } else if feature & 1 != 0 {
                self.style[index].saturating_add(2).min(7)
            } else {
                self.style[index]
            };
            let pixel_x = x as usize;
            let offset = y as usize * self.width + pixel_x;
            destination[offset] = ARCADE_PALETTE[usize::from(style)];
            pixel_writes = pixel_writes.saturating_add(1);
            if feature != 0 && index & 3 == 0 && pixel_x + 1 < self.width {
                destination[offset + 1] = ARCADE_PALETTE[usize::from(style.saturating_sub(1))];
                pixel_writes = pixel_writes.saturating_add(1);
            }
            visible = visible.saturating_add(1);
        }

        Ok(ArcadeCabinetFrameStats {
            particles: ARCADE_PARTICLE_COUNT,
            visible,
            pixel_writes,
        })
    }

    fn initialize(&mut self, seed: u64) -> Result<(), String> {
        let mut state = fold_seed(seed);
        for index in 0..ARCADE_PARTICLE_COUNT {
            state = xorshift32(state);
            self.random[index] = state;
            self.source_x[index] = unit_signed(state.rotate_left(3)) * 510.0;
            self.source_y[index] = unit_signed(state.rotate_left(13)) * 300.0;
            self.source_z[index] = unit_signed(state.rotate_left(23)) * 360.0;
        }
        decode_particle_cloud(
            ARCADE_CLOUD,
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

#[allow(clippy::too_many_arguments)]
fn decode_particle_cloud(
    bytes: &[u8],
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
    if version != 1 || stride != PARTICLE_CLOUD_RECORD_BYTES || count != ARCADE_PARTICLE_COUNT {
        return Err(format!(
            "arcade particle cloud contract mismatch: version={version} stride={stride} count={count}"
        ));
    }
    let expected = PARTICLE_CLOUD_HEADER_BYTES.saturating_add(count.saturating_mul(stride));
    if bytes.len() != expected {
        return Err(format!(
            "arcade particle cloud length {} does not match expected {expected}",
            bytes.len()
        ));
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
        target_x[index] = f32::from(x) * (390.0 / 32_767.0);
        target_y[index] = 220.0 - f32::from(y) * (440.0 / 32_767.0);
        target_z[index] = f32::from(z) * (390.0 / 32_767.0);
        style[index] = palette;
        flags[index] = feature_flags;
        life[index] = unit01(random[index].rotate_left(17));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_arcade_cloud_preserves_the_approved_particle_count() {
        let renderer = ArcadeCabinetFormation::new(960, 540, 0x4d61_6769_4b).unwrap();
        assert_eq!(renderer.particle_count(), ARCADE_PARTICLE_COUNT);
    }

    #[test]
    fn arcade_formation_is_deterministic() {
        let first = ArcadeCabinetFormation::new(960, 540, 0x4d61_6769_4b).unwrap();
        let second = ArcadeCabinetFormation::new(960, 540, 0x4d61_6769_4b).unwrap();
        let mut first_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let mut second_pixels = vec![Rgb565Pixel(0); 960 * 540];
        let elapsed = Duration::from_secs(12);

        let first_stats = first.render(&mut first_pixels, elapsed).unwrap();
        let second_stats = second.render(&mut second_pixels, elapsed).unwrap();

        assert_eq!(first_stats, second_stats);
        assert_eq!(first_pixels, second_pixels);
        assert!(first_stats.visible > 10_000);
    }
}
