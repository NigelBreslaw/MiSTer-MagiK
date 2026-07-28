// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Declarative, deterministic fireworks rendered directly into RGB565.

use serde::Deserialize;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Duration;

const SCHEMA: &str = "mister-magik-firework-v1";
const MAX_PARTICLES: usize = 98_304;
const MAX_TRAIL_SAMPLES: u8 = 16;
const OLED_PEONY: &str = include_str!("../assets/particles/fireworks/oled-peony.json");
const RECURSIVE_HALO: &str = include_str!("../assets/particles/fireworks/recursive-halo.json");
const SOLAR_CHRYSANTHEMUM: &str =
    include_str!("../assets/particles/fireworks/solar-chrysanthemum.json");

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FireworkRenderStats {
    pub particles: usize,
    pub visible: usize,
    pub pixel_writes: usize,
}

pub struct FireworkRenderer {
    width: usize,
    height: usize,
    seed: u64,
    show: CompiledShow,
}

impl FireworkRenderer {
    pub fn from_json(json: &str, width: usize, height: usize, seed: u64) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("firework viewport must be non-zero".into());
        }
        let spec: ShowSpec =
            serde_json::from_str(json).map_err(|error| format!("parse firework spec: {error}"))?;
        let show = CompiledShow::compile(spec)?;
        Ok(Self {
            width,
            height,
            seed,
            show,
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.show.id
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.show.label
    }

    #[must_use]
    pub const fn duration(&self) -> Duration {
        self.show.duration
    }

    pub fn render(
        &self,
        destination: &mut [Rgb565Pixel],
        elapsed: Duration,
    ) -> Result<FireworkRenderStats, String> {
        let expected = self.width.saturating_mul(self.height);
        if destination.len() != expected {
            return Err(format!(
                "firework destination has {} pixels, expected {expected}",
                destination.len()
            ));
        }
        destination.fill(Rgb565Pixel(0));
        let show_ms = elapsed.as_secs_f32() * 1000.0;
        let mut stats = FireworkRenderStats::default();
        for (emitter_index, emitter) in self.show.emitters.iter().enumerate() {
            for repeat in 0..emitter.repeats {
                let start_ms = emitter.start_ms + emitter.repeat_ms * repeat as f32;
                let local_seconds = (show_ms - start_ms) / 1000.0;
                if local_seconds < 0.0 {
                    continue;
                }
                for particle_index in 0..emitter.count {
                    stats.particles += 1;
                    let random = particle_seed(self.seed, emitter_index, repeat, particle_index);
                    let life_seconds = lerp(
                        emitter.life_seconds.0,
                        emitter.life_seconds.1,
                        random_unit(random.rotate_left(17)),
                    );
                    if local_seconds > life_seconds {
                        continue;
                    }
                    let age = local_seconds / life_seconds;
                    let intensity =
                        envelope(age) * twinkle_intensity(emitter.twinkle, random, show_ms);
                    if intensity <= 0.01 {
                        continue;
                    }
                    let color = emitter.palette[((random.rotate_left(31) as usize)
                        ^ particle_index)
                        % emitter.palette.len()];
                    let mut drew = false;
                    for sample in (0..emitter.trail.samples).rev() {
                        let sample_seconds =
                            local_seconds - f32::from(sample) * emitter.trail.step_seconds;
                        if sample_seconds < 0.0 {
                            continue;
                        }
                        let trail_age = (1.0
                            - f32::from(sample) / f32::from(emitter.trail.samples.max(1)))
                        .powi(2);
                        let point = emitter.position(
                            particle_index,
                            random,
                            sample_seconds,
                            self.width,
                            self.height,
                        );
                        let brush = if sample == 0 {
                            emitter.brush
                        } else {
                            Brush::Spark
                        };
                        if draw_brush(
                            destination,
                            self.width,
                            self.height,
                            point.0,
                            point.1,
                            color,
                            intensity * trail_age,
                            brush,
                            &mut stats.pixel_writes,
                        ) {
                            drew = true;
                        }
                    }
                    stats.visible += usize::from(drew);
                }
            }
        }
        Ok(stats)
    }
}

