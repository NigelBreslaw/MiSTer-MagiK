// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! The game-database delivery lane is deliberately independent from platform
//! delivery.  It accepts an already verified local release and exposes only
//! the database transaction to the device adapter.  In particular, this
//! module must not resolve a platform candidate, build an artifact, or read a
//! platform manifest.

use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use std::fs;
use std::path::{Path, PathBuf};

const STAGE_FILES: [&str; 5] = [
    "mame.sqlite3",
    "hbmame.sqlite3",
    "arcade-updater-index-v1.lz4b",
    "game-databases-SHA256SUMS",
    "game-databases-manifest.json",
];

/// The only device operations available to this delivery lane.
///
/// Keeping the trait this small is intentional: runtime, platform, Main,
/// kernel, and FPGA delivery methods are not reachable from database-only
/// execution, including from its mocked tests.
trait DatabaseDeliveryDevice {
    fn connect(&mut self) -> AgentResult<()>;
    fn deliver_databases(&mut self, stage: &Path) -> AgentResult<()>;
}

impl DatabaseDeliveryDevice for DeviceClient {
    fn connect(&mut self) -> AgentResult<()> {
        self.read(crate::NativeDevice::discover)
    }

    fn deliver_databases(&mut self, stage: &Path) -> AgentResult<()> {
        let stage = stage.to_path_buf();
        self.mutate(|device| device.deliver_databases(&stage))
    }
}

struct PreparedStage {
    root: PathBuf,
    stage: PathBuf,
    release_version: u64,
}

/// Validate and deploy a local game-database release using only the database
/// transaction.  The caller is responsible for choosing the release; this
/// function never performs GitHub or platform resolution.
pub fn execute(
    repository: &Path,
    expected_commit: &str,
    release_dir: &Path,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    validate_clean_exact_commit(repository, expected_commit)?;
    let release = resolve_release_dir(repository, release_dir)?;

    reporter.emit(
        EventKind::Progress,
        "database-validation",
        &format!(
            "validating local game-database release {}",
            release.display()
        ),
        Some(15),
    )?;
    let prepared = prepare_stage(repository, &release, expected_commit)?;
    reporter.emit(
        EventKind::Completed,
        "database-validation",
        &format!(
            "validated game-databases-v{}; staged exactly five database files",
            prepared.release_version
        ),
        Some(45),
    )?;

    let result = execute_stage_with_device(&prepared.stage, reporter, DeviceClient::default());
    let cleanup = fs::remove_dir_all(&prepared.root).map_err(|error| {
        format!(
            "cannot clear database delivery staging {}: {error}",
            prepared.root.display()
        )
    });
    match (result, cleanup) {
        (Ok(()), Ok(())) => {
            reporter.emit(
                EventKind::Completed,
                "database-delivery",
                "database transaction complete",
                Some(100),
            )?;
            Ok(Outcome::Passed)
        }
        (Ok(()), Err(error)) => Err(error.into()),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!(
            "database delivery failed ({error}); staging cleanup failed ({cleanup})"
        )
        .into()),
    }
}

fn execute_stage_with_device<D: DatabaseDeliveryDevice>(
    stage: &Path,
    reporter: &mut Reporter<'_>,
    mut device: D,
) -> AgentResult<()> {
    reporter.emit(
        EventKind::Progress,
        "database-connect",
        "connecting to MiSTer for database delivery",
        Some(60),
    )?;
    device.connect()?;
    reporter.emit(
        EventKind::Progress,
        "database-deploy",
        "invoking the rollback-capable database transaction",
        Some(75),
    )?;
    device.deliver_databases(stage)?;
    Ok(())
}

fn validate_clean_exact_commit(repository: &Path, expected_commit: &str) -> AgentResult<()> {
    let head = crate::git::value(repository, &["rev-parse", "HEAD"])?;
    let dirty = crate::git::value(repository, &["status", "--porcelain"])?;
    validate_commit_identity(&head, expected_commit, !dirty.is_empty())
}

fn validate_commit_identity(head: &str, expected_commit: &str, dirty: bool) -> AgentResult<()> {
    if head != expected_commit {
        return Err("database delivery HEAD does not match the recorded exact commit".into());
    }
    if dirty {
        return Err("dirty_worktree: commit or discard changes before database delivery".into());
    }
    Ok(())
}

fn resolve_release_dir(repository: &Path, release_dir: &Path) -> AgentResult<PathBuf> {
    let path = if release_dir.is_absolute() {
        release_dir.to_path_buf()
    } else {
        repository.join(release_dir)
    };
    if !path.is_dir() {
        return Err(format!(
            "game-database release directory is missing: {}",
            path.display()
        )
        .into());
    }
    fs::canonicalize(&path).map_err(|error| {
        format!(
            "cannot resolve game-database release directory {}: {error}",
            path.display()
        )
        .into()
    })
}

