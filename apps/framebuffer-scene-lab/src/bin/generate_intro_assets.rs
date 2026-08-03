// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic Press Start 2P intro particle asset compiler.

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
    left: i32,
    top: i32,
    advance: i32,
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
        left: image.placement.left,
        top: image.placement.top,
        advance: (font.glyph_metrics(&[]).advance_width(id)
            * (128.0 / font.metrics(&[]).units_per_em as f32)) as i32,
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
    mask_points: &[(usize, usize)],
    anchor_x: f32,
    center_y: f32,
    group: u8,
    count: usize,
) -> Result<Vec<Point>, String> {
    if mask_points.is_empty() {
        return Err(format!("glyph track {group} has no opaque samples"));
    }

    // Match the original MagiK effect: every opaque mask sample receives
    // particles before any sample is duplicated. Duplicates get the same
    // small target jitter and shallow distributed depth used by that effect,
    // producing a dense phosphor face rather than separated hologram sheets.
    let mut points = Vec::with_capacity(count);
    for index in 0..count {
        let mask_index = index.saturating_mul(mask_points.len()) / count;
        let (x, y) = mask_points[mask_index.min(mask_points.len() - 1)];
        let hash = mix32((index as u32) ^ (u32::from(group) << 24) ^ 0x9e37_79b9);
        let duplicate = count > mask_points.len();
        let jitter_x = duplicate.then(|| signed_unit(hash) * 0.4).unwrap_or(0.0);
        let jitter_y = duplicate
            .then(|| signed_unit(hash.rotate_left(11)) * 0.4)
            .unwrap_or(0.0);
        let layer = ((hash.rotate_left(19) as usize) % LAYERS) as f32;
        points.push(Point {
            x: quantize(anchor_x + x as f32 + jitter_x, 480.0),
            y: quantize(y as f32 - center_y + jitter_y, 220.0),
            z: quantize((layer - 4.0) * 2.5, 96.0),
            palette: (hash >> 29) as u8,
            group,
        });
    }
    Ok(points)
}

fn mix32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn signed_unit(value: u32) -> f32 {
    (value >> 8) as f32 * (2.0 / 16_777_215.0) - 1.0
}

fn text_target(font: swash::FontRef<'_>, text: &str) -> Result<Vec<Point>, String> {
    let mut context = ScaleContext::new();
    let mut glyphs = Vec::new();
    for ch in text.chars() {
        glyphs.push(glyph(font, &mut context, ch)?);
    }
    let mut pen_x = 0_i32;
    let mut bounds: Option<(i32, i32, i32, i32)> = None;
    for glyph in &glyphs {
        let left = pen_x + glyph.left;
        let top = -glyph.top;
        let right = left + glyph.width as i32;
        let bottom = top + glyph.height as i32;
        bounds = Some(match bounds {
            Some((min_x, min_y, max_x, max_y)) => (
                min_x.min(left),
                min_y.min(top),
                max_x.max(right),
                max_y.max(bottom),
            ),
            None => (left, top, right, bottom),
        });
        pen_x += glyph.advance;
    }
    let (min_x, min_y, max_x, max_y) = bounds.ok_or("text has no ink bounds")?;
    let width = usize::try_from(max_x - min_x).map_err(|_| "text width is invalid")?;
    let height = usize::try_from(max_y - min_y).map_err(|_| "text height is invalid")?;
    let mut tracks = vec![Vec::new(); 6];
    pen_x = 0;
    for (letter_index, glyph) in glyphs.iter().enumerate() {
        let left = pen_x + glyph.left - min_x;
        let top = -glyph.top - min_y;
        for y in 0..glyph.height {
            for x in 0..glyph.width {
                if glyph.alpha[y * glyph.width + x] < 128 {
                    continue;
                }
                let word_x = usize::try_from(left + x as i32).map_err(|_| "negative glyph x")?;
                let word_y = usize::try_from(top + y as i32).map_err(|_| "negative glyph y")?;
                let group = if text == "MagiK" && letter_index == 4 {
                    4 + ((word_x + word_y) & 1)
                } else {
                    letter_index
                };
                tracks[group].push((word_x, word_y));
            }
        }
        pen_x += glyph.advance;
    }
    if text == "MagiK" && width != 624 {
        return Err(format!(
            "whole-word MagiK layout is {width}px wide, expected original 624px"
        ));
    }
    let mut points = Vec::with_capacity(POINT_COUNT);
    for group in 0..tracks.len() {
        points.extend(text_track(
            &tracks[group],
            -(width as f32) * 0.5,
            height as f32 * 0.5,
            group as u8,
            TRACK_COUNTS[group],
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
    if mister.len() != POINT_COUNT || magik.len() != POINT_COUNT {
        return Err("intro target does not contain exactly 40960 particles".into());
    }
    let mut provenance = vec![
        "MiSTer MagiK intro particle assets".to_owned(),
        format!("font_sha256={FONT_SHA256}"),
        format!("particles={POINT_COUNT}"),
        format!("distributed_depth_layers={LAYERS}"),
        "text_style=original-magik-phosphor".to_owned(),
        "text_layout=whole-string-original-mask-advances".to_owned(),
        "text_alignment=independent-whole-word-center".to_owned(),
    ];
    for (name, bytes) in [
        ("mister.pcloud", encode_cloud(&mister)),
        ("mister.pgroup", encode_groups(&mister)),
        ("magik.pcloud", encode_cloud(&magik)),
        ("magik.pgroup", encode_groups(&magik)),
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
