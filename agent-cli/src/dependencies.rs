// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Standard Cargo dependency and lockfile maintenance.

use crate::error::AgentResult;
use crate::process;
use crate::progress::{EventKind, Reporter};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const CARGO_DEADLINE: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum DependenciesCommand {
    /// Update Cargo.lock from Cargo.toml, then verify it with --locked.
    Sync {
        manifest: PathBuf,
        #[arg(long)]
        package: Option<String>,
    },
}

pub fn execute(
    repository: &Path,
    command: &DependenciesCommand,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    match command {
        DependenciesCommand::Sync { manifest, package } => {
            sync(repository, manifest, package.as_deref(), reporter)
        }
    }
}

fn sync(
    repository: &Path,
    manifest: &Path,
    package: Option<&str>,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    let (manifest, lock) = tracked_manifest_and_lock(repository, manifest)?;
    reporter.emit(
        EventKind::Progress,
        "cargo-update",
        "updating Cargo.lock",
        Some(20),
    )?;
    let mut update = Command::new("cargo");
    update
        .args(["update", "--manifest-path"])
        .arg(&manifest)
        .stdin(Stdio::null());
    if let Some(package) = package {
        update.args(["--package", package]);
    }
    run(&mut update, "cargo update")?;

    reporter.emit(
        EventKind::Progress,
        "cargo-locked",
        "checking the updated lockfile",
        Some(70),
    )?;
    let mut check = Command::new("cargo");
    check
        .args([
            "metadata",
            "--locked",
            "--all-features",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    run(&mut check, "cargo metadata --locked")?;

    if !lock.is_file() {
        return Err(format!("cargo did not generate {}", lock.display()).into());
    }
    Ok(())
}

fn tracked_manifest_and_lock(
    repository: &Path,
    requested: &Path,
) -> AgentResult<(PathBuf, PathBuf)> {
    let manifest = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        repository.join(requested)
    };
    let repository = repository
        .canonicalize()
        .map_err(|error| format!("resolve repository path: {error}"))?;
    let manifest = manifest
        .canonicalize()
        .map_err(|error| format!("resolve Cargo manifest {}: {error}", manifest.display()))?;
    let relative = manifest.strip_prefix(&repository).map_err(|_| {
        format!(
            "Cargo manifest must be inside the repository: {}",
            manifest.display()
        )
    })?;
    if relative.file_name().and_then(|name| name.to_str()) != Some("Cargo.toml") {
        return Err(format!(
            "dependency sync requires a Cargo.toml: {}",
            relative.display()
        )
        .into());
    }
    let lock = manifest.with_file_name("Cargo.lock");
    let relative_lock = relative.with_file_name("Cargo.lock");
    let tracked = Command::new("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(relative)
        .arg(&relative_lock)
        .current_dir(&repository)
        .output()
        .map_err(|error| format!("check dependency files in Git: {error}"))?;
    if !tracked.status.success() {
        return Err(format!(
            "dependency sync requires tracked Cargo.toml and Cargo.lock: {}",
            String::from_utf8_lossy(&tracked.stderr).trim()
        )
        .into());
    }
    Ok((manifest, lock))
}

fn run(command: &mut Command, label: &str) -> AgentResult<()> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start {label}: {error}"))?;
    let status = process::wait(&mut child, Some(CARGO_DEADLINE), label, None, || Ok(()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} exited with {status}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_manifest_paths_before_running_cargo() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let error = tracked_manifest_and_lock(repository, Path::new("README.md")).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("dependency sync requires a Cargo.toml")
        );
    }
}
