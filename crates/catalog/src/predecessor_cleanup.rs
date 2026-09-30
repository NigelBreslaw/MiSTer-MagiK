// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded removal of catalog artifacts retired by the fast-catalog migration.

use crate::device_layout::CatalogPaths;
use std::fs;
use std::path::Path;

const FAST_CATALOG_DIR_NAME: &str = "catalog-fast-v1";
const PREDECESSOR_CATALOG_DIR_NAME: &str = "catalog-v3";
const PREDECESSOR_SQLITE_NAME: &str = "library.sqlite3";
const PREDECESSOR_ARCADE_BOOTSTRAP_NAME: &str = "arcade-bootstrap.nav.lz4b";
const PREDECESSOR_DETECTION_FILES: &[&str] = &[
    PREDECESSOR_SQLITE_NAME,
    "library.summary.json",
    "library.nav.lz4b",
    "database-build-time.txt",
    PREDECESSOR_ARCADE_BOOTSTRAP_NAME,
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PredecessorCleanupReport {
    pub detected: bool,
    pub removed_artifacts: usize,
}

/// Detect only the fixed installed-layout predecessor footprint. Environment
/// overrides with a differently named catalog root are deliberately excluded
/// from automatic deletion.
#[must_use]
pub fn predecessor_catalog_artifacts_present(paths: &CatalogPaths) -> bool {
    let Some(app_dir) = installed_app_dir(paths.sharded_catalog_dir()) else {
        return false;
    };
    predecessor_catalog_artifacts_present_at(app_dir)
}

/// Remove the retired generated catalog while preserving screenshots, media,
/// user state, source metadata databases, and the active fast catalog.
pub fn remove_predecessor_catalog_artifacts(
    paths: &CatalogPaths,
) -> Result<PredecessorCleanupReport, String> {
    let Some(app_dir) = installed_app_dir(paths.sharded_catalog_dir()) else {
        return Ok(PredecessorCleanupReport::default());
    };
    remove_predecessor_catalog_artifacts_at(
        app_dir,
        paths.library_sqlite_build_dir(),
        Path::new("/tmp/mister-magik"),
    )
}

/// Explicit full reset of generated catalog state. The caller holds the
/// mutation lease until associated screenshot deletion is complete.
pub fn remove_generated_catalog_artifacts_with_lease(
    paths: &CatalogPaths,
    _lease: &crate::catalog_lease::CatalogMutationLease,
) -> Result<usize, String> {
    remove_generated_catalog_artifacts_at(paths, Path::new("/tmp/mister-magik"))
}

fn remove_generated_catalog_artifacts_at(
    paths: &CatalogPaths,
    volatile: &Path,
) -> Result<usize, String> {
    let app = installed_app_dir(paths.sharded_catalog_dir()).ok_or_else(|| {
        format!(
            "refusing to reset unexpected catalog path {}",
            paths.sharded_catalog_dir().display()
        )
    })?;
    let mut removed = remove_catalog_artifacts_at(
        paths.library_sqlite(),
        paths.library_sqlite_build_dir(),
        volatile,
        &app.join("rebuild-on-next-boot"),
    )?;
    for path in [
        paths.sharded_catalog_dir().to_path_buf(),
        app.join(PREDECESSOR_CATALOG_DIR_NAME),
        app.join("diagnostics/catalog"),
        volatile.join("catalog-worker"),
        volatile.join("fast-five-catalog"),
        volatile.join("fast-five-catalog-serial"),
    ] {
        removed += usize::from(remove_dir_or_symlink_if_exists(&path)?);
    }
    for path in [
        app.join(PREDECESSOR_ARCADE_BOOTSTRAP_NAME),
        volatile.join("launcher-return-catalog.json"),
        volatile.join("launcher-return-catalog.json.tmp"),
    ] {
        removed += usize::from(remove_file_if_exists(&path, "catalog reset artifact")?);
    }
    Ok(removed)
}

fn installed_app_dir(fast_catalog_root: &Path) -> Option<&Path> {
    (fast_catalog_root.file_name().and_then(|name| name.to_str()) == Some(FAST_CATALOG_DIR_NAME))
        .then(|| fast_catalog_root.parent())
        .flatten()
}

fn predecessor_catalog_artifacts_present_at(app_dir: &Path) -> bool {
    path_or_symlink_exists(&app_dir.join(PREDECESSOR_CATALOG_DIR_NAME))
        || PREDECESSOR_DETECTION_FILES
            .iter()
            .any(|name| path_or_symlink_exists(&app_dir.join(name)))
        || matching_file_exists(app_dir, predecessor_adjacent_file)
}

fn remove_predecessor_catalog_artifacts_at(
    app_dir: &Path,
    build_dir: &Path,
    snapshot_dir: &Path,
) -> Result<PredecessorCleanupReport, String> {
    let detected = predecessor_catalog_artifacts_present_at(app_dir);
    if !detected {
        return Ok(PredecessorCleanupReport::default());
    }

    let mut removed_artifacts = usize::from(remove_dir_or_symlink_if_exists(
        &app_dir.join(PREDECESSOR_CATALOG_DIR_NAME),
    )?);
    removed_artifacts = removed_artifacts.saturating_add(remove_catalog_artifacts_at(
        &app_dir.join(PREDECESSOR_SQLITE_NAME),
        build_dir,
        snapshot_dir,
        &app_dir.join("rebuild-on-next-boot"),
    )?);
    removed_artifacts = removed_artifacts.saturating_add(usize::from(remove_file_if_exists(
        &app_dir.join(PREDECESSOR_ARCADE_BOOTSTRAP_NAME),
        "predecessor arcade bootstrap",
    )?));

    Ok(PredecessorCleanupReport {
        detected,
        removed_artifacts,
    })
}

fn remove_catalog_artifacts_at(
    sqlite_path: &Path,
    build_dir: &Path,
    default_snapshot_dir: &Path,
    rebuild_marker: &Path,
) -> Result<usize, String> {
    let mut removed = 0usize;
    for (path, label) in [
        (sqlite_path.to_path_buf(), "database"),
        (
            sqlite_path.with_file_name("library.summary.json"),
            "catalog summary",
        ),
        (
            sqlite_path.with_file_name("library.nav.lz4b"),
            "catalog navigation",
        ),
        (
            sqlite_path.with_file_name("database-build-time.txt"),
            "catalog build duration",
        ),
        (rebuild_marker.to_path_buf(), "catalog rebuild marker"),
    ] {
        removed += usize::from(remove_file_if_exists(&path, label)?);
    }

    let sqlite_name = sqlite_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("library.sqlite3");
    if let Some(parent) = sqlite_path.parent() {
        let sqlite_tmp_prefix = format!(".{sqlite_name}.tmp.");
        let sqlite_journal = format!("{sqlite_name}-journal");
        let sqlite_wal = format!("{sqlite_name}-wal");
        let sqlite_shm = format!("{sqlite_name}-shm");
        removed += remove_matching_files(parent, "catalog adjacent temp", |name| {
            name.starts_with(&sqlite_tmp_prefix)
                || name == sqlite_journal
                || name == sqlite_wal
                || name == sqlite_shm
                || name == ".library.summary.json.tmp"
                || name == ".library.nav.lz4b.tmp"
                || name == ".library-build-seconds.tmp"
        })?;
    }
    let build_prefix = format!(".{sqlite_name}.build.");
    removed += remove_matching_files(build_dir, "catalog build temp", |name| {
        name.starts_with(&build_prefix)
    })?;
    removed += remove_matching_files(default_snapshot_dir, "catalog ready snapshot", |name| {
        name.starts_with("catalog-ready-")
            && (name.ends_with(".nav.lz4b") || name.ends_with(".nav.lz4b.tmp"))
    })?;
    Ok(removed)
}

fn remove_matching_files(
    dir: &Path,
    label: &str,
    mut matches: impl FnMut(&str) -> bool,
) -> Result<usize, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("read {label} directory {}: {error}", dir.display())),
    };
    let mut removed = 0usize;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {label} entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("stat {label} {}: {error}", entry.path().display()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if (file_type.is_file() || file_type.is_symlink()) && matches(&name) {
            removed += usize::from(remove_file_if_exists(&entry.path(), label)?);
        }
    }
    Ok(removed)
}

