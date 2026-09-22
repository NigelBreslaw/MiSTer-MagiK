// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
use mister_magik_visual_concepts::{EFFECTS, Pixel, Preset, Scene};
use std::time::Duration;

#[test]
fn reported_damage_matches_full_redraw_in_both_geometries() {
    for height in [540, 600] {
        for &effect in EFFECTS {
            let mut scene = Scene::new(effect, Preset::Reduced, 960, height).unwrap();
            let mut retained = vec![Pixel(0xf81f); 960 * height];
            for step in [0, 17, 333, 500, 2000] {
                scene.advance(Duration::from_millis(step));
                let r = scene.render().unwrap();
                assert!(r.x0 <= r.x1 && r.y0 <= r.y1 && r.x1 <= 960 && r.y1 <= height);
                for y in r.y0..r.y1 {
                    let row = y * 960 + r.x0..y * 960 + r.x1;
                    retained[row.clone()].copy_from_slice(&scene.pixels()[row]);
                }
                assert_eq!(
                    retained,
                    scene.pixels(),
                    "damage mismatch: {effect} {height} {step}"
                );
            }
            assert!(
                scene.storage_bytes() < 128 * 1024 * 1024,
                "{effect} memory budget"
            );
        }
    }
}

#[test]
fn every_concept_resets_to_the_same_seed_and_timeline() {
    for &effect in EFFECTS {
        let mut scene = Scene::new(effect, Preset::Reduced, 320, 180).unwrap();
        let mut first = Vec::new();
        for pass in 0..2 {
            scene.reset().unwrap();
            let frames = if effect == "point-cloud-morph" {
                720
            } else {
                70
            };
            for step in 0..frames {
                scene.render().unwrap();
                if step == frames - 1 {
                    if pass == 0 {
                        first = scene.pixels().to_vec();
                    } else {
                        assert_eq!(scene.pixels(), first, "reset mismatch: {effect}");
                    }
                }
                scene.advance(Duration::from_nanos(16_666_667));
            }
        }
    }
}
