// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const WORKFLOW: &str = "platform-bundle.yml";
const WAIT_DEADLINE: Duration = Duration::from_secs(45 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(15);
const COMMAND_DEADLINE: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Candidate {
    pub run_id: u64,
    pub head_sha: String,
    pub archive: PathBuf,
    pub manifest: PathBuf,
    pub reused: bool,
    pub head_branch: String,
    pub bundle_id: String,
    pub main_identity: String,
    pub fpga_identity: String,
    pub kernel_identity: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Run {
    pub id: u64,
    pub head_sha: String,
    pub status: String,
    pub conclusion: String,
    pub workflow: String,
    pub head_branch: String,
}

pub trait PlatformCi {
    fn runs(&mut self, head_sha: &str) -> Result<Vec<Run>, String>;
    fn dispatch(&mut self, branch: &str) -> Result<(), String>;
    fn download(&mut self, run_id: u64, destination: &Path) -> Result<(), String>;
}

pub fn resolve(
    ci: &mut dyn PlatformCi,
    repository: &Path,
    branch: &str,
    head_sha: &str,
    destination: &Path,
    mut progress: impl FnMut(&str) -> Result<(), String>,
) -> Result<Candidate, String> {
    if let Some(run) = exact_success(ci.runs(head_sha)?, branch, head_sha)? {
        progress("reusing exact verified platform candidate")?;
        match download_and_verify(
            ci,
            repository,
            run.id,
            head_sha,
            &run.head_branch,
            destination,
            true,
        ) {
            Ok(candidate) => return Ok(candidate),
            Err(_) => progress("exact platform candidate is unavailable; dispatching replacement")?,
        }
    }
    progress("dispatching platform candidate workflow")?;
    ci.dispatch(branch)?;
    let started = Instant::now();
    while started.elapsed() < WAIT_DEADLINE {
        let runs = ci.runs(head_sha)?;
        if let Some(run) = exact_success(runs.clone(), branch, head_sha)? {
            progress("platform candidate workflow completed")?;
            return download_and_verify(
                ci,
                repository,
                run.id,
                head_sha,
                &run.head_branch,
                destination,
                false,
            );
        }
        if runs.iter().any(|run| {
            run.head_sha == head_sha && run.status == "completed" && run.conclusion != "success"
        }) {
            return Err("platform CI workflow failed for the requested commit".into());
        }
        progress("waiting for platform candidate workflow")?;
        thread::sleep(POLL_INTERVAL);
    }
    Err("platform CI workflow exceeded its 2700s deadline".into())
}

fn exact_success(runs: Vec<Run>, branch: &str, head_sha: &str) -> Result<Option<Run>, String> {
    let mut matching: Vec<_> = runs
        .into_iter()
        .filter(|run| {
            run.head_sha == head_sha
                && run.head_branch == branch
                && run.workflow == WORKFLOW
                && run.status == "completed"
                && run.conclusion == "success"
        })
        .collect();
    matching.sort_by_key(|run| std::cmp::Reverse(run.id));
    Ok(matching.into_iter().next())
}

fn download_and_verify(
    ci: &mut dyn PlatformCi,
    repository: &Path,
    run_id: u64,
    head_sha: &str,
    head_branch: &str,
    destination: &Path,
    reused: bool,
) -> Result<Candidate, String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("cannot create platform candidate directory: {error}"))?;
    ci.download(run_id, destination)?;
    let archive = find_named(destination, "mister-magik-platform-", ".zip")?;
    let manifest = find_exact(destination, "platform-bundle-v0.2.json")?;
    let mut verify = Command::new("python3");
    verify
        .arg(repository.join("scripts/release/platform/platform-bundle.py"))
        .arg("verify")
        .arg(&archive)
        .arg("--manifest")
        .arg(&manifest)
        .current_dir(repository);
    let verification = bounded_output(verify, "platform candidate verification", COMMAND_DEADLINE)?;
    if !verification.status.success() {
        let detail = String::from_utf8_lossy(&verification.stderr);
        return Err(format!(
            "downloaded platform candidate failed verification: {}",
            detail.trim()
        ));
    }
    let payload: Value = serde_json::from_slice(
        &std::fs::read(&manifest)
            .map_err(|error| format!("cannot read platform candidate manifest: {error}"))?,
    )
    .map_err(|error| format!("invalid platform candidate manifest: {error}"))?;
    let origin_sha = payload
        .get("source")
        .and_then(|source| source.get("magik_revision"))
        .or_else(|| payload.get("magik_revision"))
        .and_then(Value::as_str);
    if origin_sha.is_some_and(|sha| sha != head_sha) {
        return Err("platform candidate manifest does not match the requested commit".into());
    }
    let required = |name: &str| {
        payload
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("platform candidate manifest is missing {name}"))
    };
    Ok(Candidate {
        run_id,
        head_sha: head_sha.into(),
        archive,
        manifest,
        reused,
        head_branch: head_branch.into(),
        bundle_id: required("bundle_id")?,
        main_identity: required("main_input_sha256")?,
        fpga_identity: required("fpga_input_sha256")?,
        kernel_identity: required("kernel_input_sha256")?,
    })
}

