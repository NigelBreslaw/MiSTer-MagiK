// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Root-level catalog stamp support.
//!
//! Warm cached validation uses this module to decide whether the full catalog
//! builder needs to run.

use crate::catalog_config::{
    CATALOG_BUILD_VERSION, SCHEMA_VERSION, default_hbmame_sqlite_path, default_mame_sqlite_path,
    default_runtime_metadata_path,
};
use crate::core_audit::CatalogAuditRow;
use crate::launch_profiles::{
    CORE_LAUNCH_MANIFEST_VERSION, PROFILE_SET_VERSION, core_launch_manifest_fingerprint,
};
use crate::prepared_collections::PREPARED_COLLECTION_ADAPTER_VERSION;
use std::path::Path;

const STAMP_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const STAMP_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogStamp {
    lines: Vec<String>,
}

impl CatalogStamp {
    pub fn from_lines(lines: Vec<String>) -> Self {
        Self { lines }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hash = STAMP_HASH_OFFSET;
        for line in &self.lines {
            hash = fnv64_update(hash, line.as_bytes());
            hash = fnv64_update(hash, &[0xff]);
        }
        hash
    }

    pub fn fingerprint_hex(&self) -> String {
        format!("{:016x}", self.fingerprint())
    }

    pub(crate) fn has_same_live_inputs(&self, current: &Self) -> bool {
        fn is_retained_audit_line(line: &str) -> bool {
            line.starts_with("core-audit\t") || line.starts_with("core-audit-row\t")
        }
        self.lines
            .iter()
            .filter(|line| !is_retained_audit_line(line))
            .eq(current
                .lines
                .iter()
                .filter(|line| !is_retained_audit_line(line)))
    }
}

pub fn compute_default_catalog_stamp(roots: &[String]) -> CatalogStamp {
    compute_catalog_stamp_for_paths(
        roots,
        &default_mame_sqlite_path(),
        &default_hbmame_sqlite_path(),
    )
}

pub(crate) fn compute_default_catalog_stamp_with_audit(
    roots: &[String],
    audit_rows: &[CatalogAuditRow],
) -> CatalogStamp {
    compute_catalog_stamp_for_paths_with_audit(
        roots,
        &default_mame_sqlite_path(),
        &default_hbmame_sqlite_path(),
        audit_rows,
    )
}

pub fn compute_catalog_stamp_for_paths(
    roots: &[String],
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
) -> CatalogStamp {
    compute_catalog_stamp_for_paths_with_audit(roots, mame_sqlite_path, hbmame_sqlite_path, &[])
}

pub(crate) fn compute_catalog_stamp_for_paths_with_audit(
    roots: &[String],
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
    audit_rows: &[CatalogAuditRow],
) -> CatalogStamp {
    let mut lines = vec![
        format!("schema\t{SCHEMA_VERSION}"),
        format!("catalog-build\t{CATALOG_BUILD_VERSION}"),
        format!("profile-set\t{PROFILE_SET_VERSION}"),
        format!("prepared-collection-adapters\t{PREPARED_COLLECTION_ADAPTER_VERSION}"),
        format!(
            "core-launch-manifest\t{}\t{:016x}",
            CORE_LAUNCH_MANIFEST_VERSION,
            core_launch_manifest_fingerprint()
        ),
        format!("roots\t{}", roots.len()),
    ];
    for (idx, root) in roots.iter().enumerate() {
        append_path_signature(&mut lines, "root", idx, Path::new(root));
    }
    append_prepared_collection_root_signatures(&mut lines, roots);
    let rom_inventory = crate::arcade_rom_inventory::ArcadeRomInventory::from_library_roots(roots);
    let (mame_roms, hbmame_roms) = rom_inventory.counts();
    lines.push(format!(
        "arcade-rom-inventory\t{}\t{mame_roms}\t{hbmame_roms}",
        rom_inventory.fingerprint()
    ));
    lines.push("stamp-targets\t0".to_string());
    lines.push(format!("core-audit\t{}", audit_rows.len()));
    for (idx, row) in audit_rows.iter().enumerate() {
        lines.push(format!(
            "core-audit-row\t{idx}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.core_id,
            row.core_path,
            row.expected_game_dir,
            row.extensions,
            row.mount_kind,
            row.source,
            row.catalog_status,
            row.reason
        ));
    }
    append_named_file_signature(&mut lines, "mame-metadata", mame_sqlite_path);
    append_named_file_signature(&mut lines, "hbmame-metadata", hbmame_sqlite_path);
    append_runtime_metadata_signature(&mut lines, &default_runtime_metadata_path());
    CatalogStamp { lines }
}

