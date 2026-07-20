// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use agent_cli::cli::Cli;
use agent_cli::evidence::Evidence;
use agent_cli::model::{Intent, Outcome};
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
    let intent = cli.into_intent();
    evidence
        .record_intent(&raw.id, &intent)
        .unwrap_or_else(|error| fatal(&error));
    let outcome = dispatch(&evidence, &intent).unwrap_or_else(|error| fatal(&error));
    evidence
        .finish(&raw.id, outcome)
        .unwrap_or_else(|error| fatal(&error));
}

fn dispatch(evidence: &Evidence, intent: &Intent) -> Result<Outcome, String> {
    match intent {
        Intent::DatabaseStatus => {
            println!(
                "{}",
                serde_json::to_string_pretty(&evidence.status()?).unwrap()
            );
        }
        Intent::ListRuns { failed, recent } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&evidence.recent_runs(*failed, *recent)?).unwrap()
            );
        }
        Intent::ShowRun { run_id } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&evidence.run_detail(run_id)?).unwrap()
            );
        }
        Intent::PruneLogs => println!("removed {} captured logs", evidence.prune_logs()?),
        other => println!("request accepted: {other:?}"),
    }
    Ok(Outcome::NoOp)
}

fn fatal(message: &str) -> ! {
    eprintln!("agent-cli: {message}");
    std::process::exit(70);
}
