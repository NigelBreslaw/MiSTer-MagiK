// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Durable first-visible Arcade navigation bootstrap.
//!
//! The navigation snapshot is already the bounded, versioned representation
//! of the exact canonical rows and structured launch plans consumed by the
//! launcher. Keeping one copy outside `catalog-v3` lets a clean catalog rebuild
//! reveal Arcade without reopening every MRA. The embedded catalog stamp binds
//! the snapshot to the cheap live input facts; the authoritative full scan
//! still follows and replaces it if a nested/manual edit escaped those facts.

use crate::arcade_catalog::ArcadeCatalog;
use crate::catalog_navigation::read_catalog_navigation_snapshot;
use crate::catalog_stamp::{compute_catalog_stamp_for_paths, CatalogStamp};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

const FILE_NAME: &str = "arcade-bootstrap.nav.lz4b";
const MAX_INDEX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum ProbeResult {
    Hit(Box<LoadedIndex>),
    Miss { reason: String, probe_us: u64 },
}

#[derive(Debug)]
pub(crate) struct LoadedIndex {
    pub(crate) catalog: ArcadeCatalog,
    pub(crate) stamp: CatalogStamp,
    pub(crate) probe_us: u64,
    pub(crate) decode_us: u64,
    pub(crate) bytes: u64,
}

pub(crate) fn default_path() -> PathBuf {
    std::env::var_os("MISTER_ARCADE_BOOTSTRAP_INDEX")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::device_layout::current_app_path(FILE_NAME))
}

pub(crate) fn probe(root: &Path) -> ProbeResult {
    probe_at(
        root,
        &default_path(),
        &crate::catalog_config::default_mame_sqlite_path(),
        &crate::catalog_config::default_hbmame_sqlite_path(),
    )
}

fn probe_at(root: &Path, path: &Path, mame: &Path, hbmame: &Path) -> ProbeResult {
    let probe_started = Instant::now();
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProbeResult::Miss {
                reason: "missing".to_string(),
                probe_us: probe_started.elapsed().as_micros() as u64,
            };
        }
        Err(error) => {
            return ProbeResult::Miss {
                reason: format!("metadata-error:{error}"),
                probe_us: probe_started.elapsed().as_micros() as u64,
            };
        }
    };
    if metadata.len() > MAX_INDEX_BYTES {
        return ProbeResult::Miss {
            reason: format!("oversize:{}", metadata.len()),
            probe_us: probe_started.elapsed().as_micros() as u64,
        };
    }

    let decode_started = Instant::now();
    let projection = match read_catalog_navigation_snapshot(path) {
        Ok(projection) => projection,
        Err(error) => {
            return ProbeResult::Miss {
                reason: format!("decode-error:{error}"),
                probe_us: probe_started.elapsed().as_micros() as u64,
            };
        }
    };
    let decode_us = decode_started.elapsed().as_micros() as u64;
    let embedded_stamp = CatalogStamp::from_lines(projection.catalog_stamp_lines.clone());
    let roots = vec![root.display().to_string()];
    let live_stamp = compute_catalog_stamp_for_paths(&roots, mame, hbmame);
    if !embedded_stamp.has_same_live_inputs(&live_stamp) {
        return ProbeResult::Miss {
            reason: "live-input-mismatch".to_string(),
            probe_us: probe_started.elapsed().as_micros() as u64,
        };
    }
    let catalog = ArcadeCatalog::from_navigation_projection(root.to_path_buf(), projection);
    if catalog.is_empty() {
        return ProbeResult::Miss {
            reason: "empty".to_string(),
            probe_us: probe_started.elapsed().as_micros() as u64,
        };
    }
    ProbeResult::Hit(Box::new(LoadedIndex {
        catalog,
        stamp: embedded_stamp,
        probe_us: probe_started.elapsed().as_micros() as u64,
        decode_us,
        bytes: metadata.len(),
    }))
}

pub(crate) fn publish_from_snapshot(snapshot_path: &Path) -> Result<(u64, u64), String> {
    publish_from_snapshot_at(snapshot_path, &default_path())
}

pub(crate) fn publish_from_full_catalog(catalog: &ArcadeCatalog) -> Result<(u64, u64), String> {
    let started = Instant::now();
    let root = Path::new(crate::arcade_catalog::DEFAULT_ARCADE_ROOT);
    let arcade = catalog.isolated_system_catalog("arcade");
    if arcade.is_empty() {
        return Err("full catalog has no resident Arcade rows".to_string());
    }
    let roots = vec![root.display().to_string()];
    let stamp = compute_catalog_stamp_for_paths(
        &roots,
        &crate::catalog_config::default_mame_sqlite_path(),
        &crate::catalog_config::default_hbmame_sqlite_path(),
    );
    let path = default_path();
    crate::catalog_navigation::write_catalog_navigation_snapshot(&path, &arcade, &stamp)?;
    let bytes = std::fs::metadata(&path)
        .map_err(|error| format!("inspect Arcade bootstrap index {}: {error}", path.display()))?
        .len();
    Ok((bytes, started.elapsed().as_micros() as u64))
}

