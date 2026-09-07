// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use agent_cli::cli::{
    Cli, Command as CliCommand, FrameEvidenceCommand, OutputFormat, ReleaseCommand,
    ReturnQualificationCommand,
};
use agent_cli::error::{AgentError, AgentResult};
use agent_cli::evidence::Evidence;
use agent_cli::model::Outcome;
use agent_cli::progress::{EventKind, Reporter};
use agent_cli::request::RawRequest;
use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{}", fatal_error_line(&error));
            ExitCode::from(70)
        }
    }
}

fn fatal_error_line(error: &AgentError) -> String {
    format!("agent-cli: {error}")
}

fn run() -> AgentResult<ExitCode> {
    let args: Vec<_> = std::env::args_os().collect();
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            eprint!("{error}");
            return Ok(ExitCode::from(error.exit_code() as u8));
        }
    };
    let output = cli.output_format;
    let command = match cli.command {
        Some(CliCommand::Device { command }) => {
            return match agent_cli::commands::device::run(command) {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(error) => {
                    eprintln!("{error}");
                    Ok(ExitCode::FAILURE)
                }
            };
        }
        Some(command) => command,
        None => unreachable!("clap requires a command"),
    };
    let raw = RawRequest::capture(std::env::args_os());
    let context = RepoContext::open()?;
    context.evidence.begin_request(&raw)?;
    context.evidence.record_intent(
        &raw.id,
        &serde_json::json!({"command": command_label(&command)}),
    )?;
    let mut reporter = Reporter::new_at(&context.evidence, output, &raw.id, raw.started);
    reporter.emit(EventKind::Started, "request", "Accepted request", None)?;
    let outcome = match dispatch(
        &context.evidence,
        &context.repository,
        &command,
        output,
        &mut reporter,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            reporter.emit_failure("request", &error)?;
            context.evidence.finish(&raw.id, Outcome::Failed)?;
            return Ok(ExitCode::FAILURE);
        }
    };
    reporter.emit(
        EventKind::Completed,
        "request",
        "Request complete",
        Some(100),
    )?;
    context.evidence.finish(&raw.id, outcome)?;
    if outcome == Outcome::ExternalRequired {
        return Ok(ExitCode::from(3));
    }
    Ok(ExitCode::SUCCESS)
}

struct RepoContext {
    repository: PathBuf,
    evidence: Evidence,
}

impl RepoContext {
    fn open() -> AgentResult<Self> {
        let repository = std::env::current_dir().map_err(|error| error.to_string())?;
        let evidence = Evidence::open_for_repository(&repository)?;
        Ok(Self {
            repository,
            evidence,
        })
    }
}

fn command_label(command: &CliCommand) -> &'static str {
    match command {
        CliCommand::Device { .. } => "device",
        CliCommand::Benchmark { .. } => "benchmark",
        CliCommand::Release { .. } => "release",
    }
}

fn dispatch(
    _evidence: &Evidence,
    repository: &std::path::Path,
    command: &CliCommand,
    _output: OutputFormat,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    match command {
        CliCommand::Benchmark { .. } => agent_cli::benchmark::execute(repository, reporter),
        CliCommand::Release {
            command:
                ReleaseCommand::FrameEvidence {
                    command: FrameEvidenceCommand::Verify { evidence },
                },
        } => {
            let verified = agent_cli::return_qualification::read_frame_evidence(evidence)?;
            println!(
                "frame-evidence=valid capture={} board={} transitions={}",
                verified.capture_id, verified.board_id, verified.transitions_observed
            );
            Ok(Outcome::Passed)
        }
        CliCommand::Release {
            command:
                ReleaseCommand::ReturnQualification {
                    command:
                        ReturnQualificationCommand::VerifyAggregate {
                            candidate,
                            layout,
                            certificate,
                        },
                },
        } => {
            let manifest = std::fs::read_to_string(candidate)
                .map_err(|error| format!("cannot read {}: {error}", candidate.display()))?;
            let verified = agent_cli::return_qualification::verify_aggregate_for_manifest(
                certificate,
                &manifest,
                agent_cli::platform_manifest::parse_layout(layout)?,
            )?;
            println!(
                "return-qualification=valid candidate={} boards={} sinks={} sink_chipsets={} transitions={}",
                verified.candidate.qualification_candidate_id,
                verified.distinct_boards,
                verified.distinct_sinks,
                verified.distinct_sink_chipsets,
                verified.total_transitions
            );
            Ok(Outcome::Passed)
        }
        CliCommand::Device { .. } => {
            unreachable!("non-repository device commands dispatch before RepoContext")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn fatal_error_first_line_remains_compatible() {
        let error = AgentError::phase(
            "install",
            AgentError::structured_device(
                "installed hash mismatch",
                mister_magik_agent_protocol::FailureMetadata {
                    code: mister_magik_agent_protocol::FailureCode::ArtifactMismatch,
                    detail: "installed hash mismatch".to_string(),
                    phase: mister_magik_agent_protocol::FailurePhase::Artifact,
                    retry_policy: mister_magik_agent_protocol::RetryPolicy::ReconcileThenRetry,
                    recovery_required: false,
                },
            ),
        );
        assert_eq!(
            fatal_error_line(&error),
            "agent-cli: install: installed hash mismatch"
        );
        assert_eq!(
            error.structured_failure().unwrap().code,
            mister_magik_agent_protocol::FailureCode::ArtifactMismatch
        );
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
        let first = agent_cli::git::value(&root, &["rev-parse", "HEAD"]).unwrap();
        fs::write(root.join("follow-up"), "follow-up\n").unwrap();
        git(&["add", "follow-up"]);
        git(&["commit", "-qm", "follow-up"]);
        let last = agent_cli::git::value(&root, &["rev-parse", "HEAD"]).unwrap();

        assert!(
            agent_cli::git::succeeds(&root, &["merge-base", "--is-ancestor", &first, &last])
                .unwrap()
        );
        assert_eq!(
            agent_cli::git::changed_paths_including(&root, &first, &last).unwrap(),
            vec![
                std::path::PathBuf::from("follow-up"),
                std::path::PathBuf::from("platform")
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
