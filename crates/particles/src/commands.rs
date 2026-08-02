// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared packed-command ABI and RGB565 raster operations for MagiK.

use crate::cabinet::Rgb565Pixel;
use crate::engine::{PARTICLE_NOT_VISIBLE_OFFSET, ParticleEngine};

pub const COMMAND_OFFSET_BITS: u32 = 20;
pub const COMMAND_OFFSET_MASK: u32 = (1 << COMMAND_OFFSET_BITS) - 1;
pub const COMMAND_PALETTE_SHIFT: u32 = COMMAND_OFFSET_BITS;
pub const COMMAND_NEIGHBOR: u32 = 1 << (COMMAND_PALETTE_SHIFT + 2);

#[must_use]
pub const fn pack_visual_command(offset: u32, palette_index: usize, neighbor: bool) -> u32 {
    debug_assert!(offset <= COMMAND_OFFSET_MASK);
    offset
        | ((palette_index as u32) << COMMAND_PALETTE_SHIFT)
        | if neighbor { COMMAND_NEIGHBOR } else { 0 }
}

#[must_use]
pub const fn unpack_visual_command(command: u32) -> Option<(usize, usize, bool)> {
    if command == PARTICLE_NOT_VISIBLE_OFFSET {
        return None;
    }
    Some((
        (command & COMMAND_OFFSET_MASK) as usize,
        ((command >> COMMAND_PALETTE_SHIFT) & 3) as usize,
        command & COMMAND_NEIGHBOR != 0,
    ))
}

/// Projects a complete visual frame into reusable initialized storage.
pub fn write_packed_visual_commands(engine: &ParticleEngine, output: &mut Vec<u32>) -> usize {
    output.clear();
    let count = engine.particle_count();
    output.reserve(count);
    let visible = engine.project_packed_commands(&mut output.spare_capacity_mut()[..count], true);
    // SAFETY: `project_packed_commands` initializes exactly `count` entries.
    unsafe {
        output.set_len(count);
    }
    visible
}

pub fn raster_packed_visual_commands(
    destination: &mut [Rgb565Pixel],
    commands: &[u32],
    palette: [Rgb565Pixel; 4],
    neighbor_palette_index: usize,
) -> usize {
    raster_packed_visual_commands_inner(
        destination,
        commands,
        palette,
        neighbor_palette_index,
        |_| {},
    )
}

pub fn raster_packed_visual_commands_recording(
    destination: &mut [Rgb565Pixel],
    commands: &[u32],
    palette: [Rgb565Pixel; 4],
    neighbor_palette_index: usize,
    dirty_offsets: &mut Vec<u32>,
) -> usize {
    raster_packed_visual_commands_inner(
        destination,
        commands,
        palette,
        neighbor_palette_index,
        |offset| dirty_offsets.push(offset),
    )
}

fn raster_packed_visual_commands_inner(
    destination: &mut [Rgb565Pixel],
    commands: &[u32],
    palette: [Rgb565Pixel; 4],
    neighbor_palette_index: usize,
    mut record: impl FnMut(u32),
) -> usize {
    assert!(neighbor_palette_index < palette.len());
    let mut written = 0usize;
    for &command in commands {
        let Some((offset, palette_index, neighbor)) = unpack_visual_command(command) else {
            continue;
        };
        let Some(pixel) = destination.get_mut(offset) else {
            continue;
        };
        *pixel = palette[palette_index];
        record(offset as u32);
        written = written.saturating_add(1);
        if neighbor {
            let Some(pixel) = destination.get_mut(offset.saturating_add(1)) else {
                continue;
            };
            *pixel = palette[neighbor_palette_index];
            record(offset.saturating_add(1) as u32);
            written = written.saturating_add(1);
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ParticleConfig, ParticlePreset, magik_target_mask};
    use std::time::Duration;

    #[test]
    fn safe_writer_initializes_one_command_per_particle() {
        let mut engine = ParticleEngine::new(
            ParticleConfig {
                count: 64,
                width: 960,
                height: 540,
                seed: 7,
                preset: ParticlePreset::Visual,
            },
            magik_target_mask().unwrap(),
        )
        .unwrap();
        engine.step(Duration::from_secs(6));
        let mut commands = Vec::new();

        let visible = write_packed_visual_commands(&engine, &mut commands);

        assert_eq!(commands.len(), engine.particle_count());
        assert!(visible <= commands.len());
        assert_eq!(
            visible,
            commands
                .iter()
                .filter(|command| **command != PARTICLE_NOT_VISIBLE_OFFSET)
                .count()
        );
    }

    #[test]
    fn packed_visual_commands_use_the_supplied_palette_and_neighbor() {
        let palette = [
            Rgb565Pixel(0x0001),
            Rgb565Pixel(0x0002),
            Rgb565Pixel(0x0003),
            Rgb565Pixel(0x0004),
        ];
        let commands = [
            pack_visual_command(2, 1, false),
            pack_visual_command(5, 3, true),
        ];
        let mut destination = [Rgb565Pixel(0); 8];
        let mut dirty = Vec::new();

        assert_eq!(
            raster_packed_visual_commands_recording(
                &mut destination,
                &commands,
                palette,
                2,
                &mut dirty,
            ),
            3
        );
        assert_eq!(destination[2], palette[1]);
        assert_eq!(destination[5], palette[3]);
        assert_eq!(destination[6], palette[2]);
        assert_eq!(dirty, vec![2, 5, 6]);
    }
}
