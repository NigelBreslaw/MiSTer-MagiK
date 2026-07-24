// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::error::AgentResult;
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
    #[serde(default)]
    pub launcher_heartbeat_advancing: bool,
    #[serde(default)]
    pub launcher_state: String,
    #[serde(default)]
    pub crash_count: u64,
    #[serde(default)]
    pub last_crash_reason: String,
    #[serde(default)]
    pub last_crash_report: String,
    #[serde(default)]
    pub last_crash_report_id: String,
    #[serde(default)]
    pub last_crash_kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticReport {
    pub status: &'static str,
    pub repaired_temporary_state: bool,
    pub next_action: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub launcher_state: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub crash_count: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_crash_reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_crash_report: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_crash_report_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_crash_kind: String,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

pub trait DiagnoseActions {
    fn run(&mut self, phase: Phase) -> AgentResult<()>;
}

pub fn run_workflow(
    actions: &mut dyn DiagnoseActions,
    progress: &mut dyn FnMut(Phase, u8) -> AgentResult<()>,
) -> AgentResult<()> {
    const PHASES: &[(Phase, u8)] = &[
        (Phase::Discover, 5),
        (Phase::HostFacts, 18),
        (Phase::DeviceFacts, 35),
        (Phase::Correlate, 52),
        (Phase::SafeRepair, 68),
        (Phase::Recheck, 84),
        (Phase::Report, 100),
    ];
    crate::workflow::run_phases(
        actions,
        PHASES,
        progress,
        |actions, phase| actions.run(phase),
        Phase::label,
    )
}

pub fn execute(repository: &Path, reporter: &mut Reporter<'_>) -> AgentResult<Outcome> {
    let mut actions = ProcessActions {
        repository,
        device: DeviceClient::default(),
        facts: None,
        report: None,
        repair_needed: false,
        repaired: false,
        geometry_trial: geometry_trial_from_env()?,
        geometry_trial_detail: None,
        screensaver_trial: screensaver_trial_from_env(),
        screensaver_trial_detail: None,
        screensaver_matrix: screensaver_matrix_from_env(),
        screensaver_matrix_detail: None,
        crash_report: crash_report_from_env(),
        crash_report_detail: None,
    };
    run_workflow(&mut actions, &mut |phase, percent| {
        Ok(reporter.emit(
            EventKind::Progress,
            phase.label(),
            &format!("diagnose {}", phase.label()),
            Some(percent),
        )?)
    })?;
    let report = actions.report.ok_or("diagnosis produced no report")?;
    if let Some(detail) = actions.geometry_trial_detail.as_deref() {
        reporter.emit(EventKind::Progress, "geometry-trial", detail, Some(95))?;
    }
    if let Some(detail) = actions.screensaver_trial_detail.as_deref() {
        reporter.emit(EventKind::Progress, "screensaver-trial", detail, Some(95))?;
    }
    if let Some(detail) = actions.screensaver_matrix_detail.as_deref() {
        reporter.emit(EventKind::Progress, "screensaver-matrix", detail, Some(95))?;
    }
    if let Some(detail) = actions.crash_report_detail.as_deref() {
        reporter.emit(EventKind::Progress, "crash-report", detail, Some(95))?;
    }
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
    geometry_trial: Option<[u16; 4]>,
    geometry_trial_detail: Option<String>,
    screensaver_trial: bool,
    screensaver_trial_detail: Option<String>,
    screensaver_matrix: bool,
    screensaver_matrix_detail: Option<String>,
    crash_report: bool,
    crash_report_detail: Option<String>,
}

fn geometry_trial_from_env() -> AgentResult<Option<[u16; 4]>> {
    // Geometry trials are deliberately available only through `diagnose`.
    let Some(value) = std::env::var("MISTER_CRT_GEOMETRY_TRIAL_RECT").ok() else {
        return Ok(None);
    };
    let values = value
        .split(',')
        .map(str::parse::<u16>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| "MISTER_CRT_GEOMETRY_TRIAL_RECT must contain four unsigned integers")?;
    values
        .try_into()
        .map(Some)
        .map_err(|_| "MISTER_CRT_GEOMETRY_TRIAL_RECT must contain four coordinates".into())
}

fn screensaver_trial_from_env() -> bool {
    matches!(
        std::env::var("MISTER_CRT_SCREENSAVER_TRIAL").as_deref(),
        Ok("1" | "true")
    )
}

fn screensaver_matrix_from_env() -> bool {
    matches!(
        std::env::var("MISTER_CRT_SCREENSAVER_MATRIX").as_deref(),
        Ok("1" | "true")
    )
}

fn crash_report_from_env() -> bool {
    matches!(
        std::env::var("MISTER_CRASH_REPORT").as_deref(),
        Ok("1" | "true")
    )
}

impl ProcessActions<'_> {
    fn collect_facts(&mut self) -> AgentResult<()> {
        let detail = self.device.execute(DeviceRequest::CollectDiagnosticFacts)?;
        self.facts = Some(
            serde_json::from_str(&detail)
                .map_err(|error| format!("invalid structured device facts: {error}"))?,
        );
        Ok(())
    }
}