fn publish_from_snapshot_at(snapshot_path: &Path, final_path: &Path) -> Result<(u64, u64), String> {
    let started = Instant::now();
    let bytes = std::fs::read(snapshot_path).map_err(|error| {
        format!(
            "read Arcade bootstrap snapshot {}: {error}",
            snapshot_path.display()
        )
    })?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(format!(
            "Arcade bootstrap snapshot size {} exceeds limit {MAX_INDEX_BYTES}",
            bytes.len()
        ));
    }
    // Decode before publication so a malformed transient snapshot can never
    // replace the retained, known-good index.
    read_catalog_navigation_snapshot(snapshot_path)?;
    crate::atomic_publish::write_atomically(
        final_path,
        "Arcade bootstrap index",
        FILE_NAME,
        Some("arcade_bootstrap_index"),
        |file| file.write_all(&bytes),
    )?;
    Ok((bytes.len() as u64, started.elapsed().as_micros() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_catalog::{ArcadeCatalog, GameSystemEntry};
    use crate::catalog_navigation::write_catalog_navigation_snapshot;
    use crate::test_support::arcade_game;

    fn unique_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    fn catalog(root: &Path) -> ArcadeCatalog {
        ArcadeCatalog::new(
            root.to_path_buf(),
            vec![arcade_game("1942")
                .path(root.join("1942.mra").display().to_string())
                .build()],
            vec![GameSystemEntry {
                id: "arcade".to_string(),
                title: "Arcade".to_string(),
                count: 1,
            }],
        )
    }

    #[test]
    fn valid_index_round_trips_exact_navigation_and_live_input_binding() {
        let dir = unique_dir("arcade-bootstrap-round-trip");
        let root = dir.join("_Arcade");
        let mame = dir.join("mame.sqlite3");
        let hbmame = dir.join("hbmame.sqlite3");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("1942.mra"), b"fixture").unwrap();
        std::fs::write(&mame, b"mame").unwrap();
        std::fs::write(&hbmame, b"hbmame").unwrap();
        let roots = vec![root.display().to_string()];
        let stamp = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);
        let snapshot = dir.join("ready.nav.lz4b");
        let index = dir.join(FILE_NAME);
        write_catalog_navigation_snapshot(&snapshot, &catalog(&root), &stamp).unwrap();

        publish_from_snapshot_at(&snapshot, &index).unwrap();
        let ProbeResult::Hit(loaded) = probe_at(&root, &index, &mame, &hbmame) else {
            panic!("valid index did not load");
        };
        assert_eq!(loaded.catalog.len(), 1);
        assert_eq!(loaded.catalog.games[0].title.as_ref(), "1942");
        assert_eq!(loaded.stamp, stamp);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_or_corrupt_index_falls_back_without_replacing_known_good_file() {
        let dir = unique_dir("arcade-bootstrap-fallback");
        let root = dir.join("_Arcade");
        let mame = dir.join("mame.sqlite3");
        let hbmame = dir.join("hbmame.sqlite3");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&mame, b"mame").unwrap();
        std::fs::write(&hbmame, b"hbmame").unwrap();
        let roots = vec![root.display().to_string()];
        let stamp = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);
        let snapshot = dir.join("ready.nav.lz4b");
        let index = dir.join(FILE_NAME);
        write_catalog_navigation_snapshot(&snapshot, &catalog(&root), &stamp).unwrap();
        publish_from_snapshot_at(&snapshot, &index).unwrap();

        std::fs::write(&mame, b"changed metadata").unwrap();
        assert!(matches!(
            probe_at(&root, &index, &mame, &hbmame),
            ProbeResult::Miss { reason, .. } if reason == "live-input-mismatch"
        ));
        std::fs::write(&index, b"corrupt").unwrap();
        assert!(matches!(
            probe_at(&root, &index, &mame, &hbmame),
            ProbeResult::Miss { reason, .. } if reason.starts_with("decode-error:")
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_and_oversize_indexes_are_bounded_misses() {
        let dir = unique_dir("arcade-bootstrap-bounds");
        let root = dir.join("_Arcade");
        std::fs::create_dir_all(&root).unwrap();
        let index = dir.join(FILE_NAME);
        assert!(matches!(
            probe_at(&root, &index, &dir.join("mame"), &dir.join("hbmame")),
            ProbeResult::Miss { reason, .. } if reason == "missing"
        ));
        let file = std::fs::File::create(&index).unwrap();
        file.set_len(MAX_INDEX_BYTES + 1).unwrap();
        assert!(matches!(
            probe_at(&root, &index, &dir.join("mame"), &dir.join("hbmame")),
            ProbeResult::Miss { reason, .. } if reason.starts_with("oversize:")
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}
