// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn value(repository: &Path, args: &[&str]) -> Result<String, String> {
    value_with_failure(
        repository,
        args,
        |error| error.to_string(),
        |stderr| stderr.to_owned(),
    )
}

pub fn value_with_context(
    repository: &Path,
    args: &[&str],
    context: &str,
) -> Result<String, String> {
    value_with_failure(
        repository,
        args,
        |error| format!("cannot {context}: {error}"),
        |_| format!("cannot {context}"),
    )
}

fn value_with_failure(
    repository: &Path,
    args: &[&str],
    spawn_error: impl FnOnce(std::io::Error) -> String,
    command_error: impl FnOnce(&str) -> String,
) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(spawn_error)?;
    if !output.status.success() {
        return Err(command_error(
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

pub fn succeeds(repository: &Path, args: &[&str]) -> Result<bool, String> {
    Command::new("git")
        .args(args)
        .current_dir(repository)
        .status()
        .map(|status| status.success())
        .map_err(|error| error.to_string())
}

pub fn changed_paths_including(
    repository: &Path,
    first_commit: &str,
    last_commit: &str,
) -> Result<Vec<PathBuf>, String> {
    let first_parent = format!("{first_commit}^");
    let output = Command::new("git")
        .args([
            "diff",
            "--name-only",
            "-z",
            "--diff-filter=ACMRD",
            &first_parent,
            last_commit,
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect())
}

pub fn head_changed_paths(repository: &Path) -> Result<Vec<PathBuf>, String> {
    let revision = value(repository, &["rev-list", "--parents", "-n", "1", "HEAD"])?;
    let mut revisions = revision.split_whitespace();
    let head = revisions
        .next()
        .ok_or_else(|| "cannot inspect HEAD parents".to_owned())?;
    let parents: Vec<_> = revisions.collect();
    let mut paths = BTreeSet::new();
    if parents.is_empty() {
        paths.extend(diff_paths(
            repository,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "-z",
                "--diff-filter=ACMRD",
                "-r",
                head,
            ],
        )?);
    } else {
        for parent in parents {
            paths.extend(diff_paths(
                repository,
                &[
                    "diff",
                    "--name-only",
                    "-z",
                    "--diff-filter=ACMRD",
                    parent,
                    head,
                ],
            )?);
        }
    }
    Ok(paths.into_iter().collect())
}

fn diff_paths(repository: &Path, args: &[&str]) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn repository() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("agent-cli-git-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        root
    }

    fn commit(root: &Path, path: &str, contents: &str, message: &str) {
        fs::write(root.join(path), contents).unwrap();
        assert!(
            Command::new("git")
                .args(["add", path])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args([
                    "-c",
                    "user.name=Agent",
                    "-c",
                    "user.email=agent@example.invalid",
                    "-c",
                    "maintenance.auto=false",
                ])
                .args(["commit", "-qm", message])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn value_trims_successful_output_and_reports_git_failures() {
        let root = repository();
        assert_eq!(value(&root, &["rev-parse", "--git-dir"]).unwrap(), ".git");
        let error = value(&root, &["rev-parse", "missing-object"]).unwrap_err();
        assert!(error.contains("missing-object"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn contextual_value_uses_stable_failure_classification() {
        let root = repository();
        assert_eq!(
            value_with_context(
                &root,
                &["rev-parse", "missing-object"],
                "inspect Git identity"
            )
            .unwrap_err(),
            "cannot inspect Git identity"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn head_paths_support_root_and_merge_commits() {
        let root = repository();
        commit(&root, "root.txt", "root\n", "root");
        assert_eq!(
            head_changed_paths(&root).unwrap(),
            [PathBuf::from("root.txt")]
        );

        assert!(
            Command::new("git")
                .args(["switch", "-qc", "side"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        commit(&root, "side.txt", "side\n", "side");
        assert!(
            Command::new("git")
                .args(["switch", "-q", "-"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        commit(&root, "main.txt", "main\n", "main");
        assert!(
            Command::new("git")
                .args([
                    "-c",
                    "user.name=Agent",
                    "-c",
                    "user.email=agent@example.invalid",
                    "-c",
                    "maintenance.auto=false",
                ])
                .args(["merge", "-q", "--no-ff", "side", "-m", "merge"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            head_changed_paths(&root).unwrap(),
            [PathBuf::from("main.txt"), PathBuf::from("side.txt")]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
