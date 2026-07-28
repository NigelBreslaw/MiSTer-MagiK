// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deterministic in-memory content supplied to the macOS launcher adapter.

use crate::arcade_catalog::{ArcadeCatalog, ArcadeGameEntry, GameSystemEntry};
use mister_magik_catalog::catalog_classify::system_definitions;
use slint::platform::software_renderer::Rgb565Pixel;
use std::path::PathBuf;
use std::sync::Arc;

pub const FIXTURE_ARCADE_GAME_COUNT: usize = 48;
pub const FIXTURE_SCREENSHOT_WIDTH: usize = 160;
pub const FIXTURE_SCREENSHOT_HEIGHT: usize = 120;

pub struct UiPreviewFixtures {
    pub catalog: ArcadeCatalog,
    pub screenshots: Vec<FixtureScreenshot>,
    pub shell_system_ids: Vec<String>,
}

#[derive(Clone)]
pub struct FixtureScreenshot {
    pub key: Arc<str>,
    pub pixels: Vec<Rgb565Pixel>,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl UiPreviewFixtures {
    pub fn new() -> Result<Self, String> {
        let games = fixture_arcade_games();
        let definitions = system_definitions()?;
        let systems = definitions
            .iter()
            .map(|definition| GameSystemEntry {
                id: definition.id.clone(),
                title: definition.title.clone(),
                count: usize::from(definition.id == "arcade") * games.len(),
            })
            .collect::<Vec<_>>();
        let shell_system_ids = systems
            .iter()
            .filter(|system| system.count == 0)
            .map(|system| system.id.clone())
            .collect();
        let screenshots = (0..FIXTURE_ARCADE_GAME_COUNT)
            .map(fixture_screenshot)
            .collect();
        Ok(Self {
            catalog: ArcadeCatalog::new(PathBuf::from("/fixture"), games, systems),
            screenshots,
            shell_system_ids,
        })
    }

    pub fn arcade_games(&self) -> &[ArcadeGameEntry] {
        self.catalog.games.as_slice()
    }

    pub fn screenshot(&self, key: &str) -> Option<&FixtureScreenshot> {
        self.screenshots
            .iter()
            .find(|screenshot| screenshot.key.as_ref() == key)
    }
}

fn fixture_arcade_games() -> Vec<ArcadeGameEntry> {
    const TITLES: [&str; 24] = [
        "1942",
        "Alien Syndrome",
        "Bubble Bobble",
        "Centipede",
        "Donkey Kong",
        "Elevator Action",
        "Frogger",
        "Galaga",
        "Hyper Sports",
        "Ikari Warriors",
        "Joust",
        "Klax",
        "Metal Slug",
        "Out Run",
        "Pac-Man",
        "R-Type",
        "Robotron: 2084",
        "Space Harrier",
        "Tempest",
        "Ultimate Ghosts 'n Goblins",
        "Vanguard",
        "Wonder Boy",
        "Xevious",
        "Zaxxon",
    ];
    (0..FIXTURE_ARCADE_GAME_COUNT)
        .map(|index| {
            let title = TITLES.get(index).map_or_else(
                || format!("Fixture Arcade Game {:02}", index + 1),
                |title| (*title).to_string(),
            );
            ArcadeGameEntry {
                title: Arc::from(title),
                mra_path: Arc::from(format!("/fixture/arcade/{index:02}.mra")),
                preview_archive_path: Arc::from("/fixture/arcade-screenshots.zip"),
                preview_asset_key: Arc::from(format!("fixture-{index:02}")),
                has_preview: true,
                system_id: Arc::from("arcade"),
                year: Some(1978 + (index % 24) as u16),
                manufacturer: Arc::from(match index % 4 {
                    0 => "Atari",
                    1 => "Namco",
                    2 => "Sega",
                    _ => "Taito",
                }),
                players: Some(if index % 3 == 0 { 2 } else { 1 }),
                control: Arc::from(if index % 5 == 0 {
                    "Trackball"
                } else {
                    "Joystick"
                }),
                is_new: index < 4,
            }
        })
        .collect()
}

fn fixture_screenshot(index: usize) -> FixtureScreenshot {
    let width = FIXTURE_SCREENSHOT_WIDTH;
    let height = FIXTURE_SCREENSHOT_HEIGHT;
    let mut pixels = vec![Rgb565Pixel(0); width * height];
    let accent = rgb565(
        ((index * 47 + 32) & 0xff) as u8,
        ((index * 83 + 64) & 0xff) as u8,
        ((index * 29 + 96) & 0xff) as u8,
    );
    let secondary = rgb565(
        ((index * 19 + 140) & 0xff) as u8,
        ((index * 31 + 72) & 0xff) as u8,
        ((index * 61 + 28) & 0xff) as u8,
    );
    for y in 0..height {
        for x in 0..width {
            let checker = ((x / 10) + (y / 10) + index) & 1;
            let horizon = ((y * 31 / height) as u16) << 11;
            let grid = ((x * 63 / width) as u16) << 5;
            pixels[y * width + x] = if checker == 0 {
                Rgb565Pixel(horizon | grid | ((index * 3) as u16 & 0x1f))
            } else {
                Rgb565Pixel((horizon >> 1) | (grid >> 1) | 4)
            };
        }
    }
    let inset = 12 + index % 18;
    for y in (18 + index % 12)..(height - 18) {
        for x in inset..(width - inset) {
            let diagonal = (x + y + index * 7) % 23;
            if diagonal < 5 {
                pixels[y * width + x] = accent;
            } else if diagonal > 19 {
                pixels[y * width + x] = secondary;
            }
        }
    }
    FixtureScreenshot {
        key: Arc::from(format!("fixture-{index:02}")),
        pixels,
        width,
        height,
        stride: width,
    }
}

fn rgb565(red: u8, green: u8, blue: u8) -> Rgb565Pixel {
    Rgb565Pixel((u16::from(red) >> 3) << 11 | (u16::from(green) >> 2) << 5 | (u16::from(blue) >> 3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_arcade_has_fixture_content() {
        let fixtures = UiPreviewFixtures::new().expect("preview fixtures");
        assert_eq!(
            fixtures.catalog.system_game_count("arcade"),
            FIXTURE_ARCADE_GAME_COUNT
        );
        assert!(
            fixtures
                .catalog
                .systems
                .iter()
                .filter(|system| system.id != "arcade")
                .all(|system| fixtures.catalog.system_game_count(&system.id) == 0)
        );
    }

    #[test]
    fn every_arcade_game_has_a_distinct_screenshot_key() {
        let fixtures = UiPreviewFixtures::new().expect("preview fixtures");
        assert_eq!(fixtures.screenshots.len(), FIXTURE_ARCADE_GAME_COUNT);
        for game in fixtures.arcade_games() {
            assert!(fixtures.screenshot(&game.preview_asset_key).is_some());
        }
    }
}
