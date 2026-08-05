// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Focused Magik/cabinet particle development workflows.

use crate::build::{BuildSpec, execute};
use crate::commands::device::{
    SceneLabArgs, SceneLabScene as DeviceSceneLabScene, StartupParticleRuntime,
    StartupParticlesArgs,
};
use crate::error::AgentResult;
use crate::process;
use crate::progress::Reporter;
use clap::{Args, Subcommand, ValueEnum};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BUILD_DEADLINE: Duration = Duration::from_secs(30 * 60);
const LAB_DIR: &str = "apps/framebuffer-scene-lab";
const LAB_BINARY: &str = "mister-magik-framebuffer-scene-lab";
const MAGIK_SCHEMA: &str = "mister-magik-particle-magik-v1";
const CABINET_SCHEMA: &str = "mister-magik-particle-cabinet-v1";
const INTRO_SCHEMA: &str = "mister-magik-particle-intro-v1";
const MAX_RECIPE_BYTES: u64 = 1024 * 1024;
const CABINET_CASES: [&str; 27] = [
    "baseline-24064",
    "baseline-36096",
    "baseline-48128",
    "baseline-60160",
    "baseline-72192",
    "satellites-48128",
    "satellites-72192",
    "history-48128",
    "history-72192",
    "depth-48128",
    "depth-72192",
    "jitter-48128",
    "jitter-72192",
    "all-48128",
    "all-72192",
    "prism-39936",
    "aurora-39936",
    "vortex-39936",
    "studio-39936",
    "depth-prism-39936",
    "motion-heat-39936",
    "directional-39936",
    "phase-story-39936",
    "interference-39936",
    "arcade-palettes-39936",
    "texture-exact-39936",
    "texture-glow-39936",
];

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
    Capture(SceneCaptureArgs),
    AnalyzeCabinetCodegen,
    GenerateIntroAssets(GenerateIntroAssetsArgs),
}

#[derive(Clone, Debug, Eq, PartialEq, Args)]
pub struct GenerateIntroAssetsArgs {
    #[arg(long)]
    output: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Args)]
