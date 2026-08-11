// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Catalog source, metadata, and V3 storage path configuration.

use std::path::PathBuf;

use crate::device_layout::current_app_path;

pub const DEFAULT_ROOTS: &[&str] = &[
    "/media/fat/_Arcade",
    "/media/fat/games",
    "/media/fat/_DOS Games",
    "/media/fat/_LLAPI",
];

pub const DEFAULT_SQLITE_PATH: &str = "/media/fat/mister-magik/library.sqlite3";
pub const DEFAULT_MAME_SQLITE_PATH: &str = "/media/fat/mister-magik/mame.sqlite3";
pub const DEFAULT_HBMAME_SQLITE_PATH: &str = "/media/fat/mister-magik/hbmame.sqlite3";
pub const DEFAULT_SQLITE_BUILD_DIR: &str = "/tmp/mister-magik/sqlite-build";
pub const DEFAULT_SHARDED_CATALOG_DIR: &str = "/media/fat/mister-magik/catalog-v3";
pub const DEFAULT_USER_STATE_PATH: &str = "/media/fat/mister-magik/user-state.sqlite3";

pub const SCHEMA_VERSION: u32 = 67;
pub const CATALOG_BUILD_VERSION: u32 = 17;

pub fn default_sqlite_path() -> PathBuf {
    configured_path(
        std::env::var("MISTER_LIBRARY_SQLITE").ok().as_deref(),
        "library.sqlite3",
    )
}

pub fn default_mame_sqlite_path() -> PathBuf {
    configured_path(
        std::env::var("MISTER_MAME_SQLITE").ok().as_deref(),
        "mame.sqlite3",
    )
}

pub fn default_hbmame_sqlite_path() -> PathBuf {
    configured_path(
        std::env::var("MISTER_HBMAME_SQLITE").ok().as_deref(),
        "hbmame.sqlite3",
    )
}

pub fn default_sharded_catalog_path() -> PathBuf {
    std::env::var("MISTER_SHARDED_CATALOG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| current_app_path("catalog-v3"))
}

pub fn default_user_state_path() -> PathBuf {
    configured_path(
        std::env::var("MISTER_USER_STATE_SQLITE").ok().as_deref(),
        "user-state.sqlite3",
    )
}

/// Mutable, non-authoritative progress for an interrupted catalog build.
pub fn default_build_progress_path() -> PathBuf {
    crate::build_progress::path_for_root(&default_sharded_catalog_path())
}

/// Last successfully published scan-target facts used for warm planning.
pub fn default_builder_state_path() -> PathBuf {
    crate::build_progress::committed_path_for_root(&default_sharded_catalog_path())
}

pub fn library_roots_from_env() -> Vec<String> {
    library_roots_from_value(std::env::var("MISTER_LIBRARY_ROOTS").ok().as_deref())
}

fn configured_path(value: Option<&str>, default_name: &str) -> PathBuf {
    value
        .map(PathBuf::from)
        .unwrap_or_else(|| current_app_path(default_name))
}

fn library_roots_from_value(value: Option<&str>) -> Vec<String> {
    value
        .map(|s| {
            s.split('|')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_else(|| DEFAULT_ROOTS.iter().map(|s| s.to_string()).collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathMapRule {
    pub from: String,
    pub to: String,
}

pub fn library_path_map_from_env() -> Vec<PathMapRule> {
    std::env::var("MISTER_LIBRARY_PATH_MAP")
        .ok()
        .map(|s| parse_library_path_map(&s))
        .unwrap_or_default()
}

pub fn parse_library_path_map(value: &str) -> Vec<PathMapRule> {
    let mut rules = value
        .split('|')
        .filter_map(|part| {
            let (from, to) = part.split_once('=')?;
            let from = trim_trailing_slash(from.trim());
            let to = trim_trailing_slash(to.trim());
            if from.is_empty() || to.is_empty() {
                return None;
            }
            Some(PathMapRule {
                from: from.to_string(),
                to: to.to_string(),
            })
        })
        .collect::<Vec<_>>();
    rules.sort_by_key(|rule| std::cmp::Reverse(rule.from.len()));
    rules
}

pub fn map_library_path(value: &str, rules: &[PathMapRule]) -> String {
    for rule in rules {
        if value == rule.from {
            return rule.to.clone();
        }
        if let Some(rest) = value.strip_prefix(&rule.from)
            && rest.starts_with('/')
        {
            return format!("{}{}", rule.to, rest);
        }
    }
    value.to_string()
}

fn trim_trailing_slash(value: &str) -> &str {
    if value == "/" {
        value
    } else {
        value.trim_end_matches('/')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_paths_use_env_overrides_and_defaults() {
        assert_eq!(
            configured_path(Some("/tmp/library.sqlite3"), "library.sqlite3"),
            PathBuf::from("/tmp/library.sqlite3")
        );
        assert_eq!(
            configured_path(Some("/tmp/mame.sqlite3"), "mame.sqlite3"),
            PathBuf::from("/tmp/mame.sqlite3")
        );
        assert_eq!(
            configured_path(None, "hbmame.sqlite3"),
            PathBuf::from(DEFAULT_HBMAME_SQLITE_PATH)
        );
        assert_eq!(
            configured_path(None, "user-state.sqlite3"),
            PathBuf::from(DEFAULT_USER_STATE_PATH)
        );
    }

    #[test]
    fn library_roots_trim_empty_env_segments_and_default_when_absent() {
        assert_eq!(
            library_roots_from_value(Some(" /media/fat/_Arcade | | /media/fat/games ||")),
            vec!["/media/fat/_Arcade", "/media/fat/games"]
        );

        assert_eq!(
            library_roots_from_value(None),
            DEFAULT_ROOTS
                .iter()
                .map(|root| root.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn path_map_parses_longest_prefix_first_and_maps_boundaries() {
        let rules = parse_library_path_map(
            "/tmp/mirror/games=/media/fat/games|/tmp/mirror=/media/fat|broken",
        );

        assert_eq!(
            map_library_path("/tmp/mirror/games/NES/Zelda.nes", &rules),
            "/media/fat/games/NES/Zelda.nes"
        );
        assert_eq!(
            map_library_path("/tmp/mirror/_Arcade", &rules),
            "/media/fat/_Arcade"
        );
        assert_eq!(
            map_library_path("/tmp/mirrorish/_Arcade", &rules),
            "/tmp/mirrorish/_Arcade"
        );
    }
}