fn predecessor_adjacent_file(name: &str) -> bool {
    name.starts_with(".library.sqlite3.tmp.")
        || matches!(
            name,
            "library.sqlite3-journal"
                | "library.sqlite3-wal"
                | "library.sqlite3-shm"
                | ".library.summary.json.tmp"
                | ".library.nav.lz4b.tmp"
                | ".library-build-seconds.tmp"
        )
}

fn matching_file_exists(dir: &Path, matches: impl Fn(&str) -> bool) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        entry
            .file_type()
            .is_ok_and(|file_type| file_type.is_file() || file_type.is_symlink())
            && entry.file_name().to_str().is_some_and(&matches)
    })
}

fn path_or_symlink_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn remove_dir_or_symlink_if_exists(path: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "stat predecessor catalog {}: {error}",
                path.display()
            ));
        }
    };
    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result
        .map(|()| true)
        .map_err(|error| format!("remove predecessor catalog {}: {error}", path.display()))
}

fn remove_file_if_exists(path: &Path, label: &str) -> Result<bool, String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("remove {label} {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "mister-magik-predecessor-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn full_reset_removes_all_generated_outputs_without_touching_user_or_source_data() {
        use crate::device_layout::{CatalogPathOverrides, DevicePaths};
        let root = unique_temp_dir("full-reset");
        let device = DevicePaths::remapped(
            mister_magik_platform_manifest_contract::Layout::Public,
            root.clone(),
        );
        let app = device.app_dir();
        let volatile = root.join("volatile");
        let build = volatile.join("sqlite-build");
        let paths = CatalogPaths::derive(
            &device,
            CatalogPathOverrides::capture_with(|name| {
                (name == "MISTER_LIBRARY_SQLITE_BUILD_DIR").then_some(build.as_path())
            }),
        );
        let removable = [
            app.join("catalog-fast-v1/systems/arcade/generation.navpack"),
            app.join("catalog-fast-v1/systems/arcade/generation.sqlite3-wal"),
            app.join("catalog-fast-v1/fast-refresh-v4/snapshot.bin"),
            app.join("catalog-v3/systems/old.navpack"),
            app.join("library.sqlite3"),
            app.join("library.sqlite3-wal"),
            app.join("library.sqlite3-shm"),
            app.join("library.sqlite3-journal"),
            app.join("library.summary.json"),
            app.join("library.nav.lz4b"),
            app.join(".library.sqlite3.tmp.crashed"),
            app.join("database-build-time.txt"),
            app.join("arcade-bootstrap.nav.lz4b"),
            app.join("rebuild-on-next-boot"),
            app.join("diagnostics/catalog/latest.json"),
            build.join(".library.sqlite3.build.crashed"),
            volatile.join("catalog-ready-123.nav.lz4b"),
            volatile.join("catalog-worker/seed.bin"),
            volatile.join("fast-five-catalog/run/system.sqlite3"),
            volatile.join("fast-five-catalog-serial/run/system.navpack"),
            volatile.join("launcher-return-catalog.json"),
        ];
        let retained = [
            app.join("user-state.sqlite3"),
            app.join("settings.json"),
            app.join("magik-metadata-v1.bin"),
            app.join("mame.sqlite3"),
            app.join("assets/manual.pdf"),
            root.join("games/game.rom"),
            volatile.join("other-worker/state.json"),
        ];
        for path in removable.iter().chain(retained.iter()) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"fixture").unwrap();
        }
        assert!(remove_generated_catalog_artifacts_at(&paths, &volatile).unwrap() > 0);
        for path in &removable {
            assert!(!path.exists(), "left generated file {}", path.display());
        }
        for path in &retained {
            assert_eq!(fs::read(path).unwrap(), b"fixture");
        }
        assert_eq!(
            remove_generated_catalog_artifacts_at(&paths, &volatile).unwrap(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removes_complete_predecessor_footprint_and_preserves_owned_data() {
        let root = unique_temp_dir("cleanup");
        let app = root.join("mister-magik");
        let build = root.join("sqlite-build");
        let snapshots = root.join("volatile");
        fs::create_dir_all(app.join("catalog-v3/systems/arcade")).unwrap();
        fs::create_dir_all(app.join("catalog-fast-v1/registry")).unwrap();
        fs::create_dir_all(app.join("assets")).unwrap();
        fs::create_dir_all(&build).unwrap();
        fs::create_dir_all(&snapshots).unwrap();
        fs::write(app.join("catalog-v3/systems/arcade/active.navpack"), b"old").unwrap();

        let mut removable = [
            PREDECESSOR_SQLITE_NAME,
            "library.summary.json",
            "library.nav.lz4b",
            "database-build-time.txt",
            PREDECESSOR_ARCADE_BOOTSTRAP_NAME,
            "rebuild-on-next-boot",
            ".library.sqlite3.tmp.42",
            "library.sqlite3-wal",
            "library.sqlite3-shm",
            "library.sqlite3-journal",
            ".library.summary.json.tmp",
            ".library.nav.lz4b.tmp",
            ".library-build-seconds.tmp",
        ]
        .map(|name| app.join(name))
        .to_vec();
        removable.push(build.join(".library.sqlite3.build.42"));
        removable.push(snapshots.join("catalog-ready-42.nav.lz4b"));
        for path in &removable {
            fs::write(path, b"old").unwrap();
        }

        let preserved = [
            app.join("mame.sqlite3"),
            app.join("hbmame.sqlite3"),
            app.join("user-state.sqlite3"),
            app.join("arcade-updater-index-v1.lz4b"),
            app.join("library-scan-bench.sqlite3"),
            app.join("assets/arcade.zip"),
            app.join("catalog-fast-v1/registry/manifest-a.bin"),
            app.join("unrelated.sqlite3"),
            build.join("keep.bin"),
            snapshots.join("keep.nav.lz4b"),
        ];
        for path in &preserved {
            fs::write(path, b"keep").unwrap();
        }

        assert!(predecessor_catalog_artifacts_present_at(&app));
        let report = remove_predecessor_catalog_artifacts_at(&app, &build, &snapshots).unwrap();
        assert!(report.detected);
        assert_eq!(report.removed_artifacts, removable.len() + 1);
        assert!(!predecessor_catalog_artifacts_present_at(&app));
        assert!(!app.join(PREDECESSOR_CATALOG_DIR_NAME).exists());
        for path in removable {
            assert!(!path.exists(), "removed {}", path.display());
        }
        for path in preserved {
            assert!(path.exists(), "preserved {}", path.display());
        }
        assert_eq!(
            remove_predecessor_catalog_artifacts_at(&app, &build, &snapshots).unwrap(),
            PredecessorCleanupReport::default()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn absent_predecessor_is_a_noop() {
        let root = unique_temp_dir("absent");
        let app = root.join("mister-magik");
        fs::create_dir_all(app.join("assets")).unwrap();
        fs::write(app.join("assets/arcade.zip"), b"screenshot").unwrap();
        fs::write(app.join("rebuild-on-next-boot"), b"current request").unwrap();

        let report = remove_predecessor_catalog_artifacts_at(
            &app,
            &root.join("build"),
            &root.join("volatile"),
        )
        .unwrap();
        assert_eq!(report, PredecessorCleanupReport::default());
        assert!(app.join("assets/arcade.zip").exists());
        assert!(app.join("rebuild-on-next-boot").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_detection_rejects_nonstandard_fast_catalog_roots() {
        assert_eq!(
            installed_app_dir(Path::new("/tmp/catalog-fast-v1")),
            Some(Path::new("/tmp"))
        );
        assert_eq!(installed_app_dir(Path::new("/tmp/catalog-v3")), None);
        assert_eq!(installed_app_dir(Path::new("/tmp/custom")), None);
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_unlinks_predecessor_symlink_without_following_it() {
        use std::os::unix::fs::symlink;

        let root = unique_temp_dir("symlink");
        let app = root.join("mister-magik");
        let external = root.join("external-catalog");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("keep.navpack"), b"keep").unwrap();
        symlink(&external, app.join(PREDECESSOR_CATALOG_DIR_NAME)).unwrap();

        assert!(predecessor_catalog_artifacts_present_at(&app));
        let report = remove_predecessor_catalog_artifacts_at(
            &app,
            &root.join("build"),
            &root.join("volatile"),
        )
        .unwrap();

        assert_eq!(report.removed_artifacts, 1);
        assert!(fs::symlink_metadata(app.join(PREDECESSOR_CATALOG_DIR_NAME)).is_err());
        assert_eq!(fs::read(external.join("keep.navpack")).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }
}
