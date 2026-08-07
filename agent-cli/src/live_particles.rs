// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Build-and-run entrypoints for the standalone particle lab.

use crate::build::{BuildSpec, execute};
use crate::commands::device::LiveParticlesArgs;
use crate::error::AgentResult;
use crate::process;
use crate::progress::Reporter;
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const BUILD_DEADLINE: Duration = Duration::from_secs(30 * 60);
const LAB_DIR: &str = "apps/framebuffer-lab";
const LAB_BINARY: &str = "mister-magik-particle-lab";

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum LiveParticlesCommand {
    Preview(PreviewArgs),
}

#[derive(Clone, Debug, Eq, PartialEq, Args)]
pub struct PreviewArgs {
    family: PathBuf,
    #[arg(long)]
    demo: String,
}

pub fn execute_preview(repository: &Path, command: &LiveParticlesCommand) -> AgentResult<()> {
    match command {
        LiveParticlesCommand::Preview(args) => preview(repository, args),
    }
}

pub fn execute_device(
    repository: &Path,
    args: &LiveParticlesArgs,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    let spec = BuildSpec::framebuffer_lab_device();
    execute(repository, &spec, reporter)?;
    crate::commands::device::run_live_particles(args, &repository.join(spec.artifact()))
}

fn preview(repository: &Path, args: &PreviewArgs) -> AgentResult<()> {
    if !cfg!(target_os = "macos") {
        return Err("particle lab preview is available only on macOS".into());
    }
    let lab = repository.join(LAB_DIR);
    let mut build = crate::lab_build::command(&lab, None);
    build.stdin(Stdio::null());
    let mut child = build
        .spawn()
        .map_err(|error| format!("cannot start particle lab preview build: {error}"))?;
    let status = process::wait(
        &mut child,
        Some(BUILD_DEADLINE),
        "particle lab preview build",
        None,
        || Ok(()),
    )?;
    if !status.success() {
        return Err(format!("particle lab preview build exited with {status}").into());
    }
    let binary = crate::lab_build::artifact(&lab, LAB_BINARY);
    let status = Command::new(&binary)
        .args(["--demo", &args.demo, "--family"])
        .arg(&args.family)
        .status()
        .map_err(|error| format!("cannot run {}: {error}", binary.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("particle lab preview exited with {status}").into())
    }
}
