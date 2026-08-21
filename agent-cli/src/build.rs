// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::deploy::UiScope;
use crate::error::AgentResult;
use crate::progress::{EventKind, Reporter};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TARGET: &str = "armv7-unknown-linux-gnueabihf";
const IMAGE: &str = "mister-magik-cross-armv7:ubuntu20-arm64";
const IMAGE_STAMP: &str = "/private/tmp/mister-magik-apple-container-image.tsv";
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
    RuntimeCi,
    RuntimeAnalysis,
    ValidateLauncher,
    ValidateLibrary,
    ValidateRuntime,
    DeviceAgent,
    DeviceAgentCi,
    ManagerDevice,
    FramebufferLabDevice,
    FramebufferSceneLabDevice,
    FramebufferSceneLabAnalysis,
    ReleaseBinaries,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildTarget {
    Runtime,
    DeviceAgent,
    Manager,
    FramebufferLab,
    FramebufferSceneLab,
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
}

impl BuildSpec {
    #[must_use]
    pub fn for_command(command: BuildCommand) -> Option<Self> {
        let configuration = match command {
            BuildCommand::RuntimeDevice => (
                BuildTarget::Runtime,
                BuildMode::Build,
                "release-device",
                vec!["ui", "profile"],
                UiScope::All,
                runtime_artifact("release-device"),
            ),
            BuildCommand::RuntimeCi => (
                BuildTarget::Runtime,
                BuildMode::Build,
                "ci-fast",
                vec!["ui"],
                UiScope::All,
                runtime_artifact("ci-fast"),
            ),
            BuildCommand::RuntimeAnalysis => (
                BuildTarget::Runtime,
                BuildMode::Build,
                "release-device-profile",
                vec!["ui", "profile"],
                UiScope::All,
                runtime_artifact("release-device-profile"),
            ),
            BuildCommand::ValidateLauncher | BuildCommand::ValidateRuntime => (
                BuildTarget::Runtime,
                BuildMode::Check,
                "release-device",
                vec!["ui"],
                UiScope::Launcher,
                runtime_artifact("release-device"),
            ),
            BuildCommand::ValidateLibrary => (
                BuildTarget::Runtime,
                BuildMode::CheckLibrary,
                "release-device",
                Vec::new(),
                UiScope::All,
                runtime_artifact("release-device"),
            ),
            BuildCommand::DeviceAgent => (
                BuildTarget::DeviceAgent,
                BuildMode::Build,
                "release",
                Vec::new(),
                UiScope::All,
                PathBuf::from(
                    "mister/tools/agent/target/armv7-unknown-linux-gnueabihf/release/mister-magik-agent",
                ),
            ),
            BuildCommand::DeviceAgentCi => (
                BuildTarget::DeviceAgent,
                BuildMode::Build,
                "ci-fast",
                Vec::new(),
                UiScope::All,
                PathBuf::from(
                    "mister/tools/agent/target/armv7-unknown-linux-gnueabihf/ci-fast/mister-magik-agent",
                ),
            ),
            BuildCommand::ManagerDevice => (
                BuildTarget::Manager,
                BuildMode::Build,
                "release",
                Vec::new(),
                UiScope::All,
                PathBuf::from(
                    "mister/tools/manager/target/armv7-unknown-linux-gnueabihf/release/mister-magik-manager",
                ),
            ),
            BuildCommand::FramebufferLabDevice => (
                BuildTarget::FramebufferLab,
                BuildMode::Build,
                "release-live",
                Vec::new(),
                UiScope::All,
                framebuffer_lab_artifact("release-live"),
            ),
            BuildCommand::FramebufferSceneLabDevice => (
                BuildTarget::FramebufferSceneLab,
                BuildMode::Build,
                "release-device",
                vec!["profile"],
                UiScope::All,
                framebuffer_scene_lab_artifact("release-device"),
            ),
            BuildCommand::FramebufferSceneLabAnalysis => (
                BuildTarget::FramebufferSceneLab,
                BuildMode::Build,
                "release-device-profile",
                vec!["profile"],
                UiScope::All,
                framebuffer_scene_lab_artifact("release-device-profile"),
            ),
            BuildCommand::ReleaseBinaries => return None,
        };
        Some(Self::from_configuration(configuration))
    }

    fn from_configuration(
        (target, mode, profile, features, scope, artifact): (
            BuildTarget,
            BuildMode,
            &'static str,
            Vec<&'static str>,
            UiScope,
            PathBuf,
        ),
    ) -> Self {
        let receipt = PathBuf::from(format!("{}.build-receipt.tsv", artifact.display()));
        let cache_identity = format!(
            "v5:{TARGET}:{target:?}:{mode:?}:{profile}:{}:{}",
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
        }
    }

    #[must_use]
    pub fn canonical(ui_scope: UiScope) -> Self {
        Self::canonical_profile(ui_scope, "release-device")
    }

    #[must_use]
    pub fn canonical_profile(ui_scope: UiScope, profile: &'static str) -> Self {
        let mut spec = Self::for_command(BuildCommand::RuntimeDevice)
            .expect("runtime device builds have a specification");
        spec.profile = profile;
        spec.artifact = runtime_artifact(profile);
        spec.receipt = PathBuf::from(format!("{}.build-receipt.tsv", spec.artifact.display()));
        spec.ui_scope = ui_scope;
        spec.cache_identity = format!(
            "v5:{TARGET}:{:?}:{:?}:{}:{}:{}",
            spec.target,
            spec.mode,
            spec.profile,
            spec.features.join(","),
            ui_scope.label()
        );
        spec
    }

    #[must_use]
    pub fn framebuffer_lab_device() -> Self {
        Self::for_command(BuildCommand::FramebufferLabDevice)
            .expect("framebuffer lab device builds have a specification")
    }

    #[must_use]
    pub fn framebuffer_scene_lab_device() -> Self {
        Self::for_command(BuildCommand::FramebufferSceneLabDevice)
            .expect("startup particle lab device builds have a specification")
    }

    #[must_use]
    pub fn framebuffer_scene_lab_analysis() -> Self {
        Self::for_command(BuildCommand::FramebufferSceneLabAnalysis)
            .expect("startup particle analysis builds have a specification")
    }

    /// Reproduces the current full Slint application build used to quantify
    /// the cost of editing the production Magik particle engine. The build is
    /// build-only and never installs or runs the artifact.
    #[must_use]
    pub fn magik_full_app_baseline() -> Self {
        Self::from_configuration((
            BuildTarget::Runtime,
            BuildMode::Build,
            "release-live",
            vec!["ui"],
            UiScope::Launcher,
            runtime_artifact("release-live"),
        ))
    }

    /// Reproduces the former pull-request ARM build for revision comparisons.
    #[must_use]
    pub fn runtime_release_baseline() -> Self {
        Self::from_configuration((
            BuildTarget::Runtime,
            BuildMode::Build,
            "release",
            vec!["ui"],
            UiScope::All,
            runtime_artifact("release"),
        ))
    }

    #[must_use]
    pub fn runtime_ci() -> Self {
        Self::for_command(BuildCommand::RuntimeCi)
            .expect("ordinary ARM CI builds have a specification")
    }

    #[must_use]
    pub const fn profile(&self) -> &'static str {
        self.profile
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

    pub fn verify(&self, repository: &Path) -> AgentResult<BuildReceipt> {
        let metadata = build_metadata(repository)?;
        self.verify_with_metadata(repository, &metadata)
    }

