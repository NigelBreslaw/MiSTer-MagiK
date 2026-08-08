// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Repository-wide Cargo artifact cleanup.

use crate::error::AgentResult;
use crate::process;
use crate::progress::{EventKind, Reporter};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const CARGO_DEADLINE: Duration = Duration::from_secs(10 * 60);

pub fn execute(repository: &Path, reporter: &mut Reporter<'_>) -> AgentResult<()> {
    let manifests = cargo_manifests(repository)?;
    if manifests.is_empty() {
        return Err("no Cargo.toml files found in the repository".into());
    }

    for (index, manifest) in manifests.iter().enumerate() {
        let relative = manifest.to_string_lossy();
        let progress = 5 + ((index * 90) / manifests.len()) as u8;
        reporter.emit(
            EventKind::Progress,
            "cargo-clean",
            &format!("cleaning {relative}"),
            Some(progress),
        )?;

        let mut command = Command::new("cargo");
        command
            .args(["clean", "--manifest-path"])
            .arg(manifest)
            .current_dir(repository)
            .stdin(Stdio::null());
        let label = format!("cargo clean --manifest-path {relative}");
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start {label}: {error}"))?;
        let status = process::wait(&mut child, Some(CARGO_DEADLINE), &label, None, || Ok(()))?;
        if !status.success() {
            return Err(format!("{label} exited with {status}").into());
        }
    }
    Ok(())
}

fn cargo_manifests(repository: &Path) -> AgentResult<Vec<PathBuf>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            "*Cargo.toml",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot enumerate Cargo manifests: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot enumerate Cargo manifests: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }

    let mut manifests: Vec<_> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .collect();
    manifests.sort();
    manifests.dedup();
    Ok(manifests)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_discovery_finds_agent_cli_manifest() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let manifests = cargo_manifests(repository).unwrap();
        assert!(manifests.contains(&PathBuf::from("agent-cli/Cargo.toml")));
        assert!(manifests.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
