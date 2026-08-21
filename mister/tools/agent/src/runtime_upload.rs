// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_magik_agent_protocol::RuntimeUploadSpec;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const COPY_BUFFER_BYTES: usize = 64 * 1024;
static RUNTIME_UPLOAD_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UploadFailureKind {
    Busy,
    Artifact,
    Operation,
}

#[derive(Debug)]
pub struct UploadFailure {
    pub kind: UploadFailureKind,
    pub message: String,
}

impl UploadFailure {
    fn busy(message: impl Into<String>) -> Self {
        Self {
            kind: UploadFailureKind::Busy,
            message: message.into(),
        }
    }

    fn artifact(message: impl Into<String>) -> Self {
        Self {
            kind: UploadFailureKind::Artifact,
            message: message.into(),
        }
    }

    fn operation(message: impl Into<String>) -> Self {
        Self {
            kind: UploadFailureKind::Operation,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadResult {
    pub payload_bytes: u64,
    pub sha256: String,
    pub receive_ms: u64,
    pub bytes_per_second: u64,
}

pub struct UploadPaths {
    pub lock: PathBuf,
    pub upload: PathBuf,
    pub part: PathBuf,
}

struct ActiveUpload;

impl ActiveUpload {
    fn claim() -> Result<Self, UploadFailure> {
        RUNTIME_UPLOAD_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| UploadFailure::busy("runtime upload is already active"))
    }
}

impl Drop for ActiveUpload {
    fn drop(&mut self) {
        RUNTIME_UPLOAD_ACTIVE.store(false, Ordering::Release);
    }
}

pub fn receive(
    reader: &mut impl Read,
    spec: &RuntimeUploadSpec,
    paths: &UploadPaths,
) -> Result<UploadResult, UploadFailure> {
    let _active = ActiveUpload::claim()?;
    if !paths.lock.is_file() {
        return Err(UploadFailure::busy(
            "runtime upload requires the active development deploy lock",
        ));
    }
    if paths.upload.exists() || paths.part.exists() {
        return Err(UploadFailure::busy(
            "runtime upload staging path already exists",
        ));
    }

    let mut part = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&paths.part)
        .map_err(|error| {
            UploadFailure::operation(format!("create runtime upload part: {error}"))
        })?;
    let result = receive_to_part(reader, spec, &mut part, paths);
    if result.is_err() {
        let _ = fs::remove_file(&paths.part);
    }
    result
}

fn receive_to_part(
    reader: &mut impl Read,
    spec: &RuntimeUploadSpec,
    part: &mut File,
    paths: &UploadPaths,
) -> Result<UploadResult, UploadFailure> {
    let started = Instant::now();
    let mut remaining = spec.payload_bytes;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    while remaining != 0 {
        let limit = usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64))
            .expect("copy buffer limit fits usize");
        let read = reader
            .read(&mut buffer[..limit])
            .map_err(|error| UploadFailure::operation(format!("read runtime payload: {error}")))?;
        if read == 0 {
            return Err(UploadFailure::artifact(format!(
                "runtime payload truncated with {remaining} bytes remaining"
            )));
        }
        part.write_all(&buffer[..read]).map_err(|error| {
            UploadFailure::operation(format!("write runtime upload part: {error}"))
        })?;
        hasher.update(&buffer[..read]);
        remaining = remaining.saturating_sub(read as u64);
    }

    let mut surplus = [0_u8; 1];
    if reader
        .read(&mut surplus)
        .map_err(|error| UploadFailure::operation(format!("finish runtime payload: {error}")))?
        != 0
    {
        return Err(UploadFailure::artifact(
            "runtime payload contains bytes beyond payload_bytes",
        ));
    }

    let actual_sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual_sha256 != spec.sha256 {
        return Err(UploadFailure::artifact(format!(
            "runtime payload SHA-256 mismatch expected={} actual={actual_sha256}",
            spec.sha256
        )));
    }
    part.sync_all()
        .map_err(|error| UploadFailure::operation(format!("sync runtime upload part: {error}")))?;
    fs::rename(&paths.part, &paths.upload)
        .map_err(|error| UploadFailure::operation(format!("stage runtime upload: {error}")))?;
    sync_parent(&paths.upload).map_err(|error| {
        UploadFailure::operation(format!("sync runtime upload directory: {error}"))
    })?;

    let receive_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let bytes_per_second = if receive_ms == 0 {
        spec.payload_bytes.saturating_mul(1_000)
    } else {
        spec.payload_bytes.saturating_mul(1_000) / receive_ms
    };
    Ok(UploadResult {
        payload_bytes: spec.payload_bytes,
        sha256: actual_sha256,
        receive_ms,
        bytes_per_second,
    })
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "runtime upload has no parent")
    })?;
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, MutexGuard};

    static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);
    static TEST_UPLOAD: Mutex<()> = Mutex::new(());

    fn serialize_uploads() -> MutexGuard<'static, ()> {
        TEST_UPLOAD.lock().unwrap()
    }

    struct Fixture {
        root: PathBuf,
        paths: UploadPaths,
    }

    impl Fixture {
        fn new() -> Self {
            let id = FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "mister-magik-runtime-upload-{}-{id}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            let lock = root.join("deploy.lock");
            File::create(&lock).unwrap();
            Self {
                paths: UploadPaths {
                    lock,
                    upload: root.join("mister-magik-fb.upload"),
                    part: root.join("mister-magik-fb.upload.part"),
                },
                root,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn spec(payload: &[u8]) -> RuntimeUploadSpec {
        RuntimeUploadSpec {
            payload_bytes: payload.len() as u64,
            sha256: Sha256::digest(payload)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        }
    }

    #[test]
    fn stages_exact_payload_without_activating_it() {
        let _serial = serialize_uploads();
        let fixture = Fixture::new();
        let payload = b"runtime artifact";
        let result = receive(&mut Cursor::new(payload), &spec(payload), &fixture.paths).unwrap();

        assert_eq!(result.payload_bytes, payload.len() as u64);
        assert_eq!(fs::read(&fixture.paths.upload).unwrap(), payload);
        assert!(!fixture.paths.part.exists());
        assert!(!fixture.root.join("mister-magik-fb").exists());
    }

    #[test]
    fn rejects_truncation_surplus_and_hash_mismatch_without_residue() {
        let _serial = serialize_uploads();
        for (payload, declared) in [
            (b"short".as_slice(), b"shorter".as_slice()),
            (b"surplus".as_slice(), b"surplu".as_slice()),
            (b"wrong".as_slice(), b"other".as_slice()),
        ] {
            let fixture = Fixture::new();
            assert!(receive(&mut Cursor::new(payload), &spec(declared), &fixture.paths).is_err());
            assert!(!fixture.paths.upload.exists());
            assert!(!fixture.paths.part.exists());
        }
    }

    #[test]
    fn requires_lock_and_exclusive_empty_staging_paths() {
        let _serial = serialize_uploads();
        let fixture = Fixture::new();
        fs::remove_file(&fixture.paths.lock).unwrap();
        let payload = b"runtime";
        assert_eq!(
            receive(&mut Cursor::new(payload), &spec(payload), &fixture.paths)
                .unwrap_err()
                .kind,
            UploadFailureKind::Busy
        );

        File::create(&fixture.paths.lock).unwrap();
        File::create(&fixture.paths.upload).unwrap();
        assert_eq!(
            receive(&mut Cursor::new(payload), &spec(payload), &fixture.paths)
                .unwrap_err()
                .kind,
            UploadFailureKind::Busy
        );
    }
}
