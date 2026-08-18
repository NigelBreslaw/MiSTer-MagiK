// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use agent_cli::cli::{
    AlphaCommand, CaptureCommand, CiCommand, Cli, Command as CliCommand, DbCommand, DeliverTarget,
    FrameEvidenceCommand, OutputFormat, PlatformManifestCommand, ReleaseCommand,
    ReturnQualificationCommand, RunCommand,
};
use agent_cli::error::{AgentError, AgentResult};
use agent_cli::evidence::Evidence;
use agent_cli::executor;
use agent_cli::model::{AssuranceRequest, Outcome};
use agent_cli::planner;
use agent_cli::progress::{EventKind, Reporter};
use agent_cli::request::RawRequest;
use agent_cli::scope;
use clap::Parser;
use std::io::{Read, Write};
use std::path::Path;
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
        &raw.id,
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
        CliCommand::PrePush { .. } => "pre-push",
        CliCommand::Plan(_) => "plan",
        CliCommand::Architecture { .. } => "architecture",
        CliCommand::Run { .. } => "run",
        CliCommand::Db { .. } => "db",
        CliCommand::Diagnose => "diagnose",
        CliCommand::Device { .. } => "device",
        CliCommand::Deliver { .. } => "deliver",
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
        CliCommand::Ci { .. } => "ci",
    }
}