impl DiagnoseActions for ProcessActions<'_> {
    fn run(&mut self, phase: Phase) -> AgentResult<()> {
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
                if let Some(rectangle) = self.geometry_trial.take() {
                    self.geometry_trial_detail = Some(
                        self.device
                            .execute(DeviceRequest::RunCrtGeometryTrial { rectangle })?,
                    );
                }
                if self.screensaver_trial {
                    self.screensaver_trial = false;
                    self.screensaver_trial_detail =
                        Some(self.device.execute(DeviceRequest::RunCrtScreensaverTrial)?);
                }
                if self.screensaver_matrix {
                    self.screensaver_matrix = false;
                    self.screensaver_matrix_detail = Some(
                        self.device
                            .execute(DeviceRequest::RunCrtScreensaverMatrix)?,
                    );
                }
                if self.crash_report {
                    self.crash_report = false;
                    self.crash_report_detail = Some(
                        self.device
                            .execute(DeviceRequest::CollectLatestCrashReport)?,
                    );
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
    } else if !facts.launcher_heartbeat_advancing {
        Some("The MagiK launcher event loop is stalled; reboot through the attended recovery path, then rerun diagnosis.".into())
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
        launcher_state: facts.launcher_state.clone(),
        crash_count: facts.crash_count,
        last_crash_reason: facts.last_crash_reason.clone(),
        last_crash_report: facts.last_crash_report.clone(),
        last_crash_report_id: facts.last_crash_report_id.clone(),
        last_crash_kind: facts.last_crash_kind.clone(),
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
        fn run(&mut self, phase: Phase) -> AgentResult<()> {
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
            launcher_heartbeat_advancing: true,
            ..DeviceFacts::default()
        }
    }

    #[test]
    fn workflow_is_fixed_and_cancellable() {
        let mut actions = FakeActions::default();
        run_workflow(&mut actions, &mut |_, _| Ok(())).unwrap();
        assert_eq!(actions.phases.len(), 7);
        let mut actions = FakeActions::default();
        assert!(
            run_workflow(&mut actions, &mut |phase, _| {
                if phase == Phase::Correlate {
                    Err("stop".into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err()
            .to_string()
            .starts_with("cancelled:")
        );
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
    fn stalled_launcher_heartbeat_is_not_reported_healthy() {
        let report = correlate(
            &DeviceFacts {
                launcher_heartbeat_advancing: false,
                ..healthy()
            },
            false,
        );
        assert_eq!(report.status, "user_action_required");
        assert!(
            report
                .next_action
                .unwrap()
                .contains("event loop is stalled")
        );
    }

    #[test]
    fn crash_metadata_is_preserved_in_the_diagnostic_report() {
        let report = correlate(
            &DeviceFacts {
                launcher_running: false,
                launcher_state: "LauncherCrashed".into(),
                crash_count: 2,
                last_crash_reason: "exit=1".into(),
                last_crash_report: "/media/fat/mister-magik-dev/crashes/report-2.json".into(),
                last_crash_report_id: "report-2".into(),
                last_crash_kind: "unexpected-child-exit".into(),
                ..healthy()
            },
            false,
        );

        assert_eq!(report.launcher_state, "LauncherCrashed");
        assert_eq!(report.crash_count, 2);
        assert_eq!(report.last_crash_report_id, "report-2");
        assert_eq!(report.last_crash_kind, "unexpected-child-exit");
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
