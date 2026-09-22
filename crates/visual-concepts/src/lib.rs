// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic, bounded RGB565 concepts. No device or application dependencies.
pub use mister_magik_framebuffer_scenes::{Rgb565Pixel as Pixel, Rgb565Rect as Rect};
use std::time::Duration;
mod aurora;
mod depth;
mod dissolve;
pub mod fixture;
mod light;
mod mirror;
mod point_cloud;
mod stars;
mod tunnel;

pub const EFFECTS: &[&str] = &[
    "texture-tunnel",
    "palette-aurora",
    "starfield-comets",
    "pixel-dissolve",
    "mirror-floor",
    "light-sweep",
    "depth-parallax",
    "point-cloud-morph",
    "diagnostic",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Default,
    Reduced,
}
impl Preset {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "default" => Ok(Self::Default),
            "reduced" => Ok(Self::Reduced),
            _ => Err(format!("unknown preset: {value}")),
        }
    }
    pub const fn choose(self, default: usize, reduced: usize) -> usize {
        match self {
            Self::Default => default,
            Self::Reduced => reduced,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Reduced => "reduced",
        }
    }
}
pub trait Effect {
    fn render(&mut self, elapsed: Duration, pixels: &mut [Pixel]) -> Result<Rect, String>;
    fn storage_bytes(&self) -> usize;
}
pub struct Scene {
    effect: Box<dyn Effect>,
    pixels: Vec<Pixel>,
    width: usize,
    height: usize,
    elapsed: Duration,
    first: bool,
}
impl Scene {
    pub fn new(name: &str, preset: Preset, width: usize, height: usize) -> Result<Self, String> {
        if width == 0 || height == 0 || width > 1366 || height > 768 {
            return Err("unsupported concept geometry".into());
        }
        let effect: Box<dyn Effect> = match name {
            "texture-tunnel" => Box::new(tunnel::new(preset, width, height)?),
            "palette-aurora" => Box::new(aurora::new(preset, width, height)?),
            "starfield-comets" => Box::new(stars::new(preset, width, height)?),
            "pixel-dissolve" => Box::new(dissolve::new(preset, width, height)?),
            "mirror-floor" => Box::new(mirror::new(preset, width, height)?),
            "light-sweep" => Box::new(light::new(preset, width, height)?),
            "depth-parallax" => Box::new(depth::new(preset, width, height)?),
            "point-cloud-morph" => Box::new(point_cloud::new(preset, width, height)?),
            "diagnostic" => Box::new(Diagnostic { width, height }),
            _ => return Err(format!("unknown concept: {name}")),
        };
        let _ = preset;
        Ok(Self {
            effect,
            pixels: vec![Pixel(0); width * height],
            width,
            height,
            elapsed: Duration::ZERO,
            first: true,
        })
    }
    pub fn advance(&mut self, interval: Duration) {
        self.elapsed += interval;
    }
    pub fn reset(&mut self) {
        self.elapsed = Duration::ZERO;
        self.first = true;
    }
    pub fn render(&mut self) -> Result<Rect, String> {
        let damage = self.effect.render(self.elapsed, &mut self.pixels)?;
        if self.first {
            self.first = false;
            Ok(full(self.width, self.height))
        } else {
            Ok(damage)
        }
    }
    pub fn pixels(&self) -> &[Pixel] {
        &self.pixels
    }
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }
    pub fn storage_bytes(&self) -> usize {
        self.pixels.capacity() * 2 + self.effect.storage_bytes()
    }
}
pub const fn full(width: usize, height: usize) -> Rect {
    Rect {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    }
}
pub const fn rgb(r: u8, g: u8, b: u8) -> Pixel {
    Pixel(((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3))
}
struct Diagnostic {
    width: usize,
    height: usize,
}
impl Effect for Diagnostic {
    fn render(&mut self, elapsed: Duration, pixels: &mut [Pixel]) -> Result<Rect, String> {
        pixels.fill(Pixel(0));
        let x = elapsed.as_millis() as usize / 8 % self.width;
        for y in 0..self.height.min(32) {
            pixels[y * self.width + x] = rgb(255, 64, 64);
        }
        Ok(full(self.width, self.height))
    }
    fn storage_bytes(&self) -> usize {
        0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_reset_restores_exposed_pixels() {
        for height in [540, 600] {
            let mut scene = Scene::new("diagnostic", Preset::Default, 960, height).unwrap();
            scene.render().unwrap();
            let initial = scene.pixels().to_vec();
            scene.advance(Duration::from_millis(100));
            scene.render().unwrap();
            assert_ne!(scene.pixels(), initial);
            scene.reset();
            scene.render().unwrap();
            assert_eq!(scene.pixels(), initial);
        }
    }
}
