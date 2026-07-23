// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::evidence::{now_ms, Evidence};
use crate::model::{Intent, Outcome, Scope};
use crate::progress::{EventKind, Reporter};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn run(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    task_id: &str,
    message: &str,
    reporter: &mut Reporter<'_>,
) -> Result<(Outcome, String, String, Vec<PathBuf>), String> {
    run_with_external_policy(
        evidence, request_id, repository, task_id, message, reporter, true,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_with_external_policy(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    task_id: &str,
    message: &str,
    reporter: &mut Reporter<'_>,
    allow_external: bool,
) -> Result<(Outcome, String, String, Vec<PathBuf>), String> {
    evidence.begin_commit_attempt(request_id, task_id, message)?;
    let result = run_inner(
        evidence,
        request_id,
        repository,
        task_id,
        message,
        reporter,
        allow_external,
    );
    match &result {
        Ok((_, sha, subject, paths)) => {
            evidence.update_commit_attempt(
                request_id,
                paths,
                "committed",
                Some(sha),
                Some(subject),
                None,
            )?;
        }
        Err(error) => {
            let paths = crate::task::changes(evidence, repository, task_id).unwrap_or_default();
            evidence.update_commit_attempt(
                request_id,
                &paths,
                "failed",
                None,
                None,
                Some(error),
            )?;
        }
    }
    result
}

fn run_inner(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    task_id: &str,
    message: &str,
    reporter: &mut Reporter<'_>,
    allow_external: bool,
) -> Result<(Outcome, String, String, Vec<PathBuf>), String> {
    if task_id.is_empty() {
        return Err("task_baseline_missing: run `scripts/agent task begin` before editing".into());
    }
    if message.trim().is_empty() {
        return Err("commit_failed: commit message must not be empty".into());
    }
    let baseline = crate::task::load(evidence, repository, task_id)?;
    if baseline.planner_schema < 3
        && !crate::task::legacy_baseline_was_clean(repository, &baseline)?
    {
        return Err(
            "commit_scope_ambiguous: legacy task baseline contained pre-existing changes and cannot prove ownership"
                .into(),
        );
    }
    let paths = crate::task::changes(evidence, repository, task_id)?;
    let baseline = crate::task::load(evidence, repository, task_id)?;
    let head = crate::task::current_head(repository)?;
    if baseline.head != head {
        return Err(format!(
            "baseline_head_changed: task began at {}, current HEAD is {head}",
            baseline.head
        ));
    }
    if !baseline.staged_paths.is_empty() || has_staged_changes(repository)? {
        return Err("staged_changes_present: commit requires an untouched Git index".into());
    }
    if paths.is_empty() {
        return Err("nothing_to_commit: no task-owned changes were found".into());
    }
    let overlapping_baseline: Vec<_> = paths
        .iter()
        .filter(|path| baseline.dirty_paths.contains(*path))
        .cloned()
        .collect();
    if !overlapping_baseline.is_empty() {
        return Err(format!(
            "commit_scope_ambiguous: task changed paths that were dirty at baseline: {}",
            display_paths(&overlapping_baseline)
        ));
    }
    reject_forbidden_paths(repository, &paths)?;
    let claims: BTreeSet<_> = evidence.task_claims(task_id)?.into_iter().collect();
    let unclaimed: Vec<_> = paths
        .iter()
        .filter(|path| !claims.contains(*path))
        .cloned()
        .collect();
    if !unclaimed.is_empty() {
        return Err(format!(
            "commit_scope_ambiguous: task paths were not claimed by successful validation: {}; run `scripts/agent verify`",
            display_paths(&unclaimed)
        ));
    }
    evidence.claim_task_paths(task_id, &paths)?;
    validate_submodules(repository, &paths)?;
    evidence.update_commit_attempt(request_id, &paths, "ownership_resolved", None, None, None)?;

    let index_path = git_index_path(repository)?;
    let original_index = fs::read(&index_path)
        .map_err(|error| classify_git_write(evidence, request_id, error.to_string()))?;
    let original_permissions = fs::metadata(&index_path)
        .map_err(|error| error.to_string())?
        .permissions();

    let stage_args = git_path_args(&["add", "-A", "--"], &paths);
    let stage = git_output(
        evidence,
        request_id,
        repository,
        "commit.stage",
        &stage_args,
    )?;
    if !stage.status.success() {
        let error = classify_git_output(evidence, request_id, &stage);
        restore_index_after_stage_failure(&index_path, &original_index, original_permissions)?;
        evidence.record_commit_rollback(request_id, &error)?;
        return Err(error);
    }
    let staged_tree = git_text(repository, &["write-tree"])?;

    reporter.emit(
        EventKind::Progress,
        "commit",
        "validating staged changes",
        Some(25),
    )?;
    let verify_result = verify_staged(evidence, request_id, repository, reporter, allow_external);
    if let Err(error) = verify_result {
        restore_index(
            repository,
            &index_path,
            &original_index,
            &staged_tree,
            original_permissions,
        )?;
        evidence.record_commit_rollback(request_id, &error)?;
        return Err(format!("verification_failed: {error}"));
    }
    if git_text(repository, &["write-tree"])? != staged_tree {
        return Err("commit_failed: Git index changed concurrently after verification".into());
    }

    reporter.emit(EventKind::Progress, "commit", "creating commit", Some(75))?;
    let commit_args = vec!["commit".into(), "-m".into(), message.into()];
    let committed = git_output(
        evidence,
        request_id,
        repository,
        "commit.create",
        &commit_args,
    )?;
    if !committed.status.success() {
        restore_index(
            repository,
            &index_path,
            &original_index,
            &staged_tree,
            original_permissions,
        )?;
        let error = classify_git_output(evidence, request_id, &committed);
        evidence.record_commit_rollback(request_id, &error)?;
        return Err(error);
    }
    let sha = git_text(repository, &["rev-parse", "HEAD"])?;
    let subject = git_text(repository, &["log", "-1", "--format=%s"])?;
    evidence.close_task(task_id, &sha)?;
    Ok((Outcome::Passed, sha, subject, paths))
}

fn verify_staged(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    reporter: &mut Reporter<'_>,
    allow_external: bool,
) -> Result<(), String> {
    let intent = Intent::Verify {
        scope: Scope::Staged,
    };
    let paths = crate::scope::collect(evidence, request_id, repository, &Scope::Staged)?;
    let plan = crate::planner::affected_plan_at(repository, intent, paths)?;
    evidence.record_plan(request_id, &plan)?;
    crate::executor::execute(evidence, request_id, repository, &plan, reporter)?;
    if plan.external_requirements.is_empty() || allow_external {
        Ok(())
    } else {
        Err(plan
            .external_requirements
            .iter()
            .map(|requirement| requirement.message.as_str())
            .collect::<Vec<_>>()
            .join("; "))
    }
}

fn reject_forbidden_paths(repository: &Path, paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        let text = path.to_string_lossy();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let private_image = text.starts_with("private/")
            && matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp"
            );
        if text.starts_with("private/test-fixtures/")
            || path
                .components()
                .any(|part| part.as_os_str() == ".wrangler")
            || name == ".env"
            || name.starts_with(".env.")
            || private_image
            || matches!(
                extension.to_ascii_lowercase().as_str(),
                "zip" | "7z" | "tgz" | "tar" | "gz" | "bz2" | "xz" | "rar" | "key" | "p12" | "pfx"
            )
            || matches!(name, "credentials" | "secrets" | "id_rsa" | "id_ed25519")
            || name.starts_with("credentials.")
            || name.starts_with("secrets.")
        {
            return Err(format!(
                "commit_scope_ambiguous: forbidden path {}",
                path.display()
            ));
        }
        let output = Command::new("git")
            .args(["check-ignore", "-q", "--"])
            .arg(path)
            .current_dir(repository)
            .output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            return Err(format!(
                "commit_scope_ambiguous: ignored path {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn validate_submodules(repository: &Path, paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        let mode = git_text(
            repository,
            &["ls-files", "-s", "--", &path.to_string_lossy()],
        )?;
        if !mode.starts_with("160000 ") {
            continue;
        }
        let submodule = repository.join(path);
        if !git_text(&submodule, &["status", "--porcelain"])?.is_empty() {
            return Err(format!(
                "commit_scope_ambiguous: submodule {} is dirty",
                path.display()
            ));
        }
        if path == Path::new("private/magik-cloud") {
            let upstream = git_text(&submodule, &["rev-parse", "@{u}"]).map_err(|_| {
                "commit_scope_ambiguous: private/magik-cloud has no upstream".to_owned()
            })?;
            let head = git_text(&submodule, &["rev-parse", "HEAD"])?;
            let status = Command::new("git")
                .args(["merge-base", "--is-ancestor", &head, &upstream])
                .current_dir(&submodule)
                .status()
                .map_err(|error| error.to_string())?;
            if !status.success() {
                return Err(
                    "commit_scope_ambiguous: private/magik-cloud must be pushed first".into(),
                );
            }
        }
    }
    Ok(())
}

fn has_staged_changes(repository: &Path) -> Result<bool, String> {
    let status = Command::new("git")
        .args(["diff", "--cached", "--quiet", "--exit-code"])
        .current_dir(repository)
        .status()
        .map_err(|error| error.to_string())?;
    Ok(!status.success())
}

fn git_index_path(repository: &Path) -> Result<PathBuf, String> {
    let path = PathBuf::from(git_text(repository, &["rev-parse", "--git-path", "index"])?);
    Ok(if path.is_absolute() {
        path
    } else {
        repository.join(path)
    })
}

fn restore_index(
    repository: &Path,
    index_path: &Path,
    original: &[u8],
    staged_tree: &str,
    permissions: fs::Permissions,
) -> Result<(), String> {
    let observed = fs::read(index_path).map_err(|error| error.to_string())?;
    if git_text(repository, &["write-tree"])? != staged_tree {
        return Err(
            "commit_failed: Git index changed concurrently; refusing to overwrite it".into(),
        );
    }
    let lock_path = index_lock_path(index_path)?;
    let mut lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| {
            "commit_failed: Git index changed concurrently; index lock is busy".to_owned()
        })?;
    let result = (|| {
        if fs::read(index_path).map_err(|error| error.to_string())? != observed {
            return Err(
                "commit_failed: Git index changed concurrently; refusing to overwrite it".into(),
            );
        }
        lock.write_all(original)
            .map_err(|error| error.to_string())?;
        lock.sync_all().map_err(|error| error.to_string())?;
        fs::set_permissions(&lock_path, permissions).map_err(|error| error.to_string())?;
        Ok(())
    })();
    drop(lock);
    if let Err(error) = result {
        let _ = fs::remove_file(&lock_path);
        return Err(error);
    }
    fs::rename(&lock_path, index_path).map_err(|error| error.to_string())
}

