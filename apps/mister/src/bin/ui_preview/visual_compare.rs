// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use flate2::read::ZlibDecoder;
use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;

const BASELINE_SCHEMA: &str = "mister-magik-launcher-visual-baseline-v1";

#[derive(Deserialize)]
struct BaselineManifest {
    schema: String,
    scenes: Vec<BaselineScene>,
}

#[derive(Deserialize)]
struct BaselineScene {
    id: String,
    rgb565_hash: String,
}

#[derive(Deserialize)]
struct ActualProvenance {
    rgb565_hash: String,
}

struct DecodedPng {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

pub(crate) fn compare_launcher_matrix(
    expected_dir: &Path,
    actual_dir: &Path,
    mismatch_dir: &Path,
    scene_ids: &[String],
) -> Result<(), Box<dyn Error>> {
    validate_comparison_paths(expected_dir, actual_dir, mismatch_dir)?;
    if mismatch_dir.exists() {
        return Err(format!(
            "mismatch artifact directory already exists: {}",
            mismatch_dir.display()
        )
        .into());
    }
    let manifest: BaselineManifest =
        serde_json::from_slice(&std::fs::read(expected_dir.join("manifest.json"))?)?;
    if manifest.schema != BASELINE_SCHEMA {
        return Err(format!("unsupported launcher baseline schema {:?}", manifest.schema).into());
    }
    let manifest_ids = manifest
        .scenes
        .iter()
        .map(|scene| scene.id.as_str())
        .collect::<Vec<_>>();
    let expected_ids = scene_ids.iter().map(String::as_str).collect::<Vec<_>>();
    if manifest_ids != expected_ids {
        return Err("launcher baseline scenes do not exactly match manifest order".into());
    }
    let baselines = manifest
        .scenes
        .into_iter()
        .map(|scene| (scene.id.clone(), scene))
        .collect::<HashMap<_, _>>();

    let mut mismatch_count = 0usize;
    for scene_id in scene_ids {
        let baseline = baselines
            .get(scene_id)
            .ok_or_else(|| format!("launcher baseline is missing scene {scene_id:?}"))?;
        let expected_png = std::fs::read(expected_dir.join(format!("{scene_id}.png")))?;
        let actual_png = std::fs::read(actual_dir.join(format!("{scene_id}.png")))?;
        let actual_provenance: ActualProvenance =
            serde_json::from_slice(&std::fs::read(actual_dir.join(format!("{scene_id}.json")))?)?;
        if baseline.rgb565_hash == actual_provenance.rgb565_hash && expected_png == actual_png {
            continue;
        }

        if mismatch_count == 0 {
            std::fs::create_dir(mismatch_dir)?;
        }
        mismatch_count += 1;
        write_new(
            &mismatch_dir.join(format!("{scene_id}.expected.png")),
            &expected_png,
        )?;
        write_new(
            &mismatch_dir.join(format!("{scene_id}.actual.png")),
            &actual_png,
        )?;
        let diff = encode_diff_png(&decode_png(&expected_png)?, &decode_png(&actual_png)?)?;
        write_new(&mismatch_dir.join(format!("{scene_id}.diff.png")), &diff)?;
        eprintln!(
            "scene={scene_id} status=mismatch expected_hash={} actual_hash={}",
            baseline.rgb565_hash, actual_provenance.rgb565_hash
        );
    }
    if mismatch_count != 0 {
        return Err(format!(
            "launcher visual comparison failed: {mismatch_count} scene(s) differed; artifacts={}",
            mismatch_dir.display()
        )
        .into());
    }
    Ok(())
}

pub(crate) fn validate_comparison_paths(
    expected_dir: &Path,
    actual_dir: &Path,
    mismatch_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let expected = std::fs::canonicalize(expected_dir)?;
    for (label, candidate) in [("actual", actual_dir), ("mismatch", mismatch_dir)] {
        let parent = candidate
            .parent()
            .ok_or_else(|| format!("{label} matrix path has no parent"))?;
        let parent = std::fs::canonicalize(parent)?;
        if parent.starts_with(&expected) {
            return Err(format!(
                "{label} matrix path must be outside read-only baseline directory {}",
                expected_dir.display()
            )
            .into());
        }
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn encode_diff_png(expected: &DecodedPng, actual: &DecodedPng) -> Result<Vec<u8>, Box<dyn Error>> {
    let (width, height) = (actual.width, actual.height);
    let mut pixels = vec![0u8; width.saturating_mul(height).saturating_mul(3)];
    if expected.width != actual.width || expected.height != actual.height {
        for pixel in pixels.chunks_exact_mut(3) {
            pixel.copy_from_slice(&[255, 0, 255]);
        }
    } else {
        for ((expected, actual), diff) in expected
            .pixels
            .chunks_exact(3)
            .zip(actual.pixels.chunks_exact(3))
            .zip(pixels.chunks_exact_mut(3))
        {
            if expected != actual {
                diff.copy_from_slice(&[255, 0, 255]);
            }
        }
    }
    encode_rgb8_png(&pixels, width, height)
}

fn decode_png(encoded: &[u8]) -> Result<DecodedPng, Box<dyn Error>> {
    if !encoded.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("launcher baseline is not a PNG".into());
    }
    let mut cursor = 8usize;
    let mut dimensions = None;
    let mut compressed = Vec::new();
    let mut saw_end = false;
    while cursor < encoded.len() {
        let length = read_u32(encoded, &mut cursor)? as usize;
        let kind = take(encoded, &mut cursor, 4)?;
        let data = take(encoded, &mut cursor, length)?;
        let recorded_crc = read_u32(encoded, &mut cursor)?;
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(data);
        if crc.finalize() != recorded_crc {
            return Err("launcher PNG chunk has an invalid CRC".into());
        }
        match kind {
            b"IHDR" => {
                if data.len() != 13 || data[8..] != [8, 2, 0, 0, 0] || dimensions.is_some() {
                    return Err("launcher PNG must be non-interlaced RGB8".into());
                }
                let width = u32::from_be_bytes(data[..4].try_into()?) as usize;
                let height = u32::from_be_bytes(data[4..8].try_into()?) as usize;
                if width == 0 || height == 0 {
                    return Err("launcher PNG dimensions must be non-zero".into());
                }
                dimensions = Some((width, height));
            }
            b"IDAT" => compressed.extend_from_slice(data),
            b"IEND" => {
                saw_end = true;
                break;
            }
            _ => {}
        }
    }
    let (width, height) = dimensions.ok_or("launcher PNG has no IHDR")?;
    if !saw_end {
        return Err("launcher PNG has no IEND".into());
    }
    let row_bytes = width.checked_mul(3).ok_or("launcher PNG width overflow")?;
    let raw_len = row_bytes
        .checked_add(1)
        .and_then(|len| len.checked_mul(height))
        .ok_or("launcher PNG dimensions overflow")?;
    let mut raw = Vec::with_capacity(raw_len);
    ZlibDecoder::new(compressed.as_slice()).read_to_end(&mut raw)?;
    if raw.len() != raw_len {
        return Err("launcher PNG decompressed length is invalid".into());
    }
    let mut pixels = Vec::with_capacity(row_bytes.saturating_mul(height));
    for row in raw.chunks_exact(row_bytes + 1) {
        if row[0] != 0 {
            return Err("launcher PNG uses an unsupported row filter".into());
        }
        pixels.extend_from_slice(&row[1..]);
    }
    Ok(DecodedPng {
        width,
        height,
        pixels,
    })
}

fn read_u32(encoded: &[u8], cursor: &mut usize) -> Result<u32, Box<dyn Error>> {
    Ok(u32::from_be_bytes(take(encoded, cursor, 4)?.try_into()?))
}

fn take<'a>(
    encoded: &'a [u8],
    cursor: &mut usize,
    length: usize,
) -> Result<&'a [u8], Box<dyn Error>> {
    let end = cursor
        .checked_add(length)
        .ok_or("launcher PNG chunk length overflow")?;
    let bytes = encoded
        .get(*cursor..end)
        .ok_or("launcher PNG is truncated")?;
    *cursor = end;
    Ok(bytes)
}

