// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable orientation zoom compositor shared by the app and Mini.
pub const COLUMNS: usize = 16;
pub const ROWS: usize = 9;
pub const TILES: usize = COLUMNS * ROWS;
pub const SKIP: u8 = 255;

pub fn centered_span(start: usize, end: usize, level: u8) -> (usize, usize) {
    let span = end.saturating_sub(start);
    if span == 0 {
        return (start, start);
    }
    let scaled = span
        .saturating_sub(1)
        .saturating_mul(usize::from(level))
        .saturating_add(16)
        / 32;
    let visible = 1usize.saturating_add(scaled).min(span);
    let center = start + span.saturating_sub(1) / 2;
    let first = center.saturating_sub((visible - 1) / 2).max(start);
    (first, first.saturating_add(visible).min(end))
}

fn valid(
    source: &[u16],
    output: &[u16],
    width: usize,
    height: usize,
    levels: &[u8; TILES],
) -> bool {
    width.checked_mul(height) == Some(source.len())
        && source.len() == output.len()
        && levels.iter().all(|&v| v <= 32 || v == SKIP)
}

/// Previous levels must describe this exact destination buffer and unchanged
/// source contents. Pass None on source/reveal change, buffer replacement or reset.
#[allow(clippy::too_many_arguments)]
pub fn render_zoom_retained(
    source: &[u16],
    output: &mut [u16],
    width: usize,
    height: usize,
    levels: &[u8; TILES],
    dirty_rows: &[u16; ROWS],
    previous: Option<&[u8; TILES]>,
) -> Option<usize> {
    if !valid(source, output, width, height, levels) {
        return None;
    }
    let Some(previous) = previous else {
        if !render_zoom(source, output, width, height, levels, dirty_rows) {
            return None;
        }
        return Some(
            (0..ROWS)
                .map(|row| {
                    (0..COLUMNS)
                        .filter(|&col| dirty_rows[row] & (1 << col) != 0)
                        .map(|col| {
                            ((col + 1) * width / COLUMNS - col * width / COLUMNS)
                                * ((row + 1) * height / ROWS - row * height / ROWS)
                        })
                        .sum::<usize>()
                })
                .sum(),
        );
    };
    if !previous.iter().all(|&v| v <= 32 || v == SKIP) {
        return None;
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    if width >= COLUMNS && height >= ROWS && {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| {
            std::env::var("MISTER_ORIENTATION_SIMD")
                .ok()
                .is_none_or(|v| !v.trim().eq_ignore_ascii_case("scalar"))
        })
    } {
        unsafe extern "C" {
            fn mister_magik_orientation_zoom_delta_neon(
                source: *const u16,
                output: *mut u16,
                width: usize,
                height: usize,
                levels: *const u8,
                previous: *const u8,
                dirty_rows: *const u16,
            ) -> usize;
        }
        // SAFETY: complete disjoint planes, valid nonempty tile geometry and arrays.
        return Some(unsafe {
            mister_magik_orientation_zoom_delta_neon(
                source.as_ptr(),
                output.as_mut_ptr(),
                width,
                height,
                levels.as_ptr(),
                previous.as_ptr(),
                dirty_rows.as_ptr(),
            )
        });
    }
    let mut writes = 0;
    for row in 0..ROWS {
        for col in 0..COLUMNS {
            if dirty_rows[row] & (1 << col) == 0 {
                continue;
            }
            let tile = row * COLUMNS + col;
            let rect = |level| {
                if level == SKIP {
                    return (0, 0, 0, 0);
                }
                let (x0, x1) =
                    centered_span(col * width / COLUMNS, (col + 1) * width / COLUMNS, level);
                let (y0, y1) = centered_span(row * height / ROWS, (row + 1) * height / ROWS, level);
                (x0, y0, x1, y1)
            };
            let old = rect(previous[tile]);
            let now = rect(levels[tile]);
            // Scalar reference visits the tile, changing only the symmetric difference.
            for y in row * height / ROWS..(row + 1) * height / ROWS {
                for x in col * width / COLUMNS..(col + 1) * width / COLUMNS {
                    let inside = |r: (usize, usize, usize, usize)| {
                        x >= r.0 && x < r.2 && y >= r.1 && y < r.3
                    };
                    if inside(old) != inside(now) {
                        output[y * width + x] = if inside(now) {
                            0
                        } else {
                            source[y * width + x]
                        };
                        writes += 1;
                    }
                }
            }
        }
    }
    Some(writes)
}

