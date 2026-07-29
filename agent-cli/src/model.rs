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
    #[serde(rename = "particle-demo-01")]
    #[value(name = "particle-demo-01")]
    ParticleDemo01,
    #[serde(rename = "particle-demo-profile-01")]
    #[value(name = "particle-demo-profile-01")]
    ParticleDemoProfile01,
    #[serde(rename = "particle-demo-02")]
    #[value(name = "particle-demo-02")]
    ParticleDemo02,
    #[serde(rename = "particle-demo-profile-02")]
    #[value(name = "particle-demo-profile-02")]
    ParticleDemoProfile02,
    #[serde(rename = "particle-demo-03")]
    #[value(name = "particle-demo-03")]
    ParticleDemo03,
    #[serde(rename = "particle-demo-profile-03")]
    #[value(name = "particle-demo-profile-03")]
    ParticleDemoProfile03,
    #[serde(rename = "particle-demo-04")]
    #[value(name = "particle-demo-04")]
    ParticleDemo04,
    #[serde(rename = "particle-demo-profile-04")]
    #[value(name = "particle-demo-profile-04")]
    ParticleDemoProfile04,
    #[serde(rename = "particle-demo-05")]
    #[value(name = "particle-demo-05")]
    ParticleDemo05,
    #[serde(rename = "particle-demo-profile-05")]
    #[value(name = "particle-demo-profile-05")]
    ParticleDemoProfile05,
    #[serde(rename = "particle-demo-06")]
    #[value(name = "particle-demo-06")]
    ParticleDemo06,
    #[serde(rename = "particle-demo-profile-06")]
    #[value(name = "particle-demo-profile-06")]
    ParticleDemoProfile06,
    #[serde(rename = "particle-demo-07")]
    #[value(name = "particle-demo-07")]
    ParticleDemo07,
    #[serde(rename = "particle-demo-profile-07")]
    #[value(name = "particle-demo-profile-07")]
    ParticleDemoProfile07,
    #[serde(rename = "particle-demo-08")]
    #[value(name = "particle-demo-08")]
    ParticleDemo08,
    #[serde(rename = "particle-demo-profile-08")]
    #[value(name = "particle-demo-profile-08")]
    ParticleDemoProfile08,
    #[serde(rename = "particle-demo-09")]
    #[value(name = "particle-demo-09")]
    ParticleDemo09,
    #[serde(rename = "particle-demo-profile-09")]
    #[value(name = "particle-demo-profile-09")]
    ParticleDemoProfile09,
    #[serde(rename = "particle-demo-10")]
    #[value(name = "particle-demo-10")]
    ParticleDemo10,
    #[serde(rename = "particle-demo-profile-10")]
    #[value(name = "particle-demo-profile-10")]
    ParticleDemoProfile10,
    #[serde(rename = "particle-demos-carousel")]
    #[value(name = "particle-demos-carousel")]
    ParticleDemosCarousel,
    #[serde(rename = "particle-demos-profile")]
    #[value(name = "particle-demos-profile")]
    ParticleDemosProfile,
    #[serde(rename = "particle-techniques")]
    #[value(name = "particle-techniques")]
    ParticleTechniques,
    #[serde(rename = "particle-techniques-profile")]
    #[value(name = "particle-techniques-profile")]
    ParticleTechniquesProfile,
    #[serde(rename = "particle-technique-images")]
    #[value(name = "particle-technique-images")]
    ParticleTechniqueImages,
    #[serde(rename = "firework-visual")]
    #[value(name = "firework-visual")]
    FireworkVisual,
    CatalogLifecycle,
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
            Self::ParticleDemo01 => "particle-demo-01",
            Self::ParticleDemoProfile01 => "particle-demo-profile-01",
            Self::ParticleDemo02 => "particle-demo-02",
            Self::ParticleDemoProfile02 => "particle-demo-profile-02",
            Self::ParticleDemo03 => "particle-demo-03",
            Self::ParticleDemoProfile03 => "particle-demo-profile-03",
            Self::ParticleDemo04 => "particle-demo-04",
            Self::ParticleDemoProfile04 => "particle-demo-profile-04",
            Self::ParticleDemo05 => "particle-demo-05",
            Self::ParticleDemoProfile05 => "particle-demo-profile-05",
            Self::ParticleDemo06 => "particle-demo-06",
            Self::ParticleDemoProfile06 => "particle-demo-profile-06",
            Self::ParticleDemo07 => "particle-demo-07",
            Self::ParticleDemoProfile07 => "particle-demo-profile-07",
            Self::ParticleDemo08 => "particle-demo-08",
            Self::ParticleDemoProfile08 => "particle-demo-profile-08",
            Self::ParticleDemo09 => "particle-demo-09",
            Self::ParticleDemoProfile09 => "particle-demo-profile-09",
            Self::ParticleDemo10 => "particle-demo-10",
            Self::ParticleDemoProfile10 => "particle-demo-profile-10",
            Self::ParticleDemosCarousel => "particle-demos-carousel",
            Self::ParticleDemosProfile => "particle-demos-profile",
            Self::ParticleTechniques => "particle-techniques",
            Self::ParticleTechniquesProfile => "particle-techniques-profile",
            Self::ParticleTechniqueImages => "particle-technique-images",
            Self::FireworkVisual => "firework-visual",
            Self::CatalogLifecycle => "catalog-lifecycle",
            Self::Search => "search",
        }
    }

    #[must_use]
    pub const fn particle_showcase(self) -> Option<(u8, bool)> {
        match self {
            Self::ParticleDemo01 => Some((1, false)),
            Self::ParticleDemoProfile01 => Some((1, true)),
            Self::ParticleDemo02 => Some((2, false)),
            Self::ParticleDemoProfile02 => Some((2, true)),
            Self::ParticleDemo03 => Some((3, false)),
            Self::ParticleDemoProfile03 => Some((3, true)),
            Self::ParticleDemo04 => Some((4, false)),
            Self::ParticleDemoProfile04 => Some((4, true)),
            Self::ParticleDemo05 => Some((5, false)),
            Self::ParticleDemoProfile05 => Some((5, true)),
            Self::ParticleDemo06 => Some((6, false)),
            Self::ParticleDemoProfile06 => Some((6, true)),
            Self::ParticleDemo07 => Some((7, false)),
            Self::ParticleDemoProfile07 => Some((7, true)),
            Self::ParticleDemo08 => Some((8, false)),
            Self::ParticleDemoProfile08 => Some((8, true)),
            Self::ParticleDemo09 => Some((9, false)),
            Self::ParticleDemoProfile09 => Some((9, true)),
            Self::ParticleDemo10 => Some((10, false)),
            Self::ParticleDemoProfile10 => Some((10, true)),
            _ => None,
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
    Deliver,
    Benchmark {
        scenario: BenchmarkScenario,
    },
    FireworkVisual {
        firework: Option<String>,
        all: bool,
    },
    LaunchParticleShowcase,
    CaptureUsbVideo {
        output: Option<PathBuf>,
        seconds: Option<u64>,
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
            | Self::CaptureUsbVideo { .. } => Risk::LocalWrite,
            Self::ReleaseQualify | Self::DatabaseRotate => Risk::Destructive,
            Self::Deliver { .. }
            | Self::Benchmark { .. }
            | Self::FireworkVisual { .. }
            | Self::Diagnose
            | Self::LaunchParticleShowcase => Risk::DeviceWrite,
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
