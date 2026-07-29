// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Allocation-free runtime for Trapcode Form-style particle compositions.
//!
//! Form owns its sources and modifier semantics, while the showcase owns the
//! RGB565 command buffers, rasterizer, dirty slots, and presentation contract.

use std::f32::consts::{PI, TAU};
use std::time::Duration;

pub const FORM_MAX_PARTICLES: usize = 98_304;
pub const FORM_MAX_SEGMENTS: usize = 32_768;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FormSceneKind {
    FractalGridTerrain,
    LayerMappedHologram,
    SphericalFieldObservatory,
    TwistedMultiFormCathedral,
    PointCloudMorphPassage,
}

impl FormSceneKind {
    pub const ALL: [Self; 5] = [
        Self::FractalGridTerrain,
        Self::LayerMappedHologram,
        Self::SphericalFieldObservatory,
        Self::TwistedMultiFormCathedral,
        Self::PointCloudMorphPassage,
    ];

    #[must_use]
    pub const fn count(self) -> usize {
        match self {
            Self::FractalGridTerrain => 49_152,
            Self::LayerMappedHologram => 40_960,
            Self::SphericalFieldObservatory => 32_768,
            Self::TwistedMultiFormCathedral => 65_536,
            Self::PointCloudMorphPassage => 24_576,
        }
    }

