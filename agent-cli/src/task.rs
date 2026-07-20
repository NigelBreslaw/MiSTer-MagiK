// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::Evidence;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLANNER_SCHEMA: u8 = 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    planner_schema: u8,
    head: String,
    toolchain: String,
    files: BTreeMap<PathBuf, Fingerprint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Fingerprint {
    kind: String,
    mode: u32,
    hash: String,
}

pub fn begin(
    evidence: &Evidence,
    repository: &Path,
    task_id: &str,
    replace: bool,
) -> Result<(), String> {
    if task_id.is_empty() {
        return Err(
            "no task identity is available; pass --task-id ID or set MISTER_AGENT_TASK_ID".into(),
        );
    }
    let baseline = capture(repository)?;
    evidence.save_task_baseline(task_id, repository, &baseline, replace)
}

pub fn changes(
    evidence: &Evidence,
    repository: &Path,
    task_id: &str,
) -> Result<Vec<PathBuf>, String> {
    if task_id.is_empty() {
        return Err(
            "No task baseline exists. Run `scripts/agent task begin` before editing.".into(),
        );
    }
    let Some((worktree, baseline)): Option<(PathBuf, Baseline)> =
        evidence.load_task_baseline(task_id)?
    else {
        return Err(
            "No task baseline exists. Run `scripts/agent task begin` before editing.".into(),
        );
    };
    if canonical(&worktree) != canonical(repository) {
        return Err(format!(
            "task {task_id} belongs to {}, not {}",
            worktree.display(),
            repository.display()
        ));
    }
    let current = capture(repository)?;
    let paths: BTreeSet<_> = baseline
        .files
        .keys()
        .chain(current.files.keys())
        .cloned()
        .collect();
    Ok(paths
        .into_iter()
        .filter(|path| baseline.files.get(path) != current.files.get(path))
        .collect())
}

pub fn status(
    evidence: &Evidence,
    repository: &Path,
    task_id: &str,
) -> Result<Vec<PathBuf>, String> {
    changes(evidence, repository, task_id)
}

fn capture(repository: &Path) -> Result<Baseline, String> {
    let head = git(repository, &["rev-parse", "HEAD"])?;
    let output = Command::new("git")
        .args(["ls-files", "-co", "--exclude-standard", "-z"])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot enumerate task files: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let mut files = BTreeMap::new();
    for raw in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
    {
        let path = PathBuf::from(String::from_utf8_lossy(raw).into_owned());
        if let Some(fingerprint) = fingerprint(&repository.join(&path))? {
            files.insert(path, fingerprint);
        }
    }
    Ok(Baseline {
        planner_schema: PLANNER_SCHEMA,
        head,
        toolchain: toolchain_identity(repository),
        files,
    })
}

fn toolchain_identity(repository: &Path) -> String {
    let rustc = Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "rustc-unavailable".into());
    let pin =
        fs::read_to_string(repository.join("apps/mister/rust-toolchain.toml")).unwrap_or_default();
    format!("{rustc}\n{pin}")
}

fn fingerprint(path: &Path) -> Result<Option<Fingerprint>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    let mode = metadata.permissions().mode();
    #[cfg(not(unix))]
    let mode = u32::from(metadata.permissions().readonly());
    let (kind, bytes) = if metadata.file_type().is_symlink() {
        (
            "symlink",
            fs::read_link(path)
                .map_err(|error| error.to_string())?
                .as_os_str()
                .as_encoded_bytes()
                .to_vec(),
        )
    } else if metadata.is_file() {
        let mut bytes = Vec::new();
        fs::File::open(path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|error| format!("cannot fingerprint {}: {error}", path.display()))?;
        ("file", bytes)
    } else {
        return Ok(None);
    };
    Ok(Some(Fingerprint {
        kind: kind.into(),
        mode,
        hash: format!("{:016x}", fnv1a(&bytes)),
    }))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn git(repository: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn baselines_isolate_preexisting_and_concurrent_task_changes() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-task-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        run(&root, &["init", "-q"]);
        run(&root, &["config", "user.name", "Agent CLI Test"]);
        run(
            &root,
            &["config", "user.email", "agent-cli@example.invalid"],
        );
        fs::write(root.join("existing.txt"), "original\n").unwrap();
        fs::write(root.join("feature.txt"), "original\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-qm", "fixture"]);
        fs::write(root.join("existing.txt"), "pre-existing dirty\n").unwrap();

        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        begin(&evidence, &root, "task-a", false).unwrap();
        fs::write(root.join("feature.txt"), "task change\n").unwrap();

        assert_eq!(
            changes(&evidence, &root, "task-a").unwrap(),
            [PathBuf::from("feature.txt")]
        );
        begin(&evidence, &root, "task-b", false).unwrap();
        assert_eq!(
            changes(&evidence, &root, "task-b").unwrap(),
            Vec::<PathBuf>::new()
        );
        fs::write(root.join("existing.txt"), "task changed dirty file\n").unwrap();
        assert_eq!(
            changes(&evidence, &root, "task-a").unwrap(),
            [PathBuf::from("existing.txt"), PathBuf::from("feature.txt")]
        );
        assert_eq!(
            changes(&evidence, &root, "task-b").unwrap(),
            [PathBuf::from("existing.txt")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn run(root: &Path, args: &[&str]) {
        assert!(Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
}
