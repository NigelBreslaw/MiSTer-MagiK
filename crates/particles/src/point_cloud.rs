// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared packed point-cloud projection primitives.

pub(crate) const PARTICLE_LANES: usize = 4;
pub(crate) const INVALID_PARTICLE_OFFSET: u32 = u32::MAX;
pub(crate) const POSITION_Q5_SCALE: f32 = 32.0;
const COMMAND_OFFSET_MASK: u32 = (1 << 20) - 1;
const COMMAND_DEPTH_SHIFT: u32 = 20;
const COMMAND_X_SHIFT: u32 = 22;
const COMMAND_X_MASK: u32 = (1 << 10) - 1;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub(crate) struct PointCloudPositionBlock {
    pub(crate) target_x: [f32; PARTICLE_LANES],
    pub(crate) target_y: [f32; PARTICLE_LANES],
    pub(crate) target_z: [f32; PARTICLE_LANES],
    pub(crate) source_x: [f32; PARTICLE_LANES],
    pub(crate) source_y: [f32; PARTICLE_LANES],
    pub(crate) source_z: [f32; PARTICLE_LANES],
}

#[cfg_attr(not(all(target_os = "linux", target_arch = "arm")), allow(dead_code))]
pub(crate) struct QuantizedPointCloud {
    pub(crate) x_q5: Vec<i16>,
    pub(crate) y_q5: Vec<i16>,
    pub(crate) z_q5: Vec<i16>,
}

impl QuantizedPointCloud {
    pub(crate) fn from_positions(positions: &[[f32; 3]]) -> Self {
        let mut x_q5 = Vec::with_capacity(positions.len());
        let mut y_q5 = Vec::with_capacity(positions.len());
        let mut z_q5 = Vec::with_capacity(positions.len());
        for position in positions {
            x_q5.push(quantize_q5(position[0]));
            y_q5.push(quantize_q5(position[1]));
            z_q5.push(quantize_q5(position[2]));
        }
        Self { x_q5, y_q5, z_q5 }
    }

    pub(crate) fn from_unit_vectors(vectors: &[[f32; 3]]) -> Self {
        let mut x_q5 = Vec::with_capacity(vectors.len());
        let mut y_q5 = Vec::with_capacity(vectors.len());
        let mut z_q5 = Vec::with_capacity(vectors.len());
        for vector in vectors {
            x_q5.push(quantize_unit_q15(vector[0]));
            y_q5.push(quantize_unit_q15(vector[1]));
            z_q5.push(quantize_unit_q15(vector[2]));
        }
        Self { x_q5, y_q5, z_q5 }
    }
}

