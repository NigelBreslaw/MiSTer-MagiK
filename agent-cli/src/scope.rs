// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::{now_ms, Evidence};
use crate::model::Scope;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn collect(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    scope: &Scope,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = BTreeSet::new();
    match scope {
        Scope::Paths(explicit) => {
            paths.extend(explicit.iter().map(|path| normalize(repository, path)))
        }
        Scope::Staged => paths.extend(run_git(
            evidence,
            request_id,
            repository,
            &[
                "diff",
                "--cached",
                "--name-only",
                "-z",
                "--diff-filter=ACMRD",
            ],
        )?),
        Scope::WorkingTree => {
            paths.extend(run_git(
                evidence,
                request_id,
                repository,
                &[
                    "diff",
                    "--cached",
                    "--name-only",
                    "-z",
                    "--diff-filter=ACMRD",
                ],
            )?);
            paths.extend(run_git(
                evidence,
                request_id,
                repository,
                &["diff", "--name-only", "-z", "--diff-filter=ACMRD"],
            )?);
            paths.extend(run_git(
                evidence,
                request_id,
                repository,
                &["ls-files", "--others", "--exclude-standard", "-z"],
            )?);
        }
    }
    Ok(paths
        .into_iter()
        .filter(|path| !path.as_os_str().is_empty())
        .collect())
}

fn run_git(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    args: &[&str],
) -> Result<Vec<PathBuf>, String> {
    let owned: Vec<_> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let started = now_ms();
    let command_id = evidence.begin_command(request_id, "scope.git", "git", &owned, None)?;
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    let code = output.status.code().unwrap_or(1);
    evidence.finish_command(command_id, started, code)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| PathBuf::from(String::from_utf8_lossy(part).into_owned()))
        .collect())
}

fn normalize(repository: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(repository).unwrap_or(path).to_path_buf()
}
