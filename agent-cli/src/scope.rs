// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::{Evidence, now_ms};
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
        Scope::Task(task_id) => paths.extend(crate::task::changes(evidence, repository, task_id)?),
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
    let command_id =
        evidence.begin_command(request_id, "scope.git", "git", &owned, None, "git_index")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::Evidence;
    use crate::request::RawRequest;
    use std::fs;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn git_scopes_distinguish_working_tree_from_index() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-scope-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Agent"],
            vec!["config", "user.email", "agent@example.invalid"],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        fs::write(root.join("modified.txt"), "before\n").unwrap();
        fs::write(root.join("deleted.txt"), "delete me\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "."])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["commit", "-qm", "baseline"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );

        fs::write(root.join("modified.txt"), "after\n").unwrap();
        fs::remove_file(root.join("deleted.txt")).unwrap();
        fs::write(root.join("untracked.txt"), "new\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "modified.txt"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );

        let evidence_root = root.with_extension("evidence");
        let evidence = Evidence::open_at(&evidence_root).unwrap();
        let request = RawRequest {
            id: "scope-test".into(),
            args: vec!["agent-cli".into(), "check".into()],
            started_ms: now_ms(),
            started: Instant::now(),
        };
        evidence.begin_request(&request).unwrap();

        assert_eq!(
            collect(&evidence, &request.id, &root, &Scope::Staged).unwrap(),
            [PathBuf::from("modified.txt")]
        );
        assert_eq!(
            collect(&evidence, &request.id, &root, &Scope::WorkingTree).unwrap(),
            [
                PathBuf::from("deleted.txt"),
                PathBuf::from("modified.txt"),
                PathBuf::from("untracked.txt")
            ]
        );
        drop(evidence);
        fs::remove_dir_all(evidence_root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
