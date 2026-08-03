// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standalone twenty-second startup intro scene.

use crate::intro_recipe::{IntroCue, IntroRecipe};
use crate::point_cloud::{
    INVALID_PARTICLE_OFFSET, PARTICLE_LANES, PointCloudDrawCommand, PointCloudPositionBlock,
    project_stable_neon,
};
use crate::recipes::RecipeEasing;
use crate::targets::{ParticleGroupSpan, decode_particle_groups};
use mister_magik_framebuffer_scenes::{
    FramebufferScene, Rgb565Pixel, SceneBufferId, SceneClock, SceneError, SceneGeometry,
    SceneTarget,
};
use std::time::{Duration, Instant};

const MISTER_CLOUD: &[u8] = include_bytes!("../assets/intro/mister.pcloud");
const MISTER_GROUPS: &[u8] = include_bytes!("../assets/intro/mister.pgroup");
const MAGIK_CLOUD: &[u8] = include_bytes!("../assets/intro/magik.pcloud");
const MAGIK_GROUPS: &[u8] = include_bytes!("../assets/intro/magik.pgroup");
const CABINET_CLOUD: &[u8] = include_bytes!("../assets/cabinet/arcade-cabinet.pcloud");
const LAUNCHER_CLOUD: &[u8] = include_bytes!("../assets/intro/launcher-mock.pcloud");
const LAUNCHER_GROUPS: &[u8] = include_bytes!("../assets/intro/launcher-mock.pgroup");
const LAUNCHER_SNAPSHOT: &[u8] = include_bytes!("../assets/intro/launcher-mock.rgb565");
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
    pub cue_start_ms: u64,
    pub previous_cue_start_ms: u64,
    pub cue_elapsed_ms: u64,
    pub cue_duration_ms: u64,
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
    mister_screen: Vec<[f32; 2]>,
    mister_pivots: [[f32; 3]; 6],
    magik: PointTarget,
    magik_pivots: [[f32; 3]; 6],
    scatter_vectors: Vec<[f32; 3]>,
    cloud: PointTarget,
    cabinet: PointTarget,
    launcher: PointTarget,
    launcher_snapshot: Vec<Rgb565Pixel>,
    static_xy: Vec<[f32; 2]>,
    dynamic_positions: Vec<[f32; 3]>,
    positions: Vec<PointCloudPositionBlock>,
    commands: Vec<PointCloudDrawCommand>,
}

