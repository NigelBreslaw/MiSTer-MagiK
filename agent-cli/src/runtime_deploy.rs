// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::deploy::{DeploymentKind, DeploymentPlan};
use crate::device::DeviceClient;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::{DeviceRequest, Layout};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
const REMOTE_RUNTIME: &str = "/media/fat/mister-magik-dev/mister-magik-fb";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Resolve,
    Build,
    VerifyLocal,
    Snapshot,
    Stage,
    Suspend,
    Activate,
    Resume,
    VerifyHealth,
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
            Self::Resume => "resume",
            Self::VerifyHealth => "verify-health",
            Self::Complete => "complete",
        }
    }
}

pub trait RuntimeActions {
    fn run(&mut self, phase: Phase) -> Result<(), String>;
}

pub fn run_transaction(
    actions: &mut dyn RuntimeActions,
    progress: &mut dyn FnMut(Phase, u8) -> Result<(), String>,
) -> Result<(), String> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::Resolve, 2),
        (Phase::Build, 10),
        (Phase::VerifyLocal, 35),
        (Phase::Snapshot, 45),
        (Phase::Stage, 55),
        (Phase::Suspend, 65),
        (Phase::Activate, 75),
        (Phase::Resume, 85),
        (Phase::VerifyHealth, 95),
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
    if deployment.kind != DeploymentKind::Runtime {
        return Err("runtime deployment received a non-runtime plan".into());
    }
    let mut actions = ProcessActions {
        repository,
        deployment,
        device: DeviceClient::default(),
    };
    run_transaction(&mut actions, &mut |phase, percent| {
        reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("deployment {}", phase.label()),
            Some(percent),
        )
    })?;
    Ok(Outcome::Passed)
}

struct ProcessActions<'a> {
    repository: &'a Path,
    deployment: &'a DeploymentPlan,
    device: DeviceClient,
}

impl RuntimeActions for ProcessActions<'_> {
    fn run(&mut self, phase: Phase) -> Result<(), String> {
        match phase {
            Phase::Resolve | Phase::Complete => Ok(()),
            Phase::Build => crate::build::execute_quiet(self.repository, &self.deployment.build),
            Phase::VerifyLocal => self.deployment.build.verify(self.repository).map(|_| ()),
            Phase::Snapshot => self.device.execute(DeviceRequest::Status).map(|_| ()),
            Phase::Stage => self
                .device
                .execute(DeviceRequest::DeployRuntime {
                    local: self.deployment.build.artifact.clone(),
                    remote: REMOTE_RUNTIME.into(),
                })
                .map(|_| ()),
            Phase::Suspend | Phase::Activate | Phase::Resume => {
                // The resident agent owns these indivisible phases and rolls back before
                // deploy-magik-bin returns an error. They are retained in the state model
                // so failure injection and evidence preserve the transaction boundary.
                Ok(())
            }
            Phase::VerifyHealth => self
                .device
                .execute(DeviceRequest::VerifyHealth(Layout::Development))
                .map(|_| ()),
        }
    }
}

pub(crate) fn run_bounded(
    repository: &Path,
    program: &str,
    args: &[String],
    deadline: Duration,
) -> Result<(), String> {
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
            Ok(None) if started.elapsed() < deadline => thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} exceeded its {}s deadline",
                    deadline.as_secs()
                ));
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
        visited: Vec<Phase>,
    }

    impl RuntimeActions for FakeActions {
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
    fn successful_transaction_visits_every_phase_in_order() {
        let mut actions = FakeActions::default();
        run_transaction(&mut actions, &mut |_, _| Ok(())).unwrap();
        assert_eq!(
            actions.visited,
            vec![
                Phase::Resolve,
                Phase::Build,
                Phase::VerifyLocal,
                Phase::Snapshot,
                Phase::Stage,
                Phase::Suspend,
                Phase::Activate,
                Phase::Resume,
                Phase::VerifyHealth,
                Phase::Complete,
            ]
        );
    }

    #[test]
    fn every_phase_failure_stops_at_its_mutation_boundary() {
        let phases = [
            Phase::Resolve,
            Phase::Build,
            Phase::VerifyLocal,
            Phase::Snapshot,
            Phase::Stage,
            Phase::Suspend,
            Phase::Activate,
            Phase::Resume,
            Phase::VerifyHealth,
            Phase::Complete,
        ];
        for (index, phase) in phases.iter().enumerate() {
            let mut actions = FakeActions {
                fail_at: Some(*phase),
                visited: Vec::new(),
            };
            let error = run_transaction(&mut actions, &mut |_, _| Ok(())).unwrap_err();
            assert!(error.starts_with(phase.label()));
            assert_eq!(actions.visited, phases[..=index]);
        }
    }
}
