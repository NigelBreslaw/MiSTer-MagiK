// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_fb::experiments::particles::showcase::{
    ParticleDemoKind, ParticleShowcaseConfig, ParticleShowcaseRenderer,
};
use slint::platform::software_renderer::Rgb565Pixel;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WIDTH: usize = 960;
const HEIGHT: usize = 540;
const SEED: u64 = 827_141_709_451;
const FRAME_US: u64 = 1_000_000 / 60;

fn main() -> Result<(), String> {
    let options = Options::parse(env::args().skip(1))?;
    let demo = ParticleDemoKind::parse(&options.demo)
        .ok_or_else(|| format!("unknown particle showcase demo {:?}", options.demo))?;
    let mut renderer = ParticleShowcaseRenderer::new(ParticleShowcaseConfig {
        width: WIDTH,
        height: HEIGHT,
        seed: SEED,
        initial_demo: demo,
    })?;
    if let Some(path) = options.family {
        renderer.load_family_file(&path)?;
    }
    renderer.configure_capture_hud(options.hud);
    let mut slots = [
        vec![Rgb565Pixel(0); WIDTH * HEIGHT],
        vec![Rgb565Pixel(0); WIDTH * HEIGHT],
    ];
    let target = Duration::from_millis(options.time_ms);
    let mut elapsed = Duration::ZERO;
    let mut frame = 0usize;
    loop {
        let slot = frame & 1;
        renderer.render(&mut slots[slot], (slot + 1) as u8, elapsed)?;
        if elapsed >= target {
            write_ppm(&options.output, &slots[slot])?;
            println!(
                "capture={} demo={} time_ms={} hud={} hash={:016x}",
                options.output.display(),
                demo.telemetry_label(),
                options.time_ms,
                options.hud,
                frame_hash(&slots[slot])
            );
            break;
        }
        elapsed = (elapsed + Duration::from_micros(FRAME_US)).min(target);
        frame += 1;
    }
    Ok(())
}

struct Options {
    demo: String,
    time_ms: u64,
    hud: bool,
    family: Option<PathBuf>,
    output: PathBuf,
}

impl Options {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut demo = None;
        let mut time_ms = None;
        let mut hud = false;
        let mut family = None;
        let mut output = None;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--demo" => demo = arguments.next(),
                "--time-ms" => {
                    let value = arguments.next().ok_or("--time-ms requires milliseconds")?;
                    time_ms = Some(
                        value
                            .parse()
                            .map_err(|_| format!("invalid time in milliseconds {value:?}"))?,
                    );
                }
                "--hud" => {
                    hud = match arguments.next().as_deref() {
                        Some("on") => true,
                        Some("off") => false,
                        _ => return Err("--hud requires on or off".into()),
                    };
                }
                "--family" => family = arguments.next().map(PathBuf::from),
                "--output" => output = arguments.next().map(PathBuf::from),
                "--help" | "-h" => {
                    return Err(
                        "usage: mister-magik-particle-preview --demo ID [--family FILE.json] --time-ms N --hud on|off --output FILE.ppm"
                            .into(),
                    );
                }
                other => return Err(format!("unknown preview argument {other:?}")),
            }
        }
        Ok(Self {
            demo: demo.ok_or("--demo is required")?,
            time_ms: time_ms.ok_or("--time-ms is required")?,
            hud,
            family,
            output: output.ok_or("--output is required")?,
        })
    }
}

fn write_ppm(path: &Path, pixels: &[Rgb565Pixel]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(format!("P6\n{WIDTH} {HEIGHT}\n255\n").as_bytes())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    for pixel in pixels {
        let red = ((pixel.0 >> 11) & 0x1f) as u8;
        let green = ((pixel.0 >> 5) & 0x3f) as u8;
        let blue = (pixel.0 & 0x1f) as u8;
        file.write_all(&[
            (u16::from(red) * 255 / 31) as u8,
            (u16::from(green) * 255 / 63) as u8,
            (u16::from(blue) * 255 / 31) as u8,
        ])
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    }
    Ok(())
}

fn frame_hash(pixels: &[Rgb565Pixel]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for pixel in pixels {
        for byte in pixel.0.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_capture_contract() {
        let options = Options::parse(
            [
                "--demo",
                "grid-flocking",
                "--time-ms",
                "15000",
                "--family",
                "procedural.json",
                "--hud",
                "off",
                "--output",
                "flock.ppm",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(options.demo, "grid-flocking");
        assert_eq!(options.time_ms, 15_000);
        assert_eq!(options.family, Some(PathBuf::from("procedural.json")));
        assert!(!options.hud);
        assert_eq!(options.output, PathBuf::from("flock.ppm"));
    }
}
