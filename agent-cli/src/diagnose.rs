// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::device::DeviceClient;
use crate::error::AgentResult;
use crate::model::Outcome;
use crate::progress::{EventKind, Reporter};
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
    #[serde(default)]
    pub scanout_ready: bool,
    #[serde(default)]
    pub latch_ready: bool,
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
    #[serde(default)]
    pub scene: String,
    #[serde(default)]
    pub screen: String,
    #[serde(default)]
    pub effective_view: String,
    #[serde(default)]
    pub return_screen: String,
    #[serde(default)]
    pub input_enabled: bool,
    #[serde(default)]
    pub present_backend: String,
    #[serde(default)]
    pub present_status: String,
    #[serde(default)]
    pub latch_failure_state: String,
    #[serde(default)]
    pub latch_failure_stage: String,
    #[serde(default)]
    pub latch_failure_reason: String,
    #[serde(default)]
    pub latch_failure_detail: String,
    #[serde(default)]
    pub latch_latest_state: String,
    #[serde(default)]
    pub latch_latest_stage: String,
    #[serde(default)]
    pub latch_latest_reason: String,
    #[serde(default)]
    pub latch_latest_detail: String,
    #[serde(default)]
    pub latch_recovery_attempt_count: u8,
    #[serde(default)]
    pub latch_latest_retry_result: String,
    #[serde(default)]
    pub latch_recovery_state: String,
    #[serde(default)]
    pub compatibility_prompt_visible: bool,
    #[serde(default)]
    pub capture_source: String,
    #[serde(default)]
    pub capture_authoritative_scanout: bool,
    #[serde(default)]
    pub capture_error: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticReport {
    pub status: &'static str,
    pub repaired_temporary_state: bool,
    pub recovered_by_reboot: bool,
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
    #[serde(skip_serializing_if = "String::is_empty")]
    pub scene: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub screen: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effective_view: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub return_screen: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub present_backend: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub present_status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_failure_state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_failure_stage: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_failure_reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_failure_detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_latest_state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_latest_stage: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_latest_reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_latest_detail: String,
    #[serde(skip_serializing_if = "is_zero_u8")]
    pub latch_recovery_attempt_count: u8,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_latest_retry_result: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latch_recovery_state: String,
    pub compatibility_prompt_visible: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub capture_source: String,
    pub capture_authoritative_scanout: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub capture_error: String,
}

trait DiagnosticDevice {
    fn connect(&mut self) -> AgentResult<()>;
    fn collect_facts(&mut self) -> AgentResult<DeviceFacts>;
    fn repair_safe_state(&mut self) -> AgentResult<()>;
    fn recover_with_one_shot_reboot(&mut self) -> AgentResult<()>;
}

impl DiagnosticDevice for DeviceClient {
    fn connect(&mut self) -> AgentResult<()> {
        self.read(crate::NativeDevice::discover)
    }

    fn collect_facts(&mut self) -> AgentResult<DeviceFacts> {
        let facts = self.read(crate::NativeDevice::collect_diagnostic_facts)?;
        serde_json::from_value(facts)
            .map_err(|error| format!("invalid structured device facts: {error}").into())
    }

    fn repair_safe_state(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::repair_safe_device_state)
    }

    fn recover_with_one_shot_reboot(&mut self) -> AgentResult<()> {
        self.mutate(crate::NativeDevice::recover_with_one_shot_reboot)
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn is_zero_u8(value: &u8) -> bool {
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
        reboot_needed: false,
        repair_temporary_state: false,
        repaired: false,
        rebooted: false,
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

struct ProcessActions<'a, D = DeviceClient> {
    repository: &'a Path,
    device: D,
    facts: Option<DeviceFacts>,
    report: Option<DiagnosticReport>,
    repair_needed: bool,
    reboot_needed: bool,
    repair_temporary_state: bool,
    repaired: bool,
    rebooted: bool,
}

impl<D: DiagnosticDevice> ProcessActions<'_, D> {
    fn collect_facts(&mut self) -> AgentResult<()> {
        self.facts = Some(self.device.collect_facts()?);
        Ok(())
    }
}

