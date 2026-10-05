// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Offline production-art/font review. These renders are not device captures.
use mister_magik_fb::bitmap_font_resource::{
    jersey_25_console_bitmap_font, launcher_bitmap_font, nocive_15_console_bitmap_font,
    spleen_6x12_native_console_bitmap_font, xerxes_10_console_bitmap_font,
};
use mister_magik_fb::launcher_home::{CardLevelSnapshot, LauncherHomeCounts, LauncherHomeSnapshot};
use mister_magik_framebuffer_scenes::{
    Rgb565Pixel,
    launcher::{
        LEVEL_TRICK_EDGE_MILLIS, LEVEL_TRICK_MILLIS, LauncherCard, LauncherCardId, LauncherData,
        LauncherLevel, LauncherScene, LauncherTypography, LevelChange, NestedLevel,
        PreparedLauncher,
    },
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
    let root_keys: Vec<_> = CardLevelSnapshot::root(&snapshot)
        .cards
        .iter()
        .map(|card| card.artwork_key.clone())
        .collect();
    let rgb_assets = mister_magik_fb::launcher_artwork::load_cards(
        &mister_magik_fb::launcher_artwork::asset_root(),
        &root_keys,
    );
    let rgb_artwork: Vec<_> = rgb_assets.iter().map(|pixels| pixels.as_ref()).collect();
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
            level: mister_magik_framebuffer_scenes::launcher::LauncherLevel::Root,
        };
        let mut prepared = scene
            .prepare_initial_with_rgb888_artwork_and_typography(data, &rgb_artwork, fonts)
            .finish();
        for (state, selected, progress) in [
            ("arcade", 0, 0),
            ("computers", 2, 0),
            ("favourites", 4, 0),
            ("settings", 5, 0),
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
        if name.ends_with("landscape") && (name.starts_with("hdmi") || name.starts_with("crt-240"))
        {
            review_level_trick(&output, name, scene, &mut prepared, fonts)?;
            review_installed_systems(&output, name, scene, fonts)?;
        }
    }
    Ok(())
}

/// Consoles opened from the root: installed maker artwork, breadcrumb and the
/// level-change trick from the Consoles root card.
fn review_level_trick(
    output: &std::path::Path,
    name: &str,
    scene: LauncherScene,
    root: &mut PreparedLauncher,
    fonts: LauncherTypography<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    const CONSOLES: u16 = 0x2a7f;
    let makers = [
        ("ATARI", 1353),
        ("SEGA", 1897),
        ("SONY PLAYSTATION", 1291),
        ("NINTENDO", 4170),
        ("NEC", 1103),
        ("SNK NEOGEO", 184),
    ]
    .map(|(name, games)| LauncherCard {
        id: LauncherCardId::Consoles,
        name,
        games: Some(games),
        colour: CONSOLES,
    });
    let data = LauncherData {
        cards: &makers,
        selected: 0,
        library_games: 0,
        collections: 0,
        favourites: 0,
        clock: "07:28",
        level: LauncherLevel::Nested(NestedLevel {
            path: &["CONSOLES"],
            games: 9998,
            children: makers.len() as u32,
            children_label: "MAKERS",
            detail: Some((17, "SYSTEMS")),
            accent: CONSOLES,
        }),
    };
    let prepare_started = std::time::Instant::now();
    let keys = ["atari", "sega", "sony", "nintendo", "nec", "snk"]
        .map(|maker| format!("menu:consoles:{maker}"));
    let mut artwork: Vec<_> = mister_magik_fb::launcher_artwork::load_artwork(
        &mister_magik_fb::launcher_artwork::asset_root(),
        &keys,
    )
    .into_iter()
    .map(Some)
    .collect();
    let mut consoles = scene
        .prepare_initial_with_rgb888_loader_and_cache(
            data,
            &mut |index| artwork[index].take().unwrap(),
            Some(fonts),
            &mut mister_magik_framebuffer_scenes::launcher::LauncherFaceCache::default(),
            1,
        )
        .finish();
    eprintln!(
        "{name}: nested level prepared in {:?}",
        prepare_started.elapsed()
    );
    let source_slot = root.slot_zero();
    let destination_slot = consoles.slot_zero();
    for t in [0, 150, 300, 459, 460, 461, 580, 740, LEVEL_TRICK_MILLIS] {
        let frame = if t < LEVEL_TRICK_EDGE_MILLIS {
            root.render_level_gather_to(1, LevelChange::Descend, t, destination_slot);
            root.render_transition_title_from(&consoles, t);
            root.pixels()
        } else {
            consoles.render_level_deal_from(0, LevelChange::Descend, t, source_slot);
            consoles.pixels()
        };
        write_ppm(
            &output.join(format!("{name}-trick-{t:04}.ppm")),
            scene,
            frame,
        )?;
    }
    root.restore_chrome();
    consoles.render_frame(BrowseFrame {
        selected: 3,
        target: 3,
        phase: BrowsePhase::Settled,
        direction: None,
        progress_millis: 0,
        duration_millis: 0,
    });
    write_ppm(
        &output.join(format!("{name}-consoles-nintendo.ppm")),
        scene,
        consoles.pixels(),
    )?;
    // Browsing a nested level: every card slides one slot and only the end
    // cards flip (one turns away, one turns in showing its MagiK back).
    for progress in [80, 160, 240, 320, 400] {
        consoles.render_frame(BrowseFrame {
            selected: 3,
            target: 4,
            phase: BrowsePhase::Flipping,
            direction: Some(BrowseDirection::Right),
            progress_millis: progress,
            duration_millis: 460,
        });
        write_ppm(
            &output.join(format!("{name}-browse-{progress:03}.ppm")),
            scene,
            consoles.pixels(),
        )?;
    }
    Ok(())
}

