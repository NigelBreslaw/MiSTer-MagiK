// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Host-neutral RGB565 monitor-orientation transition compositor.

use crate::settings::ScreenOrientation;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::{Duration, Instant};

pub const ORIENTATION_WAVE_PHASE_DURATION: Duration = Duration::from_millis(1_500);
pub const ORIENTATION_WAVE_TOTAL_DURATION: Duration = Duration::from_millis(3_000);
const ORIENTATION_GRID_COLUMNS: usize = 16;
const ORIENTATION_GRID_ROWS: usize = 9;
const ORIENTATION_TILE_DELAY_US: u64 = 40_000;
const ORIENTATION_TILE_FADE_US: u64 = 300_000;
const RGB565_OPACITY_LEVELS: u8 = 32;
const ORIENTATION_TRANSITION_COLOR: Rgb565Pixel = Rgb565Pixel(0x1082);
const ORIENTATION_TILE_COUNT: usize = ORIENTATION_GRID_COLUMNS * ORIENTATION_GRID_ROWS;
const ORIENTATION_TILE_SKIP: u8 = u8::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrientationTransitionEffect {
    BrightnessFade,
    CenterPixelZoom,
}

impl OrientationTransitionEffect {
    pub const fn id(self) -> &'static str {
        match self {
            Self::BrightnessFade => "brightness-fade",
            Self::CenterPixelZoom => "center-pixel-zoom",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "brightness-fade" => Some(Self::BrightnessFade),
            "center-pixel-zoom" => Some(Self::CenterPixelZoom),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrientationTransitionCompletion {
    pub from: ScreenOrientation,
    pub to: ScreenOrientation,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrientationTransitionRenderStats {
    pub fill_us: u64,
    pub map_us: u64,
    pub crossfade_us: u64,
    pub total_us: u64,
    pub mapped_pixels: u64,
    pub blended_pixels: u64,
    pub progress_ppm: u32,
}

#[derive(Clone, Copy)]
pub enum OrientationPmuPhase {
    Destination,
    Fill,
    Map,
    Crossfade,
    CacheRestore,
}

pub const fn orientation_pmu_label(
    effect: OrientationTransitionEffect,
    from: ScreenOrientation,
    to: ScreenOrientation,
    phase: OrientationPmuPhase,
) -> &'static str {
    const FADE_LABELS: [[&str; 5]; 6] = [
        [
            "orientation.fade.normal-clockwise.destination",
            "orientation.fade.normal-clockwise.fill",
            "orientation.fade.normal-clockwise.map",
            "orientation.fade.normal-clockwise.crossfade",
            "orientation.fade.normal-clockwise.cache-restore",
        ],
        [
            "orientation.fade.clockwise-counterclockwise.destination",
            "orientation.fade.clockwise-counterclockwise.fill",
            "orientation.fade.clockwise-counterclockwise.map",
            "orientation.fade.clockwise-counterclockwise.crossfade",
            "orientation.fade.clockwise-counterclockwise.cache-restore",
        ],
        [
            "orientation.fade.counterclockwise-normal.destination",
            "orientation.fade.counterclockwise-normal.fill",
            "orientation.fade.counterclockwise-normal.map",
            "orientation.fade.counterclockwise-normal.crossfade",
            "orientation.fade.counterclockwise-normal.cache-restore",
        ],
        [
            "orientation.fade.normal-counterclockwise.destination",
            "orientation.fade.normal-counterclockwise.fill",
            "orientation.fade.normal-counterclockwise.map",
            "orientation.fade.normal-counterclockwise.crossfade",
            "orientation.fade.normal-counterclockwise.cache-restore",
        ],
        [
            "orientation.fade.counterclockwise-clockwise.destination",
            "orientation.fade.counterclockwise-clockwise.fill",
            "orientation.fade.counterclockwise-clockwise.map",
            "orientation.fade.counterclockwise-clockwise.crossfade",
            "orientation.fade.counterclockwise-clockwise.cache-restore",
        ],
        [
            "orientation.fade.clockwise-normal.destination",
            "orientation.fade.clockwise-normal.fill",
            "orientation.fade.clockwise-normal.map",
            "orientation.fade.clockwise-normal.crossfade",
            "orientation.fade.clockwise-normal.cache-restore",
        ],
    ];
    const ZOOM_LABELS: [[&str; 5]; 6] = [
        [
            "orientation.zoom.normal-clockwise.destination",
            "orientation.zoom.normal-clockwise.fill",
            "orientation.zoom.normal-clockwise.map",
            "orientation.zoom.normal-clockwise.crossfade",
            "orientation.zoom.normal-clockwise.cache-restore",
        ],
        [
            "orientation.zoom.clockwise-counterclockwise.destination",
            "orientation.zoom.clockwise-counterclockwise.fill",
            "orientation.zoom.clockwise-counterclockwise.map",
            "orientation.zoom.clockwise-counterclockwise.crossfade",
            "orientation.zoom.clockwise-counterclockwise.cache-restore",
        ],
        [
            "orientation.zoom.counterclockwise-normal.destination",
            "orientation.zoom.counterclockwise-normal.fill",
            "orientation.zoom.counterclockwise-normal.map",
            "orientation.zoom.counterclockwise-normal.crossfade",
            "orientation.zoom.counterclockwise-normal.cache-restore",
        ],
        [
            "orientation.zoom.normal-counterclockwise.destination",
            "orientation.zoom.normal-counterclockwise.fill",
            "orientation.zoom.normal-counterclockwise.map",
            "orientation.zoom.normal-counterclockwise.crossfade",
            "orientation.zoom.normal-counterclockwise.cache-restore",
        ],
        [
            "orientation.zoom.counterclockwise-clockwise.destination",
            "orientation.zoom.counterclockwise-clockwise.fill",
            "orientation.zoom.counterclockwise-clockwise.map",
            "orientation.zoom.counterclockwise-clockwise.crossfade",
            "orientation.zoom.counterclockwise-clockwise.cache-restore",
        ],
        [
            "orientation.zoom.clockwise-normal.destination",
            "orientation.zoom.clockwise-normal.fill",
            "orientation.zoom.clockwise-normal.map",
            "orientation.zoom.clockwise-normal.crossfade",
            "orientation.zoom.clockwise-normal.cache-restore",
        ],
    ];
    let leg = match (from, to) {
        (ScreenOrientation::Normal, ScreenOrientation::MonitorClockwise) => 0,
        (ScreenOrientation::MonitorClockwise, ScreenOrientation::MonitorCounterclockwise) => 1,
        (ScreenOrientation::MonitorCounterclockwise, ScreenOrientation::Normal) => 2,
        (ScreenOrientation::Normal, ScreenOrientation::MonitorCounterclockwise) => 3,
        (ScreenOrientation::MonitorCounterclockwise, ScreenOrientation::MonitorClockwise) => 4,
        (ScreenOrientation::MonitorClockwise, ScreenOrientation::Normal) => 5,
        _ => return "orientation.invalid",
    };
    let phase = match phase {
        OrientationPmuPhase::Destination => 0,
        OrientationPmuPhase::Fill => 1,
        OrientationPmuPhase::Map => 2,
        OrientationPmuPhase::Crossfade => 3,
        OrientationPmuPhase::CacheRestore => 4,
    };
    match effect {
        OrientationTransitionEffect::BrightnessFade => FADE_LABELS[leg][phase],
        OrientationTransitionEffect::CenterPixelZoom => ZOOM_LABELS[leg][phase],
    }
}

pub struct OrientationTransitionRuntime {
    width: usize,
    height: usize,
    from: ScreenOrientation,
    to: ScreenOrientation,
    started_at: Instant,
    duration: Duration,
    effect: OrientationTransitionEffect,
    source: Vec<Rgb565Pixel>,
    destination: Vec<Rgb565Pixel>,
    destination_ready: bool,
    active: bool,
    completion: Option<OrientationTransitionCompletion>,
    last_render_stats: OrientationTransitionRenderStats,
    previous_levels: [u8; ORIENTATION_TILE_COUNT],
    previous_levels_valid: bool,
    previous_revealing: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrientationTransitionDamage {
    rows: [u16; ORIENTATION_GRID_ROWS],
}

impl OrientationTransitionDamage {
    const fn full() -> Self {
        Self {
            rows: [u16::MAX; ORIENTATION_GRID_ROWS],
        }
    }

    fn changed(
        previous: &[u8; ORIENTATION_TILE_COUNT],
        current: &[u8; ORIENTATION_TILE_COUNT],
    ) -> Self {
        let mut damage = Self::default();
        for row in 0..ORIENTATION_GRID_ROWS {
            let mut mask = 0_u16;
            for column in 0..ORIENTATION_GRID_COLUMNS {
                let index = row * ORIENTATION_GRID_COLUMNS + column;
                if previous[index] != current[index] {
                    mask |= 1_u16 << column;
                }
            }
            damage.rows[row] = mask;
        }
        damage
    }

    pub fn rect_for_row(
        self,
        row: usize,
        width: usize,
        height: usize,
    ) -> Option<(usize, usize, usize, usize)> {
        let mask = *self.rows.get(row)?;
        if mask == 0 {
            return None;
        }
        let first_column = mask.trailing_zeros() as usize;
        let last_column = ORIENTATION_GRID_COLUMNS - mask.leading_zeros() as usize;
        Some((
            first_column * width / ORIENTATION_GRID_COLUMNS,
            row * height / ORIENTATION_GRID_ROWS,
            last_column * width / ORIENTATION_GRID_COLUMNS,
            (row + 1) * height / ORIENTATION_GRID_ROWS,
        ))
    }

    fn dirty_pixels(self, width: usize, height: usize) -> u64 {
        let mut pixels = 0_u64;
        for row in 0..ORIENTATION_GRID_ROWS {
            let tile_height =
                (row + 1) * height / ORIENTATION_GRID_ROWS - row * height / ORIENTATION_GRID_ROWS;
            for column in 0..ORIENTATION_GRID_COLUMNS {
                if self.rows[row] & (1_u16 << column) == 0 {
                    continue;
                }
                let tile_width = (column + 1) * width / ORIENTATION_GRID_COLUMNS
                    - column * width / ORIENTATION_GRID_COLUMNS;
                pixels = pixels.saturating_add(
                    u64::try_from(tile_width.saturating_mul(tile_height)).unwrap_or(u64::MAX),
                );
            }
        }
        pixels
    }
}

impl OrientationTransitionRuntime {
    pub fn new(width: usize, height: usize) -> Self {
        Self::new_with_effect(width, height, OrientationTransitionEffect::CenterPixelZoom)
    }

    pub fn new_with_effect(
        width: usize,
        height: usize,
        effect: OrientationTransitionEffect,
    ) -> Self {
        let len = width.saturating_mul(height);
        Self {
            width,
            height,
            from: ScreenOrientation::Normal,
            to: ScreenOrientation::Normal,
            started_at: Instant::now(),
            duration: ORIENTATION_WAVE_TOTAL_DURATION,
            effect,
            source: vec![Rgb565Pixel(0); len],
            destination: vec![Rgb565Pixel(0); len],
            destination_ready: false,
            active: false,
            completion: None,
            last_render_stats: OrientationTransitionRenderStats::default(),
            previous_levels: [0; ORIENTATION_TILE_COUNT],
            previous_levels_valid: false,
            previous_revealing: false,
        }
    }

    pub fn start(
        &mut self,
        from: ScreenOrientation,
        to: ScreenOrientation,
        source: &[Rgb565Pixel],
        now: Instant,
        reduce_motion: bool,
    ) -> bool {
        if reduce_motion || from == to || source.len() != self.source.len() {
            self.active = false;
            self.completion = Some(OrientationTransitionCompletion { from, to });
            return false;
        }
        self.from = from;
        self.to = to;
        self.started_at = now;
        self.duration = ORIENTATION_WAVE_TOTAL_DURATION;
        self.source.copy_from_slice(source);
        self.destination.fill(Rgb565Pixel(0));
        self.destination_ready = false;
        self.active = true;
        self.completion = None;
        self.last_render_stats = OrientationTransitionRenderStats::default();
        self.previous_levels_valid = false;
        self.previous_revealing = false;
        true
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn capture_destination(&mut self, pixels: &[Rgb565Pixel]) -> bool {
        if !self.active || pixels.len() != self.destination.len() {
            return false;
        }
        if self.destination_ready {
            return true;
        }
        self.destination.copy_from_slice(pixels);
        self.destination_ready = true;
        true
    }

    pub fn destination_ready(&self) -> bool {
        self.destination_ready
    }

    pub fn cancel(&mut self) -> bool {
        let was_active = self.active;
        self.active = false;
        self.destination_ready = false;
        self.completion = None;
        self.previous_levels_valid = false;
        was_active
    }

    pub const fn from(&self) -> ScreenOrientation {
        self.from
    }

    pub const fn effect(&self) -> OrientationTransitionEffect {
        self.effect
    }

    pub fn set_effect(&mut self, effect: OrientationTransitionEffect) -> bool {
        if self.active {
            return false;
        }
        self.effect = effect;
        true
    }

    pub const fn to(&self) -> ScreenOrientation {
        self.to
    }

    pub const fn last_render_stats(&self) -> OrientationTransitionRenderStats {
        self.last_render_stats
    }

    pub fn render_into(
        &mut self,
        output: &mut [Rgb565Pixel],
        now: Instant,
    ) -> Option<(
        bool,
        OrientationTransitionRenderStats,
        OrientationTransitionDamage,
    )> {
        if !self.active || output.len() != self.source.len() {
            return None;
        }
        if !self.destination_ready {
            output.copy_from_slice(&self.source);
            self.last_render_stats = OrientationTransitionRenderStats::default();
            return Some((
                false,
                self.last_render_stats,
                OrientationTransitionDamage::full(),
            ));
        }
        let render_started = Instant::now();
        let elapsed = now
            .saturating_duration_since(self.started_at)
            .min(self.duration);
        let fill_started = Instant::now();
        let fill_pmu = mister_magik_perf_events::sampled_span(orientation_pmu_label(
            self.effect,
            self.from,
            self.to,
            OrientationPmuPhase::Fill,
        ));
        drop(fill_pmu);
        let fill_us = elapsed_us(fill_started);
        let map_started = Instant::now();
        let map_pmu = mister_magik_perf_events::sampled_span(orientation_pmu_label(
            self.effect,
            self.from,
            self.to,
            OrientationPmuPhase::Map,
        ));
        drop(map_pmu);
        let map_us = elapsed_us(map_started);
        let crossfade_started = Instant::now();
        let crossfade_pmu = mister_magik_perf_events::sampled_span(orientation_pmu_label(
            self.effect,
            self.from,
            self.to,
            OrientationPmuPhase::Crossfade,
        ));
        let (frame, levels, revealing) =
            orientation_wave_state(self.effect, &self.source, &self.destination, elapsed);
        let done = elapsed >= self.duration;
        let damage = if !self.previous_levels_valid || self.previous_revealing != revealing || done
        {
            OrientationTransitionDamage::full()
        } else {
            OrientationTransitionDamage::changed(&self.previous_levels, &levels)
        };
        match self.effect {
            OrientationTransitionEffect::BrightnessFade => render_brightness_wave_dirty(
                frame,
                output,
                self.width,
                self.height,
                &levels,
                damage,
            ),
            OrientationTransitionEffect::CenterPixelZoom => render_center_pixel_zoom_wave_dirty(
                frame,
                output,
                self.width,
                self.height,
                &levels,
                damage,
            ),
        };
        self.previous_levels = levels;
        self.previous_levels_valid = true;
        self.previous_revealing = revealing;
        let blended_pixels = damage.dirty_pixels(self.width, self.height);
        drop(crossfade_pmu);
        let crossfade_us = elapsed_us(crossfade_started);
        if done {
            self.active = false;
            self.completion = Some(OrientationTransitionCompletion {
                from: self.from,
                to: self.to,
            });
        }
        self.last_render_stats = OrientationTransitionRenderStats {
            fill_us,
            map_us,
            crossfade_us,
            total_us: elapsed_us(render_started),
            mapped_pixels: 0,
            blended_pixels,
            progress_ppm: duration_progress_ppm(elapsed, self.duration),
        };
        Some((done, self.last_render_stats, damage))
    }

    pub fn take_completion(&mut self) -> Option<OrientationTransitionCompletion> {
        self.completion.take()
    }
}

#[cfg(test)]
fn render_brightness_wave(
    source: &[Rgb565Pixel],
    destination: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    elapsed: Duration,
) -> u64 {
    let (frame, levels, _) = orientation_wave_state(
        OrientationTransitionEffect::BrightnessFade,
        source,
        destination,
        elapsed,
    );
    render_brightness_wave_dirty(
        frame,
        output,
        width,
        height,
        &levels,
        OrientationTransitionDamage::full(),
    );
    output.len().min(u64::MAX as usize) as u64
}

fn render_brightness_wave_dirty(
    frame: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    levels: &[u8; ORIENTATION_TILE_COUNT],
    damage: OrientationTransitionDamage,
) {
    if render_brightness_wave_neon(frame, output, width, height, levels, damage) {
        return;
    }
    render_brightness_wave_scalar(frame, output, width, height, levels, damage);
}

fn render_brightness_wave_scalar(
    frame: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    levels: &[u8; ORIENTATION_TILE_COUNT],
    damage: OrientationTransitionDamage,
) {
    for tile_row in 0..ORIENTATION_GRID_ROWS {
        let y0 = tile_row * height / ORIENTATION_GRID_ROWS;
        let y1 = (tile_row + 1) * height / ORIENTATION_GRID_ROWS;
        for tile_column in 0..ORIENTATION_GRID_COLUMNS {
            if damage.rows[tile_row] & (1_u16 << tile_column) == 0 {
                continue;
            }
            let x0 = tile_column * width / ORIENTATION_GRID_COLUMNS;
            let x1 = (tile_column + 1) * width / ORIENTATION_GRID_COLUMNS;
            let level = levels[tile_row * ORIENTATION_GRID_COLUMNS + tile_column];
            render_color_faded_tile(frame, output, width, x0, x1, y0, y1, level);
        }
    }
}

#[cfg(test)]
fn render_center_pixel_zoom_wave(
    source: &[Rgb565Pixel],
    destination: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    elapsed: Duration,
) -> u64 {
    let (frame, black_levels, _) = orientation_wave_state(
        OrientationTransitionEffect::CenterPixelZoom,
        source,
        destination,
        elapsed,
    );
    render_center_pixel_zoom_wave_dirty(
        frame,
        output,
        width,
        height,
        &black_levels,
        OrientationTransitionDamage::full(),
    );
    output.len().min(u64::MAX as usize) as u64
}

fn render_center_pixel_zoom_wave_dirty(
    frame: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    black_levels: &[u8; ORIENTATION_TILE_COUNT],
    damage: OrientationTransitionDamage,
) {
    if render_center_pixel_zoom_wave_neon(frame, output, width, height, black_levels, damage) {
        return;
    }
    render_center_pixel_zoom_wave_scalar(frame, output, width, height, black_levels, damage);
}

fn render_center_pixel_zoom_wave_scalar(
    frame: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    black_levels: &[u8; ORIENTATION_TILE_COUNT],
    damage: OrientationTransitionDamage,
) {
    for tile_row in 0..ORIENTATION_GRID_ROWS {
        let tile_y0 = tile_row * height / ORIENTATION_GRID_ROWS;
        let tile_y1 = (tile_row + 1) * height / ORIENTATION_GRID_ROWS;
        for tile_column in 0..ORIENTATION_GRID_COLUMNS {
            if damage.rows[tile_row] & (1_u16 << tile_column) == 0 {
                continue;
            }
            let tile_x0 = tile_column * width / ORIENTATION_GRID_COLUMNS;
            let tile_x1 = (tile_column + 1) * width / ORIENTATION_GRID_COLUMNS;
            copy_rect(frame, output, width, tile_x0, tile_x1, tile_y0, tile_y1);
            let black_level = black_levels[tile_row * ORIENTATION_GRID_COLUMNS + tile_column];
            if black_level == ORIENTATION_TILE_SKIP {
                continue;
            }
            let (x0, x1) = centered_span(tile_x0, tile_x1, black_level);
            let (y0, y1) = centered_span(tile_y0, tile_y1, black_level);
            fill_transition_color_rect(output, width, x0, x1, y0, y1);
        }
    }
}

fn orientation_wave_state<'a>(
    effect: OrientationTransitionEffect,
    source: &'a [Rgb565Pixel],
    destination: &'a [Rgb565Pixel],
    elapsed: Duration,
) -> (&'a [Rgb565Pixel], [u8; ORIENTATION_TILE_COUNT], bool) {
    let revealing = elapsed >= ORIENTATION_WAVE_PHASE_DURATION;
    let (frame, phase_elapsed_us) = if revealing {
        (
            destination,
            duration_us(elapsed.saturating_sub(ORIENTATION_WAVE_PHASE_DURATION)),
        )
    } else {
        (source, duration_us(elapsed))
    };
    let initial = if effect == OrientationTransitionEffect::CenterPixelZoom {
        ORIENTATION_TILE_SKIP
    } else {
        0
    };
    let mut levels = [initial; ORIENTATION_TILE_COUNT];
    for tile_row in 0..ORIENTATION_GRID_ROWS {
        for tile_column in 0..ORIENTATION_GRID_COLUMNS {
            let eased = orientation_tile_eased_level(phase_elapsed_us, tile_row, tile_column);
            let level = match (effect, eased, revealing) {
                (OrientationTransitionEffect::BrightnessFade, Some(level), true) => level,
                (OrientationTransitionEffect::BrightnessFade, Some(level), false) => {
                    RGB565_OPACITY_LEVELS.saturating_sub(level)
                }
                (OrientationTransitionEffect::BrightnessFade, None, true) => 0,
                (OrientationTransitionEffect::BrightnessFade, None, false) => RGB565_OPACITY_LEVELS,
                (OrientationTransitionEffect::CenterPixelZoom, Some(level), false) => level,
                (OrientationTransitionEffect::CenterPixelZoom, Some(level), true) => {
                    let level = RGB565_OPACITY_LEVELS.saturating_sub(level);
                    if level == 0 {
                        ORIENTATION_TILE_SKIP
                    } else {
                        level
                    }
                }
                (OrientationTransitionEffect::CenterPixelZoom, None, true) => RGB565_OPACITY_LEVELS,
                (OrientationTransitionEffect::CenterPixelZoom, None, false) => {
                    ORIENTATION_TILE_SKIP
                }
            };
            levels[tile_row * ORIENTATION_GRID_COLUMNS + tile_column] = level;
        }
    }
    (frame, levels, revealing)
}

#[cfg(all(target_os = "linux", target_arch = "arm"))]
fn orientation_neon_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MISTER_ORIENTATION_SIMD")
            .ok()
            .is_none_or(|value| !value.trim().eq_ignore_ascii_case("scalar"))
    })
}