pub struct SceneCaptureArgs {
    #[arg(long, value_enum)]
    scene: SceneLabScene,
    #[arg(long)]
    recipe: Option<PathBuf>,
    #[arg(long)]
    archive: Option<PathBuf>,
    #[arg(long)]
    seed: Option<String>,
    #[arg(long, value_enum)]
    direction: Option<CardFlipDirection>,
    #[arg(long)]
    time_ms: u64,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum SceneLabScene {
    Magik,
    Cabinet,
    Intro,
    NavigationTransition,
    CardFlip,
    ScreenshotScreensaver,
}

impl SceneLabScene {
    const fn label(self) -> &'static str {
        match self {
            Self::Magik => "magik",
            Self::Cabinet => "cabinet",
            Self::Intro => "intro",
            Self::NavigationTransition => "navigation-transition",
            Self::CardFlip => "card-flip",
            Self::ScreenshotScreensaver => "screenshot-screensaver",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum CardFlipDirection {
    #[default]
    Forward,
    Reverse,
}

impl CardFlipDirection {
    const fn label(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Reverse => "reverse",
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
    archive: Option<PathBuf>,
    #[arg(long)]
    seed: Option<String>,
    #[arg(long)]
    fixture: Option<String>,
}

pub fn execute_preview(repository: &Path, command: &StartupParticlesCommand) -> AgentResult<()> {
    match command {
        StartupParticlesCommand::Preview(args) => preview(repository, args),
    }
}

pub fn execute_scene_preview(
    repository: &Path,
    command: &SceneLabCommand,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    match command {
        SceneLabCommand::Preview(args) => scene_preview(repository, args),
        SceneLabCommand::Capture(args) => scene_capture(repository, args),
        SceneLabCommand::AnalyzeCabinetCodegen => analyze_cabinet_codegen(repository, reporter),
        SceneLabCommand::GenerateIntroAssets(args) => generate_intro_assets(repository, args),
    }
}

fn scene_capture(repository: &Path, args: &SceneCaptureArgs) -> AgentResult<()> {
    match args.scene {
        SceneLabScene::Magik | SceneLabScene::Cabinet | SceneLabScene::Intro => {
            let recipe = args
                .recipe
                .as_deref()
                .ok_or("particle scene capture requires --recipe")?;
            if args.direction.is_some() {
                return Err("particle scene capture does not accept --direction".into());
            }
            let actual = recipe_scene(recipe)?;
            if actual != args.scene.label() {
                return Err(format!(
                    "--scene {} does not match the {actual} recipe",
                    args.scene.label()
                )
                .into());
            }
        }
        SceneLabScene::NavigationTransition => {
            return Err("scene-lab capture does not yet accept navigation fixtures".into());
        }
        SceneLabScene::CardFlip => {
            if args.recipe.is_some() || args.archive.is_some() || args.seed.is_some() {
                return Err("card-flip does not accept recipe, archive, or seed options".into());
            }
        }
        SceneLabScene::ScreenshotScreensaver => {
            if args.recipe.is_some() || args.direction.is_some() {
                return Err(
                    "screenshot-screensaver does not accept recipe or direction options".into(),
                );
            }
            let archive = args
                .archive
                .as_deref()
                .ok_or("screenshot-screensaver capture requires --archive")?;
            if !archive.is_file() {
                return Err(format!("screenshot archive is missing: {}", archive.display()).into());
            }
            if let Some(seed) = args.seed.as_deref() {
                parse_screenshot_seed(seed)?;
            }
        }
    }
    if args.scene != SceneLabScene::ScreenshotScreensaver
        && (args.archive.is_some() || args.seed.is_some())
    {
        return Err("--archive and --seed are valid only for screenshot-screensaver".into());
    }
    let lab = repository.join(LAB_DIR);
    let mut build = Command::new("cargo");
    build
        .current_dir(&lab)
        .args(["build", "--locked", "--bin", LAB_BINARY])
        .stdin(Stdio::null());
    let mut child = build
        .spawn()
        .map_err(|error| format!("cannot start scene capture build: {error}"))?;
    let status = process::wait(
        &mut child,
        Some(BUILD_DEADLINE),
        "scene capture build",
        None,
        || Ok(()),
    )?;
    if !status.success() {
        return Err(format!("scene capture build exited with {status}").into());
    }
    let binary = lab.join("target/debug").join(LAB_BINARY);
    let output = if args.output.is_absolute() {
        args.output.clone()
    } else {
        repository.join(&args.output)
    };
    let mut capture = Command::new(&binary);
    capture.args(["--scene", args.scene.label()]);
    if let Some(recipe) = args.recipe.as_deref() {
        capture.arg("--recipe").arg(recipe);
    }
    if let Some(archive) = args.archive.as_deref() {
        capture.arg("--archive").arg(archive);
    }
    if let Some(seed) = args.seed.as_deref() {
        capture.arg("--seed").arg(seed);
    }
    if args.scene == SceneLabScene::CardFlip {
        capture
            .arg("--direction")
            .arg(args.direction.unwrap_or_default().label());
    }
    let status = capture
        .args(["--time-ms", &args.time_ms.to_string(), "--output"])
        .arg(output)
        .status()
        .map_err(|error| format!("cannot start scene capture: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("scene capture exited with {status}").into())
    }
}

fn generate_intro_assets(repository: &Path, args: &GenerateIntroAssetsArgs) -> AgentResult<()> {
    let lab = repository.join(LAB_DIR);
    let output = if args.output.is_absolute() {
        args.output.clone()
    } else {
        repository.join(&args.output)
    };
    let mut command = Command::new("cargo");
    command
        .current_dir(&lab)
        .args([
            "run",
            "--locked",
            "--features",
            "asset-tools",
            "--bin",
            "generate-intro-assets",
            "--",
        ])
        .arg(output)
        .stdin(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start intro asset generator: {error}"))?;
    let status = process::wait(
        &mut child,
        Some(BUILD_DEADLINE),
        "intro asset generator",
        None,
        || Ok(()),
    )?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("intro asset generator exited with {status}").into())
    }
}

fn analyze_cabinet_codegen(repository: &Path, reporter: &mut Reporter<'_>) -> AgentResult<()> {
    let release = BuildSpec::framebuffer_scene_lab_device();
    execute(repository, &release, reporter)?;
    let spec = BuildSpec::framebuffer_scene_lab_analysis();
    execute(repository, &spec, reporter)?;
    let status = Command::new(
        repository.join("apps/framebuffer-scene-lab/scripts/analyze-cabinet-codegen.sh"),
    )
    .arg(repository.join(spec.artifact()))
    .status()
    .map_err(|error| format!("cannot start cabinet codegen analysis: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cabinet codegen analysis exited with {status}").into())
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

pub fn execute_scene_device(
    repository: &Path,
    args: &SceneLabArgs,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    require_attended_terminal()?;
    match args.scene {
        DeviceSceneLabScene::Magik | DeviceSceneLabScene::Cabinet | DeviceSceneLabScene::Intro => {
            let recipe = args
                .recipe
                .as_deref()
                .ok_or("particle scenes require --recipe")?;
            if args.fixture.is_some() {
                return Err("particle scenes do not accept --fixture".into());
            }
            let actual = recipe_scene(recipe)?;
            if actual != args.scene.as_str() {
                return Err(format!(
                    "--scene {} does not match the {actual} recipe",
                    args.scene.as_str()
                )
                .into());
            }
        }
        DeviceSceneLabScene::NavigationTransition => {
            if args.recipe.is_some() {
                return Err("navigation-transition does not accept --recipe".into());
            }
            validate_navigation_fixture(
                args.fixture
                    .as_deref()
                    .ok_or("navigation-transition requires --fixture")?,
            )?;
        }
        DeviceSceneLabScene::CardFlip => {
            if args.recipe.is_some() || args.fixture.is_some() {
                return Err("card-flip does not accept --recipe or --fixture".into());
            }
        }
        DeviceSceneLabScene::ScreenshotScreensaver => {
            if args.recipe.is_some() || args.fixture.is_some() {
                return Err("screenshot-screensaver does not accept --recipe or --fixture".into());
            }
            if let Some(seed) = args.seed.as_deref() {
                parse_screenshot_seed(seed)?;
            }
        }
    }
    if args.scene != DeviceSceneLabScene::ScreenshotScreensaver && args.seed.is_some() {
        return Err("scene-lab --seed is valid only for screenshot-screensaver".into());
    }
    if let Some(case) = args.case.as_deref() {
        if args.scene != DeviceSceneLabScene::Cabinet {
            return Err("scene-lab --case is valid only for the cabinet scene".into());
        }
        if !CABINET_CASES.contains(&case) {
            return Err(format!("unknown closed cabinet case {case:?}").into());
        }
        if args.seconds.is_none() {
            return Err("scene-lab --case requires --seconds".into());
        }
    }
    if args.warmup_seconds > 0 && args.seconds.is_none() {
        return Err("scene-lab --warmup-seconds requires --seconds".into());
    }
    if (args.profile || args.assess) && args.seconds.is_none() {
        return Err("scene-lab --profile and --assess require --seconds".into());
    }
    if args.profile && args.scene != DeviceSceneLabScene::CardFlip && args.case.is_none() {
        return Err("scene-lab --profile requires card-flip or a closed cabinet --case".into());
    }
    if args.assess && args.scene != DeviceSceneLabScene::CardFlip {
        return Err("scene-lab --assess requires card-flip".into());
    }
    if args.assess && args.profile {
        return Err("scene-lab --assess cannot be combined with --profile".into());
    }
    let spec = if args.profile && !args.assess {
        BuildSpec::framebuffer_scene_lab_analysis()
    } else {
        BuildSpec::framebuffer_scene_lab_device()
    };
    execute(repository, &spec, reporter)?;
    let output_dir = args.assess.then(|| {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after Unix epoch")
            .as_secs();
        repository
            .join("build/scene-lab/card-flip")
            .join(timestamp.to_string())
    });
    crate::commands::device::run_scene_lab(
        args,
        &repository.join(spec.artifact()),
        output_dir.as_deref(),
    )
}

fn preview(repository: &Path, args: &PreviewArgs) -> AgentResult<()> {
    if !cfg!(target_os = "macos") {
        return Err("startup particle preview is available only on macOS".into());
    }
    let scene = recipe_scene(&args.recipe)?;
    run_preview(repository, scene, Some(&args.recipe), None, None, None)
}

fn scene_preview(repository: &Path, args: &ScenePreviewArgs) -> AgentResult<()> {
    if args.scene != SceneLabScene::ScreenshotScreensaver
        && (args.archive.is_some() || args.seed.is_some())
    {
        return Err("--archive and --seed are valid only for screenshot-screensaver".into());
    }
    match args.scene {
        SceneLabScene::Magik | SceneLabScene::Cabinet | SceneLabScene::Intro => {
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
            run_preview(repository, actual, Some(recipe), None, None, None)
        }
        SceneLabScene::NavigationTransition => {
            if args.recipe.is_some() {
                return Err("navigation-transition does not accept --recipe".into());
            }
            let fixture = args
                .fixture
                .as_deref()
                .ok_or("navigation-transition requires --fixture")?;
            validate_navigation_fixture(fixture)?;
            run_preview(
                repository,
                args.scene.label(),
                None,
                Some(fixture),
                None,
                None,
            )
        }
        SceneLabScene::CardFlip => {
            if args.recipe.is_some() || args.fixture.is_some() {
                return Err("card-flip does not accept --recipe or --fixture".into());
            }
            run_preview(repository, args.scene.label(), None, None, None, None)
        }
        SceneLabScene::ScreenshotScreensaver => {
            if args.recipe.is_some() || args.fixture.is_some() {
                return Err(
                    "screenshot-screensaver does not accept recipe or fixture options".into(),
                );
            }
            let archive = args
                .archive
                .as_deref()
                .ok_or("screenshot-screensaver preview requires --archive")?;
            if !archive.is_file() {
                return Err(format!("screenshot archive is missing: {}", archive.display()).into());
            }
            if let Some(seed) = args.seed.as_deref() {
                parse_screenshot_seed(seed)?;
            }
            run_preview(
                repository,
                args.scene.label(),
                None,
                None,
                Some(archive),
                args.seed.as_deref(),
            )
        }
    }
}

fn validate_navigation_fixture(fixture: &str) -> AgentResult<()> {
    if matches!(fixture, "home-arcade" | "home-consoles" | "consoles-system") {
        Ok(())
    } else {
        Err(format!(
            "unsupported navigation fixture {fixture:?}; expected home-arcade, home-consoles, or consoles-system"
        )
        .into())
    }
}

pub(crate) fn parse_screenshot_seed(value: &str) -> AgentResult<u64> {
    let value = value.trim();
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or_else(
            || value.parse::<u64>(),
            |digits| u64::from_str_radix(digits, 16),
        )
        .map_err(|_| format!("invalid screenshot seed {value:?}").into())
}

fn run_preview(
    repository: &Path,
    scene: &str,
    recipe: Option<&Path>,
    fixture: Option<&str>,
    archive: Option<&Path>,
    seed: Option<&str>,
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
    if let Some(archive) = archive {
        preview.arg("--archive").arg(archive);
    }
    if let Some(seed) = seed {
        preview.arg("--seed").arg(seed);
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
        Some(INTRO_SCHEMA) => Ok("intro"),
        _ => Err("scene lab accepts only MagiK V1, cabinet V1, or intro V1 recipes".into()),
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
