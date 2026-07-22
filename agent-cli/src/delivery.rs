// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::deploy::{DeploymentKind, DeploymentPlan};
use crate::device::DeviceClient;
use crate::error::{AgentError, AgentResult};
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{DeviceRequest, Layout, MainSelection};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const REMOTE_RUNTIME: &str = "/media/fat/mister-magik-dev/mister-magik-fb";
const PREPARE_DEADLINE: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Classify,
    ValidateCommit,
    QualifyArtifact,
    Connect,
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
            Self::QualifyArtifact => "qualify-artifact",
            Self::Connect => "connect",
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
        (Phase::QualifyArtifact, 15),
        (Phase::Connect, 38),
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
    deployment: &DeploymentPlan,
    expected_commit: &str,
    reporter: &mut Reporter<'_>,
) -> AgentResult<Outcome> {
    let mut actions = ProcessActions {
        repository,
        deployment,
        expected_commit,
        artifact_sha256: None,
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
    Ok(Outcome::Passed)
}

pub fn cleanup_workspace(repository: &Path) -> Result<(), String> {
    let workspace = repository.join("build/agent-deploy");
    if workspace.exists() {
        fs::remove_dir_all(&workspace).map_err(|error| {
            format!(
                "cannot clear delivery workspace {}: {error}",
                workspace.display()
            )
        })?;
    }
    Ok(())
}

struct ProcessActions<'a> {
    repository: &'a Path,
    deployment: &'a DeploymentPlan,
    expected_commit: &'a str,
    artifact_sha256: Option<String>,
    stage: PathBuf,
    device: DeviceClient,
}

