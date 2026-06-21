//! Arcade catalog helpers.
//!
//! The runtime launcher catalog is SQLite-backed. This module keeps the shared
//! in-memory catalog types and presentation helpers used by the SQLite loader.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

pub const DEFAULT_ARCADE_ROOT: &str = "/media/fat/_Arcade";

/// Logical row height for the Rust-painted arcade list viewport.
pub const ARCADE_ROW_HEIGHT: i32 = 48;
/// Visible list height: 8 exact arcade rows (matches `arcade_list.slint` left pane).
pub const ARCADE_LIST_VISIBLE_H: i32 = 384;
pub const HOME_TILE_WIDTH: i32 = 220;
pub const HOME_TILE_GAP: i32 = 16;
pub const HOME_LIST_VISIBLE_W: i32 = 912;

#[derive(Clone, Debug)]
pub struct ArcadeGameEntry {
    pub title: Arc<str>,
    pub mra_path: Arc<str>,
    pub preview_archive_path: Arc<str>,
    pub preview_asset_key: Arc<str>,
    pub has_preview: bool,
    pub system_id: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSystemEntry {
    pub id: String,
    pub title: String,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct ArcadeCatalog {
    pub root: PathBuf,
    pub games: Vec<ArcadeGameEntry>,
    pub systems: Vec<GameSystemEntry>,
    games_by_system: HashMap<String, Vec<ArcadeGameEntry>>,
    preview_games_by_system: HashMap<String, Vec<ArcadeGameEntry>>,
}

impl ArcadeCatalog {
    pub fn new(root: PathBuf, games: Vec<ArcadeGameEntry>, systems: Vec<GameSystemEntry>) -> Self {
        let games_by_system = games_by_system(&games);
        let preview_games_by_system = preview_games_by_system(&games);
        Self {
            root,
            games,
            systems,
            games_by_system,
            preview_games_by_system,
        }
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    pub fn title_for_path(&self, mra_path: &str) -> &str {
        self.games
            .iter()
            .find(|g| g.mra_path.as_ref() == mra_path)
            .map(|g| g.title.as_ref())
            .unwrap_or("Game")
    }

    pub fn system_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.system_game_slice(system_id).to_vec()
    }

    pub fn system_game_count(&self, system_id: &str) -> usize {
        self.system_game_slice(system_id).len()
    }

    pub fn system_game_at(&self, system_id: &str, index: usize) -> Option<&ArcadeGameEntry> {
        self.system_game_slice(system_id).get(index)
    }

    pub fn system_game_slice(&self, system_id: &str) -> &[ArcadeGameEntry] {
        self.games_by_system
            .get(system_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn system_preview_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.system_preview_game_slice(system_id).to_vec()
    }

    pub fn system_preview_game_count(&self, system_id: &str) -> usize {
        self.system_preview_game_slice(system_id).len()
    }

    pub fn system_preview_game_at(&self, system_id: &str, index: usize) -> Option<ArcadeGameEntry> {
        self.system_preview_game_slice(system_id)
            .get(index)
            .cloned()
    }

    pub fn system_preview_game_slice(&self, system_id: &str) -> &[ArcadeGameEntry] {
        self.preview_games_by_system
            .get(system_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

fn games_by_system(games: &[ArcadeGameEntry]) -> HashMap<String, Vec<ArcadeGameEntry>> {
    let mut by_system: HashMap<String, Vec<ArcadeGameEntry>> = HashMap::new();
    for game in games {
        by_system
            .entry(game.system_id.to_string())
            .or_default()
            .push(game.clone());
    }
    by_system
}

fn preview_games_by_system(games: &[ArcadeGameEntry]) -> HashMap<String, Vec<ArcadeGameEntry>> {
    let mut by_system: HashMap<String, Vec<&ArcadeGameEntry>> = HashMap::new();
    for game in games {
        by_system
            .entry(game.system_id.to_string())
            .or_default()
            .push(game);
    }
    by_system
        .into_iter()
        .map(|(system_id, games)| (system_id, preview_games(games.into_iter())))
        .collect()
}

fn preview_games<'a>(games: impl Iterator<Item = &'a ArcadeGameEntry>) -> Vec<ArcadeGameEntry> {
    let mut best_idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<ArcadeGameEntry> = Vec::new();

    for game in games {
        if !has_preview_image(game) {
            continue;
        }
        let key = preview_dedupe_key(&game.title);
        if let Some(&idx) = best_idx.get(&key) {
            if prefer_preview_game(game, &out[idx]) {
                out[idx] = game.clone();
            }
        } else {
            best_idx.insert(key, out.len());
            out.push(game.clone());
        }
    }

    out
}

fn preview_dedupe_key(title: &str) -> String {
    let base = title
        .split_once('(')
        .map(|(before, _)| before.trim())
        .filter(|before| !before.is_empty())
        .unwrap_or(title);
    base.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn prefer_preview_game(a: &ArcadeGameEntry, b: &ArcadeGameEntry) -> bool {
    let a_exact = !a.title.contains('(');
    let b_exact = !b.title.contains('(');
    if a_exact != b_exact {
        return a_exact;
    }
    if a.title.len() != b.title.len() {
        return a.title.len() < b.title.len();
    }
    a.mra_path < b.mra_path
}

fn has_preview_image(game: &ArcadeGameEntry) -> bool {
    game.has_preview && !game.preview_archive_path.is_empty() && !game.preview_asset_key.is_empty()
}

pub fn systems_from_games(games: &[ArcadeGameEntry]) -> Vec<GameSystemEntry> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for game in games {
        *counts.entry(game.system_id.to_string()).or_default() += 1;
    }
    let mut systems: Vec<GameSystemEntry> = counts
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(id, count)| GameSystemEntry {
            title: system_title(&id),
            id,
            count,
        })
        .collect();
    systems.sort_by_cached_key(system_sort_key);
    systems
}

fn system_sort_key(system: &GameSystemEntry) -> String {
    let rank = match system.id.as_str() {
        "arcade" => 0,
        "amiga" => 1,
        "neogeo" => 2,
        "nes" => 3,
        "snes" => 4,
        "saturn" => 5,
        "megadrive" => 6,
        "gba" => 7,
        "gbc" => 8,
        "n64" => 9,
        "gamegear" => 10,
        "vectrex" => 11,
        "ao486" => 12,
        "dos" => 13,
        "unknown" => 999,
        _ => 100,
    };
    format!("{rank:03}-{}", system.title.to_lowercase())
}

pub fn system_title(id: &str) -> String {
    match id {
        "arcade" => "Arcade".to_string(),
        "neogeo" | "neo-geo" | "snk-neo-geo" => "NeoGeo".to_string(),
        "cps1" | "capcom-cps1" => "CPS1".to_string(),
        "cps2" | "capcom-cps2" => "CPS2".to_string(),
        "cps3" | "capcom-cps3" => "CPS3".to_string(),
        "system16" | "sega-system16" => "System 16".to_string(),
        "system18" | "sega-system18" => "System 18".to_string(),
        "m72" | "irem-m72" => "Irem M72".to_string(),
        "m92" | "irem-m92" => "Irem M92".to_string(),
        "gba" => "GBA".to_string(),
        "gbc" => "GBC".to_string(),
        "gb" => "GB".to_string(),
        "nes" => "NES".to_string(),
        "snes" => "SNES".to_string(),
        "n64" => "N64".to_string(),
        "sms" => "SMS".to_string(),
        "psx" => "PSX".to_string(),
        "ao486" => "ao486".to_string(),
        "dos" => "DOS Games".to_string(),
        "megadrive" => "Mega Drive".to_string(),
        "megacd" => "Mega CD".to_string(),
        "gamegear" => "Game Gear".to_string(),
        "unknown" => "Unknown".to_string(),
        other => other
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(
        title: &str,
        mra_path: &str,
        preview_asset_key: &str,
        system_id: &str,
    ) -> ArcadeGameEntry {
        let has_preview = !preview_asset_key.is_empty();
        ArcadeGameEntry {
            title: title.into(),
            mra_path: mra_path.into(),
            preview_archive_path: if has_preview {
                "/media/fat/_Arcade/media/screenshot-magik/320x320-screenshots.mmlz4b".into()
            } else {
                "".into()
            },
            preview_asset_key: preview_asset_key.into(),
            has_preview,
            system_id: system_id.into(),
        }
    }

    #[test]
    fn preview_games_require_images_and_collapse_parenthetical_clones() {
        let root = PathBuf::from("/media/fat/_Arcade");
        let systems = vec![GameSystemEntry {
            id: "arcade".into(),
            title: "Arcade".into(),
            count: 5,
        }];
        let games = vec![
            ArcadeGameEntry {
                title: "1941: Counter Attack (Japan)".into(),
                mra_path: "/media/fat/_Arcade/1941 Japan.mra".into(),
                preview_archive_path:
                    "/media/fat/_Arcade/media/screenshot-magik/320x320-screenshots.mmlz4b".into(),
                preview_asset_key: "1941u".into(),
                has_preview: true,
                system_id: "arcade".into(),
            },
            ArcadeGameEntry {
                title: "1941: Counter Attack (World)".into(),
                mra_path: "/media/fat/_Arcade/1941 World.mra".into(),
                preview_archive_path:
                    "/media/fat/_Arcade/media/screenshot-magik/320x320-screenshots.mmlz4b".into(),
                preview_asset_key: "1941u".into(),
                has_preview: true,
                system_id: "arcade".into(),
            },
            ArcadeGameEntry {
                title: "1942".into(),
                mra_path: "/media/fat/_Arcade/1942.mra".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "arcade".into(),
            },
            ArcadeGameEntry {
                title: "1943".into(),
                mra_path: "/media/fat/_Arcade/1943.mra".into(),
                preview_archive_path:
                    "/media/fat/_Arcade/media/screenshot-magik/320x320-screenshots.mmlz4b".into(),
                preview_asset_key: "1943".into(),
                has_preview: true,
                system_id: "arcade".into(),
            },
            ArcadeGameEntry {
                title: "Astra SuperStars".into(),
                mra_path: "/media/fat/_Arcade/Astra SuperStars.mra".into(),
                preview_archive_path:
                    "/media/fat/_Arcade/media/screenshot-magik/320x320-screenshots.mmlz4b".into(),
                preview_asset_key: "astrass".into(),
                has_preview: true,
                system_id: "arcade".into(),
            },
        ];
        let catalog = ArcadeCatalog::new(root, games, systems);

        let games = catalog.system_preview_games("arcade");
        assert_eq!(games.len(), 3);
        assert_eq!(catalog.system_preview_game_count("arcade"), 3);
        assert_eq!(games[0].title.as_ref(), "1941: Counter Attack (Japan)");
        assert_eq!(games[1].title.as_ref(), "1943");
        assert_eq!(games[2].title.as_ref(), "Astra SuperStars");
        assert_eq!(
            catalog
                .system_preview_game_at("arcade", 1)
                .map(|game| game.title.to_string()),
            Some("1943".to_string())
        );
    }

    #[test]
    fn system_game_count_includes_games_without_preview_assets() {
        let root = PathBuf::from("/media/fat/_Arcade");
        let systems = vec![GameSystemEntry {
            id: "amiga".into(),
            title: "Amiga".into(),
            count: 1,
        }];
        let games = vec![ArcadeGameEntry {
            title: "Agony".into(),
            mra_path: "magik-plan:amiga-agony".into(),
            preview_archive_path: "".into(),
            preview_asset_key: "".into(),
            has_preview: false,
            system_id: "amiga".into(),
        }];
        let catalog = ArcadeCatalog::new(root, games, systems);

        assert_eq!(catalog.system_game_count("amiga"), 1);
        assert_eq!(catalog.system_game_slice("amiga").len(), 1);
        assert_eq!(catalog.system_preview_game_count("amiga"), 0);
    }

    #[test]
    fn catalog_lookup_falls_back_cleanly_for_missing_paths_and_systems() {
        let catalog = ArcadeCatalog::new(
            PathBuf::from("/media/fat/_Arcade"),
            vec![ArcadeGameEntry {
                title: "1942".into(),
                mra_path: "/media/fat/_Arcade/1942.mra".into(),
                preview_archive_path:
                    "/media/fat/_Arcade/media/screenshot-magik/320x320-screenshots.mmlz4b".into(),
                preview_asset_key: "1942".into(),
                has_preview: true,
                system_id: "arcade".into(),
            }],
            vec![GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 1,
            }],
        );

        assert_eq!(catalog.len(), 1);
        assert!(!catalog.is_empty());
        assert_eq!(catalog.title_for_path("/missing.mra"), "Game");
        assert!(catalog.system_games("missing").is_empty());
        assert_eq!(catalog.system_game_count("missing"), 0);
        assert!(catalog.system_game_at("missing", 0).is_none());
        assert!(catalog.system_preview_games("missing").is_empty());
        assert_eq!(catalog.system_preview_game_count("missing"), 0);
        assert!(catalog.system_preview_game_at("missing", 0).is_none());
    }

    #[test]
    fn preview_games_prefer_exact_or_shorter_title_for_same_family() {
        let games = [
            game(
                "Puzzle Star (World)",
                "/games/puzzle-world.mra",
                "puzzle-world",
                "arcade",
            ),
            game("Puzzle Star", "/games/puzzle.mra", "puzzle", "arcade"),
            game(
                "Space   Duel Alpha",
                "/games/space-extended.mra",
                "space-extended",
                "arcade",
            ),
            game("Space Duel Alpha", "/games/space.mra", "space", "arcade"),
        ];

        let previews = preview_games(games.iter());

        assert_eq!(
            previews
                .iter()
                .map(|game| game.title.as_ref())
                .collect::<Vec<_>>(),
            vec!["Puzzle Star", "Space Duel Alpha"]
        );
    }

    #[test]
    fn preview_games_require_preview_archive_and_asset_key() {
        let games = [
            game("Still Image", "/games/still.mra", "", "arcade"),
            game("Photo", "/games/photo.mra", "photo", "arcade"),
        ];

        let previews = preview_games(games.iter());

        assert_eq!(previews.len(), 1);
        assert_eq!(previews[0].title.as_ref(), "Photo");
    }

    #[test]
    fn systems_from_games_uses_runtime_order_and_human_titles() {
        let games = vec![
            ArcadeGameEntry {
                title: "Unknown Thing".into(),
                mra_path: "/media/fat/_Arcade/Unknown.mra".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "unknown".into(),
            },
            ArcadeGameEntry {
                title: "Sonic".into(),
                mra_path: "/media/fat/games/MegaDrive/Sonic.md".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "megadrive".into(),
            },
            ArcadeGameEntry {
                title: "1942".into(),
                mra_path: "/media/fat/_Arcade/1942.mra".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "arcade".into(),
            },
            ArcadeGameEntry {
                title: "Another Sonic".into(),
                mra_path: "/media/fat/games/MegaDrive/Another Sonic.md".into(),
                preview_archive_path: "".into(),
                preview_asset_key: "".into(),
                has_preview: false,
                system_id: "megadrive".into(),
            },
        ];

        let systems = systems_from_games(&games);

        assert_eq!(
            systems,
            vec![
                GameSystemEntry {
                    id: "arcade".into(),
                    title: "Arcade".into(),
                    count: 1
                },
                GameSystemEntry {
                    id: "megadrive".into(),
                    title: "Mega Drive".into(),
                    count: 2
                },
                GameSystemEntry {
                    id: "unknown".into(),
                    title: "Unknown".into(),
                    count: 1
                }
            ]
        );
    }
}
