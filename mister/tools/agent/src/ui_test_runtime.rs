// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Volatile, content-addressed storage for attended UI-test runtimes.

use mister_magik_agent_protocol::UiTestRuntimeSpec;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const DEFAULT_ROOT: &str = "/tmp/mister-magik/ui-tests";
const COPY_BUFFER_BYTES: usize = 64 * 1024;
static RECEIVE_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CachedRuntime {
    pub(crate) path: PathBuf,
    pub(crate) reused: bool,
}

struct ReceiveGuard;

impl ReceiveGuard {
    fn claim() -> Result<Self, String> {
        RECEIVE_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "UI-test runtime receive is already active".to_string())
    }
}

impl Drop for ReceiveGuard {
    fn drop(&mut self) {
        RECEIVE_ACTIVE.store(false, Ordering::Release);
    }
}

#[allow(dead_code)]
pub(crate) fn prepare(spec: &UiTestRuntimeSpec) -> Result<Option<CachedRuntime>, String> {
    prepare_at(Path::new(DEFAULT_ROOT), spec)
}

#[allow(dead_code)]
pub(crate) fn receive(
    reader: &mut impl Read,
    spec: &UiTestRuntimeSpec,
) -> Result<CachedRuntime, String> {
    receive_at(reader, Path::new(DEFAULT_ROOT), spec)
}

fn prepare_at(root: &Path, spec: &UiTestRuntimeSpec) -> Result<Option<CachedRuntime>, String> {
    let path = runtime_path(root, spec);
    let metadata = metadata_path(root, spec);
    if !path.is_file() || !metadata.is_file() {
        return Ok(None);
    }
    let metadata_text = fs::read_to_string(&metadata)
        .map_err(|error| format!("read UI-test runtime metadata: {error}"))?;
    if metadata_text != metadata_value(spec) {
        return Ok(None);
    }
    let actual_bytes = fs::metadata(&path)
        .map_err(|error| format!("stat cached UI-test runtime: {error}"))?
        .len();
    if actual_bytes != spec.payload_bytes {
        return Ok(None);
    }
    if file_sha256(&path)? != spec.sha256 {
        return Ok(None);
    }
    Ok(Some(CachedRuntime { path, reused: true }))
}

fn receive_at(
    reader: &mut impl Read,
    root: &Path,
    spec: &UiTestRuntimeSpec,
) -> Result<CachedRuntime, String> {
    let _guard = ReceiveGuard::claim()?;
    if let Some(cached) = prepare_at(root, spec)? {
        return Ok(cached);
    }
    fs::create_dir_all(root).map_err(|error| format!("create UI-test runtime cache: {error}"))?;
    for path in [
        runtime_path(root, spec),
        part_path(root, spec),
        metadata_path(root, spec),
    ] {
        if path.is_file() {
            fs::remove_file(&path)
                .map_err(|error| format!("replace invalid UI-test runtime cache: {error}"))?;
        }
    }
    remove_stale_entries(root, spec)?;
    let path = runtime_path(root, spec);
    let part = part_path(root, spec);
    let metadata = metadata_path(root, spec);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .map_err(|error| format!("create UI-test runtime part: {error}"))?;
    let result = receive_to_part(reader, spec, &mut output);
    drop(output);
    if let Err(error) = result {
        let _ = fs::remove_file(&part);
        return Err(error);
    }
    set_executable(&part)?;
    fs::rename(&part, &path).map_err(|error| {
        let _ = fs::remove_file(&part);
        format!("publish UI-test runtime: {error}")
    })?;
    fs::write(&metadata, metadata_value(spec))
        .map_err(|error| format!("publish UI-test runtime metadata: {error}"))?;
    Ok(CachedRuntime {
        path,
        reused: false,
    })
}

fn receive_to_part(
    reader: &mut impl Read,
    spec: &UiTestRuntimeSpec,
    output: &mut File,
) -> Result<(), String> {
    let mut remaining = spec.payload_bytes;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    while remaining != 0 {
        let limit = usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64))
            .map_err(|_| "UI-test runtime byte count overflows host usize".to_string())?;
        let read = reader
            .read(&mut buffer[..limit])
            .map_err(|error| format!("read UI-test runtime payload: {error}"))?;
        if read == 0 {
            return Err(format!(
                "UI-test runtime payload truncated with {remaining} bytes remaining"
            ));
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| format!("write UI-test runtime payload: {error}"))?;
        hasher.update(&buffer[..read]);
        remaining = remaining.saturating_sub(read as u64);
    }
    let actual_sha256 = hex_digest(hasher.finalize());
    if actual_sha256 != spec.sha256 {
        return Err(format!(
            "UI-test runtime SHA-256 mismatch expected={} actual={actual_sha256}",
            spec.sha256
        ));
    }
    Ok(())
}

