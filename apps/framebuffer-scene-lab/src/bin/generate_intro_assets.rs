// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic Press Start 2P and launcher-mock particle asset compiler.

use std::fs;
use std::path::{Path, PathBuf};
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;

const FONT: &[u8] = include_bytes!("../../../mister/ui/fonts/PressStart2P-Regular.ttf");
const FONT_SHA256: &str = "8d0248e41694fdd875dbcde859ee1bae5982ecfdc6c7e5e451b48950d29ba95a";
const POINT_COUNT: usize = 40_960;
const LAYERS: usize = 9;
const PCLOUD_MAGIC: &[u8; 8] = b"PCLOUD1\0";
const PGROUP_MAGIC: &[u8; 8] = b"PGROUP1\0";
const MOCK_MAGIC: &[u8; 8] = b"RGB565M1";
const WIDTH: usize = 960;
const HEIGHT: usize = 540;
const TRACK_COUNTS: [usize; 6] = [8_192, 4_096, 8_192, 4_096, 8_192, 8_192];

#[derive(Clone, Copy)]
struct Point {
    x: i16,
    y: i16,
    z: i16,
    palette: u8,
    group: u8,
}

struct Glyph {
    width: usize,
    height: usize,
    alpha: Vec<u8>,
}

fn glyph(font: swash::FontRef<'_>, context: &mut ScaleContext, ch: char) -> Result<Glyph, String> {
    let id = font.charmap().map(ch);
    if id == 0 {
        return Err(format!("Press Start 2P does not contain {ch:?}"));
    }
    let mut scaler = context.builder(font).size(128.0).build();
    let image = Render::new(&[Source::Outline])
        .format(Format::Alpha)
        .render(&mut scaler, id)
        .ok_or_else(|| format!("cannot rasterize {ch:?}"))?;
    Ok(Glyph {
        width: image.placement.width as usize,
        height: image.placement.height as usize,
        alpha: image.data,
    })
}

fn quantize(value: f32, extent: f32) -> i16 {
    (value / extent * 32_767.0)
        .round()
        .clamp(-32_767.0, 32_767.0) as i16
}

fn text_track(
    glyph: &Glyph,
    origin_x: f32,
    group: u8,
    count: usize,
    partition: usize,
    partitions: usize,
) -> Result<Vec<Point>, String> {
    let mut candidates = Vec::new();
    for y in 0..glyph.height {
        for x in 0..glyph.width {
            if glyph.alpha[y * glyph.width + x] < 128 {
                continue;
            }
            for layer in 0..LAYERS {
                let ordinal = (y * glyph.width + x) * LAYERS + layer;
                if ordinal % partitions != partition {
                    continue;
                }
                let world_x = origin_x + x as f32;
                let world_y = 48.0 + y as f32;
                let world_z = (layer as f32 - 4.0) * 10.0;
                candidates.push(Point {
                    x: quantize(world_x, 480.0),
                    y: quantize(world_y, 220.0),
                    z: quantize(world_z, 96.0),
                    palette: ((layer + usize::from(group)) & 7) as u8,
                    group,
                });
            }
        }
    }
    candidates.sort_unstable_by_key(|point| {
        let x = u16::from_le_bytes(point.x.to_le_bytes()) as u64;
        let y = u16::from_le_bytes(point.y.to_le_bytes()) as u64;
        let z = u16::from_le_bytes(point.z.to_le_bytes()) as u64;
        x.wrapping_mul(0x9e37) ^ y.wrapping_mul(0x85eb) ^ z.wrapping_mul(0xc2b2)
    });
    candidates.dedup_by_key(|point| (point.x, point.y, point.z));
    if candidates.len() < count {
        return Err(format!(
            "glyph track {group} has {} unique samples, needs {count}",
            candidates.len()
        ));
    }
    candidates.truncate(count);
    Ok(candidates)
}

fn text_target(font: swash::FontRef<'_>, text: &str) -> Result<Vec<Point>, String> {
    let (letters, origins, partitions): (&[char], &[f32], &[(usize, usize)]) = match text {
        "MiSTer" => (
            &['M', 'i', 'S', 'T', 'e', 'r'],
            &[-360.0, -224.0, -128.0, 0.0, 128.0, 256.0],
            &[(0, 1); 6],
        ),
        "MagiK" => (
            &['M', 'a', 'g', 'i', 'K', 'K'],
            &[-360.0, -224.0, -96.0, 48.0, 128.0, 128.0],
            &[(0, 1), (0, 1), (0, 1), (0, 1), (0, 2), (1, 2)],
        ),
        _ => return Err(format!("unsupported text target {text:?}")),
    };
    let mut context = ScaleContext::new();
    let mut points = Vec::with_capacity(POINT_COUNT);
    for group in 0..letters.len() {
        let glyph = glyph(font, &mut context, letters[group])?;
        let (partition, partition_count) = partitions[group];
        points.extend(text_track(
            &glyph,
            origins[group],
            group as u8,
            TRACK_COUNTS[group],
            partition,
            partition_count,
        )?);
    }
    Ok(points)
}

fn encode_cloud(points: &[Point]) -> Vec<u8> {
    let bounds = [
        points.iter().map(|point| point.x).min().unwrap_or(0),
        points.iter().map(|point| point.x).max().unwrap_or(0),
        points.iter().map(|point| point.y).min().unwrap_or(0),
        points.iter().map(|point| point.y).max().unwrap_or(0),
        points.iter().map(|point| point.z).min().unwrap_or(0),
        points.iter().map(|point| point.z).max().unwrap_or(0),
    ];
    let mut bytes = Vec::with_capacity(28 + points.len() * 8);
    bytes.extend_from_slice(PCLOUD_MAGIC);
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&8_u16.to_le_bytes());
    bytes.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for bound in bounds {
        bytes.extend_from_slice(&bound.to_le_bytes());
    }
    for point in points {
        bytes.extend_from_slice(&point.x.to_le_bytes());
        bytes.extend_from_slice(&point.y.to_le_bytes());
        bytes.extend_from_slice(&point.z.to_le_bytes());
        bytes.push(point.palette);
        bytes.push(0);
    }
    bytes
}

