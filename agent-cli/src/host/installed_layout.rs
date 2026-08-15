// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Typed host-side access to the schema-owned installed platform layouts.

use super::{Layout, Result};
use mister_magik_platform_manifest_contract::{DEVELOPMENT_PATHS, InstalledPaths, PUBLIC_PATHS};
use std::path::{Component, Path};

pub(super) const fn paths(layout: Layout) -> InstalledPaths {
    match layout {
        Layout::Development => DEVELOPMENT_PATHS,
        Layout::Public => PUBLIC_PATHS,
    }
}

pub(super) fn app_path(layout: Layout, relative: &str) -> Result<String> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "invalid installed-layout relative path: {}",
            relative.display()
        )
        .into());
    }
    Ok(format!("{}/{}", paths(layout).root, relative.display()))
}

pub(super) fn arming_paths() -> [String; 7] {
    [
        app_path(Layout::Public, "launcher.env").expect("static installed path"),
        app_path(Layout::Development, "launcher.env").expect("static installed path"),
        "/tmp/mister-magik/fs-fault-launcher.env".to_owned(),
        "/tmp/mister-magik/fs-fault-session".to_owned(),
        "/tmp/mister-magik/fs-fault.json".to_owned(),
        app_path(Layout::Public, "rebuild-on-next-boot").expect("static installed path"),
        app_path(Layout::Development, "rebuild-on-next-boot").expect("static installed path"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_both_layouts_and_rejects_traversal() {
        assert_eq!(
            app_path(Layout::Public, "platform-v3.manifest").unwrap(),
            format!("{}/platform-v3.manifest", PUBLIC_PATHS.root)
        );
        assert_eq!(
            app_path(Layout::Development, "fpga/latch.rbf").unwrap(),
            format!("{}/fpga/latch.rbf", DEVELOPMENT_PATHS.root)
        );
        assert!(app_path(Layout::Development, "../launcher.env").is_err());
        assert!(app_path(Layout::Public, "/tmp/manifest").is_err());
    }
}