fn remove_stale_entries(root: &Path, spec: &UiTestRuntimeSpec) -> Result<(), String> {
    let expected = runtime_path(root, spec);
    let expected_part = part_path(root, spec);
    let expected_metadata = metadata_path(root, spec);
    let entries =
        fs::read_dir(root).map_err(|error| format!("list UI-test runtime cache: {error}"))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("read UI-test runtime cache entry: {error}"))?
            .path();
        if path == expected || path == expected_part || path == expected_metadata {
            continue;
        }
        if path.extension().is_some_and(|extension| {
            extension == "runtime" || extension == "part" || extension == "json"
        }) {
            fs::remove_file(&path)
                .map_err(|error| format!("remove stale UI-test runtime: {error}"))?;
        }
    }
    Ok(())
}

fn runtime_path(root: &Path, spec: &UiTestRuntimeSpec) -> PathBuf {
    root.join(format!("{}.runtime", spec.sha256))
}

fn part_path(root: &Path, spec: &UiTestRuntimeSpec) -> PathBuf {
    root.join(format!("{}.part", spec.sha256))
}

fn metadata_path(root: &Path, spec: &UiTestRuntimeSpec) -> PathBuf {
    root.join(format!("{}.json", spec.sha256))
}

fn metadata_value(spec: &UiTestRuntimeSpec) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\n",
        spec.payload_bytes,
        spec.sha256,
        spec.source_revision,
        spec.profile,
        spec.features.join(",")
    )
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut input = File::open(path)
        .map_err(|error| format!("open cached UI-test runtime for verification: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| format!("read cached UI-test runtime for verification: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_digest(hasher.finalize()))
}

fn set_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|error| format!("stat UI-test runtime: {error}"))?
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("make UI-test runtime executable: {error}"))?;
    }
    Ok(())
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn spec(payload: &[u8]) -> UiTestRuntimeSpec {
        UiTestRuntimeSpec {
            payload_bytes: payload.len() as u64,
            sha256: hex_digest(Sha256::digest(payload)),
            source_revision: "deadbeef".into(),
            profile: "release-device-ui-tests".into(),
            features: vec!["ui".into(), "ui-device-tests".into()],
        }
    }

    fn root() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("mister-magik-ui-test-runtime-{stamp}"))
    }

    #[test]
    fn receive_verifies_and_reuses_exact_runtime() {
        let payload = b"runtime";
        let root = root();
        let spec = spec(payload);
        let first = receive_at(&mut Cursor::new(payload), &root, &spec).unwrap();
        assert!(!first.reused);
        assert!(first.path.is_file());
        assert_eq!(
            prepare_at(&root, &spec).unwrap(),
            Some(CachedRuntime {
                path: first.path.clone(),
                reused: true,
            })
        );
        let second = receive_at(&mut Cursor::new(b"ignored"), &root, &spec).unwrap();
        assert!(second.reused);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn receive_rejects_truncation_and_hash_mismatch() {
        let root = root();
        let payload = b"runtime";
        let mut short = spec(payload);
        short.payload_bytes += 1;
        assert!(receive_at(&mut Cursor::new(payload), &root, &short).is_err());
        let mut wrong = spec(payload);
        wrong.sha256 = "a".repeat(64);
        assert!(receive_at(&mut Cursor::new(payload), &root, &wrong).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn receive_leaves_following_session_bytes_unread() {
        let root = root();
        let payload = b"runtime";
        let mut framed = Cursor::new(b"runtime-next".to_vec());

        let received = receive_at(&mut framed, &root, &spec(payload)).unwrap();
        let mut following = Vec::new();
        framed.read_to_end(&mut following).unwrap();

        assert!(!received.reused);
        assert_eq!(following, b"-next");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prepare_rejects_corrupted_runtime_and_allows_replacement() {
        let payload = b"runtime";
        let root = root();
        let spec = spec(payload);
        let first = receive_at(&mut Cursor::new(payload), &root, &spec).unwrap();
        fs::write(&first.path, b"corrupt").unwrap();
        assert_eq!(prepare_at(&root, &spec).unwrap(), None);
        let replacement = receive_at(&mut Cursor::new(payload), &root, &spec).unwrap();
        assert!(!replacement.reused);
        assert_eq!(fs::read(replacement.path).unwrap(), payload);
        let _ = fs::remove_dir_all(root);
    }
}
