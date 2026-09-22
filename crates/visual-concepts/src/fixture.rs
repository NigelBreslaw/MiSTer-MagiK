// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic artwork and text fixtures, prepared once, independent of catalog state.
use crate::{Pixel, Rect, rgb};
use mister_magik_framebuffer_scenes::{
    bitmap_text::{BitmapFont, BitmapGlyph},
    launcher::{
        LauncherCard, LauncherCardId, LauncherData, LauncherScene, LauncherTypography,
        PreparedLauncher,
    },
};
const ART: [&[u8]; 6] = [
    include_bytes!("../../../apps/mister/assets/ui/launcher-cards/01_arcade.rgb565"),
    include_bytes!("../../../apps/mister/assets/ui/launcher-cards/02_consoles.rgb565"),
    include_bytes!("../../../apps/mister/assets/ui/launcher-cards/03_computers.rgb565"),
    include_bytes!("../../../apps/mister/assets/ui/launcher-cards/04_handhelds.rgb565"),
    include_bytes!("../../../apps/mister/assets/ui/launcher-cards/05_favourites.rgb565"),
    include_bytes!("../../../apps/mister/assets/ui/launcher-cards/06_settings.rgb565"),
];
fn font(scale: usize) -> BitmapFont {
    let source = include_str!("../../../apps/mister/assets/fonts/spleen/spleen-6x12.bdf");
    let mut glyphs = Vec::new();
    for chunk in source.split("STARTCHAR ").skip(1) {
        let mut code = None;
        let mut rows = Vec::new();
        let mut bitmap = false;
        for line in chunk.lines() {
            if let Some(v) = line.strip_prefix("ENCODING ") {
                code = v.parse::<u32>().ok();
            }
            if line == "ENDCHAR" {
                break;
            }
            if line == "BITMAP" {
                bitmap = true;
                continue;
            }
            if bitmap && let Ok(bits) = u8::from_str_radix(line, 16) {
                rows.push(bits);
            }
        }
        if let Some(code) = code.filter(|c| *c >= 32 && *c < 127) {
            let mut alpha = vec![0; 6 * 12 * scale * scale];
            for y in 0..12 * scale {
                for x in 0..6 * scale {
                    if rows
                        .get(y / scale)
                        .is_some_and(|bits| bits & (128 >> (x / scale)) != 0)
                    {
                        alpha[y * 6 * scale + x] = 255;
                    }
                }
            }
            glyphs.push(BitmapGlyph {
                code_point: char::from_u32(code).unwrap(),
                left: 0,
                top: (12 * scale) as i32,
                width: 6 * scale,
                height: 12 * scale,
                advance: (6 * scale) as i32,
                alpha,
            });
        }
    }
    glyphs.sort_by_key(|g| g.code_point);
    BitmapFont {
        ascent: (12 * scale) as i32,
        descent: 0,
        glyphs,
    }
}
fn prepare(width: usize, height: usize) -> PreparedLauncher {
    let cards = [
        LauncherCard {
            id: LauncherCardId::Arcade,
            name: "ARCADE",
            games: Some(987),
            colour: rgb(230, 40, 55).0,
        },
        LauncherCard {
            id: LauncherCardId::Consoles,
            name: "CONSOLES",
            games: Some(8421),
            colour: rgb(64, 140, 255).0,
        },
        LauncherCard {
            id: LauncherCardId::Computers,
            name: "COMPUTERS",
            games: Some(6140),
            colour: rgb(160, 170, 100).0,
        },
        LauncherCard {
            id: LauncherCardId::Handhelds,
            name: "HANDHELDS",
            games: Some(921),
            colour: rgb(90, 180, 100).0,
        },
        LauncherCard {
            id: LauncherCardId::Favourites,
            name: "FAVOURITES",
            games: Some(42),
            colour: rgb(235, 70, 130).0,
        },
        LauncherCard {
            id: LauncherCardId::Settings,
            name: "SETTINGS",
            games: None,
            colour: rgb(150, 60, 220).0,
        },
    ];
    let art = ART.map(|bytes| {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| Pixel(u16::from_le_bytes([p[0], p[1]])))
            .collect::<Vec<_>>()
    });
    let refs = art.each_ref().map(|a| a.as_slice());
    let small = font(1);
    let large = font(2);
    LauncherScene::new(width, height)
        .prepare_initial_with_artwork_and_typography(
            LauncherData {
                cards: &cards,
                selected: 0,
                library_games: 35216,
                collections: 77,
                favourites: 42,
                clock: "12:35",
            },
            &refs,
            LauncherTypography {
                heading: &small,
                number: &large,
                metadata: &small,
                fallback: &small,
            },
        )
        .finish()
}
pub struct Fixture {
    pub width: usize,
    pub height: usize,
    pub base: Vec<Pixel>,
    pub card: Rect,
    pub content: Rect,
    pub floor: Rect,
}
impl Fixture {
    pub fn new(width: usize, height: usize) -> Self {
        let base = prepare(width, height).pixels().to_vec();
        let mut result = Self {
            width,
            height,
            base,
            card: crate::full(1, 1),
            content: crate::full(1, 1),
            floor: crate::full(1, 1),
        };
        result.card = result.rect(520, 158, 700, 410);
        result.content = result.rect(296, 120, 934, 495);
        result.floor = result.rect(296, 412, 934, 477);
        result
    }
    pub fn list(&self) -> Vec<Pixel> {
        let mut list = self.base.clone();
        let width = self.width;
        let height = self.height;
        let r = self.content;
        for y in r.y0..r.y1 {
            list[y * width + r.x0..y * width + r.x1].fill(Pixel(0));
        }
        let face = font(2);
        for (i, name) in [
            "1942",
            "AFTER BURNER",
            "ARKANOID",
            "BUBBLE BOBBLE",
            "CONTRA",
            "FINAL FIGHT",
            "GRADIUS",
            "METAL SLUG",
            "OUT RUN",
            "R-TYPE",
        ]
        .iter()
        .enumerate()
        {
            let p = self.rect(320, 130 + i * 31, 900, 150 + i * 31);
            face.draw(
                &mut list,
                width,
                height,
                p.x0 as i32,
                p.y0 as i32,
                name,
                rgb(238, 232, 213).0,
            );
        }
        list
    }
    pub fn rect(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Rect {
        let scale = (self.width as f64 / 960.0).min(self.height as f64 / 540.0);
        let ox = (self.width as f64 - 960.0 * scale) / 2.0;
        let oy = (self.height as f64 - 540.0 * scale) / 2.0;
        Rect {
            x0: (ox + x0 as f64 * scale) as usize,
            y0: (oy + y0 as f64 * scale) as usize,
            x1: ((ox + x1 as f64 * scale) as usize).min(self.width),
            y1: ((oy + y1 as f64 * scale) as usize).min(self.height),
        }
    }
    pub fn storage_bytes(&self) -> usize {
        self.base.capacity() * 2
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures_preserve_chrome() {
        for h in [540, 600] {
            let f = Fixture::new(960, h);
            let list = f.list();
            let r = f.content;
            for y in 0..h {
                for x in 0..960 {
                    if x < r.x0 || x >= r.x1 || y < r.y0 || y >= r.y1 {
                        assert_eq!(f.base[y * 960 + x], list[y * 960 + x]);
                    }
                }
            }
            assert_ne!(f.base, list);
        }
    }
}
