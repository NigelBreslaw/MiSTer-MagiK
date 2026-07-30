// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Character-ROM snapshot recompiler with deterministic exact-pixel verification.

use super::{
    NavigationTransitionBuffers, NavigationTransitionDirection, NavigationTransitionFailure,
    NavigationTransitionFrame, NavigationTransitionPhase, NavigationTransitionRect,
    NavigationTransitionRenderStats, NavigationTransitionRequest, PROGRESS_MAX,
    render_super_scaler_shell,
};
use slint::platform::software_renderer::Rgb565Pixel;

const FULL_COLUMNS: usize = 30;
const FULL_ROWS: usize = 17;
const FALLBACK_COLUMNS: usize = 20;
const FALLBACK_ROWS: usize = 12;
const FRAME_COUNT: usize = 26;
const MAX_NEW_FLIPS_PER_FRAME: usize = 96;
const VERIFY_START_Q16: u16 = 45_000;
const PALETTE: [Rgb565Pixel; 16] = [
    Rgb565Pixel(0x0000),
    Rgb565Pixel(0x000f),
    Rgb565Pixel(0x03e0),
    Rgb565Pixel(0x03ef),
    Rgb565Pixel(0x7800),
    Rgb565Pixel(0x780f),
    Rgb565Pixel(0x7be0),
    Rgb565Pixel(0xbdf7),
    Rgb565Pixel(0x4208),
    Rgb565Pixel(0x001f),
    Rgb565Pixel(0x07e0),
    Rgb565Pixel(0x07ff),
    Rgb565Pixel(0xf800),
    Rgb565Pixel(0xf81f),
    Rgb565Pixel(0xffe0),
    Rgb565Pixel(0xffff),
];
const CHARACTER_ROM: [[u8; 8]; 128] = build_character_rom();

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CharacterCell {
    glyph: u8,
    foreground: u8,
    background: u8,
}

#[derive(Debug)]
pub(super) struct CharacterRomRenderer {
    columns: usize,
    rows: usize,
    width: usize,
    height: usize,
    source_cells: Vec<CharacterCell>,
    destination_cells: Vec<CharacterCell>,
    flip_order: Vec<u16>,
    destination_rank: Vec<usize>,
    rendered_flips: usize,
    last_requested_flips: usize,
}

impl CharacterRomRenderer {
    pub(super) fn new(fallback: bool) -> Self {
        let (columns, rows) = if fallback {
            (FALLBACK_COLUMNS, FALLBACK_ROWS)
        } else {
            (FULL_COLUMNS, FULL_ROWS)
        };
        Self {
            columns,
            rows,
            width: 0,
            height: 0,
            source_cells: Vec::new(),
            destination_cells: Vec::new(),
            flip_order: Vec::new(),
            destination_rank: Vec::new(),
            rendered_flips: 0,
            last_requested_flips: 0,
        }
    }

    pub(super) fn prepare(
        &mut self,
        source: &[Rgb565Pixel],
        destination: &[Rgb565Pixel],
        width: usize,
        height: usize,
    ) {
        self.width = width;
        self.height = height;
        quantize_snapshot(
            source,
            width,
            height,
            self.columns,
            self.rows,
            &mut self.source_cells,
        );
        quantize_snapshot(
            destination,
            width,
            height,
            self.columns,
            self.rows,
            &mut self.destination_cells,
        );
        let cell_count = self.columns.saturating_mul(self.rows);
        self.flip_order.clear();
        self.flip_order
            .extend((0..cell_count).map(|index| index as u16));
        self.flip_order
            .sort_unstable_by_key(|index| mix32(u32::from(*index) ^ 0x4348_4152));
        self.destination_rank.clear();
        self.destination_rank.resize(cell_count, usize::MAX);
        for (rank, cell) in self.flip_order.iter().copied().enumerate() {
            self.destination_rank[cell as usize] = rank;
        }
        self.rendered_flips = 0;
        self.last_requested_flips = 0;
    }

    fn ready(&self, width: usize, height: usize) -> bool {
        let count = self.columns.saturating_mul(self.rows);
        self.width == width
            && self.height == height
            && self.source_cells.len() == count
            && self.destination_cells.len() == count
            && self.flip_order.len() == count
            && self.destination_rank.len() == count
    }

