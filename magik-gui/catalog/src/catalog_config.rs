//! Catalog v2 configuration, path, and version ownership.
//!
//! This module is intentionally small at first. The catalog refactor moves
//! constants and environment handling here before changing behavior.

use std::path::PathBuf;

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

pub const SCHEMA_VERSION: u32 = 37;
pub const CATALOG_BUILD_VERSION: u32 = 7;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct EnvRestore {
        key: &'static str,
        value: Option<String>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self {
                key,
                value: previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::remove_var(key);
            Self {
                key,
                value: previous,
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            if let Some(value) = &self.value {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn catalog_paths_use_env_overrides_and_defaults() {
        let _guard = env_lock();
        let _library = EnvRestore::set("MISTER_LIBRARY_SQLITE", "/tmp/library.sqlite3");
        let _mame = EnvRestore::set("MISTER_MAME_SQLITE", "/tmp/mame.sqlite3");
        let _hbmame = EnvRestore::remove("MISTER_HBMAME_SQLITE");

        assert_eq!(default_sqlite_path(), PathBuf::from("/tmp/library.sqlite3"));
        assert_eq!(
            default_mame_sqlite_path(),
            PathBuf::from("/tmp/mame.sqlite3")
        );
        assert_eq!(
            default_hbmame_sqlite_path(),
            PathBuf::from(DEFAULT_HBMAME_SQLITE_PATH)
        );
    }

    #[test]
    fn library_roots_trim_empty_env_segments_and_default_when_absent() {
        let _guard = env_lock();
        let roots = EnvRestore::set(
            "MISTER_LIBRARY_ROOTS",
            " /media/fat/_Arcade | | /media/fat/games ||",
        );

        assert_eq!(
            library_roots_from_env(),
            vec!["/media/fat/_Arcade", "/media/fat/games"]
        );
        drop(roots);

        assert_eq!(
            library_roots_from_env(),
            DEFAULT_ROOTS
                .iter()
                .map(|root| root.to_string())
                .collect::<Vec<_>>()
        );
    }
}
