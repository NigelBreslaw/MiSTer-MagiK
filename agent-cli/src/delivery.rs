// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::deploy::{DeliveryDecision, DeploymentKind, DeploymentPlan, UiScope};
use crate::device::DeviceClient;
use crate::error::{AgentError, AgentResult};
use crate::model::Outcome;
use crate::platform_stage::{generate_platform_manifest, stage_published_platform_components};
use crate::progress::{EventKind, Reporter};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PREPARE_DEADLINE: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Classify,
    ValidateCommit,
    Connect,
    GithubResolution,
    Reconcile,
    RuntimeBuild,
    ManagerQualification,
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
            Self::ManagerQualification => "manager-qualification",
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

    fn record_timing(&mut self, _sample: DeliveryPhaseTiming) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryPhaseTiming {
    phase: &'static str,
    status: crate::host::DeliveryTimingStatus,
    elapsed_ms: u64,
}

pub fn run_transaction(
    actions: &mut dyn DeliveryActions,
    progress: &mut dyn FnMut(Step, u8) -> AgentResult<()>,
) -> AgentResult<()> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::Classify, 2),
        (Phase::ValidateCommit, 7),
        (Phase::Connect, 12),
        (Phase::GithubResolution, 15),
        (Phase::Reconcile, 22),
        (Phase::RuntimeBuild, 32),
        (Phase::ManagerQualification, 50),
        (Phase::LocalStaging, 55),
        (Phase::DatabasePreparation, 62),
        (Phase::Snapshot, 68),
        (Phase::RemoteInventoryUpload, 76),
        (Phase::Activate, 82),
        (Phase::RebootIfNeeded, 88),
        (Phase::Smoke, 95),
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
                return match compensate_with_timing(actions) {
                    Ok(()) => Err(format!("cancelled: {error}; rollback=complete").into()),
                    Err(rollback) => Err(AgentError::recovery_required(
                        format!("delivery cancelled ({error})"),
                        format!("rollback failed ({rollback})"),
                    )),
                };
            }
            return Err(AgentError::cancelled(error));
        }
        let started = Instant::now();
        let phase_result = actions.run(*phase);
        actions.record_timing(DeliveryPhaseTiming {
            phase: phase.label(),
            status: if phase_result.is_ok() {
                crate::host::DeliveryTimingStatus::Passed
            } else {
                crate::host::DeliveryTimingStatus::Failed
            },
            elapsed_ms: elapsed_millis(started),
        });
        match phase_result {
            Ok(()) => {
                mutation_started |= phase.starts_mutation();
                if actions.is_complete() {
                    break;
                }
            }
            Err(error) if mutation_started || phase.may_have_mutated() => {
                let _ = progress(Step::Compensation, 95);
                return match compensate_with_timing(actions) {
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

fn compensate_with_timing(actions: &mut dyn DeliveryActions) -> AgentResult<()> {
    let started = Instant::now();
    let compensation = actions.compensate();
    actions.record_timing(DeliveryPhaseTiming {
        phase: "compensation",
        status: if compensation.is_ok() {
            crate::host::DeliveryTimingStatus::Passed
        } else {
            crate::host::DeliveryTimingStatus::Failed
        },
        elapsed_ms: elapsed_millis(started),
    });
    compensation
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryExecution {
    pub outcome: Outcome,
    pub decision: DeliveryDecision,
}

trait DeliveryDevice {
    fn connect(&mut self) -> AgentResult<()>;
    fn read_development_manifest(&mut self) -> AgentResult<String>;
    fn read_active_runtime(&mut self) -> AgentResult<crate::host::ActiveRuntime>;
    fn deliver_runtime(
        &mut self,
        delivery: RuntimeDelivery,
        timings: &mut Vec<crate::host::DeliveryTimingSample>,
    ) -> AgentResult<()>;
    fn deliver_platform(
        &mut self,
        stage: PathBuf,
        expected_sha256: String,
        timings: &mut Vec<crate::host::DeliveryTimingSample>,
    ) -> AgentResult<()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeDelivery {
    local: PathBuf,
    manifest_local: PathBuf,
    expected_sha256: String,
    artwork_local: PathBuf,
    artwork_expected_sha256: String,
}

impl DeliveryDevice for DeviceClient {
    fn connect(&mut self) -> AgentResult<()> {
        self.read(crate::NativeDevice::discover)
    }

    fn read_development_manifest(&mut self) -> AgentResult<String> {
        self.read(crate::NativeDevice::read_development_manifest)
    }

    fn read_active_runtime(&mut self) -> AgentResult<crate::host::ActiveRuntime> {
        self.read(crate::NativeDevice::read_active_runtime)
    }

    fn deliver_runtime(
        &mut self,
        delivery: RuntimeDelivery,
        timings: &mut Vec<crate::host::DeliveryTimingSample>,
    ) -> AgentResult<()> {
        self.mutate(|device| {
            device.deliver_runtime(
                &delivery.local,
                &delivery.manifest_local,
                &delivery.expected_sha256,
                &delivery.artwork_local,
                &delivery.artwork_expected_sha256,
                timings,
            )
        })
    }

    fn deliver_platform(
        &mut self,
        stage: PathBuf,
        expected_sha256: String,
        timings: &mut Vec<crate::host::DeliveryTimingSample>,
    ) -> AgentResult<()> {
        self.mutate(|device| device.deliver_platform(&stage, &expected_sha256, timings))
    }
}

pub fn execute(
    repository: &Path,
    expected_commit: &str,
    reporter: &mut Reporter<'_>,
) -> AgentResult<DeliveryExecution> {
    execute_with_device(
        repository,
        expected_commit,
        None,
        reporter,
        DeviceClient::default(),
    )
}

fn execute_with_device<D: DeliveryDevice>(
    repository: &Path,
    expected_commit: &str,
    platform_candidate: Option<crate::platform_ci::Candidate>,
    reporter: &mut Reporter<'_>,
    device: D,
) -> AgentResult<DeliveryExecution> {
    let planning_started = Instant::now();
    let deployment = crate::deploy::plan(repository, Vec::new());
    emit_phase_timing(
        reporter,
        DeliveryPhaseTiming {
            phase: "planning",
            status: if deployment.is_ok() {
                crate::host::DeliveryTimingStatus::Passed
            } else {
                crate::host::DeliveryTimingStatus::Failed
            },
            elapsed_ms: elapsed_millis(planning_started),
        },
    )?;
    let mut deployment = deployment?;
    deployment.platform_candidate = platform_candidate;
    let mut actions = ProcessActions {
        repository,
        deployment,
        expected_commit,
        artifact_sha256: None,
        decision: DeliveryDecision::Platform,
        manager_artifact: None,
        main_revision: None,
        installed_manifest: None,
        phase_timings: Vec::new(),
        timing_samples: Vec::new(),
        stage: repository
            .join("build/agent-deploy/stage")
            .join(expected_commit),
        device,
    };
    let transaction = run_transaction(&mut actions, &mut |step, percent| match step {
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
    });
    match transaction {
        Ok(()) => {
            emit_phase_timings(reporter, &actions.phase_timings)?;
            emit_delivery_timings(reporter, &actions.timing_samples)?;
        }
        Err(error) => {
            let _ = emit_phase_timings(reporter, &actions.phase_timings);
            let _ = emit_delivery_timings(reporter, &actions.timing_samples);
            return Err(error);
        }
    }
    if let Some(candidate) = actions.deployment.platform_candidate.as_ref() {
        let release = candidate.release_tag.as_deref().unwrap_or("candidate");
        let cache = if candidate.reused {
            "reused-cache"
        } else {
            "downloaded"
        };
        reporter.emit(
            EventKind::Progress,
            "platform-release",
            &format!("platform {release} {cache} bundle={}", candidate.bundle_id),
            Some(100),
        )?;
    }
    Ok(DeliveryExecution {
        outcome: Outcome::Passed,
        decision: actions.decision,
    })
}

fn emit_phase_timings(
    reporter: &mut Reporter<'_>,
    samples: &[DeliveryPhaseTiming],
) -> AgentResult<()> {
    for sample in samples {
        emit_phase_timing(reporter, *sample)?;
    }
    Ok(())
}

fn emit_phase_timing(reporter: &mut Reporter<'_>, sample: DeliveryPhaseTiming) -> AgentResult<()> {
    reporter.emit(
        if sample.status == crate::host::DeliveryTimingStatus::Passed {
            EventKind::Completed
        } else {
            EventKind::Warning
        },
        "delivery-timing",
        &format!(
            "delivery_phase_tsv\tscope=cli\tphase={}\tstatus={}\tseconds={:.3}",
            sample.phase,
            sample.status.label(),
            sample.elapsed_ms as f64 / 1_000.0,
        ),
        None,
    )?;
    Ok(())
}

fn emit_delivery_timings(
    reporter: &mut Reporter<'_>,
    samples: &[crate::host::DeliveryTimingSample],
) -> AgentResult<()> {
    for sample in samples {
        let (kind, phase, message) = render_delivery_timing(*sample);
        reporter.emit(kind, phase, &message, None)?;
    }
    Ok(())
}

fn render_delivery_timing(
    sample: crate::host::DeliveryTimingSample,
) -> (EventKind, &'static str, String) {
    use crate::host::{DeliveryTimingSample, DeliveryTimingStatus};

    match sample {
        DeliveryTimingSample::Transfer {
            lane,
            status,
            metrics,
        } => (
            if status == DeliveryTimingStatus::Passed {
                EventKind::Completed
            } else {
                EventKind::Warning
            },
            "delivery-transfer",
            format!(
                "delivery_transfer_tsv\tscope=device\tlane={}\tstatus={}\tseconds={:.3}\tfiles={}\tbytes={}\tupload_seconds={:.3}\tupload_ms={}\tdeploy_ms={}\tbytes_per_second={}",
                lane.label(),
                status.label(),
                metrics.deploy_ms as f64 / 1_000.0,
                metrics.files,
                metrics.bytes,
                metrics.upload_ms as f64 / 1_000.0,
                metrics.upload_ms,
                metrics.deploy_ms,
                metrics.bytes_per_second(),
            ),
        ),
        DeliveryTimingSample::Smoke {
            lane,
            status,
            smoke_ms,
        } => (
            if status == DeliveryTimingStatus::Passed {
                EventKind::Completed
            } else {
                EventKind::Warning
            },
            "delivery-smoke",
            format!(
                "delivery_smoke_tsv\tscope=device\tlane={}\tstatus={}\tseconds={:.3}\tsmoke_ms={smoke_ms}",
                lane.label(),
                status.label(),
                smoke_ms as f64 / 1_000.0,
            ),
        ),
        DeliveryTimingSample::Stage {
            lane,
            stage,
            status,
            elapsed_ms,
        } => (
            if status == DeliveryTimingStatus::Passed {
                EventKind::Completed
            } else {
                EventKind::Warning
            },
            "delivery-device-stage",
            format!(
                "delivery_stage_tsv\tscope=device\tlane={}\tstage={}\tstatus={}\tseconds={:.3}",
                lane.label(),
                stage,
                status.label(),
                elapsed_ms as f64 / 1_000.0,
            ),
        ),
    }
}

pub fn cleanup_workspace(repository: &Path) -> Result<(), String> {
    let workspace = repository.join("build/agent-deploy");
    for transient in ["stage", "platform", "local-main"] {
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

struct ProcessActions<'a, D = DeviceClient> {
    repository: &'a Path,
    deployment: DeploymentPlan,
    expected_commit: &'a str,
    artifact_sha256: Option<String>,
    decision: DeliveryDecision,
    manager_artifact: Option<PathBuf>,
    main_revision: Option<String>,
    installed_manifest: Option<String>,
    phase_timings: Vec<DeliveryPhaseTiming>,
    timing_samples: Vec<crate::host::DeliveryTimingSample>,
    stage: PathBuf,
    device: D,
}

impl<D> ProcessActions<'_, D> {
    fn validate_commit(&self) -> AgentResult<()> {
        let head = crate::git::value(self.repository, &["rev-parse", "HEAD"])?;
        let dirty = !crate::git::value(self.repository, &["status", "--porcelain"])?.is_empty();
        validate_commit_identity(&head, self.expected_commit, dirty, None)
    }

    fn resolve_github(&mut self) -> AgentResult<()> {
        if self.deployment.platform_candidate.is_some() {
            return Ok(());
        }
        self.deployment.platform_candidate = Some(
            crate::platform_ci::resolve_published_repository(self.repository, |_| Ok(()))?,
        );
        Ok(())
    }

    fn build_runtime(&mut self) -> AgentResult<()> {
        crate::build::execute_quiet(self.repository, &self.deployment.build)?;
        let receipt = self.deployment.build.verify(self.repository)?;
        if receipt.source_commit != self.expected_commit || receipt.source_dirty {
            return Err("runtime artifact was not built from the exact clean commit".into());
        }
        self.artifact_sha256 = Some(receipt.binary_sha256);
        Ok(())
    }

    fn prepare_local_stage(&mut self) -> AgentResult<()> {
        let manager = self
            .manager_artifact
            .clone()
            .ok_or("manager qualification did not produce an artifact")?;
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
            &manager,
        )?);
        Ok(())
    }

    fn prepare_runtime_manifest(&self) -> AgentResult<()> {
        crate::platform_manifest::update_runtime(
            &self.stage.join(crate::platform_manifest::FILE_NAME),
            self.installed_manifest
                .as_deref()
                .ok_or("runtime delivery is missing the installed platform manifest")?,
            &self.repository.join(self.deployment.build.artifact()),
            self.expected_commit,
        )
    }

    fn qualify_manager(&mut self) -> AgentResult<()> {
        self.manager_artifact = Some(self.prepare_manager()?);
        Ok(())
    }

    fn prepare_manager(&mut self) -> AgentResult<PathBuf> {
        let spec = crate::build::BuildSpec::for_command(crate::build::BuildCommand::ManagerDevice)
            .expect("manager builds have a specification");
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

fn reconcile_active_runtime(
    artifact_decision: DeliveryDecision,
    active: &crate::host::ActiveRuntime,
) -> DeliveryDecision {
    if active.is_development_launcher() {
        artifact_decision
    } else {
        DeliveryDecision::Platform
    }
}

impl<D: DeliveryDevice> DeliveryActions for ProcessActions<'_, D> {
    fn run(&mut self, phase: Phase) -> AgentResult<()> {
        match phase {
            Phase::Classify => Ok(()),
            Phase::ValidateCommit => self.validate_commit(),
            Phase::Connect => self.device.connect(),
            Phase::GithubResolution => self.resolve_github(),
            Phase::Reconcile => {
                let installed_manifest = self.device.read_development_manifest()?;
                let platform_candidate =
                    self.deployment.platform_candidate.as_ref().ok_or(
                        "delivery reconciliation is missing its published platform candidate",
                    )?;
                let release_tag = platform_candidate
                    .release_tag
                    .as_deref()
                    .ok_or("published platform candidate is missing its release tag")?;
                let reconciliation = crate::deploy::reconcile_with_platform(
                    self.repository,
                    &installed_manifest,
                    self.expected_commit,
                    release_tag,
                    &platform_candidate.bundle_id,
                );
                let platform_candidate = self.deployment.platform_candidate.take();
                self.deployment =
                    crate::deploy::plan(self.repository, reconciliation.changed_paths)?;
                self.deployment.platform_candidate = platform_candidate;
                let active = self.device.read_active_runtime()?;
                self.decision = reconcile_active_runtime(reconciliation.decision, &active);
                if self.decision == DeliveryDecision::Platform {
                    self.deployment.kind = DeploymentKind::Platform;
                    self.deployment.ui_scope = UiScope::All;
                    self.deployment.build = crate::build::BuildSpec::canonical(UiScope::All);
                }
                self.installed_manifest = Some(installed_manifest);
                Ok(())
            }
            Phase::RuntimeBuild => self.build_runtime(),
            Phase::ManagerQualification => self.qualify_manager(),
            Phase::LocalStaging => match self.decision {
                DeliveryDecision::Platform => self.prepare_local_stage(),
                DeliveryDecision::Runtime => self.prepare_runtime_manifest(),
                DeliveryDecision::NoOp => Ok(()),
            },
            Phase::DatabasePreparation => self.prepare_databases(),
            Phase::Snapshot => Ok(()),
            Phase::RemoteInventoryUpload => {
                let expected_sha256 = self
                    .artifact_sha256
                    .clone()
                    .ok_or("qualified runtime identity is missing")?;
                match self.decision {
                    DeliveryDecision::Runtime => self.device.deliver_runtime(
                        RuntimeDelivery {
                            local: self.repository.join(self.deployment.build.artifact()),
                            manifest_local: self.stage.join(crate::platform_manifest::FILE_NAME),
                            expected_sha256,
                            artwork_local: self
                                .repository
                                .join("apps/mister/assets/snes/snes-small-v1.rgb565a"),
                            artwork_expected_sha256:
                                "7a76993e7e1b0063832b94e9d2ad588549587cf09a14ac2ced72d349ed12f766"
                                    .into(),
                        },
                        &mut self.timing_samples,
                    ),
                    DeliveryDecision::Platform => self.device.deliver_platform(
                        self.stage.clone(),
                        expected_sha256,
                        &mut self.timing_samples,
                    ),
                    DeliveryDecision::NoOp => Ok(()),
                }
            }
            Phase::Activate | Phase::RebootIfNeeded | Phase::Smoke | Phase::Complete => Ok(()),
        }
    }

    fn should_run(&self, phase: Phase) -> bool {
        match phase {
            Phase::GithubResolution => true,
            Phase::ManagerQualification | Phase::DatabasePreparation => {
                self.decision == DeliveryDecision::Platform
            }
            Phase::RuntimeBuild
            | Phase::LocalStaging
            | Phase::Snapshot
            | Phase::RemoteInventoryUpload => self.decision != DeliveryDecision::NoOp,
            Phase::Activate | Phase::RebootIfNeeded | Phase::Smoke => false,
            _ => true,
        }
    }

    fn is_complete(&self) -> bool {
        self.decision == DeliveryDecision::NoOp
    }

    fn record_timing(&mut self, sample: DeliveryPhaseTiming) {
        self.phase_timings.push(sample);
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
    manager: &Path,
) -> AgentResult<String> {
    let manifest: Value =
        crate::platform_bundle::verify(&candidate.archive, Some(&candidate.manifest), None)?;
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
    stage_published_platform_components(&extracted, stage)?;
    copy(repository.join(gui_artifact), stage.join("mister-magik-fb"))?;
    copy(manager.to_path_buf(), stage.join("mister-magik-manager"))?;
    fs::create_dir_all(stage.join("assets/snes"))
        .map_err(|error| format!("cannot create SNES artwork stage: {error}"))?;
    copy(
        repository.join("apps/mister/assets/snes/snes-small-v1.rgb565a"),
        stage.join("assets/snes/snes-small-v1.rgb565a"),
    )?;
    Ok(candidate_main_revision.to_owned())
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
    generate_platform_manifest(repository, stage, main_revision)
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
    let mut child = Command::new(program)
        .args(args)
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
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::rc::Rc;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum DeliveryCall {
        Runtime,
        Platform,
    }

    struct RequestRecorder(Rc<RefCell<Vec<DeliveryCall>>>);

    fn sample_timings(lane: crate::host::DeliveryLane) -> [crate::host::DeliveryTimingSample; 2] {
        [
            crate::host::DeliveryTimingSample::Transfer {
                lane,
                status: crate::host::DeliveryTimingStatus::Passed,
                metrics: crate::host::DeliveryTransferMetrics {
                    files: 3,
                    bytes: 1_500,
                    upload_ms: 500,
                    deploy_ms: 900,
                },
            },
            crate::host::DeliveryTimingSample::Smoke {
                lane,
                status: crate::host::DeliveryTimingStatus::Passed,
                smoke_ms: 250,
            },
        ]
    }

    impl DeliveryDevice for RequestRecorder {
        fn connect(&mut self) -> AgentResult<()> {
            Ok(())
        }

        fn read_development_manifest(&mut self) -> AgentResult<String> {
            Ok(String::new())
        }

        fn read_active_runtime(&mut self) -> AgentResult<crate::host::ActiveRuntime> {
            Ok(crate::host::ActiveRuntime::new(
                Some("/media/fat/MiSTer_MagiKDev"),
                Some("LauncherActive"),
            ))
        }

        fn deliver_runtime(
            &mut self,
            _delivery: RuntimeDelivery,
            timings: &mut Vec<crate::host::DeliveryTimingSample>,
        ) -> AgentResult<()> {
            self.0.borrow_mut().push(DeliveryCall::Runtime);
            timings.extend(sample_timings(crate::host::DeliveryLane::Runtime));
            Ok(())
        }

        fn deliver_platform(
            &mut self,
            _stage: PathBuf,
            _expected_sha256: String,
            timings: &mut Vec<crate::host::DeliveryTimingSample>,
        ) -> AgentResult<()> {
            self.0.borrow_mut().push(DeliveryCall::Platform);
            timings.extend(sample_timings(crate::host::DeliveryLane::Platform));
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeActions {
        fail_at: Option<Phase>,
        rollback_fails: bool,
        visited: Vec<Step>,
        timings: Vec<DeliveryPhaseTiming>,
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

        fn record_timing(&mut self, sample: DeliveryPhaseTiming) {
            self.timings.push(sample);
        }
    }

    const PHASES: [Phase; 15] = [
        Phase::Classify,
        Phase::ValidateCommit,
        Phase::Connect,
        Phase::GithubResolution,
        Phase::Reconcile,
        Phase::RuntimeBuild,
        Phase::ManagerQualification,
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
        assert_eq!(actions.timings.len(), PHASES.len());
        assert!(
            actions
                .timings
                .iter()
                .all(|sample| { sample.status == crate::host::DeliveryTimingStatus::Passed })
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
        assert_eq!(actions.timings.last().unwrap().phase, "compensation");
    }

    #[test]
    fn exact_commit_identity_is_mandatory() {
        assert!(validate_commit_identity("abc", "abc", false, Some("abc")).is_ok());
        assert!(validate_commit_identity("other", "abc", false, None).is_err());
        assert!(validate_commit_identity("abc", "abc", true, None).is_err());
        assert!(validate_commit_identity("abc", "abc", false, Some("other")).is_err());
    }

    #[test]
    fn non_development_launcher_promotes_delivery_to_platform() {
        for active in [
            crate::host::ActiveRuntime::new(
                Some("/media/fat/MiSTer_MagiK"),
                Some("LauncherActive"),
            ),
            crate::host::ActiveRuntime::new(
                Some("/media/fat/MiSTer_MagiKDev"),
                Some("LauncherSuspended"),
            ),
            crate::host::ActiveRuntime::new(Some("unknown"), Some("Unconfigured")),
            crate::host::ActiveRuntime::new(None, None),
        ] {
            assert_eq!(
                reconcile_active_runtime(DeliveryDecision::NoOp, &active),
                DeliveryDecision::Platform
            );
        }
        let development = crate::host::ActiveRuntime::new(
            Some("/media/fat/MiSTer_MagiKDev"),
            Some("LauncherActive"),
        );
        assert_eq!(
            reconcile_active_runtime(DeliveryDecision::Runtime, &development),
            DeliveryDecision::Runtime
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
    fn runtime_delivery_uses_no_reboot_transaction() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut actions = scenario_actions(
            crate::deploy::DeploymentKind::Runtime,
            RequestRecorder(Rc::clone(&requests)),
        );
        actions.run(Phase::RemoteInventoryUpload).unwrap();
        assert!(matches!(
            requests.borrow().as_slice(),
            [DeliveryCall::Runtime]
        ));
        assert_eq!(
            actions.timing_samples,
            sample_timings(crate::host::DeliveryLane::Runtime)
        );
    }

    #[test]
    fn runtime_delivery_resolves_platform_before_skipping_platform_staging_and_reboot() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let actions = scenario_actions(
            crate::deploy::DeploymentKind::Runtime,
            RequestRecorder(Rc::clone(&requests)),
        );
        for phase in [
            Phase::ManagerQualification,
            Phase::DatabasePreparation,
            Phase::RebootIfNeeded,
            Phase::Activate,
            Phase::Smoke,
        ] {
            assert!(
                !actions.should_run(phase),
                "runtime delivery unexpectedly runs {phase:?}"
            );
        }
        assert!(actions.should_run(Phase::GithubResolution));
        assert!(actions.should_run(Phase::RuntimeBuild));
        assert!(actions.should_run(Phase::LocalStaging));
        assert!(actions.should_run(Phase::RemoteInventoryUpload));
    }

    #[test]
    fn no_op_delivery_has_no_transfer_or_smoke_samples() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut actions = scenario_actions(
            crate::deploy::DeploymentKind::Runtime,
            RequestRecorder(Rc::clone(&requests)),
        );
        actions.decision = DeliveryDecision::NoOp;

        assert!(!actions.should_run(Phase::RemoteInventoryUpload));
        assert!(actions.timing_samples.is_empty());
        assert!(requests.borrow().is_empty());
    }

    #[test]
    fn deterministic_platform_uses_one_transaction_request() {
        let requests = Rc::new(RefCell::new(Vec::new()));
        let mut actions = scenario_actions(
            crate::deploy::DeploymentKind::Platform,
            RequestRecorder(Rc::clone(&requests)),
        );
        actions.run(Phase::RemoteInventoryUpload).unwrap();
        assert!(matches!(
            requests.borrow().as_slice(),
            [DeliveryCall::Platform]
        ));
        assert_eq!(
            actions.timing_samples,
            sample_timings(crate::host::DeliveryLane::Platform)
        );
    }

    #[test]
    fn timing_samples_render_as_stable_machine_readable_events() {
        let [transfer, smoke] = sample_timings(crate::host::DeliveryLane::Runtime);
        assert_eq!(
            render_delivery_timing(transfer),
            (
                EventKind::Completed,
                "delivery-transfer",
                "delivery_transfer_tsv\tscope=device\tlane=runtime\tstatus=passed\tseconds=0.900\tfiles=3\tbytes=1500\tupload_seconds=0.500\tupload_ms=500\tdeploy_ms=900\tbytes_per_second=3000".into(),
            )
        );
        assert_eq!(
            render_delivery_timing(smoke),
            (
                EventKind::Completed,
                "delivery-smoke",
                "delivery_smoke_tsv\tscope=device\tlane=runtime\tstatus=passed\tseconds=0.250\tsmoke_ms=250".into(),
            )
        );
        let stage = crate::host::DeliveryTimingSample::Stage {
            lane: crate::host::DeliveryLane::Runtime,
            stage: "activate",
            status: crate::host::DeliveryTimingStatus::Passed,
            elapsed_ms: 125,
        };
        assert_eq!(
            render_delivery_timing(stage),
            (
                EventKind::Completed,
                "delivery-device-stage",
                "delivery_stage_tsv\tscope=device\tlane=runtime\tstage=activate\tstatus=passed\tseconds=0.125".into(),
            )
        );
        let failed_transfer = crate::host::DeliveryTimingSample::Transfer {
            lane: crate::host::DeliveryLane::Platform,
            status: crate::host::DeliveryTimingStatus::Failed,
            metrics: crate::host::DeliveryTransferMetrics {
                files: 1,
                bytes: 1_024,
                upload_ms: 100,
                deploy_ms: 150,
            },
        };
        let failed_smoke = crate::host::DeliveryTimingSample::Smoke {
            lane: crate::host::DeliveryLane::Platform,
            status: crate::host::DeliveryTimingStatus::Failed,
            smoke_ms: 400,
        };
        assert_eq!(
            render_delivery_timing(failed_transfer).0,
            EventKind::Warning
        );
        assert_eq!(render_delivery_timing(failed_smoke).0, EventKind::Warning);
    }

    #[test]
    fn timing_events_are_retained_without_progress_coalescing() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-delivery-timing-evidence-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let evidence = crate::evidence::Evidence::open_at(&root).unwrap();
        let request = crate::request::RawRequest::capture([OsString::from("agent-cli")]);
        evidence.begin_request(&request).unwrap();
        let mut reporter = Reporter::new(&evidence, crate::cli::OutputFormat::Human, &request.id);

        let mut samples = sample_timings(crate::host::DeliveryLane::Runtime).to_vec();
        samples.push(crate::host::DeliveryTimingSample::Smoke {
            lane: crate::host::DeliveryLane::Runtime,
            status: crate::host::DeliveryTimingStatus::Failed,
            smoke_ms: 400,
        });
        emit_delivery_timings(&mut reporter, &samples).unwrap();

        let detail = evidence.run_detail(&request.id).unwrap().unwrap();
        assert_eq!(detail.events.len(), 3);
        assert_eq!(detail.events[0].phase, "delivery-transfer");
        assert_eq!(detail.events[0].kind, "completed");
        assert_eq!(detail.events[1].phase, "delivery-smoke");
        assert_eq!(detail.events[1].kind, "completed");
        assert_eq!(detail.events[2].phase, "delivery-smoke");
        assert_eq!(detail.events[2].kind, "warning");
        let _ = fs::remove_dir_all(root);
    }

    fn scenario_actions(
        kind: crate::deploy::DeploymentKind,
        device: RequestRecorder,
    ) -> ProcessActions<'static, RequestRecorder> {
        let mut deployment = crate::deploy::plan(Path::new("."), Vec::new()).unwrap();
        deployment.kind = kind;
        let decision = match kind {
            crate::deploy::DeploymentKind::Runtime => DeliveryDecision::Runtime,
            crate::deploy::DeploymentKind::Platform => DeliveryDecision::Platform,
        };
        ProcessActions {
            repository: Path::new("."),
            deployment,
            expected_commit: "revision",
            artifact_sha256: Some("a".repeat(64)),
            decision,
            manager_artifact: None,
            main_revision: None,
            installed_manifest: None,
            phase_timings: Vec::new(),
            timing_samples: Vec::new(),
            stage: PathBuf::from("stage"),
            device,
        }
    }
}
