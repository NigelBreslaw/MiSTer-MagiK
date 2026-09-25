// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline composition review only: these files are NOT device captures.
use mister_magik_framebuffer_scenes::launcher::{
    LauncherCard, LauncherCardId, LauncherData, LauncherScene,
};
use mister_magik_framebuffer_scenes::launcher_navigation::{
    BrowseDirection, BrowseFrame, BrowsePhase,
};
use std::io::Write;
fn main() -> std::io::Result<()> {
    let directory = std::env::args().nth(1).expect("output directory required");
    std::fs::create_dir_all(&directory)?;
    let cards = [
        LauncherCard {
            id: LauncherCardId::Arcade,
            name: "ARCADE",
            games: Some(1752),
            colour: 0x88a6,
        },
        LauncherCard {
            id: LauncherCardId::Consoles,
            name: "SNK NEOGEO",
            games: Some(324),
            colour: 0x195f,
        },
        LauncherCard {
            id: LauncherCardId::Consoles,
            name: "CONSOLES",
            games: Some(842),
            colour: 0xc5b5,
        },
        LauncherCard {
            id: LauncherCardId::Handhelds,
            name: "HANDHELDS",
            games: Some(126),
            colour: 0x2c92,
        },
        LauncherCard {
            id: LauncherCardId::Computers,
            name: "COMPUTERS",
            games: Some(86),
            colour: 0xb9a6,
        },
    ];
    let start = std::time::Instant::now();
    let mut scene = LauncherScene::new(960, 540).prepare(LauncherData {
        cards: &cards,
        selected: 0,
        library_games: 6842,
        collections: 18,
        favourites: 126,
        clock: "21:37",
    });
    eprintln!(
        "Preparation: {:?}; raster capacity: {} bytes",
        start.elapsed(),
        scene.cached_raster_bytes()
    );
    for (name, selected, progress) in [
        ("arcade", 0, 0),
        ("neogeo", 1, 0),
        ("early", 0, 150),
        ("late", 0, 310),
        ("almost", 0, 459),
    ] {
        scene.render_frame(BrowseFrame {
            selected,
            target: (selected + 1) % 5,
            phase: if progress == 0 {
                BrowsePhase::Settled
            } else {
                BrowsePhase::Flipping
            },
            direction: Some(BrowseDirection::Right),
            progress_millis: progress,
            duration_millis: 460,
        });
        let mut file =
            std::io::BufWriter::new(std::fs::File::create(format!("{directory}/{name}.ppm"))?);
        file.write_all(b"P6\n960 540\n255\n")?;
        for p in scene.pixels() {
            let r = (p.0 >> 11) as u8;
            let g = ((p.0 >> 5) & 63) as u8;
            let b = (p.0 & 31) as u8;
            file.write_all(&[
                (r << 3) | (r >> 2),
                (g << 2) | (g >> 4),
                (b << 3) | (b >> 2),
            ])?;
        }
    }
    Ok(())
}
