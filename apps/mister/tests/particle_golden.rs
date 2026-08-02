// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#![cfg(any(feature = "ui", feature = "ui-preview"))]

use mister_magik_fb::particle_engine::{ParticleConfig, ParticlePreset};
use mister_magik_fb::particle_renderer::ParticleRenderer;
use mister_magik_fb::startup_particles::ArcadeCabinetFormation;
use slint::platform::software_renderer::Rgb565Pixel;
use std::time::Duration;

const WIDTH: usize = 960;
const HEIGHT: usize = 540;
const MAGIK_SEED: u64 = 0x4d61_6769_4b;

fn frame_signature(frame: &[Rgb565Pixel]) -> u64 {
    frame.iter().fold(0xcbf2_9ce4_8422_2325, |hash, pixel| {
        pixel.0.to_le_bytes().into_iter().fold(hash, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    })
}

fn elapsed_at_60_hz(frame: u64) -> Duration {
    Duration::from_nanos(frame.saturating_mul(1_000_000_000) / 60)
}

#[test]
fn production_magik_rgb565_frame_matches_the_pre_consolidation_golden() {
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

    assert_eq!(frame_signature(&frame), 0xbce2_cba6_cc4a_9199);
}

#[test]
fn arcade_cabinet_rgb565_frame_matches_the_approved_showcase_golden() {
    let renderer = ArcadeCabinetFormation::new(WIDTH, HEIGHT, 827_141_709_451).unwrap();
    let mut frame = vec![Rgb565Pixel(0); WIDTH * HEIGHT];

    renderer
        .render(&mut frame, Duration::from_secs(15))
        .unwrap();

    assert_eq!(frame_signature(&frame), 0xac1d_5455_b6dc_5fad);
}
