// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::error::{AgentError, AgentResult};
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::DeviceRequest;
use std::io::{self, IsTerminal, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    ConfirmAttendance,
    RecoveryPreflight,
    Runtime,
    Catalog,
    InputAndHandoff,
    Display,
    Recovery,
    Restore,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::ConfirmAttendance => "confirm-attendance",
            Self::RecoveryPreflight => "recovery-preflight",
            Self::Runtime => "runtime",
            Self::Catalog => "catalog",
            Self::InputAndHandoff => "input-and-handoff",
            Self::Display => "display",
            Self::Recovery => "recovery",
            Self::Restore => "restore",
        }
    }
}

pub trait ReleaseActions {
    fn run(&mut self, phase: Phase) -> AgentResult<()>;
    fn armed(&self) -> bool;
    fn restore(&mut self) -> AgentResult<()>;
}

pub fn run_workflow(
    actions: &mut dyn ReleaseActions,
    progress: &mut dyn FnMut(Phase, u8) -> AgentResult<()>,
) -> AgentResult<()> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::ConfirmAttendance, 2),
        (Phase::RecoveryPreflight, 10),
        (Phase::Runtime, 25),
        (Phase::Catalog, 40),
        (Phase::InputAndHandoff, 58),
        (Phase::Display, 72),
        (Phase::Recovery, 88),
        (Phase::Restore, 100),
    ];
    for (phase, percent) in PHASES {
        if let Err(error) = progress(*phase, *percent) {
            return restore_after_error(actions, AgentError::cancelled(error));
        }
        let result = if *phase == Phase::Restore {
            actions.restore()
        } else {
            actions.run(*phase)
        };
        if let Err(error) = result {
            return restore_after_error(actions, AgentError::phase(phase.label(), error));
        }
    }
    Ok(())
}

fn restore_after_error(actions: &mut dyn ReleaseActions, error: AgentError) -> AgentResult<()> {
    if !actions.armed() {
        return Err(error);
    }
    match actions.restore() {
        Ok(()) => Err(format!("{error}; restore=complete").into()),
        Err(restore) => Err(AgentError::recovery_required(
            error.to_string(),
            format!("release qualification restore failed ({restore})"),
        )),
    }
}

pub fn execute(reporter: &mut Reporter<'_>) -> AgentResult<Outcome> {
    let mut actions = ProcessActions {
        device: DeviceClient::default(),
        confirmed: false,
        armed: false,
    };
    run_workflow(&mut actions, &mut |phase, percent| {
        Ok(reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("release qualification {}", phase.label()),
            Some(percent),
        )?)
    })?;
    Ok(Outcome::Passed)
}

struct ProcessActions {
    device: DeviceClient,
    confirmed: bool,
    armed: bool,
}

impl ProcessActions {
    fn confirm_attendance(&mut self) -> AgentResult<()> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err("attendance_required: run this command in an attended terminal".into());
        }
        print!("Type QUALIFY to confirm continuous attendance and a non-network recovery path: ");
        io::stdout().flush().map_err(|error| error.to_string())?;
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        if answer.trim() != "QUALIFY" {
            return Err("attendance_refused: release qualification was not armed".into());
        }
        self.confirmed = true;
        Ok(())
    }
}

impl ReleaseActions for ProcessActions {
    fn run(&mut self, phase: Phase) -> AgentResult<()> {
        match phase {
            Phase::ConfirmAttendance => self.confirm_attendance(),
            Phase::RecoveryPreflight => {
                if !self.confirmed {
                    return Err("attendance was not confirmed".into());
                }
                self.armed = true;
                self.device
                    .execute(DeviceRequest::BeginReleaseQualification)
                    .map(|_| ())
            }
            Phase::Runtime => self
                .device
                .execute(DeviceRequest::QualifyReleaseRuntime)
                .map(|_| ()),
            Phase::Catalog => self
                .device
                .execute(DeviceRequest::QualifyReleaseCatalog)
                .map(|_| ()),
            Phase::InputAndHandoff => self
                .device
                .execute(DeviceRequest::QualifyReleaseInputAndHandoff)
                .map(|_| ()),
            Phase::Display => self
                .device
                .execute(DeviceRequest::QualifyReleaseDisplay)
                .map(|_| ()),
            Phase::Recovery => self
                .device
                .execute(DeviceRequest::QualifyReleaseRecovery)
                .map(|_| ()),
            Phase::Restore => unreachable!("restore has a dedicated action"),
        }
    }

    fn armed(&self) -> bool {
        self.armed
    }

    fn restore(&mut self) -> AgentResult<()> {
        if !self.armed {
            return Ok(());
        }
        self.device
            .execute(DeviceRequest::RestoreReleaseQualification)?;
        self.armed = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeActions {
        refuse: bool,
        fail_at: Option<Phase>,
        armed: bool,
        restored: usize,
        restore_fails: bool,
    }

    impl ReleaseActions for FakeActions {
        fn run(&mut self, phase: Phase) -> AgentResult<()> {
            if phase == Phase::ConfirmAttendance && self.refuse {
                return Err("attendance refused".into());
            }
            if phase == Phase::RecoveryPreflight {
                self.armed = true;
            }
            if self.fail_at == Some(phase) {
                Err("injected failure".into())
            } else {
                Ok(())
            }
        }

        fn armed(&self) -> bool {
            self.armed
        }

        fn restore(&mut self) -> AgentResult<()> {
            self.restored += 1;
            self.armed = false;
            if self.restore_fails {
                Err("restore failed".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn attendance_refusal_never_arms_or_restores() {
        let mut actions = FakeActions {
            refuse: true,
            ..FakeActions::default()
        };
        assert!(run_workflow(&mut actions, &mut |_, _| Ok(())).is_err());
        assert!(!actions.armed);
        assert_eq!(actions.restored, 0);
    }

    #[test]
    fn every_armed_failure_and_interruption_restores() {
        for phase in [
            Phase::RecoveryPreflight,
            Phase::Runtime,
            Phase::Catalog,
            Phase::InputAndHandoff,
            Phase::Display,
            Phase::Recovery,
        ] {
            let mut actions = FakeActions {
                fail_at: Some(phase),
                ..FakeActions::default()
            };
            let error = run_workflow(&mut actions, &mut |_, _| Ok(())).unwrap_err();
            assert!(error.to_string().contains("restore=complete"));
            assert_eq!(actions.restored, 1);
        }
        let mut actions = FakeActions::default();
        let error = run_workflow(&mut actions, &mut |phase, _| {
            if phase == Phase::Display {
                Err("interrupted".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(error.to_string().starts_with("cancelled:"));
        assert_eq!(actions.restored, 1);
    }

    #[test]
    fn cleanup_failure_requires_recovery() {
        let mut actions = FakeActions {
            fail_at: Some(Phase::Recovery),
            restore_fails: true,
            ..FakeActions::default()
        };
        assert!(run_workflow(&mut actions, &mut |_, _| Ok(()))
            .unwrap_err()
            .is_recovery_required());
    }
}
