// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read-only MiSTer card discovery and Mac-local preview storage.

use crc32fast::Hasher;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};

const CANONICAL_DEVICE_ROOT: &str = "/media/fat";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContentMode {
    #[default]
    Auto,
    Fixtures,
    Card,
}

impl ContentMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "fixtures" | "fixture" => Ok(Self::Fixtures),
            "card" | "sd" => Ok(Self::Card),
            _ => Err(format!(
                "invalid content mode {value:?}; expected auto, fixtures, or card"
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewContent {
    Fixtures,
    Card(HostContentLayout),
}

impl PreviewContent {
    pub fn label(&self) -> String {
        match self {
            Self::Fixtures => "fixtures".to_owned(),
            Self::Card(layout) => format!("card:{}", layout.volume_label),
        }
    }

    pub fn card(&self) -> Option<&HostContentLayout> {
        match self {
            Self::Fixtures => None,
            Self::Card(layout) => Some(layout),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostContentLayout {
    pub card_root: PathBuf,
    pub cache_root: PathBuf,
    pub catalog_root: PathBuf,
    pub media_root: PathBuf,
    pub work_root: PathBuf,
    pub volume_label: String,
    pub volume_key: String,
}

impl HostContentLayout {
    pub fn new(card_root: impl AsRef<Path>, cache_base: impl AsRef<Path>) -> Result<Self, String> {
        let card_root = fs::canonicalize(card_root.as_ref()).map_err(|error| {
            format!(
                "resolve MiSTer card root {}: {error}",
                card_root.as_ref().display()
            )
        })?;
        validate_card_root(&card_root)?;
        let volume_label = card_root
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("MiSTer")
            .to_owned();
        let volume_key = volume_key(&volume_label, &card_root);
        let cache_root = cache_base.as_ref().join("cards").join(&volume_key);
        Ok(Self {
            card_root,
            catalog_root: cache_root.join("catalog-v3"),
            media_root: cache_root.join("assets"),
            work_root: cache_root.join("work"),
            cache_root,
            volume_label,
            volume_key,
        })
    }

    pub fn to_canonical(&self, physical: impl AsRef<Path>) -> Result<PathBuf, String> {
        let physical = normalize_without_parent_components(physical.as_ref())?;
        let physical = canonicalize_existing_ancestor(&physical)?;
        let relative = physical.strip_prefix(&self.card_root).map_err(|_| {
            format!(
                "{} is outside MiSTer card {}",
                physical.display(),
                self.card_root.display()
            )
        })?;
        Ok(Path::new(CANONICAL_DEVICE_ROOT).join(relative))
    }

    pub fn to_card_path(&self, canonical: impl AsRef<Path>) -> Result<PathBuf, String> {
        let canonical = normalize_without_parent_components(canonical.as_ref())?;
        let relative = canonical.strip_prefix(CANONICAL_DEVICE_ROOT).map_err(|_| {
            format!(
                "{} is not beneath {CANONICAL_DEVICE_ROOT}",
                canonical.display()
            )
        })?;
        Ok(self.card_root.join(relative))
    }

    pub fn cache_path(&self, relative: impl AsRef<Path>) -> Result<PathBuf, String> {
        let relative = normalize_relative(relative.as_ref())?;
        Ok(self.cache_root.join(relative))
    }
}

fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf, String> {
    let mut current = path;
    let mut missing = Vec::<OsString>::new();
    loop {
        match fs::canonicalize(current) {
            Ok(mut canonical) => {
                for component in missing.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = current.file_name() else {
                    return Err(format!("resolve path {}: {error}", path.display()));
                };
                missing.push(name.to_os_string());
                let Some(parent) = current.parent() else {
                    return Err(format!("resolve path {}: {error}", path.display()));
                };
                current = parent;
            }
            Err(error) => return Err(format!("resolve path {}: {error}", path.display())),
        }
    }
}

pub fn resolve_preview_content(
    mode: ContentMode,
    explicit_root: Option<&Path>,
    cache_root: Option<&Path>,
    headless: bool,
) -> Result<PreviewContent, String> {
    if mode == ContentMode::Fixtures || (mode == ContentMode::Auto && headless) {
        if explicit_root.is_some() {
            return Err("--sd-root cannot be combined with fixture content".into());
        }
        return Ok(PreviewContent::Fixtures);
    }
    let cache_base = cache_root
        .map(Path::to_path_buf)
        .unwrap_or_else(default_cache_root);
    if let Some(root) = explicit_root {
        return HostContentLayout::new(root, cache_base).map(PreviewContent::Card);
    }
    let cards = discover_mister_cards(Path::new("/Volumes"))?;
    match (mode, cards.as_slice()) {
        (ContentMode::Auto, []) => Ok(PreviewContent::Fixtures),
        (ContentMode::Card, []) => {
            Err("no MiSTer card found under /Volumes; connect one or pass --sd-root PATH".into())
        }
        (_, [card]) => HostContentLayout::new(card, cache_base).map(PreviewContent::Card),
        (_, cards) => Err(format!(
            "multiple MiSTer cards found: {}; pass --sd-root PATH",
            cards
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub fn discover_mister_cards(volumes_root: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(volumes_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "read mounted volumes {}: {error}",
                volumes_root.display()
            ));
        }
    };
    let mut cards = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| validate_card_root(path).is_ok())
        .collect::<Vec<_>>();
    cards.sort();
    Ok(cards)
}

pub fn default_cache_root() -> PathBuf {
    user_home()
        .join("Library")
        .join("Caches")
        .join("MiSTer MagiK")
        .join("ui-preview")
}

pub fn default_settings_path() -> PathBuf {
    user_home()
        .join("Library")
        .join("Application Support")
        .join("MiSTer MagiK")
        .join("ui-preview")
        .join("settings.json")
}

fn validate_card_root(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    if !root.join("MiSTer").is_file() {
        return Err(format!("{} does not contain MiSTer", root.display()));
    }
    if !root.join("_Arcade").is_dir() && !root.join("games").is_dir() {
        return Err(format!(
            "{} contains neither _Arcade nor games",
            root.display()
        ));
    }
    Ok(())
}

fn volume_key(label: &str, root: &Path) -> String {
    let mut hasher = Hasher::new();
    hasher.update(root.as_os_str().as_encoded_bytes());
    let hash = hasher.finalize();
    let label = label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("{label}-{hash:08x}")
}

fn normalize_without_parent_components(path: &Path) -> Result<PathBuf, String> {
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(format!(
            "parent traversal is not allowed: {}",
            path.display()
        ));
    }
    Ok(path.to_path_buf())
}

fn normalize_relative(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "cache path must be relative and cannot traverse: {}",
            path.display()
        ));
    }
    Ok(path.to_path_buf())
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("mister-magik-card-{label}-{stamp}"))
    }

    fn card(root: &Path, name: &str) -> PathBuf {
        let card = root.join(name);
        fs::create_dir_all(card.join("_Arcade")).expect("arcade directory");
        fs::write(card.join("MiSTer"), b"fixture").expect("MiSTer marker");
        card
    }

    #[test]
    fn discovers_only_mister_layouts() {
        let root = temp_root("discover");
        let expected = card(&root, "MiSTer_Data");
        fs::create_dir_all(root.join("ordinary")).expect("ordinary volume");

        assert_eq!(discover_mister_cards(&root).expect("discover"), [expected]);
    }

    #[test]
    fn maps_card_paths_to_canonical_identities() {
        let root = temp_root("mapping");
        let card = card(&root, "MiSTer Data");
        let layout = HostContentLayout::new(&card, root.join("cache")).expect("layout");
        let game = card.join("_Arcade").join("1942.mra");

        assert_eq!(
            layout.to_canonical(&game).expect("canonical"),
            PathBuf::from("/media/fat/_Arcade/1942.mra")
        );
        assert_eq!(
            layout
                .to_card_path("/media/fat/_Arcade/1942.mra")
                .expect("card path"),
            layout.card_root.join("_Arcade/1942.mra")
        );
    }

    #[test]
    fn cache_paths_cannot_escape_mac_storage() {
        let root = temp_root("escape");
        let card = card(&root, "MiSTer_Data");
        let layout = HostContentLayout::new(&card, root.join("cache")).expect("layout");

        assert!(layout.cache_path("../card-write").is_err());
        assert!(layout.cache_path("/Volumes/card-write").is_err());
    }

    #[test]
    fn headless_auto_content_is_always_fixture_backed() {
        assert_eq!(
            resolve_preview_content(ContentMode::Auto, None, None, true).expect("headless content"),
            PreviewContent::Fixtures
        );
    }
}
