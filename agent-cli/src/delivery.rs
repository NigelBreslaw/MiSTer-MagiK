// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::deploy::{DeliveryDecision, DeploymentKind, DeploymentPlan};
use crate::device::DeviceClient;
use crate::error::{AgentError, AgentResult};
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{DeviceRequest, Layout, MainSelection};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const REMOTE_RUNTIME: &str = "/media/fat/mister-magik-dev/mister-magik-fb";
const REMOTE_MANIFEST: &str = "/media/fat/mister-magik-dev/platform-v2.manifest";
const PREPARE_DEADLINE: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Classify,
    ValidateCommit,
    Connect,
    Reconcile,
    QualifyArtifact,
    Snapshot,
    Stage,
    Activate,
    RebootIfNeeded,
    Smoke,
    Complete,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Classify => "classify",
            Self::ValidateCommit => "validate-commit",
            Self::Connect => "connect",
            Self::Reconcile => "reconcile",
            Self::QualifyArtifact => "qualify-artifact",
            Self::Snapshot => "snapshot",
            Self::Stage => "stage",
            Self::Activate => "activate",
            Self::RebootIfNeeded => "reboot-if-needed",
            Self::Smoke => "smoke",
            Self::Complete => "complete",
        }
    }

    fn may_have_mutated(self) -> bool {
        matches!(
            self,
            Self::Snapshot | Self::Stage | Self::Activate | Self::RebootIfNeeded | Self::Smoke
        )
    }

    fn starts_mutation(self) -> bool {
        matches!(
            self,
            Self::Snapshot | Self::Stage | Self::Activate | Self::RebootIfNeeded | Self::Smoke
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Action(Phase),
    Compensation,
}

pub trait DeliveryActions {
    fn run(&mut self, phase: Phase) -> AgentResult<()>;
    fn compensate(&mut self) -> AgentResult<()>;
}

pub fn run_transaction(
    actions: &mut dyn DeliveryActions,
    progress: &mut dyn FnMut(Step, u8) -> AgentResult<()>,
) -> AgentResult<()> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::Classify, 2),
        (Phase::ValidateCommit, 7),
        (Phase::Connect, 12),
        (Phase::Reconcile, 15),
        (Phase::QualifyArtifact, 38),
        (Phase::Snapshot, 46),
        (Phase::Stage, 56),
        (Phase::Activate, 68),
        (Phase::RebootIfNeeded, 78),
        (Phase::Smoke, 90),
        (Phase::Complete, 100),
    ];
    let mut mutation_started = false;
    for (phase, percent) in PHASES {
        if let Err(error) = progress(Step::Action(*phase), *percent) {
            if mutation_started {
                let _ = progress(Step::Compensation, 95);
                return match actions.compensate() {
                    Ok(()) => Err(format!("cancelled: {error}; rollback=complete").into()),
                    Err(rollback) => Err(AgentError::recovery_required(
                        format!("delivery cancelled ({error})"),
                        format!("rollback failed ({rollback})"),
                    )),
                };
            }
            return Err(AgentError::cancelled(error));
        }
        match actions.run(*phase) {
            Ok(()) => mutation_started |= phase.starts_mutation(),
            Err(error) if mutation_started || phase.may_have_mutated() => {
                let _ = progress(Step::Compensation, 95);
                return match actions.compensate() {
                    Ok(()) => Err(format!("{}: {error}; rollback=complete", phase.label()).into()),
                    Err(rollback) => Err(AgentError::recovery_required(
                        format!("{} failed ({error})", phase.label()),
                        format!("rollback failed ({rollback})"),
                    )),
                };
            }
            Err(error) => return Err(AgentError::phase(phase.label(), error)),
        }
    }
    Ok(())
}