impl IntroScene {
    pub fn new(width: usize, height: usize, recipe: IntroRecipe) -> Result<Self, String> {
        let geometry = SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
        let mister = decode_target(MISTER_CLOUD, Some((MISTER_GROUPS, 6)), TargetScale::Text)?;
        let magik = decode_target(MAGIK_CLOUD, Some((MAGIK_GROUPS, 6)), TargetScale::Text)?;
        if mister.positions.len() != recipe.steady_particle_count {
            return Err(format!(
                "MiSTer target has {} particles, expected {}",
                mister.positions.len(),
                recipe.steady_particle_count
            ));
        }
        if magik.positions.len() != recipe.steady_particle_count
            || magik.groups != mister.groups
        {
            return Err("MagiK target does not match the six-track MiSTer contract".into());
        }
        let mister_pivots = std::array::from_fn(|group| {
            let span = mister.groups[group];
            pivot(&mister.positions[span.start..span.start + span.count])
        });
        let magik_pivots = std::array::from_fn(|group| {
            let span = magik.groups[group];
            pivot(&magik.positions[span.start..span.start + span.count])
        });
        let mister_screen = mister
            .positions
            .iter()
            .map(|position| project(*position, 0.0, 0.0, &recipe, geometry))
            .collect();
        let scatter_vectors = (0..recipe.steady_particle_count)
            .map(|index| {
                let random = mix32(index as u32 ^ recipe.seed as u32);
                [
                    signed_unit(random),
                    signed_unit(random.rotate_left(11)),
                    signed_unit(random.rotate_left(21)),
                ]
            })
            .collect();
        let cloud_radius = match recipe.cues.get(5) {
            Some(IntroCue::Cloud { radius, .. }) => *radius,
            _ => return Err("intro cloud cue is missing".into()),
        };
        let cloud = cloud_target(recipe.steady_particle_count, recipe.seed, cloud_radius);
        let mut cabinet = decode_target(CABINET_CLOUD, None, TargetScale::Cabinet)?;
        cabinet.positions.truncate(recipe.steady_particle_count);
        cabinet.palette.truncate(recipe.steady_particle_count);
        cabinet.groups = vec![ParticleGroupSpan {
            id: 0,
            start: 0,
            count: recipe.steady_particle_count,
        }];
        let launcher = decode_target(
            LAUNCHER_CLOUD,
            Some((LAUNCHER_GROUPS, 1)),
            TargetScale::Launcher,
        )?;
        let launcher_snapshot = decode_launcher_snapshot(LAUNCHER_SNAPSHOT, width, height)?;
        let mut static_xy = Vec::with_capacity(recipe.initial_particle_count);
        for index in 0..recipe.initial_particle_count {
            let random = mix32((recipe.seed as u32).wrapping_add(index as u32));
            static_xy.push([
                unit01(random) * width as f32,
                unit01(random.rotate_left(13)) * height as f32,
            ]);
        }
        let positions = vec![empty_block(); recipe.steady_particle_count.div_ceil(PARTICLE_LANES)];
        let dynamic_positions = vec![[0.0; 3]; recipe.steady_particle_count];
        let commands = vec![
            PointCloudDrawCommand(INVALID_PARTICLE_OFFSET);
            recipe.steady_particle_count
        ];
        Ok(Self {
            geometry,
            recipe,
            mister,
            mister_screen,
            mister_pivots,
            magik,
            magik_pivots,
            scatter_vectors,
            cloud,
            cabinet,
            launcher,
            launcher_snapshot,
            static_xy,
            dynamic_positions,
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
        let palette = self.recipe.appearance.crt_palette;
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
                Rgb565Pixel(palette[((noise >> 30) & 3) as usize].0);
            visible += 1;
        }
        (visible, visible)
    }

    fn render_mister_formation(
        &self,
        destination: &mut [Rgb565Pixel],
        progress: f32,
        frame: u64,
    ) -> (usize, usize) {
        let progress = ease(progress, RecipeEasing::EaseOutCubic);
        let crt_palette = self.recipe.appearance.crt_palette;
        let text_palette = self.recipe.appearance.text_palette;
        let mut visible = 0;
        for index in 0..self.recipe.initial_particle_count {
            let target_index = index % self.recipe.steady_particle_count;
            let target = self.mister_screen[target_index];
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
                let color = if retire {
                    let fade = ((1.0 - progress) * 3.0) as usize;
                    crt_palette[fade.min(3)]
                } else {
                    let flicker = text_flicker_index(
                        self.mister.palette[target_index],
                        target_index,
                        frame,
                    );
                    text_palette[flicker]
                };
                destination[offset] = Rgb565Pixel(color.0);
                visible += 1;
            }
        }
        (visible, visible)
    }

    fn render_point_target(
        &mut self,
        destination: &mut [Rgb565Pixel],
        target: ScenePointTarget,
        frame: u64,
    ) -> (usize, usize, &'static str) {
        let text_palette_mix =
            matches!(target, ScenePointTarget::Mister | ScenePointTarget::Magik).then_some(0.0);
        let target = match target {
            ScenePointTarget::Mister => &self.mister,
            ScenePointTarget::Magik => &self.magik,
            ScenePointTarget::Cabinet => &self.cabinet,
            ScenePointTarget::Launcher => &self.launcher,
        };
        render_point_cloud(
            destination,
            &self.recipe,
            self.geometry,
            &target.positions,
            &target.palette,
            &mut self.positions,
            &mut self.commands,
            text_palette_mix,
            frame,
        )
    }