impl ProcessActions<'_> {
    fn validate_commit(&self) -> AgentResult<()> {
        let head = crate::git::value(self.repository, &["rev-parse", "HEAD"])?;
        let dirty = !crate::git::value(self.repository, &["status", "--porcelain"])?.is_empty();
        validate_commit_identity(
            &head,
            self.expected_commit,
            dirty,
            self.deployment
                .platform_candidate
                .as_ref()
                .map(|candidate| candidate.head_sha.as_str()),
        )
    }

    fn qualify(&mut self) -> AgentResult<()> {
        crate::build::execute_quiet(self.repository, &self.deployment.build)?;
        let receipt = self.deployment.build.verify(self.repository)?;
        if receipt.source_commit != self.expected_commit || receipt.source_dirty {
            return Err("runtime artifact was not built from the exact clean commit".into());
        }
        self.artifact_sha256 = Some(receipt.binary_sha256);
        if self.deployment.kind == DeploymentKind::Platform {
            let candidate = self
                .deployment
                .platform_candidate
                .as_ref()
                .ok_or("platform delivery is missing its qualified candidate")?;
            if self.stage.exists() {
                fs::remove_dir_all(&self.stage).map_err(|error| error.to_string())?;
            }
            fs::create_dir_all(self.stage.join("fpga")).map_err(|error| error.to_string())?;
            prepare_stage(
                self.repository,
                candidate,
                &self.stage,
                self.deployment.build.artifact(),
            )?;
        }
        Ok(())
    }

    fn smoke(&mut self) -> AgentResult<()> {
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
            Phase::QualifyArtifact => self.qualify(),
            Phase::Connect => self.device.execute(DeviceRequest::Discover).map(|_| ()),
            Phase::Snapshot => match self.deployment.kind {
                DeploymentKind::Runtime => self
                    .device
                    .execute(DeviceRequest::SnapshotRuntime {
                        remote: REMOTE_RUNTIME.into(),
                    })
                    .map(|_| ()),
                DeploymentKind::Platform => self
                    .device
                    .execute(DeviceRequest::SnapshotPlatform)
                    .map(|_| ()),
            },
            Phase::Stage => match self.deployment.kind {
                DeploymentKind::Runtime => self
                    .device
                    .execute(DeviceRequest::DeployRuntime {
                        local: self.deployment.build.artifact().to_path_buf(),
                        remote: REMOTE_RUNTIME.into(),
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
                DeploymentKind::Runtime => Ok(()),
                DeploymentKind::Platform => self
                    .device
                    .execute(DeviceRequest::SelectMain(MainSelection::Development))
                    .map(|_| ()),
            },
            Phase::RebootIfNeeded => match self.deployment.kind {
                DeploymentKind::Runtime => Ok(()),
                DeploymentKind::Platform => {
                    self.device.execute(DeviceRequest::RebootWait).map(|_| ())
                }
            },
            Phase::Smoke => self.smoke(),
            Phase::Complete => match self.deployment.kind {
                DeploymentKind::Runtime => self
                    .device
                    .execute(DeviceRequest::CommitRuntime {
                        remote: REMOTE_RUNTIME.into(),
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
                self.device.execute(DeviceRequest::RollbackRuntime {
                    remote: REMOTE_RUNTIME.into(),
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
) -> AgentResult<()> {
    let manifest: Value =
        serde_json::from_slice(&fs::read(&candidate.manifest).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid platform candidate manifest: {error}"))?;
    let main_revision = manifest
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
        ("main/MiSTer_MagiK", "MiSTer_MagiKDev"),
        (
            "scanout/mister_magik_scanout_slots.ko",
            "mister_magik_scanout_slots.ko",
        ),
        (
            "scanout/provenance.txt",
            "mister_magik_scanout_slots.metadata.txt",
        ),
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
    copy(repository.join(gui_artifact), stage.join("mister-magik-fb"))?;
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
    let args = vec![
        "scripts/release/platform/platform-manifest.py".into(),
        "generate".into(),
        "--output".into(),
        stage.join("platform-v2.manifest").display().to_string(),
        "--main".into(),
        stage.join("MiSTer_MagiKDev").display().to_string(),
        "--gui".into(),
        stage.join("mister-magik-fb").display().to_string(),
        "--scanout-module".into(),
        stage
            .join("mister_magik_scanout_slots.ko")
            .display()
            .to_string(),
        "--scanout-metadata".into(),
        stage
            .join("mister_magik_scanout_slots.metadata.txt")
            .display()
            .to_string(),
        "--latch-rbf".into(),
        stage
            .join("fpga/menu-magik-vblank-latch.rbf")
            .display()
            .to_string(),
        "--latch-metadata".into(),
        stage
            .join("fpga/menu-magik-vblank-latch.metadata.txt")
            .display()
            .to_string(),
        "--main-revision".into(),
        main_revision.into(),
        "--magik-revision".into(),
        crate::git::value(repository, &["rev-parse", "HEAD"])?,
        "--layout".into(),
        "dev".into(),
    ];
    run_bounded(repository, "/usr/bin/python3", &args)
}

fn copy(from: PathBuf, to: PathBuf) -> AgentResult<()> {
    Ok(fs::copy(&from, &to)
        .map(|_| ())
        .map_err(|error| format!("cannot copy {}: {error}", from.display()))?)
}

fn prepare_game_databases(repository: &Path, output: &Path) -> AgentResult<()> {
    let (owner, tag, version) = crate::platform_ci::latest_game_database_release(repository)?;
    let cache_root = repository.join("build/agent-deploy/release-cache/game-databases");
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
    run_bounded(
        repository,
        "/usr/bin/python3",
        &[
            repository
                .join("scripts/release/databases/game-databases-bundle.py")
                .display()
                .to_string(),
            "extract-release".into(),
            release.display().to_string(),
            "--output".into(),
            output.display().to_string(),
        ],
    )
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
    let status = crate::process::wait(&mut child, Some(PREPARE_DEADLINE), program, || Ok(()))?;
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

    const PHASES: [Phase; 10] = [
        Phase::Classify,
        Phase::ValidateCommit,
        Phase::QualifyArtifact,
        Phase::Connect,
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
    fn delivery_workspace_is_removed_after_use() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-delivery-workspace-test-{}",
            std::process::id()
        ));
        let workspace = root.join("build/agent-deploy/stage/commit");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("artifact"), b"generated").unwrap();

        cleanup_workspace(&root).unwrap();

        assert!(!root.join("build/agent-deploy").exists());
        let _ = fs::remove_dir_all(root);
    }
}