#[cfg(not(all(target_os = "linux", target_arch = "arm")))]
const fn orientation_neon_enabled() -> bool {
    false
}

fn render_brightness_wave_neon(
    frame: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    levels: &[u8; ORIENTATION_TILE_COUNT],
    damage: OrientationTransitionDamage,
) -> bool {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    if orientation_neon_enabled()
        && frame.len() == output.len()
        && frame.len() == width.saturating_mul(height)
        && width >= ORIENTATION_GRID_COLUMNS
        && height >= ORIENTATION_GRID_ROWS
    {
        unsafe extern "C" {
            fn mister_magik_orientation_fade_neon(
                source: *const u16,
                output: *mut u16,
                width: usize,
                height: usize,
                levels: *const u8,
                dirty_rows: *const u16,
            );
        }
        // SAFETY: the slices are complete, distinct RGB565 planes with the
        // supplied geometry; the fixed level array covers every grid tile.
        unsafe {
            mister_magik_orientation_fade_neon(
                frame.as_ptr().cast(),
                output.as_mut_ptr().cast(),
                width,
                height,
                levels.as_ptr(),
                damage.rows.as_ptr(),
            );
        }
        return true;
    }
    let _ = (
        frame,
        output,
        width,
        height,
        levels,
        damage,
        orientation_neon_enabled(),
    );
    false
}

