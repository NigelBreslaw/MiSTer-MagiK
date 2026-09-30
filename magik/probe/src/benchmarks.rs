// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Isolated production-renderer workloads for the native benchmark contract.
use mister_magik_framebuffer_scenes::launcher::{
    LauncherCard, LauncherCardId, LauncherData, LauncherFaceCache, LauncherLevel, LauncherScene,
    NestedLevel,
};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase,
};
use std::time::Instant;

const FIXTURE: &str = "6b74c8b73e354935a730a02c2eb4ddf144f83e90ccd96e3ab9b46c63189b6522";
const REFRESHES: usize = 1;
const PIXELS: usize = 960 * 540;

fn cards() -> [LauncherCard<'static>; 6] {
    ["ATARI", "SEGA", "NINTENDO", "SONY", "NEC", "SNK"].map(|name| LauncherCard {
        id: LauncherCardId::Consoles,
        name,
        games: Some(10),
        colour: 0x2a7f,
    })
}
fn data<'a>(cards: &'a [LauncherCard<'a>], nested: bool) -> LauncherData<'a> {
    LauncherData {
        cards,
        selected: 0,
        library_games: 60,
        collections: 6,
        favourites: 1,
        clock: "07:28",
        level: if nested {
            LauncherLevel::Nested(NestedLevel {
                path: &["CONSOLES"],
                games: 60,
                children: 6,
                children_label: "MAKERS",
                detail: Some((9, "SYSTEMS")),
                accent: 0x2a7f,
            })
        } else {
            LauncherLevel::Root
        },
    }
}
fn correctness() -> Result<(), String> {
    let scene = LauncherScene::new(960, 540);
    for nested in [false, true] {
        let mut cards = cards();
        let mut cache = LauncherFaceCache::default();
        let _ = scene.prepare_with_face_cache(data(&cards, nested), &mut cache);
        cards[0].games = Some(999);
        let mut cached = scene.prepare_with_face_cache(data(&cards, nested), &mut cache);
        let mut cold = scene.prepare(data(&cards, nested));
        for phase in [0, 1, 90, 179, 180] {
            let frame = BrowseFrame {
                selected: 0,
                target: 1,
                phase: BrowsePhase::Flipping,
                direction: Some(BrowseDirection::Right),
                progress_millis: phase,
                duration_millis: 180,
            };
            cached.render_frame(frame);
            cold.render_frame(frame);
            if cached.pixels() != cold.pixels() {
                return Err(format!(
                    "count refresh pixel mismatch: nested={nested}, phase={phase}"
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn requested() -> Option<Result<(), String>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_none_or(|arg| arg != "--bench") {
        return None;
    }
    Some(run(&args))
}
fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 4 || args[2] != "--mode" {
        return Err("expected --bench WORKLOAD --mode MODE".into());
    }
    if args[1] == "controller-save" {
        if args[3] != "timing" {
            return Err("controller-save supports timing mode".into());
        }
        println!("{}", crate::controller_save::run()?);
        return Ok(());
    }
    if args[1] == "incremental-refresh" {
        if args[3] != "timing" {
            return Err("incremental-refresh supports timing mode".into());
        }
        // Single-threaded benchmark dispatch; reserve stdout for its result.
        unsafe {
            std::env::set_var("MISTER_CATALOG_PROTOCOL_STDOUT", "1");
        }
        println!("{}", crate::incremental_refresh::run()?);
        return Ok(());
    }
    if args[1] == "preview-shards" {
        if args[3] != "timing" {
            return Err("preview-shards supports timing mode".into());
        }
        // Benchmark dispatch precedes initialization and is single-threaded.
        // Existing catalog protocol routing reserves stdout for the result.
        unsafe {
            std::env::set_var("MISTER_CATALOG_PROTOCOL_STDOUT", "1");
        }
        println!("{}", crate::preview_shards::run()?);
        return Ok(());
    }
    if args[1] == "catalog-launch" {
        if args[3] != "timing" {
            return Err("catalog-launch supports timing mode".into());
        }
        println!("{}", crate::catalog_launch::run()?);
        return Ok(());
    }
    if args[1] == "catalog-sort" {
        if args[3] != "timing" {
            return Err("catalog-sort supports timing mode".into());
        }
        println!("{}", crate::catalog_sort::run()?);
        return Ok(());
    }
    if args[1] == "retained-home-tiles" {
        if args[3] != "timing" {
            return Err("retained-home-tiles supports timing mode".into());
        }
        println!("{}", crate::retained_tiles::run()?);
        return Ok(());
    }
    if args[0] != "--bench" || args[1] != "home-count-refresh" {
        return Err("unknown Mini workload".into());
    }
    if args[3] != "timing" {
        return Err("home-count-refresh supports timing mode".into());
    }
    correctness()?;
    let scene = LauncherScene::new(960, 540);
    let work_count = PIXELS * REFRESHES * 2;
    let mut samples = Vec::with_capacity(2);
    for repetition in 0..2 {
        let mut cards = [cards(), cards()];
        let mut caches = [LauncherFaceCache::default(), LauncherFaceCache::default()];
        // Seed unchanged faces outside timing. Both revisions run this same
        // fixture and lifecycle; the parent adapter performs cold preparation.
        for nested in [false, true] {
            let index = usize::from(nested);
            let _ = scene.prepare_with_face_cache(data(&cards[index], nested), &mut caches[index]);
        }
        let start = Instant::now();
        for count in 0..REFRESHES {
            for nested in [false, true] {
                let index = usize::from(nested);
                cards[index][0].games = Some(1_000 + count as u32);
                let prepared =
                    scene.prepare_with_face_cache(data(&cards[index], nested), &mut caches[index]);
                std::hint::black_box(prepared.pixels());
            }
        }
        let duration_ns = start.elapsed().as_nanos() as u64;
        samples.push(serde_json::json!({"repetition": repetition, "fixture_identity": FIXTURE,
            "work_count": work_count, "duration_ns": duration_ns, "ns_per_pixel": duration_ns as f64 / work_count as f64}));
    }
    let sha = std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256")
        .map_err(|_| "native benchmark artifact identity missing")?;
    println!(
        "{}",
        serde_json::json!({"schema_version": 1, "workload": "home-count-refresh",
        "mode": "timing", "artifact_sha256": sha, "correctness": "passed", "fixture": {
            "identity": FIXTURE, "geometry": [960, 540], "levels": ["root", "consoles"],
            "cards_per_level": 6, "refreshes_per_level": REFRESHES, "changed_cards_per_refresh": 1,
            "artwork": "generic", "fonts": "portable", "timed_work": "prepare, resting raster, drop"},
        "work_count": work_count, "samples": samples})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn count_refresh_matches_cold_motion_pixels() {
        super::correctness().unwrap();
    }
}
