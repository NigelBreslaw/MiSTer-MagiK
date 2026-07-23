// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::Evidence;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

const PLANNER_SCHEMA: u8 = 4;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    pub(crate) planner_schema: u8,
    pub(crate) head: String,
    toolchain: String,
    files: BTreeMap<PathBuf, Fingerprint>,
    #[serde(default)]
    pub(crate) dirty_paths: BTreeSet<PathBuf>,
    #[serde(default)]
    pub(crate) staged_paths: BTreeSet<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Fingerprint {
    kind: String,
    mode: u32,
    hash: String,
}

#[derive(Debug, Default)]
struct WorkspaceState {
    dirty: BTreeSet<PathBuf>,
    staged: BTreeSet<PathBuf>,
    untracked: BTreeSet<PathBuf>,
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
    let baseline = if replace {
        if let Some(active_id) = evidence.active_task_id_for_session(repository, task_id)? {
            let mut baseline = load(evidence, repository, &active_id)?;
            reconcile_head_advance(evidence, repository, &active_id, &mut baseline)?;
            baseline
        } else {
            capture(repository)?
        }
    } else {
        capture(repository)?
    };
    evidence
        .save_task_baseline(task_id, repository, &baseline, replace)
        .map(|_| ())
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
    let Some((worktree, mut baseline)): Option<(PathBuf, Baseline)> =
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
    reconcile_head_advance(evidence, repository, task_id, &mut baseline)?;
    let mut paths = changed_paths(repository, false)?;
    paths.extend(baseline.dirty_paths.iter().cloned());
    if baseline.head != current_head(repository)? {
        paths.extend(diff_paths(repository, &baseline.head)?);
    }
    let mut changes = Vec::new();
    for path in paths {
        let current = if baseline.planner_schema >= 4 {
            fingerprint_v4(repository, &path)?
        } else {
            fingerprint(&repository.join(&path))?
        };
        if baseline.planner_schema < 3
            && !baseline.files.contains_key(&path)
            && current
                .as_ref()
                .is_some_and(|fingerprint| fingerprint.kind == "gitlink")
        {
            continue;
        }
        if baseline.files.get(&path) != current.as_ref() {
            changes.push(path);
        }
    }
    Ok(changes)
}

fn reconcile_head_advance(
    evidence: &Evidence,
    repository: &Path,
    task_id: &str,
    baseline: &mut Baseline,
) -> Result<(), String> {
    let head = current_head(repository)?;
    if baseline.head == head {
        return Ok(());
    }
    if !is_ancestor(repository, &baseline.head, &head)? {
        return Err(format!(
            "baseline_head_changed: task began at {}, current HEAD is {head}; history diverged",
            baseline.head
        ));
    }

    let state = workspace_state(repository)?;
    if !baseline.staged_paths.is_empty() || !state.staged.is_empty() {
        return Err(format!(
            "baseline_head_changed: task began at {}, current HEAD is {head}; staged changes prevent safe reconciliation",
            baseline.head
        ));
    }

    let intervening = diff_paths(repository, &baseline.head)?;
    let protected: BTreeSet<_> = state.dirty.union(&baseline.dirty_paths).cloned().collect();
    let overlap: Vec<_> = intervening.intersection(&protected).cloned().collect();
    if !overlap.is_empty() {
        return Err(format!(
            "baseline_head_changed: task began at {}, current HEAD is {head}; intervening commits overlap task or baseline paths: {}",
            baseline.head,
            display_paths(&overlap)
        ));
    }

    for path in intervening {
        match fingerprint_v4(repository, &path)? {
            Some(fingerprint) => {
                baseline.files.insert(path, fingerprint);
            }
            None => {
                baseline.files.remove(&path);
            }
        }
    }
    if current_head(repository)? != head {
        return Err(
            "baseline_head_changed: HEAD changed during baseline reconciliation; retry the command"
                .into(),
        );
    }
    baseline.head = head;
    baseline.toolchain = toolchain_identity(repository);
    evidence.update_task_baseline(task_id, repository, baseline)
}

