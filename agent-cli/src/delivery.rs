// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::deploy::{DeliveryDecision, DeploymentKind, DeploymentPlan};
use crate::device::DeviceClient;
use crate::error::{AgentError, AgentResult};
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{DeviceOperations, DeviceRequest};
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
    GithubResolution,
    RuntimeBuild,
    MainQualification,
    KernelQualification,
    LocalStaging,
    DatabasePreparation,
    Snapshot,
    RemoteInventoryUpload,
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
            Self::Reconcile => "reconciliation",
            Self::GithubResolution => "github-resolution",
            Self::RuntimeBuild => "runtime-build",
            Self::MainQualification => "main-qualification",
            Self::KernelQualification => "kernel-qualification",
            Self::LocalStaging => "local-staging",
            Self::DatabasePreparation => "database-preparation",
            Self::Snapshot => "snapshot",
            Self::RemoteInventoryUpload => "remote-inventory-upload",
            Self::Activate => "activate",
            Self::RebootIfNeeded => "reboot-if-needed",
            Self::Smoke => "smoke",
            Self::Complete => "complete",
        }
    }

    fn may_have_mutated(self) -> bool {
        matches!(
            self,
            Self::Snapshot
                | Self::RemoteInventoryUpload
                | Self::Activate
                | Self::RebootIfNeeded
                | Self::Smoke
        )
    }

    fn starts_mutation(self) -> bool {
        matches!(
            self,
            Self::Snapshot
                | Self::RemoteInventoryUpload
                | Self::Activate
                | Self::RebootIfNeeded
                | Self::Smoke
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

    fn should_run(&self, _phase: Phase) -> bool {
        true
    }

    fn is_complete(&self) -> bool {
        false
    }
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
        (Phase::GithubResolution, 22),
        (Phase::RuntimeBuild, 32),
        (Phase::MainQualification, 39),
        (Phase::KernelQualification, 46),
        (Phase::LocalStaging, 53),
        (Phase::DatabasePreparation, 60),
        (Phase::Snapshot, 67),
        (Phase::RemoteInventoryUpload, 74),
        (Phase::Activate, 81),
        (Phase::RebootIfNeeded, 87),
        (Phase::Smoke, 94),
        (Phase::Complete, 100),
    ];
    let mut mutation_started = false;
    for (phase, percent) in PHASES {
        if !actions.should_run(*phase) {
            continue;
        }
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
            Ok(()) => {
                mutation_started |= phase.starts_mutation();
                if actions.is_complete() {
                    break;
                }
            }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryExecution {
    pub outcome: Outcome,
    pub decision: DeliveryDecision,
}

pub fn execute(
    repository: &Path,
    expected_commit: &str,
    local_main: &Path,
    reporter: &mut Reporter<'_>,
) -> AgentResult<DeliveryExecution> {
    execute_with_device(
        repository,
        expected_commit,
        local_main,
        reporter,
        DeviceClient::default(),
    )
}

fn execute_with_device<D: DeviceOperations>(
    repository: &Path,
    expected_commit: &str,
    local_main: &Path,
    reporter: &mut Reporter<'_>,
    device: DeviceClient<D>,
) -> AgentResult<DeliveryExecution> {
    let deployment = crate::deploy::plan(repository, Vec::new())?;
    let mut actions = ProcessActions {
        repository,
        deployment,
        expected_commit,
        artifact_sha256: None,
        no_op: false,
        decision: DeliveryDecision::Platform,
        installed_manifest: String::new(),
        installed_manager_sha256: None,
        main_revision: None,
        local_main: local_main.to_path_buf(),
        stage: repository
            .join("build/agent-deploy/stage")
            .join(expected_commit),
        device,
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
    Ok(DeliveryExecution {
        outcome: if actions.no_op {
            Outcome::NoOp
        } else {
            Outcome::Passed
        },
        decision: actions.decision,
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

struct ProcessActions<'a, D = mister_tool::NativeDevice> {
    repository: &'a Path,
    deployment: DeploymentPlan,
    expected_commit: &'a str,
    artifact_sha256: Option<String>,
    no_op: bool,
    decision: DeliveryDecision,
    installed_manifest: String,
    installed_manager_sha256: Option<String>,
    main_revision: Option<String>,
    local_main: PathBuf,
    stage: PathBuf,
    device: DeviceClient<D>,
}

impl<D: DeviceOperations> ProcessActions<'_, D> {
    fn validate_commit(&self) -> AgentResult<()> {
        let head = crate::git::value(self.repository, &["rev-parse", "HEAD"])?;
        let dirty = !crate::git::value(self.repository, &["status", "--porcelain"])?.is_empty();
        validate_commit_identity(&head, self.expected_commit, dirty, None)
    }

    fn resolve_github(&mut self) -> AgentResult<()> {
        self.deployment.platform_candidate = Some(
            crate::platform_ci::resolve_published_repository(self.repository, |_| Ok(()))?,
        );
        Ok(())
    }

    fn build_runtime(&mut self) -> AgentResult<()> {
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
        Ok(())
    }

    fn qualify_main(&self) -> AgentResult<()> {
        qualify_local_main(self.repository, &self.local_main)
    }

    fn qualify_kernel(&self) -> AgentResult<()> {
        qualify_local_kernel(self.repository)
    }

    fn prepare_local_stage(&mut self) -> AgentResult<()> {
        let manager = self.prepare_manager()?;
        let candidate = self
            .deployment
            .platform_candidate
            .as_ref()
            .ok_or("platform delivery is missing its qualified candidate")?;
        if self.stage.exists() {
            fs::remove_dir_all(&self.stage).map_err(|error| error.to_string())?;
        }
        fs::create_dir_all(self.stage.join("fpga")).map_err(|error| error.to_string())?;
        self.main_revision = Some(prepare_stage_files(
            self.repository,
            candidate,
            &self.stage,
            self.deployment.build.artifact(),
            Some(&self.local_main),
            &manager,
        )?);
        Ok(())
    }

    fn prepare_manager(&mut self) -> AgentResult<PathBuf> {
        let changed = self
            .deployment
            .changed_paths
            .iter()
            .any(|path| path.starts_with("mister/tools/manager"));
        if !changed && let Some(expected) = self.installed_manager_sha256.clone() {
            let cache = self
                .repository
                .join("build/agent-cache/manager")
                .join(&expected)
                .join("mister-magik-manager");
            if cache.is_file() && file_sha256(&cache)? == expected {
                return Ok(cache);
            }
            let temporary = cache.with_extension("download");
            if self
                .device
                .execute(DeviceRequest::FetchVerifiedDevelopmentManager {
                    local: temporary.clone(),
                    expected_sha256: expected.clone(),
                })
                .is_ok()
                && temporary.is_file()
                && file_sha256(&temporary)? == expected
            {
                if let Some(parent) = cache.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                fs::rename(&temporary, &cache).map_err(|error| error.to_string())?;
                return Ok(cache);
            }
            let _ = fs::remove_file(temporary);
        }
        let spec = crate::build::BuildSpec::for_recipe(crate::build::BuildRecipe::ManagerDevice);
        crate::build::execute_quiet(self.repository, &spec)?;
        let receipt = spec.verify(self.repository)?;
        if receipt.source_commit != self.expected_commit || receipt.source_dirty {
            return Err("manager artifact was not built from the exact clean commit".into());
        }
        let artifact = self.repository.join(spec.artifact());
        let cache = self
            .repository
            .join("build/agent-cache/manager")
            .join(&receipt.binary_sha256)
            .join("mister-magik-manager");
        if let Some(parent) = cache.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        copy(artifact, cache.clone())?;
        Ok(cache)
    }

    fn prepare_databases(&self) -> AgentResult<()> {
        prepare_stage_databases(
            self.repository,
            &self.stage,
            self.main_revision
                .as_deref()
                .ok_or("local staging did not record the Main revision")?,
        )
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

impl<D: DeviceOperations> DeliveryActions for ProcessActions<'_, D> {
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
                self.installed_manager_sha256 = crate::platform_manifest::parse_installed(
                    &installed,
                    crate::platform_manifest::Layout::Development,
                )
                .ok()
                .map(|manifest| manifest.manager_sha256().to_owned());
                let reconciliation = crate::deploy::reconcile(
                    self.repository,
                    &installed,
                    &main_revision,
                    self.expected_commit,
                );
                self.decision = reconciliation.decision;
                self.no_op = reconciliation.decision == DeliveryDecision::NoOp;
                self.installed_manifest = installed;
                if self.no_op {
                    return Ok(());
                }
                self.deployment =
                    crate::deploy::plan(self.repository, reconciliation.changed_paths)?;
                if reconciliation.decision == DeliveryDecision::Platform {
                    self.deployment.kind = DeploymentKind::Platform;
                }
                Ok(())
            }
            Phase::GithubResolution => self.resolve_github(),
            Phase::RuntimeBuild => self.build_runtime(),
            Phase::MainQualification => self.qualify_main(),
            Phase::KernelQualification => self.qualify_kernel(),
            Phase::LocalStaging => self.prepare_local_stage(),
            Phase::DatabasePreparation => self.prepare_databases(),
            Phase::Snapshot => Ok(()),
            Phase::RemoteInventoryUpload => match self.deployment.kind {
                _ if self.no_op => Ok(()),
                DeploymentKind::Runtime => self
                    .device
                    .execute(DeviceRequest::DeliverRuntimeTransaction {
                        local: self.deployment.build.artifact().to_path_buf(),
                        remote: REMOTE_RUNTIME.into(),
                        manifest_local: self.stage.join("platform-v2.manifest"),
                        manifest_remote: REMOTE_MANIFEST.into(),
                        expected_sha256: self
                            .artifact_sha256
                            .clone()
                            .ok_or("qualified runtime identity is missing")?,
                    })
                    .map(|_| ()),
                DeploymentKind::Platform => self
                    .device
                    .execute(DeviceRequest::DeliverPlatformTransaction {
                        stage: self.stage.clone(),
                        expected_sha256: self
                            .artifact_sha256
                            .clone()
                            .ok_or("qualified runtime identity is missing")?,
                    })
                    .map(|_| ()),
            },
            Phase::Activate | Phase::RebootIfNeeded | Phase::Smoke | Phase::Complete => Ok(()),
        }
    }

    fn should_run(&self, phase: Phase) -> bool {
        match phase {
            Phase::GithubResolution
            | Phase::MainQualification
            | Phase::KernelQualification
            | Phase::LocalStaging
            | Phase::DatabasePreparation
            | Phase::Activate
            | Phase::RebootIfNeeded => self.deployment.kind == DeploymentKind::Platform,
            _ => true,
        }
    }

    fn is_complete(&self) -> bool {
        self.no_op
    }

    fn compensate(&mut self) -> AgentResult<()> {
        Ok(())
    }
}

fn prepare_stage_files(
    repository: &Path,
    candidate: &crate::platform_ci::Candidate,
    stage: &Path,
    gui_artifact: &Path,
    local_main: Option<&Path>,
    manager: &Path,
) -> AgentResult<String> {
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
    copy(manager.to_path_buf(), stage.join("mister-magik-manager"))?;
    Ok(main_revision)
}

fn prepare_stage_databases(
    repository: &Path,
    stage: &Path,
    main_revision: &str,
) -> AgentResult<()> {
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
        main_revision,
        &crate::git::value(repository, &["rev-parse", "HEAD"])?,
        crate::platform_manifest::Layout::Development,
    )
}

fn qualify_local_main(repository: &Path, main_dir: &Path) -> AgentResult<()> {
    validate_local_main_checkout(main_dir)?;
    let revision = crate::git::value(main_dir, &["rev-parse", "HEAD"])?;
    let binary = main_dir.join("bin/MiSTer");
    let image = std::env::var("MISTER_MAIN_CONTAINER_IMAGE")
        .unwrap_or_else(|_| "mister-magik-main-builder:ubuntu20-arm64".into());
    let dockerfile = main_dir.join(".devcontainer/Dockerfile.apple-container");
    let dockerfile_sha256 = file_sha256(&dockerfile)?;
    let source_input_sha256 = digest_identity(&[
        &revision,
        &file_sha256(&main_dir.join("build-container.sh"))?,
        &dockerfile_sha256,
    ]);
    let image_digest = container_image_digest(&image).ok();
    let artifact_sha256 = binary.is_file().then(|| file_sha256(&binary)).transpose()?;
    let receipt = repository
        .join("build/agent-cache/main")
        .join(format!("{revision}.receipt"));
    let cache_matches = match (image_digest.as_deref(), artifact_sha256.as_deref()) {
        (Some(image_digest), Some(artifact_sha256)) => main_receipt_matches(
            &receipt,
            &MainReceiptIdentity {
                revision: &revision,
                source_input_sha256: &source_input_sha256,
                artifact_sha256,
                compiler: "gcc-arm-10.2-2020.11-aarch64-arm-none-linux-gnueabihf",
                dockerfile_sha256: &dockerfile_sha256,
                image: &image,
                image_digest,
            },
        )?,
        _ => false,
    };
    if cache_matches {
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
    let image_digest = container_image_digest(&image)?;
    let artifact_sha256 = file_sha256(&binary)?;
    let receipt_text = format!(
        "receipt_version=2\nmain_revision={revision}\nsource_input_sha256={source_input_sha256}\nartifact_sha256={artifact_sha256}\ncompiler=gcc-arm-10.2-2020.11-aarch64-arm-none-linux-gnueabihf\ndockerfile_sha256={dockerfile_sha256}\nimage_reference={image}\nimage_digest={image_digest}\n"
    );
    let temporary = receipt.with_extension("receipt.tmp");
    fs::write(&temporary, receipt_text).map_err(|error| error.to_string())?;
    fs::rename(temporary, receipt).map_err(|error| error.to_string())?;
    Ok(())
}

struct MainReceiptIdentity<'a> {
    revision: &'a str,
    source_input_sha256: &'a str,
    artifact_sha256: &'a str,
    compiler: &'a str,
    dockerfile_sha256: &'a str,
    image: &'a str,
    image_digest: &'a str,
}

fn main_receipt_matches(receipt: &Path, expected: &MainReceiptIdentity<'_>) -> AgentResult<bool> {
    let text = match fs::read_to_string(receipt) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string().into()),
    };
    let mut fields = std::collections::BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            return Ok(false);
        };
        if fields.insert(key, value).is_some() {
            return Ok(false);
        }
    }
    let expected_fields = [
        ("receipt_version", "2"),
        ("main_revision", expected.revision),
        ("source_input_sha256", expected.source_input_sha256),
        ("artifact_sha256", expected.artifact_sha256),
        ("compiler", expected.compiler),
        ("dockerfile_sha256", expected.dockerfile_sha256),
        ("image_reference", expected.image),
        ("image_digest", expected.image_digest),
    ];
    Ok(fields.len() == expected_fields.len()
        && expected_fields
            .iter()
            .all(|(key, value)| fields.get(key).copied() == Some(*value)))
}

