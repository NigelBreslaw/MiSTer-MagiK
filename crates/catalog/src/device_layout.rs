// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fixed public/development device layouts.

use std::path::{Path, PathBuf};

pub const PUBLIC_APP_DIR: &str = "/media/fat/mister-magik";
pub const DEV_APP_DIR: &str = "/media/fat/mister-magik-dev";
pub const PUBLIC_MAIN: &str = "/media/fat/MiSTer_MagiK";
pub const DEV_MAIN: &str = "/media/fat/MiSTer_MagiKDev";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceLayout {
    Public,
    Dev,
}

impl DeviceLayout {
    pub fn for_executable(path: &Path) -> Self {
        match path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
        {
            Some("mister-magik-dev") => Self::Dev,
            _ => Self::Public,
        }
    }

    pub fn current() -> Self {
        std::env::current_exe()
            .ok()
            .as_deref()
            .map(Self::for_executable)
            .unwrap_or(Self::Public)
    }

    pub const fn app_dir(self) -> &'static str {
        match self {
            Self::Public => PUBLIC_APP_DIR,
            Self::Dev => DEV_APP_DIR,
        }
    }

    pub const fn main_path(self) -> &'static str {
        match self {
            Self::Public => PUBLIC_MAIN,
            Self::Dev => DEV_MAIN,
        }
    }

    pub fn app_path(self, relative: &str) -> PathBuf {
        Path::new(self.app_dir()).join(relative)
    }
}

pub fn current_app_path(relative: &str) -> PathBuf {
    DeviceLayout::current().app_path(relative)
}

/// Seed existing path override interfaces from the executable's fixed layout.
/// Explicit benchmark/test overrides retain precedence.
///
/// # Safety
///
/// The caller must ensure no other thread can read or write the process
/// environment for the duration of this call.
pub unsafe fn initialize_process_env() {
    let layout = DeviceLayout::current();
    initialize_process_env_with(
        layout,
        |name| std::env::var_os(name).is_some(),
        |name, value| {
            // SAFETY: upheld by initialize_process_env's caller.
            unsafe { std::env::set_var(name, value) };
        },
    );
}

fn initialize_process_env_with(
    layout: DeviceLayout,
    mut is_set: impl FnMut(&str) -> bool,
    mut set: impl FnMut(&str, PathBuf),
) {
    for (name, relative) in [
        ("MISTER_LIBRARY_SQLITE", "library.sqlite3"),
        ("MISTER_MAME_SQLITE", "mame.sqlite3"),
        ("MISTER_HBMAME_SQLITE", "hbmame.sqlite3"),
        ("MISTER_PREVIEW_CACHE_DIR", "assets"),
        ("MISTER_MEDIA_ASSET_DIR", "assets"),
        ("MISTER_USER_STATE_SQLITE", "user-state.sqlite3"),
        ("MISTER_LIBRARY_BENCH_SQLITE", "library-scan-bench.sqlite3"),
    ] {
        if !is_set(name) {
            set(name, layout.app_path(relative));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn resolves_fixed_layout_from_executable_parent() {
        assert_eq!(
            DeviceLayout::for_executable(Path::new("/media/fat/mister-magik/mister-magik-fb")),
            DeviceLayout::Public
        );
        assert_eq!(
            DeviceLayout::for_executable(Path::new("/media/fat/mister-magik-dev/mister-magik-fb")),
            DeviceLayout::Dev
        );
        assert_eq!(DeviceLayout::Dev.main_path(), DEV_MAIN);
        assert_eq!(
            DeviceLayout::Dev.app_path("settings.json"),
            PathBuf::from("/media/fat/mister-magik-dev/settings.json")
        );
        assert_eq!(DeviceLayout::Public.app_dir(), PUBLIC_APP_DIR);
        assert_eq!(DeviceLayout::Public.main_path(), PUBLIC_MAIN);
        assert_eq!(
            DeviceLayout::for_executable(Path::new("mister-magik-fb")),
            DeviceLayout::Public
        );
    }

    #[test]
    fn process_environment_defaults_preserve_explicit_overrides() {
        let existing = BTreeSet::from(["MISTER_LIBRARY_SQLITE"]);
        let mut seeded = BTreeMap::new();

        initialize_process_env_with(
            DeviceLayout::Dev,
            |name| existing.contains(name),
            |name, value| {
                seeded.insert(name.to_string(), value);
            },
        );

        assert!(!seeded.contains_key("MISTER_LIBRARY_SQLITE"));
        assert_eq!(
            seeded.get("MISTER_MEDIA_ASSET_DIR"),
            Some(&PathBuf::from("/media/fat/mister-magik-dev/assets"))
        );
        assert_eq!(
            seeded.get("MISTER_USER_STATE_SQLITE"),
            Some(&PathBuf::from(
                "/media/fat/mister-magik-dev/user-state.sqlite3"
            ))
        );
        assert_eq!(seeded.len(), 6);
    }
}