fn restore_index_after_stage_failure(
    index_path: &Path,
    original: &[u8],
    permissions: fs::Permissions,
) -> Result<(), String> {
    let observed = fs::read(index_path).map_err(|error| error.to_string())?;
    let lock_path = index_lock_path(index_path)?;
    let mut lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| {
            "commit_failed: Git index changed concurrently; index lock is busy".to_owned()
        })?;
    let result = (|| {
        if fs::read(index_path).map_err(|error| error.to_string())? != observed {
            return Err(
                "commit_failed: Git index changed concurrently; refusing to overwrite it".into(),
            );
        }
        lock.write_all(original)
            .map_err(|error| error.to_string())?;
        lock.sync_all().map_err(|error| error.to_string())?;
        fs::set_permissions(&lock_path, permissions).map_err(|error| error.to_string())?;
        Ok(())
    })();
    drop(lock);
    if let Err(error) = result {
        let _ = fs::remove_file(&lock_path);
        return Err(error);
    }
    fs::rename(&lock_path, index_path).map_err(|error| error.to_string())
}

fn index_lock_path(index_path: &Path) -> Result<PathBuf, String> {
    let name = index_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "commit_failed: Git index path has no filename".to_owned())?;
    Ok(index_path.with_file_name(format!("{name}.lock")))
}

