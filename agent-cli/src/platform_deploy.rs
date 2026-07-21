// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::deploy::{DeploymentKind, DeploymentPlan};
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use crate::runtime_deploy::run_bounded;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BUILD_DEADLINE: Duration = Duration::from_secs(30 * 60);
const PREPARE_DEADLINE: Duration = Duration::from_secs(10 * 60);
const DEVICE_DEADLINE: Duration = Duration::from_secs(5 * 60);
const REBOOT_DEADLINE: Duration = Duration::from_secs(3 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Resolve,
    Build,
    VerifyLocal,
    Snapshot,
    Stage,
    Suspend,
    Activate,
    Reboot,
    VerifyHealth,
    Cleanup,
    Complete,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::Build => "build",
            Self::VerifyLocal => "verify-local",
            Self::Snapshot => "snapshot",
            Self::Stage => "stage",
            Self::Suspend => "suspend",
            Self::Activate => "activate",
            Self::Reboot => "reboot",
            Self::VerifyHealth => "verify-health",
            Self::Cleanup => "cleanup",
            Self::Complete => "complete",
        }
    }
}

pub trait PlatformActions {
    fn run(&mut self, phase: Phase) -> Result<(), String>;
}

pub fn run_transaction(
    actions: &mut dyn PlatformActions,
    progress: &mut dyn FnMut(Phase, u8) -> Result<(), String>,
) -> Result<(), String> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::Resolve, 2),
        (Phase::Build, 10),
        (Phase::VerifyLocal, 30),
        (Phase::Snapshot, 40),
        (Phase::Stage, 50),
        (Phase::Suspend, 60),
        (Phase::Activate, 70),
        (Phase::Reboot, 80),
        (Phase::VerifyHealth, 92),
        (Phase::Cleanup, 98),
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
    deployment: &DeploymentPlan,
    reporter: &mut Reporter<'_>,
) -> Result<Outcome, String> {
    if deployment.kind != DeploymentKind::Platform {
        return Err("platform deployment received a non-platform plan".into());
    }
    let candidate = deployment
        .platform_candidate
        .as_ref()
        .ok_or("platform deployment is missing its verified CI candidate")?;
    let stage = repository
        .join("build/agent-deploy/stage")
        .join(&candidate.head_sha);
    let mut actions = ProcessActions {
        repository,
        deployment,
        stage,
    };
    run_transaction(&mut actions, &mut |phase, percent| {
        reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("platform deployment {}", phase.label()),
            Some(percent),
        )
    })?;
    Ok(Outcome::Passed)
}

struct ProcessActions<'a> {
    repository: &'a Path,
    deployment: &'a DeploymentPlan,
    stage: PathBuf,
}

impl PlatformActions for ProcessActions<'_> {
    fn run(&mut self, phase: Phase) -> Result<(), String> {
        let candidate = self.deployment.platform_candidate.as_ref().unwrap();
        match phase {
            Phase::Resolve => {
                if self.stage.exists() { fs::remove_dir_all(&self.stage).map_err(|e| e.to_string())?; }
                fs::create_dir_all(self.stage.join("fpga")).map_err(|e| e.to_string())
            }
            Phase::Build => run_bounded(self.repository, self.deployment.build.program,
                &self.deployment.build.args, BUILD_DEADLINE),
            Phase::VerifyLocal => {
                self.deployment.build.verify(self.repository)?;
                prepare_stage(
                    self.repository,
                    candidate,
                    &self.stage,
                    &self.deployment.build.artifact,
                )
            }
            Phase::Snapshot => run_bounded(self.repository, "scripts/mister",
                &["status".into(), "--json".into()], DEVICE_DEADLINE),
            Phase::Stage => run_bounded(self.repository, "scripts/mister",
                &["platform-deploy".into(), self.stage.display().to_string()], DEVICE_DEADLINE),
            Phase::Suspend | Phase::Activate => Ok(()),
            Phase::Reboot => {
                run_bounded(self.repository, "scripts/mister",
                    &["ini-select-main".into(), "MiSTer_MagiKDev".into()], DEVICE_DEADLINE)?;
                run_bounded(self.repository, "scripts/mister", &["reboot-wait".into()], REBOOT_DEADLINE)
            }
            Phase::VerifyHealth => run_bounded(self.repository, "scripts/mister",
                &["status".into(), "--json".into()], DEVICE_DEADLINE),
            Phase::Cleanup => run_bounded(self.repository, "scripts/mister", &["run".into(),
                "test ! -e /media/fat/mister-magik/launcher.env; test ! -e /media/fat/mister-magik-dev/launcher.env; test ! -e /tmp/mister-magik/fs-fault-session; test ! -e /media/fat/mister-magik/rebuild-on-next-boot; test ! -e /media/fat/mister-magik-dev/rebuild-on-next-boot".into()], DEVICE_DEADLINE),
            Phase::Complete => Ok(()),
        }
    }
}