    fn advance_flips(&mut self, requested: usize) -> usize {
        if requested == self.last_requested_flips {
            return 0;
        }
        let increasing = requested > self.last_requested_flips;
        self.last_requested_flips = requested;
        let before = self.rendered_flips;
        if increasing && requested > self.rendered_flips {
            self.rendered_flips = self
                .rendered_flips
                .saturating_add((requested - self.rendered_flips).min(MAX_NEW_FLIPS_PER_FRAME));
        } else if !increasing && requested < self.rendered_flips {
            self.rendered_flips = self
                .rendered_flips
                .saturating_sub((self.rendered_flips - requested).min(MAX_NEW_FLIPS_PER_FRAME));
        }
        before.abs_diff(self.rendered_flips)
    }

    #[cfg(test)]
    fn cell_count(&self) -> usize {
        self.columns * self.rows
    }
}

pub(super) fn configured_fallback() -> bool {
    super::env_flag("MISTER_NAV_TRANSITION_REDUCED_EFFECTS")
        || std::env::var("MISTER_NAV_TRANSITION_CHARACTER_GRID")
            .ok()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("20x12"))
}

pub(super) fn render_character_rom(
    renderer: &mut CharacterRomRenderer,
    buffers: &mut NavigationTransitionBuffers,
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
) -> Result<NavigationTransitionRenderStats, NavigationTransitionFailure> {
    if frame.progress_q16 == 0 || frame.phase == NavigationTransitionPhase::Settled {
        return render_super_scaler_shell(buffers, request, frame);
    }
    let width = buffers.width;
    let height = buffers.height;
    if !renderer.ready(width, height) {
        let source = buffers
            .source
            .get(..)
            .filter(|_| buffers.source_ready)
            .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        let destination = buffers
            .destination
            .get(..)
            .filter(|_| buffers.destination_ready)
            .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
        renderer.prepare(source, destination, width, height);
    }
    let mut stats = render_super_scaler_shell(buffers, request, frame)?;
    let destination = buffers
        .destination
        .get(..)
        .filter(|_| buffers.destination_ready)
        .ok_or(NavigationTransitionFailure::SnapshotSizeMismatch)?;
    let clip = overlay_rect(request, frame, width, height);
    let working = buffers.working.as_mut_slice();
    let frame_index =
        frame.progress_q16 as usize * (FRAME_COUNT - 1) / PROGRESS_MAX.max(1) as usize;
    let requested_flips = frame_index * renderer.flip_order.len() / (FRAME_COUNT - 1);
    let new_flips = renderer.advance_flips(requested_flips);
    let flipped = renderer.rendered_flips;
    render_cells(
        working,
        width,
        height,
        clip,
        renderer,
        &renderer.destination_rank,
        flipped,
        &mut stats,
    );
    stats.cell_flips = flipped as u64;
    stats.new_cell_flips = new_flips as u64;

    if frame.reveal_progress_q16 >= VERIFY_START_Q16 {
        let verify_q16 = ((frame.reveal_progress_q16 - VERIFY_START_Q16) as u32
            * PROGRESS_MAX as u32
            / (PROGRESS_MAX - VERIFY_START_Q16) as u32)
            .min(PROGRESS_MAX as u32) as u16;
        let verified_rows = height * verify_q16 as usize / PROGRESS_MAX as usize;
        for y in 0..verified_rows.min(height) {
            let start = y * width;
            working[start..start + width].copy_from_slice(&destination[start..start + width]);
            stats.copied_pixels = stats.copied_pixels.saturating_add(width as u64);
        }
        stats.verified_rows = verified_rows as u64;
        if verified_rows > 0 && verified_rows < height {
            let beam_y = verified_rows.min(height - 1);
            working[beam_y * width..beam_y * width + width].fill(Rgb565Pixel(0xffff));
            stats.filled_pixels = stats.filled_pixels.saturating_add(width as u64);
        }
    }
    Ok(stats)
}