fn render_center_pixel_zoom_wave_neon(
    frame: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    black_levels: &[u8; ORIENTATION_TILE_COUNT],
    damage: OrientationTransitionDamage,
) -> bool {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    if orientation_neon_enabled()
        && frame.len() == output.len()
        && frame.len() == width.saturating_mul(height)
        && width >= ORIENTATION_GRID_COLUMNS
        && height >= ORIENTATION_GRID_ROWS
    {
        unsafe extern "C" {
            fn mister_magik_orientation_zoom_neon(
                source: *const u16,
                output: *mut u16,
                width: usize,
                height: usize,
                black_levels: *const u8,
                dirty_rows: *const u16,
            );
        }
        // SAFETY: the slices are complete, distinct RGB565 planes with the
        // supplied geometry; the fixed level array covers every grid tile.
        unsafe {
            mister_magik_orientation_zoom_neon(
                frame.as_ptr().cast(),
                output.as_mut_ptr().cast(),
                width,
                height,
                black_levels.as_ptr(),
                damage.rows.as_ptr(),
            );
        }
        return true;
    }
    let _ = (
        frame,
        output,
        width,
        height,
        black_levels,
        damage,
        orientation_neon_enabled(),
    );
    false
}

fn orientation_tile_eased_level(phase_elapsed_us: u64, row: usize, column: usize) -> Option<u8> {
    let delay_us = u64::try_from(row.saturating_add(column))
        .unwrap_or(u64::MAX)
        .saturating_mul(ORIENTATION_TILE_DELAY_US);
    if phase_elapsed_us < delay_us {
        return None;
    }
    let local_us = phase_elapsed_us
        .saturating_sub(delay_us)
        .min(ORIENTATION_TILE_FADE_US);
    let fade_squared = ORIENTATION_TILE_FADE_US.saturating_mul(ORIENTATION_TILE_FADE_US);
    let eased = local_us
        .saturating_mul(local_us)
        .saturating_mul(u64::from(RGB565_OPACITY_LEVELS))
        .saturating_add(fade_squared / 2)
        / fade_squared;
    Some(u8::try_from(eased).unwrap_or(RGB565_OPACITY_LEVELS))
}