    fn render_letter_morph(
        &mut self,
        destination: &mut [Rgb565Pixel],
        cue_elapsed_ms: u64,
        duration_ms: u64,
        turns: f32,
        stagger_ms: u64,
        easing: RecipeEasing,
        frame: u64,
    ) -> (usize, usize, &'static str) {
        let local_duration = duration_ms.saturating_sub(stagger_ms.saturating_mul(5)).max(1);
        for (group_index, span) in self.mister.groups.iter().enumerate() {
            let start_ms = stagger_ms.saturating_mul(group_index as u64);
            let progress = cue_elapsed_ms.saturating_sub(start_ms) as f32 / local_duration as f32;
            let progress = ease(progress, easing);
            let source_pivot = self.mister_pivots[group_index];
            let destination_pivot = self.magik_pivots[group_index];
            let angle = progress * turns * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            for index in span.start..span.start + span.count {
                let source = self.mister.positions[index];
                let destination_point = self.magik.positions[index];
                let source_local = [
                    source[0] - source_pivot[0],
                    source[1] - source_pivot[1],
                    source[2] - source_pivot[2],
                ];
                let destination_local = [
                    destination_point[0] - destination_pivot[0],
                    destination_point[1] - destination_pivot[1],
                    destination_point[2] - destination_pivot[2],
                ];
                let local = [
                    source_local[0] + (destination_local[0] - source_local[0]) * progress,
                    source_local[1] + (destination_local[1] - source_local[1]) * progress,
                    source_local[2] + (destination_local[2] - source_local[2]) * progress,
                ];
                let center = [
                    source_pivot[0] + (destination_pivot[0] - source_pivot[0]) * progress,
                    source_pivot[1] + (destination_pivot[1] - source_pivot[1]) * progress,
                    source_pivot[2] + (destination_pivot[2] - source_pivot[2]) * progress,
                ];
                let scatter_vector = self.scatter_vectors[index];
                let scatter = if group_index == 0 {
                    0.0
                } else {
                    (progress * std::f32::consts::PI).sin() * 58.0
                };
                self.dynamic_positions[index] = [
                    center[0] + local[0].mul_add(cos, local[2] * sin)
                        + scatter_vector[0] * scatter,
                    center[1] + local[1] + scatter_vector[1] * scatter,
                    center[2] + (-local[0]).mul_add(sin, local[2] * cos)
                        + scatter_vector[2] * scatter,
                ];
            }
        }
        render_point_cloud(
            destination,
            &self.recipe,
            self.geometry,
            &self.dynamic_positions,
            &self.magik.palette,
            &mut self.positions,
            &mut self.commands,
            Some(0.0),
            frame,
        )
    }

    fn render_cloud_transition(
        &mut self,
        destination: &mut [Rgb565Pixel],
        cue_index: usize,
        cue_elapsed_ms: u64,
        duration_ms: u64,
        turns: f32,
        easing: RecipeEasing,
    ) -> (usize, usize, &'static str) {
        let progress = ease(cue_elapsed_ms as f32 / duration_ms as f32, easing);
        let (source, target, palette) = if cue_index == 5 {
            (&self.magik, &self.cloud, &self.magik.palette)
        } else {
            (&self.cloud, &self.cabinet, &self.cabinet.palette)
        };
        let angle = progress * turns * std::f32::consts::TAU;
        let (sin, cos) = angle.sin_cos();
        for index in 0..self.recipe.steady_particle_count {
            let from = source.positions[index];
            let rotated = [
                from[0].mul_add(cos, from[2] * sin),
                from[1],
                (-from[0]).mul_add(sin, from[2] * cos),
            ];
            let to = target.positions[index];
            self.dynamic_positions[index] = [
                rotated[0] + (to[0] - rotated[0]) * progress,
                rotated[1] + (to[1] - rotated[1]) * progress,
                rotated[2] + (to[2] - rotated[2]) * progress,
            ];
        }
        render_point_cloud(
            destination,
            &self.recipe,
            self.geometry,
            &self.dynamic_positions,
            palette,
            &mut self.positions,
            &mut self.commands,
            (cue_index == 5).then_some(progress),
            0,
        )
    }

