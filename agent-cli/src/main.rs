// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use agent_cli::cli::{
    AlphaCommand, CaptureCommand, Cli, Command as CliCommand, DbCommand, DeliverTarget,
    FrameEvidenceCommand, OutputFormat, ReleaseCommand, ReturnQualificationCommand, RunCommand,
};
use agent_cli::error::{AgentError, AgentResult};
use agent_cli::evidence::Evidence;
use agent_cli::model::Outcome;
use agent_cli::progress::{EventKind, Reporter};
use agent_cli::request::RawRequest;
use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

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
        Some(CliCommand::Device { command }) if !command.requires_repository() => {
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
        CliCommand::Guidance { .. } => "guidance",
        CliCommand::Run { .. } => "run",
        CliCommand::Db { .. } => "db",
        CliCommand::Diagnose => "diagnose",
        CliCommand::Device { .. } => "device",
        CliCommand::Deliver { .. } => "deliver",
        CliCommand::RestartUi => "restart-ui",
        CliCommand::Benchmark { .. } => "benchmark",
        CliCommand::Capture { .. } => "capture",
        CliCommand::Alpha { .. } => "alpha",
        CliCommand::Release { .. } => "release",
        CliCommand::CompileTime { .. } => "compile-time",
        CliCommand::LiveParticles { .. } => "live-particles",
        CliCommand::StartupParticles { .. } => "startup-particles",
        CliCommand::SceneLab { .. } => "scene-lab",
        CliCommand::Clean => "clean",
        CliCommand::Dependencies { .. } => "dependencies",
        CliCommand::Fpga { .. } => "fpga",
        CliCommand::Build { .. } => "build",
    }
}