impl<D: DiagnosticDevice> DiagnoseActions for ProcessActions<'_, D> {
    fn run(&mut self, phase: Phase) -> AgentResult<()> {
        match phase {
            Phase::Discover => self.device.connect(),
            Phase::HostFacts => {
                if !self.repository.join(".git").exists() {
                    return Err("current directory is not the repository root".into());
                }
                Ok(())
            }
            Phase::DeviceFacts => self.collect_facts(),
            Phase::Correlate => {
                let facts = self.facts.as_ref().ok_or("device facts are missing")?;
                self.repair_temporary_state = facts.temporary_state;
                self.repair_needed = diagnostic_safe_repair_needed(facts);
                self.reboot_needed = diagnostic_one_shot_reboot_needed(facts);
                self.report = Some(correlate(facts, false));
                Ok(())
            }
            Phase::SafeRepair => {
                if self.repair_needed {
                    self.device.repair_safe_state()?;
                    self.repaired = self.repair_temporary_state;
                }
                if self.reboot_needed {
                    self.device.recover_with_one_shot_reboot()?;
                    self.rebooted = true;
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
                let mut report = correlate(
                    self.facts.as_ref().ok_or("device facts are missing")?,
                    self.repaired,
                );
                report.recovered_by_reboot = self.rebooted;
                self.report = Some(report);
                Ok(())
            }
        }
    }
}

fn diagnostic_safe_repair_needed(facts: &DeviceFacts) -> bool {
    facts.temporary_state
        || (facts.main_running
            && !facts.launcher_running
            && facts.agent_running
            && facts.credentials_ready
            && facts.firmware_compatible
            && facts.scanout_ready
            && facts.latch_ready
            && !facts.reboot_unstable
            && facts.arming_files == 0
            && facts.launcher_state == "Unconfigured")
}

