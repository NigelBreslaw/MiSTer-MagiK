// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const WORKFLOW: &str = "platform-bundle.yml";
const WAIT_DEADLINE: Duration = Duration::from_secs(45 * 60);
const COMMAND_DEADLINE: Duration = Duration::from_secs(5 * 60);
const POLL_DELAYS: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(3),
    Duration::from_secs(5),
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Candidate {
    pub run_id: u64,
    pub head_sha: String,
    pub archive: PathBuf,
    pub manifest: PathBuf,
    pub reused: bool,
    pub release_tag: Option<String>,
    pub head_branch: String,
    pub bundle_id: String,
    pub main_identity: String,
    pub main_revision: String,
    pub fpga_identity: String,
    pub kernel_identity: String,
}

#[derive(Debug, Deserialize)]
struct PlatformManifest {
    #[serde(default)]
    bundle_id: String,
    #[serde(default)]
    main_input_sha256: String,
    #[serde(default)]
    fpga_input_sha256: String,
    #[serde(default)]
    kernel_input_sha256: String,
    source: Option<ManifestSource>,
    magik_revision: Option<String>,
    components: Option<ManifestComponents>,
}

impl PlatformManifest {
    fn origin_sha(&self) -> Option<&str> {
        self.source
            .as_ref()
            .and_then(|source| source.magik_revision.as_deref())
            .or(self.magik_revision.as_deref())
    }

    fn main_run_id(&self) -> Option<u64> {
        self.components
            .as_ref()
            .and_then(|components| components.main.as_ref())
            .and_then(|main| main.run_id.as_ref())
            .and_then(ManifestRunId::get)
    }

    fn main_revision(&self) -> Option<&str> {
        self.components
            .as_ref()
            .and_then(|components| components.main.as_ref())
            .and_then(|main| main.head_sha.as_deref())
    }
}

