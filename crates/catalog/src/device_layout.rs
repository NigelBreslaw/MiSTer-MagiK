// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Fixed public/development device layouts.

use mister_magik_platform_manifest_contract::{
    DEVELOPMENT_PATHS, InstalledPaths, Layout as InstalledLayout, PUBLIC_PATHS,
};
use std::path::{Path, PathBuf};

pub const PUBLIC_APP_DIR: &str = PUBLIC_PATHS.root;
pub const DEV_APP_DIR: &str = DEVELOPMENT_PATHS.root;
pub const PUBLIC_MAIN: &str = PUBLIC_PATHS.main;
pub const DEV_MAIN: &str = DEVELOPMENT_PATHS.main;

const LIBRARY_SQLITE_ENV: &str = "MISTER_LIBRARY_SQLITE";
const MAME_SQLITE_ENV: &str = "MISTER_MAME_SQLITE";
const HBMAME_SQLITE_ENV: &str = "MISTER_HBMAME_SQLITE";
const PREVIEW_CACHE_DIR_ENV: &str = "MISTER_PREVIEW_CACHE_DIR";
const MEDIA_ASSET_DIR_ENV: &str = "MISTER_MEDIA_ASSET_DIR";
const USER_STATE_SQLITE_ENV: &str = "MISTER_USER_STATE_SQLITE";
const LIBRARY_BENCH_SQLITE_ENV: &str = "MISTER_LIBRARY_BENCH_SQLITE";
const LIBRARY_SQLITE_BUILD_DIR_ENV: &str = "MISTER_LIBRARY_SQLITE_BUILD_DIR";
const SHARDED_CATALOG_DIR_ENV: &str = "MISTER_SHARDED_CATALOG_DIR";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevicePaths {
    layout: InstalledLayout,
    device_root: PathBuf,
}

impl DevicePaths {
    pub fn for_layout(layout: InstalledLayout) -> Self {
        let installed = layout.paths();
        let device_root = Path::new(installed.root)
            .parent()
            .expect("installed app root has a device parent")
            .to_path_buf();
        Self {
            layout,
            device_root,
        }
    }

    pub fn for_executable(path: &Path) -> Self {
        let development_directory = Path::new(DEVELOPMENT_PATHS.root).file_name();
        let layout = match path.parent().and_then(Path::file_name) {
            name if name == development_directory => InstalledLayout::Development,
            _ => InstalledLayout::Public,
        };
        Self::for_layout(layout)
    }

    pub fn current() -> Self {
        std::env::current_exe()
            .ok()
            .as_deref()
            .map(Self::for_executable)
            .unwrap_or_else(|| Self::for_layout(InstalledLayout::Public))
    }

    pub fn remapped(layout: InstalledLayout, device_root: impl Into<PathBuf>) -> Self {
        Self {
            layout,
            device_root: device_root.into(),
        }
    }

    pub const fn layout(&self) -> InstalledLayout {
        self.layout
    }

    pub fn device_root(&self) -> &Path {
        &self.device_root
    }

    pub fn app_dir(&self) -> PathBuf {
        self.map_installed(self.installed().root)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.map_installed(self.installed().manifest)
    }

    pub fn main_path(&self) -> PathBuf {
        self.map_installed(self.installed().main)
    }

    pub fn gui_path(&self) -> PathBuf {
        self.map_installed(self.installed().gui)
    }

    pub fn manager_path(&self) -> PathBuf {
        self.map_installed(self.installed().manager)
    }

    pub fn scanout_module_path(&self) -> PathBuf {
        self.map_installed(self.installed().scanout_module)
    }

    pub fn scanout_metadata_path(&self) -> PathBuf {
        self.map_installed(self.installed().scanout_metadata)
    }

    pub fn latch_rbf_path(&self) -> PathBuf {
        self.map_installed(self.installed().latch_rbf)
    }

    pub fn latch_metadata_path(&self) -> PathBuf {
        self.map_installed(self.installed().latch_metadata)
    }

