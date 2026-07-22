// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::deploy::UiScope;
use crate::progress::{EventKind, Reporter};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const TARGET: &str = "armv7-unknown-linux-gnueabihf";
const IMAGE: &str = "mister-magik-cross-armv7:ubuntu20-arm64";
const BUILD_DEADLINE: Duration = Duration::from_secs(30 * 60);
const FFMPEG_APPLE_CONTAINER_ENV: [(&str, &str); 5] = [
    (
        "FFMPEG_DIR",
        "/project/apps/mister/target/ffmpeg-minimal/armv7/dist",
    ),
    (
        "PKG_CONFIG_PATH",
        "/project/apps/mister/target/ffmpeg-minimal/armv7/dist/lib/pkgconfig",
    ),
    ("PKG_CONFIG_ALLOW_CROSS", "1"),
    (
        "CFLAGS",
        "-I/project/apps/mister/target/ffmpeg-minimal/armv7/dist/include",
    ),
    (
        "HOST_CFLAGS",
        "-I/project/apps/mister/target/ffmpeg-minimal/armv7/dist/include",
    ),
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum BuildCommand {
    RuntimeDevice,
    RuntimeFast,
    RuntimeProfile,
    ValidateLauncher,
    ValidateLibrary,
    DeviceAgent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildRecipe {
    RuntimeDevice(UiScope),
    RuntimeFast,
    RuntimeBenchmark,
    RuntimeProfile,
    ValidateLauncher,
    ValidateLibrary,
    DeviceAgent,
}

impl From<BuildCommand> for BuildRecipe {
    fn from(command: BuildCommand) -> Self {
        match command {
            BuildCommand::RuntimeDevice => Self::RuntimeDevice(UiScope::All),
            BuildCommand::RuntimeFast => Self::RuntimeFast,
            BuildCommand::RuntimeProfile => Self::RuntimeProfile,
            BuildCommand::ValidateLauncher => Self::ValidateLauncher,
            BuildCommand::ValidateLibrary => Self::ValidateLibrary,
            BuildCommand::DeviceAgent => Self::DeviceAgent,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildTarget {
    Runtime,
    DeviceAgent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildMode {
    Build,
    Check,
    CheckLibrary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildBackend {
    AppleContainer,
    Cross,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildSpec {
    target: BuildTarget,
    mode: BuildMode,
    profile: &'static str,
    features: Vec<&'static str>,
    ui_scope: UiScope,
    artifact: PathBuf,
    receipt: PathBuf,
    cache_identity: String,
    strict_receipt: bool,
}

impl BuildSpec {
    #[must_use]
    pub fn for_recipe(recipe: BuildRecipe) -> Self {
        let (target, mode, profile, features, scope, artifact, strict_receipt) = match recipe {
            BuildRecipe::RuntimeDevice(scope) => (
                BuildTarget::Runtime,
                BuildMode::Build,
                "release-device",
                vec!["ui"],
                scope,
                runtime_artifact("release-device"),
                true,
            ),
            BuildRecipe::RuntimeFast => (
                BuildTarget::Runtime,
                BuildMode::Build,
                "release",
                vec!["ui"],
                UiScope::All,
                runtime_artifact("release"),
                true,
            ),
            BuildRecipe::RuntimeBenchmark => (
                BuildTarget::Runtime,
                BuildMode::Build,
                "release-device",
                vec!["ui", "bench-tools"],
                UiScope::All,
                runtime_artifact("release-device"),
                true,
            ),
            BuildRecipe::RuntimeProfile => (
                BuildTarget::Runtime,
                BuildMode::Build,
                "release-device-profile",
                vec!["ui", "bench-tools", "profile"],
                UiScope::All,
                runtime_artifact("release-device-profile"),
                true,
            ),
            BuildRecipe::ValidateLauncher => (
                BuildTarget::Runtime,
                BuildMode::Check,
                "release-device",
                vec!["ui"],
                UiScope::Launcher,
                runtime_artifact("release-device"),
                false,
            ),
            BuildRecipe::ValidateLibrary => (
                BuildTarget::Runtime,
                BuildMode::CheckLibrary,
                "release-device",
                Vec::new(),
                UiScope::All,
                runtime_artifact("release-device"),
                false,
            ),
            BuildRecipe::DeviceAgent => (
                BuildTarget::DeviceAgent,
                BuildMode::Build,
                "release",
                Vec::new(),
                UiScope::All,
                PathBuf::from(
                    "mister/tools/agent/target/armv7-unknown-linux-gnueabihf/release/mister-magik-agent",
                ),
                false,
            ),
        };
        let receipt = PathBuf::from(format!("{}.build-receipt.tsv", artifact.display()));
        let cache_identity = format!(
            "v4:{TARGET}:{target:?}:{mode:?}:{profile}:{}:{}",
            features.join(","),
            scope.label()
        );
        Self {
            target,
            mode,
            profile,
            features,
            ui_scope: scope,
            artifact,
            receipt,
            cache_identity,
            strict_receipt,
        }
    }

    #[must_use]
    pub fn canonical(ui_scope: UiScope) -> Self {
        Self::for_recipe(BuildRecipe::RuntimeDevice(ui_scope))
    }

    #[must_use]
    pub fn artifact(&self) -> &Path {
        &self.artifact
    }

    #[must_use]
    pub fn features(&self) -> &[&'static str] {
        &self.features
    }

    #[must_use]
    pub const fn ui_scope(&self) -> UiScope {
        self.ui_scope
    }

    pub fn verify(&self, repository: &Path) -> Result<BuildReceipt, String> {
        if self.mode != BuildMode::Build {
            return Ok(BuildReceipt::empty(self));
        }
        let artifact = repository.join(&self.artifact);
        if !artifact.is_file() {
            return Err(format!("build artifact is missing: {}", artifact.display()));
        }
        if !self.strict_receipt {
            return Ok(BuildReceipt::empty(self));
        }
        let receipt_text = std::fs::read_to_string(repository.join(&self.receipt))
            .map_err(|error| format!("cannot read build receipt: {error}"))?;
        let receipt = BuildReceipt::parse(&receipt_text)?;
        if receipt.profile != self.profile
            || receipt.features != self.features.join(",")
            || receipt.ui_scope != self.ui_scope.label()
            || receipt.cache_identity != self.cache_identity
            || receipt.lock_sha256 != sha256(&repository.join(lockfile(self.target)))?
            || receipt.toolchain_sha256
                != sha256(&repository.join("apps/mister/rust-toolchain.toml"))?
        {
            return Err("build receipt does not match the inferred build".into());
        }
        if sha256(&artifact)? != receipt.binary_sha256 {
            return Err("build artifact checksum does not match its receipt".into());
        }
        Ok(receipt)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildReceipt {
    pub binary_sha256: String,
    pub profile: String,
    pub features: String,
    pub ui_scope: String,
    pub source_commit: String,
    pub source_dirty: bool,
    pub cache_identity: String,
    pub lock_sha256: String,
    pub toolchain_sha256: String,
}

impl BuildReceipt {
    fn empty(spec: &BuildSpec) -> Self {
        Self {
            binary_sha256: String::new(),
            profile: spec.profile.into(),
            features: spec.features.join(","),
            ui_scope: spec.ui_scope.label().into(),
            source_commit: String::new(),
            source_dirty: true,
            cache_identity: spec.cache_identity.clone(),
            lock_sha256: String::new(),
            toolchain_sha256: String::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let fields: BTreeMap<_, _> = text
            .trim()
            .split('\t')
            .skip(1)
            .filter_map(|field| field.split_once('='))
            .collect();
        let required = |name: &str| {
            fields
                .get(name)
                .map(|value| (*value).to_owned())
                .ok_or_else(|| format!("build receipt is missing {name}"))
        };
        let source_dirty = match required("source_dirty")?.as_str() {
            "0" => false,
            "1" => true,
            _ => return Err("build receipt has invalid source_dirty".into()),
        };
        Ok(Self {
            binary_sha256: required("binary_sha256")?,
            profile: required("profile")?,
            features: required("features")?,
            ui_scope: required("ui_scope")?,
            source_commit: required("source_commit")?,
            source_dirty,
            cache_identity: required("cache_identity")?,
            lock_sha256: required("lock_sha256")?,
            toolchain_sha256: required("toolchain_sha256")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Infer,
    Preflight,
    PrepareContainer,
    Compile,
    Verify,
    Receipt,
    Complete,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Infer => "infer",
            Self::Preflight => "preflight",
            Self::PrepareContainer => "prepare-container",
            Self::Compile => "compile",
            Self::Verify => "verify",
            Self::Receipt => "receipt",
            Self::Complete => "complete",
        }
    }
}

pub trait BuildActions {
    fn run(&mut self, phase: Phase) -> Result<(), String>;
}

pub fn run_state_machine(
    actions: &mut dyn BuildActions,
    progress: &mut dyn FnMut(Phase, u8) -> Result<(), String>,
) -> Result<(), String> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::Infer, 2),
        (Phase::Preflight, 8),
        (Phase::PrepareContainer, 18),
        (Phase::Compile, 35),
        (Phase::Verify, 82),
        (Phase::Receipt, 92),
        (Phase::Complete, 100),
    ];
    for (phase, percent) in PHASES {
        progress(*phase, *percent)?;
        actions
            .run(*phase)
            .map_err(|error| format!("{}: {error}", phase.label()))?;
    }
    Ok(())
}

pub fn execute(
    repository: &Path,
    spec: &BuildSpec,
    reporter: &mut Reporter<'_>,
) -> Result<(), String> {
    let mut actions = ProcessBuildActions::new(repository, spec)?;
    run_state_machine(&mut actions, &mut |phase, percent| {
        reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("build {}", phase.label()),
            Some(percent),
        )
    })
}

pub fn execute_quiet(repository: &Path, spec: &BuildSpec) -> Result<(), String> {
    let mut actions = ProcessBuildActions::new(repository, spec)?;
    run_state_machine(&mut actions, &mut |_, _| Ok(()))
}

struct ProcessBuildActions<'a> {
    repository: &'a Path,
    spec: &'a BuildSpec,
    backend: BuildBackend,
    target_dir: PathBuf,
}

impl<'a> ProcessBuildActions<'a> {
    fn new(repository: &'a Path, spec: &'a BuildSpec) -> Result<Self, String> {
        let backend = infer_backend()?;
        let target_dir = match spec.target {
            BuildTarget::DeviceAgent => {
                PathBuf::from("/private/tmp/mister-magik-agent-apple-container-target")
            }
            _ => PathBuf::from("/private/tmp/mister-magik-apple-container-target"),
        };
        Ok(Self {
            repository,
            spec,
            backend,
            target_dir,
        })
    }

    fn compile(&self) -> Result<(), String> {
        if self.spec.target == BuildTarget::Runtime && self.spec.mode != BuildMode::CheckLibrary {
            let mut ffmpeg = Command::new(
                self.repository
                    .join("apps/mister/scripts/build-minimal-ffmpeg.sh"),
            );
            ffmpeg.current_dir(self.repository.join("apps/mister"));
            run_bounded(&mut ffmpeg, BUILD_DEADLINE)?;
        }
        match self.backend {
            BuildBackend::AppleContainer => self.compile_in_apple_container(),
            BuildBackend::Cross => self.compile_with_cross(),
        }
    }

    fn compile_in_apple_container(&self) -> Result<(), String> {
        let cpus = std::thread::available_parallelism()
            .map_err(|error| format!("cannot detect online CPUs: {error}"))?
            .get()
            .to_string();
        let cargo_home = home_dir()?.join(".cargo");
        let rust_toolchain =
            home_dir()?.join(".rustup/toolchains/stable-aarch64-unknown-linux-gnu");
        let (build_number, version, build_time) = build_metadata(self.repository)?;
        let rustflags = if self.spec.profile == "release-device-profile" {
            "-D warnings -C target-cpu=cortex-a9 -C force-frame-pointers=yes"
        } else {
            "-D warnings -C target-cpu=cortex-a9"
        };
        let mut command = Command::new("container");
        command
            .current_dir(self.repository)
            .args(["run", "--arch", "arm64", "--rm", "--cpus"])
            .arg(&cpus)
            .args([
                "--memory",
                "8g",
                "--env",
                "CARGO_HOME=/cargo",
                "--env",
                "CARGO_TARGET_DIR=/target",
            ])
            .arg("--env")
            .arg(format!("CARGO_BUILD_JOBS={cpus}"))
            .arg("--env")
            .arg(format!(
                "MISTER_UI_BUILD_SCOPE={}",
                self.spec.ui_scope.label()
            ))
            .args(["--env", "RUSTC_WRAPPER=", "--env"])
            .arg(format!("RUSTFLAGS={rustflags}"))
            .args(["--env", "SLINT_FONT_SIZES=8,16,24,32"]);
        for value in [
            format!("MISTER_MAGIK_BUILD_NUMBER={build_number}"),
            format!("MISTER_MAGIK_VERSION={version}"),
            format!("MISTER_MAGIK_BUILD_TIME={build_time}"),
        ] {
            command.arg("--env").arg(value);
        }
        if self.spec.target == BuildTarget::Runtime && self.spec.mode != BuildMode::CheckLibrary {
            for (name, value) in FFMPEG_APPLE_CONTAINER_ENV {
                command.arg("--env").arg(format!("{name}={value}"));
            }
        }
        command
            .arg("--volume")
            .arg(format!("{}:/cargo", cargo_home.display()))
            .arg("--volume")
            .arg(format!("{}:/rust:ro", rust_toolchain.display()))
            .arg("--volume")
            .arg(format!("{}:/project", self.repository.display()))
            .arg("--volume")
            .arg(format!("{}:/target", self.target_dir.display()))
            .args([
                "--workdir",
                container_workdir(self.spec.target),
                IMAGE,
                "sh",
                "-lc",
                "PATH=/rust/bin:$PATH cargo \"$@\"",
                "sh",
            ])
            .args(cargo_args(self.spec));
        run_bounded(&mut command, BUILD_DEADLINE)
    }

    fn compile_with_cross(&self) -> Result<(), String> {
        let (build_number, version, build_time) = build_metadata(self.repository)?;
        let rustflags = if self.spec.profile == "release-device-profile" {
            "-D warnings -C target-cpu=cortex-a9 -C force-frame-pointers=yes"
        } else {
            "-D warnings -C target-cpu=cortex-a9"
        };
        let mut command = Command::new("cross");
        command
            .current_dir(self.repository.join(host_workdir(self.spec.target)))
            .env("RUSTC_WRAPPER", "")
            .env("RUSTFLAGS", rustflags)
            .env("MISTER_UI_BUILD_SCOPE", self.spec.ui_scope.label())
            .env("MISTER_MAGIK_BUILD_NUMBER", build_number)
            .env("MISTER_MAGIK_VERSION", version)
            .env("MISTER_MAGIK_BUILD_TIME", build_time)
            .args(cargo_args(self.spec));
        if self.spec.target == BuildTarget::Runtime && self.spec.mode != BuildMode::CheckLibrary {
            command.envs(ffmpeg_cross_env(self.repository));
        }
        run_bounded(&mut command, BUILD_DEADLINE)
    }

    fn mirror_artifact(&self) -> Result<(), String> {
        if self.spec.mode != BuildMode::Build || self.backend == BuildBackend::Cross {
            return Ok(());
        }
        let source = self.target_dir.join(TARGET).join(self.spec.profile).join(
            self.spec
                .artifact
                .file_name()
                .ok_or("build artifact has no filename")?,
        );
        if !source.is_file() {
            return Err(format!(
                "expected container output is missing: {}",
                source.display()
            ));
        }
        let destination = self.repository.join(&self.spec.artifact);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create artifact directory: {error}"))?;
        }
        std::fs::copy(&source, &destination)
            .map_err(|error| format!("cannot mirror build artifact: {error}"))?;
        Ok(())
    }

    fn write_receipt(&self) -> Result<(), String> {
        if self.spec.mode != BuildMode::Build || !self.spec.strict_receipt {
            return Ok(());
        }
        let artifact = self.repository.join(&self.spec.artifact);
        let commit = git_output(self.repository, &["rev-parse", "HEAD"])?;
        let dirty = !git_output(
            self.repository,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty();
        let receipt = format!(
            "build_receipt_tsv\tbinary_sha256={}\tprofile={}\tfeatures={}\tui_scope={}\tsource_commit={}\tsource_dirty={}\tcache_identity={}\tlock_sha256={}\ttoolchain_sha256={}\n",
            sha256(&artifact)?,
            self.spec.profile,
            self.spec.features.join(","),
            self.spec.ui_scope.label(),
            commit,
            u8::from(dirty),
            self.spec.cache_identity,
            sha256(&self.repository.join(lockfile(self.spec.target)))?,
            sha256(&self.repository.join("apps/mister/rust-toolchain.toml"))?,
        );
        let receipt_path = self.repository.join(&self.spec.receipt);
        let receipt_tmp = receipt_path.with_extension("tsv.tmp");
        std::fs::write(&receipt_tmp, receipt)
            .map_err(|error| format!("cannot write build receipt: {error}"))?;
        std::fs::rename(receipt_tmp, receipt_path)
            .map_err(|error| format!("cannot publish build receipt: {error}"))?;
        std::fs::write(
            format!("{}.features", artifact.display()),
            self.spec.features.join(","),
        )
        .map_err(|error| format!("cannot write build feature identity: {error}"))
    }
}

impl BuildActions for ProcessBuildActions<'_> {
    fn run(&mut self, phase: Phase) -> Result<(), String> {
        match phase {
            Phase::Infer | Phase::Complete => Ok(()),
            Phase::Preflight => preflight(self.backend),
            Phase::PrepareContainer => {
                if self.backend == BuildBackend::AppleContainer {
                    prepare_container(self.repository, &self.target_dir)
                } else {
                    Ok(())
                }
            }
            Phase::Compile => self.compile(),
            Phase::Verify => {
                self.mirror_artifact()?;
                if self.spec.mode == BuildMode::Build
                    && !self.repository.join(&self.spec.artifact).is_file()
                {
                    return Err("build completed without its expected output".into());
                }
                Ok(())
            }
            Phase::Receipt => self.write_receipt(),
        }
    }
}

fn infer_backend() -> Result<BuildBackend, String> {
    match std::env::var("MISTER_ARM_BUILD_BACKEND").as_deref() {
        Ok("cross") => return Ok(BuildBackend::Cross),
        Ok("apple-container") => return Ok(BuildBackend::AppleContainer),
        Ok(other) => return Err(format!("unsupported ARM build backend: {other}")),
        Err(_) => {}
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
        Ok(BuildBackend::Cross)
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok(BuildBackend::AppleContainer)
    } else {
        Err("ARM builds require Apple container locally; cross is reserved for explicit CI/operator comparison".into())
    }
}

fn preflight(backend: BuildBackend) -> Result<(), String> {
    let program = match backend {
        BuildBackend::AppleContainer => "container",
        BuildBackend::Cross => "cross",
    };
    let status = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("{program} is unavailable: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} preflight failed"))
    }
}

fn prepare_container(repository: &Path, target_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target_dir)
        .map_err(|error| format!("cannot create target cache: {error}"))?;
    let dockerfile = repository.join("apps/mister/Dockerfile.cross-armv7");
    let stamp_path = PathBuf::from(format!("{}.image.sha256", target_dir.display()));
    let expected = format!("{IMAGE}  {}", sha256(&dockerfile)?);
    let current = std::fs::read_to_string(&stamp_path).unwrap_or_default();
    let image_ok = Command::new("container")
        .args(["run", "--arch", "arm64", "--rm", IMAGE, "uname", "-m"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if current.trim() == expected && image_ok {
        return Ok(());
    }
    let mut build = Command::new("container");
    build.current_dir(repository.join("apps/mister")).args([
        "build",
        "--arch",
        "arm64",
        "--file",
        "Dockerfile.cross-armv7",
        "--tag",
        IMAGE,
        ".",
    ]);
    run_bounded(&mut build, BUILD_DEADLINE)?;
    std::fs::write(stamp_path, format!("{expected}\n"))
        .map_err(|error| format!("cannot update image stamp: {error}"))
}

fn cargo_args(spec: &BuildSpec) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![match spec.mode {
        BuildMode::Build => "build".into(),
        BuildMode::Check | BuildMode::CheckLibrary => "check".into(),
    }];
    args.extend(["--target".into(), TARGET.into(), "--locked".into()]);
    if spec.mode == BuildMode::Build {
        args.extend(["--profile".into(), spec.profile.into()]);
    }
    match spec.target {
        BuildTarget::Runtime | BuildTarget::DeviceAgent => {}
    }
    if spec.mode == BuildMode::CheckLibrary {
        args.extend(["--lib".into(), "--no-default-features".into()]);
    } else if !spec.features.is_empty() {
        args.extend(["--features".into(), spec.features.join(",").into()]);
    }
    args
}

fn host_workdir(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Runtime => "apps/mister",
        BuildTarget::DeviceAgent => "mister/tools/agent",
    }
}

fn container_workdir(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Runtime => "/project/apps/mister",
        BuildTarget::DeviceAgent => "/project/mister/tools/agent",
    }
}

fn lockfile(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Runtime => "apps/mister/Cargo.lock",
        BuildTarget::DeviceAgent => "mister/tools/agent/Cargo.lock",
    }
}

fn runtime_artifact(profile: &str) -> PathBuf {
    PathBuf::from(format!(
        "apps/mister/target/{TARGET}/{profile}/mister-magik-fb"
    ))
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unavailable for the build cache".into())
}

fn sha256(path: &Path) -> Result<String, String> {
    for (program, args) in [("shasum", vec!["-a", "256"]), ("sha256sum", Vec::new())] {
        if let Ok(output) = Command::new(program).args(args).arg(path).output() {
            if output.status.success() {
                if let Some(value) = String::from_utf8_lossy(&output.stdout)
                    .split_whitespace()
                    .next()
                {
                    return Ok(value.to_lowercase());
                }
            }
        }
    }
    Err(format!("cannot hash {}", path.display()))
}

fn git_output(repository: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| format!("cannot inspect Git build identity: {error}"))?;
    if !output.status.success() {
        return Err("cannot inspect Git build identity".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn ffmpeg_cross_env(repository: &Path) -> Vec<(&'static str, OsString)> {
    let dist = repository.join("apps/mister/target/ffmpeg-minimal/armv7/dist");
    let include = dist.join("include");
    vec![
        ("FFMPEG_DIR", dist.as_os_str().to_owned()),
        (
            "PKG_CONFIG_PATH",
            dist.join("lib/pkgconfig").into_os_string(),
        ),
        ("PKG_CONFIG_ALLOW_CROSS", OsString::from("1")),
        ("CFLAGS", OsString::from(format!("-I{}", include.display()))),
        (
            "HOST_CFLAGS",
            OsString::from(format!("-I{}", include.display())),
        ),
    ]
}

fn build_metadata(repository: &Path) -> Result<(String, String, String), String> {
    let build_number = std::env::var("MISTER_MAGIK_BUILD_NUMBER")
        .unwrap_or(git_output(repository, &["rev-list", "--count", "HEAD"])?);
    let version =
        std::env::var("MISTER_MAGIK_VERSION").unwrap_or_else(|_| format!("0.2.{build_number}"));
    let build_time = match std::env::var("MISTER_MAGIK_BUILD_TIME") {
        Ok(value) => value,
        Err(_) => git_output(
            repository,
            &[
                "show",
                "-s",
                "--format=%cd",
                "--date=format:%-d.%-m.%Y %H:%M",
                "HEAD",
            ],
        )?,
    };
    Ok((build_number, version, build_time))
}

fn run_bounded(command: &mut Command, deadline: Duration) -> Result<(), String> {
    let description = format!("{command:?}");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot start {description}: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("command exited with {status}")),
            Ok(None) if started.elapsed() < deadline => thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "command exceeded its {}s deadline",
                    deadline.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for build command: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeActions {
        fail_at: Option<Phase>,
        visited: Vec<Phase>,
    }

    impl BuildActions for FakeActions {
        fn run(&mut self, phase: Phase) -> Result<(), String> {
            self.visited.push(phase);
            if self.fail_at == Some(phase) {
                Err("injected failure".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn runtime_and_validation_intents_infer_fixed_identity() {
        let runtime = BuildSpec::for_recipe(BuildRecipe::RuntimeDevice(UiScope::All));
        assert_eq!(runtime.profile, "release-device");
        assert_eq!(runtime.features, ["ui"]);
        assert_eq!(runtime.ui_scope, UiScope::All);
        let launcher = BuildSpec::for_recipe(BuildRecipe::ValidateLauncher);
        assert_eq!(launcher.mode, BuildMode::Check);
        assert_eq!(launcher.ui_scope, UiScope::Launcher);
        let library = BuildSpec::for_recipe(BuildRecipe::ValidateLibrary);
        assert_eq!(library.mode, BuildMode::CheckLibrary);
    }

    #[test]
    fn cache_identity_changes_with_profile_scope_and_target() {
        let device = BuildSpec::for_recipe(BuildRecipe::RuntimeDevice(UiScope::All));
        let fast = BuildSpec::for_recipe(BuildRecipe::RuntimeFast);
        let agent = BuildSpec::for_recipe(BuildRecipe::DeviceAgent);
        assert_ne!(device.cache_identity, fast.cache_identity);
        assert_ne!(device.cache_identity, agent.cache_identity);
        assert_ne!(
            device.cache_identity,
            BuildSpec::canonical(UiScope::Launcher).cache_identity
        );
        assert!(BuildSpec::canonical(UiScope::Launcher)
            .cache_identity
            .ends_with(":launcher"));
        assert!(!BuildSpec::canonical(UiScope::Launcher)
            .cache_identity
            .contains(":all:launcher"));
    }

    #[test]
    fn cross_runtime_receives_minimal_ffmpeg_environment() {
        let environment = ffmpeg_cross_env(Path::new("/checkout"));
        assert!(environment.contains(&(
            "PKG_CONFIG_PATH",
            OsString::from("/checkout/apps/mister/target/ffmpeg-minimal/armv7/dist/lib/pkgconfig")
        )));
        assert!(environment.contains(&("PKG_CONFIG_ALLOW_CROSS", OsString::from("1"))));
    }

    #[test]
    fn state_machine_stops_at_every_failure_boundary() {
        let phases = [
            Phase::Infer,
            Phase::Preflight,
            Phase::PrepareContainer,
            Phase::Compile,
            Phase::Verify,
            Phase::Receipt,
            Phase::Complete,
        ];
        for (index, phase) in phases.iter().enumerate() {
            let mut actions = FakeActions {
                fail_at: Some(*phase),
                visited: Vec::new(),
            };
            let error = run_state_machine(&mut actions, &mut |_, _| Ok(())).unwrap_err();
            assert!(error.starts_with(phase.label()));
            assert_eq!(actions.visited, phases[..=index]);
        }
    }

    #[test]
    fn receipt_requires_artifact_and_cache_identity() {
        let valid = "build_receipt_tsv\tbinary_sha256=abc\tprofile=release-device\tfeatures=ui\tui_scope=all\tsource_commit=deadbeef\tsource_dirty=0\tcache_identity=v3\tlock_sha256=lock\ttoolchain_sha256=toolchain\n";
        assert_eq!(BuildReceipt::parse(valid).unwrap().cache_identity, "v3");
        assert!(BuildReceipt::parse(valid.replace("\tcache_identity=v3", "").as_str()).is_err());
    }
}