#[derive(Debug, Deserialize)]
struct ManifestSource {
    magik_revision: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestComponents {
    main: Option<ManifestMain>,
}

#[derive(Debug, Deserialize)]
struct ManifestMain {
    run_id: Option<ManifestRunId>,
    head_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ManifestRunId {
    Number(u64),
    Text(String),
}

impl ManifestRunId {
    fn get(&self) -> Option<u64> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Text(value) => value.parse().ok(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubRun {
    database_id: u64,
    head_sha: String,
    head_branch: String,
    status: String,
    conclusion: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubRelease {
    tag_name: String,
    is_draft: bool,
}

#[derive(Debug, Deserialize)]
struct GithubApiRelease {
    tag_name: String,
    draft: bool,
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
    _repository: &Path,
    branch: &str,
    head_sha: &str,
    destination: &Path,
    mut progress: impl FnMut(&str) -> Result<(), String>,
) -> Result<Candidate, String> {
    let mut clock = RealPollClock {
        started: Instant::now(),
    };
    resolve_with_clock(ci, branch, head_sha, destination, &mut progress, &mut clock)
}

trait PollClock {
    fn elapsed(&self) -> Duration;
    fn sleep(&mut self, duration: Duration);
}

struct RealPollClock {
    started: Instant,
}

impl PollClock for RealPollClock {
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn sleep(&mut self, duration: Duration) {
        thread::sleep(duration);
    }
}

fn resolve_with_clock(
    ci: &mut dyn PlatformCi,
    branch: &str,
    head_sha: &str,
    destination: &Path,
    progress: &mut impl FnMut(&str) -> Result<(), String>,
    clock: &mut dyn PollClock,
) -> Result<Candidate, String> {
    if let Some(run) = exact_success(ci.runs(head_sha)?, branch, head_sha)? {
        progress("reusing exact verified platform candidate")?;
        match download_and_verify(ci, run.id, head_sha, &run.head_branch, destination, true) {
            Ok(candidate) => return Ok(candidate),
            Err(_) => progress("exact platform candidate is unavailable; dispatching replacement")?,
        }
    }
    progress("dispatching platform candidate workflow")?;
    ci.dispatch(branch)?;
    let mut previous = None;
    let mut delay_index = 0;
    while clock.elapsed() < WAIT_DEADLINE {
        let runs = ci.runs(head_sha)?;
        if let Some(run) = exact_success(runs.clone(), branch, head_sha)? {
            progress("platform candidate workflow completed")?;
            return download_and_verify(ci, run.id, head_sha, &run.head_branch, destination, false);
        }
        if runs.iter().any(|run| {
            run.head_sha == head_sha && run.status == "completed" && run.conclusion != "success"
        }) {
            return Err("platform CI workflow failed for the requested commit".into());
        }
        progress("waiting for platform candidate workflow")?;
        let state = runs
            .iter()
            .filter(|run| run.head_sha == head_sha && run.workflow == WORKFLOW)
            .map(|run| (run.id, run.status.clone(), run.conclusion.clone()))
            .collect::<Vec<_>>();
        if previous.as_ref() != Some(&state) {
            delay_index = 0;
        }
        let remaining = WAIT_DEADLINE.saturating_sub(clock.elapsed());
        clock.sleep(POLL_DELAYS[delay_index].min(remaining));
        previous = Some(state);
        delay_index = (delay_index + 1).min(POLL_DELAYS.len() - 1);
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
    let payload = verify_manifest(&archive, &manifest, "platform candidate", false)?;
    if payload.origin_sha().is_some_and(|sha| sha != head_sha) {
        return Err("platform candidate manifest does not match the requested commit".into());
    }
    let main_revision = required_manifest_value(
        payload.main_revision().unwrap_or_default().to_owned(),
        "components.main.head_sha",
        "platform candidate",
    )?;
    Ok(Candidate {
        run_id,
        head_sha: head_sha.into(),
        archive,
        manifest,
        reused,
        release_tag: None,
        head_branch: head_branch.into(),
        bundle_id: required_manifest_value(payload.bundle_id, "bundle_id", "platform candidate")?,
        main_identity: required_manifest_value(
            payload.main_input_sha256,
            "main_input_sha256",
            "platform candidate",
        )?,
        main_revision,
        fpga_identity: required_manifest_value(
            payload.fpga_input_sha256,
            "fpga_input_sha256",
            "platform candidate",
        )?,
        kernel_identity: required_manifest_value(
            payload.kernel_input_sha256,
            "kernel_input_sha256",
            "platform candidate",
        )?,
    })
}

fn verify_manifest(
    archive: &Path,
    manifest: &Path,
    label: &str,
    historical_baseline: bool,
) -> Result<PlatformManifest, String> {
    let verification = if historical_baseline {
        crate::platform_bundle::verify_historical_baseline(archive, Some(manifest), None)
    } else {
        crate::platform_bundle::verify(archive, Some(manifest), None)
    };
    verification.map_err(|error| format!("{label} failed verification: {error}"))?;
    serde_json::from_slice(
        &std::fs::read(manifest)
            .map_err(|error| format!("cannot read {label} manifest: {error}"))?,
    )
    .map_err(|error| format!("invalid {label} manifest: {error}"))
}

fn required_manifest_value(value: String, name: &str, label: &str) -> Result<String, String> {
    if value.is_empty() {
        Err(format!("{label} manifest is missing {name}"))
    } else {
        Ok(value)
    }
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

pub fn resolve_published_repository(
    repository: &Path,
    mut progress: impl FnMut(&str) -> Result<(), String>,
) -> Result<Candidate, String> {
    let owner = std::env::var("MISTER_MAGIK_GITHUB_REPOSITORY")
        .unwrap_or_else(|_| "NigelBreslaw/MiSTer-MagiK".into());
    let branch = command_text(repository, "git", &["branch", "--show-current"])?;
    let head_sha = command_text(repository, "git", &["rev-parse", "HEAD"])?;
    progress("checking latest qualified platform release")?;
    let rows = published_releases(repository, &owner)?;
    let tag = latest_platform_release(&rows)
        .ok_or("no published numbered platform release is available")?;
    let cache_root = repository.join("build/agent-cache/release-cache/platform");
    let destination = cache_root.join(&tag);
    if let Ok(candidate) =
        published_candidate(repository, &destination, &branch, &head_sha, &tag, true)
    {
        progress("reusing cached qualified platform components")?;
        return Ok(candidate);
    }
    if destination.exists() {
        std::fs::remove_dir_all(&destination)
            .map_err(|error| format!("cannot clear invalid published platform cache: {error}"))?;
    }
    std::fs::create_dir_all(&cache_root)
        .map_err(|error| format!("cannot create published platform cache: {error}"))?;
    let temporary = cache_root.join(format!(".{tag}.download-{}", std::process::id()));
    if temporary.exists() {
        std::fs::remove_dir_all(&temporary)
            .map_err(|error| format!("cannot clear temporary platform download: {error}"))?;
    }
    std::fs::create_dir_all(&temporary)
        .map_err(|error| format!("cannot create temporary platform download: {error}"))?;
    progress("downloading latest qualified platform components")?;
    let mut download = Command::new("gh");
    download
        .args(["release", "download", &tag, "--repo", &owner, "--dir"])
        .arg(&temporary)
        .args([
            "--pattern",
            "mister-magik-platform-v0.*.zip",
            "--pattern",
            "platform-bundle-v0.2.json",
        ])
        .current_dir(repository);
    let output = match bounded_output(download, "published platform download", COMMAND_DEADLINE) {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&temporary);
            return Err(error);
        }
    };
    if !output.status.success() {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let mut candidate =
        match published_candidate(repository, &temporary, &branch, &head_sha, &tag, false) {
            Ok(candidate) => candidate,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&temporary);
                return Err(error);
            }
        };
    if let Err(error) = std::fs::rename(&temporary, &destination) {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(format!("cannot publish platform cache: {error}"));
    }
    candidate.archive = destination.join(
        candidate
            .archive
            .file_name()
            .ok_or("downloaded platform archive has no filename")?,
    );
    candidate.manifest = destination.join(
        candidate
            .manifest
            .file_name()
            .ok_or("downloaded platform manifest has no filename")?,
    );
    progress("qualified platform components ready for exact runtime manifest")?;
    Ok(candidate)
}

pub fn latest_game_database_release(repository: &Path) -> Result<(String, String, u64), String> {
    let owner = std::env::var("MISTER_MAGIK_GITHUB_REPOSITORY")
        .unwrap_or_else(|_| "NigelBreslaw/MiSTer-MagiK".into());
    let rows = published_releases(repository, &owner)?;
    latest_game_database_release_from(&rows)
        .map(|(tag, version)| (owner, tag, version))
        .ok_or_else(|| "no published numbered game-database release is available".into())
}

fn published_releases(repository: &Path, owner: &str) -> Result<Vec<GithubRelease>, String> {
    let endpoint = format!("repos/{owner}/releases?per_page=100");
    let releases = command_text(
        repository,
        "gh",
        &["api", "--paginate", "--slurp", &endpoint],
    )?;
    let pages: Vec<Vec<GithubApiRelease>> = serde_json::from_str(&releases)
        .map_err(|error| format!("invalid GitHub release response: {error}"))?;
    Ok(pages
        .into_iter()
        .flatten()
        .map(|release| GithubRelease {
            tag_name: release.tag_name,
            is_draft: release.draft,
        })
        .collect())
}

fn published_candidate(
    _repository: &Path,
    destination: &Path,
    branch: &str,
    head_sha: &str,
    tag: &str,
    reused: bool,
) -> Result<Candidate, String> {
    let archive = find_named(destination, "mister-magik-platform-", ".zip")?;
    let manifest = find_exact(destination, "platform-bundle-v0.2.json")?;
    let payload = verify_manifest(&archive, &manifest, "published platform", true)?;
    let run_id = payload
        .main_run_id()
        .ok_or("published platform manifest is missing Main run_id")?;
    let main_revision = required_manifest_value(
        payload.main_revision().unwrap_or_default().to_owned(),
        "components.main.head_sha",
        "published platform",
    )?;
    Ok(Candidate {
        run_id,
        head_sha: head_sha.into(),
        archive,
        manifest,
        reused,
        release_tag: Some(tag.into()),
        head_branch: branch.into(),
        bundle_id: required_manifest_value(payload.bundle_id, "bundle_id", "published platform")?,
        main_identity: required_manifest_value(
            payload.main_input_sha256,
            "main_input_sha256",
            "published platform",
        )?,
        main_revision,
        fpga_identity: required_manifest_value(
            payload.fpga_input_sha256,
            "fpga_input_sha256",
            "published platform",
        )?,
        kernel_identity: required_manifest_value(
            payload.kernel_input_sha256,
            "kernel_input_sha256",
            "published platform",
        )?,
    })
}

fn latest_platform_release(rows: &[GithubRelease]) -> Option<String> {
    rows.iter()
        .filter(|row| !row.is_draft)
        .filter_map(|row| {
            let tag = &row.tag_name;
            let version = tag.strip_prefix("platform-v0.")?.parse::<u64>().ok()?;
            (version > 0).then(|| (version, tag.clone()))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, tag)| tag)
}

fn latest_game_database_release_from(rows: &[GithubRelease]) -> Option<(String, u64)> {
    rows.iter()
        .filter(|row| !row.is_draft)
        .filter_map(|row| {
            let version = row
                .tag_name
                .strip_prefix("game-databases-v")?
                .parse::<u64>()
                .ok()?;
            (version > 0).then(|| (row.tag_name.clone(), version))
        })
        .max_by_key(|(_, version)| *version)
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
        let rows: Vec<GithubRun> = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("invalid GitHub run response: {error}"))?;
        Ok(rows
            .into_iter()
            .map(|row| Run {
                id: row.database_id,
                head_sha: row.head_sha,
                status: row.status,
                conclusion: row.conclusion,
                workflow: WORKFLOW.into(),
                head_branch: row.head_branch,
            })
            .collect())
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
    let status = crate::process::wait(&mut child, Some(deadline), label, None, || Ok(()))?;
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

    #[derive(Default)]
    struct FakeClock {
        elapsed: Duration,
        sleeps: Vec<Duration>,
    }

    impl PollClock for FakeClock {
        fn elapsed(&self) -> Duration {
            self.elapsed
        }

        fn sleep(&mut self, duration: Duration) {
            self.sleeps.push(duration);
            self.elapsed += duration;
        }
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

    fn pending_run(id: u64, status: &str) -> Run {
        Run {
            id,
            head_sha: "wanted".into(),
            status: status.into(),
            conclusion: String::new(),
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
        assert!(
            exact_success(
                vec![run(1, "stale", "success"), run(2, "wanted", "failure")],
                "main",
                "wanted"
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn successful_run_from_wrong_branch_is_not_reused() {
        let mut candidate = run(1, "wanted", "success");
        candidate.head_branch = "other".into();
        assert!(
            exact_success(vec![candidate], "main", "wanted")
                .unwrap()
                .is_none()
        );
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
    fn polling_backs_off_and_resets_when_workflow_state_changes() {
        let mut ci = FakeCi {
            batches: VecDeque::from([
                Vec::new(),
                vec![pending_run(3, "queued")],
                vec![pending_run(3, "queued")],
                vec![pending_run(3, "in_progress")],
                vec![run(3, "wanted", "failure")],
            ]),
            dispatches: 0,
            downloads: VecDeque::new(),
        };
        let mut clock = FakeClock::default();
        let result = resolve_with_clock(
            &mut ci,
            "main",
            "wanted",
            Path::new("/tmp/unused-platform-fixture"),
            &mut |_| Ok(()),
            &mut clock,
        );
        assert!(result.is_err());
        assert_eq!(
            clock.sleeps,
            [
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(1)
            ]
        );
        assert_eq!(ci.dispatches, 1);
    }

    #[test]
    fn latest_published_platform_accepts_prereleases_and_ignores_drafts() {
        let rows: Vec<GithubRelease> = serde_json::from_value(serde_json::json!([
            serde_json::json!({"tagName":"platform-v0.7","isDraft":false,"isPrerelease":false}),
            serde_json::json!({"tagName":"platform-v0.9","isDraft":true,"isPrerelease":false}),
            serde_json::json!({"tagName":"platform-v0.1-deadbeef","isDraft":false,"isPrerelease":false}),
            serde_json::json!({"tagName":"platform-v0.8","isDraft":false,"isPrerelease":true}),
        ]))
        .unwrap();
        assert_eq!(
            latest_platform_release(&rows).as_deref(),
            Some("platform-v0.8")
        );
    }

    #[test]
    fn latest_game_database_release_is_numeric_and_ignores_drafts() {
        let rows: Vec<GithubRelease> = serde_json::from_value(serde_json::json!([
            {"tagName":"game-databases-v9","isDraft":false},
            {"tagName":"game-databases-v10","isDraft":false},
            {"tagName":"game-databases-v99","isDraft":true},
            {"tagName":"game-databases-v10-invalid","isDraft":false},
            {"tagName":"platform-v0.8","isDraft":false}
        ]))
        .unwrap();
        assert_eq!(
            latest_game_database_release_from(&rows),
            Some(("game-databases-v10".into(), 10))
        );
    }

    #[test]
    fn published_platform_accepts_canonical_string_run_ids() {
        let string_id: ManifestRunId =
            serde_json::from_value(serde_json::json!("29856409043")).unwrap();
        let number_id: ManifestRunId = serde_json::from_value(serde_json::json!(42)).unwrap();
        let invalid_id: ManifestRunId =
            serde_json::from_value(serde_json::json!("invalid")).unwrap();
        assert_eq!(string_id.get(), Some(29_856_409_043));
        assert_eq!(number_id.get(), Some(42));
        assert_eq!(invalid_id.get(), None);
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