pub fn execute(
    repository: &Path,
    expected_commit: &str,
    local_main: &Path,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    let deployment = crate::deploy::plan(repository, Vec::new())?;
    let mut actions = ProcessActions {
        repository,
        deployment,
        expected_commit,
        artifact_sha256: None,
        no_op: false,
        installed_manifest: String::new(),
        local_main: local_main.to_path_buf(),
        stage: repository
            .join("build/agent-deploy/stage")
            .join(expected_commit),
        device: DeviceClient::default(),
    };
    run_transaction(&mut actions, &mut |step, percent| match step {
        Step::Action(phase) => Ok(reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("delivery {}", phase.label()),
            Some(percent),
        )?),
        Step::Compensation => Ok(reporter.emit(
            EventKind::Warning,
            "compensate",
            "delivery failed; restoring verified snapshot",
            Some(percent),
        )?),
    })?;
    Ok(if actions.no_op {
        Outcome::NoOp
    } else {
        Outcome::Passed
    })
}

pub fn cleanup_workspace(repository: &Path) -> Result<(), String> {
    let workspace = repository.join("build/agent-deploy");
    for transient in ["stage", "platform"] {
        let path = workspace.join(transient);
        if !path.exists() {
            continue;
        }
        fs::remove_dir_all(&path).map_err(|error| {
            format!(
                "cannot clear delivery workspace {}: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

struct ProcessActions<'a> {
    repository: &'a Path,
    deployment: DeploymentPlan,
    expected_commit: &'a str,
    artifact_sha256: Option<String>,
    no_op: bool,
    installed_manifest: String,
    local_main: PathBuf,
    stage: PathBuf,
    device: DeviceClient,
}

impl ProcessActions<'_> {
    fn validate_commit(&self) -> AgentResult<()> {
        let head = crate::git::value(self.repository, &["rev-parse", "HEAD"])?;
        let dirty = !crate::git::value(self.repository, &["status", "--porcelain"])?.is_empty();
        validate_commit_identity(&head, self.expected_commit, dirty, None)
    }

    fn qualify(&mut self) -> AgentResult<()> {
        if self.no_op {
            return Ok(());
        }
        if self.deployment.kind == DeploymentKind::Runtime {
            self.device
                .execute(DeviceRequest::VerifyDevelopmentPlatform)?;
        }
        crate::build::execute_quiet(self.repository, &self.deployment.build)?;
        let receipt = self.deployment.build.verify(self.repository)?;
        if receipt.source_commit != self.expected_commit || receipt.source_dirty {
            return Err("runtime artifact was not built from the exact clean commit".into());
        }
        self.artifact_sha256 = Some(receipt.binary_sha256);
        if self.deployment.kind == DeploymentKind::Runtime {
            crate::platform_manifest::update_runtime(
                &self.stage.join("platform-v2.manifest"),
                &self.installed_manifest,
                &self.repository.join(self.deployment.build.artifact()),
                self.expected_commit,
            )?;
        }
        if self.deployment.kind == DeploymentKind::Platform {
            if self
                .deployment
                .changed_paths
                .iter()
                .any(|path| path.starts_with("mister/tools/manager"))
            {
                crate::build::execute_quiet(
                    self.repository,
                    &crate::build::BuildSpec::for_recipe(crate::build::BuildRecipe::ManagerDevice),
                )?;
            }
            let candidate = self
                .deployment
                .platform_candidate
                .as_ref()
                .ok_or("platform delivery is missing its qualified candidate")?;
            if self.stage.exists() {
                fs::remove_dir_all(&self.stage).map_err(|error| error.to_string())?;
            }
            fs::create_dir_all(self.stage.join("fpga")).map_err(|error| error.to_string())?;
            qualify_local_main(self.repository, &self.local_main)?;
            qualify_local_kernel(self.repository)?;
            prepare_stage(
                self.repository,
                candidate,
                &self.stage,
                self.deployment.build.artifact(),
                Some(&self.local_main),
            )?;
        }
        Ok(())
    }

    fn smoke(&mut self) -> AgentResult<()> {
        if self.no_op {
            return Ok(());
        }
        self.device
            .execute(DeviceRequest::SmokeDelivery {
                layout: Layout::Development,
                expected_sha256: self
                    .artifact_sha256
                    .clone()
                    .ok_or("qualified artifact identity is missing")?,
            })
            .map(|_| ())
    }
}

fn validate_commit_identity(
    head: &str,
    expected: &str,
    dirty: bool,
    platform_candidate: Option<&str>,
) -> AgentResult<()> {
    if head != expected {
        return Err("delivery HEAD does not match the recorded commit".into());
    }
    if dirty {
        return Err("delivery requires a clean exact-commit worktree".into());
    }
    if platform_candidate.is_some_and(|candidate| candidate != expected) {
        return Err("platform artifact does not match the recorded commit".into());
    }
    Ok(())
}

impl DeliveryActions for ProcessActions<'_> {
    fn run(&mut self, phase: Phase) -> AgentResult<()> {
        match phase {
            Phase::Classify => Ok(()),
            Phase::ValidateCommit => self.validate_commit(),
            Phase::Connect => self.device.execute(DeviceRequest::Discover).map(|_| ()),
            Phase::Reconcile => {
                validate_local_main_checkout(&self.local_main)?;
                let main_revision = crate::git::value(&self.local_main, &["rev-parse", "HEAD"])?;
                let installed = self
                    .device
                    .execute(DeviceRequest::ReadDevelopmentManifest)?;
                let reconciliation = crate::deploy::reconcile(
                    self.repository,
                    &installed,
                    &main_revision,
                    self.expected_commit,
                );
                self.no_op = reconciliation.decision == DeliveryDecision::NoOp;
                self.installed_manifest = installed;
                if self.no_op {
                    return Ok(());
                }
                self.deployment =
                    crate::deploy::plan(self.repository, reconciliation.changed_paths)?;
                if reconciliation.decision == DeliveryDecision::Platform {
                    self.deployment.kind = DeploymentKind::Platform;
                    self.deployment.platform_candidate = Some(
                        crate::platform_ci::resolve_published_repository(self.repository, |_| {
                            Ok(())
                        })?,
                    );
                }
                Ok(())
            }
            Phase::QualifyArtifact => self.qualify(),
            Phase::Snapshot => match self.deployment.kind {
                _ if self.no_op => Ok(()),
                DeploymentKind::Runtime => self
                    .device
                    .execute(DeviceRequest::SnapshotRuntimeBundle {
                        remote: REMOTE_RUNTIME.into(),
                        manifest: REMOTE_MANIFEST.into(),
                    })
                    .map(|_| ()),
                DeploymentKind::Platform => self
                    .device
                    .execute(DeviceRequest::SnapshotPlatform)
                    .map(|_| ()),
            },
            Phase::Stage => match self.deployment.kind {
                _ if self.no_op => Ok(()),
                DeploymentKind::Runtime => self
                    .device
                    .execute(DeviceRequest::DeployRuntimeBundle {
                        local: self.deployment.build.artifact().to_path_buf(),
                        remote: REMOTE_RUNTIME.into(),
                        manifest_local: self.stage.join("platform-v2.manifest"),
                        manifest_remote: REMOTE_MANIFEST.into(),
                    })
                    .map(|_| ()),
                DeploymentKind::Platform => self
                    .device
                    .execute(DeviceRequest::DeployPlatform {
                        stage: self.stage.clone(),
                    })
                    .map(|_| ()),
            },
            Phase::Activate => match self.deployment.kind {
                _ if self.no_op => Ok(()),
                DeploymentKind::Runtime => Ok(()),
                DeploymentKind::Platform => self
                    .device
                    .execute(DeviceRequest::SelectMain(MainSelection::Development))
                    .map(|_| ()),
            },
            Phase::RebootIfNeeded => match self.deployment.kind {
                _ if self.no_op => Ok(()),
                DeploymentKind::Runtime => Ok(()),
                DeploymentKind::Platform => {
                    self.device.execute(DeviceRequest::RebootWait).map(|_| ())
                }
            },
            Phase::Smoke => self.smoke(),
            Phase::Complete => match self.deployment.kind {
                _ if self.no_op => Ok(()),
                DeploymentKind::Runtime => self
                    .device
                    .execute(DeviceRequest::CommitRuntimeBundle {
                        remote: REMOTE_RUNTIME.into(),
                        manifest: REMOTE_MANIFEST.into(),
                    })
                    .map(|_| ()),
                DeploymentKind::Platform => self
                    .device
                    .execute(DeviceRequest::CommitPlatform)
                    .map(|_| ()),
            },
        }
    }

    fn compensate(&mut self) -> AgentResult<()> {
        match self.deployment.kind {
            DeploymentKind::Runtime => {
                self.device.execute(DeviceRequest::RollbackRuntimeBundle {
                    remote: REMOTE_RUNTIME.into(),
                    manifest: REMOTE_MANIFEST.into(),
                })?;
                self.device
                    .execute(DeviceRequest::VerifyHealth(Layout::Development))?;
            }
            DeploymentKind::Platform => {
                self.device.execute(DeviceRequest::RollbackPlatform)?;
                self.device.execute(DeviceRequest::RebootWait)?;
                self.device
                    .execute(DeviceRequest::VerifyHealth(Layout::Development))?;
            }
        }
        Ok(())
    }
}

fn prepare_stage(
    repository: &Path,
    candidate: &crate::platform_ci::Candidate,
    stage: &Path,
    gui_artifact: &Path,
    local_main: Option<&Path>,
) -> AgentResult<()> {
    let manifest: Value =
        serde_json::from_slice(&fs::read(&candidate.manifest).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid platform candidate manifest: {error}"))?;
    let candidate_main_revision = manifest
        .pointer("/components/main/head_sha")
        .and_then(Value::as_str)
        .ok_or("platform candidate is missing Main revision")?;
    let extracted = stage.join("candidate");
    run_bounded(
        repository,
        "/usr/bin/unzip",
        &[
            "-q".into(),
            candidate.archive.display().to_string(),
            "-d".into(),
            extracted.display().to_string(),
        ],
    )?;
    for (from, to) in [
        (
            "fpga/patched/menu-magik-vblank-latch.rbf",
            "fpga/menu-magik-vblank-latch.rbf",
        ),
        (
            "fpga/patched/menu-magik-vblank-latch.metadata.txt",
            "fpga/menu-magik-vblank-latch.metadata.txt",
        ),
    ] {
        copy(extracted.join(from), stage.join(to))?;
    }
    copy(
        repository.join("build/scanout-slots/mister_magik_scanout_slots.ko"),
        stage.join("mister_magik_scanout_slots.ko"),
    )?;
    copy(
        repository.join("build/scanout-slots/provenance.txt"),
        stage.join("mister_magik_scanout_slots.metadata.txt"),
    )?;
    let main_revision = if let Some(main_dir) = local_main {
        copy(main_dir.join("bin/MiSTer"), stage.join("MiSTer_MagiKDev"))?;
        crate::git::value(main_dir, &["rev-parse", "HEAD"])?
    } else {
        copy(
            extracted.join("main/MiSTer_MagiK"),
            stage.join("MiSTer_MagiKDev"),
        )?;
        candidate_main_revision.to_owned()
    };
    copy(repository.join(gui_artifact), stage.join("mister-magik-fb"))?;
    copy(
        repository.join("mister/tools/manager/target/armv7-unknown-linux-gnueabihf/release/mister-magik-manager"),
        stage.join("mister-magik-manager"),
    )?;
    let databases = stage.join("databases");
    prepare_game_databases(repository, &databases)?;
    for name in [
        "mame.sqlite3",
        "hbmame.sqlite3",
        "game-databases-manifest.json",
    ] {
        copy(databases.join(name), stage.join(name))?;
    }
    copy(
        databases.join("SHA256SUMS"),
        stage.join("game-databases-SHA256SUMS"),
    )?;
    crate::platform_manifest::generate(
        &stage.join("platform-v2.manifest"),
        &crate::platform_manifest::Artifacts {
            main: stage.join("MiSTer_MagiKDev"),
            gui: stage.join("mister-magik-fb"),
            manager: stage.join("mister-magik-manager"),
            scanout_module: stage.join("mister_magik_scanout_slots.ko"),
            scanout_metadata: stage.join("mister_magik_scanout_slots.metadata.txt"),
            latch_rbf: stage.join("fpga/menu-magik-vblank-latch.rbf"),
            latch_metadata: stage.join("fpga/menu-magik-vblank-latch.metadata.txt"),
        },
        &main_revision,
        &crate::git::value(repository, &["rev-parse", "HEAD"])?,
        crate::platform_manifest::Layout::Development,
    )
}

fn qualify_local_main(repository: &Path, main_dir: &Path) -> AgentResult<()> {
    validate_local_main_checkout(main_dir)?;
    let revision = crate::git::value(main_dir, &["rev-parse", "HEAD"])?;
    let binary = main_dir.join("bin/MiSTer");
    let receipt = repository
        .join("build/agent-cache/main")
        .join(format!("{revision}.receipt"));
    if binary.is_file() && receipt_matches(&receipt, &revision, &binary)? {
        return Ok(());
    }
    for (program, args) in [
        ("./build-container.sh", Vec::<String>::new()),
        ("scripts/test-magik-state.sh", Vec::<String>::new()),
        ("scripts/check-magik-patch-surface.sh", Vec::<String>::new()),
    ] {
        run_bounded(main_dir, program, &args)?;
    }
    if !main_dir.join("bin/MiSTer").is_file() {
        return Err("local_main_build: bin/MiSTer was not produced".into());
    }
    if let Some(parent) = receipt.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        receipt,
        format!(
            "main_revision={revision}\nbinary_sha256={}\n",
            file_sha256(&binary)?
        ),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn receipt_matches(receipt: &Path, revision: &str, artifact: &Path) -> AgentResult<bool> {
    let text = match fs::read_to_string(receipt) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string().into()),
    };
    let binary_sha256 = file_sha256(artifact)?;
    Ok(text
        .lines()
        .any(|line| line == format!("main_revision={revision}"))
        && text
            .lines()
            .any(|line| line == format!("binary_sha256={binary_sha256}")))
}

fn file_sha256(path: &Path) -> AgentResult<String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn validate_local_main_checkout(main_dir: &Path) -> AgentResult<()> {
    if !main_dir.join("build-container.sh").is_file() {
        return Err(format!(
            "local_main_missing: {} does not contain build-container.sh",
            main_dir.display()
        )
        .into());
    }
    let dirty = !crate::git::value(main_dir, &["status", "--porcelain"])?.is_empty();
    let branch = crate::git::value(main_dir, &["branch", "--show-current"])?;
    validate_local_main_identity(dirty, &branch)?;
    Ok(())
}

fn qualify_local_kernel(repository: &Path) -> AgentResult<()> {
    let kernel_source = kernel_source_directory(repository, std::env::var_os("MISTER_KERNEL_DIR"));
    if !kernel_source.is_dir() {
        return Err(format!(
            "local_kernel_missing: {} does not contain the pinned Linux-Kernel_MiSTer checkout; set MISTER_KERNEL_DIR to override it",
            kernel_source.display()
        )
        .into());
    }
    run_bounded_with_env(
        repository,
        "scripts/build-scanout-slots-module.sh",
        &Vec::<String>::new(),
        &[("KERNEL_SRC", kernel_source.as_os_str().to_os_string())],
    )?;
    for artifact in [
        "build/scanout-slots/mister_magik_scanout_slots.ko",
        "build/scanout-slots/provenance.txt",
    ] {
        if !repository.join(artifact).is_file() {
            return Err(format!("local_kernel_build: {artifact} was not produced").into());
        }
    }
    Ok(())
}

fn kernel_source_directory(repository: &Path, configured: Option<OsString>) -> PathBuf {
    configured
        .map(PathBuf::from)
        .unwrap_or_else(|| repository.join("../Linux-Kernel_MiSTer"))
}

fn validate_local_main_identity(dirty: bool, branch: &str) -> AgentResult<()> {
    if dirty {
        return Err(
            "local_main_dirty: commit or discard Main_MiSTer changes before delivery".into(),
        );
    }
    if branch != "mister-magik" {
        return Err(format!("local_main_branch: expected mister-magik, found {branch}").into());
    }
    Ok(())
}

fn copy(from: PathBuf, to: PathBuf) -> AgentResult<()> {
    Ok(fs::copy(&from, &to)
        .map(|_| ())
        .map_err(|error| format!("cannot copy {}: {error}", from.display()))?)
}

fn prepare_game_databases(repository: &Path, output: &Path) -> AgentResult<()> {
    let (owner, tag, version) = crate::platform_ci::latest_game_database_release(repository)?;
    let cache_root = repository.join("build/agent-cache/release-cache/game-databases");
    let cached = cache_root.join(&tag);
    if reuse_verified_cache(&cached, output, || {
        extract_game_databases(repository, &cached, output)
    })? {
        return Ok(());
    }
    fs::create_dir_all(&cache_root)
        .map_err(|error| format!("cannot create game-database cache: {error}"))?;
    let temporary = cache_root.join(format!(".{tag}.download-{}", std::process::id()));
    if temporary.exists() {
        fs::remove_dir_all(&temporary)
            .map_err(|error| format!("cannot clear temporary game-database download: {error}"))?;
    }
    fs::create_dir_all(&temporary)
        .map_err(|error| format!("cannot create temporary game-database download: {error}"))?;
    let archive_pattern = format!("mister-magik-game-databases-v{version}.zip");
    let download = run_bounded(
        repository,
        "gh",
        &[
            "release".into(),
            "download".into(),
            tag.clone(),
            "--repo".into(),
            owner,
            "--dir".into(),
            temporary.display().to_string(),
            "--pattern".into(),
            archive_pattern,
            "--pattern".into(),
            "game-databases-manifest.json".into(),
            "--pattern".into(),
            "SHA256SUMS".into(),
        ],
    );
    if let Err(error) = download {
        let _ = fs::remove_dir_all(&temporary);
        return Err(error);
    }
    if let Err(error) = extract_game_databases(repository, &temporary, output) {
        let _ = fs::remove_dir_all(&temporary);
        if output.exists() {
            let _ = fs::remove_dir_all(output);
        }
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, &cached) {
        let _ = fs::remove_dir_all(&temporary);
        return Err(format!("cannot publish game-database cache: {error}").into());
    }
    Ok(())
}

fn reuse_verified_cache(
    cached: &Path,
    output: &Path,
    verify: impl FnOnce() -> AgentResult<()>,
) -> AgentResult<bool> {
    if !cached.is_dir() {
        return Ok(false);
    }
    if verify().is_ok() {
        return Ok(true);
    }
    if output.exists() {
        fs::remove_dir_all(output).map_err(|error| error.to_string())?;
    }
    fs::remove_dir_all(cached)
        .map_err(|error| format!("cannot clear invalid game-database cache: {error}"))?;
    Ok(false)
}

fn extract_game_databases(repository: &Path, release: &Path, output: &Path) -> AgentResult<()> {
    let _ = repository;
    crate::game_databases::extract_release(release, output).map(|_| ())
}

fn run_bounded(repository: &Path, program: &str, args: &[String]) -> AgentResult<()> {
    run_bounded_with_env(repository, program, args, &[])
}

fn run_bounded_with_env(
    repository: &Path,
    program: &str,
    args: &[String],
    environment: &[(&str, OsString)],
) -> AgentResult<()> {
    let mut child = Command::new(program)
        .args(args)
        .envs(environment.iter().cloned())
        .current_dir(repository)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot start {program}: {error}"))?;
    let status =
        crate::process::wait(&mut child, Some(PREPARE_DEADLINE), program, None, || Ok(()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeActions {
        fail_at: Option<Phase>,
        rollback_fails: bool,
        visited: Vec<Step>,
    }

    impl DeliveryActions for FakeActions {
        fn run(&mut self, phase: Phase) -> AgentResult<()> {
            self.visited.push(Step::Action(phase));
            if self.fail_at == Some(phase) {
                Err("injected failure".into())
            } else {
                Ok(())
            }
        }

        fn compensate(&mut self) -> AgentResult<()> {
            self.visited.push(Step::Compensation);
            if self.rollback_fails {
                Err("injected rollback failure".into())
            } else {
                Ok(())
            }
        }
    }

    const PHASES: [Phase; 11] = [
        Phase::Classify,
        Phase::ValidateCommit,
        Phase::Connect,
        Phase::Reconcile,
        Phase::QualifyArtifact,
        Phase::Snapshot,
        Phase::Stage,
        Phase::Activate,
        Phase::RebootIfNeeded,
        Phase::Smoke,
        Phase::Complete,
    ];

    #[test]
    fn successful_delivery_visits_the_shared_state_chart() {
        let mut actions = FakeActions::default();
        run_transaction(&mut actions, &mut |_, _| Ok(())).unwrap();
        assert_eq!(
            actions.visited,
            PHASES.map(Step::Action).into_iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_phase_that_may_have_mutated_compensates() {
        for (index, phase) in PHASES.iter().enumerate() {
            let mut actions = FakeActions {
                fail_at: Some(*phase),
                ..FakeActions::default()
            };
            let error = run_transaction(&mut actions, &mut |_, _| Ok(())).unwrap_err();
            assert_eq!(
                actions.visited.last() == Some(&Step::Compensation),
                index
                    >= PHASES
                        .iter()
                        .position(|item| *item == Phase::Snapshot)
                        .unwrap()
            );
            assert!(error.to_string().contains("injected failure"));
            assert!(actions.visited.len() > index);
        }
    }

    #[test]
    fn rollback_failure_requires_recovery() {
        let mut actions = FakeActions {
            fail_at: Some(Phase::Smoke),
            rollback_fails: true,
            ..FakeActions::default()
        };
        let error = run_transaction(&mut actions, &mut |_, _| Ok(())).unwrap_err();
        assert!(error.is_recovery_required());
    }

    #[test]
    fn cancellation_after_mutation_compensates() {
        let mut actions = FakeActions::default();
        let error = run_transaction(&mut actions, &mut |step, _| {
            if step == Step::Action(Phase::Smoke) {
                Err("interrupted".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(error.to_string().starts_with("cancelled:"));
        assert_eq!(actions.visited.last(), Some(&Step::Compensation));
    }

    #[test]
    fn exact_commit_identity_is_mandatory() {
        assert!(validate_commit_identity("abc", "abc", false, Some("abc")).is_ok());
        assert!(validate_commit_identity("other", "abc", false, None).is_err());
        assert!(validate_commit_identity("abc", "abc", true, None).is_err());
        assert!(validate_commit_identity("abc", "abc", false, Some("other")).is_err());
    }

    #[test]
    fn local_main_requires_only_clean_mister_magik_branch() {
        assert!(validate_local_main_identity(false, "mister-magik").is_ok());
        assert!(
            validate_local_main_identity(true, "mister-magik")
                .unwrap_err()
                .to_string()
                .contains("local_main_dirty")
        );
        assert!(
            validate_local_main_identity(false, "feature")
                .unwrap_err()
                .to_string()
                .contains("local_main_branch")
        );
    }

    #[test]
    fn kernel_source_defaults_next_to_the_repository_and_accepts_an_override() {
        let repository = Path::new("/work/mister-slint");
        assert_eq!(
            kernel_source_directory(repository, None),
            PathBuf::from("/work/mister-slint/../Linux-Kernel_MiSTer")
        );
        assert_eq!(
            kernel_source_directory(repository, Some(OsString::from("/srv/mister-kernel"))),
            PathBuf::from("/srv/mister-kernel")
        );
    }

    #[test]
    fn delivery_contains_no_git_mutation_commands() {
        let source = include_str!("delivery.rs");
        for forbidden in ["\"add\"", "\"commit\"", "\"push\"", "\"reset\""] {
            assert!(
                !source.contains(forbidden),
                "Git mutation remains: {forbidden}"
            );
        }
    }

    #[test]
    fn verified_release_cache_is_reused_and_invalid_cache_is_evicted() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-delivery-cache-test-{}",
            std::process::id()
        ));
        let cached = root.join("cached");
        let output = root.join("output");
        fs::create_dir_all(&cached).unwrap();
        assert!(reuse_verified_cache(&cached, &output, || Ok(())).unwrap());
        assert!(cached.is_dir());
        fs::create_dir_all(&output).unwrap();
        assert!(!reuse_verified_cache(&cached, &output, || Err("corrupt".into())).unwrap());
        assert!(!cached.exists());
        assert!(!output.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn delivery_cleanup_removes_transient_stage_and_preserves_cache() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-delivery-workspace-test-{}",
            std::process::id()
        ));
        let workspace = root.join("build/agent-deploy/stage/commit");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("artifact"), b"generated").unwrap();
        let cache = root.join("build/agent-cache/release-cache/platform/tag");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("artifact"), b"cached").unwrap();

        cleanup_workspace(&root).unwrap();

        assert!(!root.join("build/agent-deploy/stage").exists());
        assert!(cache.join("artifact").is_file());
        let _ = fs::remove_dir_all(root);
    }
}
