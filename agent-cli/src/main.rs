// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use agent_cli::cli::{Cli, OutputFormat};
use agent_cli::error::AgentResult;
use agent_cli::evidence::Evidence;
use agent_cli::executor;
use agent_cli::model::{Intent, Outcome};
use agent_cli::planner;
use agent_cli::progress::{EventKind, Reporter};
use agent_cli::request::RawRequest;
use agent_cli::scope;
use clap::Parser;
use std::io::Write;
use std::path::Path;

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
    let output = cli.output_format;
    let intent = resolve_task_intent(&evidence, &repository, cli.into_intent())
        .unwrap_or_else(|error| fatal(&error));
    evidence
        .record_intent(&raw.id, &intent)
        .unwrap_or_else(|error| fatal(&error));
    let mut reporter = Reporter::new_at(&evidence, output, &raw.id, raw.started);
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
            let rendered = error.to_string();
            let (phase, message) = rendered
                .split_once(": ")
                .filter(|(phase, _)| matches!(*phase, "check" | "verify"))
                .unwrap_or(("request", rendered.as_str()));
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
    if matches!(intent, Intent::DatabaseRotate) {
        let sha = agent_cli::task::current_head(&repository).unwrap_or_else(|error| fatal(&error));
        let archive = evidence.rotate(&sha).unwrap_or_else(|error| fatal(&error));
        println!("archived evidence: {}", archive.display());
    }
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
            return evidence
                .active_task_id_for_session(repository, &task_id)?
                .ok_or_else(|| {
                    format!(
                        "No active task lifecycle exists for {task_id}. Run `scripts/agent task begin` before editing."
                    )
                });
        }
        let session_id = evidence.active_manual_task_id(repository)?.ok_or_else(|| {
            "No task baseline exists. Run `scripts/agent task begin` before editing.".to_owned()
        })?;
        evidence
            .active_task_id_for_session(repository, &session_id)?
            .ok_or_else(|| {
                "No task baseline exists. Run `scripts/agent task begin` before editing.".to_owned()
            })
    };
    Ok(match intent {
        Intent::TaskStatus { task_id } => Intent::TaskStatus {
            task_id: resolve(task_id)?,
        },
        Intent::TaskSupersede { task_id } => Intent::TaskSupersede { task_id },
        Intent::Commit { task_id, message } => Intent::Commit {
            task_id: resolve(task_id)?,
            message,
        },
        Intent::Deliver => Intent::Deliver,
        Intent::Benchmark => Intent::Benchmark,
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
) -> AgentResult<Outcome> {
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
        Intent::TaskSupersede { task_id } => {
            evidence.supersede_task(repository, task_id)?;
            if output == OutputFormat::Human {
                println!("task: superseded ({task_id})");
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
        Intent::Deliver => return deliver(evidence, repository, reporter),
        Intent::Benchmark => {
            if !agent_cli::git::value(repository, &["status", "--porcelain"])?.is_empty() {
                return Err("dirty_worktree: commit changes before benchmarking".into());
            }
            let commit = agent_cli::git::value(repository, &["rev-parse", "HEAD"])?;
            let paths = agent_cli::git::head_changed_paths(repository)?;
            return agent_cli::benchmark::execute(repository, &paths, &commit, reporter);
        }
        Intent::CaptureUsbVideo {
            output: destination,
            seconds,
        } => {
            let artifact = match seconds {
                Some(seconds) => {
                    agent_cli::capture::execute_movie(destination.as_deref(), *seconds)?
                }
                None => agent_cli::capture::execute(destination.as_deref())?,
            };
            match output {
                OutputFormat::Human => println!("{}", artifact.markdown_link()),
                OutputFormat::Ndjson => {
                    println!("{}", serde_json::to_string(&artifact).unwrap());
                }
            }
            return Ok(Outcome::Passed);
        }
        Intent::ReleaseQualify => {
            return agent_cli::release::execute(reporter);
        }
        Intent::Diagnose => {
            return agent_cli::diagnose::execute(repository, reporter);
        }
        Intent::Build { intent } => {
            if matches!(intent, agent_cli::build::BuildCommand::ValidateRuntime) {
                agent_cli::build::execute_runtime_validation(repository, reporter)?;
                return Ok(Outcome::Passed);
            }
            let spec = agent_cli::build::BuildSpec::for_recipe((*intent).into());
            agent_cli::build::execute(repository, &spec, reporter)?;
        }
        Intent::CiPlatformCandidates { artifacts, name } => {
            reporter.emit(
                EventKind::Progress,
                "ci",
                "Selecting reusable platform artifacts",
                Some(50),
            )?;
            agent_cli::ci::print_candidates(artifacts, name)?;
            return Ok(Outcome::Passed);
        }
        Intent::CiPlatformEligibleRun { run, head_sha } => {
            reporter.emit(
                EventKind::Progress,
                "ci",
                "Checking platform run provenance",
                Some(50),
            )?;
            agent_cli::ci::require_eligible_run(run, head_sha)?;
            return Ok(Outcome::Passed);
        }
        Intent::CiRequireAlphaPromotion {
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
        Intent::CiPlatformManifestGenerate {
            output,
            main,
            gui,
            manager,
            scanout_module,
            scanout_metadata,
            latch_rbf,
            latch_metadata,
            main_revision,
            magik_revision,
            layout,
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
                main_revision,
                magik_revision,
                agent_cli::platform_manifest::Layout::parse(layout)?,
            )?;
            return Ok(Outcome::Passed);
        }
        Intent::CiPlatformManifestVerify {
            manifest,
            root,
            layout,
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
                agent_cli::platform_manifest::Layout::parse(layout)?,
            )?;
            return Ok(Outcome::Passed);
        }
        Intent::CiGameDatabases { command } => {
            use agent_cli::cli::GameDatabaseCommand;
            reporter.emit(
                EventKind::Progress,
                "databases",
                "Processing game-database bundle",
                Some(30),
            )?;
            match command {
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
                    output,
                } => {
                    let archive =
                        agent_cli::game_databases::create(&agent_cli::game_databases::Create {
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
                            output,
                        })?;
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
                    github_output,
                } => {
                    let current = manifest
                        .as_ref()
                        .map(std::fs::read_to_string)
                        .transpose()
                        .map_err(|error| error.to_string())?
                        .map(|text| serde_json::from_str(&text).map_err(|error| error.to_string()))
                        .transpose()?;
                    let result = agent_cli::game_databases::update_plan(
                        current.as_ref(),
                        mame_tag,
                        mame_sha,
                        hbmame_tag,
                        hbmame_sha,
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
                                "update_needed",
                            ],
                        )?;
                    }
                    println!("{}", serde_json::to_string(&result).unwrap());
                }
            }
            return Ok(Outcome::Passed);
        }
        Intent::CiPlatformBundle { command } => {
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
            return agent_cli::doctor::execute(repository, reporter);
        }
        Intent::DatabaseStatus => {
            let status = evidence.status()?;
            if output == OutputFormat::Human {
                println!("{}", serde_json::to_string_pretty(&status).unwrap());
            }
        }
        Intent::DatabaseReport => {
            let report = evidence.report()?;
            if output == OutputFormat::Human {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            }
        }
        Intent::DatabaseRotate => {
            return Ok(Outcome::Passed);
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
    let delivery = deliver_inner(evidence, repository, reporter);
    let cleanup = agent_cli::delivery::cleanup_workspace(repository);
    match (delivery, cleanup) {
        (Ok(outcome), Ok(())) => Ok(outcome),
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
    }
}

