// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared packed-command ABI and RGB565 raster operations for MagiK.

use crate::cabinet::Rgb565Pixel;
use crate::engine::PARTICLE_NOT_VISIBLE_OFFSET;

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

pub fn raster_packed_visual_commands_recording(
    destination: &mut [Rgb565Pixel],
    commands: &[u32],
    palette: [Rgb565Pixel; 4],
    neighbor_palette_index: usize,
    dirty_offsets: &mut Vec<u32>,
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
        dirty_offsets.push(offset as u32);
        written = written.saturating_add(1);
        if neighbor {
            let neighbor_offset = offset.saturating_add(1);
            let Some(pixel) = destination.get_mut(neighbor_offset) else {
                continue;
            };
            *pixel = palette[neighbor_palette_index];
            dirty_offsets.push(neighbor_offset as u32);
            written = written.saturating_add(1);
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

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
