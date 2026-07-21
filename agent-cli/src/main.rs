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
use agent_cli::tui;
use clap::Parser;
use std::io::IsTerminal;

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
    if intent == Intent::Interactive && std::io::stdout().is_terminal() {
        tui::run(&evidence, &raw.id).unwrap_or_else(|error| fatal(&error));
        evidence
            .finish(&raw.id, Outcome::Passed)
            .unwrap_or_else(|error| fatal(&error));
        return;
    }
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
    matches!(
        args.last().and_then(|arg| arg.to_str()),
        Some("-h" | "--help")
    ) || (args.len() == 2 && matches!(args[1].to_str(), Some("-V" | "--version")))
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
        Intent::Deliver { task_id, message } => Intent::Deliver {
            task_id: if task_id.is_empty() {
                evidence
                    .active_manual_task_id(repository)?
                    .unwrap_or_default()
            } else {
                task_id
            },
            message,
        },
        Intent::Deploy { task_id } => Intent::Deploy {
            task_id: resolve(task_id)?,
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
        Intent::Deliver { task_id, message } => {
            return deliver(
                evidence, request_id, repository, task_id, message, output, reporter,
            );
        }
        Intent::Deploy { task_id } => {
            let task_paths = agent_cli::task::changes(evidence, repository, task_id)?;
            let paths = agent_cli::deploy::deployment_paths(repository, task_paths)?;
            let mut deployment = agent_cli::deploy::plan(repository, paths)?;
            if deployment.kind == agent_cli::deploy::DeploymentKind::Platform {
                let candidate =
                    agent_cli::platform_ci::resolve_repository(repository, |message| {
                        reporter.emit(EventKind::Progress, "platform-ci", message, None)
                    })?;
                deployment.platform_candidate = Some(candidate);
            }
            let plan = deployment.as_evidence_plan(intent.clone());
            evidence.record_plan(request_id, &plan)?;
            reporter.emit(
                EventKind::Progress,
                "deploy-plan",
                &format!(
                    "{} deployment planned (release-device, {} UI)",
                    deployment.kind.label(),
                    deployment.ui_scope.label()
                ),
                Some(100),
            )?;
            if output == OutputFormat::Human {
                println!("{}", serde_json::to_string_pretty(&deployment).unwrap());
            }
            if deployment.kind == agent_cli::deploy::DeploymentKind::Runtime {
                return agent_cli::runtime_deploy::execute(repository, &deployment, reporter);
            }
            return agent_cli::platform_deploy::execute(repository, &deployment, reporter);
        }
        Intent::DeployRecipe { recipe } => {
            let deployment = agent_cli::deploy::recipe_plan(recipe)?;
            let plan = deployment.as_evidence_plan(intent.clone());
            evidence.record_plan(request_id, &plan)?;
            return agent_cli::runtime_deploy::execute(repository, &deployment, reporter);
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
            if !plan.external_requirements.is_empty() {
                for requirement in &plan.external_requirements {
                    reporter.emit(EventKind::Warning, "external", &requirement.message, None)?;
                }
                return Ok(Outcome::ExternalRequired);
            }
            if let agent_cli::model::Scope::Task(task_id) = selected {
                evidence.claim_task_paths(task_id, &claimed_paths)?;
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
        other if output == OutputFormat::Human => println!("request accepted: {other:?}"),
        _ => {}
    }
    Ok(Outcome::NoOp)
}

#[allow(clippy::too_many_arguments)]
fn deliver(
    evidence: &Evidence,
    request_id: &str,
    repository: &std::path::Path,
    task_id: &str,
    message: &str,
    output: OutputFormat,
    reporter: &mut Reporter<'_>,
) -> Result<Outcome, String> {
    use agent_cli::components::{self, DeploymentImpact};
    use agent_cli::evidence::DeliveryRecord;

    if let Some(mut pending) = evidence.delivery(task_id)? {
        if pending.state == "complete" {
            return Ok(Outcome::NoOp);
        }
        if pending.state != "external_pending" {
            return Err(format!(
                "delivery_state_invalid: cannot resume delivery in {}",
                pending.state
            ));
        }
        let sha = pending
            .commit_sha
            .clone()
            .ok_or("delivery_state_invalid: pending delivery has no commit")?;
        if agent_cli::task::current_head(repository)? != sha {
            return Err("external_pending: publish the recorded commit to main, check it out, and rerun `scripts/agent deliver -m ...`".into());
        }
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
        let paths = agent_cli::task::changes(evidence, repository, task_id)?;
        let mut deployment = agent_cli::deploy::plan(repository, paths)?;
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
        agent_cli::platform_deploy::execute(repository, &deployment, reporter)?;
        pending.state = "complete".into();
        evidence.save_delivery(&pending)?;
        return Ok(Outcome::Passed);
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
    let verify_intent = Intent::Verify {
        scope: agent_cli::model::Scope::Task(task_id.into()),
    };
    let plan = planner::affected_plan_at(repository, verify_intent, paths.clone())?;
    evidence.record_plan(request_id, &plan)?;
    executor::execute_with_changes(evidence, request_id, repository, &plan, &paths, reporter)?;
    evidence.claim_task_paths(task_id, &paths)?;
    let external = impact == DeploymentImpact::Platform;
    let (_, sha, subject, committed_paths) = if external {
        agent_cli::commit::run_allowing_external(
            evidence, request_id, repository, task_id, message, reporter,
        )?
    } else {
        agent_cli::commit::run(evidence, request_id, repository, task_id, message, reporter)?
    };
    let mut delivery = DeliveryRecord {
        task_id: task_id.into(),
        worktree: repository.to_path_buf(),
        source_tree: sha.clone(),
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
        let deployment = agent_cli::deploy::plan(repository, committed_paths)?;
        agent_cli::runtime_deploy::execute(repository, &deployment, reporter)?;
    }
    delivery.state = "complete".into();
    evidence.save_delivery(&delivery)?;
    if output == OutputFormat::Human {
        println!("delivery: {sha} — {subject}");
    }
    Ok(Outcome::Passed)
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
}
