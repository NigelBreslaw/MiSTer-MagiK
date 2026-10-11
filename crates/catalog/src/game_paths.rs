// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! MiSTer Main's `findGamesDir` storage order, remappable for host fixtures.

use std::path::{Path, PathBuf};

pub(crate) fn search_roots(storage_root: &Path) -> Vec<PathBuf> {
    // Explicit host library/game roots retain their configured scope. An
    // installed `media/fat` layout uses its sibling USB and network mounts.
    if storage_root.file_name().is_some_and(|name| name == "games") {
        return vec![storage_root.to_path_buf()];
    }
    if !storage_root.file_name().is_some_and(|name| name == "fat") {
        return vec![storage_root.join("games")];
    }
    let media = storage_root.parent().unwrap_or(storage_root);
    let volumes = (0..6)
        .map(|index| media.join(format!("usb{index}")))
        .chain([
            media.join("network"),
            storage_root.join("cifs"),
            storage_root.to_path_buf(),
        ]);
    volumes
        .flat_map(|volume| [volume.clone(), volume.join("games")])
        .collect()
}

pub fn game_dir(storage_root: &Path, name: &str) -> PathBuf {
    search_roots(storage_root)
        .into_iter()
        .map(|root| root.join(name))
        .find(|path| path.is_dir())
        .unwrap_or_else(|| storage_root.join("games").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_temp_dir;

    #[test]
    fn usb_priority_matches_main_and_empty_directories_still_shadow() {
        let fixture = unique_temp_dir("usb-game-paths");
        let fat = fixture.join("media/fat");
        let roots = search_roots(&fat);
        assert_eq!(roots.len(), 18);
        assert_eq!(roots[0], fixture.join("media/usb0"));
        assert_eq!(roots[1], fixture.join("media/usb0/games"));
        assert_eq!(roots[2], fixture.join("media/usb1"));
        assert_eq!(roots[12], fixture.join("media/network"));
        assert_eq!(roots[14], fat.join("cifs"));
        assert_eq!(roots[16], fat);
        // Create in reverse order to prove selection is independent of readdir.
        for root in roots.iter().rev() {
            std::fs::create_dir_all(root.join("SNES")).unwrap();
        }
        for expected in &roots {
            assert_eq!(game_dir(&fat, "SNES"), expected.join("SNES"));
            std::fs::remove_dir_all(expected.join("SNES")).unwrap();
        }
        assert_eq!(game_dir(&fat, "SNES"), fat.join("games/SNES"));
        std::fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn files_do_not_shadow_directories_and_selection_is_per_system() {
        let fixture = unique_temp_dir("usb-game-path-files");
        let fat = fixture.join("media/fat");
        std::fs::create_dir_all(fixture.join("media/usb0")).unwrap();
        std::fs::write(fixture.join("media/usb0/NES"), b"file").unwrap();
        let usb_nes = fixture.join("media/usb0/games/NES");
        let sd_snes = fat.join("games/SNES");
        std::fs::create_dir_all(&usb_nes).unwrap();
        std::fs::create_dir_all(&sd_snes).unwrap();
        assert_eq!(game_dir(&fat, "NES"), usb_nes);
        assert_eq!(game_dir(&fat, "SNES"), sd_snes);
        assert_eq!(search_roots(&fixture), [fixture.join("games")]);
        std::fs::remove_dir_all(fixture).unwrap();
    }
}
