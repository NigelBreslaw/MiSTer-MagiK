// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum BenchmarkScenario {
    #[default]
    Screensaver,
    Particles,
    ParticleCapacity,
    #[serde(rename = "particle-demo-40k")]
    #[value(name = "particle-demo-40k")]
    ParticleDemo40k,
    ParticleStep,
    ParticleProfile,
    CatalogLifecycle,
    LaunchReturn,
    NavigationTransitions,
    Search,
}

impl BenchmarkScenario {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Screensaver => "screensaver",
            Self::Particles => "particles",
            Self::ParticleCapacity => "particle-capacity",
            Self::ParticleDemo40k => "particle-demo-40k",
            Self::ParticleStep => "particle-step",
            Self::ParticleProfile => "particle-profile",
            Self::CatalogLifecycle => "catalog-lifecycle",
            Self::LaunchReturn => "launch-return",
            Self::NavigationTransitions => "navigation-transitions",
            Self::Search => "search",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    ReadOnly,
    LocalWrite,
    DeviceWrite,
    Destructive,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    Cpu,
    Cargo,
    AppleContainer,
    GitIndex,
    Network,
    Device,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Builtin,
    Cargo { offline_first: bool },
    Script,
    AppleContainer,
    Git,
    PlatformCi,
    DeviceTransaction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinOperation {
    AgentGuidance,
    LicenseHeaders,
    ShellOwnership,
    DistributionWorkflow,
    KernelWorkflow,
    PlatformWorkflow,
    CiCache,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    Cheap,
    Host,
    Expensive,
    External,
    Device,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Staged,
    WorkingTree,
    Paths(Vec<PathBuf>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    PrePush {
        remote: String,
    },
    Plan {
        scope: Scope,
        verbose: bool,
    },
    Check {
        scope: Scope,
    },
    Verify {
        scope: Scope,
    },
    ListRuns {
        failed: bool,
        recent: usize,
    },
    ShowRun {
        run_id: String,
    },
    DatabaseStatus,
    DatabaseReport,
    DatabaseRotate,
    PruneLogs,
    Doctor,
    Diagnose,
    ClearLatchDiagnostics,
    Deliver,
    Benchmark {
        scenario: BenchmarkScenario,
    },
    CaptureUsbVideo {
        output: Option<PathBuf>,
        seconds: Option<u64>,
    },
    AlphaAccept {
        candidate: PathBuf,
        output: PathBuf,
    },
    AlphaVerify {
        candidate: PathBuf,
        receipt: PathBuf,
        marker: PathBuf,
    },
    ReleaseQualify,
    Build {
        intent: crate::build::BuildCommand,
    },
    CiHostAssurance {
        scope: Scope,
    },
    CiPlatformCandidates {
        artifacts: PathBuf,
        name: String,
    },
    CiPlatformEligibleRun {
        run: PathBuf,
        head_sha: String,
    },
    CiRequireAlphaPromotion {
        channel: String,
        alpha_sha: String,
        candidate_sha: String,
    },
    CiPlatformManifestGenerate {
        output: PathBuf,
        main: PathBuf,
        gui: PathBuf,
        manager: PathBuf,
        scanout_module: PathBuf,
        scanout_metadata: PathBuf,
        latch_rbf: PathBuf,
        latch_metadata: PathBuf,
        platform_bundle_manifest: PathBuf,
        main_revision: String,
        magik_revision: String,
        layout: String,
    },
    CiPlatformManifestVerify {
        manifest: PathBuf,
        root: Option<PathBuf>,
        layout: String,
    },
    CiGameDatabases {
        command: crate::cli::GameDatabaseCommand,
    },
    CiPlatformBundle {
        command: crate::cli::PlatformBundleCommand,
    },
}

impl Intent {
    #[must_use]
    pub const fn risk(&self) -> Risk {
        match self {
            Self::Verify { .. }
            | Self::PrePush { .. }
            | Self::CiHostAssurance { .. }
            | Self::CaptureUsbVideo { .. }
            | Self::AlphaVerify { .. } => Risk::LocalWrite,
            Self::ReleaseQualify | Self::DatabaseRotate | Self::ClearLatchDiagnostics => {
                Risk::Destructive
            }
            Self::Deliver { .. }
            | Self::Benchmark { .. }
            | Self::Diagnose
            | Self::AlphaAccept { .. } => Risk::DeviceWrite,
            Self::Build { .. } => Risk::LocalWrite,
            _ => Risk::ReadOnly,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub title: String,
    pub risk: Risk,
    pub action: ActionKind,
    pub phase: WorkflowPhase,
    pub program: String,
    pub args: Vec<String>,
    pub reason: String,
    pub failure_hint: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<BuiltinOperation>,
}

impl Operation {
    #[must_use]
    pub fn action_kind(&self) -> ActionKind {
        self.action
    }

    #[must_use]
    pub fn resource_class(&self) -> ResourceClass {
        match self.action_kind() {
            ActionKind::Builtin => ResourceClass::Cpu,
            ActionKind::Cargo { .. } => ResourceClass::Cargo,
            ActionKind::AppleContainer => ResourceClass::AppleContainer,
            ActionKind::Git => ResourceClass::GitIndex,
            ActionKind::PlatformCi => ResourceClass::Network,
            ActionKind::DeviceTransaction => ResourceClass::Device,
            ActionKind::Script => ResourceClass::Cpu,
        }
    }

    #[must_use]
    pub fn workflow_phase(&self) -> WorkflowPhase {
        self.phase
    }

    #[must_use]
    pub const fn cargo_offline_first(&self) -> bool {
        matches!(
            self.action,
            ActionKind::Cargo {
                offline_first: true
            }
        )
    }
}

impl ResourceClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Cargo => "cargo",
            Self::AppleContainer => "apple_container",
            Self::GitIndex => "git_index",
            Self::Network => "network",
            Self::Device => "device",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub intent: Intent,
    pub operations: Vec<Operation>,
    pub external_requirements: Vec<ExternalRequirement>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExternalRequirement {
    pub id: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Rejected,
    NoOp,
    ExternalRequired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usb_video_capture_is_a_local_write() {
        assert_eq!(
            Intent::CaptureUsbVideo {
                output: None,
                seconds: None,
            }
            .risk(),
            Risk::LocalWrite
        );
    }
}
