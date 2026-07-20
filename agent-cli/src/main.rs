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
    let intent = cli.into_intent();
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
        Intent::Plan {
            scope: selected, ..
        }
        | Intent::Check { scope: selected }
        | Intent::Verify { scope: selected } => {
            let paths = scope::collect(evidence, request_id, repository, selected)?;
            let plan = planner::affected_plan(intent.clone(), paths)?;
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
            let outcome = executor::execute(evidence, request_id, repository, &plan, reporter)?;
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
        Intent::ReviewScripts => {
            if output == OutputFormat::Human {
                println!("decision\tscript\tevidence");
                for (script, evidence) in agent_cli::registry::SCRIPT_REVIEW {
                    println!("review\tscripts/{script}\t{evidence}");
                }
            }
        }
        other if output == OutputFormat::Human => println!("request accepted: {other:?}"),
        _ => {}
    }
    Ok(Outcome::NoOp)
}

fn fatal(message: &str) -> ! {
    eprintln!("agent-cli: {message}");
    std::process::exit(70);
}