#[allow(clippy::too_many_arguments)]
fn render_cells(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    clip: NavigationTransitionRect,
    renderer: &CharacterRomRenderer,
    destination_rank: &[usize],
    flipped: usize,
    stats: &mut NavigationTransitionRenderStats,
) {
    if clip.width == 0 || clip.height == 0 {
        return;
    }
    for row in 0..renderer.rows {
        let y0 = row * height / renderer.rows;
        let y1 = (row + 1) * height / renderer.rows;
        if y1 <= clip.y as usize || y0 >= clip.bottom() as usize {
            continue;
        }
        for column in 0..renderer.columns {
            let x0 = column * width / renderer.columns;
            let x1 = (column + 1) * width / renderer.columns;
            if x1 <= clip.x as usize || x0 >= clip.right() as usize {
                continue;
            }
            let index = row * renderer.columns + column;
            let cell = if destination_rank[index] < flipped {
                renderer.destination_cells[index]
            } else {
                renderer.source_cells[index]
            };
            draw_cell(
                working,
                width,
                height,
                (
                    x0.max(clip.x as usize),
                    y0.max(clip.y as usize),
                    x1.min(clip.right() as usize),
                    y1.min(clip.bottom() as usize),
                ),
                cell,
                stats,
            );
        }
    }
}

fn draw_cell(
    working: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    bounds: (usize, usize, usize, usize),
    cell: CharacterCell,
    stats: &mut NavigationTransitionRenderStats,
) {
    let (x0, y0, x1, y1) = bounds;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    for y in y0..y1.min(height) {
        working[y * width + x0..y * width + x1].fill(PALETTE[cell.background as usize]);
        stats.filled_pixels = stats
            .filled_pixels
            .saturating_add(x1.saturating_sub(x0) as u64);
    }
    let glyph = &CHARACTER_ROM[cell.glyph as usize];
    for y in y0..y1.min(height) {
        let glyph_y = (y - y0) * 8 / (y1 - y0).max(1);
        let bits = glyph[glyph_y.min(7)];
        for x in x0..x1.min(width) {
            let glyph_x = (x - x0) * 8 / (x1 - x0).max(1);
            if bits & (1 << (7 - glyph_x.min(7))) != 0 {
                working[y * width + x] = PALETTE[cell.foreground as usize];
                stats.outline_pixels = stats.outline_pixels.saturating_add(1);
            }
        }
    }
}

fn quantize_snapshot(
    snapshot: &[Rgb565Pixel],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
    output: &mut Vec<CharacterCell>,
) {
    output.clear();
    output.reserve(columns.saturating_mul(rows));
    for row in 0..rows {
        let y0 = row * height / rows;
        let y1 = (row + 1) * height / rows;
        for column in 0..columns {
            let x0 = column * width / columns;
            let x1 = (column + 1) * width / columns;
            let samples = [
                snapshot.get(y0 * width + x0).copied().unwrap_or_default(),
                snapshot
                    .get(y0 * width + x1.saturating_sub(1))
                    .copied()
                    .unwrap_or_default(),
                snapshot
                    .get(y1.saturating_sub(1) * width + x0)
                    .copied()
                    .unwrap_or_default(),
                snapshot
                    .get(y1.saturating_sub(1) * width + x1.saturating_sub(1))
                    .copied()
                    .unwrap_or_default(),
            ];
            let average = average_rgb565(samples);
            let foreground = nearest_palette(average);
            let luminance = rgb565_luminance(average);
            let variance = samples
                .iter()
                .map(|pixel| rgb565_luminance(*pixel).abs_diff(luminance) as u16)
                .sum::<u16>();
            let glyph =
                ((luminance as u16 * 3 + variance + (row ^ column) as u16 * 11) & 0x7f) as u8;
            output.push(CharacterCell {
                glyph,
                foreground,
                background: if luminance < 64 { 0 } else { 8 },
            });
        }
    }
}

#[cfg(test)]
fn flips_for_frame(frame_index: usize, cell_count: usize) -> usize {
    if frame_index == 0 {
        return 0;
    }
    let current = frame_index * cell_count / (FRAME_COUNT - 1);
    let previous = (frame_index - 1) * cell_count / (FRAME_COUNT - 1);
    current.saturating_sub(previous)
}

fn average_rgb565(samples: [Rgb565Pixel; 4]) -> Rgb565Pixel {
    let mut red = 0u16;
    let mut green = 0u16;
    let mut blue = 0u16;
    for pixel in samples {
        red += (pixel.0 >> 11) & 0x1f;
        green += (pixel.0 >> 5) & 0x3f;
        blue += pixel.0 & 0x1f;
    }
    Rgb565Pixel(((red / 4) << 11) | ((green / 4) << 5) | (blue / 4))
}

fn nearest_palette(pixel: Rgb565Pixel) -> u8 {
    PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| rgb565_distance(pixel, **candidate))
        .map(|(index, _)| index as u8)
        .unwrap_or(0)
}

