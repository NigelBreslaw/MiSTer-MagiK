// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standalone twenty-second startup intro scene.

use crate::intro_recipe::{IntroCue, IntroRecipe, RecipeEasing};
use crate::point_cloud::{
    INVALID_PARTICLE_OFFSET, PARTICLE_LANES, PointCloudDrawCommand, PointCloudPositionBlock,
    project_stable_neon,
};
use crate::targets::{ParticleGroupSpan, decode_particle_groups};
use mister_magik_framebuffer_scenes::{
    FramebufferScene, Rgb565Pixel, SceneBufferId, SceneClock, SceneError, SceneGeometry,
    SceneTarget,
};
use std::time::{Duration, Instant};

const MISTER_CLOUD: &[u8] = include_bytes!("../assets/intro/mister.pcloud");
const MISTER_GROUPS: &[u8] = include_bytes!("../assets/intro/mister.pgroup");
const PCLOUD_HEADER_BYTES: usize = 28;
const PCLOUD_RECORD_BYTES: usize = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IntroStageTimings {
    pub clear_us: u64,
    pub transform_us: u64,
    pub projection_us: u64,
    pub raster_us: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IntroFrameStats {
    pub particles: usize,
    pub projected_particles: usize,
    pub projection_cohorts: u8,
    pub visible: usize,
    pub pixel_writes: usize,
    pub cue_index: usize,
    pub cue_elapsed_ms: u64,
    pub cue_id: &'static str,
    pub projection_backend: &'static str,
    pub stages: IntroStageTimings,
}

struct PointTarget {
    positions: Vec<[f32; 3]>,
    palette: Vec<u8>,
    groups: Vec<ParticleGroupSpan>,
}

pub struct IntroScene {
    geometry: SceneGeometry,
    recipe: IntroRecipe,
    mister: PointTarget,
    static_xy: Vec<[f32; 2]>,
    positions: Vec<PointCloudPositionBlock>,
    commands: Vec<PointCloudDrawCommand>,
}

impl IntroScene {
    pub fn new(width: usize, height: usize, recipe: IntroRecipe) -> Result<Self, String> {
        let geometry = SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
        let mister = decode_target(MISTER_CLOUD, Some((MISTER_GROUPS, 6)), TargetScale::Text)?;
        if mister.positions.len() != recipe.steady_particle_count {
            return Err(format!(
                "MiSTer target has {} particles, expected {}",
                mister.positions.len(),
                recipe.steady_particle_count
            ));
        }
        let mut static_xy = Vec::with_capacity(recipe.initial_particle_count);
        for index in 0..recipe.initial_particle_count {
            let random = mix32((recipe.seed as u32).wrapping_add(index as u32));
            static_xy.push([
                unit01(random) * width as f32,
                unit01(random.rotate_left(13)) * height as f32,
            ]);
        }
        let positions = vec![empty_block(); recipe.steady_particle_count.div_ceil(PARTICLE_LANES)];
        let commands = vec![
            PointCloudDrawCommand(INVALID_PARTICLE_OFFSET);
            recipe.steady_particle_count
        ];
        Ok(Self {
            geometry,
            recipe,
            mister,
            static_xy,
            positions,
            commands,
        })
    }

    #[must_use]
    pub fn recipe(&self) -> &IntroRecipe {
        &self.recipe
    }

    #[must_use]
    pub fn cue_at(&self, elapsed: Duration) -> (usize, u64) {
        let elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        self.recipe.cue_at(elapsed_ms.min(self.recipe.total_ms))
    }

    fn render_crt(
        &self,
        destination: &mut [Rgb565Pixel],
        frame: u64,
    ) -> (usize, usize) {
        let palette = self.recipe.appearance.palette;
        let mut visible = 0;
        for (index, source) in self.static_xy.iter().enumerate() {
            let noise = mix32((index as u32) ^ (frame as u32).wrapping_mul(0x9e37_79b9));
            if noise & 7 == 0 {
                continue;
            }
            let x = (source[0] as usize + usize::from((noise & 3) as u8)) % self.geometry.width();
            let y = (source[1] as usize + usize::from(((noise >> 2) & 3) as u8))
                % self.geometry.height();
            destination[y * self.geometry.width() + x] =
                Rgb565Pixel(palette[((noise >> 29) & 7) as usize].0);
            visible += 1;
        }
        (visible, visible)
    }

    fn render_mister_formation(
        &self,
        destination: &mut [Rgb565Pixel],
        progress: f32,
    ) -> (usize, usize) {
        let progress = ease(progress, RecipeEasing::EaseOutCubic);
        let palette = self.recipe.appearance.palette;
        let mut visible = 0;
        for index in 0..self.recipe.initial_particle_count {
            let target_index = index % self.recipe.steady_particle_count;
            let target = project(
                self.mister.positions[target_index],
                0.0,
                0.0,
                &self.recipe,
                self.geometry,
            );
            let source = self.static_xy[index];
            let x = source[0] + (target[0] - source[0]) * progress;
            let y = source[1] + (target[1] - source[1]) * progress;
            let retire = index >= self.recipe.steady_particle_count;
            let keep_threshold = ((1.0 - progress) * 65_535.0) as u32;
            let random = mix32((index as u32).wrapping_add(self.recipe.seed as u32));
            if retire && (random & 0xffff) > keep_threshold {
                continue;
            }
            if x >= 0.0
                && y >= 0.0
                && x < self.geometry.width() as f32
                && y < self.geometry.height() as f32
            {
                let offset = y as usize * self.geometry.width() + x as usize;
                let fade = if retire {
                    ((1.0 - progress) * 7.0) as usize
                } else {
                    usize::from(self.mister.palette[target_index])
                };
                destination[offset] = Rgb565Pixel(palette[fade.min(7)].0);
                visible += 1;
            }
        }
        (visible, visible)
    }

    fn render_target(
        &mut self,
        destination: &mut [Rgb565Pixel],
    ) -> (usize, usize, &'static str) {
        let transform_started = Instant::now();
        copy_target_to_blocks(&self.mister.positions, &mut self.positions);
        let _transform_us = elapsed_us(transform_started.elapsed());
        let projection_started = Instant::now();
        self.commands
            .fill(PointCloudDrawCommand(INVALID_PARTICLE_OFFSET));
        let vector_end = project_stable_neon(
            self.recipe.steady_particle_count,
            &self.positions,
            0,
            1,
            0.0,
            1.0,
            0.0,
            1.0,
            self.recipe.camera.dolly,
            self.recipe.camera.near_depth,
            self.recipe.camera.focal_length,
            self.geometry.width() as f32 * 0.5 + self.recipe.camera.center_offset_x,
            self.geometry.height() as f32 * 0.5 + self.recipe.camera.center_offset_y,
            self.geometry.width(),
            self.geometry.height(),
            &mut self.commands,
        );
        for index in vector_end..self.recipe.steady_particle_count {
            self.commands[index] = project_command(
                self.mister.positions[index],
                &self.recipe,
                self.geometry,
            );
        }
        let backend = if vector_end > 0 {
            "point-cloud-neon"
        } else {
            for index in 0..self.recipe.steady_particle_count {
                self.commands[index] = project_command(
                    self.mister.positions[index],
                    &self.recipe,
                    self.geometry,
                );
            }
            "point-cloud-scalar"
        };
        let _projection_us = elapsed_us(projection_started.elapsed());
        let mut visible = 0;
        for (index, command) in self.commands.iter().copied().enumerate() {
            let Some(offset) = command.offset() else {
                continue;
            };
            destination[offset] = Rgb565Pixel(
                self.recipe.appearance.palette[usize::from(self.mister.palette[index])].0,
            );
            visible += 1;
        }
        (visible, visible, backend)
    }
}

impl FramebufferScene for IntroScene {
    type Stats = IntroFrameStats;

    fn geometry(&self) -> SceneGeometry {
        self.geometry
    }

    fn render(
        &mut self,
        mut target: SceneTarget<'_>,
        clock: SceneClock,
    ) -> Result<Self::Stats, SceneError> {
        if target.geometry() != self.geometry {
            return Err(SceneError::Render("intro frame geometry changed".into()));
        }
        let elapsed_ms = clock
            .elapsed
            .as_millis()
            .min(u128::from(self.recipe.total_ms)) as u64;
        let (cue_index, cue_elapsed_ms) = self.recipe.cue_at(elapsed_ms);
        let cue = &self.recipe.cues[cue_index];
        let clear_started = Instant::now();
        target
            .pixels_mut()
            .fill(Rgb565Pixel(self.recipe.appearance.background.0));
        let clear_us = elapsed_us(clear_started.elapsed());
        let render_started = Instant::now();
        let (visible, pixel_writes, projection_backend) = match cue {
            IntroCue::CrtStatic { .. } => {
                let (visible, writes) = self.render_crt(target.pixels_mut(), clock.frame);
                (visible, writes, "crt-packed")
            }
            IntroCue::MorphTarget { duration_ms, .. } if cue_index == 1 => {
                let progress = cue_elapsed_ms as f32 / *duration_ms as f32;
                let (visible, writes) =
                    self.render_mister_formation(target.pixels_mut(), progress);
                (visible, writes, "crt-to-point-cloud")
            }
            _ => self.render_target(target.pixels_mut()),
        };
        let raster_us = elapsed_us(render_started.elapsed());
        let particles = if cue_index < 2 {
            self.recipe.initial_particle_count
        } else {
            self.recipe.steady_particle_count
        };
        Ok(IntroFrameStats {
            particles,
            projected_particles: particles,
            projection_cohorts: 1,
            visible,
            pixel_writes,
            cue_index,
            cue_elapsed_ms,
            cue_id: cue_label(cue_index),
            projection_backend,
            stages: IntroStageTimings {
                clear_us,
                transform_us: 0,
                projection_us: 0,
                raster_us,
            },
        })
    }

    fn invalidate_buffer(&mut self, _buffer: SceneBufferId) {}
}

#[derive(Clone, Copy)]
enum TargetScale {
    Text,
}

fn decode_target(
    bytes: &[u8],
    group_bytes: Option<(&[u8], u8)>,
    scale: TargetScale,
) -> Result<PointTarget, String> {
    if bytes.len() < PCLOUD_HEADER_BYTES || &bytes[..8] != b"PCLOUD1\0" {
        return Err("intro point-cloud header is invalid".into());
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    let stride = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
    let count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    if version != 1 || stride != PCLOUD_RECORD_BYTES {
        return Err("intro point-cloud contract mismatch".into());
    }
    if bytes.len() != PCLOUD_HEADER_BYTES + count * stride {
        return Err("intro point-cloud length mismatch".into());
    }
    let mut positions = Vec::with_capacity(count);
    let mut palette = Vec::with_capacity(count);
    for index in 0..count {
        let offset = PCLOUD_HEADER_BYTES + index * stride;
        let x = i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let y = i16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        let z = i16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
        let style = bytes[offset + 6];
        if style > 7 || bytes[offset + 7] != 0 {
            return Err(format!("intro point-cloud record {index} is invalid"));
        }
        let position = match scale {
            TargetScale::Text => [
                f32::from(x) * (480.0 / 32_767.0),
                f32::from(y) * (220.0 / 32_767.0) - 110.0,
                f32::from(z) * (96.0 / 32_767.0),
            ],
        };
        positions.push(position);
        palette.push(style);
    }
    let groups = if let Some((bytes, count)) = group_bytes {
        decode_particle_groups(bytes, positions.len(), count)?.spans().to_vec()
    } else {
        vec![ParticleGroupSpan {
            id: 0,
            start: 0,
            count: positions.len(),
        }]
    };
    Ok(PointTarget {
        positions,
        palette,
        groups,
    })
}

fn empty_block() -> PointCloudPositionBlock {
    PointCloudPositionBlock {
        target_x: [0.0; PARTICLE_LANES],
        target_y: [0.0; PARTICLE_LANES],
        target_z: [0.0; PARTICLE_LANES],
        source_x: [0.0; PARTICLE_LANES],
        source_y: [0.0; PARTICLE_LANES],
        source_z: [0.0; PARTICLE_LANES],
    }
}

fn copy_target_to_blocks(source: &[[f32; 3]], blocks: &mut [PointCloudPositionBlock]) {
    for (index, point) in source.iter().enumerate() {
        let block = &mut blocks[index / PARTICLE_LANES];
        let lane = index % PARTICLE_LANES;
        block.target_x[lane] = point[0];
        block.target_y[lane] = point[1];
        block.target_z[lane] = point[2];
    }
}

fn project_command(
    position: [f32; 3],
    recipe: &IntroRecipe,
    geometry: SceneGeometry,
) -> PointCloudDrawCommand {
    let depth = recipe.camera.dolly + position[2];
    if depth <= recipe.camera.near_depth {
        return PointCloudDrawCommand(INVALID_PARTICLE_OFFSET);
    }
    let scale = recipe.camera.focal_length / depth;
    let x = geometry.width() as f32 * 0.5 + recipe.camera.center_offset_x + position[0] * scale;
    let y = geometry.height() as f32 * 0.5 + recipe.camera.center_offset_y + position[1] * scale;
    if x < 0.0 || y < 0.0 || x >= geometry.width() as f32 || y >= geometry.height() as f32 {
        return PointCloudDrawCommand(INVALID_PARTICLE_OFFSET);
    }
    PointCloudDrawCommand::visible(
        y as usize * geometry.width() + x as usize,
        depth,
        x as usize,
    )
}

fn project(
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
    recipe: &IntroRecipe,
    geometry: SceneGeometry,
) -> [f32; 2] {
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let (sin_pitch, cos_pitch) = pitch.sin_cos();
    let rotated_x = position[0].mul_add(cos_yaw, position[2] * sin_yaw);
    let yaw_z = (-position[0]).mul_add(sin_yaw, position[2] * cos_yaw);
    let rotated_y = position[1].mul_add(cos_pitch, -(yaw_z * sin_pitch));
    let rotated_z = position[1].mul_add(sin_pitch, yaw_z * cos_pitch);
    let depth = recipe.camera.dolly + rotated_z;
    let scale = recipe.camera.focal_length / depth.max(recipe.camera.near_depth);
    [
        geometry.width() as f32 * 0.5 + recipe.camera.center_offset_x + rotated_x * scale,
        geometry.height() as f32 * 0.5 + recipe.camera.center_offset_y + rotated_y * scale,
    ]
}

fn mix32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn unit01(value: u32) -> f32 {
    (value >> 8) as f32 * (1.0 / 16_777_215.0)
}

fn ease(value: f32, easing: RecipeEasing) -> f32 {
    let value = value.clamp(0.0, 1.0);
    match easing {
        RecipeEasing::Linear => value,
        RecipeEasing::Smoothstep => value * value * (3.0 - 2.0 * value),
        RecipeEasing::EaseOutCubic => 1.0 - (1.0 - value).powi(3),
    }
}

fn elapsed_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

const fn cue_label(index: usize) -> &'static str {
    match index {
        0 => "crt",
        1 => "form-mister",
        2 => "hold-mister",
        3 => "letters",
        4 => "hold-magik",
        5 => "spin-out",
        6 => "form-cabinet",
        7 => "cabinet-orbit",
        8 => "form-launcher",
        9 => "crossfade",
        _ => "hold-launcher",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intro_recipe::embedded_intro_recipe;

    #[test]
    fn formation_retires_the_excess_population_at_four_seconds() {
        let recipe = embedded_intro_recipe().unwrap();
        let scene = IntroScene::new(960, 540, recipe).unwrap();
        assert_eq!(scene.cue_at(Duration::from_millis(3_999)), (1, 1_499));
        assert_eq!(scene.cue_at(Duration::from_millis(4_000)), (2, 0));
        assert_eq!(scene.static_xy.len(), 102_400);
        assert_eq!(scene.mister.positions.len(), 40_960);
    }

    #[test]
    fn mister_target_has_six_aligned_tracks() {
        let scene = IntroScene::new(960, 540, embedded_intro_recipe().unwrap()).unwrap();
        assert_eq!(scene.mister.groups.len(), 6);
        assert!(scene.mister.groups.iter().all(|span| span.start % 4 == 0));
        assert!(scene.mister.groups.iter().all(|span| span.count % 4 == 0));
    }
}
