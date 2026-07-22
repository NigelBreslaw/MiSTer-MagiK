// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use agent_cli::cli::{Cli, OutputFormat};
use agent_cli::evidence::Evidence;
use agent_cli::executor;
use agent_cli::model::{Intent, Outcome};
use agent_cli::planner;
use agent_cli::progress::{EventKind, Reporter};
use agent_cli::request::RawRequest;
use agent_cli::scope;
use clap::Parser;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if is_discovery_request(&args) {
        let _ = Cli::parse_from(args);
        return;
    }
    let raw = RawRequest::capture(args.clone());
    let repository = std::env::current_dir().unwrap_or_else(|error| fatal(&error.to_string()));
    let evidence = Evidence::open_for_repository(&repository).unwrap_or_else(|error| fatal(&error));
    evidence
        .begin_request(&raw)
        .unwrap_or_else(|error| fatal(&error));
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            evidence
                .reject_parse(&raw.id, &error.to_string())
                .unwrap_or_else(|audit_error| fatal(&audit_error));
            eprint!("{error}");
            std::process::exit(2);
        }
    };
    let output = cli.output;
    let intent = resolve_task_intent(&evidence, &repository, cli.into_intent())
        .unwrap_or_else(|error| fatal(&error));
    evidence
        .record_intent(&raw.id, &intent)
        .unwrap_or_else(|error| fatal(&error));
    let mut reporter = Reporter::new(&evidence, output, &raw.id);
    reporter
        .emit(EventKind::Started, "request", "Accepted request", None)
        .unwrap_or_else(|error| fatal(&error));
    let outcome = match dispatch(
        &evidence,
        &raw.id,
        &repository,
        &intent,
        output,
        &mut reporter,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            let (phase, message) = error
                .split_once(": ")
                .filter(|(phase, _)| matches!(*phase, "check" | "verify"))
                .unwrap_or(("request", error.as_str()));
            reporter
                .emit(EventKind::Failed, phase, message, None)
                .unwrap_or_else(|audit_error| fatal(&audit_error));
            evidence
                .finish(&raw.id, Outcome::Failed)
                .unwrap_or_else(|audit_error| fatal(&audit_error));
            std::process::exit(1);
        }
    };
    reporter
        .emit(
            EventKind::Completed,
            "request",
            "Request complete",
            Some(100),
        )
        .unwrap_or_else(|error| fatal(&error));
    evidence
        .finish(&raw.id, outcome)
        .unwrap_or_else(|error| fatal(&error));
    if outcome == Outcome::ExternalRequired {
        std::process::exit(3);
    }
}

fn is_discovery_request(args: &[std::ffi::OsString]) -> bool {
    args.len() == 1
        || matches!(
            args.last().and_then(|arg| arg.to_str()),
            Some("-h" | "--help")
        )
        || (args.len() == 2 && matches!(args[1].to_str(), Some("-V" | "--version")))
}

fn resolve_task_intent(
    evidence: &Evidence,
    repository: &std::path::Path,
    intent: Intent,
) -> Result<Intent, String> {
    let resolve = |task_id: String| -> Result<String, String> {
        if !task_id.is_empty() {
            return Ok(task_id);
        }
        evidence.active_manual_task_id(repository)?.ok_or_else(|| {
            "No task baseline exists. Run `scripts/agent task begin` before editing.".into()
        })
    };
    Ok(match intent {
        Intent::TaskStatus { task_id } => Intent::TaskStatus {
            task_id: resolve(task_id)?,
        },
        Intent::Commit { task_id, message } => Intent::Commit {
            task_id: if task_id.is_empty() {
                evidence
                    .active_manual_task_id(repository)?
                    .unwrap_or_default()
            } else {
                task_id
            },
            message,
        },
        Intent::Deliver { task_id } => Intent::Deliver {
            task_id: if task_id.is_empty() {
                evidence
                    .latest_committed_task(repository)?
                    .map(|(task_id, _)| task_id)
                    .ok_or("nothing_to_deliver: commit the verified task first")?
            } else {
                task_id
            },
        },
        Intent::Benchmark { task_id } => Intent::Benchmark {
            task_id: if task_id.is_empty() {
                evidence
                    .latest_committed_task(repository)?
                    .map(|(task_id, _)| task_id)
                    .ok_or("nothing_to_benchmark: commit the verified task first")?
            } else {
                task_id
            },
        },
        Intent::Plan {
            scope: agent_cli::model::Scope::Task(task_id),
            verbose,
        } => Intent::Plan {
            scope: agent_cli::model::Scope::Task(resolve(task_id)?),
            verbose,
        },
        Intent::Check {
            scope: agent_cli::model::Scope::Task(task_id),
        } => Intent::Check {
            scope: agent_cli::model::Scope::Task(resolve(task_id)?),
        },
        Intent::Verify {
            scope: agent_cli::model::Scope::Task(task_id),
        } => Intent::Verify {
            scope: agent_cli::model::Scope::Task(resolve(task_id)?),
        },
        other => other,
    })
}

