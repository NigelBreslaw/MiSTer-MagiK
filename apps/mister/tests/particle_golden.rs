// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#![cfg(any(feature = "ui", feature = "ui-preview"))]

use mister_magik_fb::particle_engine::{ParticleConfig, ParticlePreset};
use mister_magik_fb::particle_renderer::ParticleRenderer;
use mister_magik_fb::startup_particles::{
    ArcadeCabinetFormation, Rgb565Pixel as CabinetRgb565Pixel,
};
use mister_magik_particles::recipes::embedded_cabinet_recipe;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Duration;

const WIDTH: usize = 960;
const HEIGHT: usize = 540;
const MAGIK_SEED: u64 = 0x4d61_6769_4b;

fn frame_signature(words: impl IntoIterator<Item = u16>) -> u64 {
    words.into_iter().fold(0xcbf2_9ce4_8422_2325, |hash, word| {
        word.to_le_bytes().into_iter().fold(hash, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    })
}

fn elapsed_at_60_hz(frame: u64) -> Duration {
    Duration::from_nanos(frame.saturating_mul(1_000_000_000) / 60)
}

#[test]
fn production_magik_rgb565_frame_matches_the_approved_golden() {
    let mut renderer = ParticleRenderer::new_magik(ParticleConfig {
        count: 16_384,
        width: WIDTH,
        height: HEIGHT,
        seed: MAGIK_SEED,
        preset: ParticlePreset::Visual,
    })
    .unwrap();
    let mut frame = vec![Rgb565Pixel(0); WIDTH * HEIGHT];
    for frame_number in 0..=360 {
        renderer
            .render(&mut frame, 1, elapsed_at_60_hz(frame_number))
            .unwrap();
    }

    assert_eq!(
        frame_signature(frame.iter().map(|pixel| pixel.0)),
        0x8588_b0ed_5857_049d
    );
}

#[test]
fn arcade_cabinet_rgb565_frame_matches_the_approved_showcase_golden() {
    let mut recipe = embedded_cabinet_recipe().unwrap();
    recipe.seed = 827_141_709_451;
    let mut renderer = ArcadeCabinetFormation::new(WIDTH, HEIGHT, recipe).unwrap();
    let mut frame = vec![CabinetRgb565Pixel(0); WIDTH * HEIGHT];

    renderer
        .render(&mut frame, Duration::from_secs(15), 0)
        .unwrap();

    assert_eq!(
        frame_signature(frame.iter().map(|pixel| pixel.0)),
        0x7a94_91aa_cae3_c433
    );
}