fn dispatch(
    evidence: &Evidence,
    request_id: &str,
    repository: &std::path::Path,
    command: &CliCommand,
    output: OutputFormat,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    match command {
        CliCommand::PrePush { remote } => {
            let mut updates = String::new();
            std::io::stdin()
                .read_to_string(&mut updates)
                .map_err(|error| format!("pre_push_input_failed: {error}"))?;
            let paths = agent_cli::hooks::pre_push_paths(repository, remote, &updates)?;
            if paths.is_empty() {
                reporter.emit(
                    EventKind::Progress,
                    "pre-push",
                    "No branch updates require verification",
                    Some(100),
                )?;
                return Ok(Outcome::NoOp);
            }
            let plan = planner::affected_plan_at(
                repository,
                AssuranceRequest::PrePush {
                    remote: remote.clone(),
                },
                paths.clone(),
            )?;
            evidence.record_plan(request_id, &plan)?;
            reporter.emit(
                EventKind::Progress,
                "pre-push",
                &format!("{} full checks planned", plan.operations.len()),
                Some(0),
            )?;
            let outcome = executor::execute_with_changes(
                evidence, request_id, repository, &plan, &paths, reporter,
            )?;
            if !plan.external_requirements.is_empty() {
                for requirement in &plan.external_requirements {
                    reporter.emit(EventKind::Warning, "external", &requirement.message, None)?;
                }
                return Ok(Outcome::ExternalRequired);
            }
            return Ok(outcome);
        }
        CliCommand::Deliver { target: None } => return deliver(evidence, repository, reporter),
        CliCommand::Deliver {
            target: Some(DeliverTarget::LocalMain),
        } => return deliver_local_main(repository, reporter),
        CliCommand::Benchmark { scenario, arm } => {
            return agent_cli::benchmark::execute(repository, *scenario, *arm, reporter);
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
        CliCommand::Ci { command } => match command {
            CiCommand::HostAssurance(scope) => {
                return run_assurance(
                    evidence,
                    request_id,
                    repository,
                    AssuranceRequest::CiHostAssurance {
                        scope: scope.scope(),
                    },
                    true,
                    reporter,
                );
            }
            CiCommand::PlatformCandidates { artifacts, name } => {
                reporter.emit(
                    EventKind::Progress,
                    "ci",
                    "Selecting reusable platform artifacts",
                    Some(50),
                )?;
                agent_cli::ci::print_candidates(artifacts, name)?;
                return Ok(Outcome::Passed);
            }
            CiCommand::PlatformEligibleRun { run, head_sha } => {
                reporter.emit(
                    EventKind::Progress,
                    "ci",
                    "Checking platform run provenance",
                    Some(50),
                )?;
                agent_cli::ci::require_eligible_run(run, head_sha)?;
                return Ok(Outcome::Passed);
            }
            CiCommand::RequireAlphaPromotion {
                channel,
                alpha_sha,
                candidate_sha,
            } => {
                reporter.emit(
                    EventKind::Progress,
                    "ci",
                    "Checking release promotion",
                    Some(50),
                )?;
                agent_cli::ci::require_alpha_promotion(channel, alpha_sha, candidate_sha)?;
                return Ok(Outcome::Passed);
            }
            CiCommand::PlatformManifest {
                command:
                    PlatformManifestCommand::Generate {
                        output,
                        main,
                        gui,
                        manager,
                        scanout_module,
                        scanout_metadata,
                        latch_rbf,
                        latch_metadata,
                        platform_bundle_manifest,
                        main_revision,
                        magik_revision,
                        layout,
                    },
            } => {
                reporter.emit(
                    EventKind::Progress,
                    "manifest",
                    "Generating platform manifest",
                    Some(40),
                )?;
                agent_cli::platform_manifest::generate(
                    output,
                    &agent_cli::platform_manifest::Artifacts {
                        main: main.clone(),
                        gui: gui.clone(),
                        manager: manager.clone(),
                        scanout_module: scanout_module.clone(),
                        scanout_metadata: scanout_metadata.clone(),
                        latch_rbf: latch_rbf.clone(),
                        latch_metadata: latch_metadata.clone(),
                    },
                    &agent_cli::platform_manifest::ReleaseIdentity::from_bundle_manifest(
                        platform_bundle_manifest,
                    )?,
                    main_revision,
                    magik_revision,
                    agent_cli::platform_manifest::parse_layout(layout)?,
                )?;
                return Ok(Outcome::Passed);
            }
            CiCommand::PlatformManifest {
                command:
                    PlatformManifestCommand::Verify {
                        manifest,
                        root,
                        layout,
                    },
            } => {
                reporter.emit(
                    EventKind::Progress,
                    "manifest",
                    "Verifying platform manifest",
                    Some(40),
                )?;
                agent_cli::platform_manifest::verify(
                    manifest,
                    root.as_deref(),
                    agent_cli::platform_manifest::parse_layout(layout)?,
                )?;
                return Ok(Outcome::Passed);
            }
            CiCommand::GameDatabases { command } => {
                use agent_cli::cli::GameDatabaseCommand;
                reporter.emit(
                    EventKind::Progress,
                    "databases",
                    "Processing game-database bundle",
                    Some(30),
                )?;
                match command {
                    GameDatabaseCommand::BuildMame {
                        out,
                        listxml,
                        mame,
                        machine_sqlite,
                        software_dir,
                    } => {
                        let mut args = vec![
                            "mame-metadata-build".to_owned(),
                            "--out".to_owned(),
                            out.to_string_lossy().into_owned(),
                        ];
                        for (flag, path) in [
                            ("--listxml", listxml.as_ref()),
                            ("--mame", mame.as_ref()),
                            ("--machine-sqlite", machine_sqlite.as_ref()),
                            ("--software-dir", software_dir.as_ref()),
                        ] {
                            if let Some(path) = path {
                                args.extend([flag.to_owned(), path.to_string_lossy().into_owned()]);
                            }
                        }
                        agent_cli::commands::ci::run_local_host(args)?;
                    }
                    GameDatabaseCommand::ImportArcade {
                        sqlite,
                        csv,
                        source_sha,
                    } => {
                        agent_cli::commands::ci::run_local_host(vec![
                            "arcade-database-import".to_owned(),
                            "--sqlite".to_owned(),
                            sqlite.to_string_lossy().into_owned(),
                            "--csv".to_owned(),
                            csv.to_string_lossy().into_owned(),
                            "--source-sha".to_owned(),
                            source_sha.clone(),
                        ])?;
                    }
                    GameDatabaseCommand::Create {
                        mame_sqlite,
                        hbmame_sqlite,
                        release_version,
                        mame_tag,
                        mame_sha,
                        mame_listxml_asset,
                        mame_listxml_sha256,
                        hbmame_tag,
                        hbmame_sha,
                        mame_builder_sha,
                        hbmame_builder_sha,
                        arcade_database_csv,
                        arcade_database_license,
                        arcade_database_sha,
                        arcade_database_builder_sha,
                        output,
                    } => {
                        let archive = agent_cli::game_databases::create(
                            &agent_cli::game_databases::Create {
                                mame: mame_sqlite,
                                hbmame: hbmame_sqlite,
                                release_version: *release_version,
                                mame_tag,
                                mame_sha,
                                listxml_asset: mame_listxml_asset,
                                listxml_sha256: mame_listxml_sha256,
                                hbmame_tag,
                                hbmame_sha,
                                mame_builder_sha,
                                hbmame_builder_sha,
                                arcade_database_csv,
                                arcade_database_license,
                                arcade_database_sha,
                                arcade_database_builder_sha,
                                output,
                            },
                        )?;
                        println!("{}", archive.display());
                    }
                    GameDatabaseCommand::Verify {
                        archive,
                        manifest,
                        checksums,
                    } => {
                        println!(
                            "{}",
                            serde_json::to_string(&agent_cli::game_databases::verify(
                                archive,
                                manifest.as_deref(),
                                checksums.as_deref()
                            )?)
                            .unwrap()
                        );
                    }
                    GameDatabaseCommand::ExtractRelease { release, output } => {
                        println!(
                            "{}",
                            serde_json::to_string(&agent_cli::game_databases::extract_release(
                                release, output
                            )?)
                            .unwrap()
                        );
                    }
                    GameDatabaseCommand::PlanUpdate {
                        manifest,
                        mame_tag,
                        mame_sha,
                        hbmame_tag,
                        hbmame_sha,
                        arcade_database_sha,
                        github_output,
                    } => {
                        let current = manifest
                            .as_ref()
                            .map(std::fs::read_to_string)
                            .transpose()
                            .map_err(|error| error.to_string())?
                            .map(|text| {
                                serde_json::from_str(&text).map_err(|error| error.to_string())
                            })
                            .transpose()?;
                        let result = agent_cli::game_databases::update_plan(
                            current.as_ref(),
                            mame_tag,
                            mame_sha,
                            hbmame_tag,
                            hbmame_sha,
                            arcade_database_sha,
                        )?;
                        if let Some(path) = github_output {
                            append_github_output(
                                path,
                                &result,
                                &[
                                    "current_version",
                                    "next_version",
                                    "mame_changed",
                                    "hbmame_changed",
                                    "arcade_database_changed",
                                    "update_needed",
                                ],
                            )?;
                        }
                        println!("{}", serde_json::to_string(&result).unwrap());
                    }
                }
                return Ok(Outcome::Passed);
            }
            CiCommand::PlatformBundle { command } => {
                use agent_cli::cli::PlatformBundleCommand;
                reporter.emit(
                    EventKind::Progress,
                    "platform",
                    "Processing platform bundle",
                    Some(30),
                )?;
                let output = match command {
                    PlatformBundleCommand::Create {
                        main_dir,
                        fpga_dir,
                        scanout_dir,
                        main_id,
                        fpga_id,
                        kernel_id,
                        main_run_id,
                        fpga_run_id,
                        kernel_run_id,
                        main_head_sha,
                        fpga_head_sha,
                        kernel_head_sha,
                        main_source,
                        fpga_source,
                        kernel_source,
                        release_version,
                        output,
                        ..
                    } => agent_cli::platform_bundle::create(&agent_cli::platform_bundle::Create {
                        main: main_dir,
                        fpga: fpga_dir,
                        scanout: scanout_dir,
                        main_id,
                        fpga_id,
                        kernel_id,
                        main_run_id,
                        fpga_run_id,
                        kernel_run_id,
                        main_head_sha,
                        fpga_head_sha,
                        kernel_head_sha,
                        release_version: *release_version,
                        output,
                        main_source,
                        fpga_source,
                        kernel_source,
                    })?
                    .display()
                    .to_string(),
                    PlatformBundleCommand::Verify {
                        archive,
                        manifest,
                        release_version,
                    } => serde_json::to_string(&agent_cli::platform_bundle::verify(
                        archive,
                        manifest.as_deref(),
                        *release_version,
                    )?)
                    .unwrap(),
                    PlatformBundleCommand::ExtractComponent {
                        archive,
                        manifest,
                        component,
                        component_id,
                        output,
                    } => serde_json::to_string(&agent_cli::platform_bundle::extract_component(
                        archive,
                        manifest,
                        component,
                        component_id,
                        output,
                    )?)
                    .unwrap(),
                    PlatformBundleCommand::VerifyComponent {
                        component,
                        artifact,
                        component_id,
                        revision,
                    } => serde_json::to_string(&agent_cli::platform_bundle::verify_component(
                        component,
                        artifact,
                        component_id,
                        revision.as_deref(),
                    )?)
                    .unwrap(),
                    PlatformBundleCommand::CompactComponent {
                        component,
                        artifact,
                        output,
                        component_id,
                    } => agent_cli::platform_bundle::compact_component(
                        component,
                        artifact,
                        output,
                        component_id,
                    )?
                    .display()
                    .to_string(),
                    PlatformBundleCommand::WriteComponentCache {
                        component,
                        artifact,
                        component_id,
                        run_id,
                        head_sha,
                    } => {
                        agent_cli::platform_bundle::write_component_cache(
                            component,
                            artifact,
                            component_id,
                            run_id,
                            head_sha,
                        )?;
                        String::new()
                    }
                    PlatformBundleCommand::PlanUpdate {
                        manifest,
                        current_version,
                        main_id,
                        fpga_id,
                        kernel_id,
                        github_output,
                    } => {
                        let current: Option<serde_json::Value> = manifest
                            .as_ref()
                            .map(std::fs::read_to_string)
                            .transpose()
                            .map_err(|e| e.to_string())?
                            .map(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
                            .transpose()?;
                        let result = agent_cli::platform_bundle::update_plan(
                            current.as_ref(),
                            *current_version,
                            main_id,
                            fpga_id,
                            kernel_id,
                        )?;
                        if let Some(path) = github_output {
                            append_github_output(
                                path,
                                &result,
                                &[
                                    "current_version",
                                    "next_version",
                                    "current_bundle_id",
                                    "bundle_id",
                                    "update_needed",
                                    "main_changed",
                                    "fpga_changed",
                                    "kernel_changed",
                                    "release_tag",
                                ],
                            )?;
                        }
                        serde_json::to_string(&result).unwrap()
                    }
                };
                if !output.is_empty() {
                    println!("{output}");
                }
                return Ok(Outcome::Passed);
            }
        },
        CliCommand::Plan(scope) => {
            return run_assurance(
                evidence,
                request_id,
                repository,
                AssuranceRequest::Plan {
                    scope: scope.scope(),
                },
                false,
                reporter,
            );
        }
        CliCommand::Architecture { command } => {
            agent_cli::architecture::execute(repository, command)?;
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

fn run_assurance(
    evidence: &Evidence,
    request_id: &str,
    repository: &Path,
    intent: AssuranceRequest,
    execute: bool,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    let selected = match &intent {
        AssuranceRequest::Plan { scope } | AssuranceRequest::CiHostAssurance { scope } => scope,
        AssuranceRequest::PrePush { .. } => {
            unreachable!("pre-push paths are collected by the hook")
        }
    };
    let paths = scope::collect(evidence, request_id, repository, selected)?;
    let claimed_paths = paths.clone();
    let plan = planner::affected_plan_at(repository, intent, paths)?;
    evidence.record_plan(request_id, &plan)?;
    let summary = if plan.operations.is_empty() {
        "No lint operations selected".to_owned()
    } else {
        format!("{} checks planned", plan.operations.len())
    };
    reporter.emit(
        EventKind::Progress,
        if execute { "ci-assurance" } else { "plan" },
        &summary,
        Some(0),
    )?;
    let outcome = if execute {
        executor::execute_with_changes(
            evidence,
            request_id,
            repository,
            &plan,
            &claimed_paths,
            reporter,
        )?
    } else if plan.operations.is_empty() {
        Outcome::NoOp
    } else {
        Outcome::Passed
    };
    for requirement in &plan.external_requirements {
        reporter.emit(EventKind::Warning, "external", &requirement.message, None)?;
    }
    Ok(if plan.external_requirements.is_empty() {
        outcome
    } else {
        Outcome::ExternalRequired
    })
}

fn append_github_output(path: &Path, result: &serde_json::Value, keys: &[&str]) -> AgentResult<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    write_github_output(&mut file, result, keys)
}

fn write_github_output(
    output: &mut impl Write,
    result: &serde_json::Value,
    keys: &[&str],
) -> AgentResult<()> {
    for key in keys {
        let value = result[*key]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| result[*key].to_string());
        writeln!(output, "{key}={value}").map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn deliver(
    evidence: &Evidence,
    repository: &std::path::Path,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    let total_started = Instant::now();
    let delivery = deliver_inner(evidence, repository, reporter);
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
    agent_cli::delivery::execute(repository, &sha, reporter)
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

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("write failed"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn github_output_appends_ordered_scalar_fields() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-github-output-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("output");
        fs::write(&path, "existing=value\n").unwrap();
        let result = serde_json::json!({"text":"release-v1","enabled":true,"version":2});
        append_github_output(&path, &result, &["version", "text", "enabled"]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "existing=value\nversion=2\ntext=release-v1\nenabled=true\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn github_output_propagates_open_and_write_failures() {
        let result = serde_json::json!({"value":1});
        let missing_parent = std::env::temp_dir()
            .join("agent-cli-missing-output-parent")
            .join("output");
        assert!(append_github_output(&missing_parent, &result, &["value"]).is_err());
        assert_eq!(
            write_github_output(&mut FailingWriter, &result, &["value"])
                .unwrap_err()
                .to_string(),
            "write failed"
        );
    }

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