fn deliver_inner(
    _evidence: &Evidence,
    repository: &std::path::Path,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    let dirty = agent_cli::git::value(repository, &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err("dirty_worktree: commit or discard changes before delivery".into());
    }
    let sha = agent_cli::task::current_head(repository)?;
    let paths = agent_cli::deploy::deployment_paths(repository, Vec::new())?;
    let mut deployment = agent_cli::deploy::plan(repository, paths)?;
    // The development manifest binds the launcher hash to Main, the scanout
    // module, and the latch RBF. A launcher-only transaction would make that
    // installed set invalid, so every delivery must publish one coherent set.
    deployment.kind = agent_cli::deploy::DeploymentKind::Platform;
    let local_main = if deployment.kind == agent_cli::deploy::DeploymentKind::Platform {
        deployment.platform_candidate = Some(agent_cli::platform_ci::resolve_published_repository(
            repository,
            |progress| reporter.emit(EventKind::Progress, "platform", progress, None),
        )?);
        Some(local_main_directory(repository))
    } else {
        None
    };
    agent_cli::delivery::execute(
        repository,
        &deployment,
        &sha,
        local_main.as_deref(),
        reporter,
    )
}

fn local_main_directory(repository: &Path) -> std::path::PathBuf {
    std::env::var_os("MISTER_MAIN_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| repository.join("../Main_MiSTer"))
}

fn fatal(message: &str) -> ! {
    eprintln!("agent-cli: {message}");
    std::process::exit(70);
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
    fn bare_invocation_is_help_discovery() {
        assert!(is_discovery_request(&["agent-cli".into()]));
        assert!(is_discovery_request(&["agent-cli".into(), "--help".into()]));
        assert!(!is_discovery_request(&["agent-cli".into(), "check".into()]));
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
    fn repeated_session_resolves_active_and_latest_committed_lifecycles() {
        let root = std::env::temp_dir().join(format!(
            "agent-cli-session-resolution-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let evidence = Evidence::open_at(&root).unwrap();
        let worktree = Path::new("/tmp/session-resolution-worktree");
        let first = evidence
            .save_task_baseline("thread-one", worktree, &(), false)
            .unwrap();
        evidence.close_task(&first, "commit-one").unwrap();
        let second = evidence
            .save_task_baseline("thread-one", worktree, &(), false)
            .unwrap();

        assert_eq!(
            resolve_task_intent(
                &evidence,
                worktree,
                Intent::Commit {
                    task_id: "thread-one".into(),
                    message: "message".into(),
                }
            )
            .unwrap(),
            Intent::Commit {
                task_id: second,
                message: "message".into(),
            }
        );
        assert_eq!(
            resolve_task_intent(&evidence, worktree, Intent::Deliver).unwrap(),
            Intent::Deliver
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delivery_can_repeat_after_completion_or_physical_recovery() {
        use agent_cli::evidence::DeliveryState;

        assert!(DeliveryState::ExternalPending.can_resume());
        assert!(DeliveryState::RecoveryRequired.can_resume());
        assert!(DeliveryState::Complete.can_resume());
        assert!(DeliveryState::Failed.can_resume());
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
