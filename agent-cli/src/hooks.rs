// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const ZERO_OID: &str = "0000000000000000000000000000000000000000";

pub fn pre_push_paths(
    repository: &Path,
    remote: &str,
    updates: &str,
) -> Result<Vec<PathBuf>, String> {
    require_clean_tracked_tree(repository)?;
    let head = crate::git::value(repository, &["rev-parse", "HEAD"])?;
    let mut paths = BTreeSet::new();
    let mut branch_updates = 0_u64;
    for (line_number, line) in updates.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 4 {
            return Err(format!(
                "pre_push_invalid_update: line {} must contain four fields",
                line_number + 1
            ));
        }
        let local_oid = fields[1];
        let remote_ref = fields[2];
        let remote_oid = fields[3];
        if local_oid == ZERO_OID || !remote_ref.starts_with("refs/heads/") {
            continue;
        }
        branch_updates += 1;
        if local_oid != head {
            return Err(format!(
                "pre_push_non_head: {remote_ref} points to {local_oid}, but checked-out HEAD is {head}"
            ));
        }
        if remote_oid == ZERO_OID {
            paths.extend(new_branch_paths(repository, remote, local_oid)?);
        } else {
            paths.extend(diff_paths(repository, remote_oid, local_oid)?);
        }
    }
    if branch_updates == 0 {
        return Ok(Vec::new());
    }
    Ok(paths.into_iter().collect())
}

fn require_clean_tracked_tree(repository: &Path) -> Result<(), String> {
    for args in [
        &["diff", "--quiet", "--ignore-submodules", "--"][..],
        &["diff", "--cached", "--quiet", "--ignore-submodules", "--"][..],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(repository)
            .status()
            .map_err(|error| format!("pre_push_cleanliness_check_failed: {error}"))?;
        if !status.success() {
            return Err(
                "pre_push_dirty_tree: commit or restore tracked index and worktree changes before pushing"
                    .into(),
            );
        }
    }
    Ok(())
}

fn new_branch_paths(
    repository: &Path,
    remote: &str,
    local_oid: &str,
) -> Result<Vec<PathBuf>, String> {
    let remote_head = format!("refs/remotes/{remote}/HEAD");
    if let Ok(default_branch) =
        crate::git::value(repository, &["symbolic-ref", "--quiet", &remote_head])
        && let Ok(base) = crate::git::value(repository, &["merge-base", local_oid, &default_branch])
    {
        return diff_paths(repository, &base, local_oid);
    }
    root_paths(repository, local_oid)
}

fn diff_paths(repository: &Path, base: &str, head: &str) -> Result<Vec<PathBuf>, String> {
    git_paths(
        repository,
        &[
            "diff",
            "--name-only",
            "-z",
            "--diff-filter=ACMRD",
            base,
            head,
        ],
    )
}

fn root_paths(repository: &Path, head: &str) -> Result<Vec<PathBuf>, String> {
    git_paths(repository, &["ls-tree", "--name-only", "-z", "-r", head])
}

fn git_paths(repository: &Path, args: &[&str]) -> Result<Vec<PathBuf>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| format!("pre_push_diff_failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "pre_push_diff_failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    fn repository() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NONCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "agent-cli-hooks-{}-{nonce}-{sequence}",
            std::process::id()
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
        root
    }

    fn commit(root: &Path, path: &str, contents: &str) -> String {
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
                .args(["commit", "-qm", path])
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
        crate::git::value(root, &["rev-parse", "HEAD"]).unwrap()
    }

    #[test]
    fn existing_and_new_branch_updates_select_exact_committed_paths() {
        let root = repository();
        let base = commit(&root, "base.txt", "base\n");
        let head = commit(&root, "head.txt", "head\n");
        let existing = format!("{head} {head} refs/heads/main {base}\n");
        assert_eq!(
            pre_push_paths(&root, "origin", &existing).unwrap(),
            [PathBuf::from("head.txt")]
        );

        let new_branch = format!("{head} {head} refs/heads/topic {ZERO_OID}\n");
        assert_eq!(
            pre_push_paths(&root, "origin", &new_branch).unwrap(),
            [PathBuf::from("base.txt"), PathBuf::from("head.txt")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multiple_branches_union_paths_while_tags_and_deletions_are_skipped() {
        let root = repository();
        let base = commit(&root, "base.txt", "base\n");
        let head = commit(&root, "head.txt", "head\n");
        let updates = format!(
            "{head} {head} refs/heads/main {base}\n\
             {head} {head} refs/heads/topic {base}\n\
             refs/tags/v1 {head} refs/tags/v1 {ZERO_OID}\n\
             refs/heads/old {ZERO_OID} refs/heads/old {base}\n"
        );
        assert_eq!(
            pre_push_paths(&root, "origin", &updates).unwrap(),
            [PathBuf::from("head.txt")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pre_push_rejects_non_head_and_dirty_tracked_trees() {
        let root = repository();
        let base = commit(&root, "base.txt", "base\n");
        let head = commit(&root, "head.txt", "head\n");
        let non_head = format!("{base} {base} refs/heads/main {ZERO_OID}\n");
        assert!(
            pre_push_paths(&root, "origin", &non_head)
                .unwrap_err()
                .contains("pre_push_non_head")
        );

        fs::write(root.join("head.txt"), "dirty\n").unwrap();
        let update = format!("{head} {head} refs/heads/main {base}\n");
        assert!(
            pre_push_paths(&root, "origin", &update)
                .unwrap_err()
                .contains("pre_push_dirty_tree")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