fn diagnostic_one_shot_reboot_needed(facts: &DeviceFacts) -> bool {
    !facts.reboot_unstable
        && facts.credentials_ready
        && facts.firmware_compatible
        && facts.scanout_ready
        && facts.latch_ready
        && (!facts.main_running
            || !facts.launcher_running
            || !facts.agent_running
            || !facts.launcher_heartbeat_advancing)
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
    } else if !facts.scanout_ready || !facts.latch_ready {
        Some("Run scripts/agent deliver platform to restore the coherent development platform, then rerun scripts/agent diagnose.".into())
    } else if facts.present_backend == "compatibility-fb0"
        || facts.present_status == "compatibility"
    {
        Some(if facts.latch_failure_state == "platform-incompatible" {
            "Run scripts/agent deliver platform to restore the coherent development platform, then rerun scripts/agent diagnose.".into()
        } else {
            "Use A: Retry latch or B: Continue in compatibility mode on the device, then rerun scripts/agent diagnose.".into()
        })
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
        recovered_by_reboot: false,
        next_action,
        launcher_state: facts.launcher_state.clone(),
        crash_count: facts.crash_count,
        last_crash_reason: facts.last_crash_reason.clone(),
        last_crash_report: facts.last_crash_report.clone(),
        last_crash_report_id: facts.last_crash_report_id.clone(),
        last_crash_kind: facts.last_crash_kind.clone(),
        scene: facts.scene.clone(),
        screen: facts.screen.clone(),
        effective_view: facts.effective_view.clone(),
        return_screen: facts.return_screen.clone(),
        present_backend: facts.present_backend.clone(),
        present_status: facts.present_status.clone(),
        latch_failure_state: facts.latch_failure_state.clone(),
        latch_failure_stage: facts.latch_failure_stage.clone(),
        latch_failure_reason: facts.latch_failure_reason.clone(),
        latch_failure_detail: facts.latch_failure_detail.clone(),
        latch_latest_state: facts.latch_latest_state.clone(),
        latch_latest_stage: facts.latch_latest_stage.clone(),
        latch_latest_reason: facts.latch_latest_reason.clone(),
        latch_latest_detail: facts.latch_latest_detail.clone(),
        latch_recovery_attempt_count: facts.latch_recovery_attempt_count,
        latch_latest_retry_result: facts.latch_latest_retry_result.clone(),
        latch_recovery_state: facts.latch_recovery_state.clone(),
        compatibility_prompt_visible: facts.compatibility_prompt_visible,
        capture_source: facts.capture_source.clone(),
        capture_authoritative_scanout: facts.capture_authoritative_scanout,
        capture_error: facts.capture_error.clone(),
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
            scanout_ready: true,
            latch_ready: true,
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
        assert!(!report.recovered_by_reboot);
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
        let facts = DeviceFacts {
            launcher_heartbeat_advancing: false,
            ..healthy()
        };
        assert!(diagnostic_one_shot_reboot_needed(&facts));
        let report = correlate(&facts, false);
        assert_eq!(report.status, "user_action_required");
        assert!(
            report
                .next_action
                .unwrap()
                .contains("event loop is stalled")
        );
    }

    #[test]
    fn unconfigured_healthy_dev_platform_is_safe_to_recover() {
        let facts = DeviceFacts {
            launcher_running: false,
            launcher_state: "Unconfigured".into(),
            launcher_heartbeat_advancing: false,
            ..healthy()
        };
        assert!(diagnostic_safe_repair_needed(&facts));
        assert!(diagnostic_one_shot_reboot_needed(&facts));

        assert!(!diagnostic_safe_repair_needed(&DeviceFacts {
            arming_files: 1,
            ..facts.clone()
        }));
        assert!(diagnostic_one_shot_reboot_needed(&DeviceFacts {
            arming_files: 1,
            ..facts.clone()
        }));
        assert!(!diagnostic_safe_repair_needed(&DeviceFacts {
            latch_ready: false,
            ..facts.clone()
        }));
        assert!(!diagnostic_one_shot_reboot_needed(&DeviceFacts {
            latch_ready: false,
            ..facts.clone()
        }));
        assert!(!diagnostic_one_shot_reboot_needed(&DeviceFacts {
            reboot_unstable: true,
            ..facts
        }));
    }

    #[test]
    fn failed_latch_readiness_is_not_reported_healthy() {
        let report = correlate(
            &DeviceFacts {
                latch_ready: false,
                present_backend: "compatibility-fb0".into(),
                present_status: "compatibility".into(),
                ..healthy()
            },
            false,
        );
        assert_eq!(report.status, "user_action_required");
        assert_eq!(report.present_backend, "compatibility-fb0");
        assert!(
            report
                .next_action
                .unwrap()
                .contains("scripts/agent deliver platform")
        );
    }

    #[test]
    fn compatibility_presentation_fails_closed_with_preserved_evidence() {
        let report = correlate(
            &DeviceFacts {
                present_backend: "compatibility-fb0".into(),
                present_status: "compatibility".into(),
                latch_failure_state: "runtime-fault".into(),
                latch_failure_stage: "post-verification".into(),
                latch_failure_reason: "posted-sequence-unverified".into(),
                latch_failure_detail: "posted sequence 219 was not observed active".into(),
                compatibility_prompt_visible: true,
                capture_source: "producer-composition".into(),
                capture_authoritative_scanout: false,
                ..healthy()
            },
            false,
        );

        assert_eq!(report.status, "user_action_required");
        assert_eq!(report.latch_failure_reason, "posted-sequence-unverified");
        assert_eq!(report.capture_source, "producer-composition");
        assert!(!report.capture_authoritative_scanout);
        assert!(report.next_action.unwrap().contains("A: Retry latch"));
    }

    #[test]
    fn compatibility_platform_failure_recommends_delivery() {
        let report = correlate(
            &DeviceFacts {
                present_status: "compatibility".into(),
                latch_failure_state: "platform-incompatible".into(),
                ..healthy()
            },
            false,
        );

        assert_eq!(report.status, "user_action_required");
        assert!(
            report
                .next_action
                .unwrap()
                .contains("scripts/agent deliver platform")
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