    fn verify_with_metadata(
        &self,
        repository: &Path,
        metadata: &BuildMetadata,
    ) -> AgentResult<BuildReceipt> {
        if self.mode != BuildMode::Build {
            return Err("validation checks do not produce build receipts".into());
        }
        let artifact = repository.join(&self.artifact);
        if !artifact.is_file() {
            return Err(format!("build artifact is missing: {}", artifact.display()).into());
        }
        let receipt_text = std::fs::read_to_string(repository.join(&self.receipt))
            .map_err(|error| format!("cannot read build receipt: {error}"))?;
        let receipt = BuildReceipt::parse(&receipt_text)?;
        if receipt.profile != self.profile
            || receipt.features != self.features.join(",")
            || receipt.ui_scope != self.ui_scope.label()
            || receipt.build_number != metadata.build_number
            || receipt.version != metadata.version
            || receipt.source_commit != metadata.source_revision
            || receipt.source_dirty != metadata.source_dirty
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
    pub build_number: String,
    pub version: String,
    pub source_commit: String,
    pub source_dirty: bool,
    pub cache_identity: String,
    pub lock_sha256: String,
    pub toolchain_sha256: String,
}

impl BuildReceipt {
    pub fn parse(text: &str) -> AgentResult<Self> {
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
            build_number: required("build_number")?,
            version: required("version")?,
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
    fn run(&mut self, phase: Phase) -> AgentResult<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BuildTimingSample {
    pub(crate) phase: &'static str,
    pub(crate) status: crate::host::DeliveryTimingStatus,
    pub(crate) elapsed_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BuildArtifactFile {
    pub(crate) path: String,
    pub(crate) bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BuildArtifactAttribution {
    pub(crate) artifact: String,
    pub(crate) artifact_bytes: u64,
    pub(crate) release_dir_bytes: u64,
    pub(crate) largest_files: Vec<BuildArtifactFile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BuildTimingReport {
    pub(crate) samples: Vec<BuildTimingSample>,
    pub(crate) attribution: Option<BuildArtifactAttribution>,
}

pub fn run_state_machine(
    actions: &mut dyn BuildActions,
    progress: &mut dyn FnMut(Phase, u8) -> AgentResult<()>,
) -> AgentResult<()> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::Infer, 2),
        (Phase::Preflight, 8),
        (Phase::PrepareContainer, 18),
        (Phase::Compile, 35),
        (Phase::Verify, 82),
        (Phase::Receipt, 92),
        (Phase::Complete, 100),
    ];
    crate::workflow::run_phases(
        actions,
        PHASES,
        progress,
        |actions, phase| actions.run(phase),
        Phase::label,
    )
}

pub fn execute(
    repository: &Path,
    spec: &BuildSpec,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    let mut session = BuildSession::new(repository)?;
    execute_with_session(&mut session, spec, reporter)
}

fn execute_with_session(
    session: &mut BuildSession<'_>,
    spec: &BuildSpec,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    let mut actions = ProcessBuildActions::new(session, spec);
    let result = run_state_machine(&mut actions, &mut |phase, percent| {
        Ok(reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("build {}", phase.label()),
            Some(percent),
        )?)
    });
    let _ = emit_build_timings(reporter, &actions.timings);
    if let Some(attribution) = actions.attribution.as_ref() {
        let _ = emit_build_attribution(reporter, attribution);
    }
    result
}

pub fn execute_quiet(repository: &Path, spec: &BuildSpec) -> AgentResult<()> {
    let mut session = BuildSession::new(repository)?;
    let mut actions = ProcessBuildActions::new(&mut session, spec);
    run_state_machine(&mut actions, &mut |_, _| Ok(()))
}

pub(crate) fn execute_quiet_with_timings(
    repository: &Path,
    spec: &BuildSpec,
) -> AgentResult<BuildTimingReport> {
    let mut session = BuildSession::new(repository)?;
    let mut actions = ProcessBuildActions::new(&mut session, spec);
    run_state_machine(&mut actions, &mut |_, _| Ok(()))?;
    Ok(BuildTimingReport {
        samples: actions.timings,
        attribution: actions.attribution,
    })
}

/// Executes a build in an explicit Cargo target directory. This is reserved
/// for reproducible compile-time measurements and retains the normal typed
/// preflight, Apple-container, artifact, and receipt checks.
pub fn execute_quiet_at_target_dir(
    repository: &Path,
    spec: &BuildSpec,
    target_dir: &Path,
) -> AgentResult<()> {
    let mut session = BuildSession::new(repository)?;
    let mut actions = ProcessBuildActions::new_at_target_dir(&mut session, spec, target_dir)?;
    run_state_machine(&mut actions, &mut |_, _| Ok(()))
}

pub fn execute_command(
    repository: &Path,
    command: BuildCommand,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    match command {
        BuildCommand::ValidateRuntime => execute_runtime_validation(repository, reporter),
        BuildCommand::ReleaseBinaries => execute_release_binaries(repository, reporter),
        other => execute(
            repository,
            &BuildSpec::for_command(other).ok_or("build command has no single specification")?,
            reporter,
        ),
    }
}

fn execute_release_binaries(repository: &Path, reporter: &mut Reporter<'_>) -> AgentResult<()> {
    let runtime = BuildSpec::canonical(UiScope::All);
    let manager = BuildSpec::for_command(BuildCommand::ManagerDevice)
        .expect("manager builds have a specification");
    let mut session = BuildSession::new(repository)?;
    let manager_receipt = session.reusable_clean_receipt(&manager).ok();
    execute_with_session(&mut session, &runtime, reporter)?;
    if manager_receipt.is_some() {
        reporter.emit(
            EventKind::Completed,
            "manager-cache-hit",
            "reused exact clean manager build receipt",
            Some(100),
        )?;
        return Ok(());
    }
    execute_with_session(&mut session, &manager, reporter)
}

pub fn execute_runtime_validation(
    repository: &Path,
    reporter: &mut Reporter<'_>,
) -> AgentResult<()> {
    reporter.emit(
        EventKind::Progress,
        "prepare-container",
        "preparing shared runtime validation",
        Some(10),
    )?;
    let mut session = BuildSession::new(repository)?;
    session.ensure_preflight()?;
    if session.backend == BuildBackend::Cross {
        build_minimal_ffmpeg(repository, session.backend, FfmpegVerification::Full)?;
        execute_with_session(
            &mut session,
            &BuildSpec::for_command(BuildCommand::ValidateLauncher)
                .expect("launcher validation has a specification"),
            reporter,
        )?;
        return execute_with_session(
            &mut session,
            &BuildSpec::for_command(BuildCommand::ValidateLibrary)
                .expect("library validation has a specification"),
            reporter,
        );
    }
    let target_dir = PathBuf::from("/private/tmp/mister-magik-apple-container-target");
    create_target_dir(&target_dir)?;
    session.ensure_container_ready()?;
    build_minimal_ffmpeg(repository, session.backend, FfmpegVerification::Full)?;
    let cpus = std::thread::available_parallelism()
        .map_err(|error| format!("cannot detect online CPUs: {error}"))?
        .get()
        .to_string();
    let cargo_home = home_dir()?.join(".cargo");
    let rust_toolchain = home_dir()?.join(".rustup/toolchains/stable-aarch64-unknown-linux-gnu");
    let mut command = Command::new("container");
    command
        .current_dir(repository)
        .args([
            "run",
            "--progress",
            "none",
            "--arch",
            "arm64",
            "--rm",
            "--cpus",
        ])
        .arg(&cpus)
        .args([
            "--memory",
            "8g",
            "--env",
            "CARGO_HOME=/cargo",
            "--env",
            "CARGO_TARGET_DIR=/target",
            "--env",
            "RUSTC_WRAPPER=",
            "--env",
            "RUSTFLAGS=-D warnings -C target-cpu=cortex-a9",
            "--env",
            "SLINT_FONT_SIZES=8,16,24,32",
        ])
        .arg("--env")
        .arg(format!("CARGO_BUILD_JOBS={cpus}"));
    for value in session.metadata.environment() {
        command.arg("--env").arg(value);
    }
    for (name, value) in FFMPEG_APPLE_CONTAINER_ENV {
        command.arg("--env").arg(format!("{name}={value}"));
    }
    command
        .arg("--volume")
        .arg(format!("{}:/cargo", cargo_home.display()))
        .arg("--volume")
        .arg(format!("{}:/rust:ro", rust_toolchain.display()))
        .arg("--volume")
        .arg(format!("{}:/project", repository.display()))
        .arg("--volume")
        .arg(format!("{}:/target", target_dir.display()))
        .args([
            "--workdir",
            "/project/apps/mister",
            IMAGE,
            "sh",
            "-lc",
            "PATH=/rust/bin:$PATH MISTER_UI_BUILD_SCOPE=launcher cargo check --target armv7-unknown-linux-gnueabihf --locked --features ui && PATH=/rust/bin:$PATH MISTER_UI_BUILD_SCOPE=all cargo check --target armv7-unknown-linux-gnueabihf --locked --lib --no-default-features",
        ]);
    reporter.emit(
        EventKind::Progress,
        "compile",
        "checking launcher and library in one container",
        Some(35),
    )?;
    run_bounded(&mut command, BUILD_DEADLINE)?;
    reporter.emit(
        EventKind::Completed,
        "complete",
        "combined runtime validation passed",
        Some(100),
    )?;
    Ok(())
}

struct BuildSession<'a> {
    repository: &'a Path,
    backend: BuildBackend,
    metadata: BuildMetadata,
    cargo_timings: bool,
    preflight_complete: bool,
    container_ready: bool,
}

impl<'a> BuildSession<'a> {
    fn new(repository: &'a Path) -> AgentResult<Self> {
        Ok(Self {
            repository,
            backend: infer_backend()?,
            metadata: build_metadata(repository)?,
            cargo_timings: requested_cargo_timings()?,
            preflight_complete: false,
            container_ready: false,
        })
    }

