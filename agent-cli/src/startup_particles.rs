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
use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BUILD_DEADLINE: Duration = Duration::from_secs(30 * 60);
const LAB_DIR: &str = "apps/framebuffer-scene-lab";
const LAB_BINARY: &str = "mister-magik-framebuffer-scene-lab";
const PROFILE_LOCK: &str = "apps/framebuffer-scene-lab/Cargo.lock";
const MAGIK_SCHEMA: &str = "mister-magik-particle-magik-v1";
const CABINET_SCHEMA: &str = "mister-magik-particle-cabinet-v1";
const MAX_RECIPE_BYTES: u64 = 1024 * 1024;
const CABINET_CASES: [&str; 15] = [
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
    AnalyzeCabinetCodegen,
    UpdateProfileLock,
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

pub fn execute_scene_preview(
    repository: &Path,
    command: &SceneLabCommand,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    match command {
        SceneLabCommand::Preview(args) => scene_preview(repository, args),
        SceneLabCommand::AnalyzeCabinetCodegen => analyze_cabinet_codegen(repository, reporter),
        SceneLabCommand::UpdateProfileLock => update_profile_lock(repository),
    }
}

fn update_profile_lock(repository: &Path) -> AgentResult<()> {
    let clean = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(repository)
        .output()
        .map_err(|error| format!("inspect repository before profile lock update: {error}"))?;
    if !clean.status.success() {
        return Err(format!(
            "cannot inspect repository before profile lock update: {}",
            String::from_utf8_lossy(&clean.stderr).trim()
        )
        .into());
    }
    if !clean.stdout.is_empty() {
        return Err("profile lock update requires a clean tracked working tree".into());
    }
    let temporary = TemporaryDirectory::new("mister-magik-profile-lock")?;
    let archive = temporary.path().join("repository.tar");
    let snapshot = temporary.path().join("repository");
    std::fs::create_dir(&snapshot)
        .map_err(|error| format!("create profile lock snapshot: {error}"))?;
    let archive_status = Command::new("git")
        .args(["archive", "--format=tar"])
        .arg(format!("--output={}", archive.display()))
        .arg("HEAD")
        .current_dir(repository)
        .status()
        .map_err(|error| format!("start profile lock snapshot archive: {error}"))?;
    if !archive_status.success() {
        return Err(format!("profile lock snapshot archive exited with {archive_status}").into());
    }
    let extract_status = Command::new("tar")
        .args(["-xf"])
        .arg(&archive)
        .args(["-C"])
        .arg(&snapshot)
        .status()
        .map_err(|error| format!("extract profile lock snapshot: {error}"))?;
    if !extract_status.success() {
        return Err(
            format!("profile lock snapshot extraction exited with {extract_status}").into(),
        );
    }
    std::fs::remove_file(&archive)
        .map_err(|error| format!("remove profile lock snapshot archive: {error}"))?;

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unavailable for the profile lock cache")?;
    let cargo_home = home.join(".cargo");
    let rust_toolchain = home.join(".rustup/toolchains/stable-aarch64-unknown-linux-gnu");
    let image = std::env::var("MISTER_APPLE_CONTAINER_IMAGE")
        .unwrap_or_else(|_| "mister-magik-cross-armv7:ubuntu20-arm64".into());
    let validation = profile_lock_container_command(
        &snapshot,
        &cargo_home,
        &rust_toolchain,
        &image,
        &[
            "metadata",
            "--locked",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ],
    )
    .output()
    .map_err(|error| format!("start offline profile lock validation: {error}"))?;
    if validation.status.success() {
        return Ok(());
    }
    let status = profile_lock_container_command(
        &snapshot,
        &cargo_home,
        &rust_toolchain,
        &image,
        &["generate-lockfile", "--offline"],
    )
    .status()
    .map_err(|error| format!("start offline profile lock update: {error}"))?;
    if !status.success() {
        return Err(format!(
            "offline profile lock update exited with {status}; locked validation failed first: {}",
            String::from_utf8_lossy(&validation.stderr).trim()
        )
        .into());
    }

    let generated = std::fs::read(snapshot.join(PROFILE_LOCK))
        .map_err(|error| format!("read generated profile lock: {error}"))?;
    let destination = repository.join(PROFILE_LOCK);
    let current = std::fs::read(&destination)
        .map_err(|error| format!("read current profile lock: {error}"))?;
    if current != generated {
        publish_profile_lock(&destination, &generated)?;
    }
    Ok(())
}

fn profile_lock_container_command(
    snapshot: &Path,
    cargo_home: &Path,
    rust_toolchain: &Path,
    image: &str,
    cargo_arguments: &[&str],
) -> Command {
    let mut command = Command::new("container");
    command
        .args([
            "run",
            "--progress",
            "none",
            "--arch",
            "arm64",
            "--rm",
            "--env",
            "CARGO_HOME=/cargo",
            "--env",
            "PATH=/rust/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            "--volume",
        ])
        .arg(format!("{}:/cargo", cargo_home.display()))
        .arg("--volume")
        .arg(format!("{}:/rust:ro", rust_toolchain.display()))
        .arg("--volume")
        .arg(format!("{}:/project", snapshot.display()))
        .args([
            "--workdir",
            "/project/apps/framebuffer-scene-lab",
            image,
            "cargo",
        ])
        .args(cargo_arguments);
    command
}

fn publish_profile_lock(destination: &Path, contents: &[u8]) -> AgentResult<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("profile lock has no parent: {}", destination.display()))?;
    let temporary = parent.join(format!(".Cargo.lock.agent-{}", std::process::id()));
    let result = (|| -> AgentResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("create temporary profile lock: {error}"))?;
        file.write_all(contents)
            .map_err(|error| format!("write temporary profile lock: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("sync temporary profile lock: {error}"))?;
        std::fs::rename(&temporary, destination)
            .map_err(|error| format!("publish generated profile lock: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(prefix: &str) -> AgentResult<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("read clock for temporary directory: {error}"))?
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path)
            .map_err(|error| format!("create temporary directory {}: {error}", path.display()))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
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
        DeviceSceneLabScene::Magik | DeviceSceneLabScene::Cabinet => {
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
    }
    if let Some(case) = args.case.as_deref() {
        if args.scene != DeviceSceneLabScene::Cabinet {
            return Err("scene-lab --case is valid only for the cabinet scene".into());
        }
        if !CABINET_CASES.contains(&case) {
            return Err(format!("unknown closed cabinet case {case:?}").into());
        }
    }
    if args.profile && args.case.is_none() {
        return Err("scene-lab --profile requires a closed cabinet --case".into());
    }
    let spec = if args.profile {
        BuildSpec::framebuffer_scene_lab_analysis()
    } else {
        BuildSpec::framebuffer_scene_lab_device()
    };
    execute(repository, &spec, reporter)?;
    crate::commands::device::run_scene_lab(args, &repository.join(spec.artifact()))
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
            if args.recipe.is_some() {
                return Err("navigation-transition does not accept --recipe".into());
            }
            let fixture = args
                .fixture
                .as_deref()
                .ok_or("navigation-transition requires --fixture")?;
            validate_navigation_fixture(fixture)?;
            run_preview(repository, args.scene.label(), None, Some(fixture))
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

#[cfg(test)]
mod tests {
    use super::profile_lock_container_command;
    use std::path::Path;

    #[test]
    fn profile_lock_command_mounts_the_toolchain_and_uses_offline_resolution() {
        let command = profile_lock_container_command(
            Path::new("/tmp/snapshot"),
            Path::new("/tmp/cargo"),
            Path::new("/tmp/rust"),
            "test-image",
            &["generate-lockfile", "--offline"],
        );
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--volume", "/tmp/cargo:/cargo"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--volume", "/tmp/rust:/rust:ro"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--volume", "/tmp/snapshot:/project"])
        );
        assert!(arguments.ends_with(&[
            "test-image".into(),
            "cargo".into(),
            "generate-lockfile".into(),
            "--offline".into(),
        ]));
    }
}
