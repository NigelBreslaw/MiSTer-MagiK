// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Fixed-frame portable review using the exact Mini workload.
use mister_magik_visual_concepts::{Preset, RENDER_LABS, Scene};
use std::{io::Write, time::Duration};
pub(super) fn requested() -> Option<Result<(), String>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_none_or(|v| v != "--render-lab") {
        return None;
    }
    Some(run(&args))
}
fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 8
        || args[2] != "--preset"
        || args[4] != "--time-ms"
        || args[6] != "--output"
        || !RENDER_LABS.contains(&args[1].as_str())
    {
        return Err("expected --render-lab launcher-cards|arcade-transition|settings-transition --preset default --time-ms N --output FILE.ppm".into());
    }
    let mut scene = Scene::new(&args[1], Preset::parse(&args[3])?, 960, 540)?;
    let ms = args[5].parse::<u64>().map_err(|e| e.to_string())?;
    scene.advance(Duration::from_millis(ms));
    scene.render()?;
    let mut file = std::fs::File::create(&args[7]).map_err(|e| e.to_string())?;
    file.write_all(b"P6\n960 540\n255\n")
        .map_err(|e| e.to_string())?;
    let rgb: Vec<_> = scene
        .pixels()
        .iter()
        .flat_map(|p| {
            let r = p.0 >> 11;
            let g = (p.0 >> 5) & 63;
            let b = p.0 & 31;
            [
                ((r << 3) | (r >> 2)) as u8,
                ((g << 2) | (g >> 4)) as u8,
                ((b << 3) | (b >> 2)) as u8,
            ]
        })
        .collect();
    file.write_all(&rgb).map_err(|e| e.to_string())?;
    println!(
        "workload={} preset={} time_ms={} storage_bytes={} output={}",
        args[1],
        args[3],
        ms,
        scene.storage_bytes(),
        args[7]
    );
    Ok(())
}