    fn ensure_preflight(&mut self) -> AgentResult<()> {
        ensure_once(&mut self.preflight_complete, || preflight(self.backend))
    }

    fn ensure_container_ready(&mut self) -> AgentResult<()> {
        if self.backend != BuildBackend::AppleContainer {
            return Ok(());
        }
        ensure_once(&mut self.container_ready, || {
            prepare_container_image(self.repository)
        })
    }

    fn ensure_source_identity(&self) -> AgentResult<()> {
        let source_revision = git_output(self.repository, &["rev-parse", "HEAD"])?;
        let source_status = git_output(
            self.repository,
            &["status", "--porcelain", "--untracked-files=all"],
        )?;
        validate_source_identity(&self.metadata, &source_revision, &source_status)
    }

    fn reusable_clean_receipt(&self, spec: &BuildSpec) -> AgentResult<BuildReceipt> {
        if self.metadata.source_dirty {
            return Err("dirty builds cannot reuse a strict receipt".into());
        }
        let receipt = spec.verify_with_metadata(self.repository, &self.metadata)?;
        if receipt.source_dirty {
            return Err("dirty build receipt cannot be reused".into());
        }
        Ok(receipt)
    }
}

fn validate_source_identity(
    metadata: &BuildMetadata,
    source_revision: &str,
    source_status: &str,
) -> AgentResult<()> {
    let source_dirty = !source_status.is_empty();
    if source_revision != metadata.source_revision || source_dirty != metadata.source_dirty {
        let changed_paths = source_status
            .lines()
            .take(20)
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(format!(
            "source identity changed during the build: expected_revision={} actual_revision={source_revision} expected_dirty={} actual_dirty={source_dirty} changes={}",
            metadata.source_revision,
            metadata.source_dirty,
            if changed_paths.is_empty() {
                "none"
            } else {
                &changed_paths
            }
        )
        .into());
    }
    Ok(())
}

fn ensure_once(completed: &mut bool, action: impl FnOnce() -> AgentResult<()>) -> AgentResult<()> {
    if *completed {
        return Ok(());
    }
    action()?;
    *completed = true;
    Ok(())
}

struct ProcessBuildActions<'session, 'repository, 'spec> {
    session: &'session mut BuildSession<'repository>,
    spec: &'spec BuildSpec,
    target_dir: PathBuf,
    timings: Vec<BuildTimingSample>,
    attribution: Option<BuildArtifactAttribution>,
}

impl<'session, 'repository, 'spec> ProcessBuildActions<'session, 'repository, 'spec> {
    fn new(session: &'session mut BuildSession<'repository>, spec: &'spec BuildSpec) -> Self {
        let target_dir = match spec.target {
            BuildTarget::DeviceAgent => {
                PathBuf::from("/private/tmp/mister-magik-agent-apple-container-target")
            }
            BuildTarget::Manager => {
                PathBuf::from("/private/tmp/mister-magik-manager-apple-container-target")
            }
            BuildTarget::FramebufferLab => {
                PathBuf::from("/private/tmp/mister-magik-framebuffer-lab-apple-container-target")
            }
            BuildTarget::FramebufferSceneLab => PathBuf::from(
                "/private/tmp/mister-magik-framebuffer-scene-lab-apple-container-target",
            ),
            _ => PathBuf::from("/private/tmp/mister-magik-apple-container-target"),
        };
        Self {
            session,
            spec,
            target_dir,
            timings: Vec::new(),
            attribution: None,
        }
    }

    fn new_at_target_dir(
        session: &'session mut BuildSession<'repository>,
        spec: &'spec BuildSpec,
        target_dir: &Path,
    ) -> AgentResult<Self> {
        if !target_dir.is_absolute() {
            return Err("compile-time target directory must be absolute".into());
        }
        if target_dir == Path::new("/") || target_dir == session.repository {
            return Err("compile-time target directory is too broad".into());
        }
        Ok(Self {
            session,
            spec,
            target_dir: target_dir.to_path_buf(),
            timings: Vec::new(),
            attribution: None,
        })
    }

    fn compile(&mut self) -> AgentResult<()> {
        if self.spec.target == BuildTarget::Runtime && self.spec.mode != BuildMode::CheckLibrary {
            let started = Instant::now();
            let result = build_minimal_ffmpeg(
                self.session.repository,
                self.session.backend,
                FfmpegVerification::Stamp,
            );
            self.record_timing("compile.ffmpeg-cache", started, &result);
            result?;
        }
        match self.session.backend {
            BuildBackend::AppleContainer => {
                let started = Instant::now();
                let result = self.compile_in_apple_container();
                self.record_timing("compile.cargo", started, &result);
                result
            }
            BuildBackend::Cross => {
                let started = Instant::now();
                let result = self.compile_with_cross();
                self.record_timing("compile.cargo", started, &result);
                result
            }
        }
    }

    fn record_timing(&mut self, phase: &'static str, started: Instant, result: &AgentResult<()>) {
        self.timings.push(BuildTimingSample {
            phase,
            status: if result.is_ok() {
                crate::host::DeliveryTimingStatus::Passed
            } else {
                crate::host::DeliveryTimingStatus::Failed
            },
            elapsed_ms: elapsed_millis(started),
        });
    }

    fn compile_in_apple_container(&self) -> AgentResult<()> {
        let mut command = apple_container_cargo_command(
            self.session.repository,
            self.spec,
            &self.target_dir,
            &self.session.metadata,
            self.session.cargo_timings,
        )?;
        run_bounded(&mut command, BUILD_DEADLINE)
    }

    fn compile_with_cross(&self) -> AgentResult<()> {
        let metadata = &self.session.metadata;
        let rustflags = if self.spec.features.contains(&"profile") {
            "-D warnings -C target-cpu=cortex-a9 -C force-frame-pointers=yes"
        } else {
            "-D warnings -C target-cpu=cortex-a9"
        };
        let mut command = Command::new("cross");
        command
            .current_dir(self.session.repository.join(host_workdir(self.spec.target)))
            .env("RUSTC_WRAPPER", "")
            .env("RUSTFLAGS", rustflags)
            .env("MISTER_UI_BUILD_SCOPE", self.spec.ui_scope.label())
            .env("MISTER_MAGIK_BUILD_NUMBER", &metadata.build_number)
            .env("MISTER_MAGIK_VERSION", &metadata.version)
            .env("MISTER_MAGIK_BUILD_TIME", &metadata.build_time)
            .env("MISTER_MAGIK_SOURCE_REVISION", &metadata.source_revision)
            .env(
                "MISTER_MAGIK_SOURCE_DIRTY",
                u8::from(metadata.source_dirty).to_string(),
            )
            .args(cargo_args(self.spec, self.session.cargo_timings));
        configure_cross_environment(&mut command, self.session.repository)?;
        if self.spec.target == BuildTarget::Runtime && self.spec.mode != BuildMode::CheckLibrary {
            command.envs(ffmpeg_cross_env(self.session.repository));
        }
        run_bounded(&mut command, BUILD_DEADLINE)
    }