fn find_exact(root: &Path, name: &str) -> Result<PathBuf, String> {
    find(root, &|path| {
        path.file_name().and_then(|value| value.to_str()) == Some(name)
    })
}

fn find_named(root: &Path, prefix: &str, suffix: &str) -> Result<PathBuf, String> {
    find(root, &|path| {
        path.file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
    })
}

fn find(root: &Path, predicate: &dyn Fn(&Path) -> bool) -> Result<PathBuf, String> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?
        {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                pending.push(path);
            } else if predicate(&path) {
                return Ok(path);
            }
        }
    }
    Err(format!(
        "required platform artifact is missing under {}",
        root.display()
    ))
}

#[derive(Clone, Debug)]
pub struct GhPlatformCi {
    repository_name: String,
}

impl GhPlatformCi {
    #[must_use]
    pub fn new(repository_name: impl Into<String>) -> Self {
        Self {
            repository_name: repository_name.into(),
        }
    }
}

pub fn resolve_repository(
    repository: &Path,
    mut progress: impl FnMut(&str) -> Result<(), String>,
) -> Result<Candidate, String> {
    let owner = command_text(
        repository,
        "gh",
        &[
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "--jq",
            ".nameWithOwner",
        ],
    )?;
    let branch = command_text(repository, "git", &["branch", "--show-current"])?;
    if branch.is_empty() {
        return Err("platform deployment requires a named branch".into());
    }
    let head_sha = command_text(repository, "git", &["rev-parse", "HEAD"])?;
    let destination = repository
        .join("build/agent-deploy/platform")
        .join(&head_sha);
    let mut ci = GhPlatformCi::new(owner);
    resolve(
        &mut ci,
        repository,
        &branch,
        &head_sha,
        &destination,
        &mut progress,
    )
}

fn command_text(repository: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args).current_dir(repository);
    let output = bounded_output(command, program, COMMAND_DEADLINE)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

impl PlatformCi for GhPlatformCi {
    fn runs(&mut self, head_sha: &str) -> Result<Vec<Run>, String> {
        let mut command = Command::new("gh");
        command.args([
            "run",
            "list",
            "--repo",
            &self.repository_name,
            "--workflow",
            WORKFLOW,
            "--commit",
            head_sha,
            "--limit",
            "20",
            "--json",
            "databaseId,headSha,headBranch,status,conclusion,workflowName",
        ]);
        let output = bounded_output(command, "platform CI run listing", COMMAND_DEADLINE)?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
        }
        let rows: Vec<Value> = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("invalid GitHub run response: {error}"))?;
        rows.into_iter()
            .map(|row| {
                Ok(Run {
                    id: row["databaseId"]
                        .as_u64()
                        .ok_or("GitHub run is missing databaseId")?,
                    head_sha: row["headSha"].as_str().unwrap_or_default().into(),
                    status: row["status"].as_str().unwrap_or_default().into(),
                    conclusion: row["conclusion"].as_str().unwrap_or_default().into(),
                    workflow: WORKFLOW.into(),
                    head_branch: row["headBranch"].as_str().unwrap_or_default().into(),
                })
            })
            .collect()
    }

    fn dispatch(&mut self, branch: &str) -> Result<(), String> {
        let mut command = Command::new("gh");
        command.args([
            "workflow",
            "run",
            WORKFLOW,
            "--repo",
            &self.repository_name,
            "--ref",
            branch,
            "-f",
            "publish=false",
        ]);
        if bounded_output(command, "platform CI dispatch", COMMAND_DEADLINE)?
            .status
            .success()
        {
            Ok(())
        } else {
            Err("platform CI workflow dispatch failed".into())
        }
    }

    fn download(&mut self, run_id: u64, destination: &Path) -> Result<(), String> {
        let mut command = Command::new("gh");
        command
            .args([
                "run",
                "download",
                &run_id.to_string(),
                "--repo",
                &self.repository_name,
                "--pattern",
                "platform-bundle-*-candidate",
                "--dir",
            ])
            .arg(destination);
        if bounded_output(command, "platform candidate download", COMMAND_DEADLINE)?
            .status
            .success()
        {
            Ok(())
        } else {
            Err("platform candidate download failed".into())
        }
    }
}