fn is_ancestor(repository: &Path, ancestor: &str, descendant: &str) -> Result<bool, String> {
    let output = Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn diff_paths(repository: &Path, baseline_head: &str) -> Result<BTreeSet<PathBuf>, String> {
    let output = Command::new("git")
        .args([
            "diff",
            "--name-only",
            "-z",
            "--diff-filter=ACMRD",
            baseline_head,
            "HEAD",
        ])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
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

pub fn status(
    evidence: &Evidence,
    repository: &Path,
    task_id: &str,
) -> Result<Vec<PathBuf>, String> {
    changes(evidence, repository, task_id)
}

pub(crate) fn load(
    evidence: &Evidence,
    repository: &Path,
    task_id: &str,
) -> Result<Baseline, String> {
    let Some((worktree, baseline)) = evidence.load_task_baseline(task_id)? else {
        return Err(format!(
            "task_baseline_missing: no active task baseline exists for {task_id}"
        ));
    };
    if canonical(&worktree) != canonical(repository) {
        return Err(format!(
            "task_baseline_missing: task {task_id} belongs to {}, not {}",
            worktree.display(),
            repository.display()
        ));
    }
    Ok(baseline)
}

pub fn current_head(repository: &Path) -> Result<String, String> {
    git(repository, &["rev-parse", "HEAD"])
}

pub(crate) fn legacy_baseline_was_clean(
    repository: &Path,
    baseline: &Baseline,
) -> Result<bool, String> {
    let output = Command::new("git")
        .args(["ls-tree", "-r", "-z", &baseline.head])
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let mut tracked = BTreeSet::new();
    for entry in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
    {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            return Ok(false);
        };
        let header = String::from_utf8_lossy(&entry[..tab]);
        let mut fields = header.split_whitespace();
        let mode = fields.next().unwrap_or("");
        let kind = fields.next().unwrap_or("");
        let oid = fields.next().unwrap_or("");
        let path = PathBuf::from(String::from_utf8_lossy(&entry[tab + 1..]).into_owned());
        tracked.insert(path.clone());
        if kind == "commit" {
            continue;
        }
        let Some(fingerprint) = baseline.files.get(&path) else {
            return Ok(false);
        };
        let blob = Command::new("git")
            .args(["cat-file", "blob", oid])
            .current_dir(repository)
            .output()
            .map_err(|error| error.to_string())?;
        let content_matches = fingerprint.hash == format!("git:{oid}")
            || fingerprint.hash == format!("{:016x}", fnv1a(&blob.stdout));
        if !blob.status.success()
            || !content_matches
            || fingerprint.kind != if mode == "120000" { "symlink" } else { "file" }
            || (mode != "120000" && (fingerprint.mode & 0o111 != 0) != (mode == "100755"))
        {
            return Ok(false);
        }
    }
    Ok(baseline.files.keys().all(|path| tracked.contains(path)))
}

fn capture(repository: &Path) -> Result<Baseline, String> {
    let head = git(repository, &["rev-parse", "HEAD"])?;
    let state = workspace_state(repository)?;
    let output = Command::new("git")
        .args(["ls-files", "-s", "-z"])
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
        let Some(tab) = raw.iter().position(|byte| *byte == b'\t') else {
            return Err("git ls-files returned a malformed index entry".into());
        };
        let header = String::from_utf8_lossy(&raw[..tab]);
        let mut fields = header.split_whitespace();
        let mode = fields.next().unwrap_or("");
        let oid = fields.next().unwrap_or("");
        let stage = fields.next().unwrap_or("");
        if stage != "0" {
            continue;
        }
        let path = PathBuf::from(String::from_utf8_lossy(&raw[tab + 1..]).into_owned());
        let fingerprint = if state.dirty.contains(&path) {
            fingerprint_v4(repository, &path)?
        } else {
            index_fingerprint(mode, oid)
        };
        if let Some(fingerprint) = fingerprint {
            files.insert(path, fingerprint);
        }
    }
    for path in state.untracked {
        if let Some(fingerprint) = fingerprint_v4(repository, &path)? {
            files.insert(path, fingerprint);
        }
    }
    Ok(Baseline {
        planner_schema: PLANNER_SCHEMA,
        head,
        toolchain: toolchain_identity(repository),
        files,
        dirty_paths: state.dirty,
        staged_paths: state.staged,
    })
}

fn workspace_state(repository: &Path) -> Result<WorkspaceState, String> {
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot inspect task changes: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let entries: Vec<_> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect();
    let mut dirty = BTreeSet::new();
    let mut staged = BTreeSet::new();
    let mut untracked = BTreeSet::new();
    let mut index = 0;
    while index < entries.len() {
        let entry = entries[index];
        if entry.len() < 4 || entry[2] != b' ' {
            return Err("git status returned a malformed porcelain entry".into());
        }
        let x = entry[0];
        let y = entry[1];
        let path = PathBuf::from(String::from_utf8_lossy(&entry[3..]).into_owned());
        dirty.insert(path.clone());
        if x == b'?' {
            untracked.insert(path);
        } else if x != b' ' {
            staged.insert(path.clone());
        }
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            index += 1;
            let Some(origin) = entries.get(index) else {
                return Err("git status omitted a rename origin".into());
            };
            let origin = PathBuf::from(String::from_utf8_lossy(origin).into_owned());
            dirty.insert(origin.clone());
            if x != b' ' {
                staged.insert(origin);
            }
        }
        index += 1;
    }
    Ok(WorkspaceState {
        dirty,
        staged,
        untracked,
    })
}

