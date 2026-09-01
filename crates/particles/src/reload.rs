// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Bounded, last-good recipe reloading for attended particle development.

use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const STARTUP_PARTICLE_STATUS_SCHEMA: &str = "mister-magik-startup-particle-status-v1";
pub const MAX_RECIPE_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_RELOAD_ERROR_BYTES: usize = 512;
pub const RECIPE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One parsed file generation waiting for host acknowledgement.
#[derive(Debug)]
pub struct ReloadAttempt<T> {
    pub generation: u64,
    pub action: ReloadAction<T>,
}

/// The action represented by a distinct observed file state.
#[derive(Debug)]
pub enum ReloadAction<T> {
    /// A parsed candidate that the host may prepare and apply.
    Apply(T),
    /// Restore the checked-in recipe after two consecutive missing polls.
    ResetToEmbedded,
    /// Keep the last-good renderer and publish the bounded rejection.
    Reject(String),
}

/// Watches one fixed recipe file and retains only the newest attempt.
///
/// File reads and parsing happen on the worker. The render host takes at most
/// one pending attempt at a frame boundary, then explicitly publishes whether
/// it applied or rejected that generation.
pub struct LastGoodRecipeFile<T> {
    latest: Arc<Mutex<Option<ReloadAttempt<T>>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> LastGoodRecipeFile<T> {
    pub fn spawn(
        path: PathBuf,
        parser: impl Fn(&[u8]) -> Result<T, String> + Send + 'static,
    ) -> Result<Self, String> {
        Self::spawn_with_state(path, parser, PollState::default())
    }

    /// Starts after the host has already applied `initial_content` as
    /// generation zero, so the first poll cannot re-emit the same bytes.
    pub fn spawn_after_initial_content(
        path: PathBuf,
        initial_content: &[u8],
        parser: impl Fn(&[u8]) -> Result<T, String> + Send + 'static,
    ) -> Result<Self, String> {
        Self::spawn_with_state(
            path,
            parser,
            PollState::after_initial_content(initial_content),
        )
    }

    fn spawn_with_state(
        path: PathBuf,
        parser: impl Fn(&[u8]) -> Result<T, String> + Send + 'static,
        initial_state: PollState,
    ) -> Result<Self, String> {
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_latest = Arc::clone(&latest);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("startup-particle-reload".into())
            .spawn(move || {
                let mut state = initial_state;
                while !worker_stop.load(Ordering::Acquire) {
                    if let Some(attempt) = state.poll(&path, &parser) {
                        replace_latest(&worker_latest, attempt);
                    }
                    thread::park_timeout(RECIPE_POLL_INTERVAL);
                }
            })
            .map_err(|error| bounded_error(&format!("start recipe reload worker: {error}")))?;
        Ok(Self {
            latest,
            stop,
            worker: Some(worker),
        })
    }

    /// Takes the newest pending generation, discarding any it superseded.
    pub fn take_latest(&self) -> Option<ReloadAttempt<T>> {
        self.latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

impl<T> Drop for LastGoodRecipeFile<T> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn replace_latest<T>(latest: &Mutex<Option<ReloadAttempt<T>>>, attempt: ReloadAttempt<T>) {
    *latest
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(attempt);
}

#[derive(Default)]
struct PollState {
    generation: u64,
    last_content_digest: Option<u64>,
    last_error_digest: Option<u64>,
    observed_content: bool,
    consecutive_missing: u8,
}

impl PollState {
    fn after_initial_content(content: &[u8]) -> Self {
        Self {
            last_content_digest: Some(content_digest(content)),
            observed_content: true,
            ..Self::default()
        }
    }

    fn poll<T>(
        &mut self,
        path: &Path,
        parser: &impl Fn(&[u8]) -> Result<T, String>,
    ) -> Option<ReloadAttempt<T>> {
        self.observe(read_file(path), parser)
    }

    fn observe<T>(
        &mut self,
        observation: FileObservation,
        parser: &impl Fn(&[u8]) -> Result<T, String>,
    ) -> Option<ReloadAttempt<T>> {
        match observation {
            FileObservation::Missing => {
                if !self.observed_content {
                    return None;
                }
                self.consecutive_missing = self.consecutive_missing.saturating_add(1);
                if self.consecutive_missing < 2 {
                    return None;
                }
                self.observed_content = false;
                self.consecutive_missing = 0;
                self.last_content_digest = None;
                self.last_error_digest = None;
                self.attempt(ReloadAction::ResetToEmbedded)
            }
            FileObservation::ReadError { digest, error } => {
                self.consecutive_missing = 0;
                if self.last_error_digest == Some(digest) {
                    return None;
                }
                self.last_error_digest = Some(digest);
                self.attempt(ReloadAction::Reject(bounded_error(&error)))
            }
            FileObservation::Content { digest, bytes } => {
                self.observed_content = true;
                self.consecutive_missing = 0;
                self.last_error_digest = None;
                if self.last_content_digest == Some(digest) {
                    return None;
                }
                self.last_content_digest = Some(digest);
                let action = match bytes.and_then(|bytes| parser(&bytes)) {
                    Ok(candidate) => ReloadAction::Apply(candidate),
                    Err(error) => ReloadAction::Reject(bounded_error(&error)),
                };
                self.attempt(action)
            }
        }
    }

    fn attempt<T>(&mut self, action: ReloadAction<T>) -> Option<ReloadAttempt<T>> {
        let generation = self.generation.checked_add(1)?;
        self.generation = generation;
        Some(ReloadAttempt { generation, action })
    }
}

enum FileObservation {
    Missing,
    ReadError {
        digest: u64,
        error: String,
    },
    Content {
        digest: u64,
        bytes: Result<Vec<u8>, String>,
    },
}

fn read_file(path: &Path) -> FileObservation {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return FileObservation::Missing;
        }
        Err(error) => return read_error(path, error),
    };
    let mut bytes = Vec::new();
    if let Err(error) = Read::by_ref(&mut file)
        .take((MAX_RECIPE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
    {
        return read_error(path, error);
    }
    let digest = content_digest(&bytes);
    let bytes = if bytes.len() > MAX_RECIPE_FILE_BYTES {
        Err(format!(
            "{} exceeds the {MAX_RECIPE_FILE_BYTES} byte recipe limit",
            path.display()
        ))
    } else {
        Ok(bytes)
    };
    FileObservation::Content { digest, bytes }
}

fn read_error(path: &Path, error: std::io::Error) -> FileObservation {
    let error = format!("read {}: {error}", path.display());
    FileObservation::ReadError {
        digest: content_digest(error.as_bytes()),
        error,
    }
}

/// The only recipe kinds accepted by the focused startup-particle workflow.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StartupParticleRecipe {
    Magik,
    Cabinet,
    Intro,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StartupParticleStatusState {
    Embedded,
    Applied,
    Rejected,
}

/// A host acknowledgement for one reload generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StartupParticleStatus {
    pub schema: &'static str,
    pub generation: u64,
    pub recipe: StartupParticleRecipe,
    pub state: StartupParticleStatusState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl StartupParticleStatus {
    #[must_use]
    pub const fn embedded(generation: u64, recipe: StartupParticleRecipe) -> Self {
        Self {
            schema: STARTUP_PARTICLE_STATUS_SCHEMA,
            generation,
            recipe,
            state: StartupParticleStatusState::Embedded,
            error: None,
        }
    }

    #[must_use]
    pub const fn applied(generation: u64, recipe: StartupParticleRecipe) -> Self {
        Self {
            schema: STARTUP_PARTICLE_STATUS_SCHEMA,
            generation,
            recipe,
            state: StartupParticleStatusState::Applied,
            error: None,
        }
    }

    #[must_use]
    pub fn rejected(generation: u64, recipe: StartupParticleRecipe, error: &str) -> Self {
        Self {
            schema: STARTUP_PARTICLE_STATUS_SCHEMA,
            generation,
            recipe,
            state: StartupParticleStatusState::Rejected,
            error: Some(bounded_error(error)),
        }
    }
}

/// Atomically publishes an acknowledgement through a temporary sibling file.
pub fn publish_startup_particle_status(
    path: &Path,
    status: &StartupParticleStatus,
) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| bounded_error(&format!("status path {} has no parent", path.display())))?;
    fs::create_dir_all(parent)
        .map_err(|error| bounded_error(&format!("create {}: {error}", parent.display())))?;
    let upload = path.with_extension("json.upload");
    let bytes = serde_json::to_vec(status)
        .map_err(|error| bounded_error(&format!("serialize startup-particle status: {error}")))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&upload)
        .map_err(|error| bounded_error(&format!("create {}: {error}", upload.display())))?;
    file.write_all(&bytes)
        .map_err(|error| bounded_error(&format!("write {}: {error}", upload.display())))?;
    file.sync_all()
        .map_err(|error| bounded_error(&format!("sync {}: {error}", upload.display())))?;
    fs::rename(&upload, path).map_err(|error| {
        bounded_error(&format!(
            "publish startup-particle status {} -> {}: {error}",
            upload.display(),
            path.display()
        ))
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
    if error.len() <= MAX_RELOAD_ERROR_BYTES {
        return error.to_owned();
    }
    let mut end = MAX_RELOAD_ERROR_BYTES;
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
            "mister-magik-recipe-reload-{}-{name}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn content(bytes: &[u8]) -> FileObservation {
        FileObservation::Content {
            digest: content_digest(bytes),
            bytes: Ok(bytes.to_vec()),
        }
    }

    fn parse_number(bytes: &[u8]) -> Result<u8, String> {
        std::str::from_utf8(bytes)
            .map_err(|error| error.to_string())?
            .parse()
            .map_err(|error: std::num::ParseIntError| error.to_string())
    }

    #[test]
    fn newest_generation_replaces_pending_attempts() {
        let mut state = PollState::default();
        let mailbox = Mutex::new(None);
        for value in [b"1".as_slice(), b"2", b"3"] {
            let attempt = state.observe(content(value), &parse_number).unwrap();
            replace_latest(&mailbox, attempt);
        }
        let attempt = mailbox.lock().unwrap().take().unwrap();
        assert_eq!(attempt.generation, 3);
        assert!(matches!(attempt.action, ReloadAction::Apply(3)));
    }

    #[test]
    fn invalid_content_is_bounded_deduplicated_and_recoverable() {
        let mut state = PollState::default();
        assert!(
            state
                .observe(FileObservation::Missing, &parse_number)
                .is_none()
        );

        let invalid = "é".repeat(600);
        let rejected = state
            .observe(content(invalid.as_bytes()), &parse_number)
            .unwrap();
        assert_eq!(rejected.generation, 1);
        let ReloadAction::Reject(error) = rejected.action else {
            panic!("invalid content was not rejected");
        };
        assert!(error.len() <= MAX_RELOAD_ERROR_BYTES);
        assert!(std::str::from_utf8(error.as_bytes()).is_ok());
        assert!(
            state
                .observe(content(invalid.as_bytes()), &parse_number)
                .is_none()
        );

        let recovered = state.observe(content(b"7"), &parse_number).unwrap();
        assert_eq!(recovered.generation, 2);
        assert!(matches!(recovered.action, ReloadAction::Apply(7)));
    }

    #[test]
    fn two_missing_polls_reset_once_but_one_missing_poll_does_not() {
        let mut state = PollState::default();
        assert_eq!(
            state
                .observe(content(b"4"), &parse_number)
                .unwrap()
                .generation,
            1
        );
        assert!(
            state
                .observe(FileObservation::Missing, &parse_number)
                .is_none()
        );
        assert!(state.observe(content(b"4"), &parse_number).is_none());

        assert!(
            state
                .observe(FileObservation::Missing, &parse_number)
                .is_none()
        );
        let reset = state
            .observe(FileObservation::Missing, &parse_number)
            .unwrap();
        assert_eq!(reset.generation, 2);
        assert!(matches!(reset.action, ReloadAction::ResetToEmbedded));
        assert!(
            state
                .observe(FileObservation::Missing, &parse_number)
                .is_none()
        );

        let restored = state.observe(content(b"4"), &parse_number).unwrap();
        assert_eq!(restored.generation, 3);
        assert!(matches!(restored.action, ReloadAction::Apply(4)));
    }

    #[test]
    fn read_errors_are_rejections_and_break_missing_streaks() {
        let mut state = PollState::default();
        let read_error = || FileObservation::ReadError {
            digest: 9,
            error: "read failed".into(),
        };
        let first = state.observe(read_error(), &parse_number).unwrap();
        assert_eq!(first.generation, 1);
        assert!(matches!(first.action, ReloadAction::Reject(_)));
        assert!(
            state
                .observe(FileObservation::Missing, &parse_number)
                .is_none()
        );
        assert!(
            state
                .observe(FileObservation::Missing, &parse_number)
                .is_none()
        );

        assert_eq!(
            state
                .observe(content(b"8"), &parse_number)
                .unwrap()
                .generation,
            2
        );
        assert!(
            state
                .observe(FileObservation::Missing, &parse_number)
                .is_none()
        );
        let second_error = state.observe(read_error(), &parse_number).unwrap();
        assert_eq!(second_error.generation, 3);
        assert!(
            state
                .observe(FileObservation::Missing, &parse_number)
                .is_none()
        );
        let reset = state
            .observe(FileObservation::Missing, &parse_number)
            .unwrap();
        assert_eq!(reset.generation, 4);
        assert!(matches!(reset.action, ReloadAction::ResetToEmbedded));
    }

    #[test]
    fn oversized_file_is_one_deduplicated_rejection() {
        let root = temp_path("oversized");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("recipe.json");
        fs::write(&path, vec![b'x'; MAX_RECIPE_FILE_BYTES + 1]).unwrap();
        let mut state = PollState::default();
        let rejected = state.poll(&path, &parse_number).unwrap();
        assert!(matches!(rejected.action, ReloadAction::Reject(_)));
        assert!(state.poll(&path, &parse_number).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn watcher_parses_on_worker_and_exposes_one_attempt() {
        let root = temp_path("worker");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("recipe.json");
        fs::write(&path, b"6").unwrap();
        let watcher = LastGoodRecipeFile::spawn(path, parse_number).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let attempt = loop {
            if let Some(attempt) = watcher.take_latest() {
                break attempt;
            }
            assert!(Instant::now() < deadline, "reload attempt timed out");
            thread::yield_now();
        };
        assert_eq!(attempt.generation, 1);
        assert!(matches!(attempt.action, ReloadAction::Apply(6)));
        drop(watcher);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn watcher_does_not_reemit_content_already_applied_as_generation_zero() {
        let root = temp_path("initial-content");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("recipe.json");
        fs::write(&path, b"6").unwrap();
        let watcher =
            LastGoodRecipeFile::spawn_after_initial_content(path.clone(), b"6", parse_number)
                .unwrap();

        thread::sleep(RECIPE_POLL_INTERVAL.saturating_mul(2));
        assert!(watcher.take_latest().is_none());

        let upload = path.with_extension("json.upload");
        fs::write(&upload, b"7").unwrap();
        fs::rename(&upload, &path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let attempt = loop {
            if let Some(attempt) = watcher.take_latest() {
                break attempt;
            }
            assert!(Instant::now() < deadline, "reload attempt timed out");
            thread::yield_now();
        };
        assert_eq!(attempt.generation, 1);
        assert!(matches!(attempt.action, ReloadAction::Apply(7)));
        drop(watcher);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn status_is_atomic_uses_schema_and_omits_empty_error() {
        let root = temp_path("status");
        let path = root.join("status.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, b"old").unwrap();

        let status = StartupParticleStatus::applied(7, StartupParticleRecipe::Cabinet);
        publish_startup_particle_status(&path, &status).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["schema"], STARTUP_PARTICLE_STATUS_SCHEMA);
        assert_eq!(json["generation"], 7);
        assert_eq!(json["recipe"], "cabinet");
        assert_eq!(json["state"], "applied");
        assert!(json.get("error").is_none());
        assert!(!path.with_extension("json.upload").exists());

        let rejected =
            StartupParticleStatus::rejected(8, StartupParticleRecipe::Magik, &"é".repeat(600));
        publish_startup_particle_status(&path, &rejected).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["state"], "rejected");
        assert!(json["error"].as_str().unwrap().len() <= MAX_RELOAD_ERROR_BYTES);
        fs::remove_dir_all(root).unwrap();
    }
}
