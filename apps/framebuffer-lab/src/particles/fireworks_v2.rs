// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Archived deterministic Fireworks V2 renderer.

use crate::Rgb565Pixel;
use serde::Deserialize;
use std::time::Duration;

use super::recipes::embedded_firework_show;

const SCHEMA: &str = "mister-magik-firework-v2";
const MAX_PARTICLES: usize = 98_304;
const MAX_TRAIL_SAMPLES: u8 = 48;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FireworkV2RenderStats {
    pub particles: usize,
    pub visible: usize,
    pub pixel_writes: usize,
}

pub struct FireworkV2Renderer {
    width: usize,
    height: usize,
    seed: u64,
    show: CompiledShow,
}

impl FireworkV2Renderer {
    pub fn from_json(json: &str, width: usize, height: usize, seed: u64) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("firework V2 viewport must be non-zero".into());
        }
        let spec: ShowSpec = serde_json::from_str(json)
            .map_err(|error| format!("parse firework V2 spec: {error}"))?;
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
    ) -> Result<FireworkV2RenderStats, String> {
        let expected = self.width.saturating_mul(self.height);
        if destination.len() != expected {
            return Err(format!(
                "firework V2 destination has {} pixels, expected {expected}",
                destination.len()
            ));
        }
        destination.fill(Rgb565Pixel(0));
        let show_seconds = elapsed.as_secs_f32();
        let mut stats = FireworkV2RenderStats {
            particles: self.show.particle_count,
            ..FireworkV2RenderStats::default()
        };

        for (emitter_index, emitter) in self.show.emitters.iter().enumerate() {
            for repeat in 0..emitter.repeats {
                let start_seconds = emitter.start_seconds + emitter.repeat_seconds * repeat as f32;
                let emitter_seconds = show_seconds - start_seconds;
                if emitter_seconds < 0.0 {
                    continue;
                }
                for particle_index in 0..emitter.count {
                    let random = particle_seed(self.seed, emitter_index, repeat, particle_index);
                    let context = emitter.particle_context(particle_index, random);
                    let particle_seconds = emitter_seconds - context.birth_delay_seconds;
                    if particle_seconds < 0.0 {
                        continue;
                    }
                    let life_seconds = lerp(
                        emitter.life_seconds.0,
                        emitter.life_seconds.1,
                        random_unit(random.rotate_left(17)),
                    );
                    if particle_seconds > life_seconds {
                        continue;
                    }

                    let color = emitter.particle_color(&context, random);
                    let mut drew = false;
                    let mut previous = None;
                    for trail_sample in (0..emitter.trail.samples).rev() {
                        let sample_seconds =
                            particle_seconds - f32::from(trail_sample) * emitter.trail.step_seconds;
                        if sample_seconds < 0.0 {
                            continue;
                        }
                        let sample_age = sample_seconds / life_seconds;
                        let trail_progress =
                            1.0 - f32::from(trail_sample) / f32::from(emitter.trail.samples.max(1));
                        let brightness = emitter.envelope.evaluate(sample_age)
                            * twinkle_intensity(emitter.twinkle, random, show_seconds * 1000.0)
                            * context.brightness_scale
                            * trail_progress.powf(emitter.trail.fade_power);
                        if brightness <= 0.003 {
                            continue;
                        }
                        let point =
                            emitter.sample(&context, sample_seconds, self.width, self.height);
                        if let Some(older) = previous
                            && emitter.topology.segment_visible(
                                context.random,
                                trail_sample,
                                sample_age,
                            )
                        {
                            let width = lerp(
                                emitter.material.trail_width.0,
                                emitter.material.trail_width.1,
                                trail_progress,
                            ) * context.width_scale;
                            if draw_luminous_segment(
                                destination,
                                self.width,
                                self.height,
                                older,
                                point,
                                color,
                                brightness * emitter.material.trail_intensity,
                                width,
                                emitter.material.trail_white,
                                emitter.material.halo_radius,
                                emitter.material.halo_intensity,
                                emitter.material.blend,
                                &mut stats.pixel_writes,
                            ) {
                                drew = true;
                            }
                        }
                        previous = Some(point);

                        if emitter.material.bead_every > 0
                            && trail_sample % emitter.material.bead_every == 0
                            && draw_luminous_point(
                                destination,
                                self.width,
                                self.height,
                                point,
                                color,
                                brightness * emitter.material.bead_intensity,
                                emitter.material.trail_width.1 * context.width_scale,
                                emitter.material.trail_white,
                                emitter.material.halo_radius,
                                emitter.material.halo_intensity,
                                emitter.material.blend,
                                &mut stats.pixel_writes,
                            )
                        {
                            drew = true;
                        }
                    }

                    let age = particle_seconds / life_seconds;
                    let head_brightness = emitter.envelope.evaluate(age)
                        * twinkle_intensity(emitter.twinkle, random, show_seconds * 1000.0)
                        * context.brightness_scale
                        * emitter.material.head_intensity;
                    if head_brightness > 0.003 {
                        let point =
                            emitter.sample(&context, particle_seconds, self.width, self.height);
                        if draw_luminous_point(
                            destination,
                            self.width,
                            self.height,
                            point,
                            color,
                            head_brightness,
                            emitter.material.head_radius * context.width_scale,
                            emitter.material.head_white,
                            emitter.material.halo_radius,
                            emitter.material.halo_intensity,
                            emitter.material.blend,
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

pub fn embedded_firework_v2_json(id: &str) -> Option<String> {
    let canonical = match id.trim().to_ascii_lowercase().as_str() {
        "solar-chrysanthemum-v2" | "solar-v2" => "solar-chrysanthemum-v2",
        "recursive-halo-v2" | "halo-v2" => "recursive-halo-v2",
        "copper-willow-rain-v2" | "copper-v2" => "copper-willow-rain-v2",
        "phoenix-comet-v2" | "phoenix-v2" => "phoenix-comet-v2",
        "magnetic-flower-v2" | "magnetic-v2" => "magnetic-flower-v2",
        "oled-peony-v2" | "oled-v2" => "oled-peony-v2",
        _ => return None,
    };
    embedded_firework_show(canonical, SCHEMA)
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
    #[serde(default = "one_u16")]
    repeats: u16,
    origin: [f32; 2],
    count: usize,
    #[serde(default)]
    strands: u16,
    #[serde(default)]
    trajectory_seed: u64,
    #[serde(default = "default_strand_spread")]
    strand_spread_deg: f32,
    shape: Shape,
    #[serde(default = "default_direction")]
    direction_deg: f32,
    #[serde(default = "full_circle")]
    spread_deg: f32,
    #[serde(default)]
    rotation_deg: f32,
    speed: [f32; 2],
    #[serde(default)]
    gravity: f32,
    #[serde(default)]
    drag: f32,
    life_ms: [u64; 2],
    palette: Vec<String>,
    #[serde(default)]
    palette_mode: PaletteMode,
    #[serde(default)]
    tip_palette: Vec<String>,
    #[serde(default)]
    tip_fraction: f32,
    #[serde(default)]
    emission: EmissionSpec,
    #[serde(default)]
    motion: MotionSpec,
    #[serde(default)]
    envelope: EnvelopeSpec,
    #[serde(default)]
    topology: TopologySpec,
    material: MaterialSpec,
    trail: TrailSpec,
    #[serde(default)]
    twinkle: f32,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Shape {
    Point,
    Radial,
    Ring,
    Fan,
    Comet,
    Sphere,
    Hemisphere,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PaletteMode {
    #[default]
    Random,
    Strand,
    Depth,
    Speed,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum EmissionMode {
    #[default]
    Burst,
    Stream,
    TrailChildren,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EmissionSpec {
    mode: EmissionMode,
    duration_ms: u64,
    inherit_velocity: f32,
    scatter_speed: [f32; 2],
    scatter_deg: f32,
}

impl Default for EmissionSpec {
    fn default() -> Self {
        Self {
            mode: EmissionMode::Burst,
            duration_ms: 0,
            inherit_velocity: 0.0,
            scatter_speed: [0.0, 0.0],
            scatter_deg: 12.0,
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum MotionKind {
    #[default]
    Ballistic,
    Bezier,
    Vortex,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct MotionSpec {
    kind: MotionKind,
    turn_deg: [f32; 2],
    control1: Option<[f32; 2]>,
    control2: Option<[f32; 2]>,
    end: Option<[f32; 2]>,
    center: Option<[f32; 2]>,
    duration_ms: u64,
    radius_px: [f32; 2],
    turns: [f32; 2],
    path_spread_px: f32,
    time_power: f32,
    perspective: f32,
}

impl Default for MotionSpec {
    fn default() -> Self {
        Self {
            kind: MotionKind::Ballistic,
            turn_deg: [0.0, 0.0],
            control1: None,
            control2: None,
            end: None,
            center: None,
            duration_ms: 1000,
            radius_px: [0.0, 0.0],
            turns: [0.0, 0.0],
            path_spread_px: 0.0,
            time_power: 1.0,
            perspective: 0.18,
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EnvelopeSpec {
    attack: f32,
    hold: f32,
    decay_power: f32,
}

impl Default for EnvelopeSpec {
    fn default() -> Self {
        Self {
            attack: 0.025,
            hold: 0.18,
            decay_power: 2.0,
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TopologySpec {
    path_variation_px: f32,
    time_variation: f32,
    brightness_variation: f32,
    width_variation: f32,
    breakup_start: f32,
    trail_dropout: f32,
}

impl Default for TopologySpec {
    fn default() -> Self {
        Self {
            path_variation_px: 0.0,
            time_variation: 0.0,
            brightness_variation: 0.0,
            width_variation: 0.0,
            breakup_start: 1.0,
            trail_dropout: 0.0,
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum BlendMode {
    Add,
    #[default]
    Screen,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct MaterialSpec {
    head_radius: f32,
    head_intensity: f32,
    head_white: f32,
    trail_width: [f32; 2],
    trail_intensity: f32,
    trail_white: f32,
    halo_radius: f32,
    halo_intensity: f32,
    bead_every: u8,
    bead_intensity: f32,
    blend: BlendMode,
}

impl Default for MaterialSpec {
    fn default() -> Self {
        Self {
            head_radius: 1.35,
            head_intensity: 1.0,
            head_white: 0.35,
            trail_width: [0.45, 0.95],
            trail_intensity: 0.34,
            trail_white: 0.08,
            halo_radius: 1.8,
            halo_intensity: 0.09,
            bead_every: 0,
            bead_intensity: 0.55,
            blend: BlendMode::Screen,
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TrailSpec {
    samples: u8,
    step_ms: u16,
    fade_power: f32,
}

impl Default for TrailSpec {
    fn default() -> Self {
        Self {
            samples: 18,
            step_ms: 18,
            fade_power: 1.7,
        }
    }
}

struct CompiledShow {
    id: String,
    label: String,
    duration: Duration,
    particle_count: usize,
    emitters: Vec<CompiledEmitter>,
}

struct CompiledEmitter {
    start_seconds: f32,
    repeat_seconds: f32,
    repeats: usize,
    origin: [f32; 2],
    count: usize,
    strands: usize,
    trajectory_seed: u64,
    strand_spread: f32,
    shape: Shape,
    direction: f32,
    spread: f32,
    rotation: f32,
    speed: (f32, f32),
    gravity: f32,
    drag: f32,
    life_seconds: (f32, f32),
    palette: Vec<Rgb888>,
    palette_mode: PaletteMode,
    tip_palette: Vec<Rgb888>,
    tip_fraction: f32,
    emission: CompiledEmission,
    motion: CompiledMotion,
    envelope: CompiledEnvelope,
    topology: CompiledTopology,
    material: CompiledMaterial,
    trail: CompiledTrail,
    twinkle: f32,
}

#[derive(Clone, Copy)]
enum CompiledEmission {
    Burst,
    Stream {
        duration_seconds: f32,
    },
    TrailChildren {
        duration_seconds: f32,
        inherit_velocity: f32,
        scatter_speed: (f32, f32),
        scatter: f32,
    },
}

enum CompiledMotion {
    Ballistic {
        turn: (f32, f32),
        perspective: f32,
    },
    Bezier {
        control1: [f32; 2],
        control2: [f32; 2],
        end: [f32; 2],
        duration_seconds: f32,
        path_spread: f32,
        time_power: f32,
        perspective: f32,
    },
    Vortex {
        center: [f32; 2],
        duration_seconds: f32,
        radius: (f32, f32),
        turns: (f32, f32),
        path_spread: f32,
        time_power: f32,
        perspective: f32,
    },
}

#[derive(Clone, Copy)]
struct CompiledEnvelope {
    attack: f32,
    hold: f32,
    decay_power: f32,
}

#[derive(Clone, Copy)]
struct CompiledTopology {
    path_variation: f32,
    time_variation: f32,
    brightness_variation: f32,
    width_variation: f32,
    breakup_start: f32,
    trail_dropout: f32,
}

#[derive(Clone, Copy)]
struct CompiledMaterial {
    head_radius: f32,
    head_intensity: f32,
    head_white: f32,
    trail_width: (f32, f32),
    trail_intensity: f32,
    trail_white: f32,
    halo_radius: f32,
    halo_intensity: f32,
    bead_every: u8,
    bead_intensity: f32,
    blend: BlendMode,
}

#[derive(Clone, Copy)]
struct CompiledTrail {
    samples: u8,
    step_seconds: f32,
    fade_power: f32,
}

#[derive(Clone, Copy)]
struct ParticleContext {
    random: u64,
    trajectory_random: u64,
    trajectory_index: usize,
    birth_delay_seconds: f32,
    base_angle: f32,
    direction: Vec3,
    speed: f32,
    speed_normalized: f32,
    time_scale: f32,
    path_offset: f32,
    path_wobble: f32,
    brightness_scale: f32,
    width_scale: f32,
}

#[derive(Clone, Copy, Default)]
struct Vec2 {
    x: f32,
    y: f32,
}

impl Vec2 {
    fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    fn normalized(self) -> Self {
        let length = self.length();
        if length <= f32::EPSILON {
            Self { x: 1.0, y: 0.0 }
        } else {
            Self {
                x: self.x / length,
                y: self.y / length,
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Clone, Copy, Default)]
struct SamplePoint {
    x: f32,
    y: f32,
    depth_intensity: f32,
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
                "unsupported firework V2 schema {:?}; expected {SCHEMA:?}",
                spec.schema
            ));
        }
        if spec.id.trim().is_empty() || spec.label.trim().is_empty() {
            return Err("firework V2 id and label must not be empty".into());
        }
        if spec.duration_ms == 0 || spec.emitters.is_empty() {
            return Err("firework V2 duration and emitters must be non-zero".into());
        }

        let mut particle_total = 0usize;
        let mut emitters = Vec::with_capacity(spec.emitters.len());
        for (index, emitter) in spec.emitters.into_iter().enumerate() {
            validate_emitter(index, &emitter)?;
            particle_total = particle_total
                .saturating_add(emitter.count.saturating_mul(usize::from(emitter.repeats)));
            if particle_total > MAX_PARTICLES {
                return Err(format!(
                    "firework V2 declares {particle_total} particles; maximum is {MAX_PARTICLES}"
                ));
            }

            let palette = compile_palette(&emitter.palette)?;
            let tip_palette = compile_palette(&emitter.tip_palette)?;
            let emission = compile_emission(&emitter.emission);
            let motion = compile_motion(index, &emitter.motion)?;
            emitters.push(CompiledEmitter {
                start_seconds: emitter.start_ms as f32 / 1000.0,
                repeat_seconds: emitter.repeat_ms as f32 / 1000.0,
                repeats: usize::from(emitter.repeats),
                origin: emitter.origin,
                count: emitter.count,
                strands: usize::from(emitter.strands),
                trajectory_seed: emitter.trajectory_seed,
                strand_spread: emitter.strand_spread_deg.to_radians(),
                shape: emitter.shape,
                direction: emitter.direction_deg.to_radians(),
                spread: emitter.spread_deg.to_radians(),
                rotation: emitter.rotation_deg.to_radians(),
                speed: (emitter.speed[0], emitter.speed[1]),
                gravity: emitter.gravity,
                drag: emitter.drag,
                life_seconds: (
                    emitter.life_ms[0] as f32 / 1000.0,
                    emitter.life_ms[1] as f32 / 1000.0,
                ),
                palette,
                palette_mode: emitter.palette_mode,
                tip_palette,
                tip_fraction: emitter.tip_fraction,
                emission,
                motion,
                envelope: CompiledEnvelope {
                    attack: emitter.envelope.attack,
                    hold: emitter.envelope.hold,
                    decay_power: emitter.envelope.decay_power,
                },
                topology: CompiledTopology {
                    path_variation: emitter.topology.path_variation_px,
                    time_variation: emitter.topology.time_variation,
                    brightness_variation: emitter.topology.brightness_variation,
                    width_variation: emitter.topology.width_variation,
                    breakup_start: emitter.topology.breakup_start,
                    trail_dropout: emitter.topology.trail_dropout,
                },
                material: CompiledMaterial {
                    head_radius: emitter.material.head_radius,
                    head_intensity: emitter.material.head_intensity,
                    head_white: emitter.material.head_white,
                    trail_width: (
                        emitter.material.trail_width[0],
                        emitter.material.trail_width[1],
                    ),
                    trail_intensity: emitter.material.trail_intensity,
                    trail_white: emitter.material.trail_white,
                    halo_radius: emitter.material.halo_radius,
                    halo_intensity: emitter.material.halo_intensity,
                    bead_every: emitter.material.bead_every,
                    bead_intensity: emitter.material.bead_intensity,
                    blend: emitter.material.blend,
                },
                trail: CompiledTrail {
                    samples: emitter.trail.samples,
                    step_seconds: f32::from(emitter.trail.step_ms) / 1000.0,
                    fade_power: emitter.trail.fade_power,
                },
                twinkle: emitter.twinkle,
            });
        }
        Ok(Self {
            id: spec.id,
            label: spec.label,
            duration: Duration::from_millis(spec.duration_ms),
            particle_count: particle_total,
            emitters,
        })
    }
}

fn validate_emitter(index: usize, emitter: &EmitterSpec) -> Result<(), String> {
    if !normalized_point(emitter.origin) {
        return Err(format!(
            "firework V2 emitter {index} origin must be normalized"
        ));
    }
    if emitter.count == 0 || emitter.repeats == 0 {
        return Err(format!(
            "firework V2 emitter {index} count and repeats must be non-zero"
        ));
    }
    if usize::from(emitter.strands) > emitter.count {
        return Err(format!(
            "firework V2 emitter {index} strands must not exceed particle count"
        ));
    }
    if emitter.speed[0] < 0.0 || emitter.speed[0] > emitter.speed[1] {
        return Err(format!(
            "firework V2 emitter {index} speed range is invalid"
        ));
    }
    if emitter.life_ms[0] == 0 || emitter.life_ms[0] > emitter.life_ms[1] {
        return Err(format!(
            "firework V2 emitter {index} lifetime range is invalid"
        ));
    }
    if emitter.palette.is_empty() {
        return Err(format!(
            "firework V2 emitter {index} palette must not be empty"
        ));
    }
    if emitter.trail.samples == 0 || emitter.trail.samples > MAX_TRAIL_SAMPLES {
        return Err(format!(
            "firework V2 emitter {index} trail samples must be between 1 and {MAX_TRAIL_SAMPLES}"
        ));
    }
    if emitter.trail.step_ms == 0 || emitter.trail.fade_power <= 0.0 {
        return Err(format!(
            "firework V2 emitter {index} trail spacing and fade must be positive"
        ));
    }
    if !(0.0..=1.0).contains(&emitter.twinkle) || !(0.0..=1.0).contains(&emitter.tip_fraction) {
        return Err(format!(
            "firework V2 emitter {index} twinkle and tip fraction must be between 0 and 1"
        ));
    }
    if emitter.envelope.attack <= 0.0
        || emitter.envelope.attack > emitter.envelope.hold
        || emitter.envelope.hold >= 1.0
        || emitter.envelope.decay_power <= 0.0
    {
        return Err(format!("firework V2 emitter {index} envelope is invalid"));
    }
    if emitter.material.head_radius <= 0.0
        || emitter.material.head_radius > 6.0
        || emitter.material.trail_width[0] <= 0.0
        || emitter.material.trail_width[0] > emitter.material.trail_width[1]
        || emitter.material.trail_width[1] > 6.0
        || emitter.material.halo_radius < 0.0
        || emitter.material.halo_radius > 8.0
    {
        return Err(format!(
            "firework V2 emitter {index} material radii are invalid"
        ));
    }
    for intensity in [
        emitter.material.head_intensity,
        emitter.material.head_white,
        emitter.material.trail_intensity,
        emitter.material.trail_white,
        emitter.material.halo_intensity,
        emitter.material.bead_intensity,
    ] {
        if !(0.0..=2.0).contains(&intensity) {
            return Err(format!(
                "firework V2 emitter {index} material intensity is invalid"
            ));
        }
    }
    if emitter.emission.duration_ms > 10_000
        || !(0.0..=2.0).contains(&emitter.emission.inherit_velocity)
        || emitter.emission.scatter_speed[0] < 0.0
        || emitter.emission.scatter_speed[0] > emitter.emission.scatter_speed[1]
    {
        return Err(format!("firework V2 emitter {index} emission is invalid"));
    }
    if !matches!(emitter.emission.mode, EmissionMode::Burst) && emitter.emission.duration_ms == 0 {
        return Err(format!(
            "firework V2 emitter {index} streamed emission requires a duration"
        ));
    }
    if !(0.0..=0.65).contains(&emitter.motion.perspective)
        || emitter.motion.time_power <= 0.0
        || emitter.motion.path_spread_px < 0.0
    {
        return Err(format!("firework V2 emitter {index} motion is invalid"));
    }
    if emitter.topology.path_variation_px < 0.0
        || !(0.0..=0.5).contains(&emitter.topology.time_variation)
        || !(0.0..=0.8).contains(&emitter.topology.brightness_variation)
        || !(0.0..=0.8).contains(&emitter.topology.width_variation)
        || !(0.0..=1.0).contains(&emitter.topology.breakup_start)
        || !(0.0..=1.0).contains(&emitter.topology.trail_dropout)
    {
        return Err(format!("firework V2 emitter {index} topology is invalid"));
    }
    Ok(())
}

fn compile_emission(spec: &EmissionSpec) -> CompiledEmission {
    match spec.mode {
        EmissionMode::Burst => CompiledEmission::Burst,
        EmissionMode::Stream => CompiledEmission::Stream {
            duration_seconds: spec.duration_ms as f32 / 1000.0,
        },
        EmissionMode::TrailChildren => CompiledEmission::TrailChildren {
            duration_seconds: spec.duration_ms as f32 / 1000.0,
            inherit_velocity: spec.inherit_velocity,
            scatter_speed: (spec.scatter_speed[0], spec.scatter_speed[1]),
            scatter: spec.scatter_deg.to_radians(),
        },
    }
}

fn compile_motion(index: usize, spec: &MotionSpec) -> Result<CompiledMotion, String> {
    match spec.kind {
        MotionKind::Ballistic => Ok(CompiledMotion::Ballistic {
            turn: (spec.turn_deg[0].to_radians(), spec.turn_deg[1].to_radians()),
            perspective: spec.perspective,
        }),
        MotionKind::Bezier => {
            let control1 = spec
                .control1
                .filter(|point| normalized_point(*point))
                .ok_or_else(|| {
                    format!("firework V2 emitter {index} Bezier control1 must be normalized")
                })?;
            let control2 = spec
                .control2
                .filter(|point| normalized_point(*point))
                .ok_or_else(|| {
                    format!("firework V2 emitter {index} Bezier control2 must be normalized")
                })?;
            let end = spec
                .end
                .filter(|point| normalized_point(*point))
                .ok_or_else(|| {
                    format!("firework V2 emitter {index} Bezier end must be normalized")
                })?;
            if spec.duration_ms == 0 {
                return Err(format!(
                    "firework V2 emitter {index} Bezier duration must be non-zero"
                ));
            }
            Ok(CompiledMotion::Bezier {
                control1,
                control2,
                end,
                duration_seconds: spec.duration_ms as f32 / 1000.0,
                path_spread: spec.path_spread_px,
                time_power: spec.time_power,
                perspective: spec.perspective,
            })
        }
        MotionKind::Vortex => {
            let center = spec
                .center
                .filter(|point| normalized_point(*point))
                .ok_or_else(|| {
                    format!("firework V2 emitter {index} vortex center must be normalized")
                })?;
            if spec.duration_ms == 0 || spec.radius_px[0] < 0.0 || spec.radius_px[1] < 0.0 {
                return Err(format!(
                    "firework V2 emitter {index} vortex duration and radius are invalid"
                ));
            }
            Ok(CompiledMotion::Vortex {
                center,
                duration_seconds: spec.duration_ms as f32 / 1000.0,
                radius: (spec.radius_px[0], spec.radius_px[1]),
                turns: (spec.turns[0], spec.turns[1]),
                path_spread: spec.path_spread_px,
                time_power: spec.time_power,
                perspective: spec.perspective,
            })
        }
    }
}

fn compile_palette(values: &[String]) -> Result<Vec<Rgb888>, String> {
    values
        .iter()
        .map(|color| parse_color(color))
        .collect::<Result<Vec<_>, _>>()
}

impl CompiledEnvelope {
    fn evaluate(self, age: f32) -> f32 {
        if !(0.0..=1.0).contains(&age) {
            return 0.0;
        }
        let attack = (age / self.attack).clamp(0.0, 1.0);
        let decay = if age <= self.hold {
            1.0
        } else {
            ((1.0 - age) / (1.0 - self.hold))
                .clamp(0.0, 1.0)
                .powf(self.decay_power)
        };
        attack * decay
    }
}

impl CompiledTopology {
    fn segment_visible(self, random: u64, trail_sample: u8, age: f32) -> bool {
        if self.trail_dropout <= 0.0 || age <= self.breakup_start {
            return true;
        }
        let breakup =
            ((age - self.breakup_start) / (1.0 - self.breakup_start).max(0.001)).clamp(0.0, 1.0);
        let sample_random =
            splitmix64(random ^ u64::from(trail_sample).wrapping_mul(0xa076_1d64_78bd_642f));
        random_unit(sample_random) >= self.trail_dropout * breakup
    }
}

impl CompiledEmitter {
    fn particle_context(&self, particle_index: usize, random: u64) -> ParticleContext {
        let streamed = !matches!(self.emission, CompiledEmission::Burst);
        let trajectory_count = if self.strands > 0 {
            self.strands
        } else if streamed {
            1
        } else {
            self.count.max(1)
        };
        let trajectory_index = particle_index % trajectory_count;
        let child_index = particle_index / trajectory_count;
        let child_count = self.count.div_ceil(trajectory_count);
        let birth_phase = if child_count <= 1 {
            0.0
        } else {
            child_index as f32 / (child_count - 1) as f32
        };
        let birth_delay_seconds = match self.emission {
            CompiledEmission::Burst => 0.0,
            CompiledEmission::Stream { duration_seconds }
            | CompiledEmission::TrailChildren {
                duration_seconds, ..
            } => birth_phase * duration_seconds,
        };
        let family_seed = if self.trajectory_seed == 0 {
            self.count as u64
        } else {
            self.trajectory_seed
        };
        let trajectory_random =
            splitmix64((trajectory_index as u64).wrapping_mul(0xd6e8_feb8_6659_fd93) ^ family_seed);
        let ordinal = (trajectory_index as f32 + 0.5) / trajectory_count as f32;
        let angular_jitter = random_signed(trajectory_random.rotate_left(9)) * self.strand_spread;
        let (base_angle, direction) =
            self.initial_direction(ordinal, trajectory_random, angular_jitter);
        let speed_unit = random_unit(trajectory_random.rotate_left(41));
        let speed = if matches!(self.shape, Shape::Ring) {
            (self.speed.0 + self.speed.1) * 0.5
        } else {
            lerp(self.speed.0, self.speed.1, speed_unit)
        };
        let speed_normalized = if (self.speed.1 - self.speed.0).abs() <= f32::EPSILON {
            0.5
        } else {
            ((speed - self.speed.0) / (self.speed.1 - self.speed.0)).clamp(0.0, 1.0)
        };
        ParticleContext {
            random,
            trajectory_random,
            trajectory_index,
            birth_delay_seconds,
            base_angle,
            direction,
            speed,
            speed_normalized,
            time_scale: 1.0
                + random_signed(trajectory_random.rotate_left(5)) * self.topology.time_variation,
            path_offset: random_signed(trajectory_random.rotate_left(19))
                * self.topology.path_variation,
            path_wobble: random_signed(trajectory_random.rotate_left(27))
                * self.topology.path_variation,
            brightness_scale: 1.0
                + random_signed(trajectory_random.rotate_left(33))
                    * self.topology.brightness_variation,
            width_scale: 1.0
                + random_signed(trajectory_random.rotate_left(47)) * self.topology.width_variation,
        }
    }

    fn initial_direction(&self, ordinal: f32, random: u64, jitter: f32) -> (f32, Vec3) {
        match self.shape {
            Shape::Sphere | Shape::Hemisphere => {
                let depth_range = if matches!(self.shape, Shape::Hemisphere) {
                    (-0.15, 1.0)
                } else {
                    (-1.0, 1.0)
                };
                let z = lerp(depth_range.0, depth_range.1, ordinal);
                let radius = (1.0 - z * z).max(0.0).sqrt();
                let azimuth = self.rotation
                    + (self.trajectory_golden_phase(ordinal) * std::f32::consts::TAU)
                    + jitter;
                (
                    azimuth,
                    Vec3 {
                        x: azimuth.cos() * radius,
                        y: azimuth.sin() * radius,
                        z,
                    },
                )
            }
            Shape::Fan | Shape::Comet => {
                let angle = self.direction + (ordinal - 0.5) * self.spread + jitter;
                (
                    angle,
                    Vec3 {
                        x: angle.cos(),
                        y: angle.sin(),
                        z: random_signed(random.rotate_left(13)) * 0.25,
                    },
                )
            }
            Shape::Point => {
                let angle = self.direction + jitter;
                (
                    angle,
                    Vec3 {
                        x: angle.cos(),
                        y: angle.sin(),
                        z: random_signed(random.rotate_left(13)) * 0.15,
                    },
                )
            }
            Shape::Radial | Shape::Ring => {
                let angle = self.rotation + ordinal * self.spread + jitter;
                (
                    angle,
                    Vec3 {
                        x: angle.cos(),
                        y: angle.sin(),
                        z: random_signed(random.rotate_left(13)) * 0.35,
                    },
                )
            }
        }
    }

    fn trajectory_golden_phase(&self, ordinal: f32) -> f32 {
        (ordinal * self.count.max(1) as f32 * 0.618_034).fract()
    }

    fn particle_color(&self, context: &ParticleContext, random: u64) -> Rgb888 {
        let use_tip = !self.tip_palette.is_empty()
            && self.tip_fraction > 0.0
            && context.speed_normalized >= 1.0 - self.tip_fraction;
        let palette = if use_tip {
            &self.tip_palette
        } else {
            &self.palette
        };
        let index = match self.palette_mode {
            PaletteMode::Random => random.rotate_left(31) as usize % palette.len(),
            PaletteMode::Strand => context.trajectory_index % palette.len(),
            PaletteMode::Depth => (((context.direction.z + 1.0) * 0.5 * palette.len() as f32)
                as usize)
                .min(palette.len() - 1),
            PaletteMode::Speed => {
                ((context.speed_normalized * palette.len() as f32) as usize).min(palette.len() - 1)
            }
        };
        palette[index]
    }

    fn sample(
        &self,
        context: &ParticleContext,
        particle_seconds: f32,
        width: usize,
        height: usize,
    ) -> SamplePoint {
        match self.emission {
            CompiledEmission::TrailChildren {
                inherit_velocity,
                scatter_speed,
                scatter,
                ..
            } => self.sample_trail_child(
                context,
                particle_seconds,
                inherit_velocity,
                scatter_speed,
                scatter,
                width,
                height,
            ),
            CompiledEmission::Burst | CompiledEmission::Stream { .. } => {
                self.sample_primary(context, particle_seconds, width, height)
            }
        }
    }

    fn sample_primary(
        &self,
        context: &ParticleContext,
        seconds: f32,
        width: usize,
        height: usize,
    ) -> SamplePoint {
        match self.motion {
            CompiledMotion::Ballistic { turn, perspective } => {
                let seconds = seconds / context.time_scale.max(0.5);
                let turn_rate = lerp(
                    turn.0,
                    turn.1,
                    random_unit(context.trajectory_random.rotate_left(51)),
                );
                let velocity = Vec2 {
                    x: context.direction.x * context.speed,
                    y: context.direction.y * context.speed,
                };
                let displacement =
                    integrated_turning_displacement(velocity, turn_rate, seconds, self.drag);
                let depth_scale = (1.0 + context.direction.z * perspective).clamp(0.65, 1.35);
                SamplePoint {
                    x: self.origin[0] * width as f32
                        + displacement.x * depth_scale * width as f32 / 960.0,
                    y: self.origin[1] * height as f32
                        + (displacement.y * depth_scale + 0.5 * self.gravity * seconds * seconds)
                            * height as f32
                            / 540.0,
                    depth_intensity: depth_intensity(context.direction.z),
                }
            }
            CompiledMotion::Bezier {
                control1,
                control2,
                end,
                duration_seconds,
                path_spread,
                time_power,
                perspective,
            } => {
                let progress = (seconds / (duration_seconds * context.time_scale.max(0.5)))
                    .clamp(0.0, 1.0)
                    .powf(time_power);
                let point = cubic_bezier(self.origin, control1, control2, end, progress);
                let tangent = cubic_bezier_tangent(self.origin, control1, control2, end, progress)
                    .normalized();
                let normal = Vec2 {
                    x: -tangent.y,
                    y: tangent.x,
                };
                let spread = random_signed(context.random.rotate_left(23))
                    * path_spread
                    * (progress * std::f32::consts::PI).sin().abs()
                    + context.path_offset * (progress * std::f32::consts::PI).sin()
                    + context.path_wobble * 0.5 * (progress * std::f32::consts::TAU).sin();
                let depth = random_signed(context.trajectory_random.rotate_left(13));
                let depth_scale = (1.0 + depth * perspective).clamp(0.65, 1.35);
                SamplePoint {
                    x: point[0] * width as f32
                        + normal.x * spread * depth_scale * width as f32 / 960.0,
                    y: point[1] * height as f32
                        + (normal.y * spread * depth_scale
                            + 0.5 * self.gravity * seconds * seconds)
                            * height as f32
                            / 540.0,
                    depth_intensity: depth_intensity(depth),
                }
            }
            CompiledMotion::Vortex {
                center,
                duration_seconds,
                radius,
                turns,
                path_spread,
                time_power,
                perspective,
            } => {
                let progress = (seconds / (duration_seconds * context.time_scale.max(0.5)))
                    .clamp(0.0, 1.0)
                    .powf(time_power);
                let turns = lerp(
                    turns.0,
                    turns.1,
                    random_unit(context.trajectory_random.rotate_left(37)),
                );
                let angle = context.base_angle + turns * std::f32::consts::TAU * progress;
                let radius = lerp(radius.0, radius.1, progress)
                    + random_signed(context.random.rotate_left(23)) * path_spread
                    + context.path_offset
                    + context.path_wobble * 0.5 * (progress * std::f32::consts::TAU).sin();
                let depth = random_signed(context.trajectory_random.rotate_left(13));
                let depth_scale = (1.0 + depth * perspective).clamp(0.65, 1.35);
                SamplePoint {
                    x: center[0] * width as f32
                        + angle.cos() * radius * depth_scale * width as f32 / 960.0,
                    y: center[1] * height as f32
                        + (angle.sin() * radius * depth_scale
                            + 0.5 * self.gravity * seconds * seconds)
                            * height as f32
                            / 540.0,
                    depth_intensity: depth_intensity(depth),
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_trail_child(
        &self,
        context: &ParticleContext,
        child_seconds: f32,
        inherit_velocity: f32,
        scatter_speed: (f32, f32),
        scatter: f32,
        width: usize,
        height: usize,
    ) -> SamplePoint {
        let release_seconds = context.birth_delay_seconds;
        let parent = self.sample_primary(context, release_seconds, width, height);
        let ahead = self.sample_primary(context, release_seconds + 0.012, width, height);
        let tangent = Vec2 {
            x: ahead.x - parent.x,
            y: ahead.y - parent.y,
        }
        .normalized();
        let tangent_angle = tangent.y.atan2(tangent.x);
        let scatter_angle = tangent_angle + random_signed(context.random.rotate_left(29)) * scatter;
        let inherited_speed = context.speed * inherit_velocity;
        let child_scatter_speed = lerp(
            scatter_speed.0,
            scatter_speed.1,
            random_unit(context.random.rotate_left(43)),
        );
        let velocity = Vec2 {
            x: tangent.x * inherited_speed + scatter_angle.cos() * child_scatter_speed,
            y: tangent.y * inherited_speed + scatter_angle.sin() * child_scatter_speed,
        };
        let displacement = integrated_turning_displacement(velocity, 0.0, child_seconds, self.drag);
        SamplePoint {
            x: parent.x + displacement.x * width as f32 / 960.0,
            y: parent.y
                + (displacement.y + 0.5 * self.gravity * child_seconds * child_seconds)
                    * height as f32
                    / 540.0,
            depth_intensity: parent.depth_intensity,
        }
    }
}

fn integrated_turning_displacement(
    velocity: Vec2,
    turn_rate: f32,
    seconds: f32,
    drag: f32,
) -> Vec2 {
    let displacement = if turn_rate.abs() <= 0.000_1 {
        Vec2 {
            x: velocity.x * seconds,
            y: velocity.y * seconds,
        }
    } else {
        let phase = turn_rate * seconds;
        Vec2 {
            x: (velocity.x * phase.sin() + velocity.y * (phase.cos() - 1.0)) / turn_rate,
            y: (velocity.x * (1.0 - phase.cos()) + velocity.y * phase.sin()) / turn_rate,
        }
    };
    let drag_scale = if drag.abs() <= f32::EPSILON || seconds <= f32::EPSILON {
        1.0
    } else {
        (1.0 - (-drag * seconds).exp()) / (drag * seconds)
    };
    Vec2 {
        x: displacement.x * drag_scale,
        y: displacement.y * drag_scale,
    }
}

fn cubic_bezier(
    start: [f32; 2],
    control1: [f32; 2],
    control2: [f32; 2],
    end: [f32; 2],
    amount: f32,
) -> [f32; 2] {
    let inverse = 1.0 - amount;
    let w0 = inverse * inverse * inverse;
    let w1 = 3.0 * inverse * inverse * amount;
    let w2 = 3.0 * inverse * amount * amount;
    let w3 = amount * amount * amount;
    [
        start[0] * w0 + control1[0] * w1 + control2[0] * w2 + end[0] * w3,
        start[1] * w0 + control1[1] * w1 + control2[1] * w2 + end[1] * w3,
    ]
}

fn cubic_bezier_tangent(
    start: [f32; 2],
    control1: [f32; 2],
    control2: [f32; 2],
    end: [f32; 2],
    amount: f32,
) -> Vec2 {
    let inverse = 1.0 - amount;
    Vec2 {
        x: 3.0 * inverse * inverse * (control1[0] - start[0])
            + 6.0 * inverse * amount * (control2[0] - control1[0])
            + 3.0 * amount * amount * (end[0] - control2[0]),
        y: 3.0 * inverse * inverse * (control1[1] - start[1])
            + 6.0 * inverse * amount * (control2[1] - control1[1])
            + 3.0 * amount * amount * (end[1] - control2[1]),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_luminous_segment(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    start: SamplePoint,
    end: SamplePoint,
    color: Rgb888,
    intensity: f32,
    radius: f32,
    white: f32,
    halo_radius: f32,
    halo_intensity: f32,
    blend: BlendMode,
    pixel_writes: &mut usize,
) -> bool {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let steps = (dx.abs().max(dy.abs()) * 1.15).ceil().clamp(1.0, 96.0) as usize;
    let mut drew = false;
    for step in 0..=steps {
        let amount = step as f32 / steps as f32;
        let point = SamplePoint {
            x: lerp(start.x, end.x, amount),
            y: lerp(start.y, end.y, amount),
            depth_intensity: lerp(start.depth_intensity, end.depth_intensity, amount),
        };
        if draw_luminous_point(
            destination,
            width,
            height,
            point,
            color,
            intensity,
            radius,
            white,
            halo_radius,
            halo_intensity,
            blend,
            pixel_writes,
        ) {
            drew = true;
        }
    }
    drew
}

#[allow(clippy::too_many_arguments)]
fn draw_luminous_point(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    point: SamplePoint,
    color: Rgb888,
    intensity: f32,
    radius: f32,
    white: f32,
    halo_radius: f32,
    halo_intensity: f32,
    blend: BlendMode,
    pixel_writes: &mut usize,
) -> bool {
    let depth_intensity = point.depth_intensity.clamp(0.35, 1.2);
    let mut drew = false;
    if halo_radius > 0.0 && halo_intensity > 0.0 {
        drew |= draw_soft_disc(
            destination,
            width,
            height,
            point.x,
            point.y,
            color,
            intensity * halo_intensity * depth_intensity,
            radius + halo_radius,
            BlendMode::Screen,
            pixel_writes,
        );
    }
    let core = mix_white(color, white.clamp(0.0, 1.0));
    drew |= draw_soft_disc(
        destination,
        width,
        height,
        point.x,
        point.y,
        core,
        intensity * depth_intensity,
        radius,
        blend,
        pixel_writes,
    );
    drew
}

#[allow(clippy::too_many_arguments)]
fn draw_soft_disc(
    destination: &mut [Rgb565Pixel],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    color: Rgb888,
    intensity: f32,
    radius: f32,
    blend: BlendMode,
    pixel_writes: &mut usize,
) -> bool {
    let support = (radius + 0.75).ceil().clamp(1.0, 9.0) as isize;
    let center_x = x.floor() as isize;
    let center_y = y.floor() as isize;
    let mut drew = false;
    for offset_y in -support..=support {
        let pixel_y = center_y + offset_y;
        if pixel_y < 0 || pixel_y >= height as isize {
            continue;
        }
        for offset_x in -support..=support {
            let pixel_x = center_x + offset_x;
            if pixel_x < 0 || pixel_x >= width as isize {
                continue;
            }
            let dx = pixel_x as f32 + 0.5 - x;
            let dy = pixel_y as f32 + 0.5 - y;
            let distance = (dx * dx + dy * dy).sqrt();
            let weight = (1.0 - distance / (radius + 0.75))
                .clamp(0.0, 1.0)
                .powf(1.65);
            if weight <= 0.001 {
                continue;
            }
            let offset = pixel_y as usize * width + pixel_x as usize;
            destination[offset] =
                blend_rgb565(destination[offset], color, intensity * weight, blend);
            *pixel_writes += 1;
            drew = true;
        }
    }
    drew
}

fn blend_rgb565(
    pixel: Rgb565Pixel,
    color: Rgb888,
    intensity: f32,
    blend: BlendMode,
) -> Rgb565Pixel {
    match blend {
        BlendMode::Add => additive_rgb565(pixel, color, intensity),
        BlendMode::Screen => screen_rgb565(pixel, color, intensity),
    }
}

fn additive_rgb565(pixel: Rgb565Pixel, color: Rgb888, intensity: f32) -> Rgb565Pixel {
    let (red, green, blue) = unpack_rgb565(pixel);
    let amount = intensity.clamp(0.0, 2.0);
    pack_rgb565(
        red.saturating_add((f32::from(color.red) * amount) as u16)
            .min(255),
        green
            .saturating_add((f32::from(color.green) * amount) as u16)
            .min(255),
        blue.saturating_add((f32::from(color.blue) * amount) as u16)
            .min(255),
    )
}

fn screen_rgb565(pixel: Rgb565Pixel, color: Rgb888, intensity: f32) -> Rgb565Pixel {
    let (red, green, blue) = unpack_rgb565(pixel);
    let amount = intensity.clamp(0.0, 1.0);
    let source_red = f32::from(color.red) * amount;
    let source_green = f32::from(color.green) * amount;
    let source_blue = f32::from(color.blue) * amount;
    pack_rgb565(
        (255.0 - (255.0 - red as f32) * (255.0 - source_red) / 255.0) as u16,
        (255.0 - (255.0 - green as f32) * (255.0 - source_green) / 255.0) as u16,
        (255.0 - (255.0 - blue as f32) * (255.0 - source_blue) / 255.0) as u16,
    )
}

fn unpack_rgb565(pixel: Rgb565Pixel) -> (u16, u16, u16) {
    let value = pixel.0;
    (
        ((value >> 11) & 0x1f) * 255 / 31,
        ((value >> 5) & 0x3f) * 255 / 63,
        (value & 0x1f) * 255 / 31,
    )
}

fn pack_rgb565(red: u16, green: u16, blue: u16) -> Rgb565Pixel {
    let mut red5 = (red * 31 + 127) / 255;
    let green6 = (green * 63 + 127) / 255;
    let mut blue5 = (blue * 31 + 127) / 255;
    if red >= green && red > 0 && red5 == 0 && green6 > 0 {
        red5 = 1;
    }
    if blue >= green && blue > 0 && blue5 == 0 && green6 > 0 {
        blue5 = 1;
    }
    Rgb565Pixel((red5 << 11) | (green6 << 5) | blue5)
}

fn mix_white(color: Rgb888, amount: f32) -> Rgb888 {
    Rgb888 {
        red: lerp(f32::from(color.red), 255.0, amount) as u8,
        green: lerp(f32::from(color.green), 255.0, amount) as u8,
        blue: lerp(f32::from(color.blue), 255.0, amount) as u8,
    }
}

fn depth_intensity(depth: f32) -> f32 {
    lerp(0.58, 1.0, ((depth + 1.0) * 0.5).clamp(0.0, 1.0))
}

fn parse_color(value: &str) -> Result<Rgb888, String> {
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| format!("firework V2 color {value:?} must start with #"))?;
    if hex.len() != 6 {
        return Err(format!(
            "firework V2 color {value:?} must contain six hex digits"
        ));
    }
    let color = u32::from_str_radix(hex, 16)
        .map_err(|_| format!("firework V2 color {value:?} is invalid"))?;
    Ok(Rgb888 {
        red: ((color >> 16) & 0xff) as u8,
        green: ((color >> 8) & 0xff) as u8,
        blue: (color & 0xff) as u8,
    })
}

fn normalized_point(point: [f32; 2]) -> bool {
    (0.0..=1.0).contains(&point[0]) && (0.0..=1.0).contains(&point[1])
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

const fn one_u16() -> u16 {
    1
}

fn default_direction() -> f32 {
    -90.0
}

fn full_circle() -> f32 {
    360.0
}

fn default_strand_spread() -> f32 {
    0.8
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SHOW: &str = r##"{
      "schema": "mister-magik-firework-v2",
      "id": "test-shell-v2",
      "label": "TEST SHELL V2",
      "duration_ms": 4000,
      "emitters": [{
        "start_ms": 500,
        "origin": [0.5, 0.45],
        "count": 96,
        "strands": 24,
        "shape": "sphere",
        "speed": [120.0, 180.0],
        "gravity": 24.0,
        "drag": 0.12,
        "life_ms": [1800, 2400],
        "palette": ["#ff405f", "#30cfff"],
        "palette_mode": "depth",
        "motion": {
          "kind": "ballistic",
          "turn_deg": [-8.0, 8.0],
          "perspective": 0.22
        },
        "emission": {
          "mode": "trail-children",
          "duration_ms": 600,
          "inherit_velocity": 0.6,
          "scatter_speed": [12.0, 32.0],
          "scatter_deg": 18.0
        },
        "topology": {
          "path_variation_px": 5.0,
          "time_variation": 0.12,
          "brightness_variation": 0.2,
          "width_variation": 0.15,
          "breakup_start": 0.45,
          "trail_dropout": 0.35
        },
        "material": {
          "head_radius": 1.4,
          "head_white": 0.5,
          "trail_width": [0.4, 0.9],
          "bead_every": 4
        },
        "trail": {"samples": 20, "step_ms": 18}
      }]
    }"##;

    #[test]
    fn v2_shell_and_children_are_visible_and_deterministic() {
        let renderer = FireworkV2Renderer::from_json(TEST_SHOW, 96, 54, 7).unwrap();
        let mut first = vec![Rgb565Pixel(0); 96 * 54];
        let mut second = vec![Rgb565Pixel(0); 96 * 54];
        let first_stats = renderer
            .render(&mut first, Duration::from_millis(1600))
            .unwrap();
        renderer
            .render(&mut second, Duration::from_millis(1600))
            .unwrap();
        assert_eq!(first, second);
        assert!(first.iter().any(|pixel| pixel.0 != 0));
        assert!(first_stats.visible > 0);
    }

    #[test]
    fn v2_unknown_fields_are_rejected() {
        let invalid = TEST_SHOW.replacen(
            "\"duration_ms\": 4000,",
            "\"duration_ms\": 4000, \"surprise\": true,",
            1,
        );
        assert!(FireworkV2Renderer::from_json(&invalid, 96, 54, 7).is_err());
    }

    #[test]
    fn v2_unknown_topology_fields_are_rejected() {
        let invalid = TEST_SHOW.replacen(
            "\"trail_dropout\": 0.35",
            "\"trail_dropout\": 0.35, \"unbounded_noise\": true",
            1,
        );
        assert!(FireworkV2Renderer::from_json(&invalid, 96, 54, 7).is_err());
    }

    #[test]
    fn screen_blend_preserves_channel_identity() {
        let blue = Rgb888 {
            red: 0,
            green: 64,
            blue: 255,
        };
        let result = screen_rgb565(Rgb565Pixel(0), blue, 0.75);
        let (red, green, blue) = unpack_rgb565(result);
        assert_eq!(red, 0);
        assert!(blue > green);
    }

    #[test]
    fn low_intensity_warm_trail_does_not_quantize_to_green() {
        let gold = Rgb888 {
            red: 255,
            green: 155,
            blue: 28,
        };
        let result = screen_rgb565(Rgb565Pixel(0), gold, 0.02);
        let (red, green, blue) = unpack_rgb565(result);
        assert!(red >= green);
        assert!(green >= blue);
    }
}