fn prepare_stage(
    repository: &Path,
    candidate: &crate::platform_ci::Candidate,
    stage: &Path,
    gui_artifact: &Path,
) -> Result<(), String> {
    let manifest: Value =
        serde_json::from_slice(&fs::read(&candidate.manifest).map_err(|e| e.to_string())?)
            .map_err(|e| format!("invalid platform candidate manifest: {e}"))?;
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
        PREPARE_DEADLINE,
    )?;
    copy(
        extracted.join("main/MiSTer_MagiK"),
        stage.join("MiSTer_MagiKDev"),
    )?;
    copy(
        extracted.join("scanout/mister_magik_scanout_slots.ko"),
        stage.join("mister_magik_scanout_slots.ko"),
    )?;
    copy(
        extracted.join("scanout/provenance.txt"),
        stage.join("mister_magik_scanout_slots.metadata.txt"),
    )?;
    copy(
        extracted.join("fpga/patched/menu-magik-vblank-latch.rbf"),
        stage.join("fpga/menu-magik-vblank-latch.rbf"),
    )?;
    copy(
        extracted.join("fpga/patched/menu-magik-vblank-latch.metadata.txt"),
        stage.join("fpga/menu-magik-vblank-latch.metadata.txt"),
    )?;
    copy(repository.join(gui_artifact), stage.join("mister-magik-fb"))?;
    let databases = stage.join("databases");
    run_bounded(
        repository,
        "scripts/fetch-game-databases-release.sh",
        &[databases.display().to_string()],
        PREPARE_DEADLINE,
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
    let magik_revision = git_head(repository)?;
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
        magik_revision,
        "--layout".into(),
        "dev".into(),
    ];
    run_bounded(repository, "/usr/bin/python3", &args, PREPARE_DEADLINE)
}

fn copy(from: PathBuf, to: PathBuf) -> Result<(), String> {
    fs::copy(&from, &to)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {}: {e}", from.display()))
}

fn git_head(repository: &Path) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repository)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("cannot resolve MagiK revision".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        fail: Option<Phase>,
        seen: Vec<Phase>,
    }
    impl PlatformActions for Fake {
        fn run(&mut self, phase: Phase) -> Result<(), String> {
            self.seen.push(phase);
            if self.fail == Some(phase) {
                Err("injected".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn covers_all_boundaries() {
        let phases = [
            Phase::Resolve,
            Phase::Build,
            Phase::VerifyLocal,
            Phase::Snapshot,
            Phase::Stage,
            Phase::Suspend,
            Phase::Activate,
            Phase::Reboot,
            Phase::VerifyHealth,
            Phase::Cleanup,
            Phase::Complete,
        ];
        for fail in phases {
            let mut fake = Fake {
                fail: Some(fail),
                ..Fake::default()
            };
            assert!(run_transaction(&mut fake, &mut |_, _| Ok(())).is_err());
            assert_eq!(fake.seen.last(), Some(&fail));
        }
    }
    #[test]
    fn success_is_ordered() {
        let mut fake = Fake::default();
        run_transaction(&mut fake, &mut |_, _| Ok(())).unwrap();
        assert_eq!(fake.seen.len(), 11);
    }
}
