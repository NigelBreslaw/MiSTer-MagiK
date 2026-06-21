//! Root-level catalog stamp support.
//!
//! Warm cached validation uses this module to decide whether the full catalog
//! builder needs to run.

use crate::catalog_config::{
    default_hbmame_sqlite_path, default_mame_sqlite_path, CATALOG_BUILD_VERSION, SCHEMA_VERSION,
};
use crate::launch_profiles::PROFILE_SET_VERSION;
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
}

pub fn compute_default_catalog_stamp(roots: &[String]) -> CatalogStamp {
    compute_catalog_stamp_for_paths(
        roots,
        &default_mame_sqlite_path(),
        &default_hbmame_sqlite_path(),
    )
}

pub fn compute_catalog_stamp_for_paths(
    roots: &[String],
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
) -> CatalogStamp {
    let mut lines = vec![
        format!("schema\t{SCHEMA_VERSION}"),
        format!("catalog-build\t{CATALOG_BUILD_VERSION}"),
        format!("profile-set\t{PROFILE_SET_VERSION}"),
        format!("roots\t{}", roots.len()),
    ];
    for (idx, root) in roots.iter().enumerate() {
        append_path_signature(&mut lines, "root", idx, Path::new(root));
    }
    lines.push("stamp-targets\t0".to_string());
    append_named_file_signature(&mut lines, "mame-metadata", mame_sqlite_path);
    append_named_file_signature(&mut lines, "hbmame-metadata", hbmame_sqlite_path);
    CatalogStamp { lines }
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
    fn stamp_uses_roots_without_enumerating_nested_game_dirs() {
        let root = unique_temp_dir("stamp-roots-only");
        let system = root.join("games/NES");
        let nested = system.join("Nested Game");
        std::fs::create_dir_all(&nested).expect("create nested game dir");
        let roots = vec![root.display().to_string()];

        let stamp =
            compute_catalog_stamp_for_paths(&roots, &root.join("mame"), &root.join("hbmame"));

        assert!(stamp
            .lines()
            .iter()
            .any(|line| line.contains(&root.display().to_string())));
        assert!(stamp.lines().iter().any(|line| line == "stamp-targets\t0"));
        assert!(!stamp
            .lines()
            .iter()
            .any(|line| line.contains(&system.display().to_string())));
        assert!(!stamp
            .lines()
            .iter()
            .any(|line| line.contains(&nested.display().to_string())));
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
        assert!(second
            .lines()
            .iter()
            .any(|line| line.ends_with("\tmissing")));
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
        assert!(!third
            .lines()
            .iter()
            .any(|line| line.starts_with("preview-pack")));
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
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "utimensat failed for {}", path.display());
    }

    #[cfg(not(unix))]
    fn set_mtime_for_test(_path: &Path, _sec: i64, _nsec: i64) {}
}