pub(crate) fn quantize_q5(value: f32) -> i16 {
    (value * POSITION_Q5_SCALE)
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

pub(crate) fn quantize_unit_q15(value: f32) -> i16 {
    (value * f32::from(i16::MAX))
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
}

#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct PointCloudDrawCommand(pub(crate) u32);

impl PointCloudDrawCommand {
    pub(crate) fn visible(offset: usize, depth: f32, pixel_x: usize) -> Self {
        let depth_band =
            u32::from(depth >= 480.0) + u32::from(depth >= 640.0) + u32::from(depth >= 800.0);
        Self(
            (offset as u32)
                | (depth_band << COMMAND_DEPTH_SHIFT)
                | ((pixel_x as u32) << COMMAND_X_SHIFT),
        )
    }

    pub(crate) fn offset(self) -> Option<usize> {
        (self.0 != INVALID_PARTICLE_OFFSET).then_some((self.0 & COMMAND_OFFSET_MASK) as usize)
    }

    pub(crate) fn depth_band(self) -> u8 {
        ((self.0 >> COMMAND_DEPTH_SHIFT) & 3) as u8
    }

    pub(crate) fn pixel_x(self) -> usize {
        ((self.0 >> COMMAND_X_SHIFT) & COMMAND_X_MASK) as usize
    }
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
unsafe extern "C" {
    fn mister_magik_cabinet_neon_project_stable(
        count: usize,
        blocks: *const PointCloudPositionBlock,
        first_block: usize,
        block_step: usize,
        sin_yaw: f32,
        cos_yaw: f32,
        sin_pitch: f32,
        cos_pitch: f32,
        dolly: f32,
        near_depth: f32,
        focal_length: f32,
        projection_scale_x: f32,
        projection_scale_y: f32,
        center_x: f32,
        center_y: f32,
        width: u32,
        height: u32,
        offsets: *mut u32,
    ) -> usize;

    fn mister_magik_intro_neon_letter_q5(
        start: usize,
        count: usize,
        source_x: *const i16,
        source_y: *const i16,
        source_z: *const i16,
        destination_x: *const i16,
        destination_y: *const i16,
        destination_z: *const i16,
        scatter_x: *const i16,
        scatter_y: *const i16,
        scatter_z: *const i16,
        source_pivot: *const i16,
        destination_pivot: *const i16,
        progress_q15: i16,
        sin_q15: i16,
        cos_q15: i16,
        scatter_radius_q5: i16,
        output: *mut PointCloudPositionBlock,
    );

    fn mister_magik_intro_neon_cloud_q5(
        start: usize,
        count: usize,
        source_x: *const i16,
        source_y: *const i16,
        source_z: *const i16,
        cloud_x: *const i16,
        cloud_y: *const i16,
        cloud_z: *const i16,
        cabinet_x: *const i16,
        cabinet_y: *const i16,
        cabinet_z: *const i16,
        pivot: *const i16,
        progress_q15: i16,
        formation_q15: i16,
        sin_q15: i16,
        cos_q15: i16,
        output: *mut PointCloudPositionBlock,
    );

    fn mister_magik_intro_neon_lerp_q5(
        count: usize,
        source_x: *const i16,
        source_y: *const i16,
        source_z: *const i16,
        destination_x: *const i16,
        destination_y: *const i16,
        destination_z: *const i16,
        progress_q15: i16,
        output: *mut PointCloudPositionBlock,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn transform_letter_q5_neon(
    start: usize,
    count: usize,
    source: &QuantizedPointCloud,
    destination: &QuantizedPointCloud,
    scatter: &QuantizedPointCloud,
    source_pivot: [i16; 3],
    destination_pivot: [i16; 3],
    progress_q15: i16,
    sin_q15: i16,
    cos_q15: i16,
    scatter_radius_q5: i16,
    output: &mut [PointCloudPositionBlock],
) -> bool {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    unsafe {
        mister_magik_intro_neon_letter_q5(
            start,
            count,
            source.x_q5.as_ptr(),
            source.y_q5.as_ptr(),
            source.z_q5.as_ptr(),
            destination.x_q5.as_ptr(),
            destination.y_q5.as_ptr(),
            destination.z_q5.as_ptr(),
            scatter.x_q5.as_ptr(),
            scatter.y_q5.as_ptr(),
            scatter.z_q5.as_ptr(),
            source_pivot.as_ptr(),
            destination_pivot.as_ptr(),
            progress_q15,
            sin_q15,
            cos_q15,
            scatter_radius_q5,
            output.as_mut_ptr(),
        );
        true
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        let _ = (
            start,
            count,
            source,
            destination,
            scatter,
            source_pivot,
            destination_pivot,
            progress_q15,
            sin_q15,
            cos_q15,
            scatter_radius_q5,
            output,
        );
        false
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn transform_cloud_q5_neon(
    start: usize,
    count: usize,
    source: &QuantizedPointCloud,
    cloud: &QuantizedPointCloud,
    cabinet: &QuantizedPointCloud,
    pivot: [i16; 3],
    progress_q15: i16,
    formation_q15: i16,
    sin_q15: i16,
    cos_q15: i16,
    output: &mut [PointCloudPositionBlock],
) -> bool {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    unsafe {
        mister_magik_intro_neon_cloud_q5(
            start,
            count,
            source.x_q5.as_ptr(),
            source.y_q5.as_ptr(),
            source.z_q5.as_ptr(),
            cloud.x_q5.as_ptr(),
            cloud.y_q5.as_ptr(),
            cloud.z_q5.as_ptr(),
            cabinet.x_q5.as_ptr(),
            cabinet.y_q5.as_ptr(),
            cabinet.z_q5.as_ptr(),
            pivot.as_ptr(),
            progress_q15,
            formation_q15,
            sin_q15,
            cos_q15,
            output.as_mut_ptr(),
        );
        true
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        let _ = (
            start,
            count,
            source,
            cloud,
            cabinet,
            pivot,
            progress_q15,
            formation_q15,
            sin_q15,
            cos_q15,
            output,
        );
        false
    }
}

pub(crate) fn transform_lerp_q5_neon(
    source: &QuantizedPointCloud,
    destination: &QuantizedPointCloud,
    progress_q15: i16,
    output: &mut [PointCloudPositionBlock],
) -> bool {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    unsafe {
        mister_magik_intro_neon_lerp_q5(
            source.x_q5.len(),
            source.x_q5.as_ptr(),
            source.y_q5.as_ptr(),
            source.z_q5.as_ptr(),
            destination.x_q5.as_ptr(),
            destination.y_q5.as_ptr(),
            destination.z_q5.as_ptr(),
            progress_q15,
            output.as_mut_ptr(),
        );
        true
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        let _ = (source, destination, progress_q15, output);
        false
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn project_stable_neon(
    count: usize,
    positions: &[PointCloudPositionBlock],
    first_block: usize,
    block_step: usize,
    sin_yaw: f32,
    cos_yaw: f32,
    sin_pitch: f32,
    cos_pitch: f32,
    dolly: f32,
    near_depth: f32,
    focal_length: f32,
    projection_scale_x: f32,
    projection_scale_y: f32,
    center_x: f32,
    center_y: f32,
    width: usize,
    height: usize,
    offsets: &mut [PointCloudDrawCommand],
) -> usize {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        if count < PARTICLE_LANES
            || block_step == 0
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
                first_block,
                block_step,
                sin_yaw,
                cos_yaw,
                sin_pitch,
                cos_pitch,
                dolly,
                near_depth,
                focal_length,
                projection_scale_x,
                projection_scale_y,
                center_x,
                center_y,
                width,
                height,
                offsets.as_mut_ptr().cast::<u32>(),
            )
        }
    }
    #[cfg(not(all(target_os = "linux", target_arch = "arm")))]
    {
        let _ = (
            count,
            positions,
            first_block,
            block_step,
            sin_yaw,
            cos_yaw,
            sin_pitch,
            cos_pitch,
            dolly,
            near_depth,
            focal_length,
            projection_scale_x,
            projection_scale_y,
            center_x,
            center_y,
            width,
            height,
            offsets,
        );
        0
    }
}