    fn render_cabinet_orbit(
        &mut self,
        destination: &mut [Rgb565Pixel],
        cue_elapsed_ms: u64,
        duration_ms: u64,
        turns: f32,
    ) -> (usize, usize, &'static str) {
        let yaw = cabinet_yaw(cue_elapsed_ms, duration_ms, turns);
        let (sin, cos) = yaw.sin_cos();
        for (dynamic, point) in self
            .dynamic_positions
            .iter_mut()
            .zip(&self.cabinet.positions)
        {
            *dynamic = [
                point[0].mul_add(cos, point[2] * sin),
                point[1],
                (-point[0]).mul_add(sin, point[2] * cos),
            ];
        }
        render_point_cloud(
            destination,
            &self.recipe,
            self.geometry,
            &self.dynamic_positions,
            &self.cabinet.palette,
            &mut self.positions,
            &mut self.commands,
            None,
            0,
        )
    }

    fn render_launcher_morph(
        &mut self,
        destination: &mut [Rgb565Pixel],
        cue_elapsed_ms: u64,
        duration_ms: u64,
        easing: RecipeEasing,
    ) -> (usize, usize, &'static str) {
        let progress = ease(cue_elapsed_ms as f32 / duration_ms as f32, easing);
        for index in 0..self.recipe.steady_particle_count {
            let from = self.cabinet.positions[index];
            let to = self.launcher.positions[index];
            self.dynamic_positions[index] = [
                from[0] + (to[0] - from[0]) * progress,
                from[1] + (to[1] - from[1]) * progress,
                from[2] + (to[2] - from[2]) * progress,
            ];
        }
        render_point_cloud(
            destination,
            &self.recipe,
            self.geometry,
            &self.dynamic_positions,
            &self.launcher.palette,
            &mut self.positions,
            &mut self.commands,
            None,
            0,
        )
    }

    fn render_mock_crossfade(
        &mut self,
        destination: &mut [Rgb565Pixel],
        cue_elapsed_ms: u64,
        duration_ms: u64,
        easing: RecipeEasing,
        frame: u64,
    ) -> (usize, usize, &'static str) {
        let (visible, mut writes, backend) =
            self.render_point_target(destination, ScenePointTarget::Launcher, frame);
        let progress = ease(cue_elapsed_ms as f32 / duration_ms as f32, easing);
        let threshold = (progress * 64.0).round() as u8;
        for (offset, (destination, source)) in destination
            .iter_mut()
            .zip(&self.launcher_snapshot)
            .enumerate()
        {
            let x = offset % self.geometry.width();
            let y = offset / self.geometry.width();
            if bayer8(x, y) < threshold {
                *destination = *source;
                writes += 1;
            }
        }
        (visible, writes, backend)
    }
}