    fn mirror_artifact(&self) -> AgentResult<()> {
        if self.spec.mode != BuildMode::Build || self.session.backend == BuildBackend::Cross {
            return Ok(());
        }
        let source = self.target_dir.join(TARGET).join(self.spec.profile).join(
            self.spec
                .artifact
                .file_name()
                .ok_or("build artifact has no filename")?,
        );
        if !source.is_file() {
            return Err(
                format!("expected container output is missing: {}", source.display()).into(),
            );
        }
        let destination = self.session.repository.join(&self.spec.artifact);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create artifact directory: {error}"))?;
        }
        std::fs::copy(&source, &destination)
            .map_err(|error| format!("cannot mirror build artifact: {error}"))?;
        Ok(())
    }

    fn record_artifact_attribution(&mut self) {
        if self.spec.mode != BuildMode::Build {
            return;
        }
        let artifact = self.session.repository.join(&self.spec.artifact);
        let release_dir = self.target_dir.join(TARGET).join(self.spec.profile);
        let Ok(artifact_bytes) = std::fs::metadata(&artifact).map(|metadata| metadata.len()) else {
            return;
        };
        let mut files = Vec::new();
        collect_files(&release_dir, &release_dir, &mut files);
        let release_dir_bytes = files.iter().map(|file| file.bytes).sum();
        files.sort_by_key(|file| std::cmp::Reverse(file.bytes));
        files.truncate(12);
        self.attribution = Some(BuildArtifactAttribution {
            artifact: self.spec.artifact.display().to_string(),
            artifact_bytes,
            release_dir_bytes,
            largest_files: files,
        });
    }

    fn write_receipt(&self) -> AgentResult<()> {
        if self.spec.mode != BuildMode::Build {
            return Ok(());
        }
        self.session.ensure_source_identity()?;
        let artifact = self.session.repository.join(&self.spec.artifact);
        let metadata = &self.session.metadata;
        let receipt = format!(
            "build_receipt_tsv\tbinary_sha256={}\tprofile={}\tfeatures={}\tui_scope={}\tbuild_number={}\tversion={}\tsource_commit={}\tsource_dirty={}\tcache_identity={}\tlock_sha256={}\ttoolchain_sha256={}\n",
            sha256(&artifact)?,
            self.spec.profile,
            self.spec.features.join(","),
            self.spec.ui_scope.label(),
            metadata.build_number,
            metadata.version,
            metadata.source_revision,
            u8::from(metadata.source_dirty),
            self.spec.cache_identity,
            sha256(&self.session.repository.join(lockfile(self.spec.target)))?,
            sha256(
                &self
                    .session
                    .repository
                    .join("apps/mister/rust-toolchain.toml"),
            )?,
        );
        let receipt_path = self.session.repository.join(&self.spec.receipt);
        let receipt_tmp = receipt_path.with_extension("tsv.tmp");
        std::fs::write(&receipt_tmp, receipt)
            .map_err(|error| format!("cannot write build receipt: {error}"))?;
        std::fs::rename(receipt_tmp, receipt_path)
            .map_err(|error| format!("cannot publish build receipt: {error}"))?;
        std::fs::write(
            format!("{}.features", artifact.display()),
            self.spec.features.join(","),
        )
        .map_err(|error| format!("cannot write build feature identity: {error}"))?;
        Ok(())
    }
}

