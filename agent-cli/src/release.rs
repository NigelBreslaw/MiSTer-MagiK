// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use std::io::{self, IsTerminal, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    ConfirmAttendance,
    RecoveryPreflight,
    Runtime,
    Catalog,
    InputAndHandoff,
    Display,
    LatchV5Stress,
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
            Self::LatchV5Stress => "latch-v5-six-hour-stress",
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

trait ReleaseDevice {
    fn begin(&mut self) -> AgentResult<()>;
    fn qualify_runtime(&mut self) -> AgentResult<()>;
    fn qualify_catalog(&mut self) -> AgentResult<()>;
    fn qualify_input_and_handoff(&mut self) -> AgentResult<()>;
    fn qualify_display(&mut self) -> AgentResult<()>;
    fn qualify_latch_v5_stress(&mut self) -> AgentResult<()>;
    fn qualify_recovery(&mut self) -> AgentResult<()>;
    fn restore(&mut self) -> AgentResult<()>;
}

impl ReleaseDevice for DeviceClient {
    fn begin(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::begin_release_qualification)
    }

    fn qualify_runtime(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::qualify_release_runtime)
    }

    fn qualify_catalog(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::qualify_release_catalog)
    }

    fn qualify_input_and_handoff(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::qualify_release_input_and_handoff)
    }

    fn qualify_display(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::qualify_release_display)
    }

    fn qualify_latch_v5_stress(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::qualify_release_latch_v5_stress)
    }

    fn qualify_recovery(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::qualify_release_recovery)
    }

    fn restore(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::restore_release_qualification)
    }
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
        (Phase::LatchV5Stress, 65),
        (Phase::Display, 90),
        (Phase::Recovery, 92),
        (Phase::Restore, 100),
    ];
    crate::workflow::run_restorable_phases(
        actions,
        PHASES,
        progress,
        |actions, phase| actions.run(phase),
        ReleaseActions::restore,
        |actions| actions.armed(),
        |phase| phase == Phase::Restore,
        Phase::label,
        "release qualification",
    )
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

struct ProcessActions<D = DeviceClient> {
    device: D,
    confirmed: bool,
    armed: bool,
}

impl<D> ProcessActions<D> {
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

impl<D: ReleaseDevice> ReleaseActions for ProcessActions<D> {
    fn run(&mut self, phase: Phase) -> AgentResult<()> {
        match phase {
            Phase::ConfirmAttendance => self.confirm_attendance(),
            Phase::RecoveryPreflight => {
                if !self.confirmed {
                    return Err("attendance was not confirmed".into());
                }
                self.armed = true;
                self.device.begin()
            }
            Phase::Runtime => self.device.qualify_runtime(),
            Phase::Catalog => self.device.qualify_catalog(),
            Phase::InputAndHandoff => self.device.qualify_input_and_handoff(),
            Phase::Display => self.device.qualify_display(),
            Phase::LatchV5Stress => self.device.qualify_latch_v5_stress(),
            Phase::Recovery => self.device.qualify_recovery(),
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
        self.device.restore()?;
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
            Phase::LatchV5Stress,
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
        assert!(
            run_workflow(&mut actions, &mut |_, _| Ok(()))
                .unwrap_err()
                .is_recovery_required()
        );
    }
}
