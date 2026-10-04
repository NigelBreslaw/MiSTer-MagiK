// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline review of installed maker cards through the production renderer.
use mister_magik_fb::bitmap_font_resource::{
    jersey_25_console_bitmap_font, launcher_bitmap_font, nocive_15_console_bitmap_font,
    spleen_6x12_native_console_bitmap_font, xerxes_10_console_bitmap_font,
};
use mister_magik_fb::launcher_artwork::{asset_root, load_artwork};
use mister_magik_framebuffer_scenes::launcher::{
    LauncherCard, LauncherCardId, LauncherData, LauncherFaceCache, LauncherLevel, LauncherScene,
    LauncherTypography, NestedLevel,
};
use mister_magik_framebuffer_scenes::launcher_navigation::{BrowseFrame, BrowsePhase};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("output directory required")?,
    );
    std::fs::create_dir_all(&out)?;
    let err = std::io::Error::other;
    let heading = launcher_bitmap_font(nocive_15_console_bitmap_font().map_err(err)?);
    let number = launcher_bitmap_font(jersey_25_console_bitmap_font().map_err(err)?);
    let metadata = launcher_bitmap_font(xerxes_10_console_bitmap_font().map_err(err)?);
    let fallback = launcher_bitmap_font(spleen_6x12_native_console_bitmap_font().map_err(err)?);
    let fonts = LauncherTypography {
        heading: &heading,
        number: &number,
        metadata: &metadata,
        fallback: &fallback,
    };
    for (group, makers) in [
        (
            "consoles",
            vec![
                ("atari", "ATARI"),
                ("sega", "SEGA"),
                ("sony", "SONY"),
                ("nintendo", "NINTENDO"),
                ("nec", "NEC"),
                ("other", "OTHER"),
            ],
        ),
        (
            "computers",
            vec![
                ("acorn", "ACORN"),
                ("apple", "APPLE"),
                ("commodore", "COMMODORE"),
                ("atari", "ATARI"),
                ("sinclair", "SINCLAIR"),
                ("tandy", "TANDY"),
                ("dos-pc", "DOS / PC"),
                ("japanese", "JAPANESE COMPUTERS"),
                ("other", "OTHER"),
            ],
        ),
        (
            "handhelds",
            vec![
                ("nintendo", "NINTENDO"),
                ("sega", "SEGA"),
                ("atari", "ATARI"),
                ("snk", "SNK"),
                ("bandai", "BANDAI"),
                ("other", "OTHER"),
            ],
        ),
    ] {
        let cards: Vec<_> = makers
            .iter()
            .map(|(_, name)| LauncherCard {
                id: LauncherCardId::Consoles,
                name,
                games: Some(123),
                colour: 0x2a7f,
            })
            .collect();
        let keys: Vec<_> = makers
            .iter()
            .map(|(key, _)| format!("menu:{group}:{key}"))
            .collect();
        for (mode, scene) in [
            ("hdmi", LauncherScene::new(960, 540)),
            ("crt", LauncherScene::crt(640, 240)),
            ("portrait", LauncherScene::new(540, 960)),
        ] {
            let mut art: Vec<_> = load_artwork(&asset_root(), &keys)
                .into_iter()
                .map(Some)
                .collect();
            let data = LauncherData {
                cards: &cards,
                selected: 0,
                library_games: 0,
                collections: 0,
                favourites: 0,
                clock: "12:00",
                level: LauncherLevel::Nested(NestedLevel {
                    path: &[group],
                    games: 123 * cards.len() as u32,
                    children: cards.len() as u32,
                    children_label: "MAKERS",
                    detail: None,
                    accent: 0x2a7f,
                }),
            };
            let mut prepared = scene
                .prepare_initial_with_rgb888_loader_and_cache(
                    data,
                    &mut |i| art[i].take().unwrap(),
                    Some(fonts),
                    &mut LauncherFaceCache::default(),
                    1,
                )
                .finish();
            for selected in [0, 3] {
                prepared.render_frame(BrowseFrame {
                    selected,
                    target: selected,
                    phase: BrowsePhase::Settled,
                    direction: None,
                    progress_millis: 0,
                    duration_millis: 0,
                });
                let mut file = std::io::BufWriter::new(std::fs::File::create(
                    out.join(format!("{group}-{mode}-{selected}.ppm")),
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
        }
    }
    Ok(())
}