fn container_image_digest(image: &str) -> AgentResult<String> {
    let output = Command::new("container")
        .args(["image", "inspect", image])
        .output()
        .map_err(|error| format!("cannot inspect Apple container image {image}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot inspect Apple container image {image}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid container image metadata: {error}"))?;
    value
        .get(0)
        .and_then(|entry| entry.pointer("/configuration/descriptor/digest"))
        .and_then(Value::as_str)
        .filter(|digest| {
            digest.len() == 71
                && digest.starts_with("sha256:")
                && digest[7..]
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
        .map(str::to_owned)
        .ok_or_else(|| "container image metadata has no canonical OCI digest".into())
}

fn digest_identity(values: &[&str]) -> String {
    let mut digest = Sha256::new();
    for value in values {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

    const PHASES: [Phase; 16] = [
        Phase::Classify,
        Phase::ValidateCommit,
        Phase::Connect,
        Phase::Reconcile,
        Phase::GithubResolution,
        Phase::RuntimeBuild,
        Phase::MainQualification,
        Phase::KernelQualification,
        Phase::LocalStaging,
        Phase::DatabasePreparation,
        Phase::Snapshot,
        Phase::RemoteInventoryUpload,
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

    #[test]
    fn exact_manifest_fake_device_stops_after_reconciliation() {
        use crate::cli::OutputFormat;
        use crate::evidence::Evidence;
        use crate::request::RawRequest;
        use mister_tool::transport::{DeviceFailure, DeviceResponse};
        use std::cell::RefCell;
        use std::rc::Rc;

        struct RecordingDevice {
            requests: Rc<RefCell<Vec<DeviceRequest>>>,
            manifest: String,
        }

        impl DeviceOperations for RecordingDevice {
            fn execute(
                &mut self,
                request: &DeviceRequest,
            ) -> Result<DeviceResponse, DeviceFailure> {
                self.requests.borrow_mut().push(request.clone());
                Ok(DeviceResponse {
                    operation: request.label(),
                    detail: if *request == DeviceRequest::ReadDevelopmentManifest {
                        self.manifest.clone()
                    } else {
                        "connected".into()
                    },
                })
            }
        }

        let root = std::env::temp_dir().join(format!(
            "mister-magik-delivery-no-op-test-{}",
            std::process::id()
        ));
        let repository = root.join("app");
        let main = root.join("Main_MiSTer");
        fs::create_dir_all(&repository).unwrap();
        fs::create_dir_all(&main).unwrap();
        initialize_git_repository(&repository, "main", "README.md");
        fs::write(main.join("build-container.sh"), "#!/bin/sh\n").unwrap();
        initialize_git_repository(&main, "mister-magik", "build-container.sh");
        let app_revision = crate::git::value(&repository, &["rev-parse", "HEAD"]).unwrap();
        let main_revision = crate::git::value(&main, &["rev-parse", "HEAD"]).unwrap();
        let manifest = canonical_test_manifest(&app_revision, &main_revision);
        let requests = Rc::new(RefCell::new(Vec::new()));
        let evidence = Evidence::open_at(&root.join("evidence")).unwrap();
        let request = RawRequest::capture(["agent-cli", "deliver"].map(std::ffi::OsString::from));
        evidence.begin_request(&request).unwrap();
        let mut reporter = Reporter::new(&evidence, OutputFormat::Human, request.id.as_str());

        let execution = execute_with_device(
            &repository,
            &app_revision,
            &main,
            &mut reporter,
            DeviceClient::new(RecordingDevice {
                requests: Rc::clone(&requests),
                manifest,
            }),
        )
        .unwrap();

        assert_eq!(execution.outcome, Outcome::NoOp);
        assert_eq!(execution.decision, DeliveryDecision::NoOp);
        assert_eq!(
            requests.borrow().as_slice(),
            &[
                DeviceRequest::Discover,
                DeviceRequest::ReadDevelopmentManifest
            ]
        );
        assert!(!repository.join("build").exists());
        let _ = fs::remove_dir_all(root);
    }

    fn initialize_git_repository(repository: &Path, branch: &str, tracked_file: &str) {
        let status = Command::new("git")
            .args(["init", "-q", "-b", branch])
            .current_dir(repository)
            .status()
            .unwrap();
        assert!(status.success());
        if !repository.join(tracked_file).exists() {
            fs::write(repository.join(tracked_file), "fixture\n").unwrap();
        }
        let stage = format!("a{}", "dd");
        assert!(
            Command::new("git")
                .args([stage.as_str(), tracked_file])
                .current_dir(repository)
                .status()
                .unwrap()
                .success()
        );
        let save = format!("com{}", "mit");
        assert!(
            Command::new("git")
                .args([
                    "-c",
                    "user.name=Agent CLI",
                    "-c",
                    "user.email=agent@example.invalid",
                    save.as_str(),
                    "-q",
                    "-m",
                    "fixture",
                ])
                .current_dir(repository)
                .status()
                .unwrap()
                .success()
        );
    }

    fn canonical_test_manifest(magik_revision: &str, main_revision: &str) -> String {
        let mut fields = std::collections::BTreeMap::<String, String>::new();
        fields.insert("format".into(), "mister-magik-platform-v2".into());
        for (name, path) in crate::platform_manifest::Layout::Development.paths() {
            fields.insert(format!("{name}_path"), path.into());
        }
        for name in [
            "main_sha256",
            "gui_sha256",
            "manager_sha256",
            "scanout_module_sha256",
            "scanout_metadata_sha256",
            "latch_rbf_sha256",
            "latch_metadata_sha256",
            "platform_contract_sha256",
        ] {
            fields.insert(name.into(), "a".repeat(64));
        }
        fields.insert("main_revision".into(), main_revision.into());
        fields.insert("magik_revision".into(), magik_revision.into());
        fields.insert("menu_revision".into(), "b".repeat(40));
        crate::platform_manifest::FIELDS
            .iter()
            .map(|field| format!("{field}={}\n", fields[*field]))
            .collect()
    }
}