fn index_fingerprint(mode: &str, oid: &str) -> Option<Fingerprint> {
    let kind = match mode {
        "120000" => "symlink",
        "160000" => "gitlink",
        "100644" | "100755" => "file",
        _ => return None,
    };
    Some(Fingerprint {
        kind: kind.into(),
        mode: u32::from_str_radix(mode, 8).unwrap_or_default(),
        hash: format!("git:{oid}"),
    })
}

fn fingerprint_v4(repository: &Path, relative: &Path) -> Result<Option<Fingerprint>, String> {
    let path = repository.join(relative);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    let (kind, mode, hash) = if metadata.file_type().is_symlink() {
        let target = fs::read_link(&path).map_err(|error| error.to_string())?;
        (
            "symlink",
            0o120000,
            format!("{:016x}", fnv1a(target.as_os_str().as_encoded_bytes())),
        )
    } else if metadata.is_dir() && path.join(".git").exists() {
        let oid = git(&path, &["rev-parse", "HEAD"])?;
        ("gitlink", 0o160000, format!("git:{oid}"))
    } else if metadata.is_file() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        #[cfg(unix)]
        let executable = metadata.permissions().mode() & 0o111 != 0;
        #[cfg(not(unix))]
        let executable = false;
        let file = fs::File::open(&path)
            .map_err(|error| format!("cannot fingerprint {}: {error}", path.display()))?;
        let hash = fnv1a_reader(BufReader::new(file))
            .map_err(|error| format!("cannot fingerprint {}: {error}", path.display()))?;
        (
            "file",
            if executable { 0o100755 } else { 0o100644 },
            format!("{hash:016x}"),
        )
    } else {
        return Ok(None);
    };
    Ok(Some(Fingerprint {
        kind: kind.into(),
        mode,
        hash,
    }))
}

fn fnv1a_reader(mut reader: impl Read) -> std::io::Result<u64> {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(hash);
        }
        for byte in &buffer[..read] {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    }
}