pub fn embedded_firework_json(id: &str) -> Option<&'static str> {
    match id.trim().to_ascii_lowercase().as_str() {
        "oled-peony" | "oled" => Some(OLED_PEONY),
        "recursive-halo" | "halo" => Some(RECURSIVE_HALO),
        "solar-chrysanthemum" | "solar" => Some(SOLAR_CHRYSANTHEMUM),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShowSpec {
    schema: String,
    id: String,
    label: String,
    duration_ms: u64,
    emitters: Vec<EmitterSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmitterSpec {
    start_ms: u64,
    #[serde(default)]
    repeat_ms: u64,
    #[serde(default = "one")]
    repeats: u16,
    origin: [f32; 2],
    count: usize,
    shape: Shape,
    #[serde(default = "default_direction")]
    direction_deg: f32,
    #[serde(default = "full_circle")]
    spread_deg: f32,
    #[serde(default)]
    rotation_deg: f32,
    #[serde(default = "one")]
    petals: u16,
    speed: [f32; 2],
    #[serde(default)]
    gravity: f32,
    #[serde(default)]
    drag: f32,
    life_ms: [u64; 2],
    palette: Vec<String>,
    brush: Brush,
    trail: TrailSpec,
    #[serde(default)]
    twinkle: f32,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Shape {
    Radial,
    Ring,
    Fan,
    Spiral,
    Comet,
    Rosette,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Brush {
    Spark,
    Glow,
    Flare,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrailSpec {
    samples: u8,
    step_ms: u16,
}

struct CompiledShow {
    id: String,
    label: String,
    duration: Duration,
    emitters: Vec<CompiledEmitter>,
}

struct CompiledEmitter {
    start_ms: f32,
    repeat_ms: f32,
    repeats: usize,
    origin: [f32; 2],
    count: usize,
    shape: Shape,
    direction: f32,
    spread: f32,
    rotation: f32,
    petals: usize,
    speed: (f32, f32),
    gravity: f32,
    drag: f32,
    life_seconds: (f32, f32),
    palette: Vec<Rgb888>,
    brush: Brush,
    trail: CompiledTrail,
    twinkle: f32,
}

#[derive(Clone, Copy)]
struct CompiledTrail {
    samples: u8,
    step_seconds: f32,
}

#[derive(Clone, Copy)]
struct Rgb888 {
    red: u8,
    green: u8,
    blue: u8,
}

impl CompiledShow {
    fn compile(spec: ShowSpec) -> Result<Self, String> {
        if spec.schema != SCHEMA {
            return Err(format!(
                "unsupported firework schema {:?}; expected {SCHEMA:?}",
                spec.schema
            ));
        }
        if spec.id.trim().is_empty() || spec.label.trim().is_empty() {
            return Err("firework id and label must not be empty".into());
        }
        if spec.duration_ms == 0 || spec.emitters.is_empty() {
            return Err("firework duration and emitters must be non-zero".into());
        }
        let mut particle_total = 0usize;
        let mut emitters = Vec::with_capacity(spec.emitters.len());
        for (index, emitter) in spec.emitters.into_iter().enumerate() {
            if !(0.0..=1.0).contains(&emitter.origin[0])
                || !(0.0..=1.0).contains(&emitter.origin[1])
            {
                return Err(format!("emitter {index} origin must be normalized"));
            }
            if emitter.count == 0 || emitter.repeats == 0 {
                return Err(format!(
                    "emitter {index} count and repeats must be non-zero"
                ));
            }
            particle_total = particle_total
                .saturating_add(emitter.count.saturating_mul(usize::from(emitter.repeats)));
            if particle_total > MAX_PARTICLES {
                return Err(format!(
                    "firework declares {particle_total} particles; maximum is {MAX_PARTICLES}"
                ));
            }
            if emitter.speed[0] < 0.0 || emitter.speed[0] > emitter.speed[1] {
                return Err(format!("emitter {index} speed range is invalid"));
            }
            if emitter.life_ms[0] == 0 || emitter.life_ms[0] > emitter.life_ms[1] {
                return Err(format!("emitter {index} lifetime range is invalid"));
            }
            if emitter.palette.is_empty() {
                return Err(format!("emitter {index} palette must not be empty"));
            }
            if emitter.trail.samples == 0 || emitter.trail.samples > MAX_TRAIL_SAMPLES {
                return Err(format!(
                    "emitter {index} trail samples must be between 1 and {MAX_TRAIL_SAMPLES}"
                ));
            }
            if !(0.0..=1.0).contains(&emitter.twinkle) {
                return Err(format!("emitter {index} twinkle must be between 0 and 1"));
            }
            let palette = emitter
                .palette
                .iter()
                .map(|color| parse_color(color))
                .collect::<Result<Vec<_>, _>>()?;
            emitters.push(CompiledEmitter {
                start_ms: emitter.start_ms as f32,
                repeat_ms: emitter.repeat_ms as f32,
                repeats: usize::from(emitter.repeats),
                origin: emitter.origin,
                count: emitter.count,
                shape: emitter.shape,
                direction: emitter.direction_deg.to_radians(),
                spread: emitter.spread_deg.to_radians(),
                rotation: emitter.rotation_deg.to_radians(),
                petals: usize::from(emitter.petals.max(1)),
                speed: (emitter.speed[0], emitter.speed[1]),
                gravity: emitter.gravity,
                drag: emitter.drag,
                life_seconds: (
                    emitter.life_ms[0] as f32 / 1000.0,
                    emitter.life_ms[1] as f32 / 1000.0,
                ),
                palette,
                brush: emitter.brush,
                trail: CompiledTrail {
                    samples: emitter.trail.samples,
                    step_seconds: f32::from(emitter.trail.step_ms) / 1000.0,
                },
                twinkle: emitter.twinkle,
            });
        }
        Ok(Self {
            id: spec.id,
            label: spec.label,
            duration: Duration::from_millis(spec.duration_ms),
            emitters,
        })
    }
}

impl CompiledEmitter {
    fn position(
        &self,
        particle_index: usize,
        random: u64,
        seconds: f32,
        width: usize,
        height: usize,
    ) -> (f32, f32) {
        let ordinal = particle_index as f32 / self.count.max(1) as f32;
        let jitter = random_signed(random.rotate_left(9)) * self.spread / self.count.max(1) as f32;
        let base_angle = match self.shape {
            Shape::Comet | Shape::Fan => self.direction + (ordinal - 0.5) * self.spread + jitter,
            Shape::Spiral => self.rotation + ordinal * self.spread * 2.5 + jitter,
            Shape::Rosette => {
                let petal = particle_index % self.petals;
                let within = particle_index / self.petals;
                self.rotation
                    + petal as f32 * std::f32::consts::TAU / self.petals as f32
                    + random_signed(random.rotate_left(23)) * 0.12
                    + within as f32 * 0.001
            }
            Shape::Radial | Shape::Ring => self.rotation + ordinal * self.spread + jitter,
        };
        let shape_speed = match self.shape {
            Shape::Ring => (self.speed.0 + self.speed.1) * 0.5,
            Shape::Rosette => {
                let petal_phase = (ordinal * self.petals as f32 * std::f32::consts::TAU)
                    .sin()
                    .abs();
                lerp(self.speed.0, self.speed.1, petal_phase)
            }
            _ => lerp(
                self.speed.0,
                self.speed.1,
                random_unit(random.rotate_left(41)),
            ),
        };
        let travel = if self.drag.abs() < f32::EPSILON {
            shape_speed * seconds
        } else {
            shape_speed * (1.0 - (-self.drag * seconds).exp()) / self.drag
        };
        let scale_x = width as f32 / 960.0;
        let scale_y = height as f32 / 540.0;
        (
            self.origin[0] * width as f32 + base_angle.cos() * travel * scale_x,
            self.origin[1] * height as f32
                + base_angle.sin() * travel * scale_y
                + 0.5 * self.gravity * seconds * seconds * scale_y,
        )
    }
}

fn draw_brush(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    color: Rgb888,
    intensity: f32,
    brush: Brush,
    pixel_writes: &mut usize,
) -> bool {
    let x = x.round() as isize;
    let y = y.round() as isize;
    let kernel: &[(isize, isize, f32)] = match brush {
        Brush::Spark => &[(0, 0, 1.0)],
        Brush::Glow => &[
            (-1, 0, 0.24),
            (0, -1, 0.24),
            (1, 0, 0.24),
            (0, 1, 0.24),
            (0, 0, 1.0),
        ],
        Brush::Flare => &[
            (-2, 0, 0.10),
            (0, -2, 0.10),
            (2, 0, 0.10),
            (0, 2, 0.10),
            (-1, 0, 0.35),
            (0, -1, 0.35),
            (1, 0, 0.35),
            (0, 1, 0.35),
            (0, 0, 1.0),
        ],
    };
    let mut drew = false;
    for &(dx, dy, weight) in kernel {
        let px = x + dx;
        let py = y + dy;
        if px < 0 || py < 0 || px >= width as isize || py >= height as isize {
            continue;
        }
        let offset = py as usize * width + px as usize;
        destination[offset] = additive_rgb565(destination[offset], color, intensity * weight);
        *pixel_writes += 1;
        drew = true;
    }
    drew
}

fn additive_rgb565(pixel: Rgb565Pixel, color: Rgb888, intensity: f32) -> Rgb565Pixel {
    let value = pixel.0;
    let red = u16::from((value >> 11) & 0x1f) * 255 / 31;
    let green = u16::from((value >> 5) & 0x3f) * 255 / 63;
    let blue = u16::from(value & 0x1f) * 255 / 31;
    let amount = intensity.clamp(0.0, 1.0);
    let red = red
        .saturating_add((f32::from(color.red) * amount) as u16)
        .min(255);
    let green = green
        .saturating_add((f32::from(color.green) * amount) as u16)
        .min(255);
    let blue = blue
        .saturating_add((f32::from(color.blue) * amount) as u16)
        .min(255);
    Rgb565Pixel(((red * 31 / 255) << 11) | ((green * 63 / 255) << 5) | (blue * 31 / 255))
}

fn parse_color(value: &str) -> Result<Rgb888, String> {
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| format!("firework color {value:?} must start with #"))?;
    if hex.len() != 6 {
        return Err(format!(
            "firework color {value:?} must contain six hex digits"
        ));
    }
    let color =
        u32::from_str_radix(hex, 16).map_err(|_| format!("firework color {value:?} is invalid"))?;
    Ok(Rgb888 {
        red: ((color >> 16) & 0xff) as u8,
        green: ((color >> 8) & 0xff) as u8,
        blue: (color & 0xff) as u8,
    })
}

fn envelope(age: f32) -> f32 {
    let ignition = (age / 0.035).clamp(0.0, 1.0);
    let decay = ((1.0 - age) / 0.38).clamp(0.0, 1.0);
    ignition * decay * decay
}

fn twinkle_intensity(amount: f32, random: u64, show_ms: f32) -> f32 {
    if amount <= 0.0 {
        return 1.0;
    }
    let phase = random_unit(random.rotate_left(7)) * std::f32::consts::TAU;
    let pulse = (phase + show_ms * 0.021).sin() * 0.5 + 0.5;
    1.0 - amount + pulse * amount
}

fn particle_seed(seed: u64, emitter: usize, repeat: usize, particle: usize) -> u64 {
    splitmix64(
        seed ^ (emitter as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (repeat as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9)
            ^ particle as u64,
    )
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn random_unit(value: u64) -> f32 {
    ((value >> 40) as u32) as f32 / 16_777_215.0
}

fn random_signed(value: u64) -> f32 {
    random_unit(value) * 2.0 - 1.0
}

fn lerp(start: f32, end: f32, amount: f32) -> f32 {
    start + (end - start) * amount
}

const fn one() -> u16 {
    1
}

fn default_direction() -> f32 {
    -90.0
}

fn full_circle() -> f32 {
    360.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_oled_peony_is_valid_and_deterministic() {
        let renderer = FireworkRenderer::from_json(OLED_PEONY, 96, 54, 7).unwrap();
        let mut first = vec![Rgb565Pixel(0); 96 * 54];
        let mut second = vec![Rgb565Pixel(0); 96 * 54];
        renderer
            .render(&mut first, Duration::from_millis(2000))
            .unwrap();
        renderer
            .render(&mut second, Duration::from_millis(2000))
            .unwrap();
        assert_eq!(first, second);
        assert!(first.iter().any(|pixel| pixel.0 != 0));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let invalid = OLED_PEONY.replacen(
            "\"duration_ms\": 5500,",
            "\"duration_ms\": 5500, \"surprise\": true,",
            1,
        );
        assert!(FireworkRenderer::from_json(&invalid, 96, 54, 7).is_err());
    }

    #[test]
    fn additive_rgb565_saturates_without_wrapping() {
        let white = Rgb888 {
            red: 255,
            green: 255,
            blue: 255,
        };
        assert_eq!(
            additive_rgb565(Rgb565Pixel(0xffff), white, 1.0),
            Rgb565Pixel(0xffff)
        );
    }
}
