// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(feature = "launcher-profile")]

use mister_magik_framebuffer_scenes::{launcher::*, launcher_navigation::*, launcher_profile};

#[test]
fn explicit_profiling_preserves_pixels_and_drains_at_window_boundaries() {
    assert!(launcher_profile::span("disabled").is_none());
    let cards = [LauncherCard {
        id: LauncherCardId::Arcade,
        name: "ARCADE",
        games: Some(1752),
        colour: 0x88a6,
    }; 5];
    let mut scene = LauncherScene::new(960, 540).prepare(LauncherData {
        cards: &cards,
        selected: 0,
        library_games: 6842,
        collections: 18,
        favourites: 126,
        clock: "21:37",
    });
    let frames: Vec<_> = (1..460)
        .step_by(40)
        .map(|progress| BrowseFrame {
            selected: 0,
            target: 1,
            phase: BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: progress,
            duration_millis: 460,
            outgoing: None,
        })
        .collect();
    let reference: Vec<_> = frames
        .iter()
        .map(|frame| {
            scene.render_frame(*frame);
            scene.pixels().to_vec()
        })
        .collect();
    assert!(launcher_profile::take().stages.is_empty());
    launcher_profile::enable().unwrap();
    for (frame, expected) in frames.iter().zip(reference) {
        scene.render_frame(*frame);
        assert_eq!(scene.pixels(), expected);
    }
    let report = launcher_profile::take();
    for stage in [
        "scene.clear",
        "flip.compose",
        "flip.reflection",
        "flip.project",
        "flip.geometry-filter",
    ] {
        assert!(report.stages.contains_key(stage), "missing {stage}");
    }
    assert!(
        report.hardware.records.is_empty(),
        "raw samples must not grow the metrics envelope"
    );
    assert!(launcher_profile::take().stages.is_empty());
}
