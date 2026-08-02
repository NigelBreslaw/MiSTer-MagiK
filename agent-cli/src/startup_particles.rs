// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Focused Magik/cabinet particle development workflows.

use crate::build::{BuildSpec, execute};
use crate::commands::device::{StartupParticleRuntime, StartupParticlesArgs};
use crate::error::AgentResult;
use crate::process;
use crate::progress::Reporter;
use clap::{Args, Subcommand, ValueEnum};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const BUILD_DEADLINE: Duration = Duration::from_secs(30 * 60);
const LAB_DIR: &str = "apps/framebuffer-scene-lab";
const LAB_BINARY: &str = "mister-magik-framebuffer-scene-lab";
const MAGIK_SCHEMA: &str = "mister-magik-particle-magik-v1";
const CABINET_SCHEMA: &str = "mister-magik-particle-cabinet-v1";
const MAX_RECIPE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum StartupParticlesCommand {
    Preview(PreviewArgs),
}
#[derive(Clone, Debug, Eq, PartialEq, Args)]
pub struct PreviewArgs {
    recipe: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum SceneLabCommand {
    Preview(ScenePreviewArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum SceneLabScene {
    Magik,
    Cabinet,
    NavigationTransition,
}

impl SceneLabScene {
    const fn label(self) -> &'static str {
        match self {
            Self::Magik => "magik",
            Self::Cabinet => "cabinet",
            Self::NavigationTransition => "navigation-transition",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Args)]
pub struct ScenePreviewArgs {
    #[arg(long, value_enum)]
    scene: SceneLabScene,
    #[arg(long)]
    recipe: Option<PathBuf>,
    #[arg(long)]
    fixture: Option<String>,
}

pub fn execute_preview(repository: &Path, command: &StartupParticlesCommand) -> AgentResult<()> {
    match command {
        StartupParticlesCommand::Preview(args) => preview(repository, args),
    }
}

pub fn execute_scene_preview(repository: &Path, command: &SceneLabCommand) -> AgentResult<()> {
    match command {
        SceneLabCommand::Preview(args) => scene_preview(repository, args),
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
    let spec = BuildSpec::framebuffer_scene_lab_device();
    execute(repository, &spec, reporter)?;
    crate::commands::device::run_startup_particles(args, Some(&repository.join(spec.artifact())))
}

fn preview(repository: &Path, args: &PreviewArgs) -> AgentResult<()> {
    if !cfg!(target_os = "macos") {
        return Err("startup particle preview is available only on macOS".into());
    }
    let scene = recipe_scene(&args.recipe)?;
    run_preview(repository, scene, Some(&args.recipe), None)
}

fn scene_preview(repository: &Path, args: &ScenePreviewArgs) -> AgentResult<()> {
    match args.scene {
        SceneLabScene::Magik | SceneLabScene::Cabinet => {
            let recipe = args
                .recipe
                .as_deref()
                .ok_or("particle scenes require --recipe")?;
            if args.fixture.is_some() {
                return Err("particle scenes do not accept --fixture".into());
            }
            let actual = recipe_scene(recipe)?;
            if actual != args.scene.label() {
                return Err(format!(
                    "--scene {} does not match the {actual} recipe",
                    args.scene.label()
                )
                .into());
            }
            run_preview(repository, actual, Some(recipe), None)
        }
        SceneLabScene::NavigationTransition => {
            Err("navigation-transition fixtures are added by the navigation extraction".into())
        }
    }
}

fn run_preview(
    repository: &Path,
    scene: &str,
    recipe: Option<&Path>,
    fixture: Option<&str>,
) -> AgentResult<()> {
    if !cfg!(target_os = "macos") {
        return Err("framebuffer scene preview is available only on macOS".into());
    }
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
    let mut preview = Command::new(&binary);
    preview.args(["--scene", scene]);
    if let Some(recipe) = recipe {
        preview.arg("--recipe").arg(recipe);
    }
    if let Some(fixture) = fixture {
        preview.arg("--fixture").arg(fixture);
    }
    let status = preview
        .status()
        .map_err(|error| format!("cannot run {}: {error}", binary.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("startup particle preview exited with {status}").into())
    }
}

fn recipe_scene(path: &Path) -> AgentResult<&'static str> {
    let bytes = validate_recipe_file(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid scene recipe {}: {error}", path.display()))?;
    match value.get("schema").and_then(serde_json::Value::as_str) {
        Some(MAGIK_SCHEMA) => Ok("magik"),
        Some(CABINET_SCHEMA) => Ok("cabinet"),
        _ => Err("scene lab accepts only MagiK V1 or cabinet V1 recipes".into()),
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