fn centered_span(start: usize, end: usize, level: u8) -> (usize, usize) {
    let span = end.saturating_sub(start);
    if span == 0 {
        return (start, start);
    }
    let scaled = span
        .saturating_sub(1)
        .saturating_mul(usize::from(level))
        .saturating_add(usize::from(RGB565_OPACITY_LEVELS / 2))
        / usize::from(RGB565_OPACITY_LEVELS);
    let visible_span = 1usize.saturating_add(scaled).min(span);
    let center = start + span.saturating_sub(1) / 2;
    let centered_start = center.saturating_sub((visible_span - 1) / 2).max(start);
    (
        centered_start,
        centered_start.saturating_add(visible_span).min(end),
    )
}

fn fill_transition_color_rect(
    output: &mut [Rgb565Pixel],
    stride: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) {
    for y in y0..y1 {
        let start = y * stride + x0;
        let end = y * stride + x1;
        output[start..end].fill(ORIENTATION_TRANSITION_COLOR);
    }
}

fn copy_rect(
    source: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    stride: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) {
    for y in y0..y1 {
        let start = y * stride + x0;
        let end = y * stride + x1;
        output[start..end].copy_from_slice(&source[start..end]);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_color_faded_tile(
    frame: &[Rgb565Pixel],
    output: &mut [Rgb565Pixel],
    stride: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    opacity_level: u8,
) {
    for y in y0..y1 {
        let start = y * stride + x0;
        let end = y * stride + x1;
        if opacity_level == 0 {
            output[start..end].fill(ORIENTATION_TRANSITION_COLOR);
        } else if opacity_level >= RGB565_OPACITY_LEVELS {
            output[start..end].copy_from_slice(&frame[start..end]);
        } else {
            for (pixel, source) in output[start..end].iter_mut().zip(&frame[start..end]) {
                *pixel = fade_565_to_transition_color(*source, opacity_level);
            }
        }
    }
}

fn fade_565_to_transition_color(pixel: Rgb565Pixel, opacity_level: u8) -> Rgb565Pixel {
    let pixel = u32::from(pixel.0);
    let opacity = u32::from(opacity_level);
    let inverse_opacity = u32::from(RGB565_OPACITY_LEVELS) - opacity;
    let target = u32::from(ORIENTATION_TRANSITION_COLOR.0);
    let red = ((pixel >> 11) * opacity + (target >> 11) * inverse_opacity) >> 5;
    let green = (((pixel >> 5) & 63) * opacity + ((target >> 5) & 63) * inverse_opacity) >> 5;
    let blue = ((pixel & 31) * opacity + (target & 31) * inverse_opacity) >> 5;
    Rgb565Pixel(((red << 11) | (green << 5) | blue) as u16)
}

fn duration_progress_ppm(elapsed: Duration, duration: Duration) -> u32 {
    let duration_us = duration.as_micros().max(1);
    let progress = elapsed
        .as_micros()
        .saturating_mul(1_000_000)
        .saturating_add(duration_us / 2)
        / duration_us;
    u32::try_from(progress.min(1_000_000)).unwrap_or(1_000_000)
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

fn elapsed_us(started: Instant) -> u64 {
    duration_us(started.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_uses_the_supplied_delay_duration_and_quadratic_easing() {
        assert_eq!(orientation_tile_eased_level(0, 0, 0), Some(0));
        assert_eq!(orientation_tile_eased_level(75_000, 0, 0), Some(2));
        assert_eq!(orientation_tile_eased_level(150_000, 0, 0), Some(8));
        assert_eq!(orientation_tile_eased_level(225_000, 0, 0), Some(18));
        assert_eq!(
            orientation_tile_eased_level(300_000, 0, 0),
            Some(RGB565_OPACITY_LEVELS)
        );
        assert_eq!(orientation_tile_eased_level(919_999, 8, 15), None);
        assert_eq!(orientation_tile_eased_level(920_000, 8, 15), Some(0));
        assert_eq!(
            orientation_tile_eased_level(1_220_000, 8, 15),
            Some(RGB565_OPACITY_LEVELS)
        );
    }

    #[test]
    fn dirty_tiles_collapse_to_one_bounding_rectangle_per_row() {
        let previous = [0_u8; ORIENTATION_TILE_COUNT];
        let mut current = previous;
        current[2 * ORIENTATION_GRID_COLUMNS + 1] = 1;
        current[2 * ORIENTATION_GRID_COLUMNS + 7] = 1;
        current[2 * ORIENTATION_GRID_COLUMNS + 15] = 1;
        current[8 * ORIENTATION_GRID_COLUMNS + 4] = 1;

        let damage = OrientationTransitionDamage::changed(&previous, &current);
        assert_eq!(damage.rect_for_row(0, 1280, 720), None);
        assert_eq!(
            damage.rect_for_row(2, 1280, 720),
            Some((80, 160, 1280, 240))
        );
        assert_eq!(
            damage.rect_for_row(8, 1280, 720),
            Some((320, 640, 400, 720))
        );
        assert_eq!(damage.dirty_pixels(1280, 720), 4 * 80 * 80);
    }

    #[test]
    fn dirty_zoom_redraw_leaves_unchanged_tiles_alone() {
        let width = 160;
        let height = 90;
        let source = vec![Rgb565Pixel(0x07e0); width * height];
        let sentinel = Rgb565Pixel(0xf800);
        let mut output = vec![sentinel; width * height];
        let mut levels = [ORIENTATION_TILE_SKIP; ORIENTATION_TILE_COUNT];
        levels[0] = RGB565_OPACITY_LEVELS;
        let mut damage = OrientationTransitionDamage::default();
        damage.rows[0] = 1;

        render_center_pixel_zoom_wave_scalar(&source, &mut output, width, height, &levels, damage);

        assert_eq!(output[0], ORIENTATION_TRANSITION_COLOR);
        assert_eq!(output[9], ORIENTATION_TRANSITION_COLOR);
        assert_eq!(output[10], sentinel);
        assert_eq!(output[10 * width], sentinel);
    }

    #[test]
    fn wave_fades_old_frame_to_transition_color_then_reveals_new_frame() {
        let width = 16;
        let height = 9;
        let source = vec![Rgb565Pixel(0xf800); width * height];
        let destination = vec![Rgb565Pixel(0x07e0); width * height];
        let mut output = vec![Rgb565Pixel(0); width * height];

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::ZERO,
        );
        assert_eq!(output, source);

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::from_millis(300),
        );
        assert_eq!(output[0], ORIENTATION_TRANSITION_COLOR);
        assert_eq!(output[width * height - 1], source[width * height - 1]);

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::from_millis(1_220),
        );
        assert_eq!(output, vec![ORIENTATION_TRANSITION_COLOR; width * height]);

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            ORIENTATION_WAVE_PHASE_DURATION + Duration::from_millis(300),
        );
        assert_eq!(output[0], destination[0]);
        assert_eq!(output[width * height - 1], ORIENTATION_TRANSITION_COLOR);

        render_brightness_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            ORIENTATION_WAVE_TOTAL_DURATION,
        );
        assert_eq!(output, destination);
    }

    #[test]
    fn center_pixel_zoom_expands_transition_color_then_shrinks_over_destination() {
        let width = 160;
        let height = 90;
        let source = vec![Rgb565Pixel(0xf800); width * height];
        let destination = vec![Rgb565Pixel(0x07e0); width * height];
        let mut output = vec![Rgb565Pixel(0); width * height];
        let first_tile_center = 4 * width + 4;

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::ZERO,
        );
        assert_eq!(output[first_tile_center], ORIENTATION_TRANSITION_COLOR);
        assert_eq!(
            output
                .iter()
                .filter(|pixel| **pixel == ORIENTATION_TRANSITION_COLOR)
                .count(),
            1,
            "only the first tile's center pixel starts in the transition color"
        );

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::from_millis(300),
        );
        assert!((0..10).all(|y| {
            output[y * width..y * width + 10]
                .iter()
                .all(|pixel| *pixel == ORIENTATION_TRANSITION_COLOR)
        }));
        assert_eq!(output[width * height - 1], source[width * height - 1]);

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            Duration::from_millis(1_220),
        );
        assert!(
            output
                .iter()
                .all(|pixel| *pixel == ORIENTATION_TRANSITION_COLOR)
        );

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            ORIENTATION_WAVE_PHASE_DURATION + Duration::from_millis(300),
        );
        assert!((0..10).all(|y| {
            output[y * width..y * width + 10]
                .iter()
                .all(|pixel| *pixel == destination[0])
        }));
        assert_eq!(output[width * height - 1], ORIENTATION_TRANSITION_COLOR);

        render_center_pixel_zoom_wave(
            &source,
            &destination,
            &mut output,
            width,
            height,
            ORIENTATION_WAVE_TOTAL_DURATION,
        );
        assert_eq!(output, destination);
    }

    #[test]
    fn center_pixel_zoom_is_the_default_effect() {
        let runtime = OrientationTransitionRuntime::new(16, 9);
        assert_eq!(runtime.effect, OrientationTransitionEffect::CenterPixelZoom);
        let fade = OrientationTransitionRuntime::new_with_effect(
            16,
            9,
            OrientationTransitionEffect::BrightnessFade,
        );
        assert_eq!(fade.effect, OrientationTransitionEffect::BrightnessFade);
    }

    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    #[test]
    fn neon_kernels_are_pixel_identical_to_scalar_fallbacks() {
        let width = 160;
        let height = 90;
        let source = (0..width * height)
            .map(|index| Rgb565Pixel((index as u16).wrapping_mul(7919)))
            .collect::<Vec<_>>();
        let mut scalar = vec![Rgb565Pixel(0); source.len()];
        let mut neon = scalar.clone();
        let mut levels = [0_u8; ORIENTATION_TILE_COUNT];
        for (index, level) in levels.iter_mut().enumerate() {
            *level = (index % (usize::from(RGB565_OPACITY_LEVELS) + 1)) as u8;
        }
        let damage = OrientationTransitionDamage::full();
        render_brightness_wave_scalar(&source, &mut scalar, width, height, &levels, damage);
        assert!(render_brightness_wave_neon(
            &source, &mut neon, width, height, &levels, damage
        ));
        assert_eq!(neon, scalar);

        let mut black_levels = [ORIENTATION_TILE_SKIP; ORIENTATION_TILE_COUNT];
        for (index, level) in black_levels.iter_mut().enumerate() {
            if index % 5 != 0 {
                *level = (index % (usize::from(RGB565_OPACITY_LEVELS) + 1)) as u8;
            }
        }
        render_center_pixel_zoom_wave_scalar(
            &source,
            &mut scalar,
            width,
            height,
            &black_levels,
            damage,
        );
        assert!(render_center_pixel_zoom_wave_neon(
            &source,
            &mut neon,
            width,
            height,
            &black_levels,
            damage,
        ));
        assert_eq!(neon, scalar);
    }

    #[test]
    fn every_orientation_pair_uses_the_two_phase_wave_duration() {
        let start = Instant::now();
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(runtime.start(
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
            &[Rgb565Pixel(1); 12],
            start,
            false,
        ));
        assert_eq!(runtime.duration, ORIENTATION_WAVE_TOTAL_DURATION);
    }

    #[test]
    fn reduce_motion_completes_without_rendering_transition_frames() {
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(!runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &[Rgb565Pixel(1); 12],
            Instant::now(),
            true,
        ));
        assert!(!runtime.is_active());
        assert_eq!(
            runtime.take_completion(),
            Some(OrientationTransitionCompletion {
                from: ScreenOrientation::Normal,
                to: ScreenOrientation::MonitorClockwise,
            })
        );
    }

    #[test]
    fn completed_frame_is_exact_destination() {
        let start = Instant::now();
        let source = [Rgb565Pixel(1); 12];
        let destination = [Rgb565Pixel(2); 12];
        let mut output = [Rgb565Pixel(0); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        );
        assert!(runtime.capture_destination(&destination));
        let (done, _, _) = runtime
            .render_into(&mut output, start + ORIENTATION_WAVE_TOTAL_DURATION)
            .expect("transition frame");
        assert!(done);
        assert_eq!(output, destination);
    }

    #[test]
    fn render_stats_separate_mapping_and_crossfade_work() {
        let start = Instant::now();
        let source = [Rgb565Pixel(1); 12];
        let destination = [Rgb565Pixel(2); 12];
        let mut output = [Rgb565Pixel(0); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        assert!(runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        ));
        assert!(runtime.capture_destination(&destination));

        let (halfway_done, halfway, halfway_damage) = runtime
            .render_into(&mut output, start + ORIENTATION_WAVE_PHASE_DURATION)
            .expect("halfway transition frame");
        assert!(!halfway_done);
        assert_eq!(output, [Rgb565Pixel(0); 12]);
        assert_eq!(halfway.mapped_pixels, 0);
        assert_eq!(halfway.blended_pixels, 12);
        assert_eq!(halfway_damage, OrientationTransitionDamage::full());
        assert_eq!(halfway.progress_ppm, 500_000);

        let (final_done, final_stats, final_damage) = runtime
            .render_into(&mut output, start + ORIENTATION_WAVE_TOTAL_DURATION)
            .expect("final transition frame");
        assert!(final_done);
        assert_eq!(final_stats.blended_pixels, 12);
        assert_eq!(final_damage, OrientationTransitionDamage::full());
        assert_eq!(final_stats.progress_ppm, 1_000_000);
        assert!(final_stats.total_us >= final_stats.fill_us);
        assert!(final_stats.total_us >= final_stats.map_us);
        assert!(final_stats.total_us >= final_stats.crossfade_us);
    }

    #[test]
    fn transition_reuses_preallocated_buffers() {
        let start = Instant::now();
        let source = [Rgb565Pixel(1); 12];
        let destination = [Rgb565Pixel(2); 12];
        let mut output = [Rgb565Pixel(0); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);
        let source_ptr = runtime.source.as_ptr();
        let destination_ptr = runtime.destination.as_ptr();
        let capacities = (runtime.source.capacity(), runtime.destination.capacity());

        assert!(runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        ));
        assert!(runtime.capture_destination(&destination));
        let _ = runtime.render_into(&mut output, start + Duration::from_millis(750));
        let _ = runtime.render_into(&mut output, start + ORIENTATION_WAVE_TOTAL_DURATION);
        assert!(runtime.start(
            ScreenOrientation::MonitorClockwise,
            ScreenOrientation::MonitorCounterclockwise,
            &destination,
            start + ORIENTATION_WAVE_TOTAL_DURATION,
            false,
        ));
        assert!(runtime.capture_destination(&source));
        let _ = runtime.render_into(&mut output, start + ORIENTATION_WAVE_TOTAL_DURATION * 2);

        assert_eq!(runtime.source.as_ptr(), source_ptr);
        assert_eq!(runtime.destination.as_ptr(), destination_ptr);
        assert_eq!(
            (runtime.source.capacity(), runtime.destination.capacity()),
            capacities
        );
    }

    #[test]
    fn cancellation_clears_snapshot_playback_state() {
        let start = Instant::now();
        let source = [Rgb565Pixel(1); 12];
        let destination = [Rgb565Pixel(2); 12];
        let mut output = [Rgb565Pixel(0); 12];
        let mut runtime = OrientationTransitionRuntime::new(4, 3);

        assert!(runtime.start(
            ScreenOrientation::Normal,
            ScreenOrientation::MonitorClockwise,
            &source,
            start,
            false,
        ));
        assert!(runtime.capture_destination(&destination));
        assert!(runtime.cancel());
        assert!(!runtime.is_active());
        assert!(!runtime.destination_ready());
        assert!(
            runtime
                .render_into(&mut output, start + Duration::from_millis(750))
                .is_none()
        );
        assert!(runtime.take_completion().is_none());
        assert!(!runtime.cancel());
    }

    #[test]
    fn pmu_labels_cover_every_directed_leg_and_phase() {
        let phases = [
            OrientationPmuPhase::Destination,
            OrientationPmuPhase::Fill,
            OrientationPmuPhase::Map,
            OrientationPmuPhase::Crossfade,
            OrientationPmuPhase::CacheRestore,
        ];
        let mut labels = std::collections::BTreeSet::new();
        for from in ScreenOrientation::ALL {
            for to in ScreenOrientation::ALL {
                if from == to {
                    continue;
                }
                for phase in phases {
                    for effect in [
                        OrientationTransitionEffect::BrightnessFade,
                        OrientationTransitionEffect::CenterPixelZoom,
                    ] {
                        let label = orientation_pmu_label(effect, from, to, phase);
                        assert_ne!(label, "orientation.invalid");
                        assert!(labels.insert(label));
                    }
                }
            }
        }
        assert_eq!(labels.len(), 60);
    }
}