fn dispatch(
    evidence: &Evidence,
    request_id: &str,
    repository: &std::path::Path,
    intent: &Intent,
    output: OutputFormat,
    reporter: &mut Reporter<'_>,
) -> Result<Outcome, String> {
    match intent {
        Intent::TaskBegin { task_id, replace } => {
            agent_cli::task::begin(evidence, repository, task_id, *replace)?;
            if output == OutputFormat::Human {
                println!("task: baseline recorded ({task_id})");
            }
        }
        Intent::TaskStatus { task_id } => {
            let paths = agent_cli::task::status(evidence, repository, task_id)?;
            if output == OutputFormat::Human {
                println!(
                    "task: {} changed path{}",
                    paths.len(),
                    if paths.len() == 1 { "" } else { "s" }
                );
            }
        }
        Intent::Commit { task_id, message } => {
            let (outcome, sha, subject, paths) = agent_cli::commit::run(
                evidence, request_id, repository, task_id, message, reporter,
            )?;
            if output == OutputFormat::Human {
                println!(
                    "commit: {} — {}\npaths: {}",
                    sha,
                    subject,
                    paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            return Ok(outcome);
        }
        Intent::Deliver { task_id } => {
            return deliver(evidence, repository, task_id, reporter);
        }
        Intent::Benchmark { task_id } => {
            let (recorded_task, sha) = evidence
                .latest_committed_task(repository)?
                .filter(|(recorded, _)| recorded == task_id)
                .ok_or(
                    "unverified_commit: use `scripts/agent commit -m MESSAGE` before benchmarking",
                )?;
            debug_assert_eq!(recorded_task, *task_id);
            let paths = agent_cli::task::changes(evidence, repository, task_id)?;
            return agent_cli::benchmark::execute(repository, &paths, &sha, reporter);
        }
        Intent::ReleaseQualify => {
            return agent_cli::release::execute(reporter);
        }
        Intent::Diagnose => {
            return agent_cli::diagnose::execute(repository, reporter);
        }
        Intent::DisplayMode { video_mode, main } => {
            let mut device = agent_cli::device::DeviceClient::default();
            if let Some(main) = main {
                let selection = match main.as_str() {
                    "stock" => mister_tool::transport::MainSelection::Stock,
                    "dev" => mister_tool::transport::MainSelection::Development,
                    _ => return Err(format!("unsupported focused Main selection: {main}")),
                };
                device.execute(mister_tool::transport::DeviceRequest::SelectMain(selection))?;
            }
            device.execute(mister_tool::transport::DeviceRequest::SetMenuVideoMode {
                video_mode: video_mode.clone(),
            })?;
            return Ok(Outcome::Passed);
        }
        Intent::Build { intent } => {
            let spec = agent_cli::build::BuildSpec::for_recipe((*intent).into());
            agent_cli::build::execute(repository, &spec, reporter)?;
        }
        Intent::Plan {
            scope: selected, ..
        }
        | Intent::Check { scope: selected }
        | Intent::Verify { scope: selected } => {
            let paths = scope::collect(evidence, request_id, repository, selected)?;
            let claimed_paths = paths.clone();
            let plan = planner::affected_plan_at(repository, intent.clone(), paths)?;
            evidence.record_plan(request_id, &plan)?;
            let summary = if plan.operations.is_empty() {
                "No lint operations selected".to_owned()
            } else {
                format!("{} checks planned", plan.operations.len())
            };
            let phase = if matches!(intent, Intent::Verify { .. }) {
                "verify"
            } else if matches!(intent, Intent::Check { .. }) {
                "check"
            } else {
                "plan"
            };
            reporter.emit(EventKind::Progress, phase, &summary, Some(0))?;
            if matches!(intent, Intent::Plan { .. }) {
                if output == OutputFormat::Human
                    && matches!(intent, Intent::Plan { verbose: true, .. })
                {
                    for operation in &plan.operations {
                        println!(
                            "{}\t{} {}",
                            operation.id,
                            operation.program,
                            operation.args.join(" ")
                        );
                        println!("  reason: {}", operation.reason);
                    }
                }
                if !plan.external_requirements.is_empty() {
                    for requirement in &plan.external_requirements {
                        reporter.emit(
                            EventKind::Warning,
                            "external",
                            &requirement.message,
                            None,
                        )?;
                    }
                    return Ok(Outcome::ExternalRequired);
                }
                return Ok(if plan.operations.is_empty() {
                    Outcome::NoOp
                } else {
                    Outcome::Passed
                });
            }
            let outcome = executor::execute_with_changes(
                evidence,
                request_id,
                repository,
                &plan,
                &claimed_paths,
                reporter,
            )?;
            if let agent_cli::model::Scope::Task(task_id) = selected {
                evidence.claim_task_paths(task_id, &claimed_paths)?;
            }
            if !plan.external_requirements.is_empty() {
                for requirement in &plan.external_requirements {
                    reporter.emit(EventKind::Warning, "external", &requirement.message, None)?;
                }
                return Ok(Outcome::ExternalRequired);
            }
            return Ok(outcome);
        }
        Intent::Doctor => {
            let plan = planner::workflow_plan(intent.clone());
            evidence.record_plan(request_id, &plan)?;
            reporter.emit(
                EventKind::Progress,
                "plan",
                &format!("Selected {} operation", plan.operations.len()),
                Some(0),
            )?;
            return executor::execute(evidence, request_id, repository, &plan, reporter);
        }
        Intent::DatabaseStatus => {
            let status = evidence.status()?;
            if output == OutputFormat::Human {
                println!("{}", serde_json::to_string_pretty(&status).unwrap());
            }
        }
        Intent::ListRuns { failed, recent } => {
            let runs = evidence.recent_runs(*failed, *recent)?;
            if output == OutputFormat::Human {
                println!("{}", serde_json::to_string_pretty(&runs).unwrap());
            }
        }
        Intent::ShowRun { run_id } => {
            let detail = evidence.run_detail(run_id)?;
            if output == OutputFormat::Human {
                println!("{}", serde_json::to_string_pretty(&detail).unwrap());
            }
        }
        Intent::PruneLogs => {
            let removed = evidence.prune_logs()?;
            if output == OutputFormat::Human {
                println!("removed {removed} captured logs");
            }
        }
    }
    Ok(Outcome::NoOp)
}

#[allow(clippy::too_many_arguments)]
fn deliver(
    evidence: &Evidence,
    repository: &std::path::Path,
    task_id: &str,
    reporter: &mut Reporter<'_>,
) -> Result<Outcome, String> {
    use agent_cli::components::{self, DeploymentImpact};
    use agent_cli::evidence::DeliveryRecord;

    if let Some(mut pending) = evidence.delivery(task_id)? {
        if pending.state == "complete" {
            return Ok(Outcome::NoOp);
        }
        if !delivery_state_can_resume(&pending.state) {
            return Err(format!(
                "delivery_state_invalid: cannot resume delivery in {}",
                pending.state
            ));
        }
        let mut sha = pending
            .commit_sha
            .clone()
            .ok_or("delivery_state_invalid: pending delivery has no commit")?;
        let current_head = agent_cli::task::current_head(repository)?;
        let branch = git_value(repository, &["branch", "--show-current"])?;
        if branch != "main" {
            reporter.emit(
                EventKind::Warning,
                "external",
                "publish or merge this exact commit to main, then rerun deliver",
                None,
            )?;
            return Ok(Outcome::ExternalRequired);
        }
        let paths = if current_head == sha {
            agent_cli::task::changes(evidence, repository, task_id)?
        } else {
            let latest = evidence
                .latest_committed_task(repository)?
                .filter(|(recorded, committed)| recorded == task_id && committed == &current_head)
                .ok_or("external_pending: the newer HEAD was not committed by this task")?;
            debug_assert_eq!(latest.0, task_id);
            if !git_success(
                repository,
                &["merge-base", "--is-ancestor", &sha, &current_head],
            )? {
                return Err(
                    "external_pending: the recorded candidate is not an ancestor of HEAD".into(),
                );
            }
            let paths = git_changed_paths_including(repository, &sha, &current_head)?;
            sha = current_head;
            pending.commit_sha = Some(sha.clone());
            pending.source_tree = git_value(repository, &["rev-parse", "HEAD^{tree}"])?;
            pending.detail = Some("superseded by a newer verified task commit".into());
            evidence.save_delivery(&pending)?;
            paths
        };
        let mut deployment = agent_cli::deploy::plan(repository, paths)?;
        deployment.kind = recorded_delivery_kind(deployment.kind, &pending.impact);
        if delivery_requires_platform_candidate(&pending.impact) {
            let candidate = agent_cli::platform_ci::resolve_repository(repository, |progress| {
                reporter.emit(EventKind::Progress, "platform-ci", progress, None)
            })?;
            evidence.attest_delivery(
                task_id,
                pending
                    .requirement_id
                    .as_deref()
                    .unwrap_or("github-actions.rbf-build"),
                "platform-bundle.yml",
                "main",
                &sha,
                &serde_json::to_string(&candidate).map_err(|error| error.to_string())?,
            )?;
            pending.state = "external_verified".into();
            evidence.save_delivery(&pending)?;
            deployment.platform_candidate = Some(candidate);
        }
        if deployment.kind == agent_cli::deploy::DeploymentKind::Platform
            && deployment.platform_candidate.is_none()
        {
            deployment.platform_candidate = Some(
                agent_cli::platform_ci::resolve_published_repository(repository, |progress| {
                    reporter.emit(EventKind::Progress, "platform", progress, None)
                })?,
            );
        }
        if let Err(error) = agent_cli::delivery::execute(repository, &deployment, &sha, reporter) {
            pending.state = if error.starts_with("recovery_required:") {
                "recovery_required"
            } else {
                "failed"
            }
            .into();
            pending.detail = Some(error.clone());
            evidence.save_delivery(&pending)?;
            return Err(error);
        }
        pending.state = "complete".into();
        evidence.save_delivery(&pending)?;
        return Ok(Outcome::Passed);
    }

    let dirty = git_value(repository, &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err("dirty_worktree: commit or discard changes before delivery".into());
    }
    let (recorded_task, sha) = evidence
        .latest_committed_task(repository)?
        .filter(|(recorded, _)| recorded == task_id)
        .ok_or("unverified_commit: use `scripts/agent commit -m MESSAGE` before delivery")?;
    debug_assert_eq!(recorded_task, task_id);
    if agent_cli::task::current_head(repository)? != sha {
        return Err("moved_head: check out the exact commit created for this task".into());
    }
    let paths = agent_cli::task::changes(evidence, repository, task_id)?;
    if paths.is_empty() {
        return Err("nothing_to_deliver: no task-owned changes were found".into());
    }
    let impact = paths
        .iter()
        .filter_map(|path| components::classify(path))
        .map(components::Component::deployment_impact)
        .max()
        .unwrap_or(DeploymentImpact::None);
    let external = impact == DeploymentImpact::Platform;
    let mut delivery = DeliveryRecord {
        task_id: task_id.into(),
        worktree: repository.to_path_buf(),
        source_tree: git_value(repository, &["rev-parse", "HEAD^{tree}"])?,
        commit_sha: Some(sha.clone()),
        impact: match impact {
            DeploymentImpact::None => "none",
            DeploymentImpact::Runtime => "runtime",
            DeploymentImpact::Platform => "platform",
        }
        .into(),
        state: if external {
            "external_pending"
        } else {
            "committed"
        }
        .into(),
        requirement_id: external.then(|| "github-actions.rbf-build".into()),
        detail: None,
    };
    evidence.save_delivery(&delivery)?;
    if external {
        reporter.emit(
            EventKind::Warning,
            "external",
            "publish or merge this exact commit to main, then rerun deliver",
            None,
        )?;
        return Ok(Outcome::ExternalRequired);
    }
    if impact == DeploymentImpact::Runtime {
        let mut deployment = agent_cli::deploy::plan(repository, paths)?;
        if deployment.kind == agent_cli::deploy::DeploymentKind::Platform {
            deployment.platform_candidate = Some(
                agent_cli::platform_ci::resolve_published_repository(repository, |progress| {
                    reporter.emit(EventKind::Progress, "platform", progress, None)
                })?,
            );
        }
        if let Err(error) = agent_cli::delivery::execute(repository, &deployment, &sha, reporter) {
            delivery.state = if error.starts_with("recovery_required:") {
                "recovery_required"
            } else {
                "failed"
            }
            .into();
            delivery.detail = Some(error.clone());
            evidence.save_delivery(&delivery)?;
            return Err(error);
        }
    }
    delivery.state = "complete".into();
    evidence.save_delivery(&delivery)?;
    Ok(Outcome::Passed)
}

fn delivery_state_can_resume(state: &str) -> bool {
    matches!(state, "external_pending" | "recovery_required")
}

fn delivery_requires_platform_candidate(impact: &str) -> bool {
    impact == "platform"
}

fn recorded_delivery_kind(
    inferred: agent_cli::deploy::DeploymentKind,
    recorded_impact: &str,
) -> agent_cli::deploy::DeploymentKind {
    match recorded_impact {
        "platform" => agent_cli::deploy::DeploymentKind::Platform,
        "runtime" => inferred,
        _ => inferred,
    }
}

fn git_value(repository: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

fn git_success(repository: &std::path::Path, args: &[&str]) -> Result<bool, String> {
    std::process::Command::new("git")
        .args(args)
        .current_dir(repository)
        .status()
        .map(|status| status.success())
        .map_err(|error| error.to_string())
}

fn git_changed_paths_including(
    repository: &std::path::Path,
    first_commit: &str,
    last_commit: &str,
) -> Result<Vec<std::path::PathBuf>, String> {
    let first_parent = format!("{first_commit}^");
    let output = std::process::Command::new("git")
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
        .map(|path| std::path::PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect())
}

fn fatal(message: &str) -> ! {
    eprintln!("agent-cli: {message}");
    std::process::exit(70);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn bare_invocation_is_help_discovery() {
        assert!(is_discovery_request(&["agent-cli".into()]));
        assert!(is_discovery_request(&["agent-cli".into(), "--help".into()]));
        assert!(!is_discovery_request(&["agent-cli".into(), "check".into()]));
    }

    #[test]
    fn manual_task_is_reused_by_bare_commands() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-main-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let evidence = Evidence::open_at(&root).unwrap();
        evidence
            .save_task_baseline("task-manual", Path::new("/tmp/worktree"), &(), false)
            .unwrap();
        assert_eq!(
            resolve_task_intent(
                &evidence,
                Path::new("/tmp/worktree"),
                Intent::Commit {
                    task_id: String::new(),
                    message: "message".into(),
                }
            )
            .unwrap(),
            Intent::Commit {
                task_id: "task-manual".into(),
                message: "message".into(),
            }
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attended_delivery_can_resume_after_physical_recovery() {
        assert!(delivery_state_can_resume("external_pending"));
        assert!(delivery_state_can_resume("recovery_required"));
        assert!(!delivery_state_can_resume("failed"));
        assert!(!delivery_state_can_resume("complete"));
    }

    #[test]
    fn resumed_delivery_preserves_platform_impact_and_inferred_runtime_shape() {
        use agent_cli::deploy::DeploymentKind;

        assert_eq!(
            recorded_delivery_kind(DeploymentKind::Runtime, "platform"),
            DeploymentKind::Platform
        );
        assert_eq!(
            recorded_delivery_kind(DeploymentKind::Platform, "runtime"),
            DeploymentKind::Platform
        );
        assert_eq!(
            recorded_delivery_kind(DeploymentKind::Platform, "none"),
            DeploymentKind::Platform
        );
    }

    #[test]
    fn only_platform_delivery_recovery_resolves_a_platform_candidate() {
        assert!(delivery_requires_platform_candidate("platform"));
        assert!(!delivery_requires_platform_candidate("runtime"));
        assert!(!delivery_requires_platform_candidate("none"));
    }

    #[test]
    fn superseding_delivery_keeps_original_and_follow_up_paths() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-delivery-range-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        fs::write(root.join("baseline"), "baseline\n").unwrap();
        git(&["add", "baseline"]);
        git(&["commit", "-qm", "baseline"]);
        fs::write(root.join("platform"), "platform\n").unwrap();
        git(&["add", "platform"]);
        git(&["commit", "-qm", "platform"]);
        let first = git_value(&root, &["rev-parse", "HEAD"]).unwrap();
        fs::write(root.join("follow-up"), "follow-up\n").unwrap();
        git(&["add", "follow-up"]);
        git(&["commit", "-qm", "follow-up"]);
        let last = git_value(&root, &["rev-parse", "HEAD"]).unwrap();

        assert!(git_success(&root, &["merge-base", "--is-ancestor", &first, &last]).unwrap());
        assert_eq!(
            git_changed_paths_including(&root, &first, &last).unwrap(),
            vec![
                std::path::PathBuf::from("follow-up"),
                std::path::PathBuf::from("platform")
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