fn git_output(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    operation_id: &str,
    args: &[String],
) -> Result<Output, String> {
    let started = now_ms();
    let command_id = evidence.begin_command(request_id, operation_id, "git", args, None)?;
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    evidence.finish_command(command_id, started, output.status.code().unwrap_or(1))?;
    Ok(output)
}

fn git_text(repository: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn git_path_args(prefix: &[&str], paths: &[PathBuf]) -> Vec<String> {
    prefix
        .iter()
        .map(|value| (*value).to_owned())
        .chain(paths.iter().map(|path| path.to_string_lossy().into_owned()))
        .collect()
}

fn classify_git_output(evidence: &Evidence, request_id: &str, output: &Output) -> String {
    classify_git_write(
        evidence,
        request_id,
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    )
}

fn classify_git_write(evidence: &Evidence, request_id: &str, message: String) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("permission denied")
        || lower.contains("operation not permitted")
        || lower.contains("unable to create") && lower.contains("index.lock")
    {
        format!(
            "git_write_required: {message}; next=rerun with Git write access: {}",
            retry_command(evidence, request_id).unwrap_or_else(|_| "scripts/agent commit".into())
        )
    } else {
        format!("commit_failed: {message}")
    }
}

fn retry_command(evidence: &Evidence, request_id: &str) -> Result<String, String> {
    let args = evidence.request_args(request_id)?;
    Ok(crate::shell::agent_retry_command(&args))
}

fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::OutputFormat;
    use crate::request::RawRequest;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NONCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        evidence: Evidence,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "agent-cli-commit-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("docs")).unwrap();
            git(&root, &["init", "-q"]);
            git(&root, &["config", "user.name", "Agent CLI Test"]);
            git(
                &root,
                &["config", "user.email", "agent-cli@example.invalid"],
            );
            fs::write(root.join("docs/existing.md"), "original\n").unwrap();
            git(&root, &["add", "."]);
            git(&root, &["commit", "-qm", "fixture"]);
            let evidence = Evidence::open_at(&root.join(".git/agent-state")).unwrap();
            Self { root, evidence }
        }

        fn begin(&self, task_id: &str) {
            crate::task::begin(&self.evidence, &self.root, task_id, false).unwrap();
        }

        fn commit(&self, task_id: &str) -> Result<(Outcome, String, String, Vec<PathBuf>), String> {
            self.commit_with_claim(task_id, true)
        }

        fn commit_with_claim(
            &self,
            task_id: &str,
            claim: bool,
        ) -> Result<(Outcome, String, String, Vec<PathBuf>), String> {
            if claim {
                if let Ok(paths) = crate::task::changes(&self.evidence, &self.root, task_id) {
                    self.evidence.claim_task_paths(task_id, &paths)?;
                }
            }
            let request = RawRequest::capture([
                OsString::from("agent-cli"),
                OsString::from("--task-id"),
                OsString::from(task_id),
                OsString::from("commit"),
                OsString::from("-m"),
                OsString::from("Test change"),
            ]);
            self.evidence.begin_request(&request).unwrap();
            let mut reporter = Reporter::new(&self.evidence, OutputFormat::Human, &request.id);
            run(
                &self.evidence,
                &request.id,
                &self.root,
                task_id,
                "Test change",
                &mut reporter,
            )
        }

        fn staged(&self) -> String {
            git_text(&self.root, &["diff", "--cached", "--name-only"]).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn commits_new_modified_renamed_and_deleted_task_files() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("docs/delete.md"), "delete\n").unwrap();
        fs::write(fixture.root.join("docs/rename.md"), "rename\n").unwrap();
        git(&fixture.root, &["add", "."]);
        git(&fixture.root, &["commit", "-qm", "more fixtures"]);
        fixture.begin("task-a");
        fs::write(fixture.root.join("docs/existing.md"), "modified\n").unwrap();
        fs::write(fixture.root.join("docs/new.md"), "new\n").unwrap();
        fs::rename(
            fixture.root.join("docs/rename.md"),
            fixture.root.join("docs/renamed.md"),
        )
        .unwrap();
        fs::remove_file(fixture.root.join("docs/delete.md")).unwrap();

        let (_, sha, subject, paths) = fixture.commit("task-a").unwrap();
        assert_eq!(
            sha,
            git_text(&fixture.root, &["rev-parse", "HEAD"]).unwrap()
        );
        assert!(paths.contains(&PathBuf::from("docs/new.md")));
        assert_eq!(subject, "Test change");
        assert!(git_text(&fixture.root, &["status", "--porcelain"])
            .unwrap()
            .is_empty());
        assert!(fixture
            .evidence
            .active_task_ids(&fixture.root, "none")
            .unwrap()
            .is_empty());
        let run_id = fixture.evidence.recent_runs(false, 1).unwrap()[0]
            .id
            .clone();
        let detail = fixture.evidence.run_detail(&run_id).unwrap().unwrap();
        assert!(!detail
            .events
            .iter()
            .any(|event| { event.kind == "completed" && event.phase == "commit" }));
        let attempt = detail.commit_attempt.unwrap();
        assert_eq!(attempt.status, "committed");
        assert_eq!(attempt.commit_sha.as_deref(), Some(sha.as_str()));
        assert_eq!(attempt.subject.as_deref(), Some("Test change"));
        assert!(attempt.paths.to_string().contains("docs/new.md"));
        let committed = fixture
            .evidence
            .latest_committed_scope(&fixture.root)
            .unwrap()
            .unwrap();
        assert_eq!(committed.task_id, "task-a");
        assert_eq!(committed.commit_sha, sha);
        assert_eq!(committed.paths, paths);
        assert!(fixture
            .evidence
            .active_task_ids(&fixture.root, "none")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn leaves_unrelated_baseline_changes_unstaged() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("docs/existing.md"), "user change\n").unwrap();
        fixture.begin("task-a");
        fs::write(fixture.root.join("docs/new.md"), "task change\n").unwrap();

        fixture.commit("task-a").unwrap();
        assert_eq!(
            git_text(&fixture.root, &["status", "--porcelain"]).unwrap(),
            "M docs/existing.md"
        );
        assert_eq!(
            git_text(&fixture.root, &["show", "--format=", "--name-only", "HEAD"]).unwrap(),
            "docs/new.md"
        );
    }

    #[test]
    fn reconciles_safe_head_advance_and_commits_only_task_changes() {
        let fixture = Fixture::new();
        fixture.begin("advanced");
        fs::write(fixture.root.join("docs/task.md"), "task change\n").unwrap();
        fs::write(
            fixture.root.join("docs/intervening.md"),
            "intervening change\n",
        )
        .unwrap();
        git(&fixture.root, &["add", "docs/intervening.md"]);
        git(&fixture.root, &["commit", "-qm", "intervening commit"]);

        let (_, _, _, paths) = fixture.commit("advanced").unwrap();
        assert_eq!(paths, [PathBuf::from("docs/task.md")]);
        assert_eq!(
            git_text(&fixture.root, &["show", "--format=", "--name-only", "HEAD"]).unwrap(),
            "docs/task.md"
        );
    }

    #[test]
    fn refuses_changes_to_dirty_baseline_paths() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("docs/existing.md"), "user change\n").unwrap();
        fixture.begin("dirty");
        fs::write(fixture.root.join("docs/existing.md"), "task change\n").unwrap();
        assert!(fixture
            .commit("dirty")
            .unwrap_err()
            .starts_with("commit_scope_ambiguous:"));
    }

    #[test]
    fn active_task_claims_remain_isolated() {
        let fixture = Fixture::new();
        fixture.begin("thread-a");
        fs::write(fixture.root.join("docs/existing.md"), "first\n").unwrap();
        fixture.begin("thread-b");
        fs::write(fixture.root.join("docs/existing.md"), "second\n").unwrap();
        let task_b_paths =
            crate::task::changes(&fixture.evidence, &fixture.root, "thread-b").unwrap();
        fixture
            .evidence
            .claim_task_paths("thread-b", &task_b_paths)
            .unwrap();
        let error = fixture.commit("thread-a").unwrap_err();
        assert!(error.contains("already claimed by active task"), "{error}");
    }

    #[test]
    fn refuses_missing_baseline_staged_changes_and_forbidden_files() {
        let fixture = Fixture::new();
        assert!(fixture
            .commit("missing")
            .unwrap_err()
            .starts_with("task_baseline_missing:"));

        fixture.begin("staged");
        fs::write(fixture.root.join("docs/new.md"), "new\n").unwrap();
        git(&fixture.root, &["add", "docs/new.md"]);
        assert!(fixture
            .commit("staged")
            .unwrap_err()
            .starts_with("staged_changes_present:"));

        let fixture = Fixture::new();
        fixture.begin("forbidden");
        fs::write(fixture.root.join(".env"), "SECRET=value\n").unwrap();
        assert!(fixture
            .commit("forbidden")
            .unwrap_err()
            .contains("forbidden path .env"));
    }

    #[test]
    fn verification_and_hook_failures_restore_the_index() {
        let fixture = Fixture::new();
        fixture.begin("verify-failure");
        fs::create_dir_all(fixture.root.join("docs/agents")).unwrap();
        fs::write(fixture.root.join("docs/agents/test.md"), "change\n").unwrap();
        let error = fixture.commit("verify-failure").unwrap_err();
        assert!(error.starts_with("verification_failed:"), "{error}");
        assert!(fixture.staged().is_empty());
        let run_id = fixture.evidence.recent_runs(false, 1).unwrap()[0]
            .id
            .clone();
        let detail = fixture.evidence.run_detail(&run_id).unwrap().unwrap();
        assert!(detail.commit_attempt.unwrap().rolled_back);

        let fixture = Fixture::new();
        fixture.begin("hook-failure");
        fs::write(fixture.root.join("docs/new.md"), "change\n").unwrap();
        let hook = fixture.root.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
        assert!(fixture
            .commit("hook-failure")
            .unwrap_err()
            .starts_with("commit_failed:"));
        assert!(fixture.staged().is_empty());
    }

    #[test]
    fn refuses_changes_not_claimed_by_successful_validation() {
        let fixture = Fixture::new();
        fixture.begin("unclaimed");
        fs::write(fixture.root.join("docs/new.md"), "change\n").unwrap();
        assert!(fixture
            .commit_with_claim("unclaimed", false)
            .unwrap_err()
            .contains("not claimed by successful validation"));
    }

    #[test]
    fn concurrent_index_mutation_is_detected() {
        let fixture = Fixture::new();
        let index = git_index_path(&fixture.root).unwrap();
        let original = fs::read(&index).unwrap();
        let permissions = fs::metadata(&index).unwrap().permissions();
        fs::write(fixture.root.join("docs/new.md"), "change\n").unwrap();
        fs::write(fixture.root.join("other.md"), "other\n").unwrap();
        git(&fixture.root, &["add", "docs/new.md"]);
        let staged_tree = git_text(&fixture.root, &["write-tree"]).unwrap();
        git(&fixture.root, &["add", "other.md"]);
        let error =
            restore_index(&fixture.root, &index, &original, &staged_tree, permissions).unwrap_err();
        assert!(error.contains("Git index changed concurrently"), "{error}");
    }

    #[test]
    fn failed_stage_rollback_restores_exact_original_index() {
        let fixture = Fixture::new();
        let index = git_index_path(&fixture.root).unwrap();
        let original = fs::read(&index).unwrap();
        let permissions = fs::metadata(&index).unwrap().permissions();
        fs::write(fixture.root.join("docs/new.md"), "change\n").unwrap();
        git(&fixture.root, &["add", "docs/new.md"]);
        restore_index_after_stage_failure(&index, &original, permissions).unwrap();
        assert_eq!(fs::read(&index).unwrap(), original);
        assert!(fixture.staged().is_empty());
    }

    #[test]
    fn permission_failure_contains_exact_retry_command() {
        let fixture = Fixture::new();
        let request = RawRequest::capture([
            OsString::from("agent-cli/target/debug/agent-cli"),
            OsString::from("commit"),
            OsString::from("-m"),
            OsString::from("Quoted message"),
        ]);
        fixture.evidence.begin_request(&request).unwrap();
        let error = classify_git_write(
            &fixture.evidence,
            &request.id,
            "Unable to create .git/index.lock: Operation not permitted".into(),
        );
        assert!(error.starts_with("git_write_required:"));
        assert!(error.contains("scripts/agent commit -m 'Quoted message'"));
    }

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
}
