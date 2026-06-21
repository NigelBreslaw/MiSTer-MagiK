//! Catalog v2 configuration, path, and version ownership.
//!
//! This module is intentionally small at first. The catalog refactor moves
//! constants and environment handling here before changing behavior.

use std::path::PathBuf;

pub const DEFAULT_ROOTS: &[&str] = &[
    "/media/fat/_Arcade",
    "/media/fat/_Games",
    "/media/fat/games",
    "/media/fat/_DOS Games",
    "/media/fat/_Console",
    "/media/fat/_Computer",
    "/media/fat/_YCArcade",
    "/media/fat/_YCConsole",
    "/media/fat/_YCComputer",
    "/media/fat/_LLAPI",
    "/media/fat/_Other",
    "/media/fat/_Utility",
];

pub const DEFAULT_SQLITE_PATH: &str = "/media/fat/mister-magik/library.sqlite3";
pub const DEFAULT_MAME_SQLITE_PATH: &str = "/media/fat/mister-magik/mame.sqlite3";
pub const DEFAULT_HBMAME_SQLITE_PATH: &str = "/media/fat/mister-magik/hbmame.sqlite3";
pub const DEFAULT_SQLITE_BUILD_DIR: &str = "/tmp/mister-magik/sqlite-build";

pub const SCHEMA_VERSION: u32 = 30;
pub const CATALOG_BUILD_VERSION: u32 = 1;

pub fn default_sqlite_path() -> PathBuf {
    std::env::var("MISTER_LIBRARY_SQLITE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SQLITE_PATH))
}

pub fn default_mame_sqlite_path() -> PathBuf {
    std::env::var("MISTER_MAME_SQLITE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_MAME_SQLITE_PATH))
}

pub fn default_hbmame_sqlite_path() -> PathBuf {
    std::env::var("MISTER_HBMAME_SQLITE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_HBMAME_SQLITE_PATH))
}

pub fn library_roots_from_env() -> Vec<String> {
    std::env::var("MISTER_LIBRARY_ROOTS")
        .ok()
        .map(|s| {
            s.split('|')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_else(|| DEFAULT_ROOTS.iter().map(|s| s.to_string()).collect())
}
