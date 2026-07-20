// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use agent_cli::cli::{Cli, OutputFormat};
use agent_cli::evidence::Evidence;
use agent_cli::model::{Intent, Outcome};
use agent_cli::progress::{EventKind, Reporter};
use agent_cli::request::RawRequest;
use clap::Parser;

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
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
    let mut reporter = Reporter::new(&evidence, output, &raw.id);
    reporter
        .emit(EventKind::Started, "request", "Accepted request", None)
        .unwrap_or_else(|error| fatal(&error));
    let outcome = dispatch(&evidence, &intent, output).unwrap_or_else(|error| {
        reporter
            .emit(EventKind::Failed, "request", &error, None)
            .unwrap_or_else(|audit_error| fatal(&audit_error));
        fatal(&error)
    });
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
}

fn dispatch(evidence: &Evidence, intent: &Intent, output: OutputFormat) -> Result<Outcome, String> {
    match intent {
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

fn fatal(message: &str) -> ! {
    eprintln!("agent-cli: {message}");
    std::process::exit(70);
}