fn changed_paths(repository: &Path, staged_only: bool) -> Result<BTreeSet<PathBuf>, String> {
    let commands: &[&[&str]] = if staged_only {
        &[&[
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--diff-filter=ACMRD",
        ]]
    } else {
        &[
            &[
                "diff",
                "--cached",
                "--name-only",
                "-z",
                "--diff-filter=ACMRD",
            ],
            &["diff", "--name-only", "-z", "--diff-filter=ACMRD"],
            &["ls-files", "--others", "--exclude-standard", "-z"],
        ]
    };
    let mut paths = BTreeSet::new();
    for args in commands {
        let output = Command::new("git")
            .args(*args)
            .current_dir(repository)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        paths.extend(
            output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| PathBuf::from(String::from_utf8_lossy(part).into_owned())),
        );
    }
    Ok(paths)
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
    } else if metadata.is_dir() && path.join(".git").exists() {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(format!("cannot fingerprint submodule {}", path.display()));
        }
        ("gitlink", output.stdout)
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
    crate::git::value(repository, args)
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

    #[test]
    fn safe_head_advance_preserves_task_changes_and_claims() {
        let root = fixture_root("safe-head-advance");
        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        begin(&evidence, &root, "task-safe", false).unwrap();
        fs::write(root.join("task.txt"), "task change\n").unwrap();
        evidence
            .claim_task_paths("task-safe", &[PathBuf::from("task.txt")])
            .unwrap();

        fs::write(root.join("upstream.txt"), "upstream change\n").unwrap();
        run(&root, &["add", "upstream.txt"]);
        run(&root, &["commit", "-qm", "upstream"]);
        let head = current_head(&root).unwrap();

        assert_eq!(
            changes(&evidence, &root, "task-safe").unwrap(),
            [PathBuf::from("task.txt")]
        );
        let (_, baseline): (PathBuf, Baseline) =
            evidence.load_task_baseline("task-safe").unwrap().unwrap();
        assert_eq!(baseline.head, head);
        assert_eq!(
            evidence.task_claims("task-safe").unwrap(),
            [PathBuf::from("task.txt")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clean_head_advance_reconciles_to_no_task_changes() {
        let root = fixture_root("clean-head-advance");
        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        begin(&evidence, &root, "task-clean", false).unwrap();
        fs::write(root.join("upstream.txt"), "upstream change\n").unwrap();
        run(&root, &["add", "upstream.txt"]);
        run(&root, &["commit", "-qm", "upstream"]);

        assert!(changes(&evidence, &root, "task-clean").unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_replacement_preserves_stale_task_changes() {
        let root = fixture_root("dirty-baseline-replacement");
        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        begin(&evidence, &root, "thread-one", false).unwrap();
        fs::write(root.join("tracked.txt"), "task change\n").unwrap();

        begin(&evidence, &root, "thread-one", true).unwrap();
        assert_eq!(
            evidence
                .active_task_id_for_session(&root, "thread-one")
                .unwrap(),
            Some("thread-one::g2".into())
        );
        assert_eq!(
            changes(&evidence, &root, "thread-one::g2").unwrap(),
            [PathBuf::from("tracked.txt")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clean_replacement_creates_a_new_lifecycle() {
        let root = fixture_root("clean-baseline-replacement");
        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        begin(&evidence, &root, "thread-one", false).unwrap();
        begin(&evidence, &root, "thread-one", true).unwrap();

        assert_eq!(
            evidence
                .active_task_id_for_session(&root, "thread-one")
                .unwrap(),
            Some("thread-one::g2".into())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn head_advance_rejects_overlap_staging_and_divergence() {
        let root = fixture_root("overlapping-head-advance");
        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        begin(&evidence, &root, "task-overlap", false).unwrap();
        fs::write(root.join("tracked.txt"), "committed task change\n").unwrap();
        run(&root, &["add", "tracked.txt"]);
        run(&root, &["commit", "-qm", "intervening overlap"]);
        fs::write(root.join("tracked.txt"), "remaining task change\n").unwrap();
        assert!(changes(&evidence, &root, "task-overlap")
            .unwrap_err()
            .contains("intervening commits overlap"));
        fs::remove_dir_all(root).unwrap();

        let root = fixture_root("baseline-dirty-head-advance");
        fs::write(root.join("tracked.txt"), "pre-existing change\n").unwrap();
        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        begin(&evidence, &root, "task-baseline-dirty", false).unwrap();
        run(&root, &["add", "tracked.txt"]);
        run(&root, &["commit", "-qm", "intervening baseline overlap"]);
        assert!(changes(&evidence, &root, "task-baseline-dirty")
            .unwrap_err()
            .contains("intervening commits overlap"));
        fs::remove_dir_all(root).unwrap();

        let root = fixture_root("staged-head-advance");
        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        begin(&evidence, &root, "task-staged", false).unwrap();
        fs::write(root.join("upstream.txt"), "upstream change\n").unwrap();
        run(&root, &["add", "upstream.txt"]);
        run(&root, &["commit", "-qm", "upstream"]);
        fs::write(root.join("staged.txt"), "staged change\n").unwrap();
        run(&root, &["add", "staged.txt"]);
        assert!(changes(&evidence, &root, "task-staged")
            .unwrap_err()
            .contains("staged changes prevent"));
        fs::remove_dir_all(root).unwrap();

        let root = fixture_root("diverged-head-advance");
        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        run(&root, &["checkout", "-qb", "baseline"]);
        fs::write(root.join("baseline.txt"), "baseline branch\n").unwrap();
        run(&root, &["add", "baseline.txt"]);
        run(&root, &["commit", "-qm", "baseline branch"]);
        begin(&evidence, &root, "task-diverged", false).unwrap();
        run(&root, &["checkout", "-qb", "diverged", "HEAD^"]);
        fs::write(root.join("diverged.txt"), "diverged branch\n").unwrap();
        run(&root, &["add", "diverged.txt"]);
        run(&root, &["commit", "-qm", "diverged branch"]);
        assert!(changes(&evidence, &root, "task-diverged")
            .unwrap_err()
            .contains("history diverged"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_baseline_can_prove_clean_but_rejects_preexisting_changes() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-legacy-task-{}-{}",
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
        fs::write(root.join("tracked.txt"), "original\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-qm", "fixture"]);

        let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
        begin(&evidence, &root, "clean", false).unwrap();
        let (_, mut clean): (PathBuf, Baseline) =
            evidence.load_task_baseline("clean").unwrap().unwrap();
        clean.planner_schema = 2;
        clean.dirty_paths.clear();
        clean.staged_paths.clear();
        assert!(legacy_baseline_was_clean(&root, &clean).unwrap());

        fs::write(root.join("tracked.txt"), "preexisting\n").unwrap();
        begin(&evidence, &root, "dirty", false).unwrap();
        let (_, mut dirty): (PathBuf, Baseline) =
            evidence.load_task_baseline("dirty").unwrap().unwrap();
        dirty.planner_schema = 2;
        dirty.dirty_paths.clear();
        dirty.staged_paths.clear();
        assert!(!legacy_baseline_was_clean(&root, &dirty).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-task-{label}-{}-{}",
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
        fs::write(root.join("tracked.txt"), "original\n").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-qm", "fixture"]);
        root
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
