// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Focused Magik/cabinet particle development workflows.

use crate::build::{BuildSpec, execute};
use crate::commands::device::{StartupParticleRuntime, StartupParticlesArgs};
use crate::error::AgentResult;
use crate::process;
use crate::progress::Reporter;
use clap::{Args, Subcommand};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const BUILD_DEADLINE: Duration = Duration::from_secs(30 * 60);
const LAB_DIR: &str = "apps/startup-particle-lab";
const LAB_BINARY: &str = "mister-magik-startup-particle-lab";
const MAGIK_SCHEMA: &str = "mister-magik-particle-magik-v1";
const MAX_RECIPE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum StartupParticlesCommand {
    Preview(PreviewArgs),
}
#[derive(Clone, Debug, Eq, PartialEq, Args)]
pub struct PreviewArgs {
    recipe: PathBuf,
}

pub fn execute_preview(repository: &Path, command: &StartupParticlesCommand) -> AgentResult<()> {
    match command {
        StartupParticlesCommand::Preview(args) => preview(repository, args),
    }
}

pub fn execute_device(
    repository: &Path,
    args: &StartupParticlesArgs,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    require_attended_terminal()?;
    if args.runtime == StartupParticleRuntime::DevLauncher {
        validate_magik_recipe(&args.recipe)?;
        return crate::commands::device::run_startup_particles(args, None);
    }
    let spec = BuildSpec::startup_particle_lab_device();
    execute(repository, &spec, reporter)?;
    crate::commands::device::run_startup_particles(args, Some(&repository.join(spec.artifact())))
}

fn preview(repository: &Path, args: &PreviewArgs) -> AgentResult<()> {
    if !cfg!(target_os = "macos") {
        return Err("startup particle preview is available only on macOS".into());
    }
    validate_recipe_file(&args.recipe)?;
    let lab = repository.join(LAB_DIR);
    let mut build = Command::new("cargo");
    build
        .current_dir(&lab)
        .args(["build", "--locked"])
        .stdin(Stdio::null());
    let mut child = build
        .spawn()
        .map_err(|error| format!("cannot start startup particle preview build: {error}"))?;
    let status = process::wait(
        &mut child,
        Some(BUILD_DEADLINE),
        "startup particle preview build",
        None,
        || Ok(()),
    )?;
    if !status.success() {
        return Err(format!("startup particle preview build exited with {status}").into());
    }
    let binary = lab.join("target/debug").join(LAB_BINARY);
    let status = Command::new(&binary)
        .arg("--recipe")
        .arg(&args.recipe)
        .status()
        .map_err(|error| format!("cannot run {}: {error}", binary.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("startup particle preview exited with {status}").into())
    }
}

fn require_attended_terminal() -> AgentResult<()> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(
            "startup particle device sessions are attended and require an interactive terminal"
                .into(),
        );
    }
    Ok(())
}

fn validate_recipe_file(path: &Path) -> AgentResult<Vec<u8>> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("cannot inspect particle recipe {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("particle recipe is not a file: {}", path.display()).into());
    }
    if metadata.len() > MAX_RECIPE_BYTES {
        return Err(format!(
            "particle recipe exceeds the {MAX_RECIPE_BYTES} byte limit: {}",
            path.display()
        )
        .into());
    }
    std::fs::read(path)
        .map_err(|error| format!("cannot read particle recipe {}: {error}", path.display()).into())
}

fn validate_magik_recipe(path: &Path) -> AgentResult<()> {
    let bytes = validate_recipe_file(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid Magik recipe {}: {error}", path.display()))?;
    if value.get("schema").and_then(serde_json::Value::as_str) != Some(MAGIK_SCHEMA) {
        return Err("the dev launcher accepts only Magik V1 recipes".into());
    }
    Ok(())
}