fn bounded_output(mut command: Command, label: &str, deadline: Duration) -> Result<Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start {label}: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("cannot capture {label} stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("cannot capture {label} stderr"))?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < deadline => thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{label} exceeded its {}s deadline",
                    deadline.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for {label}: {error}"));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| format!("{label} stdout reader failed"))?
        .map_err(|error| format!("cannot read {label} stdout: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| format!("{label} stderr reader failed"))?
        .map_err(|error| format!("cannot read {label} stderr: {error}"))?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakeCi {
        batches: VecDeque<Vec<Run>>,
        dispatches: usize,
        downloads: VecDeque<Result<(), String>>,
    }

    impl PlatformCi for FakeCi {
        fn runs(&mut self, _: &str) -> Result<Vec<Run>, String> {
            Ok(self.batches.pop_front().unwrap_or_default())
        }

        fn dispatch(&mut self, _: &str) -> Result<(), String> {
            self.dispatches += 1;
            Ok(())
        }

        fn download(&mut self, _: u64, _: &Path) -> Result<(), String> {
            self.downloads
                .pop_front()
                .unwrap_or_else(|| Err("fixture download intentionally unavailable".into()))
        }
    }

    fn run(id: u64, sha: &str, conclusion: &str) -> Run {
        Run {
            id,
            head_sha: sha.into(),
            status: "completed".into(),
            conclusion: conclusion.into(),
            workflow: WORKFLOW.into(),
            head_branch: "main".into(),
        }
    }

    #[test]
    fn exact_success_prefers_newest_matching_run() {
        let selected = exact_success(
            vec![run(1, "wanted", "success"), run(2, "wanted", "success")],
            "main",
            "wanted",
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.id, 2);
    }

    #[test]
    fn stale_wrong_commit_and_failed_runs_are_not_reused() {
        assert!(exact_success(
            vec![run(1, "stale", "success"), run(2, "wanted", "failure")],
            "main",
            "wanted"
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn successful_run_from_wrong_branch_is_not_reused() {
        let mut candidate = run(1, "wanted", "success");
        candidate.head_branch = "other".into();
        assert!(exact_success(vec![candidate], "main", "wanted")
            .unwrap()
            .is_none());
    }

    #[test]
    fn dispatch_occurs_only_when_exact_candidate_is_absent() {
        let mut ci = FakeCi {
            batches: VecDeque::from([Vec::new(), vec![run(3, "wanted", "failure")]]),
            dispatches: 0,
            downloads: VecDeque::new(),
        };
        let result = resolve(
            &mut ci,
            Path::new("."),
            "main",
            "wanted",
            Path::new("/tmp/unused-platform-fixture"),
            |_| Ok(()),
        );
        assert!(result.is_err());
        assert_eq!(ci.dispatches, 1);
    }

    #[test]
    fn expired_exact_candidate_dispatches_replacement() {
        let mut ci = FakeCi {
            batches: VecDeque::from([
                vec![run(2, "wanted", "success")],
                vec![run(3, "wanted", "failure")],
            ]),
            dispatches: 0,
            downloads: VecDeque::from([Err("artifact expired".into())]),
        };
        let result = resolve(
            &mut ci,
            Path::new("."),
            "main",
            "wanted",
            Path::new("target/unused"),
            |_| Ok(()),
        );
        assert!(result.is_err());
        assert_eq!(ci.dispatches, 1);
    }
}
