// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Minimal exFAT-aware inventory scans for installed MRAs and ROM archives.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct InstalledMra {
    pub full_path: PathBuf,
    pub relative_path: String,
    pub path_key: String,
    pub size: Option<u64>,
}

#[derive(Debug, Default)]
pub struct RomInventory {
    pub mame: HashSet<String>,
    pub hbmame: HashSet<String>,
}

pub fn scan_installed_mras(
    arcade_root: &Path,
    verify_index_size: bool,
) -> Result<Vec<InstalledMra>, String> {
    if !arcade_root.is_dir() {
        return Err(format!(
            "Arcade root is not a directory: {}",
            arcade_root.display()
        ));
    }
    let mut stack = vec![arcade_root.to_path_buf()];
    let mut installed = Vec::new();
    while let Some(directory) = stack.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| format!("read Arcade directory {}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                format!(
                    "enumerate Arcade directory {}: {error}",
                    directory.display()
                )
            })?;
        entries
            .sort_by_cached_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
        for entry in entries.into_iter().rev() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if should_ignore_component(&name) {
                continue;
            }
            let path = entry.path();
            let is_mra = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("mra"));
            let file_type = entry
                .file_type()
                .map_err(|error| format!("inspect Arcade path {}: {error}", path.display()))?;
            if is_mra && !is_non_arcade_launcher(&name) {
                if file_type.is_symlink() || !file_type.is_file() {
                    continue;
                }
                let size = if verify_index_size {
                    Some(
                        entry
                            .metadata()
                            .map_err(|error| {
                                format!("read Arcade file size {}: {error}", path.display())
                            })?
                            .len(),
                    )
                } else {
                    None
                };
                let suffix = path
                    .strip_prefix(arcade_root)
                    .map_err(|error| format!("make Arcade path relative: {error}"))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let relative_path = format!("_Arcade/{suffix}");
                installed.push(InstalledMra {
                    full_path: path,
                    path_key: relative_path.to_ascii_lowercase(),
                    relative_path,
                    size,
                });
                continue;
            }
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if directory == arcade_root
                    && (name.eq_ignore_ascii_case("media") || name.eq_ignore_ascii_case("cores"))
                {
                    continue;
                }
                stack.push(path);
                continue;
            }
        }
    }
    installed.sort_by(|left, right| left.path_key.cmp(&right.path_key));
    if installed
        .windows(2)
        .any(|pair| pair[0].path_key == pair[1].path_key)
    {
        return Err("installed Arcade paths collide case-insensitively".to_string());
    }
    Ok(installed)
}

pub fn scan_rom_inventory(
    mame_directories: &[PathBuf],
    hbmame_directories: &[PathBuf],
) -> Result<RomInventory, String> {
    Ok(RomInventory {
        mame: scan_zip_names(mame_directories)?,
        hbmame: scan_zip_names(hbmame_directories)?,
    })
}

fn scan_zip_names(directories: &[PathBuf]) -> Result<HashSet<String>, String> {
    let mut names = HashSet::new();
    for directory in directories {
        if !directory.exists() {
            continue;
        }
        if !directory.is_dir() {
            return Err(format!(
                "ROM inventory path is not a directory: {}",
                directory.display()
            ));
        }
        for entry in fs::read_dir(directory)
            .map_err(|error| format!("read ROM directory {}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| {
                format!("enumerate ROM directory {}: {error}", directory.display())
            })?;
            let path = entry.path();
            if !path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
            {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                names.insert(stem.trim().to_ascii_lowercase());
            }
        }
    }
    Ok(names)
}

fn should_ignore_component(component: &str) -> bool {
    (component.len() > 1 && component.starts_with('.'))
        || [
            ".____padding_file",
            "__macosx",
            "images",
            "manuals",
            "screenshot",
            "screenshots",
            "screenshot-magik",
            "_organized",
            "boxart",
        ]
        .iter()
        .any(|ignored| component.eq_ignore_ascii_case(ignored))
}

fn is_non_arcade_launcher(file_name: &str) -> bool {
    file_name.eq_ignore_ascii_case("NeoGeo Pocket.mra")
        || file_name.eq_ignore_ascii_case("NeoGeo Pocket Color.mra")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "arcade-catalog-prototype-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn inventory_prunes_non_game_trees_and_symlinks() {
        let root = fixture_root("walk");
        fs::create_dir_all(root.join("Alternatives")).unwrap();
        fs::create_dir_all(root.join("_Organized")).unwrap();
        fs::create_dir_all(root.join("media")).unwrap();
        fs::create_dir_all(root.join("Not A Game.mra")).unwrap();
        fs::write(root.join("Puck Man.mra"), "mra").unwrap();
        fs::write(root.join("Alternatives/Other.MRA"), "mra2").unwrap();
        fs::write(root.join("_Organized/Duplicate.mra"), "mra").unwrap();
        fs::write(root.join("media/Launcher.mra"), "mra").unwrap();
        fs::write(root.join("NeoGeo Pocket.mra"), "mra").unwrap();

        let installed = scan_installed_mras(&root, false).unwrap();

        fs::remove_dir_all(root).unwrap();
        assert_eq!(installed.len(), 2);
        assert_eq!(installed[0].relative_path, "_Arcade/Alternatives/Other.MRA");
        assert_eq!(installed[1].relative_path, "_Arcade/Puck Man.mra");
    }

    #[test]
    fn rom_inventory_is_case_insensitive_and_shallow() {
        let root = fixture_root("roms");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("PUCKMAN.ZIP"), "zip").unwrap();
        fs::write(root.join("readme.txt"), "text").unwrap();
        fs::write(root.join("nested/hidden.zip"), "zip").unwrap();

        let inventory = scan_rom_inventory(std::slice::from_ref(&root), &[]).unwrap();

        fs::remove_dir_all(root).unwrap();
        assert!(inventory.mame.contains("puckman"));
        assert!(!inventory.mame.contains("hidden"));
    }
}