fn encode_groups(points: &[Point]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16 + points.len());
    bytes.extend_from_slice(PGROUP_MAGIC);
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&(points.len() as u32).to_le_bytes());
    bytes.extend(points.iter().map(|point| point.group));
    bytes
}

fn rgb565(red: u8, green: u8, blue: u8) -> u16 {
    (u16::from(red) >> 3) << 11 | (u16::from(green) >> 2) << 5 | (u16::from(blue) >> 3)
}

fn fill_rect(pixels: &mut [u16], x: usize, y: usize, width: usize, height: usize, color: u16) {
    for row in y..(y + height).min(HEIGHT) {
        let start = row * WIDTH + x.min(WIDTH);
        let end = row * WIDTH + (x + width).min(WIDTH);
        pixels[start..end].fill(color);
    }
}

fn launcher_mock() -> (Vec<Point>, Vec<u8>) {
    let background = rgb565(3, 6, 12);
    let mut pixels = vec![background; WIDTH * HEIGHT];
    fill_rect(&mut pixels, 24, 20, 912, 54, rgb565(20, 42, 70));
    fill_rect(&mut pixels, 24, 92, 210, 392, rgb565(10, 22, 38));
    fill_rect(&mut pixels, 44, 132, 170, 48, rgb565(32, 170, 220));
    for row in 0..6 {
        fill_rect(&mut pixels, 44, 198 + row * 44, 146, 18, rgb565(42, 62, 82));
    }
    fill_rect(&mut pixels, 254, 92, 430, 392, rgb565(8, 18, 32));
    for row in 0..7 {
        let color = if row == 2 {
            rgb565(220, 70, 190)
        } else {
            rgb565(28, 52, 78)
        };
        fill_rect(&mut pixels, 278, 122 + row * 46, 380, 30, color);
    }
    fill_rect(&mut pixels, 704, 92, 232, 300, rgb565(18, 34, 54));
    fill_rect(&mut pixels, 724, 112, 192, 192, rgb565(45, 100, 126));
    fill_rect(&mut pixels, 704, 412, 232, 72, rgb565(15, 28, 46));
    fill_rect(&mut pixels, 24, 500, 912, 20, rgb565(22, 38, 58));

    let mut candidates = pixels
        .iter()
        .enumerate()
        .filter(|(_, pixel)| **pixel != background)
        .map(|(offset, pixel)| {
            let x = offset % WIDTH;
            let y = offset / WIDTH;
            (
                (x as u64).wrapping_mul(0x9e37) ^ (y as u64).wrapping_mul(0x85eb),
                Point {
                    x: quantize(x as f32 - 480.0, 480.0),
                    y: quantize(y as f32, 540.0),
                    z: 0,
                    palette: ((*pixel >> 8) & 7) as u8,
                    group: 0,
                },
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|entry| entry.0);
    let points = candidates
        .into_iter()
        .take(POINT_COUNT)
        .map(|entry| entry.1)
        .collect();

    let mut snapshot = Vec::with_capacity(16 + pixels.len() * 2);
    snapshot.extend_from_slice(MOCK_MAGIC);
    snapshot.extend_from_slice(&(WIDTH as u16).to_le_bytes());
    snapshot.extend_from_slice(&(HEIGHT as u16).to_le_bytes());
    snapshot.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
    for pixel in pixels {
        snapshot.extend_from_slice(&pixel.to_le_bytes());
    }
    (points, snapshot)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn write_asset(output: &Path, name: &str, bytes: &[u8]) -> Result<String, String> {
    let path = output.join(name);
    fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(format!("{name} fnv1a64={:016x}", fnv1a(bytes)))
}

fn run(output: &Path) -> Result<(), String> {
    fs::create_dir_all(output).map_err(|error| format!("create {}: {error}", output.display()))?;
    let font = swash::FontRef::from_index(FONT, 0).ok_or("invalid embedded Press Start 2P")?;
    let mister = text_target(font, "MiSTer")?;
    let magik = text_target(font, "MagiK")?;
    let (launcher, snapshot) = launcher_mock();
    if mister.len() != POINT_COUNT || magik.len() != POINT_COUNT || launcher.len() != POINT_COUNT {
        return Err("intro target does not contain exactly 40960 particles".into());
    }
    let mut provenance = vec![
        "MiSTer MagiK intro particle assets".to_owned(),
        format!("font_sha256={FONT_SHA256}"),
        format!("particles={POINT_COUNT}"),
        format!("hologram_layers={LAYERS}"),
    ];
    for (name, bytes) in [
        ("mister.pcloud", encode_cloud(&mister)),
        ("mister.pgroup", encode_groups(&mister)),
        ("magik.pcloud", encode_cloud(&magik)),
        ("magik.pgroup", encode_groups(&magik)),
        ("launcher-mock.pcloud", encode_cloud(&launcher)),
        ("launcher-mock.pgroup", encode_groups(&launcher)),
        ("launcher-mock.rgb565", snapshot),
    ] {
        provenance.push(write_asset(output, name, &bytes)?);
    }
    provenance.push("generator=generate-intro-assets".to_owned());
    let provenance = provenance.join("\n") + "\n";
    write_asset(output, "PROVENANCE.txt", provenance.as_bytes())?;
    Ok(())
}

fn main() -> Result<(), String> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: generate-intro-assets OUTPUT-DIRECTORY")?;
    run(&output)
}