/// A mixture of installed systems and a missing source uses the real loader.
fn review_installed_systems(
    output: &std::path::Path,
    name: &str,
    scene: LauncherScene,
    fonts: LauncherTypography<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (group, id, colour, entries) in [
        (
            "nintendo",
            LauncherCardId::Consoles,
            0x2a7f,
            [
                ("nes", "NES"),
                ("fds", "FDS"),
                ("snes", "SNES"),
                ("satellaview", "SATELLAVIEW"),
                ("n64", "NINTENDO 64"),
            ],
        ),
        (
            "computers",
            LauncherCardId::Computers,
            0xedc6,
            [
                ("apple-ii", "APPLE II"),
                ("c64", "COMMODORE 64"),
                ("amiga", "AMIGA"),
                ("x68000", "X68000"),
                ("unavailable", "GENERIC"),
            ],
        ),
    ] {
        let cards = entries.map(|(_, name)| LauncherCard {
            id,
            name,
            colour,
            games: Some(123),
        });
        let keys = entries.map(|(key, _)| key.to_owned());
        let pixels = mister_magik_fb::launcher_artwork::load_cards(
            &mister_magik_fb::launcher_artwork::asset_root(),
            &keys,
        );
        let sources: Vec<_> = pixels.iter().map(|pixels| pixels.as_ref()).collect();
        let data = LauncherData {
            cards: &cards,
            selected: 2,
            library_games: 0,
            collections: 0,
            favourites: 0,
            clock: "07:28",
            level: LauncherLevel::Nested(NestedLevel {
                path: &[group],
                games: 615,
                children: 5,
                children_label: "SYSTEMS",
                detail: None,
                accent: colour,
            }),
        };
        let mut prepared = scene
            .prepare_initial_with_rgb888_artwork_and_typography(data, &sources, fonts)
            .finish();
        prepared.render_frame(BrowseFrame {
            selected: 2,
            target: 2,
            phase: BrowsePhase::Settled,
            direction: None,
            progress_millis: 0,
            duration_millis: 0,
        });
        write_ppm(
            &output.join(format!("{name}-{group}.ppm")),
            scene,
            prepared.pixels(),
        )?;
    }
    Ok(())
}

fn write_ppm(
    path: &std::path::Path,
    scene: LauncherScene,
    pixels: &[Rgb565Pixel],
) -> std::io::Result<()> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(file, "P6\n{} {}\n255\n", scene.width, scene.height)?;
    for pixel in pixels {
        let p = pixel.0;
        let (r, g, b) = ((p >> 11) as u8, ((p >> 5) & 63) as u8, (p & 31) as u8);
        file.write_all(&[
            (r << 3) | (r >> 2),
            (g << 2) | (g >> 4),
            (b << 3) | (b >> 2),
        ])?;
    }
    Ok(())
}