#[allow(clippy::too_many_arguments)]
fn render_point_cloud(
    destination: &mut [Rgb565Pixel],
    recipe: &IntroRecipe,
    geometry: SceneGeometry,
    target_positions: &[[f32; 3]],
    target_palette: &[u8],
    positions: &mut [PointCloudPositionBlock],
    commands: &mut [PointCloudDrawCommand],
    text_palette_mix: Option<f32>,
    frame: u64,
) -> (usize, usize, &'static str) {
        let transform_started = Instant::now();
        copy_target_to_blocks(target_positions, positions);
        let _transform_us = elapsed_us(transform_started.elapsed());
        let projection_started = Instant::now();
        commands.fill(PointCloudDrawCommand(INVALID_PARTICLE_OFFSET));
        let vector_end = project_stable_neon(
            target_positions.len(),
            positions,
            0,
            1,
            0.0,
            1.0,
            0.0,
            1.0,
            recipe.camera.dolly,
            recipe.camera.near_depth,
            recipe.camera.focal_length,
            geometry.width() as f32 * 0.5 + recipe.camera.center_offset_x,
            geometry.height() as f32 * 0.5 + recipe.camera.center_offset_y,
            geometry.width(),
            geometry.height(),
            commands,
        );
        for index in vector_end..target_positions.len() {
            commands[index] = project_command(target_positions[index], recipe, geometry);
        }
        let backend = if vector_end > 0 {
            "point-cloud-neon"
        } else {
            for index in 0..target_positions.len() {
                commands[index] = project_command(target_positions[index], recipe, geometry);
            }
            "point-cloud-scalar"
        };
        let _projection_us = elapsed_us(projection_started.elapsed());
        let mut visible = 0;
        let mut writes = 0;
        for (index, command) in commands.iter().copied().enumerate() {
            let Some(offset) = command.offset() else {
                continue;
            };
            let target_color = match text_palette_mix {
                None => true,
                Some(mix) if mix <= 0.0 => false,
                Some(mix) if mix >= 1.0 => true,
                Some(mix) => {
                    let threshold = (mix * 65_536.0) as u32;
                    mix32(index as u32 ^ recipe.seed as u32 ^ 0xa5a5_5a5a) & 0xffff
                        < threshold
                }
            };
            let color = if !target_color {
                recipe.appearance.text_palette[text_flicker_index(
                    target_palette[index],
                    index,
                    frame,
                )]
            } else {
                recipe.appearance.palette[usize::from(target_palette[index])]
            };
            destination[offset] = Rgb565Pixel(color.0);
            visible += 1;
            writes += 1;
            if !target_color
                && target_positions[index][2] < 0.0
                && offset % geometry.width() + 1 < geometry.width()
            {
                destination[offset + 1] =
                    Rgb565Pixel(recipe.appearance.text_palette[2].0);
                writes += 1;
            }
        }
        (visible, writes, backend)
}

