// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
use mister_tool::transport::DeviceRequest;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Discover,
    HostFacts,
    DeviceFacts,
    Correlate,
    SafeRepair,
    Recheck,
    Report,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::HostFacts => "host-facts",
            Self::DeviceFacts => "device-facts",
            Self::Correlate => "correlate",
            Self::SafeRepair => "safe-repair",
            Self::Recheck => "recheck",
            Self::Report => "report",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct DeviceFacts {
    pub main_running: bool,
    pub launcher_running: bool,
    pub agent_running: bool,
    pub credentials_ready: bool,
    pub firmware_compatible: bool,
    pub reboot_unstable: bool,
    pub arming_files: u64,
    pub temporary_state: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticReport {
    pub status: &'static str,
    pub repaired_temporary_state: bool,
    pub next_action: Option<String>,
}

pub trait DiagnoseActions {
    fn run(&mut self, phase: Phase) -> Result<(), String>;
}

pub fn run_workflow(
    actions: &mut dyn DiagnoseActions,
    progress: &mut dyn FnMut(Phase, u8) -> Result<(), String>,
) -> Result<(), String> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::Discover, 5),
        (Phase::HostFacts, 18),
        (Phase::DeviceFacts, 35),
        (Phase::Correlate, 52),
        (Phase::SafeRepair, 68),
        (Phase::Recheck, 84),
        (Phase::Report, 100),
    ];
    for (phase, percent) in PHASES {
        progress(*phase, *percent).map_err(|error| format!("cancelled: {error}"))?;
        actions
            .run(*phase)
            .map_err(|error| format!("{}: {error}", phase.label()))?;
    }
    Ok(())
}

pub fn execute(repository: &Path, reporter: &mut Reporter<'_>) -> Result<Outcome, String> {
    let mut actions = ProcessActions {
        repository,
        device: DeviceClient::default(),
        facts: None,
        report: None,
        repair_needed: false,
        repaired: false,
    };
    run_workflow(&mut actions, &mut |phase, percent| {
        reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("diagnose {}", phase.label()),
            Some(percent),
        )
    })?;
    let report = actions.report.ok_or("diagnosis produced no report")?;
    reporter.emit(
        if report.next_action.is_some() {
            EventKind::Warning
        } else {
            EventKind::Progress
        },
        "diagnostic-report",
        &serde_json::to_string(&report).map_err(|error| error.to_string())?,
        Some(100),
    )?;
    Ok(if report.next_action.is_some() {
        Outcome::ExternalRequired
    } else {
        Outcome::Passed
    })
}

struct ProcessActions<'a> {
    repository: &'a Path,
    device: DeviceClient,
    facts: Option<DeviceFacts>,
    report: Option<DiagnosticReport>,
    repair_needed: bool,
    repaired: bool,
}

impl ProcessActions<'_> {
    fn collect_facts(&mut self) -> Result<(), String> {
        let detail = self.device.execute(DeviceRequest::CollectDiagnosticFacts)?;
        self.facts = Some(
            serde_json::from_str(&detail)
                .map_err(|error| format!("invalid structured device facts: {error}"))?,
        );
        Ok(())
    }
}

impl DiagnoseActions for ProcessActions<'_> {
    fn run(&mut self, phase: Phase) -> Result<(), String> {
        match phase {
            Phase::Discover => self.device.execute(DeviceRequest::Discover).map(|_| ()),
            Phase::HostFacts => {
                if !self.repository.join(".git").exists() {
                    return Err("current directory is not the repository root".into());
                }
                Ok(())
            }
            Phase::DeviceFacts => self.collect_facts(),
            Phase::Correlate => {
                let facts = self.facts.as_ref().ok_or("device facts are missing")?;
                self.repair_needed = facts.temporary_state;
                self.report = Some(correlate(facts, false));
                Ok(())
            }
            Phase::SafeRepair => {
                if self.repair_needed {
                    self.device.execute(DeviceRequest::RepairSafeDeviceState)?;
                    self.repaired = true;
                }
                Ok(())
            }
            Phase::Recheck => {
                self.collect_facts()?;
                if self.repaired
                    && self
                        .facts
                        .as_ref()
                        .is_some_and(|facts| facts.temporary_state)
                {
                    return Err("temporary state remains after safe repair".into());
                }
                Ok(())
            }
            Phase::Report => {
                self.report = Some(correlate(
                    self.facts.as_ref().ok_or("device facts are missing")?,
                    self.repaired,
                ));
                Ok(())
            }
        }
    }
}

pub fn correlate(facts: &DeviceFacts, repaired: bool) -> DiagnosticReport {
    let next_action = if facts.reboot_unstable {
        Some("Power down, mount the SD card on the Mac, and remove stale arming files.".into())
    } else if facts.arming_files > 0 {
        Some("Remove the reported arming files before any deploy or reboot.".into())
    } else if !facts.credentials_ready {
        Some("Restore the device credential/token, then rerun scripts/agent diagnose.".into())
    } else if !facts.firmware_compatible {
        Some("Install the compatible MiSTer MagiK platform firmware, then rerun diagnosis.".into())
    } else if !facts.main_running || !facts.launcher_running || !facts.agent_running {
        Some("Restore the missing MiSTer MagiK service through the attended recovery path.".into())
    } else {
        None
    };
    DiagnosticReport {
        status: if next_action.is_some() {
            "user_action_required"
        } else {
            "healthy"
        },
        repaired_temporary_state: repaired,
        next_action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeActions {
        fail_at: Option<Phase>,
        phases: Vec<Phase>,
    }

    impl DiagnoseActions for FakeActions {
        fn run(&mut self, phase: Phase) -> Result<(), String> {
            self.phases.push(phase);
            if self.fail_at == Some(phase) {
                Err("injected failure".into())
            } else {
                Ok(())
            }
        }
    }

    fn healthy() -> DeviceFacts {
        DeviceFacts {
            main_running: true,
            launcher_running: true,
            agent_running: true,
            credentials_ready: true,
            firmware_compatible: true,
            ..DeviceFacts::default()
        }
    }

    #[test]
    fn workflow_is_fixed_and_cancellable() {
        let mut actions = FakeActions::default();
        run_workflow(&mut actions, &mut |_, _| Ok(())).unwrap();
        assert_eq!(actions.phases.len(), 7);
        let mut actions = FakeActions::default();
        assert!(run_workflow(&mut actions, &mut |phase, _| {
            if phase == Phase::Correlate {
                Err("stop".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err()
        .starts_with("cancelled:"));
    }

    #[test]
    fn healthy_and_self_solvable_states_need_no_user_action() {
        let report = correlate(&healthy(), false);
        assert_eq!(report.status, "healthy");
        assert!(report.next_action.is_none());
        let report = correlate(
            &DeviceFacts {
                temporary_state: false,
                ..healthy()
            },
            true,
        );
        assert!(report.repaired_temporary_state);
        assert!(report.next_action.is_none());
    }

    #[test]
    fn each_user_actionable_failure_has_exactly_one_next_action() {
        let cases = [
            DeviceFacts {
                reboot_unstable: true,
                ..healthy()
            },
            DeviceFacts {
                arming_files: 1,
                ..healthy()
            },
            DeviceFacts {
                credentials_ready: false,
                ..healthy()
            },
            DeviceFacts {
                firmware_compatible: false,
                ..healthy()
            },
            DeviceFacts {
                launcher_running: false,
                ..healthy()
            },
        ];
        for facts in cases {
            let report = correlate(&facts, false);
            assert_eq!(report.status, "user_action_required");
            assert_eq!(report.next_action.iter().count(), 1);
        }
    }
}
