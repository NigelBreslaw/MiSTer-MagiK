//! Root-level catalog stamp support.
//!
//! Warm cached validation uses this module to decide whether the full catalog
//! builder needs to run.

use crate::catalog_config::{
    default_hbmame_sqlite_path, default_mame_sqlite_path, CATALOG_BUILD_VERSION, SCHEMA_VERSION,
};
use crate::launch_profiles::PROFILE_SET_VERSION;
use crate::preview_worker;
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
        preview_worker::preview_archive_fingerprints_from_env(),
    )
}

pub fn compute_catalog_stamp_for_paths(
    roots: &[String],
    mame_sqlite_path: &Path,
    hbmame_sqlite_path: &Path,
    preview_fingerprints: Result<Vec<(String, u64, i64)>, String>,
) -> CatalogStamp {
    let mut lines = vec![
        format!("schema\t{SCHEMA_VERSION}"),
        format!("catalog-build\t{CATALOG_BUILD_VERSION}"),
        format!("profile-set\t{PROFILE_SET_VERSION}"),
        format!("roots\t{}", roots.len()),
    ];
    for (idx, root) in roots.iter().enumerate() {
        append_path_signature(&mut lines, "root", idx, Path::new(root));
        append_immediate_child_dirs(&mut lines, idx, Path::new(root));
    }
    append_named_file_signature(&mut lines, "mame-metadata", mame_sqlite_path);
    append_named_file_signature(&mut lines, "hbmame-metadata", hbmame_sqlite_path);
    append_preview_fingerprints(&mut lines, preview_fingerprints);
    CatalogStamp { lines }
}

fn append_immediate_child_dirs(lines: &mut Vec<String>, root_idx: usize, root: &Path) {
    let Ok(read_dir) = std::fs::read_dir(root) else {
        return;
    };
    let mut children = read_dir
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|ty| ty.is_dir()).unwrap_or(false))
        .filter(|entry| !should_ignore_stamp_dir_name(&entry.file_name().to_string_lossy()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    children.sort_by_key(|path| path.file_name().map(|name| name.to_os_string()));
    for child in children {
        append_path_signature(lines, "child-dir", root_idx, &child);
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

fn append_preview_fingerprints(
    lines: &mut Vec<String>,
    preview_fingerprints: Result<Vec<(String, u64, i64)>, String>,
) {
    match preview_fingerprints {
        Ok(mut fingerprints) => {
            fingerprints.sort_by(|a, b| a.0.cmp(&b.0));
            lines.push(format!("preview-packs\t{}", fingerprints.len()));
            for (path, size, mtime) in fingerprints {
                lines.push(format!("preview-pack\t{path}\t{size}\t{mtime}"));
            }
        }
        Err(_) => lines.push("preview-packs\terror".to_string()),
    }
}

fn should_ignore_stamp_dir_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "images"
            | "manuals"
            | "screenshot"
            | "screenshots"
            | "screenshot-magik"
            | "_organized"
            | "boxart"
            | "__macosx"
            | ".____padding_file"
    ) || name.starts_with("._")
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
        let system = root.join("NES");
        std::fs::create_dir_all(&system).expect("create system dir");
        set_mtime_for_test(&system, 10, 0);
        let mame = root.join("mame.sqlite3");
        let hbmame = root.join("hbmame.sqlite3");
        std::fs::write(&mame, b"mame").expect("write mame");
        std::fs::write(&hbmame, b"hbmame").expect("write hbmame");
        let roots = vec![root.display().to_string()];
        let preview = Ok(vec![("/tmp/screens.mmlz4b".to_string(), 12, 34)]);

        let first = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame, preview.clone());
        let second = compute_catalog_stamp_for_paths(&roots, &mame, &hbmame, preview);

        assert_eq!(first, second);
        assert_eq!(first.fingerprint_hex(), second.fingerprint_hex());
    }

    #[test]
    fn child_directory_metadata_changes_catalog_stamp() {
        let root = unique_temp_dir("stamp-child-change");
        let system = root.join("SNES");
        std::fs::create_dir_all(&system).expect("create system dir");
        set_mtime_for_test(&system, 10, 0);
        let roots = vec![root.display().to_string()];
        let first = compute_catalog_stamp_for_paths(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            Ok(Vec::new()),
        );

        set_mtime_for_test(&system, 20, 0);
        let second = compute_catalog_stamp_for_paths(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            Ok(Vec::new()),
        );

        assert_ne!(first.fingerprint_hex(), second.fingerprint_hex());
    }

    #[test]
    fn root_missing_changes_catalog_stamp() {
        let root = unique_temp_dir("stamp-root-missing");
        let roots = vec![root.display().to_string()];
        let first = compute_catalog_stamp_for_paths(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            Ok(Vec::new()),
        );

        std::fs::remove_dir_all(&root).expect("remove root");
        let second = compute_catalog_stamp_for_paths(
            &roots,
            &root.join("mame"),
            &root.join("hbmame"),
            Ok(Vec::new()),
        );

        assert_ne!(first.fingerprint_hex(), second.fingerprint_hex());
        assert!(second
            .lines()
            .iter()
            .any(|line| line.ends_with("\tmissing")));
    }

    #[test]
    fn metadata_and_preview_changes_catalog_stamp() {
        let root = unique_temp_dir("stamp-input-change");
        let mame = root.join("mame.sqlite3");
        let hbmame = root.join("hbmame.sqlite3");
        std::fs::write(&mame, b"mame").expect("write mame");
        std::fs::write(&hbmame, b"hbmame").expect("write hbmame");
        let roots = vec![root.display().to_string()];

        let first = compute_catalog_stamp_for_paths(
            &roots,
            &mame,
            &hbmame,
            Ok(vec![("preview-a".to_string(), 1, 1)]),
        );
        std::fs::write(&mame, b"changed").expect("change mame");
        let second = compute_catalog_stamp_for_paths(
            &roots,
            &mame,
            &hbmame,
            Ok(vec![("preview-a".to_string(), 1, 1)]),
        );
        let third = compute_catalog_stamp_for_paths(
            &roots,
            &mame,
            &hbmame,
            Ok(vec![("preview-a".to_string(), 2, 1)]),
        );

        assert_ne!(first.fingerprint_hex(), second.fingerprint_hex());
        assert_ne!(second.fingerprint_hex(), third.fingerprint_hex());
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
