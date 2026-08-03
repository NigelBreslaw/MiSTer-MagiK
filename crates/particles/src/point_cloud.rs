// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared packed point-cloud projection primitives.

pub(crate) const PARTICLE_LANES: usize = 4;
pub(crate) const INVALID_PARTICLE_OFFSET: u32 = u32::MAX;
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
        (self.0 != INVALID_PARTICLE_OFFSET)
            .then_some((self.0 & COMMAND_OFFSET_MASK) as usize)
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
        center_x: f32,
        center_y: f32,
        width: u32,
        height: u32,
        offsets: *mut u32,
    ) -> usize;
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
            center_x,
            center_y,
            width,
            height,
            offsets,
        );
        0
    }
}
