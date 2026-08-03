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

#[derive(Clone, Copy)]
struct RetiringFormationPoint {
    source: [f32; 2],
    target: [f32; 2],
    threshold: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum IntroSlotState {
    #[default]
    Uninitialized,
    Dynamic,
    Crossfade(u8),
    Snapshot,
}

#[derive(Clone, Copy)]
struct IntroRenderResult {
    visible: usize,
    pixel_writes: usize,
    projection_backend: &'static str,
    transform_us: u64,
    projection_us: u64,
    raster_us: u64,
}

impl IntroRenderResult {
    fn raster_only(
        visible: usize,
        pixel_writes: usize,
        projection_backend: &'static str,
        raster_us: u64,
    ) -> Self {
        Self {
            visible,
            pixel_writes,
            projection_backend,
            transform_us: 0,
            projection_us: 0,
            raster_us,
        }
    }

    fn with_outer_transform(mut self, transform_us: u64) -> Self {
        self.transform_us = self.transform_us.saturating_add(transform_us);
        self
    }
}

pub struct IntroScene {
    geometry: SceneGeometry,
    recipe: IntroRecipe,
    mister: PointTarget,
    mister_pivots: [[f32; 3]; 6],
    magik: PointTarget,
    magik_pivots: [[f32; 3]; 6],
    scatter_vectors: Vec<[f32; 3]>,
    cloud: PointTarget,
    cabinet_formed: Vec<[f32; 3]>,
    cabinet_formation: f32,
    cabinet_final_yaw: f32,
    launcher: PointTarget,
    launcher_snapshot: Vec<Rgb565Pixel>,
    launcher_commands: Vec<PointCloudDrawCommand>,
    launcher_thresholds: Vec<u8>,
    crossfade_visible_counts: [usize; 65],
    crossfade_buckets: Vec<Vec<u32>>,
    slot_states: [IntroSlotState; 2],
    static_xy: Vec<[f32; 2]>,
    static_origins: Vec<[u16; 2]>,
    formation_screen: Vec<[f32; 2]>,
    formation_styles: Vec<u8>,
    retiring_formation: Vec<RetiringFormationPoint>,
    dynamic_positions: Vec<[f32; 3]>,
    positions: Vec<PointCloudPositionBlock>,
    commands: Vec<PointCloudDrawCommand>,
}

impl IntroScene {
    pub fn new(width: usize, height: usize, recipe: IntroRecipe) -> Result<Self, String> {
        let geometry =
            SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
        let mister = decode_target(MISTER_CLOUD, Some((MISTER_GROUPS, 6)), TargetScale::Text)?;
        let magik = decode_target(MAGIK_CLOUD, Some((MAGIK_GROUPS, 6)), TargetScale::Text)?;
        if mister.positions.len() != recipe.steady_particle_count {
            return Err(format!(
                "MiSTer target has {} particles, expected {}",
                mister.positions.len(),
                recipe.steady_particle_count
            ));
        }
        if magik.positions.len() != recipe.steady_particle_count || magik.groups != mister.groups {
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
        let mister_screen: Vec<[f32; 2]> = mister
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
        let cloud = letter_cloud_target(
            recipe.steady_particle_count,
            recipe.seed,
            cloud_radius,
            &magik.groups,
            &magik_pivots,
        );
        let mut cabinet = decode_target(CABINET_CLOUD, None, TargetScale::Cabinet)?;
        cabinet.positions.truncate(recipe.steady_particle_count);
        cabinet.palette.truncate(recipe.steady_particle_count);
        cabinet.groups = vec![ParticleGroupSpan {
            id: 0,
            start: 0,
            count: recipe.steady_particle_count,
        }];
        let (cabinet_formation, cabinet_final_yaw) = match recipe.cues.get(6) {
            Some(IntroCue::TargetOrbit {
                start_turns,
                turns,
                formation_percent,
                ..
            }) => (
                *formation_percent / 100.0,
                (*start_turns + *turns) * std::f32::consts::TAU,
            ),
            _ => return Err("intro cabinet orbit cue is missing".into()),
        };
        let cabinet_formed = cloud
            .positions
            .iter()
            .zip(&cabinet.positions)
            .map(|(cloud, cabinet)| {
                [
                    cloud[0] + (cabinet[0] - cloud[0]) * cabinet_formation,
                    cloud[1] + (cabinet[1] - cloud[1]) * cabinet_formation,
                    cloud[2] + (cabinet[2] - cloud[2]) * cabinet_formation,
                ]
            })
            .collect();
        let mut launcher = decode_target(
            LAUNCHER_CLOUD,
            Some((LAUNCHER_GROUPS, 1)),
            TargetScale::Launcher,
        )?;
        let launcher_projection_compensation = launcher_projection_compensation(&recipe);
        for position in &mut launcher.positions {
            position[0] *= launcher_projection_compensation;
            position[1] *= launcher_projection_compensation;
        }
        let launcher_snapshot = decode_launcher_snapshot(LAUNCHER_SNAPSHOT, width, height)?;
        let launcher_commands = launcher
            .positions
            .iter()
            .map(|position| project_command(*position, &recipe, geometry))
            .collect::<Vec<_>>();
        let launcher_thresholds = launcher_commands
            .iter()
            .map(|command| {
                command.offset().map_or(64, |offset| {
                    bayer8(offset % geometry.width(), offset / geometry.width())
                })
            })
            .collect();
        let crossfade_visible_counts = std::array::from_fn(|threshold| {
            launcher_commands
                .iter()
                .zip(&launcher_thresholds)
                .filter(|(command, particle_threshold)| {
                    command.offset().is_some() && usize::from(**particle_threshold) >= threshold
                })
                .count()
        });
        let mut crossfade_buckets = vec![Vec::new(); 64];
        for offset in 0..width.saturating_mul(height) {
            crossfade_buckets[usize::from(bayer8(offset % width, offset / width))]
                .push(offset as u32);
        }
        let mut static_xy = Vec::with_capacity(recipe.initial_particle_count);
        let mut static_origins = Vec::with_capacity(recipe.initial_particle_count);
        let mut formation_screen = Vec::with_capacity(recipe.steady_particle_count);
        let mut formation_styles = Vec::with_capacity(recipe.steady_particle_count);
        let mut retiring_formation = Vec::with_capacity(
            recipe
                .initial_particle_count
                .saturating_sub(recipe.steady_particle_count),
        );
        for index in 0..recipe.initial_particle_count {
            let random = mix32((recipe.seed as u32).wrapping_add(index as u32));
            let target_index = index % recipe.steady_particle_count;
            let source = [
                unit01(random) * width as f32,
                unit01(random.rotate_left(13)) * height as f32,
            ];
            static_origins.push([source[0] as u16, source[1] as u16]);
            static_xy.push(source);
            if index < recipe.steady_particle_count {
                formation_screen.push(mister_screen[target_index]);
                formation_styles.push(mister.palette[target_index]);
            } else {
                retiring_formation.push(RetiringFormationPoint {
                    source,
                    target: mister_screen[target_index],
                    threshold: (mix32((index as u32).wrapping_add(recipe.seed as u32)) & 0xffff)
                        as u16,
                });
            }
        }
        retiring_formation.sort_by_key(|point| point.threshold);
        let positions = vec![empty_block(); recipe.steady_particle_count.div_ceil(PARTICLE_LANES)];
        let dynamic_positions = vec![[0.0; 3]; recipe.steady_particle_count];
        let commands =
            vec![PointCloudDrawCommand(INVALID_PARTICLE_OFFSET); recipe.steady_particle_count];
        Ok(Self {
            geometry,
            recipe,
            mister,
            mister_pivots,
            magik,
            magik_pivots,
            scatter_vectors,
            cloud,
            cabinet_formed,
            cabinet_formation,
            cabinet_final_yaw,
            launcher,
            launcher_snapshot,
            launcher_commands,
            launcher_thresholds,
            crossfade_visible_counts,
            crossfade_buckets,
            slot_states: [IntroSlotState::Uninitialized; 2],
            static_xy,
            static_origins,
            formation_screen,
            formation_styles,
            retiring_formation,
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

    fn render_crt(&self, destination: &mut [Rgb565Pixel], frame: u64) -> IntroRenderResult {
        let raster_started = Instant::now();
        let palette = self.recipe.appearance.crt_palette;
        let mut visible = 0;
        for (index, source) in self.static_origins.iter().enumerate() {
            let noise = mix32((index as u32) ^ (frame as u32).wrapping_mul(0x9e37_79b9));
            if noise & 7 == 0 {
                continue;
            }
            let x = wrap_small_jitter(source[0], (noise & 3) as u16, self.geometry.width());
            let y = wrap_small_jitter(source[1], ((noise >> 2) & 3) as u16, self.geometry.height());
            destination[y * self.geometry.width() + x] =
                Rgb565Pixel(palette[((noise >> 30) & 3) as usize].0);
            visible += 1;
        }
        IntroRenderResult::raster_only(
            visible,
            visible,
            "crt-packed",
            elapsed_us(raster_started.elapsed()),
        )
    }

    fn render_mister_formation(
        &self,
        destination: &mut [Rgb565Pixel],
        progress: f32,
        frame: u64,
    ) -> IntroRenderResult {
        let raster_started = Instant::now();
        let progress = ease(progress, RecipeEasing::EaseOutCubic);
        let crt_palette = self.recipe.appearance.crt_palette;
        let text_palette = self.recipe.appearance.text_palette;
        let mut visible = 0;
        let mut draw = |source: [f32; 2], target: [f32; 2], color: Rgb565Pixel| {
            let x = source[0] + (target[0] - source[0]) * progress;
            let y = source[1] + (target[1] - source[1]) * progress;
            if x >= 0.0
                && y >= 0.0
                && x < self.geometry.width() as f32
                && y < self.geometry.height() as f32
            {
                destination[y as usize * self.geometry.width() + x as usize] = color;
                visible += 1;
            }
        };
        for index in 0..self.recipe.steady_particle_count {
            let target = self.formation_screen[index];
            let source = self.static_xy[index];
            let flicker = text_flicker_index(self.formation_styles[index], index, frame);
            draw(source, target, Rgb565Pixel(text_palette[flicker].0));
        }
        let keep_threshold = ((1.0 - progress) * 65_535.0) as u16;
        let retire_count = self
            .retiring_formation
            .partition_point(|point| point.threshold <= keep_threshold);
        let fade = ((1.0 - progress) * 3.0) as usize;
        let retire_color = Rgb565Pixel(crt_palette[fade.min(3)].0);
        for point in &self.retiring_formation[..retire_count] {
            draw(point.source, point.target, retire_color);
        }
        IntroRenderResult::raster_only(
            visible,
            visible,
            "crt-to-point-cloud",
            elapsed_us(raster_started.elapsed()),
        )
    }

    fn render_point_target(
        &mut self,
        destination: &mut [Rgb565Pixel],
        target: ScenePointTarget,
        frame: u64,
    ) -> IntroRenderResult {
        let text_palette_mix =
            matches!(target, ScenePointTarget::Mister | ScenePointTarget::Magik).then_some(0.0);
        let target = match target {
            ScenePointTarget::Mister => &self.mister,
            ScenePointTarget::Magik => &self.magik,
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
            false,
            frame,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_letter_morph(
        &mut self,
        destination: &mut [Rgb565Pixel],
        cue_elapsed_ms: u64,
        duration_ms: u64,
        turns: f32,
        stagger_ms: u64,
        easing: RecipeEasing,
        frame: u64,
        update_all: bool,
    ) -> IntroRenderResult {
        let transform_started = Instant::now();
        let local_duration = duration_ms
            .saturating_sub(stagger_ms.saturating_mul(5))
            .max(1);
        for (group_index, span) in self.mister.groups.iter().enumerate() {
            let start_ms = stagger_ms.saturating_mul(group_index as u64);
            let progress = cue_elapsed_ms.saturating_sub(start_ms) as f32 / local_duration as f32;
            let progress = ease(progress, easing);
            let source_pivot = self.mister_pivots[group_index];
            let destination_pivot = self.magik_pivots[group_index];
            let angle = progress * turns * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            for index in span.start..span.start + span.count {
                if !update_all && !updates_transform_cohort(index, frame) {
                    continue;
                }
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
                    center[0] + local[0].mul_add(cos, local[2] * sin) + scatter_vector[0] * scatter,
                    center[1] + local[1] + scatter_vector[1] * scatter,
                    center[2]
                        + (-local[0]).mul_add(sin, local[2] * cos)
                        + scatter_vector[2] * scatter,
                ];
            }
        }
        let transform_us = elapsed_us(transform_started.elapsed());
        render_point_cloud(
            destination,
            &self.recipe,
            self.geometry,
            &self.dynamic_positions,
            &self.magik.palette,
            &mut self.positions,
            &mut self.commands,
            Some(0.0),
            false,
            frame,
        )
        .with_outer_transform(transform_us)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_letters_to_cabinet(
        &mut self,
        destination: &mut [Rgb565Pixel],
        cue_elapsed_ms: u64,
        duration_ms: u64,
        turns: f32,
        letter_turns: f32,
        stagger_ms: u64,
        _radius: f32,
        formation_start_percent: f32,
        formation_end_percent: f32,
        easing: RecipeEasing,
        frame: u64,
        update_all: bool,
    ) -> IntroRenderResult {
        let transform_started = Instant::now();
        let cue_progress = ease(
            cue_elapsed_ms as f32 / duration_ms.max(1) as f32,
            RecipeEasing::Linear,
        );
        let global_yaw = cue_progress * turns * std::f32::consts::TAU;
        let (global_sin, global_cos) = global_yaw.sin_cos();
        let local_duration = duration_ms
            .saturating_sub(stagger_ms.saturating_mul(4))
            .max(1);
        for (group_index, span) in self.magik.groups.iter().enumerate() {
            let start_ms = letter_stagger_start_ms(group_index, stagger_ms);
            let progress = ease(
                cue_elapsed_ms.saturating_sub(start_ms) as f32 / local_duration as f32,
                easing,
            );
            let local_angle = progress * letter_turns * std::f32::consts::TAU;
            let (local_sin, local_cos) = local_angle.sin_cos();
            let formation_percent = formation_start_percent
                + (formation_end_percent - formation_start_percent) * progress;
            let formation = formation_percent / 100.0;
            let pivot = self.magik_pivots[group_index];
            for index in span.start..span.start + span.count {
                if !update_all && !updates_transform_cohort(index, frame) {
                    continue;
                }
                let source = self.magik.positions[index];
                let local_x = source[0] - pivot[0];
                let local_z = source[2] - pivot[2];
                let spun = [
                    pivot[0] + local_x.mul_add(local_cos, local_z * local_sin),
                    source[1],
                    pivot[2] + (-local_x).mul_add(local_sin, local_z * local_cos),
                ];
                let cloud = self.cloud.positions[index];
                let cabinet = self.cabinet_formed[index];
                let formed = [
                    cloud[0] + (cabinet[0] - cloud[0]) * (formation / self.cabinet_formation),
                    cloud[1] + (cabinet[1] - cloud[1]) * (formation / self.cabinet_formation),
                    cloud[2] + (cabinet[2] - cloud[2]) * (formation / self.cabinet_formation),
                ];
                let point = [
                    spun[0] + (formed[0] - spun[0]) * progress,
                    spun[1] + (formed[1] - spun[1]) * progress,
                    spun[2] + (formed[2] - spun[2]) * progress,
                ];
                self.dynamic_positions[index] = [
                    point[0].mul_add(global_cos, point[2] * global_sin),
                    point[1],
                    (-point[0]).mul_add(global_sin, point[2] * global_cos),
                ];
            }
        }
        let transform_us = elapsed_us(transform_started.elapsed());
        render_point_cloud(
            destination,
            &self.recipe,
            self.geometry,
            &self.dynamic_positions,
            &self.magik.palette,
            &mut self.positions,
            &mut self.commands,
            Some(0.0),
            false,
            frame,
        )
        .with_outer_transform(transform_us)
    }

    #[allow(clippy::too_many_arguments)]
    fn render_cabinet_orbit(
        &mut self,
        destination: &mut [Rgb565Pixel],
        cue_elapsed_ms: u64,
        duration_ms: u64,
        start_turns: f32,
        turns: f32,
        formation_percent: f32,
        frame: u64,
    ) -> IntroRenderResult {
        let transform_started = Instant::now();
        let yaw = cabinet_yaw(cue_elapsed_ms, duration_ms, start_turns, turns);
        let (sin, cos) = yaw.sin_cos();
        debug_assert!((formation_percent / 100.0 - self.cabinet_formation).abs() < 0.0001);
        for index in 0..self.recipe.steady_particle_count {
            let point = self.cabinet_formed[index];
            let dynamic = &mut self.dynamic_positions[index];
            *dynamic = [
                point[0].mul_add(cos, point[2] * sin),
                point[1],
                (-point[0]).mul_add(sin, point[2] * cos),
            ];
        }
        let transform_us = elapsed_us(transform_started.elapsed());
        render_point_cloud(
            destination,
            &self.recipe,
            self.geometry,
            &self.dynamic_positions,
            &self.magik.palette,
            &mut self.positions,
            &mut self.commands,
            Some(0.0),
            false,
            frame,
        )
        .with_outer_transform(transform_us)
    }

    fn render_launcher_morph(
        &mut self,
        destination: &mut [Rgb565Pixel],
        cue_elapsed_ms: u64,
        duration_ms: u64,
        easing: RecipeEasing,
        frame: u64,
    ) -> IntroRenderResult {
        let transform_started = Instant::now();
        let progress = ease(cue_elapsed_ms as f32 / duration_ms as f32, easing);
        let (sin, cos) = self.cabinet_final_yaw.sin_cos();
        for index in 0..self.recipe.steady_particle_count {
            let formed = self.cabinet_formed[index];
            let from = [
                formed[0].mul_add(cos, formed[2] * sin),
                formed[1],
                (-formed[0]).mul_add(sin, formed[2] * cos),
            ];
            let to = self.launcher.positions[index];
            self.dynamic_positions[index] = [
                from[0] + (to[0] - from[0]) * progress,
                from[1] + (to[1] - from[1]) * progress,
                from[2] + (to[2] - from[2]) * progress,
            ];
        }
        let transform_us = elapsed_us(transform_started.elapsed());
        render_point_cloud(
            destination,
            &self.recipe,
            self.geometry,
            &self.dynamic_positions,
            &self.launcher.palette,
            &mut self.positions,
            &mut self.commands,
            Some(progress),
            false,
            frame,
        )
        .with_outer_transform(transform_us)
    }

    fn render_mock_crossfade(
        &mut self,
        destination: &mut [Rgb565Pixel],
        buffer_id: usize,
        cue_elapsed_ms: u64,
        duration_ms: u64,
        easing: RecipeEasing,
        _frame: u64,
    ) -> IntroRenderResult {
        let raster_started = Instant::now();
        let progress = ease(cue_elapsed_ms as f32 / duration_ms as f32, easing);
        let threshold = ((progress * 64.0).round() as u8).min(64);
        let previous = self.slot_states[buffer_id];
        if previous == IntroSlotState::Snapshot && threshold == 64 {
            return IntroRenderResult::raster_only(
                0,
                0,
                "launcher-static-crossfade",
                elapsed_us(raster_started.elapsed()),
            );
        }
        if let IntroSlotState::Crossfade(previous_threshold) = previous
            && previous_threshold <= threshold
        {
            let mut writes = 0;
            for bucket in
                &self.crossfade_buckets[usize::from(previous_threshold)..usize::from(threshold)]
            {
                for &offset in bucket {
                    destination[offset as usize] = self.launcher_snapshot[offset as usize];
                    writes += 1;
                }
            }
            self.slot_states[buffer_id] = if threshold == 64 {
                IntroSlotState::Snapshot
            } else {
                IntroSlotState::Crossfade(threshold)
            };
            return IntroRenderResult::raster_only(
                self.crossfade_visible_counts[usize::from(threshold)],
                writes,
                "launcher-static-crossfade",
                elapsed_us(raster_started.elapsed()),
            );
        }

        destination.fill(Rgb565Pixel(self.recipe.appearance.background.0));
        let mut writes = destination.len();
        for (index, command) in self.launcher_commands.iter().copied().enumerate() {
            let Some(offset) = command.offset() else {
                continue;
            };
            if self.launcher_thresholds[index] >= threshold {
                destination[offset] = Rgb565Pixel(
                    self.recipe.appearance.palette[usize::from(self.launcher.palette[index])].0,
                );
                writes += 1;
            }
        }
        for bucket in &self.crossfade_buckets[..usize::from(threshold)] {
            for &offset in bucket {
                destination[offset as usize] = self.launcher_snapshot[offset as usize];
                writes += 1;
            }
        }
        self.slot_states[buffer_id] = if threshold == 64 {
            IntroSlotState::Snapshot
        } else {
            IntroSlotState::Crossfade(threshold)
        };
        IntroRenderResult::raster_only(
            self.crossfade_visible_counts[usize::from(threshold)],
            writes,
            "launcher-static-crossfade",
            elapsed_us(raster_started.elapsed()),
        )
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
    text_neighbors: bool,
    frame: u64,
) -> IntroRenderResult {
    let transform_started = Instant::now();
    copy_target_to_blocks(target_positions, positions);
    let transform_us = elapsed_us(transform_started.elapsed());
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
    let projection_us = elapsed_us(projection_started.elapsed());
    let raster_started = Instant::now();
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
                mix32(index as u32 ^ recipe.seed as u32 ^ 0xa5a5_5a5a) & 0xffff < threshold
            }
        };
        let color = if !target_color {
            recipe.appearance.text_palette[text_flicker_index(target_palette[index], index, frame)]
        } else {
            recipe.appearance.palette[usize::from(target_palette[index])]
        };
        destination[offset] = Rgb565Pixel(color.0);
        visible += 1;
        writes += 1;
        if !target_color
            && text_neighbors
            && target_positions[index][2] < 0.0
            && offset % geometry.width() + 1 < geometry.width()
        {
            destination[offset + 1] = Rgb565Pixel(recipe.appearance.text_palette[2].0);
            writes += 1;
        }
    }
    IntroRenderResult {
        visible,
        pixel_writes: writes,
        projection_backend: backend,
        transform_us,
        projection_us,
        raster_us: elapsed_us(raster_started.elapsed()),
    }
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
        let buffer_id = usize::from(target.buffer_id().get());
        let incremental_frame = matches!(&cue, IntroCue::MockCrossfade { .. }) || cue_index == 9;
        let clear_us = if incremental_frame {
            0
        } else {
            let clear_started = Instant::now();
            target
                .pixels_mut()
                .fill(Rgb565Pixel(self.recipe.appearance.background.0));
            self.slot_states[buffer_id] = IntroSlotState::Dynamic;
            elapsed_us(clear_started.elapsed())
        };
        let update_all_transforms = clock.next_elapsed.is_none() || cue_elapsed_ms < 34;
        let rendered = match &cue {
            IntroCue::CrtStatic { .. } => self.render_crt(target.pixels_mut(), clock.frame),
            IntroCue::MorphTarget { duration_ms, .. } if cue_index == 1 => {
                let progress = cue_elapsed_ms as f32 / *duration_ms as f32;
                self.render_mister_formation(target.pixels_mut(), progress, clock.frame)
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
                update_all_transforms,
            ),
            IntroCue::Cloud {
                duration_ms,
                turns,
                letter_turns,
                stagger_ms,
                radius,
                formation_start_percent,
                formation_end_percent,
                easing,
                ..
            } => self.render_letters_to_cabinet(
                target.pixels_mut(),
                cue_elapsed_ms,
                *duration_ms,
                *turns,
                *letter_turns,
                *stagger_ms,
                *radius,
                *formation_start_percent,
                *formation_end_percent,
                *easing,
                clock.frame,
                update_all_transforms,
            ),
            IntroCue::TargetOrbit {
                duration_ms,
                start_turns,
                turns,
                formation_percent,
                ..
            } => self.render_cabinet_orbit(
                target.pixels_mut(),
                cue_elapsed_ms,
                *duration_ms,
                *start_turns,
                *turns,
                *formation_percent,
                clock.frame,
            ),
            IntroCue::MorphTarget {
                duration_ms,
                easing,
                ..
            } if cue_index == 7 => self.render_launcher_morph(
                target.pixels_mut(),
                cue_elapsed_ms,
                *duration_ms,
                *easing,
                clock.frame,
            ),
            IntroCue::MockCrossfade {
                duration_ms,
                easing,
                ..
            } => self.render_mock_crossfade(
                target.pixels_mut(),
                buffer_id,
                cue_elapsed_ms,
                *duration_ms,
                *easing,
                clock.frame,
            ),
            IntroCue::HoldTarget { .. } if cue_index == 9 => {
                let raster_started = Instant::now();
                let writes = if self.slot_states[buffer_id] == IntroSlotState::Snapshot {
                    0
                } else {
                    target.pixels_mut().copy_from_slice(&self.launcher_snapshot);
                    self.slot_states[buffer_id] = IntroSlotState::Snapshot;
                    self.launcher_snapshot.len()
                };
                IntroRenderResult::raster_only(
                    0,
                    writes,
                    "launcher-mock-rgb565",
                    elapsed_us(raster_started.elapsed()),
                )
            }
            _ => {
                let point_target = match cue_index {
                    0..=3 => ScenePointTarget::Mister,
                    4 => ScenePointTarget::Magik,
                    _ => ScenePointTarget::Launcher,
                };
                self.render_point_target(target.pixels_mut(), point_target, clock.frame)
            }
        };
        let particles = if cue_index < 2 {
            self.recipe.initial_particle_count
        } else {
            self.recipe.steady_particle_count
        };
        Ok(IntroFrameStats {
            particles,
            projected_particles: particles,
            projection_cohorts: u8::from(matches!(
                cue,
                IntroCue::LetterMorph { .. } | IntroCue::Cloud { .. }
            )) + 1,
            visible: rendered.visible,
            pixel_writes: rendered.pixel_writes,
            cue_index,
            cue_start_ms,
            previous_cue_start_ms,
            cue_elapsed_ms,
            cue_duration_ms: cue.duration_ms(),
            cue_id: cue_label(cue_index),
            projection_backend: rendered.projection_backend,
            stages: IntroStageTimings {
                clear_us,
                transform_us: rendered.transform_us,
                projection_us: rendered.projection_us,
                raster_us: rendered.raster_us,
            },
        })
    }

    fn invalidate_buffer(&mut self, buffer: SceneBufferId) {
        self.slot_states[usize::from(buffer.get())] = IntroSlotState::Uninitialized;
    }
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
                f32::from(y) * (220.0 / 32_767.0),
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
        decode_particle_groups(bytes, positions.len(), count)?
            .spans()
            .to_vec()
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

fn launcher_projection_compensation(recipe: &IntroRecipe) -> f32 {
    recipe.camera.dolly / recipe.camera.focal_length
}

#[inline(always)]
fn wrap_small_jitter(origin: u16, jitter: u16, extent: usize) -> usize {
    let coordinate = usize::from(origin + jitter);
    if coordinate >= extent {
        coordinate - extent
    } else {
        coordinate
    }
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
    [
        sum[0] * reciprocal,
        sum[1] * reciprocal,
        sum[2] * reciprocal,
    ]
}

fn letter_cloud_target(
    count: usize,
    seed: u64,
    radius: f32,
    groups: &[ParticleGroupSpan],
    pivots: &[[f32; 3]; 6],
) -> PointTarget {
    let mut positions = Vec::with_capacity(count);
    let mut palette = Vec::with_capacity(count);
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    for (group_index, span) in groups.iter().enumerate() {
        let pivot = pivots[group_index];
        for local_index in 0..span.count {
            let index = span.start + local_index;
            let unit = (local_index as f32 + 0.5) / span.count as f32;
            let y = 1.0 - unit * 2.0;
            let radial = (1.0 - y * y).sqrt();
            let angle = local_index as f32 * golden_angle;
            let jitter = 0.78 + unit01(mix32(index as u32 ^ seed as u32)) * 0.22;
            positions.push([
                pivot[0] + angle.cos() * radial * radius * jitter,
                pivot[1] + y * radius * 0.58,
                pivot[2] + angle.sin() * radial * radius * jitter,
            ]);
            palette.push((index & 7) as u8);
        }
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

fn cabinet_yaw(elapsed_ms: u64, duration_ms: u64, start_turns: f32, turns: f32) -> f32 {
    (start_turns + (elapsed_ms as f32 / duration_ms.max(1) as f32).clamp(0.0, 1.0) * turns)
        * std::f32::consts::TAU
}

fn letter_stagger_start_ms(group_index: usize, stagger_ms: u64) -> u64 {
    stagger_ms.saturating_mul(group_index.min(4) as u64)
}

fn updates_transform_cohort(index: usize, frame: u64) -> bool {
    ((index / PARTICLE_LANES) & 1) == ((frame as usize) & 1)
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
        5 => "letters-to-cabinet",
        6 => "cabinet-orbit",
        7 => "form-launcher",
        8 => "crossfade",
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
        assert_eq!(scene.static_origins.len(), 102_400);
        assert_eq!(scene.formation_screen.len(), 40_960);
        assert_eq!(scene.retiring_formation.len(), 61_440);
        assert!(
            scene
                .retiring_formation
                .windows(2)
                .all(|pair| pair[0].threshold <= pair[1].threshold)
        );
        assert_eq!(scene.mister.positions.len(), 40_960);
    }

    #[test]
    fn cached_crt_coordinate_wrap_matches_modulo() {
        for extent in [540, 960] {
            for origin in 0..extent as u16 {
                for jitter in 0..=3 {
                    assert_eq!(
                        wrap_small_jitter(origin, jitter, extent),
                        (usize::from(origin) + usize::from(jitter)) % extent
                    );
                }
            }
        }
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
    fn text_targets_are_centered_in_projected_screen_space() {
        let recipe = embedded_intro_recipe().unwrap();
        let scene = IntroScene::new(960, 540, recipe.clone()).unwrap();
        let geometry = SceneGeometry::new(960, 540, 960).unwrap();
        for target in [&scene.mister, &scene.magik] {
            let mut min = [f32::INFINITY; 2];
            let mut max = [f32::NEG_INFINITY; 2];
            for position in &target.positions {
                let screen = project(*position, 0.0, 0.0, &recipe, geometry);
                for axis in 0..2 {
                    min[axis] = min[axis].min(screen[axis]);
                    max[axis] = max[axis].max(screen[axis]);
                }
            }
            assert!(((min[0] + max[0]) * 0.5 - 480.0).abs() < 2.0);
            assert!(((min[1] + max[1]) * 0.5 - 270.0).abs() < 2.0);
        }
    }

    #[test]
    fn common_m_remains_cohesive_while_translating_between_centered_words() {
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
            true,
        );
        let m = scene.mister.groups[0];
        let source_pivot = pivot(&scene.mister.positions[m.start..m.start + m.count]);
        let destination_pivot = pivot(&scene.magik.positions[m.start..m.start + m.count]);
        let dynamic_pivot = pivot(&scene.dynamic_positions[m.start..m.start + m.count]);
        let progress = ease(1_750.0 / 2_750.0, RecipeEasing::Smoothstep);
        for axis in 0..3 {
            let expected =
                source_pivot[axis] + (destination_pivot[axis] - source_pivot[axis]) * progress;
            assert!((expected - dynamic_pivot[axis]).abs() < 0.01);
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
            true,
        );
        for (actual, expected) in scene.dynamic_positions.iter().zip(&scene.magik.positions) {
            for axis in 0..3 {
                assert!((actual[axis] - expected[axis]).abs() < 0.001);
            }
        }
    }

    #[test]
    fn final_k_shares_one_hundred_ms_stagger_and_cabinet_orbit_is_slow() {
        assert_eq!(
            (0..6)
                .map(|group| letter_stagger_start_ms(group, 100))
                .collect::<Vec<_>>(),
            [0, 100, 200, 300, 400, 400]
        );
        let yaw = cabinet_yaw(4_000, 4_000, 0.3, 0.4);
        assert!((yaw - 0.7 * std::f32::consts::TAU).abs() < f32::EPSILON * 8.0);
        let scene = IntroScene::new(960, 540, embedded_intro_recipe().unwrap()).unwrap();
        assert!((scene.cabinet_formation - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn flat_launcher_target_precompensates_the_camera_to_exact_pixels() {
        let recipe = embedded_intro_recipe().unwrap();
        let geometry = SceneGeometry::new(960, 540, 960).unwrap();
        let compensation = launcher_projection_compensation(&recipe);
        let top_left = project(
            [-480.0 * compensation, -270.0 * compensation, 0.0],
            0.0,
            0.0,
            &recipe,
            geometry,
        );
        let bottom_right = project(
            [479.0 * compensation, 269.0 * compensation, 0.0],
            0.0,
            0.0,
            &recipe,
            geometry,
        );
        assert!((top_left[0] - 0.0).abs() < 0.001);
        assert!((top_left[1] - 0.0).abs() < 0.001);
        assert!((bottom_right[0] - 959.0).abs() < 0.001);
        assert!((bottom_right[1] - 539.0).abs() < 0.001);
    }

    #[test]
    fn incremental_crossfade_matches_fresh_absolute_timestamp_frames() {
        let recipe = embedded_intro_recipe().unwrap();
        let geometry = SceneGeometry::new(960, 540, 960).unwrap();
        let mut live = IntroScene::new(960, 540, recipe.clone()).unwrap();
        let mut reference = IntroScene::new(960, 540, recipe).unwrap();
        let mut slots = [
            vec![Rgb565Pixel(0); geometry.len()],
            vec![Rgb565Pixel(0); geometry.len()],
        ];
        for (frame, time_ms) in (18_000..=19_000).step_by(125).enumerate() {
            let slot = frame & 1;
            let buffer = SceneBufferId::new(slot as u8, 2).unwrap();
            let clock = SceneClock {
                frame: frame as u64,
                elapsed: Duration::from_millis(time_ms),
                next_elapsed: Some(Duration::from_millis(time_ms + 16)),
            };
            live.render(
                SceneTarget::new(&mut slots[slot], geometry, buffer).unwrap(),
                clock,
            )
            .unwrap();

            let mut expected = vec![Rgb565Pixel(0); geometry.len()];
            reference.invalidate_buffer(buffer);
            reference
                .render(
                    SceneTarget::new(&mut expected, geometry, buffer).unwrap(),
                    clock,
                )
                .unwrap();
            assert_eq!(slots[slot], expected, "time_ms={time_ms} slot={slot}");
        }
    }
}