/// Apply changed tiles to a retained, tightly packed RGB565 plane.
pub fn render_zoom(
    source: &[u16],
    output: &mut [u16],
    width: usize,
    height: usize,
    levels: &[u8; TILES],
    dirty_rows: &[u16; ROWS],
) -> bool {
    if !valid(source, output, width, height, levels) {
        return false;
    }
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let enabled = *ENABLED.get_or_init(|| {
            std::env::var("MISTER_ORIENTATION_SIMD")
                .ok()
                .is_none_or(|v| !v.trim().eq_ignore_ascii_case("scalar"))
        });
        if enabled && width >= COLUMNS && height >= ROWS {
            unsafe extern "C" {
                fn mister_magik_orientation_zoom_neon(
                    source: *const u16,
                    output: *mut u16,
                    width: usize,
                    height: usize,
                    levels: *const u8,
                    dirty_rows: *const u16,
                );
            }
            // SAFETY: validated complete distinct planes, nonempty tiles, and fixed arrays.
            unsafe {
                mister_magik_orientation_zoom_neon(
                    source.as_ptr(),
                    output.as_mut_ptr(),
                    width,
                    height,
                    levels.as_ptr(),
                    dirty_rows.as_ptr(),
                );
            }
            return true;
        }
    }
    render_zoom_scalar(source, output, width, height, levels, dirty_rows)
}

/// Production scalar fallback and target diagnostic oracle.
pub fn render_zoom_scalar(
    source: &[u16],
    output: &mut [u16],
    width: usize,
    height: usize,
    levels: &[u8; TILES],
    dirty_rows: &[u16; ROWS],
) -> bool {
    if !valid(source, output, width, height, levels) {
        return false;
    }
    for tile_row in 0..ROWS {
        let y0 = tile_row * height / ROWS;
        let y1 = (tile_row + 1) * height / ROWS;
        for column in 0..COLUMNS {
            if dirty_rows[tile_row] & (1 << column) == 0 {
                continue;
            }
            let x0 = column * width / COLUMNS;
            let x1 = (column + 1) * width / COLUMNS;
            for y in y0..y1 {
                output[y * width + x0..y * width + x1]
                    .copy_from_slice(&source[y * width + x0..y * width + x1]);
            }
            let level = levels[tile_row * COLUMNS + column];
            if level == SKIP {
                continue;
            }
            let (bx0, bx1) = centered_span(x0, x1, level);
            let (by0, by1) = centered_span(y0, y1, level);
            for y in by0..by1 {
                output[y * width + bx0..y * width + bx1].fill(0);
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zoom_endpoints_and_untouched_tiles() {
        let source = vec![0xffff; 32 * 18];
        let mut output = vec![0xa55a; source.len()];
        let mut rows = [0; ROWS];
        rows[0] = 1;
        assert!(render_zoom(
            &source,
            &mut output,
            32,
            18,
            &[32; TILES],
            &rows
        ));
        assert_eq!(output[0], 0);
        assert_eq!(output[32], 0);
        assert_eq!(output[2], 0xa55a);
        assert_eq!(output[64], 0xa55a);
        assert!(render_zoom(
            &source,
            &mut output,
            32,
            18,
            &[SKIP; TILES],
            &rows
        ));
        assert_eq!(output[0], 0xffff);
        assert_eq!(output[2], 0xa55a);
        assert!(!render_zoom(
            &source,
            &mut output,
            32,
            18,
            &[33; TILES],
            &rows
        ));
    }
}