fn prepare_stage(
    repository: &Path,
    release: &Path,
    expected_commit: &str,
) -> AgentResult<PreparedStage> {
    let root = repository
        .join("build/agent-deploy/game-databases")
        .join(format!("{expected_commit}-{}", std::process::id()));
    if root.exists() {
        return Err(format!(
            "database delivery staging already exists: {}",
            root.display()
        )
        .into());
    }
    let extracted = root.join("extracted");
    let stage = root.join("stage");
    fs::create_dir_all(&extracted).map_err(|error| error.to_string())?;
    let manifest = match crate::game_databases::extract_release(release, &extracted) {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = fs::remove_dir_all(&root);
            return Err(error);
        }
    };
    fs::create_dir(&stage).map_err(|error| {
        let _ = fs::remove_dir_all(&root);
        error.to_string()
    })?;

    for name in STAGE_FILES {
        let source_name = if name == "game-databases-SHA256SUMS" {
            crate::game_databases::CHECKSUMS
        } else {
            name
        };
        let source = extracted.join(source_name);
        let destination = stage.join(name);
        if let Err(error) = fs::copy(&source, &destination) {
            let _ = fs::remove_dir_all(&root);
            return Err(format!("cannot stage {name}: {error}").into());
        }
    }
    validate_stage_shape(&stage).inspect_err(|_| {
        let _ = fs::remove_dir_all(&root);
    })?;
    Ok(PreparedStage {
        root,
        stage,
        release_version: manifest["release_version"].as_u64().unwrap_or_default(),
    })
}

fn validate_stage_shape(stage: &Path) -> AgentResult<()> {
    let mut entries = fs::read_dir(stage)
        .map_err(|error| format!("cannot inspect database stage {}: {error}", stage.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable();
    let mut expected = STAGE_FILES.map(str::to_owned).to_vec();
    expected.sort_unstable();
    if entries != expected {
        return Err(format!(
            "database stage must contain exactly five files; found {}",
            entries.join(", ")
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Call {
        Connect,
        Databases,
    }

    struct RecordingDevice(Rc<RefCell<Vec<Call>>>);

    impl DatabaseDeliveryDevice for RecordingDevice {
        fn connect(&mut self) -> AgentResult<()> {
            self.0.borrow_mut().push(Call::Connect);
            Ok(())
        }

        fn deliver_databases(&mut self, stage: &Path) -> AgentResult<()> {
            assert!(stage.is_dir());
            self.0.borrow_mut().push(Call::Databases);
            Ok(())
        }
    }

    fn reporter() -> (Reporter<'static>, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-database-delivery-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let evidence = Box::leak(Box::new(crate::evidence::Evidence::open_at(&root).unwrap()));
        let request = crate::request::RawRequest::capture([std::ffi::OsString::from("agent-cli")]);
        evidence.begin_request(&request).unwrap();
        (
            Reporter::new(
                evidence,
                crate::cli::OutputFormat::Human,
                "database-delivery-test",
            ),
            root,
        )
    }

    #[test]
    fn database_only_execution_calls_connect_then_database_transaction_only() {
        let stage = std::env::temp_dir().join(format!(
            "mister-magik-database-stage-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&stage);
        fs::create_dir_all(&stage).unwrap();
        for name in STAGE_FILES {
            fs::write(stage.join(name), name).unwrap();
        }
        let calls = Rc::new(RefCell::new(Vec::new()));
        let (mut reporter, evidence_root) = reporter();
        execute_stage_with_device(&stage, &mut reporter, RecordingDevice(Rc::clone(&calls)))
            .unwrap();
        assert_eq!(calls.borrow().as_slice(), &[Call::Connect, Call::Databases]);
        let _ = fs::remove_dir_all(stage);
        let _ = fs::remove_dir_all(evidence_root);
    }

    #[test]
    fn stage_shape_accepts_exactly_the_five_deploy_files() {
        let stage = std::env::temp_dir().join(format!(
            "mister-magik-database-stage-shape-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&stage);
        fs::create_dir_all(&stage).unwrap();
        for name in STAGE_FILES {
            fs::write(stage.join(name), name).unwrap();
        }
        validate_stage_shape(&stage).unwrap();
        fs::write(stage.join("platform-v3.manifest"), b"forbidden").unwrap();
        assert!(validate_stage_shape(&stage).is_err());
        let _ = fs::remove_dir_all(stage);
    }

    #[test]
    fn missing_or_invalid_release_fails_before_a_device_can_be_called() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-database-release-invalid-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert!(resolve_release_dir(&root, Path::new("missing")).is_err());
        fs::write(root.join(crate::game_databases::MANIFEST), b"{}").unwrap();
        assert!(crate::game_databases::extract_release(&root, &root.join("extracted")).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exact_commit_validation_rejects_mismatch_and_dirty_worktree() {
        assert!(validate_commit_identity("head", "head", false).is_ok());
        assert!(validate_commit_identity("other", "head", false).is_err());
        assert!(validate_commit_identity("head", "head", true).is_err());
    }
}
