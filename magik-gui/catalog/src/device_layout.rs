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
pub fn initialize_process_env() {
    let layout = DeviceLayout::current();
    for (name, relative) in [
        ("MISTER_LIBRARY_SQLITE", "library.sqlite3"),
        ("MISTER_MAME_SQLITE", "mame.sqlite3"),
        ("MISTER_HBMAME_SQLITE", "hbmame.sqlite3"),
        ("MISTER_PREVIEW_CACHE_DIR", "assets"),
        ("MISTER_MEDIA_ASSET_DIR", "assets"),
        ("MISTER_LIBRARY_BENCH_SQLITE", "library-scan-bench.sqlite3"),
    ] {
        if std::env::var_os(name).is_none() {
            std::env::set_var(name, layout.app_path(relative));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