fn rgb565_distance(a: Rgb565Pixel, b: Rgb565Pixel) -> u32 {
    let ar = ((a.0 >> 11) & 0x1f) as i32;
    let ag = ((a.0 >> 5) & 0x3f) as i32;
    let ab = (a.0 & 0x1f) as i32;
    let br = ((b.0 >> 11) & 0x1f) as i32;
    let bg = ((b.0 >> 5) & 0x3f) as i32;
    let bb = (b.0 & 0x1f) as i32;
    ((ar - br).pow(2) + (ag - bg).pow(2) + (ab - bb).pow(2)) as u32
}

fn rgb565_luminance(pixel: Rgb565Pixel) -> u8 {
    let red = ((pixel.0 >> 11) & 0x1f) as u32 * 255 / 31;
    let green = ((pixel.0 >> 5) & 0x3f) as u32 * 255 / 63;
    let blue = (pixel.0 & 0x1f) as u32 * 255 / 31;
    ((red * 3 + green * 6 + blue) / 10) as u8
}

fn overlay_rect(
    request: NavigationTransitionRequest,
    frame: NavigationTransitionFrame,
    width: usize,
    height: usize,
) -> NavigationTransitionRect {
    let full = NavigationTransitionRect {
        x: 0,
        y: 0,
        width: width.min(u16::MAX as usize) as u16,
        height: height.min(u16::MAX as usize) as u16,
    };
    match request.direction {
        NavigationTransitionDirection::Forward if frame.reveal_progress_q16 == 0 => {
            super::lerp_rect(
                request.geometry.source_card,
                full,
                super::ease_out_cubic_q16(frame.cover_progress_q16),
            )
        }
        NavigationTransitionDirection::Reverse if frame.reveal_progress_q16 > 0 => {
            super::lerp_rect(
                full,
                request.geometry.source_card,
                super::ease_out_cubic_q16(frame.reveal_progress_q16),
            )
        }
        NavigationTransitionDirection::Reverse => {
            let cover = super::ease_out_cubic_q16(frame.cover_progress_q16);
            let covered_rows = height.saturating_mul(cover as usize) / PROGRESS_MAX as usize;
            NavigationTransitionRect {
                x: 0,
                y: height.saturating_sub(covered_rows).saturating_div(2) as u16,
                width: full.width,
                height: covered_rows as u16,
            }
        }
        NavigationTransitionDirection::Forward => full,
    }
}

const fn build_character_rom() -> [[u8; 8]; 128] {
    let mut rom = [[0u8; 8]; 128];
    let mut glyph = 0usize;
    while glyph < 128 {
        let mut row = 0usize;
        while row < 8 {
            let mut value = (glyph as u32)
                .wrapping_mul(0x45d9_f3b)
                .wrapping_add((row as u32).wrapping_mul(0x9e37_79b9));
            value ^= value >> 16;
            let diagonal = 1u8 << ((glyph + row) & 7);
            rom[glyph][row] = (value as u8).rotate_left((row & 7) as u32) ^ diagonal;
            row += 1;
        }
        glyph += 1;
    }
    rom[0] = [0; 8];
    rom
}

fn mix32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_is_deterministic_and_uses_fixed_contracts() {
        let pixels = (0..160 * 90)
            .map(|value| Rgb565Pixel(value as u16))
            .collect::<Vec<_>>();
        let mut first = Vec::new();
        let mut second = Vec::new();
        quantize_snapshot(&pixels, 160, 90, 30, 17, &mut first);
        quantize_snapshot(&pixels, 160, 90, 30, 17, &mut second);
        assert_eq!(first, second);
        assert_eq!(first.len(), 30 * 17);
        assert_eq!(CHARACTER_ROM.len(), 128);
        assert_eq!(PALETTE.len(), 16);
    }

    #[test]
    fn frame_flip_limit_is_bounded_in_both_grid_modes() {
        for cells in [FULL_COLUMNS * FULL_ROWS, FALLBACK_COLUMNS * FALLBACK_ROWS] {
            for frame in 0..FRAME_COUNT {
                assert!(flips_for_frame(frame, cells) <= MAX_NEW_FLIPS_PER_FRAME);
            }
        }
    }

    #[test]
    fn fallback_grid_has_the_requested_shape() {
        let renderer = CharacterRomRenderer::new(true);
        assert_eq!(renderer.cell_count(), 20 * 12);
    }
}