fn apple_container_cargo_command(
    repository: &Path,
    spec: &BuildSpec,
    target_dir: &Path,
    metadata: &BuildMetadata,
    cargo_timings: bool,
) -> AgentResult<Command> {
    let cpus = std::thread::available_parallelism()
        .map_err(|error| format!("cannot detect online CPUs: {error}"))?
        .get()
        .to_string();
    let cargo_home = home_dir()?.join(".cargo");
    let rust_toolchain = home_dir()?.join(".rustup/toolchains/stable-aarch64-unknown-linux-gnu");
    let rustflags = if spec.features.contains(&"profile") {
        "-D warnings -C target-cpu=cortex-a9 -C force-frame-pointers=yes"
    } else {
        "-D warnings -C target-cpu=cortex-a9"
    };
    let mut command = Command::new("container");
    command
        .current_dir(repository)
        .args([
            "run",
            "--progress",
            "none",
            "--arch",
            "arm64",
            "--rm",
            "--cpus",
        ])
        .arg(&cpus)
        .args([
            "--memory",
            "8g",
            "--env",
            "CARGO_HOME=/cargo",
            "--env",
            "CARGO_TARGET_DIR=/target",
            "--env",
            "PATH=/rust/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        ])
        .arg("--env")
        .arg(format!("CARGO_BUILD_JOBS={cpus}"))
        .arg("--env")
        .arg(format!("MISTER_UI_BUILD_SCOPE={}", spec.ui_scope.label()))
        .args(["--env", "RUSTC_WRAPPER=", "--env"])
        .arg(format!("RUSTFLAGS={rustflags}"))
        .args(["--env", "SLINT_FONT_SIZES=8,16,24,32"]);
    for value in metadata.environment() {
        command.arg("--env").arg(value);
    }
    if spec.target == BuildTarget::Runtime && spec.mode != BuildMode::CheckLibrary {
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
        .arg(format!("{}:/project", repository.display()))
        .arg("--volume")
        .arg(format!("{}:/target", target_dir.display()))
        .args(["--workdir", container_workdir(spec.target), IMAGE, "cargo"])
        .args(cargo_args(spec, cargo_timings));
    Ok(command)
}

fn configure_cross_environment(command: &mut Command, repository: &Path) -> AgentResult<()> {
    let config = repository.join("apps/mister/Cross.toml");
    if !config.is_file() {
        return Err(format!("canonical cross config is missing: {}", config.display()).into());
    }
    command
        .env("CROSS_CONFIG", config)
        .env("RUSTUP_TOOLCHAIN", rust_toolchain_channel(repository)?);
    Ok(())
}

fn rust_toolchain_channel(repository: &Path) -> AgentResult<String> {
    let path = repository.join("apps/mister/rust-toolchain.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    text.lines()
        .map(str::trim)
        .find_map(|line| {
            line.strip_prefix("channel")?
                .trim_start()
                .strip_prefix('=')?
                .trim()
                .strip_prefix('"')?
                .strip_suffix('"')
                .map(str::to_owned)
        })
        .filter(|channel| !channel.is_empty())
        .ok_or_else(|| format!("{} has no toolchain channel", path.display()).into())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FfmpegVerification {
    Stamp,
    Full,
}

const FFMPEG_VERSION: &str = "8.1.2";
const FFMPEG_MODE: &str = "video-fast-noswscale";
const FFMPEG_CONFIGURE: &str = r#"rm -rf ../dist
./configure --prefix=/project/apps/mister/target/ffmpeg-minimal/armv7/dist --cross-prefix=arm-linux-gnueabihf- --arch=arm --cpu=cortex-a9 --target-os=linux --enable-cross-compile --extra-cflags='-O3 -mcpu=cortex-a9 -mfpu=neon-vfpv3 -mfloat-abi=hard' --extra-cxxflags='-O3 -mcpu=cortex-a9 -mfpu=neon-vfpv3 -mfloat-abi=hard' --enable-static --disable-shared --enable-pic --disable-autodetect --disable-programs --disable-doc --disable-debug --enable-stripping --disable-everything --disable-avdevice --disable-avfilter --enable-swresample --enable-avcodec --enable-avformat --enable-avutil --disable-swscale --enable-decoder=h264 --enable-decoder=aac --enable-decoder=pcm_s16le --enable-parser=aac --enable-parser=h264 --enable-demuxer=mov --enable-protocol=file
grep -q '^#define CONFIG_GPL 0$' config.h && grep -q '^#define CONFIG_VERSION3 0$' config.h && grep -q '^#define CONFIG_NONFREE 0$' config.h
make install"#;

fn build_minimal_ffmpeg(
    repository: &Path,
    backend: BuildBackend,
    verification: FfmpegVerification,
) -> AgentResult<()> {
    let app = repository.join("apps/mister");
    let work = app.join("target/ffmpeg-minimal/armv7");
    let source = work.join(format!("ffmpeg-{FFMPEG_VERSION}"));
    let dist = work.join("dist");
    let stamp = dist.join(".mister-minimal-ffmpeg-cache-v2");
    let expected_stamp = ffmpeg_recipe_stamp(repository, backend)?;
    let required = [
        "include/libavcodec/avcodec.h",
        "include/libavcodec/version_major.h",
        "include/libavformat/avformat.h",
        "include/libavutil/avutil.h",
        "include/libswresample/swresample.h",
        "lib/libavcodec.a",
        "lib/libavformat.a",
        "lib/libavutil.a",
        "lib/libswresample.a",
        "lib/pkgconfig/libavcodec.pc",
        "lib/pkgconfig/libswresample.pc",
    ];
    let cache_matches = ffmpeg_cache_matches(&dist, &stamp, &expected_stamp, &required);
    if cache_matches {
        return match verification {
            FfmpegVerification::Stamp => Ok(()),
            FfmpegVerification::Full => verify_cached_ffmpeg(&source, &dist, &stamp),
        };
    }
    if dist.exists() {
        std::fs::remove_dir_all(&dist)
            .map_err(|error| format!("cannot replace incomplete FFmpeg cache: {error}"))?;
    }
    std::fs::create_dir_all(&work)
        .map_err(|error| format!("cannot create FFmpeg workspace: {error}"))?;
    if !source.join(".git").is_dir() {
        if source.exists() {
            std::fs::remove_dir_all(&source)
                .map_err(|error| format!("cannot replace FFmpeg source: {error}"))?;
        }
        let mut clone = Command::new("git");
        clone
            .args([
                "clone",
                "--depth=1",
                "-b",
                &format!("n{FFMPEG_VERSION}"),
                "https://github.com/FFmpeg/FFmpeg",
            ])
            .arg(&source);
        run_bounded(&mut clone, BUILD_DEADLINE)?;
    }
    let cpus = std::thread::available_parallelism()
        .map_err(|error| format!("cannot detect online CPUs: {error}"))?
        .get()
        .to_string();
    let mut runner = match backend {
        BuildBackend::AppleContainer => {
            let mut command = Command::new("container");
            command
                .current_dir(repository)
                .args([
                    "run",
                    "--progress",
                    "none",
                    "--arch",
                    "arm64",
                    "--rm",
                    "--cpus",
                    &cpus,
                    "--memory",
                    "8g",
                    "--env",
                ])
                .arg(format!("MAKEFLAGS=-j{cpus}"))
                .arg("--volume")
                .arg(format!("{}:/project", repository.display()))
                .args([
                    "--workdir",
                    &format!(
                        "/project/apps/mister/target/ffmpeg-minimal/armv7/ffmpeg-{FFMPEG_VERSION}"
                    ),
                    IMAGE,
                    "sh",
                    "-ec",
                    FFMPEG_CONFIGURE,
                ]);
            command
        }
        BuildBackend::Cross => {
            let cross = std::fs::read_to_string(app.join("Cross.toml"))
                .map_err(|error| format!("cannot read Cross.toml: {error}"))?;
            let image = cross
                .lines()
                .find_map(|line| {
                    line.trim()
                        .strip_prefix("image = \"")
                        .and_then(|value| value.strip_suffix('"'))
                })
                .ok_or("Cross.toml has no target image")?;
            let mut command = Command::new("docker");
            let uid = numeric_identity("-u")?;
            let gid = numeric_identity("-g")?;
            command
                .current_dir(repository)
                .args(["run", "--rm", "--platform", "linux/amd64", "--user"])
                .arg(format!("{uid}:{gid}"))
                .arg("-e")
                .arg(format!("MAKEFLAGS=-j{cpus}"))
                .arg("-v")
                .arg(format!("{}:/project", repository.display()))
                .args([
                    "-w",
                    &format!(
                        "/project/apps/mister/target/ffmpeg-minimal/armv7/ffmpeg-{FFMPEG_VERSION}"
                    ),
                    image,
                    "sh",
                    "-ec",
                    FFMPEG_CONFIGURE,
                ]);
            command
        }
    };
    run_bounded(&mut runner, BUILD_DEADLINE)?;
    if !required.iter().all(|name| dist.join(name).is_file()) {
        return Err("minimal FFmpeg build did not produce every required output".into());
    }
    verify_minimal_ffmpeg(&source, &dist)?;
    write_atomic(&stamp, format!("{expected_stamp}\n").as_bytes())
}

fn verify_cached_ffmpeg(source: &Path, dist: &Path, stamp: &Path) -> AgentResult<()> {
    if let Err(error) = verify_minimal_ffmpeg(source, dist) {
        let _ = std::fs::remove_file(stamp);
        return Err(error);
    }
    Ok(())
}

fn ffmpeg_cache_matches(
    dist: &Path,
    stamp: &Path,
    expected_stamp: &str,
    required: &[&str],
) -> bool {
    required.iter().all(|name| dist.join(name).is_file())
        && std::fs::read_to_string(stamp).is_ok_and(|current| current.trim() == expected_stamp)
}

fn ffmpeg_recipe_stamp(repository: &Path, backend: BuildBackend) -> AgentResult<String> {
    let backend_recipe = match backend {
        BuildBackend::AppleContainer => {
            sha256(&repository.join("apps/mister/Dockerfile.cross-armv7"))?
        }
        BuildBackend::Cross => sha256(&repository.join("apps/mister/Cross.toml"))?,
    };
    Ok(sha256_text(&format!(
        "v2\nversion={FFMPEG_VERSION}\nmode={FFMPEG_MODE}\ntarget={TARGET}\nbackend={backend:?}\nbackend_recipe={backend_recipe}\nconfigure={FFMPEG_CONFIGURE}\n"
    )))
}

fn numeric_identity(flag: &str) -> AgentResult<String> {
    let output = Command::new("id")
        .arg(flag)
        .output()
        .map_err(|error| format!("cannot determine host identity: {error}"))?;
    if !output.status.success() {
        return Err("cannot determine host identity".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn verify_minimal_ffmpeg(source: &Path, dist: &Path) -> AgentResult<()> {
    let config = std::fs::read_to_string(source.join("config.h"))
        .map_err(|error| format!("cannot verify FFmpeg config: {error}"))?;
    for required in [
        "#define ARCH_ARM 1",
        "#define HAVE_NEON 1",
        "#define CONFIG_RUNTIME_CPUDETECT 1",
    ] {
        if !config.lines().any(|line| line == required) {
            return Err(format!("FFmpeg configuration is missing {required}").into());
        }
    }
    let output = Command::new("ar")
        .arg("t")
        .arg(dist.join("lib/libavcodec.a"))
        .output()
        .map_err(|error| format!("cannot inspect FFmpeg archive: {error}"))?;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.contains("neon") && line.contains("h264"))
    {
        return Err("FFmpeg archive does not contain H.264 NEON objects".into());
    }
    Ok(())
}

impl BuildActions for ProcessBuildActions<'_, '_, '_> {
    fn run(&mut self, phase: Phase) -> AgentResult<()> {
        let started = Instant::now();
        let result = match phase {
            Phase::Infer | Phase::Complete => Ok(()),
            Phase::Preflight => self.session.ensure_preflight(),
            Phase::PrepareContainer => {
                if self.session.backend == BuildBackend::AppleContainer {
                    create_target_dir(&self.target_dir)?;
                    self.session.ensure_container_ready()?;
                }
                Ok(())
            }
            Phase::Compile => self.compile(),
            Phase::Verify => {
                self.mirror_artifact()?;
                self.record_artifact_attribution();
                if self.spec.mode == BuildMode::Build
                    && !self.session.repository.join(&self.spec.artifact).is_file()
                {
                    return Err("build completed without its expected output".into());
                }
                Ok(())
            }
            Phase::Receipt => self.write_receipt(),
        };
        self.timings.push(BuildTimingSample {
            phase: phase.label(),
            status: if result.is_ok() {
                crate::host::DeliveryTimingStatus::Passed
            } else {
                crate::host::DeliveryTimingStatus::Failed
            },
            elapsed_ms: elapsed_millis(started),
        });
        result
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn emit_build_timings(
    reporter: &mut Reporter<'_>,
    samples: &[BuildTimingSample],
) -> AgentResult<()> {
    for sample in samples {
        reporter.emit(
            if sample.status == crate::host::DeliveryTimingStatus::Passed {
                EventKind::Completed
            } else {
                EventKind::Warning
            },
            "build-timing",
            &format!(
                "build_phase_tsv\tscope=build\tphase={}\tstatus={}\tseconds={:.3}",
                sample.phase,
                sample.status.label(),
                sample.elapsed_ms as f64 / 1_000.0,
            ),
            None,
        )?;
    }
    Ok(())
}

fn emit_build_attribution(
    reporter: &mut Reporter<'_>,
    attribution: &BuildArtifactAttribution,
) -> AgentResult<()> {
    let largest = attribution
        .largest_files
        .iter()
        .map(|file| format!("{}:{}", file.path, file.bytes))
        .collect::<Vec<_>>()
        .join(",");
    reporter.emit(
        EventKind::Completed,
        "build-artifact",
        &format!(
            "build_artifact_tsv\tscope=build\tartifact={}\tartifact_bytes={}\trelease_dir_bytes={}\tlargest_files={largest}",
            attribution.artifact,
            attribution.artifact_bytes,
            attribution.release_dir_bytes,
        ),
        None,
    )?;
    Ok(())
}

fn collect_files(root: &Path, base: &Path, files: &mut Vec<BuildArtifactFile>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_files(&path, base, files);
        } else if file_type.is_file() {
            let Ok(bytes) = entry.metadata().map(|metadata| metadata.len()) else {
                continue;
            };
            files.push(BuildArtifactFile {
                path: path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                bytes,
            });
        }
    }
}

fn infer_backend() -> AgentResult<BuildBackend> {
    match std::env::var("MISTER_ARM_BUILD_BACKEND").as_deref() {
        Ok("cross") => return Ok(BuildBackend::Cross),
        Ok("apple-container") => return Ok(BuildBackend::AppleContainer),
        Ok(other) => return Err(format!("unsupported ARM build backend: {other}").into()),
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

fn preflight(backend: BuildBackend) -> AgentResult<()> {
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
        Err(format!("{program} preflight failed").into())
    }
}

fn create_target_dir(target_dir: &Path) -> AgentResult<()> {
    std::fs::create_dir_all(target_dir)
        .map_err(|error| format!("cannot create target cache: {error}").into())
}

fn prepare_container_image(repository: &Path) -> AgentResult<()> {
    let dockerfile = repository.join("apps/mister/Dockerfile.cross-armv7");
    let stamp_path = Path::new(IMAGE_STAMP);
    let dockerfile_sha256 = sha256(&dockerfile)?;
    let current = std::fs::read_to_string(stamp_path).unwrap_or_default();
    if let Some(image_id) = inspect_container_image().unwrap_or_default() {
        let expected = image_stamp(&dockerfile_sha256, &image_id);
        if current.trim() == expected {
            return Ok(());
        }
    }
    let mut build = Command::new("container");
    build.current_dir(repository.join("apps/mister")).args([
        "build",
        "--progress",
        "plain",
        "--arch",
        "arm64",
        "--file",
        "Dockerfile.cross-armv7",
        "--tag",
        IMAGE,
        ".",
    ]);
    run_bounded(&mut build, BUILD_DEADLINE)?;
    let image_id = inspect_container_image()?
        .ok_or("container image build completed without a Linux/arm64 image")?;
    write_atomic(
        stamp_path,
        format!("{}\n", image_stamp(&dockerfile_sha256, &image_id)).as_bytes(),
    )
}

fn image_stamp(dockerfile_sha256: &str, image_id: &str) -> String {
    format!("image\t{IMAGE}\tdockerfile_sha256\t{dockerfile_sha256}\timage_id\t{image_id}")
}

fn inspect_container_image() -> AgentResult<Option<String>> {
    let output = Command::new("container")
        .args(["image", "inspect", IMAGE])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot inspect Apple container image: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    parse_container_image_id(&String::from_utf8_lossy(&output.stdout))
}

fn parse_container_image_id(text: &str) -> AgentResult<Option<String>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| format!("invalid container image JSON: {error}"))?;
    let images = value
        .as_array()
        .ok_or("container image inspection did not return an array")?;
    for image in images {
        if image.pointer("/configuration/name").and_then(Value::as_str) != Some(IMAGE) {
            continue;
        }
        let arm64 = image
            .get("variants")
            .and_then(Value::as_array)
            .is_some_and(|variants| {
                variants.iter().any(|variant| {
                    variant.pointer("/platform/os").and_then(Value::as_str) == Some("linux")
                        && variant
                            .pointer("/platform/architecture")
                            .and_then(Value::as_str)
                            == Some("arm64")
                })
            });
        if arm64 {
            let id = image
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or("container image inspection has no image ID")?;
            return Ok(Some(id.to_owned()));
        }
    }
    Ok(None)
}

fn requested_cargo_timings() -> AgentResult<bool> {
    let value = std::env::var("MISTER_CARGO_TIMINGS").ok();
    cargo_timings_enabled(value.as_deref())
}

fn cargo_timings_enabled(value: Option<&str>) -> AgentResult<bool> {
    match value {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(other) => Err(format!("invalid MISTER_CARGO_TIMINGS={other:?}; use 0 or 1").into()),
    }
}

fn cargo_args(spec: &BuildSpec, timings: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![match spec.mode {
        BuildMode::Build => "build".into(),
        BuildMode::Check | BuildMode::CheckLibrary => "check".into(),
    }];
    args.extend(["--target".into(), TARGET.into(), "--locked".into()]);
    if timings {
        args.push("--timings".into());
    }
    if spec.mode == BuildMode::Build {
        args.extend(["--profile".into(), spec.profile.into()]);
    }
    match spec.target {
        BuildTarget::Runtime
        | BuildTarget::DeviceAgent
        | BuildTarget::Manager
        | BuildTarget::FramebufferLab
        | BuildTarget::FramebufferSceneLab => {}
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
        BuildTarget::Manager => "mister/tools/manager",
        BuildTarget::FramebufferLab => "apps/framebuffer-lab",
        BuildTarget::FramebufferSceneLab => "apps/framebuffer-scene-lab",
    }
}

fn container_workdir(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Runtime => "/project/apps/mister",
        BuildTarget::DeviceAgent => "/project/mister/tools/agent",
        BuildTarget::Manager => "/project/mister/tools/manager",
        BuildTarget::FramebufferLab => "/project/apps/framebuffer-lab",
        BuildTarget::FramebufferSceneLab => "/project/apps/framebuffer-scene-lab",
    }
}

fn lockfile(target: BuildTarget) -> &'static str {
    match target {
        BuildTarget::Runtime => "apps/mister/Cargo.lock",
        BuildTarget::DeviceAgent => "mister/tools/agent/Cargo.lock",
        BuildTarget::Manager => "mister/tools/manager/Cargo.lock",
        BuildTarget::FramebufferLab => "apps/framebuffer-lab/Cargo.lock",
        BuildTarget::FramebufferSceneLab => "apps/framebuffer-scene-lab/Cargo.lock",
    }
}

fn runtime_artifact(profile: &str) -> PathBuf {
    PathBuf::from(format!(
        "apps/mister/target/{TARGET}/{profile}/mister-magik-fb"
    ))
}

fn framebuffer_lab_artifact(profile: &str) -> PathBuf {
    PathBuf::from(format!(
        "apps/framebuffer-lab/target/{TARGET}/{profile}/mister-magik-particle-lab"
    ))
}

fn framebuffer_scene_lab_artifact(profile: &str) -> PathBuf {
    PathBuf::from(format!(
        "apps/framebuffer-scene-lab/target/{TARGET}/{profile}/mister-magik-framebuffer-scene-lab"
    ))
}

fn home_dir() -> AgentResult<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unavailable for the build cache".into())
}

fn sha256(path: &Path) -> AgentResult<String> {
    for (program, args) in [("shasum", vec!["-a", "256"]), ("sha256sum", Vec::new())] {
        if let Ok(output) = Command::new(program).args(args).arg(path).output()
            && output.status.success()
            && let Some(value) = String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next()
        {
            return Ok(value.to_lowercase());
        }
    }
    Err(format!("cannot hash {}", path.display()).into())
}

fn sha256_text(text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(text.as_bytes());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_atomic(path: &Path, contents: &[u8]) -> AgentResult<()> {
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&temporary, contents)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("cannot publish {}: {error}", path.display()).into())
}

fn git_output(repository: &Path, args: &[&str]) -> AgentResult<String> {
    crate::git::value_with_context(repository, args, "inspect Git build identity")
        .map_err(Into::into)
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct BuildMetadata {
    build_number: String,
    version: String,
    build_time: String,
    source_revision: String,
    source_dirty: bool,
}

impl BuildMetadata {
    fn environment(&self) -> [String; 5] {
        [
            format!("MISTER_MAGIK_BUILD_NUMBER={}", self.build_number),
            format!("MISTER_MAGIK_VERSION={}", self.version),
            format!("MISTER_MAGIK_BUILD_TIME={}", self.build_time),
            format!("MISTER_MAGIK_SOURCE_REVISION={}", self.source_revision),
            format!("MISTER_MAGIK_SOURCE_DIRTY={}", u8::from(self.source_dirty)),
        ]
    }
}

fn build_metadata(repository: &Path) -> AgentResult<BuildMetadata> {
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
    let source_revision = std::env::var("MISTER_MAGIK_SOURCE_REVISION")
        .unwrap_or(git_output(repository, &["rev-parse", "HEAD"])?);
    let source_dirty = match std::env::var("MISTER_MAGIK_SOURCE_DIRTY") {
        Ok(value) => parse_source_dirty(&value)?,
        Err(_) => !git_output(
            repository,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty(),
    };
    Ok(BuildMetadata {
        build_number,
        version,
        build_time,
        source_revision,
        source_dirty,
    })
}

fn parse_source_dirty(value: &str) -> AgentResult<bool> {
    match value.trim() {
        "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err(format!("invalid MISTER_MAGIK_SOURCE_DIRTY={value:?}; use 0 or 1").into()),
    }
}

fn run_bounded(command: &mut Command, deadline: Duration) -> AgentResult<()> {
    let description = format!("{command:?}");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot start {description}: {error}"))?;
    let status = crate::process::wait(&mut child, Some(deadline), &description, None, || Ok(()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command exited with {status}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mister-magik-build-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn fixed_metadata(source_dirty: bool) -> BuildMetadata {
        BuildMetadata {
            build_number: "2429".into(),
            version: "0.2.2429".into(),
            build_time: "28.7.2026 12:00".into(),
            source_revision: "deadbeef".into(),
            source_dirty,
        }
    }

    fn manager_receipt_fixture() -> (PathBuf, BuildSpec, BuildMetadata) {
        let repository = temporary_directory("manager-receipt");
        let spec = BuildSpec::for_command(BuildCommand::ManagerDevice).unwrap();
        let metadata = fixed_metadata(false);
        let artifact = repository.join(&spec.artifact);
        let lock = repository.join(lockfile(spec.target));
        let toolchain = repository.join("apps/mister/rust-toolchain.toml");
        for path in [&artifact, &lock, &toolchain] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        std::fs::write(&artifact, b"manager").unwrap();
        std::fs::write(&lock, b"lock").unwrap();
        std::fs::write(&toolchain, b"toolchain").unwrap();
        let receipt = format!(
            "build_receipt_tsv\tbinary_sha256={}\tprofile={}\tfeatures={}\tui_scope={}\tbuild_number={}\tversion={}\tsource_commit={}\tsource_dirty=0\tcache_identity={}\tlock_sha256={}\ttoolchain_sha256={}\n",
            sha256(&artifact).unwrap(),
            spec.profile,
            spec.features.join(","),
            spec.ui_scope.label(),
            metadata.build_number,
            metadata.version,
            metadata.source_revision,
            spec.cache_identity,
            sha256(&lock).unwrap(),
            sha256(&toolchain).unwrap(),
        );
        let receipt_path = repository.join(&spec.receipt);
        std::fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
        std::fs::write(receipt_path, receipt).unwrap();
        (repository, spec, metadata)
    }

    #[derive(Default)]
    struct FakeActions {
        fail_at: Option<Phase>,
        visited: Vec<Phase>,
    }

    impl BuildActions for FakeActions {
        fn run(&mut self, phase: Phase) -> AgentResult<()> {
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
        let runtime = BuildSpec::canonical(UiScope::All);
        assert_eq!(runtime.profile, "release-device");
        assert_eq!(runtime.features, ["ui", "profile"]);
        assert_eq!(runtime.ui_scope, UiScope::All);
        let launcher = BuildSpec::for_command(BuildCommand::ValidateLauncher).unwrap();
        assert_eq!(launcher.mode, BuildMode::Check);
        assert_eq!(launcher.ui_scope, UiScope::Launcher);
        let library = BuildSpec::for_command(BuildCommand::ValidateLibrary).unwrap();
        assert_eq!(library.mode, BuildMode::CheckLibrary);
        assert!(BuildSpec::for_command(BuildCommand::ReleaseBinaries).is_none());
    }

    #[test]
    fn magik_full_app_baseline_reproduces_the_iteration_build() {
        let spec = BuildSpec::magik_full_app_baseline();
        assert_eq!(spec.target, BuildTarget::Runtime);
        assert_eq!(spec.mode, BuildMode::Build);
        assert_eq!(spec.profile, "release-live");
        assert_eq!(spec.features, ["ui"]);
        assert_eq!(spec.ui_scope, UiScope::Launcher);
    }

    #[test]
    fn framebuffer_scene_lab_build_is_slint_free_and_focused() {
        let spec = BuildSpec::framebuffer_scene_lab_device();
        assert_eq!(spec.target, BuildTarget::FramebufferSceneLab);
        assert_eq!(spec.features, ["profile"]);
        assert_eq!(spec.profile, "release-device");
        assert_eq!(host_workdir(spec.target), "apps/framebuffer-scene-lab");
        assert_eq!(
            spec.artifact,
            PathBuf::from(
                "apps/framebuffer-scene-lab/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-framebuffer-scene-lab"
            )
        );
        let analysis = BuildSpec::framebuffer_scene_lab_analysis();
        assert_eq!(analysis.target, BuildTarget::FramebufferSceneLab);
        assert_eq!(analysis.profile, "release-device-profile");
        assert_eq!(analysis.features, ["profile"]);
    }

    #[test]
    fn canonical_release_keeps_signed_media_manifests_disabled() {
        let runtime = BuildSpec::canonical(UiScope::All);
        assert_eq!(runtime.features, ["ui", "profile"]);
        assert!(
            !runtime.features.contains(&"signed-media-manifests"),
            "signed manifests require a separately authorized rollout release"
        );
    }

    #[test]
    fn cache_identity_changes_with_profile_scope_and_target() {
        let device = BuildSpec::canonical(UiScope::All);
        let production = BuildSpec::canonical(UiScope::Production);
        let ci = BuildSpec::for_command(BuildCommand::RuntimeCi).unwrap();
        let agent = BuildSpec::for_command(BuildCommand::DeviceAgentCi).unwrap();
        assert_eq!(ci.profile, "ci-fast");
        assert_eq!(agent.profile, "ci-fast");
        assert_ne!(device.cache_identity, ci.cache_identity);
        assert_ne!(device.cache_identity, agent.cache_identity);
        assert_ne!(device.cache_identity, production.cache_identity);
        assert_ne!(
            device.cache_identity,
            BuildSpec::canonical(UiScope::Launcher).cache_identity
        );
        assert!(
            BuildSpec::canonical(UiScope::Launcher)
                .cache_identity
                .ends_with(":launcher")
        );
        assert!(
            !BuildSpec::canonical(UiScope::Launcher)
                .cache_identity
                .contains(":all:launcher")
        );
        assert!(
            BuildSpec::canonical(UiScope::Production)
                .cache_identity
                .ends_with(":production")
        );
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
    fn cargo_timings_default_off_and_follow_the_explicit_switch() {
        assert!(!cargo_timings_enabled(None).unwrap());
        assert!(cargo_timings_enabled(Some("1")).unwrap());
        assert!(!cargo_timings_enabled(Some("0")).unwrap());
        assert!(cargo_timings_enabled(Some("true")).is_err());

        let spec = BuildSpec::for_command(BuildCommand::RuntimeCi).unwrap();
        assert!(
            cargo_args(&spec, true)
                .iter()
                .any(|argument| argument == "--timings")
        );
        assert!(
            cargo_args(&spec, false)
                .iter()
                .all(|argument| argument != "--timings")
        );
    }

    #[test]
    fn every_cross_target_uses_canonical_config_and_toolchain() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for target in [BuildTarget::Runtime, BuildTarget::DeviceAgent] {
            let mut command = Command::new("cross");
            command.current_dir(repository.join(host_workdir(target)));
            configure_cross_environment(&mut command, repository).unwrap();
            let environment: BTreeMap<_, _> = command
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
                .collect();
            let expected_config = repository.join("apps/mister/Cross.toml").into_os_string();
            let expected_toolchain = OsString::from("1.98.0");
            assert_eq!(
                environment.get(OsStr::new("CROSS_CONFIG")),
                Some(&expected_config)
            );
            assert_eq!(
                environment.get(OsStr::new("RUSTUP_TOOLCHAIN")),
                Some(&expected_toolchain)
            );
        }
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
            assert!(error.to_string().starts_with(phase.label()));
            assert_eq!(actions.visited, phases[..=index]);
        }
    }

    #[test]
    fn receipt_requires_artifact_cache_and_release_identity() {
        let valid = "build_receipt_tsv\tbinary_sha256=abc\tprofile=release-device\tfeatures=ui\tui_scope=all\tbuild_number=2429\tversion=0.2.2429\tsource_commit=deadbeef\tsource_dirty=0\tcache_identity=v3\tlock_sha256=lock\ttoolchain_sha256=toolchain\n";
        let receipt = BuildReceipt::parse(valid).unwrap();
        assert_eq!(receipt.cache_identity, "v3");
        assert_eq!(receipt.build_number, "2429");
        assert_eq!(receipt.version, "0.2.2429");
        assert!(BuildReceipt::parse(valid.replace("\tcache_identity=v3", "").as_str()).is_err());
        assert!(BuildReceipt::parse(valid.replace("\tbuild_number=2429", "").as_str()).is_err());
        assert!(BuildReceipt::parse(valid.replace("\tversion=0.2.2429", "").as_str()).is_err());
    }

    #[test]
    fn embedded_build_metadata_environment_includes_source_identity() {
        let metadata = BuildMetadata {
            build_number: "2429".into(),
            version: "0.2.2429".into(),
            build_time: "28.7.2026 12:00".into(),
            source_revision: "deadbeef".into(),
            source_dirty: true,
        };

        assert_eq!(
            metadata.environment(),
            [
                "MISTER_MAGIK_BUILD_NUMBER=2429",
                "MISTER_MAGIK_VERSION=0.2.2429",
                "MISTER_MAGIK_BUILD_TIME=28.7.2026 12:00",
                "MISTER_MAGIK_SOURCE_REVISION=deadbeef",
                "MISTER_MAGIK_SOURCE_DIRTY=1",
            ]
        );
    }

    #[test]
    fn source_dirty_environment_is_strictly_parsed() {
        assert!(!parse_source_dirty("0").unwrap());
        assert!(parse_source_dirty("true").unwrap());
        assert!(parse_source_dirty("unknown").is_err());
    }

    #[test]
    fn image_inspection_requires_the_exact_linux_arm64_image() {
        let valid = format!(
            r#"[{{"configuration":{{"name":"{IMAGE}"}},"id":"sha256:abc","variants":[{{"platform":{{"os":"linux","architecture":"arm64"}}}}]}}]"#
        );
        assert_eq!(
            parse_container_image_id(&valid).unwrap(),
            Some("sha256:abc".into())
        );
        assert_eq!(
            parse_container_image_id(&valid.replace("arm64", "amd64")).unwrap(),
            None
        );
        assert_eq!(
            parse_container_image_id(&valid.replace(IMAGE, "another:image")).unwrap(),
            None
        );
        assert_eq!(parse_container_image_id("[]").unwrap(), None);
        assert!(parse_container_image_id("{").is_err());
    }

    #[test]
    fn ensure_once_retries_failures_and_runs_one_success() {
        let mut completed = false;
        let mut attempts = 0;
        assert!(
            ensure_once(&mut completed, || {
                attempts += 1;
                Err("failure".into())
            })
            .is_err()
        );
        assert!(!completed);
        ensure_once(&mut completed, || {
            attempts += 1;
            Ok(())
        })
        .unwrap();
        ensure_once(&mut completed, || {
            attempts += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(attempts, 2);
    }

    #[test]
    fn apple_container_build_executes_cargo_directly_without_a_shell() {
        let spec = BuildSpec::canonical(UiScope::All);
        let command = apple_container_cargo_command(
            Path::new("/checkout"),
            &spec,
            Path::new("/target-cache"),
            &fixed_metadata(false),
            false,
        )
        .unwrap();
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--progress", "none"])
        );
        assert!(arguments.windows(2).any(|pair| pair == [IMAGE, "cargo"]));
        assert!(!arguments.iter().any(|argument| argument == "sh"));
        assert!(!arguments.iter().any(|argument| argument == "-lc"));
        assert!(arguments.iter().any(|argument| {
            argument
                == "PATH=/rust/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
        }));
        assert!(!arguments.iter().any(|argument| argument == "--timings"));
    }

    #[test]
    fn ffmpeg_stamp_requires_every_output_and_failed_full_verification_removes_it() {
        let root = temporary_directory("ffmpeg-stamp");
        let dist = root.join("dist");
        let source = root.join("source");
        let stamp = dist.join(".mister-minimal-ffmpeg-cache-v2");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        for required in ["one", "two"] {
            std::fs::write(dist.join(required), b"required").unwrap();
        }
        std::fs::write(&stamp, b"expected\n").unwrap();
        assert!(ffmpeg_cache_matches(
            &dist,
            &stamp,
            "expected",
            &["one", "two"]
        ));
        std::fs::remove_file(dist.join("two")).unwrap();
        assert!(!ffmpeg_cache_matches(
            &dist,
            &stamp,
            "expected",
            &["one", "two"]
        ));
        std::fs::write(dist.join("two"), b"required").unwrap();
        assert!(verify_cached_ffmpeg(&source, &dist, &stamp).is_err());
        assert!(!stamp.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ffmpeg_stamp_changes_with_the_backend_recipe() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_ne!(
            ffmpeg_recipe_stamp(repository, BuildBackend::AppleContainer).unwrap(),
            ffmpeg_recipe_stamp(repository, BuildBackend::Cross).unwrap()
        );
    }

    #[test]
    fn clean_manager_receipts_are_reusable_and_all_unsafe_variants_miss() {
        let (repository, spec, metadata) = manager_receipt_fixture();
        let clean = BuildSession {
            repository: &repository,
            backend: BuildBackend::Cross,
            metadata: metadata.clone(),
            cargo_timings: false,
            preflight_complete: false,
            container_ready: false,
        };
        assert!(clean.reusable_clean_receipt(&spec).is_ok());

        let dirty = BuildSession {
            metadata: fixed_metadata(true),
            ..clean
        };
        assert!(dirty.reusable_clean_receipt(&spec).is_err());

        let stale = BuildSession {
            metadata: BuildMetadata {
                source_revision: "stale".into(),
                ..metadata.clone()
            },
            ..dirty
        };
        assert!(stale.reusable_clean_receipt(&spec).is_err());

        std::fs::write(repository.join(&spec.artifact), b"corrupted").unwrap();
        let corrupted = BuildSession { metadata, ..stale };
        assert!(corrupted.reusable_clean_receipt(&spec).is_err());
        std::fs::write(repository.join(&spec.receipt), b"malformed").unwrap();
        assert!(corrupted.reusable_clean_receipt(&spec).is_err());
        std::fs::remove_dir_all(repository).unwrap();
    }

    #[test]
    fn source_identity_guard_rejects_head_and_dirty_state_changes() {
        let metadata = fixed_metadata(false);
        assert!(validate_source_identity(&metadata, "deadbeef", "").is_ok());
        assert!(validate_source_identity(&metadata, "changed", "").is_err());
        let error = validate_source_identity(&metadata, "deadbeef", " M generated.rs")
            .unwrap_err()
            .to_string();
        assert!(error.contains("changes= M generated.rs"));
    }
}