fn encode_rgb8_png(pixels: &[u8], width: usize, height: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    if width == 0 || height == 0 || pixels.len() != width.saturating_mul(height).saturating_mul(3) {
        return Err("diff PNG dimensions do not match its pixels".into());
    }
    let mut raw = Vec::with_capacity(pixels.len().saturating_add(height));
    for row in pixels.chunks_exact(width * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&raw)?;
    let compressed = encoder.finish()?;

    let mut encoded = Vec::with_capacity(compressed.len().saturating_add(57));
    encoded.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut header = [0u8; 13];
    header[..4].copy_from_slice(&u32::try_from(width)?.to_be_bytes());
    header[4..8].copy_from_slice(&u32::try_from(height)?.to_be_bytes());
    header[8] = 8;
    header[9] = 2;
    append_png_chunk(&mut encoded, *b"IHDR", &header)?;
    append_png_chunk(&mut encoded, *b"IDAT", &compressed)?;
    append_png_chunk(&mut encoded, *b"IEND", &[])?;
    Ok(encoded)
}

fn append_png_chunk(
    encoded: &mut Vec<u8>,
    kind: [u8; 4],
    data: &[u8],
) -> Result<(), Box<dyn Error>> {
    encoded.extend_from_slice(&u32::try_from(data.len())?.to_be_bytes());
    encoded.extend_from_slice(&kind);
    encoded.extend_from_slice(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(&kind);
    crc.update(data);
    encoded.extend_from_slice(&crc.finalize().to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pixel_difference_fails_and_emits_repeatable_artifacts() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-visual-compare-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let expected = root.join("expected");
        let actual = root.join("actual");
        let mismatch = root.join("mismatch");
        std::fs::create_dir_all(&expected).expect("create expected fixture");
        std::fs::create_dir_all(&actual).expect("create actual fixture");
        std::fs::write(
            expected.join("manifest.json"),
            br#"{"schema":"mister-magik-launcher-visual-baseline-v1","scenes":[{"id":"scene","rgb565_hash":"authority"}]}"#,
        )
        .expect("write manifest");
        std::fs::write(actual.join("scene.json"), br#"{"rgb565_hash":"authority"}"#)
            .expect("write provenance");
        let expected_png = encode_rgb8_png(&[0, 0, 0, 0, 0, 0], 2, 1).expect("encode expected");
        let actual_png = encode_rgb8_png(&[0, 0, 0, 0, 0, 1], 2, 1).expect("encode actual");
        std::fs::write(expected.join("scene.png"), &expected_png).expect("write expected PNG");
        std::fs::write(actual.join("scene.png"), &actual_png).expect("write actual PNG");

        let error = compare_launcher_matrix(&expected, &actual, &mismatch, &["scene".to_owned()])
            .expect_err("one changed pixel must fail");
        assert!(error.to_string().contains("1 scene(s) differed"));
        assert_eq!(
            std::fs::read(mismatch.join("scene.expected.png")).expect("expected artifact"),
            expected_png
        );
        assert_eq!(
            std::fs::read(mismatch.join("scene.actual.png")).expect("actual artifact"),
            actual_png
        );
        let diff =
            decode_png(&std::fs::read(mismatch.join("scene.diff.png")).expect("diff artifact"))
                .expect("decode diff");
        assert_eq!(diff.pixels, [0, 0, 0, 255, 0, 255]);
        assert!(
            compare_launcher_matrix(&expected, &actual, &mismatch, &["scene".to_owned()])
                .expect_err("artifact overwrite must be refused")
                .to_string()
                .contains("already exists")
        );
        std::fs::remove_dir_all(root).expect("remove comparison fixture");
    }
}
