// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::deploy::{DeploymentKind, DeploymentPlan};
use crate::device::DeviceClient;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{DeviceRequest, Layout, MainSelection};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
    fn run(&mut self, phase: Phase) -> Result<(), String>;
    fn compensate(&mut self) -> Result<(), String>;
}

pub fn run_transaction(
    actions: &mut dyn DeliveryActions,
    progress: &mut dyn FnMut(Step, u8) -> Result<(), String>,
) -> Result<(), String> {
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
                    Ok(()) => Err(format!("cancelled: {error}; rollback=complete")),
                    Err(rollback) => Err(format!(
                        "recovery_required: delivery cancelled ({error}); rollback failed ({rollback})"
                    )),
                };
            }
            return Err(format!("cancelled: {error}"));
        }
        match actions.run(*phase) {
            Ok(()) => mutation_started |= phase.starts_mutation(),
            Err(error) if mutation_started || phase.may_have_mutated() => {
                let _ = progress(Step::Compensation, 95);
                return match actions.compensate() {
                    Ok(()) => Err(format!("{}: {error}; rollback=complete", phase.label())),
                    Err(rollback) => Err(format!(
                        "recovery_required: {} failed ({error}); rollback failed ({rollback})",
                        phase.label()
                    )),
                };
            }
            Err(error) => return Err(format!("{}: {error}", phase.label())),
        }
    }
    Ok(())
}

pub fn execute(
    repository: &Path,
    deployment: &DeploymentPlan,
    expected_commit: &str,
    reporter: &mut Reporter<'_>,
) -> Result<Outcome, String> {
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
        Step::Action(phase) => reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("delivery {}", phase.label()),
            Some(percent),
        ),
        Step::Compensation => reporter.emit(
            EventKind::Warning,
            "compensate",
            "delivery failed; restoring verified snapshot",
            Some(percent),
        ),
    })?;
    Ok(Outcome::Passed)
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
    fn validate_commit(&self) -> Result<(), String> {
        let head = git_value(self.repository, &["rev-parse", "HEAD"])?;
        let dirty = !git_value(self.repository, &["status", "--porcelain"])?.is_empty();
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

    fn qualify(&mut self) -> Result<(), String> {
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

    fn smoke(&mut self) -> Result<(), String> {
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
) -> Result<(), String> {
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
    fn run(&mut self, phase: Phase) -> Result<(), String> {
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

    fn compensate(&mut self) -> Result<(), String> {
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
) -> Result<(), String> {
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
    run_bounded(
        repository,
        "scripts/fetch-game-databases-release.sh",
        &[databases.display().to_string()],
    )?;
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
        git_value(repository, &["rev-parse", "HEAD"])?,
        "--layout".into(),
        "dev".into(),
    ];
    run_bounded(repository, "/usr/bin/python3", &args)
}

fn copy(from: PathBuf, to: PathBuf) -> Result<(), String> {
    fs::copy(&from, &to)
        .map(|_| ())
        .map_err(|error| format!("cannot copy {}: {error}", from.display()))
}

fn git_value(repository: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

fn run_bounded(repository: &Path, program: &str, args: &[String]) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(repository)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot start {program}: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} exited with {status}")),
            Ok(None) if started.elapsed() < PREPARE_DEADLINE => {
                thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} exceeded its preparation deadline"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for {program}: {error}"));
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
        rollback_fails: bool,
        visited: Vec<Step>,
    }

    impl DeliveryActions for FakeActions {
        fn run(&mut self, phase: Phase) -> Result<(), String> {
            self.visited.push(Step::Action(phase));
            if self.fail_at == Some(phase) {
                Err("injected failure".into())
            } else {
                Ok(())
            }
        }

        fn compensate(&mut self) -> Result<(), String> {
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
            assert!(error.contains("injected failure"));
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
        assert!(error.starts_with("recovery_required:"));
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
        assert!(error.starts_with("cancelled:"));
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
}