#[inline(always)]
fn text_flicker_index(style: u8, index: usize, frame: u64) -> usize {
    (usize::from(style) ^ ((index.wrapping_mul(13) + (frame as usize >> 1)) >> 3)) & 3
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
        let cue = self.recipe.cues[cue_index].clone();
        let cue_start_ms = self.recipe.cues[..cue_index]
            .iter()
            .map(IntroCue::duration_ms)
            .sum();
        let previous_cue_start_ms = self.recipe.cues[..cue_index.saturating_sub(1)]
            .iter()
            .map(IntroCue::duration_ms)
            .sum();
        let clear_started = Instant::now();
        target
            .pixels_mut()
            .fill(Rgb565Pixel(self.recipe.appearance.background.0));
        let clear_us = elapsed_us(clear_started.elapsed());
        let render_started = Instant::now();
        let (visible, pixel_writes, projection_backend) = match &cue {
            IntroCue::CrtStatic { .. } => {
                let (visible, writes) = self.render_crt(target.pixels_mut(), clock.frame);
                (visible, writes, "crt-packed")
            }
            IntroCue::MorphTarget { duration_ms, .. } if cue_index == 1 => {
                let progress = cue_elapsed_ms as f32 / *duration_ms as f32;
                let (visible, writes) =
                    self.render_mister_formation(target.pixels_mut(), progress, clock.frame);
                (visible, writes, "crt-to-point-cloud")
            }
            IntroCue::LetterMorph {
                duration_ms,
                turns,
                stagger_ms,
                easing,
                ..
            } => self.render_letter_morph(
                target.pixels_mut(),
                cue_elapsed_ms,
                *duration_ms,
                *turns,
                *stagger_ms,
                *easing,
                clock.frame,
            ),
            IntroCue::Cloud {
                duration_ms,
                turns,
                easing,
                ..
            } => self.render_cloud_transition(
                target.pixels_mut(),
                cue_index,
                cue_elapsed_ms,
                *duration_ms,
                *turns,
                *easing,
            ),
            IntroCue::TargetOrbit {
                duration_ms, turns, ..
            } => self.render_cabinet_orbit(
                target.pixels_mut(),
                cue_elapsed_ms,
                *duration_ms,
                *turns,
            ),
            IntroCue::MorphTarget {
                duration_ms, easing, ..
            } if cue_index == 8 => self.render_launcher_morph(
                target.pixels_mut(),
                cue_elapsed_ms,
                *duration_ms,
                *easing,
            ),
            IntroCue::MockCrossfade {
                duration_ms, easing, ..
            } => self.render_mock_crossfade(
                target.pixels_mut(),
                cue_elapsed_ms,
                *duration_ms,
                *easing,
                clock.frame,
            ),
            IntroCue::HoldTarget { .. } if cue_index == 10 => {
                target.pixels_mut().copy_from_slice(&self.launcher_snapshot);
                (
                    0,
                    self.launcher_snapshot.len(),
                    "launcher-mock-rgb565",
                )
            }
            _ => {
                let point_target = match cue_index {
                    0..=3 => ScenePointTarget::Mister,
                    4..=7 => ScenePointTarget::Magik,
                    8 => ScenePointTarget::Cabinet,
                    _ => ScenePointTarget::Launcher,
                };
                self.render_point_target(target.pixels_mut(), point_target, clock.frame)
            }
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
            cue_start_ms,
            previous_cue_start_ms,
            cue_elapsed_ms,
            cue_duration_ms: cue.duration_ms(),
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
    Cabinet,
    Launcher,
}

#[derive(Clone, Copy)]
enum ScenePointTarget {
    Mister,
    Magik,
    Cabinet,
    Launcher,
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
        let flags = bytes[offset + 7];
        let flags_valid = match scale {
            TargetScale::Cabinet => flags & !3 == 0,
            TargetScale::Text | TargetScale::Launcher => flags == 0,
        };
        if style > 7 || !flags_valid {
            return Err(format!("intro point-cloud record {index} is invalid"));
        }
        let position = match scale {
            TargetScale::Text => [
                f32::from(x) * (480.0 / 32_767.0),
                f32::from(y) * (220.0 / 32_767.0) - 110.0,
                f32::from(z) * (96.0 / 32_767.0),
            ],
            TargetScale::Cabinet => [
                f32::from(x) * (390.0 / 32_767.0),
                220.0 - f32::from(y) * (440.0 / 32_767.0),
                f32::from(z) * (390.0 / 32_767.0),
            ],
            TargetScale::Launcher => [
                f32::from(x) * (480.0 / 32_767.0),
                f32::from(y) * (540.0 / 32_767.0) - 270.0,
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

fn signed_unit(value: u32) -> f32 {
    unit01(value) * 2.0 - 1.0
}

fn pivot(points: &[[f32; 3]]) -> [f32; 3] {
    let mut sum = [0.0; 3];
    for point in points {
        sum[0] += point[0];
        sum[1] += point[1];
        sum[2] += point[2];
    }
    let reciprocal = 1.0 / points.len() as f32;
    [sum[0] * reciprocal, sum[1] * reciprocal, sum[2] * reciprocal]
}

fn cloud_target(count: usize, seed: u64, radius: f32) -> PointTarget {
    let mut positions = Vec::with_capacity(count);
    let mut palette = Vec::with_capacity(count);
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    for index in 0..count {
        let unit = (index as f32 + 0.5) / count as f32;
        let y = 1.0 - unit * 2.0;
        let radial = (1.0 - y * y).sqrt();
        let angle = index as f32 * golden_angle;
        let jitter = 0.82 + unit01(mix32(index as u32 ^ seed as u32)) * 0.18;
        positions.push([
            angle.cos() * radial * radius * jitter,
            y * radius * 0.62,
            angle.sin() * radial * radius * jitter,
        ]);
        palette.push((index & 7) as u8);
    }
    PointTarget {
        positions,
        palette,
        groups: vec![ParticleGroupSpan {
            id: 0,
            start: 0,
            count,
        }],
    }
}

fn cabinet_yaw(elapsed_ms: u64, duration_ms: u64, turns: f32) -> f32 {
    (elapsed_ms as f32 / duration_ms.max(1) as f32).clamp(0.0, 1.0)
        * turns
        * std::f32::consts::TAU
}

fn decode_launcher_snapshot(
    bytes: &[u8],
    expected_width: usize,
    expected_height: usize,
) -> Result<Vec<Rgb565Pixel>, String> {
    if bytes.len() < 16 || &bytes[..8] != b"RGB565M1" {
        return Err("launcher mock snapshot header is invalid".into());
    }
    let width = usize::from(u16::from_le_bytes([bytes[8], bytes[9]]));
    let height = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
    let count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    if width != expected_width
        || height != expected_height
        || count != width.saturating_mul(height)
        || bytes.len() != 16 + count * 2
    {
        return Err("launcher mock snapshot geometry is invalid".into());
    }
    Ok(bytes[16..]
        .chunks_exact(2)
        .map(|pixel| Rgb565Pixel(u16::from_le_bytes([pixel[0], pixel[1]])))
        .collect())
}

fn bayer8(x: usize, y: usize) -> u8 {
    const MATRIX: [[u8; 8]; 8] = [
        [0, 32, 8, 40, 2, 34, 10, 42],
        [48, 16, 56, 24, 50, 18, 58, 26],
        [12, 44, 4, 36, 14, 46, 6, 38],
        [60, 28, 52, 20, 62, 30, 54, 22],
        [3, 35, 11, 43, 1, 33, 9, 41],
        [51, 19, 59, 27, 49, 17, 57, 25],
        [15, 47, 7, 39, 13, 45, 5, 37],
        [63, 31, 55, 23, 61, 29, 53, 21],
    ];
    MATRIX[y & 7][x & 7]
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
        assert_eq!(scene.mister.groups, scene.magik.groups);
    }

    #[test]
    fn common_m_remains_cohesive_at_the_middle_of_the_letter_morph() {
        let mut scene = IntroScene::new(960, 540, embedded_intro_recipe().unwrap()).unwrap();
        let mut pixels = vec![Rgb565Pixel(0); 960 * 540];
        scene.render_letter_morph(
            &mut pixels,
            1_750,
            3_500,
            1.0,
            150,
            RecipeEasing::Smoothstep,
            105,
        );
        let m = scene.mister.groups[0];
        let source_pivot = pivot(&scene.mister.positions[m.start..m.start + m.count]);
        let dynamic_pivot = pivot(&scene.dynamic_positions[m.start..m.start + m.count]);
        for axis in 0..3 {
            assert!((source_pivot[axis] - dynamic_pivot[axis]).abs() < 0.01);
        }
    }

    #[test]
    fn each_letter_finishes_its_spin_on_the_exact_destination() {
        let mut scene = IntroScene::new(960, 540, embedded_intro_recipe().unwrap()).unwrap();
        let mut pixels = vec![Rgb565Pixel(0); 960 * 540];
        scene.render_letter_morph(
            &mut pixels,
            3_500,
            3_500,
            1.0,
            150,
            RecipeEasing::Smoothstep,
            210,
        );
        for (actual, expected) in scene.dynamic_positions.iter().zip(&scene.magik.positions) {
            for axis in 0..3 {
                assert!((actual[axis] - expected[axis]).abs() < 0.001);
            }
        }
    }

    #[test]
    fn cabinet_orbit_accumulates_exactly_two_turns() {
        let yaw = cabinet_yaw(4_000, 4_000, 2.0);
        assert!((yaw - 4.0 * std::f32::consts::PI).abs() < f32::EPSILON * 8.0);
    }
}