fn dispatch(
    evidence: &Evidence,
    repository: &std::path::Path,
    command: &CliCommand,
    output: OutputFormat,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    match command {
        CliCommand::Deliver {
            target: None,
            game_databases_release_dir,
        } => {
            return deliver(
                evidence,
                repository,
                game_databases_release_dir.as_deref(),
                reporter,
            );
        }
        CliCommand::Deliver {
            target: Some(DeliverTarget::LocalMain),
            ..
        } => return deliver_local_main(repository, reporter),
        CliCommand::RestartUi => return agent_cli::delivery::restart_ui(),
        CliCommand::Benchmark {
            scenario,
            arm,
            route,
            duration_seconds,
            fresh_catalog,
        } => {
            return agent_cli::benchmark::execute(
                repository,
                *scenario,
                *arm,
                *route,
                *duration_seconds,
                *fresh_catalog,
                reporter,
            );
        }
        CliCommand::Capture {
            command:
                CaptureCommand::UsbVideo {
                    output: destination,
                    seconds,
                },
        } => {
            let artifact = match seconds {
                Some(seconds) => {
                    agent_cli::capture::execute_movie(destination.as_deref(), *seconds)?
                }
                None => agent_cli::capture::execute(destination.as_deref())?,
            };
            println!("{}", artifact.markdown_link());
            return Ok(Outcome::Passed);
        }
        CliCommand::Release {
            command: ReleaseCommand::Qualify,
        } => {
            return agent_cli::release::execute(repository, reporter);
        }
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
            return Ok(Outcome::Passed);
        }
        CliCommand::Release {
            command:
                ReleaseCommand::ReturnQualification {
                    command:
                        ReturnQualificationCommand::RecordBoard {
                            candidate,
                            layout,
                            frame_evidence,
                            output,
                            attended,
                        },
                },
        } => {
            let manifest = std::fs::read_to_string(candidate)
                .map_err(|error| format!("cannot read {}: {error}", candidate.display()))?;
            let certificate = agent_cli::return_qualification::create_board_certificate(
                &manifest,
                agent_cli::platform_manifest::parse_layout(layout)?,
                *attended,
                frame_evidence,
            )?;
            agent_cli::return_qualification::write_json(output, &certificate)?;
            println!("{}", output.display());
            return Ok(Outcome::Passed);
        }
        CliCommand::Release {
            command:
                ReleaseCommand::ReturnQualification {
                    command:
                        ReturnQualificationCommand::Aggregate {
                            candidate,
                            layout,
                            board_evidence,
                            output,
                        },
                },
        } => {
            let manifest = std::fs::read_to_string(candidate)
                .map_err(|error| format!("cannot read {}: {error}", candidate.display()))?;
            let certificate = agent_cli::return_qualification::create_aggregate_certificate(
                &manifest,
                agent_cli::platform_manifest::parse_layout(layout)?,
                board_evidence,
            )?;
            agent_cli::return_qualification::write_json(output, &certificate)?;
            println!("{}", output.display());
            return Ok(Outcome::Passed);
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
            return Ok(Outcome::Passed);
        }
        CliCommand::Alpha {
            command:
                AlphaCommand::Accept {
                    candidate,
                    output,
                    reuse_installed,
                    restore_host_mode,
                    framebuffer_only,
                },
        } => {
            let receipt = agent_cli::alpha::execute(
                candidate,
                output,
                *reuse_installed,
                *restore_host_mode,
                *framebuffer_only,
                reporter,
            )?;
            println!("{}", receipt.display());
            return Ok(Outcome::Passed);
        }
        CliCommand::Diagnose => {
            return agent_cli::diagnose::execute(repository, reporter);
        }
        CliCommand::CompileTime { command } => {
            agent_cli::compile_time::execute(repository, command, reporter)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::LiveParticles { command } => {
            agent_cli::live_particles::execute_preview(repository, command)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::StartupParticles { command } => {
            agent_cli::startup_particles::execute_preview(repository, command)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::SceneLab { command } => {
            agent_cli::startup_particles::execute_scene_preview(repository, command, reporter)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::Clean => {
            agent_cli::clean::execute(repository, reporter)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::Dependencies { command } => {
            agent_cli::dependencies::execute(repository, command, reporter)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::Fpga { command } => {
            agent_cli::fpga::execute(repository, command, reporter)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::Device {
            command: agent_cli::commands::device::DeviceCommand::LiveParticles(args),
        } => {
            agent_cli::live_particles::execute_device(repository, args, reporter)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::Device {
            command: agent_cli::commands::device::DeviceCommand::StartupParticles(args),
        } => {
            agent_cli::startup_particles::execute_device(repository, args, reporter)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::Device {
            command: agent_cli::commands::device::DeviceCommand::SceneLab(args),
        } => {
            agent_cli::startup_particles::execute_scene_device(repository, args, reporter)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::Build { intent } => {
            agent_cli::build::execute_command(repository, *intent, reporter)?;
            return Ok(Outcome::Passed);
        }
        CliCommand::Guidance { path } => {
            print!("{}", agent_cli::guidance::report(repository, path)?);
            return Ok(Outcome::Passed);
        }
        CliCommand::Db {
            command: DbCommand::Report,
        } => {
            let report = evidence.report()?;
            if output == OutputFormat::Human {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            }
        }
        CliCommand::Run {
            command: RunCommand::Show { run_id },
        } => {
            let detail = evidence.run_detail(run_id)?;
            if output == OutputFormat::Human {
                println!("{}", serde_json::to_string_pretty(&detail).unwrap());
            }
        }
        CliCommand::Device { .. } => {
            unreachable!("non-repository device commands dispatch before RepoContext")
        }
    }
    Ok(Outcome::NoOp)
}

fn deliver(
    evidence: &Evidence,
    repository: &std::path::Path,
    game_databases_release_dir: Option<&std::path::Path>,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    let total_started = Instant::now();
    let delivery = deliver_inner(evidence, repository, game_databases_release_dir, reporter);
    reporter.emit(
        EventKind::Progress,
        "cleanup",
        "cleaning transient delivery staging",
        None,
    )?;
    let cleanup_started = Instant::now();
    let cleanup = agent_cli::delivery::cleanup_workspace(repository);
    emit_delivery_timing(reporter, "cleanup", cleanup.is_ok(), cleanup_started)?;
    let result = match (delivery, cleanup) {
        (Ok(execution), Ok(())) => {
            reporter.emit(
                EventKind::Completed,
                "delivery-decision",
                execution.decision.label(),
                Some(100),
            )?;
            Ok(execution.outcome)
        }
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => {
            let _ = reporter.emit(
                EventKind::Warning,
                "cleanup",
                &format!("delivery workspace cleanup failed: {cleanup}"),
                None,
            );
            Err(error)
        }
    };
    emit_delivery_timing(reporter, "cli-total", result.is_ok(), total_started)?;
    result
}

fn deliver_inner(
    _evidence: &Evidence,
    repository: &std::path::Path,
    game_databases_release_dir: Option<&std::path::Path>,
    reporter: &mut Reporter<'_>,
) -> AgentResult<agent_cli::delivery::DeliveryExecution> {
    let preflight_started = Instant::now();
    let preflight: AgentResult<String> = (|| {
        let dirty = agent_cli::git::value(repository, &["status", "--porcelain"])?;
        if !dirty.is_empty() {
            return Err("dirty_worktree: commit or discard changes before delivery".into());
        }
        let sha = agent_cli::git::value(repository, &["rev-parse", "HEAD"])?;
        Ok(sha)
    })();
    emit_delivery_timing(reporter, "preflight", preflight.is_ok(), preflight_started)?;
    let sha = preflight?;
    agent_cli::delivery::execute(repository, &sha, game_databases_release_dir, reporter)
}

fn emit_delivery_timing(
    reporter: &mut Reporter<'_>,
    phase: &str,
    passed: bool,
    started: Instant,
) -> AgentResult<()> {
    reporter.emit(
        if passed {
            EventKind::Completed
        } else {
            EventKind::Warning
        },
        "delivery-timing",
        &format!(
            "delivery_phase_tsv\tscope=cli\tphase={phase}\tstatus={}\tseconds={:.3}",
            if passed { "passed" } else { "failed" },
            started.elapsed().as_secs_f64(),
        ),
        None,
    )?;
    Ok(())
}

fn deliver_local_main(
    repository: &std::path::Path,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    let dirty = agent_cli::git::value(repository, &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err("dirty_worktree: commit or discard changes before delivery".into());
    }
    let app_revision = agent_cli::git::value(repository, &["rev-parse", "HEAD"])?;
    let delivery = agent_cli::local_main_delivery::execute(repository, &app_revision, reporter);
    reporter.emit(
        EventKind::Progress,
        "cleanup",
        "cleaning transient local Main staging",
        None,
    )?;
    let cleanup = agent_cli::delivery::cleanup_workspace(repository);
    match (delivery, cleanup) {
        (Ok(execution), Ok(())) => {
            reporter.emit(
                EventKind::Completed,
                "delivery-decision",
                &format!(
                    "local-main app_revision={} main_revision={} main_sha256={} candidate={}",
                    execution.app_revision,
                    execution.main_revision,
                    execution.main_sha256,
                    execution.qualification_candidate_id,
                ),
                Some(100),
            )?;
            Ok(Outcome::Passed)
        }
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!(
            "local Main delivery failed ({error}); staging cleanup failed ({cleanup})"
        )
        .into()),
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