    pub fn app_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.app_dir().join(relative)
    }

    const fn installed(&self) -> InstalledPaths {
        self.layout.paths()
    }

    fn map_installed(&self, canonical: &str) -> PathBuf {
        let installed = self.installed();
        let canonical_root = Path::new(installed.root)
            .parent()
            .expect("installed app root has a device parent");
        let relative = Path::new(canonical)
            .strip_prefix(canonical_root)
            .expect("installed component remains under the device root");
        self.device_root.join(relative)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CatalogPathOverrides {
    library_sqlite: Option<PathBuf>,
    mame_sqlite: Option<PathBuf>,
    hbmame_sqlite: Option<PathBuf>,
    preview_cache_dir: Option<PathBuf>,
    media_asset_dir: Option<PathBuf>,
    user_state_sqlite: Option<PathBuf>,
    library_bench_sqlite: Option<PathBuf>,
    library_sqlite_build_dir: Option<PathBuf>,
    sharded_catalog_dir: Option<PathBuf>,
}

impl CatalogPathOverrides {
    pub fn capture_process() -> Self {
        Self {
            library_sqlite: std::env::var_os(LIBRARY_SQLITE_ENV).map(PathBuf::from),
            mame_sqlite: std::env::var_os(MAME_SQLITE_ENV).map(PathBuf::from),
            hbmame_sqlite: std::env::var_os(HBMAME_SQLITE_ENV).map(PathBuf::from),
            preview_cache_dir: std::env::var_os(PREVIEW_CACHE_DIR_ENV).map(PathBuf::from),
            media_asset_dir: std::env::var_os(MEDIA_ASSET_DIR_ENV).map(PathBuf::from),
            user_state_sqlite: std::env::var_os(USER_STATE_SQLITE_ENV).map(PathBuf::from),
            library_bench_sqlite: std::env::var_os(LIBRARY_BENCH_SQLITE_ENV).map(PathBuf::from),
            library_sqlite_build_dir: std::env::var_os(LIBRARY_SQLITE_BUILD_DIR_ENV)
                .map(PathBuf::from),
            sharded_catalog_dir: std::env::var_os(SHARDED_CATALOG_DIR_ENV).map(PathBuf::from),
        }
    }

    pub fn capture_with<'a>(mut get: impl FnMut(&str) -> Option<&'a Path>) -> Self {
        Self {
            library_sqlite: get(LIBRARY_SQLITE_ENV).map(Path::to_path_buf),
            mame_sqlite: get(MAME_SQLITE_ENV).map(Path::to_path_buf),
            hbmame_sqlite: get(HBMAME_SQLITE_ENV).map(Path::to_path_buf),
            preview_cache_dir: get(PREVIEW_CACHE_DIR_ENV).map(Path::to_path_buf),
            media_asset_dir: get(MEDIA_ASSET_DIR_ENV).map(Path::to_path_buf),
            user_state_sqlite: get(USER_STATE_SQLITE_ENV).map(Path::to_path_buf),
            library_bench_sqlite: get(LIBRARY_BENCH_SQLITE_ENV).map(Path::to_path_buf),
            library_sqlite_build_dir: get(LIBRARY_SQLITE_BUILD_DIR_ENV).map(Path::to_path_buf),
            sharded_catalog_dir: get(SHARDED_CATALOG_DIR_ENV).map(Path::to_path_buf),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogPaths {
    library_sqlite: PathBuf,
    mame_sqlite: PathBuf,
    hbmame_sqlite: PathBuf,
    preview_cache_dir: PathBuf,
    media_asset_dir: PathBuf,
    user_state_sqlite: PathBuf,
    library_bench_sqlite: PathBuf,
    library_sqlite_build_dir: PathBuf,
    sharded_catalog_dir: PathBuf,
}

impl CatalogPaths {
    /// Capture catalog path inputs once at a standalone process boundary.
    pub fn capture_process() -> Self {
        Self::derive(
            &DevicePaths::current(),
            CatalogPathOverrides::capture_process(),
        )
    }

    pub fn derive(device: &DevicePaths, overrides: CatalogPathOverrides) -> Self {
        Self {
            library_sqlite: overrides
                .library_sqlite
                .unwrap_or_else(|| device.app_path("library.sqlite3")),
            mame_sqlite: overrides
                .mame_sqlite
                .unwrap_or_else(|| device.app_path("mame.sqlite3")),
            hbmame_sqlite: overrides
                .hbmame_sqlite
                .unwrap_or_else(|| device.app_path("hbmame.sqlite3")),
            preview_cache_dir: overrides
                .preview_cache_dir
                .unwrap_or_else(|| device.app_path("assets")),
            media_asset_dir: overrides
                .media_asset_dir
                .unwrap_or_else(|| device.app_path("assets")),
            user_state_sqlite: overrides
                .user_state_sqlite
                .unwrap_or_else(|| device.app_path("user-state.sqlite3")),
            library_bench_sqlite: overrides
                .library_bench_sqlite
                .unwrap_or_else(|| device.app_path("library-scan-bench.sqlite3")),
            library_sqlite_build_dir: overrides
                .library_sqlite_build_dir
                .unwrap_or_else(|| PathBuf::from("/tmp/mister-magik/sqlite-build")),
            sharded_catalog_dir: overrides
                .sharded_catalog_dir
                .unwrap_or_else(|| device.app_path("catalog-v3")),
        }
    }

    pub fn library_sqlite(&self) -> &Path {
        &self.library_sqlite
    }

    pub fn mame_sqlite(&self) -> &Path {
        &self.mame_sqlite
    }

    pub fn hbmame_sqlite(&self) -> &Path {
        &self.hbmame_sqlite
    }

    pub fn preview_cache_dir(&self) -> &Path {
        &self.preview_cache_dir
    }

    pub fn media_asset_dir(&self) -> &Path {
        &self.media_asset_dir
    }

    pub fn user_state_sqlite(&self) -> &Path {
        &self.user_state_sqlite
    }

    pub fn library_bench_sqlite(&self) -> &Path {
        &self.library_bench_sqlite
    }

    pub fn library_sqlite_build_dir(&self) -> &Path {
        &self.library_sqlite_build_dir
    }

    pub fn sharded_catalog_dir(&self) -> &Path {
        &self.sharded_catalog_dir
    }

    fn environment_defaults(&self) -> [(&'static str, &Path); 7] {
        [
            (LIBRARY_SQLITE_ENV, self.library_sqlite()),
            (MAME_SQLITE_ENV, self.mame_sqlite()),
            (HBMAME_SQLITE_ENV, self.hbmame_sqlite()),
            (PREVIEW_CACHE_DIR_ENV, self.preview_cache_dir()),
            (MEDIA_ASSET_DIR_ENV, self.media_asset_dir()),
            (USER_STATE_SQLITE_ENV, self.user_state_sqlite()),
            (LIBRARY_BENCH_SQLITE_ENV, self.library_bench_sqlite()),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceLayout {
    Public,
    Dev,
}

impl DeviceLayout {
    pub fn for_executable(path: &Path) -> Self {
        match DevicePaths::for_executable(path).layout() {
            InstalledLayout::Public => Self::Public,
            InstalledLayout::Development => Self::Dev,
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
        self.paths().root
    }

    pub const fn main_path(self) -> &'static str {
        self.paths().main
    }

    pub const fn paths(self) -> InstalledPaths {
        match self {
            Self::Public => PUBLIC_PATHS,
            Self::Dev => DEVELOPMENT_PATHS,
        }
    }

    pub fn app_path(self, relative: &str) -> PathBuf {
        DevicePaths::for_layout(self.installed_layout()).app_path(relative)
    }

    const fn installed_layout(self) -> InstalledLayout {
        match self {
            Self::Public => InstalledLayout::Public,
            Self::Dev => InstalledLayout::Development,
        }
    }
}

pub fn current_app_path(relative: &str) -> PathBuf {
    DevicePaths::current().app_path(relative)
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
    let device = DevicePaths::for_layout(layout.installed_layout());
    let catalog = CatalogPaths::derive(&device, CatalogPathOverrides::default());
    for (name, value) in catalog.environment_defaults() {
        if !is_set(name) {
            set(name, value.to_path_buf());
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
    fn typed_device_paths_follow_manifest_layouts_and_remapped_roots() {
        let public = DevicePaths::for_layout(InstalledLayout::Public);
        assert_eq!(public.app_dir(), PathBuf::from(PUBLIC_PATHS.root));
        assert_eq!(public.main_path(), PathBuf::from(PUBLIC_PATHS.main));
        assert_eq!(public.gui_path(), PathBuf::from(PUBLIC_PATHS.gui));

        let development = DevicePaths::for_layout(InstalledLayout::Development);
        assert_eq!(development.app_dir(), PathBuf::from(DEVELOPMENT_PATHS.root));
        assert_eq!(
            development.main_path(),
            PathBuf::from(DEVELOPMENT_PATHS.main)
        );

        let remapped = DevicePaths::remapped(InstalledLayout::Development, "/tmp/card");
        assert_eq!(
            remapped.app_dir(),
            PathBuf::from("/tmp/card/mister-magik-dev")
        );
        assert_eq!(
            remapped.main_path(),
            PathBuf::from("/tmp/card/MiSTer_MagiKDev")
        );
        assert_eq!(
            remapped.scanout_module_path(),
            PathBuf::from("/tmp/card/mister-magik-dev/mister_magik_scanout_slots.ko")
        );
    }

    #[test]
    fn typed_catalog_paths_preserve_layout_defaults_and_explicit_overrides() {
        let device = DevicePaths::remapped(InstalledLayout::Public, "/tmp/card");
        let defaults = CatalogPaths::derive(&device, CatalogPathOverrides::default());
        assert_eq!(
            defaults.mame_sqlite(),
            Path::new("/tmp/card/mister-magik/mame.sqlite3")
        );
        assert_eq!(
            defaults.preview_cache_dir(),
            Path::new("/tmp/card/mister-magik/assets")
        );
        assert_eq!(
            defaults.library_sqlite_build_dir(),
            Path::new("/tmp/mister-magik/sqlite-build")
        );

        let values = BTreeMap::from([
            (
                LIBRARY_SQLITE_ENV,
                PathBuf::from("/tmp/override/library.sqlite3"),
            ),
            (MAME_SQLITE_ENV, PathBuf::from("/tmp/override/mame.sqlite3")),
            (
                HBMAME_SQLITE_ENV,
                PathBuf::from("/tmp/override/hbmame.sqlite3"),
            ),
            (PREVIEW_CACHE_DIR_ENV, PathBuf::from("preview-assets")),
            (MEDIA_ASSET_DIR_ENV, PathBuf::from("relative-assets")),
            (
                USER_STATE_SQLITE_ENV,
                PathBuf::from("/tmp/override/user-state.sqlite3"),
            ),
            (
                LIBRARY_BENCH_SQLITE_ENV,
                PathBuf::from("/tmp/override/library-bench.sqlite3"),
            ),
            (
                LIBRARY_SQLITE_BUILD_DIR_ENV,
                PathBuf::from("/tmp/override/sqlite-build"),
            ),
            (
                SHARDED_CATALOG_DIR_ENV,
                PathBuf::from("/tmp/override/catalog-v3"),
            ),
        ]);
        let overrides =
            CatalogPathOverrides::capture_with(|name| values.get(name).map(PathBuf::as_path));
        let paths = CatalogPaths::derive(&device, overrides);

        assert_eq!(
            paths.library_sqlite(),
            Path::new("/tmp/override/library.sqlite3")
        );
        assert_eq!(paths.mame_sqlite(), Path::new("/tmp/override/mame.sqlite3"));
        assert_eq!(
            paths.hbmame_sqlite(),
            Path::new("/tmp/override/hbmame.sqlite3")
        );
        assert_eq!(paths.preview_cache_dir(), Path::new("preview-assets"));
        assert_eq!(paths.media_asset_dir(), Path::new("relative-assets"));
        assert_eq!(
            paths.user_state_sqlite(),
            Path::new("/tmp/override/user-state.sqlite3")
        );
        assert_eq!(
            paths.library_bench_sqlite(),
            Path::new("/tmp/override/library-bench.sqlite3")
        );
        assert_eq!(
            paths.library_sqlite_build_dir(),
            Path::new("/tmp/override/sqlite-build")
        );
        assert_eq!(
            paths.sharded_catalog_dir(),
            Path::new("/tmp/override/catalog-v3")
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
