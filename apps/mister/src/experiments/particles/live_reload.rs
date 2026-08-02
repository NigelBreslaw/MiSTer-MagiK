// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded, last-good file reloading for attended particle experiments.

use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const LIVE_PARTICLE_STATUS_SCHEMA: &str = "mister-magik-live-particles-status-v1";
pub const LIVE_PARTICLE_MAX_FILE_BYTES: usize = 1024 * 1024;
pub const LIVE_PARTICLE_MAX_ERROR_BYTES: usize = 512;
pub const LIVE_PARTICLE_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub struct ReloadAttempt<T> {
    pub generation: u64,
    pub digest: u64,
    pub result: Result<T, String>,
}

/// Watches one fixed file and retains only its newest distinct content attempt.
///
/// Parsing happens off the render thread. The consumer atomically takes the
/// latest attempt at the start of a frame; intermediate saves may be replaced.
pub struct LastGoodFile<T> {
    latest: Arc<Mutex<Option<ReloadAttempt<T>>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> LastGoodFile<T> {
    pub fn spawn(
        path: PathBuf,
        parser: impl Fn(&[u8]) -> Result<T, String> + Send + 'static,
    ) -> Result<Self, String> {
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_latest = Arc::clone(&latest);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("particle-family-reload".into())
            .spawn(move || {
                let mut generation = 0_u64;
                let mut last_digest = None;
                while !worker_stop.load(Ordering::Acquire) {
                    if let Some((digest, bytes)) = read_distinct_content(&path, last_digest) {
                        last_digest = Some(digest);
                        generation = generation.saturating_add(1);
                        let result = bytes.and_then(|bytes| parser(&bytes));
                        let attempt = ReloadAttempt {
                            generation,
                            digest,
                            result: result.map_err(|error| bounded_error(&error)),
                        };
                        *worker_latest
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(attempt);
                    }
                    thread::sleep(LIVE_PARTICLE_POLL_INTERVAL);
                }
            })
            .map_err(|error| format!("start particle family reload worker: {error}"))?;
        Ok(Self {
            latest,
            stop,
            worker: Some(worker),
        })
    }

    pub fn take_latest(&self) -> Option<ReloadAttempt<T>> {
        self.latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl<T> Drop for LastGoodFile<T> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_distinct_content(
    path: &Path,
    last_digest: Option<u64>,
) -> Option<(u64, Result<Vec<u8>, String>)> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            let message = format!("read {}: {error}", path.display());
            let digest = content_digest(message.as_bytes());
            return (last_digest != Some(digest)).then_some((digest, Err(message)));
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file
        .by_ref()
        .take((LIVE_PARTICLE_MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
    {
        let message = format!("read {}: {error}", path.display());
        let digest = content_digest(message.as_bytes());
        return (last_digest != Some(digest)).then_some((digest, Err(message)));
    }
    let digest = content_digest(&bytes);
    if last_digest == Some(digest) {
        return None;
    }
    if bytes.len() > LIVE_PARTICLE_MAX_FILE_BYTES {
        return Some((
            digest,
            Err(format!(
                "{} exceeds the {} byte live-particle limit",
                path.display(),
                LIVE_PARTICLE_MAX_FILE_BYTES
            )),
        ));
    }
    Some((digest, Ok(bytes)))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LiveParticleStatus {
    pub schema: &'static str,
    pub generation: u64,
    pub state: LiveParticleStatusState,
    pub demo: u8,
    pub error: Option<String>,
}

impl LiveParticleStatus {
    #[must_use]
    pub const fn embedded(demo: u8) -> Self {
        Self {
            schema: LIVE_PARTICLE_STATUS_SCHEMA,
            generation: 0,
            state: LiveParticleStatusState::Embedded,
            demo,
            error: None,
        }
    }

    #[must_use]
    pub const fn applied(generation: u64, demo: u8) -> Self {
        Self {
            schema: LIVE_PARTICLE_STATUS_SCHEMA,
            generation,
            state: LiveParticleStatusState::Applied,
            demo,
            error: None,
        }
    }

    #[must_use]
    pub fn rejected(generation: u64, demo: u8, error: &str) -> Self {
        Self {
            schema: LIVE_PARTICLE_STATUS_SCHEMA,
            generation,
            state: LiveParticleStatusState::Rejected,
            demo,
            error: Some(bounded_error(error)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveParticleStatusState {
    Embedded,
    Applied,
    Rejected,
}

pub fn publish_live_particle_status(
    path: &Path,
    status: &LiveParticleStatus,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("live-particle status path {} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let upload = path.with_extension("json.upload");
    let bytes = serde_json::to_vec(status)
        .map_err(|error| format!("serialize live-particle status: {error}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&upload)
        .map_err(|error| format!("create {}: {error}", upload.display()))?;
    std::io::Write::write_all(&mut file, &bytes)
        .map_err(|error| format!("write {}: {error}", upload.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", upload.display()))?;
    fs::rename(&upload, path).map_err(|error| {
        format!(
            "publish live-particle status {} -> {}: {error}",
            upload.display(),
            path.display()
        )
    })
}

fn content_digest(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn bounded_error(error: &str) -> String {
    if error.len() <= LIVE_PARTICLE_MAX_ERROR_BYTES {
        return error.to_owned();
    }
    let mut end = LIVE_PARTICLE_MAX_ERROR_BYTES;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    error[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::time::Instant;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mister-magik-live-particle-{}-{name}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn await_attempt<T: Send + 'static>(watcher: &LastGoodFile<T>) -> ReloadAttempt<T> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(attempt) = watcher.take_latest() {
                return attempt;
            }
            assert!(Instant::now() < deadline, "reload attempt timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn publishes_only_distinct_content_and_recovers_after_rejection() {
        let root = temp_path("watch");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("family.json");
        fs::write(&path, br#"{"schema":"test"}"#).unwrap();
        let watcher = LastGoodFile::spawn(path.clone(), |bytes| {
            serde_json::from_slice::<serde_json::Value>(bytes).map_err(|error| error.to_string())
        })
        .unwrap();

        let first = await_attempt(&watcher);
        assert_eq!(first.generation, 1);
        assert!(first.result.is_ok());

        fs::write(&path, br#"{"schema":"test"}"#).unwrap();
        thread::sleep(LIVE_PARTICLE_POLL_INTERVAL * 2);
        assert!(watcher.take_latest().is_none());

        fs::write(&path, b"{").unwrap();
        let rejected = await_attempt(&watcher);
        assert_eq!(rejected.generation, 2);
        assert!(rejected.result.is_err());

        fs::write(&path, br#"{"schema":"recovered"}"#).unwrap();
        let recovered = await_attempt(&watcher);
        assert_eq!(recovered.generation, 3);
        assert!(recovered.result.is_ok());
        drop(watcher);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn status_is_atomic_and_errors_are_bounded() {
        let root = temp_path("status");
        let path = root.join("status.json");
        let status = LiveParticleStatus::rejected(7, 32, &"é".repeat(600));
        assert!(status.error.as_ref().unwrap().len() <= LIVE_PARTICLE_MAX_ERROR_BYTES);
        publish_live_particle_status(&path, &status).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["generation"], 7);
        assert_eq!(json["state"], "rejected");
        assert!(!path.with_extension("json.upload").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
