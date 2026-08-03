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
use mister_magik_framebuffer_scenes::{
    FramebufferScene, SceneBufferId, SceneClock, SceneError, SceneGeometry, SceneTarget,
};
use std::time::Duration;

const ARCADE_CLOUD_POINT_COUNT: usize = 72_704;
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
    pub projection_backend: &'static str,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CabinetCreativeMode {
    #[default]
    Baseline,
    Satellites,
    HistoryEcho,
    DepthPalette,
    MicroJitter,
    All,
}

impl CabinetCreativeMode {
    pub const ALL: [Self; 6] = [
        Self::Baseline,
        Self::Satellites,
        Self::HistoryEcho,
        Self::DepthPalette,
        Self::MicroJitter,
        Self::All,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Baseline => "BASELINE",
            Self::Satellites => "SATELLITES",
            Self::HistoryEcho => "HISTORY ECHO",
            Self::DepthPalette => "DEPTH PALETTE",
            Self::MicroJitter => "MICRO-JITTER",
            Self::All => "ALL",
        }
    }

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CabinetRenderOptions {
    pub active_count: usize,
    pub creative_mode: CabinetCreativeMode,
}

const PARTICLE_LANES: usize = 4;

#[repr(C, align(16))]
struct CabinetPositionBlock {
    target_x: [f32; PARTICLE_LANES],
    target_y: [f32; PARTICLE_LANES],
    target_z: [f32; PARTICLE_LANES],
    source_x: [f32; PARTICLE_LANES],
    source_y: [f32; PARTICLE_LANES],
    source_z: [f32; PARTICLE_LANES],
}

#[repr(C, align(16))]
struct CabinetAttributeBlock {
    random: [u32; PARTICLE_LANES],
    life: [f32; PARTICLE_LANES],
    style: [u8; PARTICLE_LANES],
    flags: [u8; PARTICLE_LANES],
}

const INVALID_PARTICLE_OFFSET: u32 = u32::MAX;

#[cfg(all(target_os = "linux", target_arch = "arm"))]
unsafe extern "C" {
    fn mister_magik_cabinet_neon_project_stable(
        count: usize,
        blocks: *const CabinetPositionBlock,
        sin_yaw: f32,
        cos_yaw: f32,
        sin_pitch: f32,
        cos_pitch: f32,
        dolly: f32,
        near_depth: f32,
        focal_length: f32,
        center_x: f32,
        center_y: f32,
        width: u32,
        height: u32,
        offsets: *mut u32,
    ) -> usize;
}

/// Exact extraction of the approved arcade-cabinet particle formation.
pub struct ArcadeCabinetFormation {
    width: usize,
    height: usize,
    recipe: CabinetRecipe,
    capacity: usize,
    positions: Vec<CabinetPositionBlock>,
    attributes: Vec<CabinetAttributeBlock>,
    projected_offsets: Vec<u32>,
    options: CabinetRenderOptions,
}

pub struct CabinetScene {
    formation: ArcadeCabinetFormation,
    geometry: SceneGeometry,
    reusable_buffers: u8,
}

impl CabinetScene {
    pub fn new(
        width: usize,
        height: usize,
        recipe: CabinetRecipe,
        reusable_buffers: u8,
    ) -> Result<Self, String> {
        let capacity = recipe.particle_count;
        Self::new_with_capacity(width, height, recipe, reusable_buffers, capacity)
    }

    pub fn new_with_capacity(
        width: usize,
        height: usize,
        recipe: CabinetRecipe,
        reusable_buffers: u8,
        capacity: usize,
    ) -> Result<Self, String> {
        if reusable_buffers == 0 {
            return Err("cabinet scene requires at least one reusable buffer".into());
        }
        let geometry =
            SceneGeometry::new(width, height, width).map_err(|error| error.to_string())?;
        Ok(Self {
            formation: ArcadeCabinetFormation::new_with_capacity(width, height, recipe, capacity)?,
            geometry,
            reusable_buffers,
        })
    }

    pub fn set_render_options(&mut self, options: CabinetRenderOptions) -> Result<(), String> {
        self.formation.set_render_options(options)
    }

    #[must_use]
    pub const fn render_options(&self) -> CabinetRenderOptions {
        self.formation.render_options()
    }

    pub fn from_embedded(
        width: usize,
        height: usize,
        reusable_buffers: u8,
    ) -> Result<Self, String> {
        Self::new(width, height, embedded_cabinet_recipe()?, reusable_buffers)
    }
}

impl FramebufferScene for CabinetScene {
    type Stats = ArcadeCabinetFrameStats;

    fn geometry(&self) -> SceneGeometry {
        self.geometry
    }