fn append_runtime_metadata_signature(lines: &mut Vec<String>, path: &Path) {
    let Ok(store) = crate::runtime_metadata::MetadataStore::open(path) else {
        append_named_file_signature(lines, "runtime-metadata", path);
        return;
    };
    lines.push(format!(
        "runtime-metadata\t{}\t{}\t{}",
        path.display(),
        store.status().file_len,
        store.status().shard_count
    ));
    for (id, digest) in store.shard_digests() {
        lines.push(format!(
            "runtime-metadata-shard\t{id}\t{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ));
    }
}

fn append_prepared_collection_root_signatures(lines: &mut Vec<String>, roots: &[String]) {
    for (idx, root) in crate::prepared_collections::storage_roots_for_library_roots(roots)
        .iter()
        .enumerate()
    {
        for (name, path) in [
            ("0mhz-root", root.join("_DOS Games")),
            ("neon68k-root", root.join("_Computer/_X68000 Games")),
            ("neon68k-legacy-root", root.join("_Computer/X68000 Games")),
            ("oneload64-root", root.join("games/C64")),
        ] {
            append_path_signature(lines, name, idx, &path);
        }
        for (name, path) in [
            ("amigavision-mgl", root.join("_Computer/Amiga.mgl")),
            ("amigavision-500-mgl", root.join("_Computer/Amiga 500.mgl")),
            ("megaags-mgl", root.join("_Computer/MegaAGS.mgl")),
            (
                "amigavision-games",
                root.join("games/Amiga/listings/games.txt"),
            ),
            (
                "amigavision-demos",
                root.join("games/Amiga/listings/demos.txt"),
            ),
        ] {
            append_named_file_signature(lines, &format!("prepared-{idx}-{name}"), &path);
        }
        for (name, path) in [
            ("amigavision-hdf", root.join("games/Amiga/AmigaVision.hdf")),
            ("megaags-hdf", root.join("games/Amiga/MegaAGS.hdf")),
        ] {
            append_named_sized_file_signature(lines, &format!("prepared-{idx}-{name}"), &path);
        }
    }
}

fn append_path_signature(lines: &mut Vec<String>, kind: &str, idx: usize, path: &Path) {
    match std::fs::metadata(path) {
        Ok(meta) => lines.push(format!(
            "{kind}\t{idx}\t{}\t{}\t{}\t{}",
            path.display(),
            if meta.is_dir() { "dir" } else { "not-dir" },
            meta.len(),
            mtime_nanos(&meta)
        )),
        Err(_) => lines.push(format!("{kind}\t{idx}\t{}\tmissing", path.display())),
    }
}

fn append_named_file_signature(lines: &mut Vec<String>, name: &str, path: &Path) {
    match std::fs::metadata(path) {
        Ok(meta) => lines.push(format!(
            "{name}\t{}\t{}\t{}",
            path.display(),
            meta.len(),
            mtime_nanos(&meta)
        )),
        Err(_) => lines.push(format!("{name}\t{}\tmissing", path.display())),
    }
}

fn append_named_sized_file_signature(lines: &mut Vec<String>, name: &str, path: &Path) {
    match std::fs::metadata(path) {
        Ok(meta) => lines.push(format!("{name}\t{}\t{}", path.display(), meta.len())),
        Err(_) => lines.push(format!("{name}\t{}\tmissing", path.display())),
    }
}

fn mtime_nanos(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn fnv64_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(STAMP_HASH_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn matching_catalog_stamp_has_stable_fingerprint() {
        let root = unique_temp_dir("stamp-stable");
        let system = root.join("games/NES");
        std::fs::create_dir_all(&system).expect("create system dir");
        set_mtime_for_test(&system, 10, 0);
        let mame = root.join("mame.sqlite3");
        let hbmame = root.join("hbmame.sqlite3");
        std::fs::write(&mame, b"mame").expect("write mame");
        std::fs::write(&hbmame, b"hbmame").expect("write hbmame");
        let roots = vec![root.display().to_string()];
        let first = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);
        let second = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);

        assert_eq!(first, second);
        assert_eq!(first.fingerprint_hex(), second.fingerprint_hex());
    }

    #[test]
    fn root_directory_metadata_changes_catalog_stamp() {
        let root = unique_temp_dir("stamp-root-change");
        set_mtime_for_test(&root, 10, 0);
        let roots = vec![root.display().to_string()];
        let first =
            compute_catalog_stamp_for_paths(&roots, &root.join("mame"), &root.join("hbmame"));

        set_mtime_for_test(&root, 20, 0);
        let second =
            compute_catalog_stamp_for_paths(&roots, &root.join("mame"), &root.join("hbmame"));

        assert_ne!(first.fingerprint_hex(), second.fingerprint_hex());
    }

    #[test]
    fn nested_prepared_launcher_change_keeps_root_stamp_stable() {
        let root = unique_temp_dir("stamp-prepared-launcher");
        let games = root.join("games");
        let dos = root.join("_DOS Games");
        std::fs::create_dir_all(&games).expect("create games dir");
        std::fs::create_dir_all(&dos).expect("create DOS dir");
        let launcher = dos.join("Doom.mgl");
        std::fs::write(&launcher, b"first").expect("write launcher");
        let mame = root.join("mame.sqlite3");
        let hbmame = root.join("hbmame.sqlite3");
        let roots = vec![games.display().to_string()];

        let first = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);
        std::fs::write(&launcher, b"second-version").expect("update launcher");
        let second = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);

        assert_eq!(first.fingerprint(), second.fingerprint());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn writable_amigavision_hdf_change_does_not_invalidate_root_stamp() {
        let root = unique_temp_dir("stamp-amigavision-writable-hdf");
        let games = root.join("games");
        let hdf = root.join("games/Amiga/AmigaVision.hdf");
        std::fs::create_dir_all(hdf.parent().expect("HDF parent")).expect("create Amiga dir");
        std::fs::write(&hdf, b"boot-hdf").expect("write HDF");
        let roots = vec![games.display().to_string()];
        let mame = root.join("mame.sqlite3");
        let hbmame = root.join("hbmame.sqlite3");

        set_mtime_for_test(&hdf, 10, 0);
        let first = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);
        set_mtime_for_test(&hdf, 20, 0);
        let runtime_write = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);
        assert_eq!(first.fingerprint_hex(), runtime_write.fingerprint_hex());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stamp_uses_roots_without_enumerating_nested_game_dirs() {
        let root = unique_temp_dir("stamp-roots-only");
        let system = root.join("games/NES");
        let nested = system.join("Nested Game");
        std::fs::create_dir_all(&nested).expect("create nested game dir");
        let roots = vec![root.display().to_string()];

        let stamp =
            compute_catalog_stamp_for_paths(&roots, &root.join("mame"), &root.join("hbmame"));

        assert!(
            stamp
                .lines()
                .iter()
                .any(|line| line.contains(&root.display().to_string()))
        );
        assert!(stamp.lines().iter().any(|line| line == "stamp-targets\t0"));
        assert!(
            !stamp
                .lines()
                .iter()
                .any(|line| line.contains(&system.display().to_string()))
        );
        assert!(
            !stamp
                .lines()
                .iter()
                .any(|line| line.contains(&nested.display().to_string()))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stamp_records_core_launch_manifest_version_and_fingerprint() {
        let root = unique_temp_dir("stamp-core-manifest");
        let roots = vec![root.display().to_string()];

        let stamp =
            compute_catalog_stamp_for_paths(&roots, &root.join("mame"), &root.join("hbmame"));

        assert!(stamp.lines().iter().any(|line| {
            line.starts_with(&format!(
                "core-launch-manifest\t{}\t",
                CORE_LAUNCH_MANIFEST_VERSION
            ))
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn root_missing_changes_catalog_stamp() {
        let root = unique_temp_dir("stamp-root-missing");
        let roots = vec![root.display().to_string()];
        let first =
            compute_catalog_stamp_for_paths(&roots, &root.join("mame"), &root.join("hbmame"));

        std::fs::remove_dir_all(&root).expect("remove root");
        let second =
            compute_catalog_stamp_for_paths(&roots, &root.join("mame"), &root.join("hbmame"));

        assert_ne!(first.fingerprint_hex(), second.fingerprint_hex());
        assert!(
            second
                .lines()
                .iter()
                .any(|line| line.ends_with("\tmissing"))
        );
    }

    #[test]
    fn metadata_changes_catalog_stamp_but_preview_packs_are_runtime_only() {
        let root = unique_temp_dir("stamp-input-change");
        let mame = root.join("mame.sqlite3");
        let hbmame = root.join("hbmame.sqlite3");
        std::fs::write(&mame, b"mame").expect("write mame");
        std::fs::write(&hbmame, b"hbmame").expect("write hbmame");
        let roots = vec![root.display().to_string()];

        let first = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);
        std::fs::write(&mame, b"changed").expect("change mame");
        let second = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);
        let third = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame);

        assert_ne!(first.fingerprint_hex(), second.fingerprint_hex());
        assert_eq!(second.fingerprint_hex(), third.fingerprint_hex());
        assert!(
            !third
                .lines()
                .iter()
                .any(|line| line.starts_with("preview-pack"))
        );
    }

    #[test]
    fn audit_rows_change_catalog_stamp() {
        let root = unique_temp_dir("stamp-audit-change");
        let roots = vec![root.display().to_string()];
        let mame = root.join("mame.sqlite3");
        let hbmame = root.join("hbmame.sqlite3");
        let base = compute_catalog_stamp_for_paths_with_audit(&roots, &mame, &hbmame, &[]);
        let audit = vec![CatalogAuditRow {
            core_id: "WonderSwanColor".to_string(),
            core_path: "/media/fat/_Console/WonderSwanColor_20260629.rbf".to_string(),
            expected_game_dir: "games/WonderSwanColor".to_string(),
            extensions: "wsc".to_string(),
            mount_kind: "load-file".to_string(),
            source: "main-derived".to_string(),
            catalog_status: "uncataloged".to_string(),
            reason: "installed-core-has-no-catalog-profile".to_string(),
        }];
        let changed = compute_catalog_stamp_for_paths_with_audit(&roots, &mame, &hbmame, &audit);

        assert_ne!(base.fingerprint_hex(), changed.fingerprint_hex());
        assert!(
            changed
                .lines()
                .iter()
                .any(|line| line.contains("WonderSwanColor"))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn unique_temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mister-magik-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[cfg(unix)]
    fn set_mtime_for_test(path: &Path, sec: i64, nsec: i64) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_path = CString::new(path.as_os_str().as_bytes()).expect("path cstring");
        let times = [
            libc::timespec {
                tv_sec: sec as libc::time_t,
                tv_nsec: nsec as libc::c_long,
            },
            libc::timespec {
                tv_sec: sec as libc::time_t,
                tv_nsec: nsec as libc::c_long,
            },
        ];
        // SAFETY: c_path is a NUL-terminated CString, and times points to two
        // initialized timespec values that live for the duration of the syscall.
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "utimensat failed for {}", path.display());
    }

    #[cfg(not(unix))]
    fn set_mtime_for_test(_path: &Path, _sec: i64, _nsec: i64) {}
}