    #[must_use]
    pub const fn beat(self, seconds: f32) -> &'static str {
        match self {
            Self::FractalGridTerrain => {
                if seconds < 8.0 {
                    "ordered-grid"
                } else if seconds < 23.0 {
                    "curl-crest"
                } else {
                    "settle"
                }
            }
            Self::LayerMappedHologram => {
                if seconds < 8.0 {
                    "scan-in"
                } else if seconds < 23.0 {
                    "terraced-hold"
                } else {
                    "scan-out"
                }
            }
            Self::SphericalFieldObservatory => {
                if seconds < 9.0 {
                    "repel"
                } else if seconds < 22.0 {
                    "capture-orbit"
                } else {
                    "release"
                }
            }
            Self::TwistedMultiFormCathedral => {
                if seconds < 8.0 {
                    "assemble"
                } else if seconds < 23.0 {
                    "cathedral-pulse"
                } else {
                    "dissolve"
                }
            }
            Self::PointCloudMorphPassage => {
                if seconds < 5.0 {
                    "spacecraft-hold"
                } else if seconds < 13.0 {
                    "morph-passage"
                } else if seconds < 21.0 {
                    "manta-hold"
                } else {
                    "return-passage"
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FormPoint {
    pub x: f32,
    pub y: f32,
    pub style: u8,
    pub neighbor: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FormSegment {
    pub x0: i16,
    pub y0: i16,
    pub x1: i16,
    pub y1: i16,
    pub style: u8,
}

pub struct FormSceneRenderer {
    scene: Option<FormSceneKind>,
    seed: u64,
    rest_x: Vec<f32>,
    rest_y: Vec<f32>,
    rest_z: Vec<f32>,
    target_x: Vec<f32>,
    target_y: Vec<f32>,
    target_z: Vec<f32>,
    aux_x: Vec<f32>,
    aux_y: Vec<f32>,
    style: Vec<u8>,
    flags: Vec<u8>,
    points: Vec<FormPoint>,
    segments: Vec<FormSegment>,
}

impl FormSceneRenderer {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            scene: None,
            seed,
            rest_x: vec![0.0; FORM_MAX_PARTICLES],
            rest_y: vec![0.0; FORM_MAX_PARTICLES],
            rest_z: vec![0.0; FORM_MAX_PARTICLES],
            target_x: vec![0.0; FORM_MAX_PARTICLES],
            target_y: vec![0.0; FORM_MAX_PARTICLES],
            target_z: vec![0.0; FORM_MAX_PARTICLES],
            aux_x: vec![0.0; FORM_MAX_PARTICLES],
            aux_y: vec![0.0; FORM_MAX_PARTICLES],
            style: vec![0; FORM_MAX_PARTICLES],
            flags: vec![0; FORM_MAX_PARTICLES],
            points: Vec::with_capacity(FORM_MAX_PARTICLES),
            segments: Vec::with_capacity(FORM_MAX_SEGMENTS),
        }
    }

    pub fn reset(&mut self, scene: FormSceneKind) {
        self.scene = Some(scene);
        self.points.clear();
        self.segments.clear();
        match scene {
            FormSceneKind::FractalGridTerrain => self.initialize_terrain(),
            FormSceneKind::LayerMappedHologram => self.initialize_hologram(),
            FormSceneKind::SphericalFieldObservatory => self.initialize_observatory(),
            FormSceneKind::TwistedMultiFormCathedral => self.initialize_cathedral(),
            FormSceneKind::PointCloudMorphPassage => self.initialize_morph(),
        }
    }

    pub fn project(
        &mut self,
        scene: FormSceneKind,
        elapsed: Duration,
        width: usize,
        height: usize,
    ) -> (&[FormPoint], &[FormSegment]) {
        if self.scene != Some(scene) {
            self.reset(scene);
        }
        self.points.clear();
        self.segments.clear();
        let seconds = elapsed.as_secs_f32().rem_euclid(30.0);
        match scene {
            FormSceneKind::FractalGridTerrain => {
                self.project_terrain(seconds, width, height);
            }
            FormSceneKind::LayerMappedHologram => {
                self.project_hologram(seconds, width, height);
            }
            FormSceneKind::SphericalFieldObservatory => {
                self.project_observatory(seconds, width, height);
            }
            FormSceneKind::TwistedMultiFormCathedral => {
                self.project_cathedral(seconds, width, height);
            }
            FormSceneKind::PointCloudMorphPassage => {
                self.project_morph(seconds, width, height);
            }
        }
        (&self.points, &self.segments)
    }

    #[must_use]
    pub fn allocated_bytes(&self) -> usize {
        (self.rest_x.capacity()
            + self.rest_y.capacity()
            + self.rest_z.capacity()
            + self.target_x.capacity()
            + self.target_y.capacity()
            + self.target_z.capacity()
            + self.aux_x.capacity()
            + self.aux_y.capacity())
        .saturating_mul(std::mem::size_of::<f32>())
        .saturating_add(self.style.capacity())
        .saturating_add(self.flags.capacity())
        .saturating_add(
            self.points
                .capacity()
                .saturating_mul(std::mem::size_of::<FormPoint>()),
        )
        .saturating_add(
            self.segments
                .capacity()
                .saturating_mul(std::mem::size_of::<FormSegment>()),
        )
    }

    fn initialize_terrain(&mut self) {
        let width = 256usize;
        let height = 192usize;
        for index in 0..width * height {
            let ix = index % width;
            let iy = index / width;
            let u = ix as f32 / (width - 1) as f32;
            let v = iy as f32 / (height - 1) as f32;
            let x = (u - 0.5) * 690.0;
            let z = (v - 0.5) * 500.0;
            let broad = (x * 0.017).sin() * 30.0 + (z * 0.022).cos() * 25.0;
            let detail = ((x + z) * 0.041).sin() * 11.0 + ((x * 0.73 - z) * 0.067).cos() * 6.0;
            let crest = (-((u - 0.68).powi(2) * 24.0 + (v - 0.46).powi(2) * 8.0)).exp();
            self.rest_x[index] = x;
            self.rest_y[index] = broad + detail - crest * 115.0;
            self.rest_z[index] = z;
            self.target_x[index] = crest * (v - 0.5) * 130.0;
            self.target_y[index] = crest * 150.0;
            self.aux_x[index] = u;
            self.aux_y[index] = v;
            self.style[index] = ((u * 3.0 + v * 5.0 + crest * 3.0) as u8).min(7);
            self.flags[index] = u8::from((ix + iy) & 3 == 0);
        }
    }

    fn initialize_hologram(&mut self) {
        let count = FormSceneKind::LayerMappedHologram.count();
        let mut state = fold_seed(self.seed, 0x484f_4c4f);
        for index in 0..count {
            state = xorshift32(state);
            let selector = index % 10;
            let a = unit01(state.rotate_left(7));
            let b = unit01(state.rotate_left(17));
            let angle = a * TAU;
            let (x, y, z) = if selector < 6 {
                let sx = unit_signed(state) * 185.0;
                let sz = unit_signed(state.rotate_left(11)) * 130.0;
                let edge = if selector & 1 == 0 { 1.0 } else { -1.0 };
                (sx, 90.0 + edge * b * 72.0, sz)
            } else if selector < 9 {
                (
                    angle.cos() * (18.0 + b * 16.0),
                    25.0 - b * 190.0,
                    angle.sin() * (18.0 + b * 16.0),
                )
            } else {
                let radius = 38.0 + b * 38.0;
                (
                    angle.cos() * radius,
                    -185.0 + unit_signed(state.rotate_left(23)) * 42.0,
                    angle.sin() * radius,
                )
            };
            self.rest_x[index] = x;
            self.rest_y[index] = y;
            self.rest_z[index] = z;
            self.aux_x[index] = a;
            self.aux_y[index] = ((y + 230.0) / 400.0).clamp(0.0, 1.0);
            self.style[index] = (self.aux_y[index] * 7.99) as u8;
            self.flags[index] = u8::from(index & 7 == 0);
        }
    }

    fn initialize_observatory(&mut self) {
        let count = FormSceneKind::SphericalFieldObservatory.count();
        for index in 0..count {
            let trail = index & 127;
            let sample = index >> 7;
            let t = sample as f32 / 255.0;
            let phase = trail as f32 * (TAU / 128.0);
            let convergence = smootherstep(t);
            let wave = t * TAU * 1.35 + phase;
            self.rest_x[index] = (t - 0.5) * 720.0;
            self.rest_y[index] = wave.sin() * (125.0 - convergence * 70.0) + phase.cos() * 70.0;
            self.rest_z[index] = phase.sin() * (95.0 - convergence * 35.0) + wave.cos() * 32.0;
            self.aux_x[index] = phase.sin();
            self.aux_y[index] = phase.cos();
            self.style[index] = (2 + ((index >> 7) & 5)) as u8;
            self.flags[index] = u8::from(index & 31 == 0);
        }
    }

    fn initialize_cathedral(&mut self) {
        let count = FormSceneKind::TwistedMultiFormCathedral.count();
        for index in 0..count {
            let group = index & 3;
            let local = index / 4;
            let t = local as f32 / (count / 4) as f32;
            let angle = t * TAU * (3.0 + group as f32);
            let (x, y, z) = match group {
                0 => {
                    let side = if local & 1 == 0 { -1.0 } else { 1.0 };
                    (side * (80.0 + 115.0 * t), (t - 0.5) * 430.0, 0.0)
                }
                1 | 2 => {
                    let center = if group == 1 { -175.0 } else { 175.0 };
                    let polar = (t * PI).sin();
                    (
                        center + angle.cos() * polar * 105.0,
                        (t * PI).cos() * 155.0,
                        angle.sin() * polar * 105.0,
                    )
                }
                _ => (
                    angle.cos() * (70.0 + 150.0 * t),
                    (t - 0.5) * 360.0,
                    angle.sin() * (70.0 + 150.0 * t),
                ),
            };
            self.rest_x[index] = x;
            self.rest_y[index] = y;
            self.rest_z[index] = z;
            self.aux_x[index] = angle.sin();
            self.aux_y[index] = angle.cos();
            self.style[index] = (1 + group * 2 + (local & 1)) as u8;
            self.flags[index] = 1;
        }
    }

    fn initialize_morph(&mut self) {
        let count = FormSceneKind::PointCloudMorphPassage.count();
        for index in 0..count {
            let t = index as f32 / count as f32;
            let band = index % 96;
            let across = band as f32 / 95.0;
            let longitudinal = (index / 96) as f32 / ((count / 96) - 1) as f32;
            let ship_x = -245.0 + (longitudinal - 0.5) * 280.0;
            let ship_y = (across - 0.5) * (65.0 + longitudinal * 190.0);
            let ship_z = ((t * TAU * 17.0).sin()) * (18.0 + longitudinal * 35.0);
            self.rest_x[index] = ship_x;
            self.rest_y[index] = ship_y;
            self.rest_z[index] = ship_z;

            let wing = (across - 0.5) * 2.0;
            let manta_width = (1.0 - (longitudinal - 0.48).abs() * 1.6).max(0.08);
            self.target_x[index] = 245.0 + wing * manta_width * 265.0;
            self.target_y[index] = (longitudinal - 0.5) * 210.0 + wing.abs().powf(1.6) * 68.0;
            self.target_z[index] =
                wing.signum() * wing.abs().powi(2) * 55.0 + (longitudinal * TAU).sin() * 20.0;
            let phase = t * TAU * 7.0;
            self.aux_x[index] = phase.sin();
            self.aux_y[index] = phase.cos();
            self.style[index] = (2 + ((index >> 8) & 5)) as u8;
            self.flags[index] = u8::from(index & 255 == 0);
        }
    }

    fn project_terrain(&mut self, seconds: f32, width: usize, height: usize) {
        let count = FormSceneKind::FractalGridTerrain.count();
        let crest = smooth_envelope(seconds, 5.0, 15.0, 26.0);
        let angle = -0.22 + seconds * 0.012;
        let (sin, cos) = angle.sin_cos();
        for index in 0..count {
            let grid_x = index & 255;
            let grid_y = index >> 8;
            if grid_x & 1 != 0 || grid_y & 1 != 0 {
                continue;
            }
            let x = self.rest_x[index] + self.target_x[index] * crest;
            let y = self.rest_y[index] + self.target_y[index] * crest;
            let z = self.rest_z[index];
            let rx = x.mul_add(cos, z * sin);
            let rz = (-x).mul_add(sin, z * cos);
            let ry = y - 45.0;
            self.push_world(rx, ry, rz, 650.0, width, height, index);
        }
    }

    fn project_hologram(&mut self, seconds: f32, width: usize, height: usize) {
        let count = FormSceneKind::LayerMappedHologram.count();
        let reveal = if seconds < 5.0 {
            smootherstep(seconds / 5.0)
        } else if seconds < 24.0 {
            1.0
        } else {
            1.0 - smootherstep((seconds - 24.0) / 6.0) * 0.65
        };
        let angle = seconds * 0.17;
        let (sin, cos) = angle.sin_cos();
        for index in 0..count {
            if (index / 10) & 3 != 0 {
                continue;
            }
            let scan = (seconds * 0.25 + self.aux_y[index]).fract();
            if scan > reveal || (index & 3 == 0 && scan > 0.82) {
                continue;
            }
            let terrace = (self.aux_y[index] * 12.0).floor() * 1.8;
            let x = self.rest_x[index].mul_add(cos, self.rest_z[index] * sin);
            let z = (-self.rest_x[index]).mul_add(sin, self.rest_z[index] * cos);
            let y = self.rest_y[index] + terrace - 18.0;
            self.push_world(x, y, z, 620.0, width, height, index);
        }
    }

    fn project_observatory(&mut self, seconds: f32, width: usize, height: usize) {
        let count = FormSceneKind::SphericalFieldObservatory.count();
        let orbit = seconds * 0.16;
        let (sin, cos) = orbit.sin_cos();
        let field = smooth_envelope(seconds, 2.0, 14.0, 27.0);
        for index in (0..count).step_by(2) {
            let phase_sin = self.aux_x[index].mul_add(cos, self.aux_y[index] * sin);
            let phase_cos = self.aux_y[index].mul_add(cos, -self.aux_x[index] * sin);
            let convergence = ((self.rest_x[index] + 360.0) / 720.0).clamp(0.0, 1.0);
            let x = self.rest_x[index] + convergence * phase_cos * field * 48.0;
            let y = self.rest_y[index] + phase_sin * convergence * field * 72.0;
            let z = self.rest_z[index] + phase_cos * convergence * field * 45.0;
            self.push_world(x, y, z, 610.0, width, height, index);
        }
        let visible = self.points.len();
        for index in (0..visible).step_by(256) {
            self.push_segment_from_points(index, (index + 7) % visible);
        }
    }

    fn project_cathedral(&mut self, seconds: f32, width: usize, height: usize) {
        let count = FormSceneKind::TwistedMultiFormCathedral.count();
        let twist = 0.18 + seconds * 0.035;
        let pulse = 1.0 + (seconds * 1.3).sin() * 0.045;
        for index in 0..count {
            if (index / 4) & 7 != 0 {
                continue;
            }
            let local_angle = twist * (self.rest_y[index] / 220.0);
            let (sin, cos) = local_angle.sin_cos();
            let x = self.rest_x[index].mul_add(cos, self.rest_z[index] * sin) * pulse;
            let z = (-self.rest_x[index]).mul_add(sin, self.rest_z[index] * cos);
            let y = self.rest_y[index] * pulse;
            self.push_world(x, y, z, 660.0, width, height, index);
        }
        let visible = self.points.len();
        for index in (0..visible.saturating_sub(16)).step_by(16) {
            self.push_segment_from_points(index, index + 16);
        }
    }

    fn project_morph(&mut self, seconds: f32, width: usize, height: usize) {
        let count = FormSceneKind::PointCloudMorphPassage.count();
        let morph = if seconds < 5.0 {
            0.0
        } else if seconds < 13.0 {
            smootherstep((seconds - 5.0) / 8.0)
        } else if seconds < 21.0 {
            1.0
        } else if seconds < 29.0 {
            1.0 - smootherstep((seconds - 21.0) / 8.0)
        } else {
            0.0
        };
        let breakup = (morph * PI).sin() * 42.0;
        let angle = -0.28 + seconds * 0.025;
        let (sin, cos) = angle.sin_cos();
        for index in (0..count).step_by(2) {
            let assignment_rank = (index % 96) as f32 / 95.0;
            let local_morph = if morph <= 0.0 {
                0.0
            } else if morph >= 1.0 {
                1.0
            } else {
                smootherstep(((morph - assignment_rank * 0.7) / 0.3).clamp(0.0, 1.0))
            };
            let wobble = self.aux_x[index] * breakup;
            let x = lerp(self.rest_x[index], self.target_x[index], local_morph) + wobble;
            let y = lerp(self.rest_y[index], self.target_y[index], local_morph)
                + self.aux_y[index] * breakup * 0.38;
            let z = lerp(self.rest_z[index], self.target_z[index], local_morph)
                + self.aux_x[index] * breakup * 0.55;
            let rx = x.mul_add(cos, z * sin);
            let rz = (-x).mul_add(sin, z * cos);
            self.push_world(rx, y, rz, 640.0, width, height, index);
        }
        if (0.05..0.95).contains(&morph) {
            let visible = self.points.len();
            for index in (0..visible).step_by(128) {
                self.push_segment_from_points(index, (index + 512) % visible);
            }
        }
    }

    fn push_world(
        &mut self,
        x: f32,
        y: f32,
        z: f32,
        camera_z: f32,
        width: usize,
        height: usize,
        source_index: usize,
    ) {
        let depth = camera_z + z;
        if depth <= 32.0 {
            return;
        }
        let scale = camera_z / depth;
        let screen_x = width as f32 * 0.5 + x * scale;
        let screen_y = height as f32 * 0.5 + y * scale;
        if screen_x < 0.0 || screen_y < 0.0 || screen_x >= width as f32 || screen_y >= height as f32
        {
            return;
        }
        self.points.push(FormPoint {
            x: screen_x,
            y: screen_y,
            style: self.style[source_index].min(7),
            neighbor: self.flags[source_index] != 0 && screen_x + 1.0 < width as f32,
        });
    }

    fn push_segment_from_points(&mut self, first: usize, second: usize) {
        if self.segments.len() >= FORM_MAX_SEGMENTS {
            return;
        }
        let Some(a) = self.points.get(first) else {
            return;
        };
        let Some(b) = self.points.get(second) else {
            return;
        };
        self.segments.push(FormSegment {
            x0: a.x as i16,
            y0: a.y as i16,
            x1: b.x as i16,
            y1: b.y as i16,
            style: a.style.max(b.style).min(7),
        });
    }
}

fn fold_seed(seed: u64, salt: u64) -> u32 {
    let folded = seed ^ (seed >> 32) ^ salt;
    let folded = folded as u32;
    if folded == 0 { 0xa341_316c } else { folded }
}

fn xorshift32(mut state: u32) -> u32 {
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
}

fn unit01(value: u32) -> f32 {
    ((value >> 8) as f32) * (1.0 / 16_777_215.0)
}

fn unit_signed(value: u32) -> f32 {
    unit01(value).mul_add(2.0, -1.0)
}

fn lerp(first: f32, second: f32, amount: f32) -> f32 {
    first + (second - first) * amount
}

fn smootherstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

fn smooth_envelope(seconds: f32, enter: f32, peak: f32, exit: f32) -> f32 {
    if seconds <= enter {
        0.0
    } else if seconds < peak {
        smootherstep((seconds - enter) / (peak - enter).max(f32::EPSILON))
    } else if seconds < exit {
        1.0 - smootherstep((seconds - peak) / (exit - peak).max(f32::EPSILON))
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_budgets_fit_the_shared_showcase_contract() {
        for scene in FormSceneKind::ALL {
            assert!(scene.count() > 0);
            assert!(scene.count() <= FORM_MAX_PARTICLES);
        }
    }

    #[test]
    fn form_scenes_are_deterministic_nonempty_and_allocation_stable() {
        let mut first = FormSceneRenderer::new(827_141_709_451);
        let mut second = FormSceneRenderer::new(827_141_709_451);
        let allocated = first.allocated_bytes();
        for scene in FormSceneKind::ALL {
            let (first_points, first_segments) =
                first.project(scene, Duration::from_secs(15), 960, 540);
            let first_signature: Vec<_> = first_points
                .iter()
                .take(128)
                .map(|point| (point.x.to_bits(), point.y.to_bits(), point.style))
                .collect();
            let first_segment_count = first_segments.len();
            let (second_points, second_segments) =
                second.project(scene, Duration::from_secs(15), 960, 540);
            let second_signature: Vec<_> = second_points
                .iter()
                .take(128)
                .map(|point| (point.x.to_bits(), point.y.to_bits(), point.style))
                .collect();
            assert!(!first_signature.is_empty());
            assert_eq!(first_signature, second_signature);
            assert_eq!(first_segment_count, second_segments.len());
            assert!(first_segment_count <= FORM_MAX_SEGMENTS);
            assert_eq!(first.allocated_bytes(), allocated);
        }
    }

    #[test]
    fn morph_endpoints_hold_exactly() {
        assert_eq!(smootherstep(0.0), 0.0);
        assert_eq!(smootherstep(1.0), 1.0);
        assert_eq!(
            FormSceneKind::PointCloudMorphPassage.beat(4.0),
            "spacecraft-hold"
        );
        assert_eq!(
            FormSceneKind::PointCloudMorphPassage.beat(15.0),
            "manta-hold"
        );
    }
}