    fn render(
        &mut self,
        target: SceneTarget<'_>,
        clock: SceneClock,
    ) -> Result<Self::Stats, SceneError> {
        if target.geometry() != self.geometry {
            return Err(SceneError::Render(format!(
                "cabinet target geometry {:?} does not match scene {:?}",
                target.geometry(),
                self.geometry
            )));
        }
        if target.buffer_id().get() >= self.reusable_buffers {
            return Err(SceneError::InvalidBufferId {
                value: target.buffer_id().get(),
                reusable_buffers: self.reusable_buffers,
            });
        }
        self.formation
            .render(target.into_pixels(), clock.elapsed)
            .map_err(SceneError::Render)
    }

    fn invalidate_buffer(&mut self, _buffer: SceneBufferId) {
        // Cabinet clears the complete target on every frame.
    }
}

impl ArcadeCabinetFormation {
    pub fn from_embedded(width: usize, height: usize) -> Result<Self, String> {
        Self::new(width, height, embedded_cabinet_recipe()?)
    }

    pub fn new(width: usize, height: usize, recipe: CabinetRecipe) -> Result<Self, String> {
        let capacity = recipe.particle_count;
        Self::new_with_capacity(width, height, recipe, capacity)
    }

    pub fn new_with_capacity(
        width: usize,
        height: usize,
        recipe: CabinetRecipe,
        capacity: usize,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("arcade cabinet formation requires a non-empty viewport".into());
        }
        if capacity < recipe.particle_count {
            return Err(format!(
                "arcade cabinet capacity {capacity} is below recipe count {}",
                recipe.particle_count
            ));
        }
        let active_count = recipe.particle_count;
        let mut target_x = vec![0.0; capacity];
        let mut target_y = vec![0.0; capacity];
        let mut target_z = vec![0.0; capacity];
        let mut source_x = vec![0.0; capacity];
        let mut source_y = vec![0.0; capacity];
        let mut source_z = vec![0.0; capacity];
        let mut random = vec![0; capacity];
        let mut life = vec![0.0; capacity];
        let mut style = vec![0; capacity];
        let mut flags = vec![0; capacity];
        let mut state = fold_seed(recipe.seed);
        for index in 0..capacity {
            state = xorshift32(state);
            random[index] = state;
            source_x[index] =
                unit_signed(state.rotate_left(3)) * recipe.source_scatter.x_half_extent;
            source_y[index] =
                unit_signed(state.rotate_left(13)) * recipe.source_scatter.y_half_extent;
            source_z[index] =
                unit_signed(state.rotate_left(23)) * recipe.source_scatter.z_half_extent;
        }
        decode_particle_cloud(
            ARCADE_CLOUD,
            recipe.model,
            &mut target_x,
            &mut target_y,
            &mut target_z,
            &mut life,
            &random,
            &mut style,
            &mut flags,
        )?;
        let block_count = capacity.div_ceil(PARTICLE_LANES);
        let mut positions = Vec::with_capacity(block_count);
        let mut attributes = Vec::with_capacity(block_count);
        for block_index in 0..block_count {
            let mut position = CabinetPositionBlock {
                target_x: [0.0; PARTICLE_LANES],
                target_y: [0.0; PARTICLE_LANES],
                target_z: [0.0; PARTICLE_LANES],
                source_x: [0.0; PARTICLE_LANES],
                source_y: [0.0; PARTICLE_LANES],
                source_z: [0.0; PARTICLE_LANES],
            };
            let mut attribute = CabinetAttributeBlock {
                random: [0; PARTICLE_LANES],
                life: [0.0; PARTICLE_LANES],
                style: [0; PARTICLE_LANES],
                flags: [0; PARTICLE_LANES],
            };
            for lane in 0..PARTICLE_LANES {
                let index = block_index * PARTICLE_LANES + lane;
                if index >= capacity {
                    break;
                }
                position.target_x[lane] = target_x[index];
                position.target_y[lane] = target_y[index];
                position.target_z[lane] = target_z[index];
                position.source_x[lane] = source_x[index];
                position.source_y[lane] = source_y[index];
                position.source_z[lane] = source_z[index];
                attribute.random[lane] = random[index];
                attribute.life[lane] = life[index];
                attribute.style[lane] = style[index];
                attribute.flags[lane] = flags[index];
            }
            positions.push(position);
            attributes.push(attribute);
        }
        Ok(Self {
            width,
            height,
            recipe,
            capacity,
            positions,
            attributes,
            projected_offsets: vec![INVALID_PARTICLE_OFFSET; capacity],
            options: CabinetRenderOptions {
                active_count,
                creative_mode: CabinetCreativeMode::Baseline,
            },
        })
    }

    #[must_use]
    pub const fn particle_count(&self) -> usize {
        self.options.active_count
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn set_render_options(&mut self, options: CabinetRenderOptions) -> Result<(), String> {
        if options.active_count == 0 || options.active_count > self.capacity() {
            return Err(format!(
                "cabinet active count {} is outside 1..={}",
                options.active_count,
                self.capacity()
            ));
        }
        self.options = options;
        Ok(())
    }

    #[must_use]
    pub const fn render_options(&self) -> CabinetRenderOptions {
        self.options
    }

    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        self.positions
            .capacity()
            .saturating_mul(std::mem::size_of::<CabinetPositionBlock>())
            .saturating_add(
                self.attributes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CabinetAttributeBlock>()),
            )
            .saturating_add(
                self.projected_offsets
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
    }

    pub fn render(
        &mut self,
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
        let appearance = self.recipe.appearance;
        let width_f32 = self.width as f32;
        let height_f32 = self.height as f32;
        let mut visible = 0usize;
        let mut pixel_writes = 0usize;
        let mut projection_backend = "cabinet-scalar";

        macro_rules! draw_offset {
            ($index:expr, $offset:expr, $pixel_x:expr, $attribute:expr, $lane:expr) => {{
                let index = $index;
                let offset = $offset;
                let pixel_x = $pixel_x;
                let attribute = $attribute;
                let lane = $lane;
                let feature = attribute.flags[lane];
                let style = if feature & appearance.priority_feature_mask != 0 {
                    appearance.priority_palette_index
                } else if feature & appearance.accent_feature_mask != 0 {
                    attribute.style[lane]
                        .saturating_add(appearance.accent_palette_add)
                        .min(7)
                } else {
                    attribute.style[lane]
                };
                destination[offset] = pixel(appearance.palette[usize::from(style)]);
                pixel_writes = pixel_writes.saturating_add(1);
                if feature & appearance.neighbor_feature_mask != 0
                    && index % usize::from(appearance.neighbor_every) == 0
                    && pixel_x + 1 < self.width
                {
                    let neighbor_style = style.saturating_sub(appearance.neighbor_palette_subtract);
                    destination[offset + 1] =
                        pixel(appearance.palette[usize::from(neighbor_style)]);
                    pixel_writes = pixel_writes.saturating_add(1);
                }
                visible = visible.saturating_add(1);
            }};
        }

        macro_rules! project_and_draw {
            ($index:expr, $attribute:expr, $lane:expr, $world_x:expr, $world_y:expr, $world_z:expr) => {{
                let index = $index;
                let world_x = $world_x;
                let world_y = $world_y;
                let world_z = $world_z;
                let rotated_x = world_x.mul_add(cos_yaw, world_z * sin_yaw);
                let yaw_z = (-world_x).mul_add(sin_yaw, world_z * cos_yaw);
                let rotated_y = world_y.mul_add(cos_pitch, -(yaw_z * sin_pitch));
                let rotated_z = world_y.mul_add(sin_pitch, yaw_z * cos_pitch);
                let depth = dolly + rotated_z;
                if depth > self.recipe.camera.near_depth {
                    let scale = self.recipe.camera.focal_length / depth;
                    let x = center_x + rotated_x * scale;
                    let y = center_y + rotated_y * scale;
                    if x >= 0.0 && y >= 0.0 && x < width_f32 && y < height_f32 {
                        let pixel_x = x as usize;
                        let offset = y as usize * self.width + pixel_x;
                        draw_offset!(index, offset, pixel_x, $attribute, $lane);
                    }
                }
            }};
        }

        if dispersal > 0.0 {
            for index in 0..self.options.active_count {
                let position = &self.positions[index / PARTICLE_LANES];
                let attribute = &self.attributes[index / PARTICLE_LANES];
                let lane = index % PARTICLE_LANES;
                let scale = 1.0
                    + dispersal
                        * (self.recipe.dispersal.radial_base
                            + attribute.life[lane] * self.recipe.dispersal.radial_life_gain);
                project_and_draw!(
                    index,
                    attribute,
                    lane,
                    position.target_x[lane] * scale,
                    position.target_y[lane] * scale
                        + dispersal
                            * unit_signed(attribute.random[lane].rotate_left(11))
                            * self.recipe.dispersal.vertical_jitter,
                    position.target_z[lane] * scale
                );
            }
        } else if formation < 1.0 {
            for index in 0..self.options.active_count {
                let position = &self.positions[index / PARTICLE_LANES];
                let attribute = &self.attributes[index / PARTICLE_LANES];
                let lane = index % PARTICLE_LANES;
                project_and_draw!(
                    index,
                    attribute,
                    lane,
                    position.source_x[lane]
                        + (position.target_x[lane] - position.source_x[lane]) * formation,
                    position.source_y[lane]
                        + (position.target_y[lane] - position.source_y[lane]) * formation,
                    position.source_z[lane]
                        + (position.target_z[lane] - position.source_z[lane]) * formation
                );
            }
        } else {
            let vector_end = project_stable_neon(
                self.options.active_count,
                &self.positions,
                sin_yaw,
                cos_yaw,
                sin_pitch,
                cos_pitch,
                dolly,
                self.recipe.camera.near_depth,
                self.recipe.camera.focal_length,
                center_x,
                center_y,
                self.width,
                self.height,
                &mut self.projected_offsets,
            );
            if vector_end > 0 {
                projection_backend = "cabinet-neon";
                for index in 0..vector_end {
                    let offset = self.projected_offsets[index];
                    if offset == INVALID_PARTICLE_OFFSET {
                        continue;
                    }
                    let attribute = &self.attributes[index / PARTICLE_LANES];
                    let lane = index % PARTICLE_LANES;
                    draw_offset!(
                        index,
                        offset as usize,
                        offset as usize % self.width,
                        attribute,
                        lane
                    );
                }
            }
            for index in vector_end..self.options.active_count {
                let position = &self.positions[index / PARTICLE_LANES];
                let attribute = &self.attributes[index / PARTICLE_LANES];
                let lane = index % PARTICLE_LANES;
                project_and_draw!(
                    index,
                    attribute,
                    lane,
                    position.target_x[lane],
                    position.target_y[lane],
                    position.target_z[lane]
                );
            }
        }

        Ok(ArcadeCabinetFrameStats {
            particles: self.options.active_count,
            visible,
            pixel_writes,
            projection_backend,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn project_stable_neon(
    count: usize,
    positions: &[CabinetPositionBlock],
    sin_yaw: f32,
    cos_yaw: f32,
    sin_pitch: f32,
    cos_pitch: f32,
    dolly: f32,
    near_depth: f32,
    focal_length: f32,
    center_x: f32,
    center_y: f32,
    width: usize,
    height: usize,
    offsets: &mut [u32],
) -> usize {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        if count < PARTICLE_LANES
            || positions.len() * PARTICLE_LANES < count
            || offsets.len() < count
        {
            return 0;
        }
        let Ok(width) = u32::try_from(width) else {
            return 0;
        };
        let Ok(height) = u32::try_from(height) else {
            return 0;
        };
        // SAFETY: position blocks have the C layout declared above, the input
        // covers count rounded down to four lanes, and offsets has count words.
        unsafe {
            mister_magik_cabinet_neon_project_stable(
                count,
                positions.as_ptr(),
                sin_yaw,
                cos_yaw,
                sin_pitch,
                cos_pitch,
                dolly,
                near_depth,
                focal_length,
                center_x,
                center_y,
                width,
                height,
                offsets.as_mut_ptr(),
            )
        }
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        let _ = (
            count,
            positions,
            sin_yaw,
            cos_yaw,
            sin_pitch,
            cos_pitch,
            dolly,
            near_depth,
            focal_length,
            center_x,
            center_y,
            width,
            height,
            offsets,
        );
        0
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
    if target_y.len() != output_count
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
    if output_count > count {
        return Err(format!(
            "arcade particle cloud capacity {output_count} exceeds its {count} unique points"
        ));
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
    fn checked_in_arcade_cloud_supports_the_48k_embedded_prefix() {
        let renderer = ArcadeCabinetFormation::from_embedded(960, 540).unwrap();
        assert_eq!(renderer.particle_count(), 48_128);
        assert_eq!(renderer.capacity(), 48_128);
        assert_eq!(
            u32::from_le_bytes(ARCADE_CLOUD[12..16].try_into().unwrap()) as usize,
            ARCADE_CLOUD_POINT_COUNT
        );
        assert_eq!(renderer.positions.as_ptr().align_offset(16), 0);
        assert_eq!(renderer.attributes.as_ptr().align_offset(16), 0);
    }

    #[test]
    fn checked_in_cloud_prefixes_have_unique_quantized_targets() {
        use std::collections::HashSet;

        for count in [1_024, 24_064, 48_128, 72_192, 72_704] {
            let unique = (0..count)
                .map(|index| {
                    let offset = PARTICLE_CLOUD_HEADER_BYTES + index * PARTICLE_CLOUD_RECORD_BYTES;
                    &ARCADE_CLOUD[offset..offset + 6]
                })
                .collect::<HashSet<_>>();
            assert_eq!(unique.len(), count);
        }
    }

    #[test]
    fn arcade_formation_is_deterministic() {
        let recipe = embedded_cabinet_recipe().unwrap();
        let mut first = ArcadeCabinetFormation::new(960, 540, recipe.clone()).unwrap();
        let mut second = ArcadeCabinetFormation::new(960, 540, recipe).unwrap();
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
