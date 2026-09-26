// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Offline production-art/font review. These renders are not device captures.
use mister_magik_fb::bitmap_font_resource::{
    jersey_25_console_bitmap_font, launcher_bitmap_font, nocive_15_console_bitmap_font,
    spleen_6x12_native_console_bitmap_font, xerxes_10_console_bitmap_font,
};
use mister_magik_fb::launcher_home::{LauncherHomeCounts, LauncherHomeSnapshot};
use mister_magik_framebuffer_scenes::{
    Rgb565Pixel,
    launcher::{LauncherData, LauncherScene, LauncherTypography},
    launcher_navigation::{BrowseDirection, BrowseFrame, BrowsePhase},
};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("output directory required")?,
    );
    std::fs::create_dir_all(&output)?;
    let font_error = |e: String| std::io::Error::other(e);
    let heading = launcher_bitmap_font(nocive_15_console_bitmap_font().map_err(font_error)?);
    let number = launcher_bitmap_font(jersey_25_console_bitmap_font().map_err(font_error)?);
    let metadata = launcher_bitmap_font(xerxes_10_console_bitmap_font().map_err(font_error)?);
    let fallback =
        launcher_bitmap_font(spleen_6x12_native_console_bitmap_font().map_err(font_error)?);
    let fonts = LauncherTypography {
        heading: &heading,
        number: &number,
        metadata: &metadata,
        fallback: &fallback,
    };
    let snapshot = LauncherHomeSnapshot::from_counts(LauncherHomeCounts {
        arcade: 987,
        consoles: 18000,
        computers: 15000,
        handhelds: 1229,
        favourites: 1,
        collections: 77,
    });
    let assets = [
        "01_arcade",
        "02_consoles",
        "03_computers",
        "04_handhelds",
        "05_favourites",
        "06_settings",
    ]
    .map(|name| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("assets/ui/launcher-cards/{name}.rgb565"));
        std::fs::read(path).map(|bytes| {
            bytes
                .chunks_exact(2)
                .map(|p| Rgb565Pixel(u16::from_le_bytes([p[0], p[1]])))
                .collect::<Vec<_>>()
        })
    });
    let assets = assets.into_iter().collect::<Result<Vec<_>, _>>()?;
    let artwork: Vec<_> = assets.iter().map(Vec::as_slice).collect();
    let rgb_assets = [
        "01_arcade",
        "02_consoles",
        "03_computers",
        "04_handhelds",
        "05_favourites",
        "06_settings",
    ]
    .map(|name| {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("assets/ui/launcher-cards/{name}.rgb888")),
        )
    })
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
    let rgb_artwork: Vec<_> = rgb_assets.iter().map(Vec::as_slice).collect();
    for (name, scene) in [
        ("hdmi-landscape", LauncherScene::new(960, 540)),
        ("hdmi-portrait", LauncherScene::new(540, 960)),
        ("crt-240-landscape", LauncherScene::crt(640, 240)),
        ("crt-240-portrait", LauncherScene::crt(240, 640)),
        ("crt-288-landscape", LauncherScene::crt(640, 288)),
        ("crt-288-portrait", LauncherScene::crt(288, 640)),
        ("crt-480-landscape", LauncherScene::crt(640, 480)),
        ("crt-480-portrait", LauncherScene::crt(480, 640)),
        ("crt-5x4-landscape", LauncherScene::crt(640, 512)),
        ("crt-5x4-portrait", LauncherScene::crt(512, 640)),
    ] {
        let data = LauncherData {
            cards: &snapshot.cards,
            selected: 0,
            library_games: snapshot.library_games,
            collections: 77,
            favourites: 1,
            clock: "07:28",
        };
        let mut prepared = if scene.uses_responsive_layout() {
            scene
                .prepare_initial_with_rgb888_artwork_and_typography(data, &rgb_artwork, fonts)
                .finish()
        } else {
            scene
                .prepare_initial_with_artwork_and_typography(data, &artwork, fonts)
                .finish()
        };
        for (state, selected, progress) in [
            ("arcade", 0, 0),
            ("computers", 2, 0),
            ("favourites", 4, 0),
            ("moving", 0, 230),
        ] {
            prepared.render_frame(BrowseFrame {
                selected,
                target: (selected + 1) % 6,
                phase: if progress == 0 {
                    BrowsePhase::Settled
                } else {
                    BrowsePhase::Flipping
                },
                direction: Some(BrowseDirection::Right),
                progress_millis: progress,
                duration_millis: 460,
            });
            let mut file = std::io::BufWriter::new(std::fs::File::create(
                output.join(format!("{name}-{state}.ppm")),
            )?);
            write!(file, "P6\n{} {}\n255\n", scene.width, scene.height)?;
            for pixel in prepared.pixels() {
                let p = pixel.0;
                let (r, g, b) = ((p >> 11) as u8, ((p >> 5) & 63) as u8, (p & 31) as u8);
                file.write_all(&[
                    (r << 3) | (r >> 2),
                    (g << 2) | (g >> 4),
                    (b << 3) | (b >> 2),
                ])?;
            }
        }
        eprintln!("{name}: {} bytes cached", prepared.cached_raster_bytes());
    }
    Ok(())
}
