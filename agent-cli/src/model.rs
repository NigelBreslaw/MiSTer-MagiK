// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum BenchmarkScenario {
    #[default]
    Screensaver,
    ColdBoot,
    ColdBootPprof,
    Particles,
    ParticleCapacity,
    #[serde(rename = "particle-demo-40k")]
    #[value(name = "particle-demo-40k")]
    ParticleDemo40k,
    ParticleStep,
    ParticleProfile,
    CatalogLifecycle,
    SystemEntry,
    SystemEntryCritical,
    SystemEntryCriticalConfirm,
    SystemEntryCriticalProfile,
    SystemEntryCriticalStreamline,
    SystemEntryQualification,
    LaunchReturn,
    LaunchReturnOnce,
    LaunchReturnFallback,
    LaunchReturnAttribution,
    ModalInput,
    InputIntegrity,
    LauncherResponse,
    LauncherResponseAttribution,
    GuiFrameAttribution,
    ArcadeVelocityScroll,
    ArcadeVelocityScrollAttribution,
    TransitionStreamline,
    AgentObserverAttribution,
    AgentIoAttribution,
    InputLatencyLab,
    LauncherResponseStreamline,
    NavigationTransitions,
    SettingsNavigation,
    SettingsNavigationPprof,
    OrientationTransitionFade,
    OrientationTransitionZoom,
    OrientationTransitionFadePprof,
    OrientationTransitionZoomPprof,
    PmuProfile,
    Search,
    Streamline,
}

impl BenchmarkScenario {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Screensaver => "screensaver",
            Self::ColdBoot => "cold-boot",
            Self::ColdBootPprof => "cold-boot-pprof",
            Self::Particles => "particles",
            Self::ParticleCapacity => "particle-capacity",
            Self::ParticleDemo40k => "particle-demo-40k",
            Self::ParticleStep => "particle-step",
            Self::ParticleProfile => "particle-profile",
            Self::CatalogLifecycle => "catalog-lifecycle",
            Self::SystemEntry => "system-entry",
            Self::SystemEntryCritical => "system-entry-critical",
            Self::SystemEntryCriticalConfirm => "system-entry-critical-confirm",
            Self::SystemEntryCriticalProfile => "system-entry-critical-profile",
            Self::SystemEntryCriticalStreamline => "system-entry-critical-streamline",
            Self::SystemEntryQualification => "system-entry-qualification",
            Self::LaunchReturn => "launch-return",
            Self::LaunchReturnOnce => "launch-return-once",
            Self::LaunchReturnFallback => "launch-return-fallback",
            Self::LaunchReturnAttribution => "launch-return-attribution",
            Self::ModalInput => "modal-input",
            Self::InputIntegrity => "input-integrity",
            Self::LauncherResponse => "launcher-response",
            Self::LauncherResponseAttribution => "launcher-response-attribution",
            Self::GuiFrameAttribution => "gui-frame-attribution",
            Self::ArcadeVelocityScroll => "arcade-velocity-scroll",
            Self::ArcadeVelocityScrollAttribution => "arcade-velocity-scroll-attribution",
            Self::TransitionStreamline => "transition-streamline",
            Self::AgentObserverAttribution => "agent-observer-attribution",
            Self::AgentIoAttribution => "agent-io-attribution",
            Self::InputLatencyLab => "input-latency-lab",
            Self::LauncherResponseStreamline => "launcher-response-streamline",
            Self::NavigationTransitions => "navigation-transitions",
            Self::SettingsNavigation => "settings-navigation",
            Self::SettingsNavigationPprof => "settings-navigation-pprof",
            Self::OrientationTransitionFade => "orientation-transition-fade",
            Self::OrientationTransitionZoom => "orientation-transition-zoom",
            Self::OrientationTransitionFadePprof => "orientation-transition-fade-pprof",
            Self::OrientationTransitionZoomPprof => "orientation-transition-zoom-pprof",
            Self::PmuProfile => "pmu-profile",
            Self::Search => "search",
            Self::Streamline => "streamline",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum ArcadeVelocityScrollArm {
    Control,
    Turbo,
    Pprof,
    Pmu,
    Streamline,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
pub enum ArcadeVelocityScrollRoute {
    #[default]
    Active,
    HdmiLandscape,
    HdmiPortraitLeft,
    Crt240PortraitLeft,
    Crt288PortraitLeft,
}

impl ArcadeVelocityScrollRoute {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::HdmiLandscape => "hdmi-landscape",
            Self::HdmiPortraitLeft => "hdmi-portrait-left",
            Self::Crt240PortraitLeft => "crt240-portrait-left",
            Self::Crt288PortraitLeft => "crt288-portrait-left",
        }
    }

    #[must_use]
    pub const fn display_mode(self) -> Option<&'static str> {
        match self {
            Self::Active => None,
            Self::HdmiLandscape | Self::HdmiPortraitLeft => Some("hdmi-1280x720p60"),
            Self::Crt240PortraitLeft => Some("crt-240p60"),
            Self::Crt288PortraitLeft => Some("crt-288p50"),
        }
    }

    #[must_use]
    pub const fn orientation(self) -> Option<&'static str> {
        match self {
            Self::Active => None,
            Self::HdmiLandscape => Some("normal"),
            Self::HdmiPortraitLeft | Self::Crt240PortraitLeft | Self::Crt288PortraitLeft => {
                Some("monitor-counterclockwise")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArcadeVelocityScrollProfiler {
    None,
    Pprof,
    Pmu,
    Streamline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArcadeVelocityScrollInputMode {
    Held,
    Turbo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArcadeVelocityScrollRunSpec {
    pub arm: ArcadeVelocityScrollArm,
    pub route: ArcadeVelocityScrollRoute,
    pub duration_ms: u64,
    pub telemetry_secs: u64,
    pub profiler: ArcadeVelocityScrollProfiler,
    pub input_mode: ArcadeVelocityScrollInputMode,
}

impl ArcadeVelocityScrollRunSpec {
    #[must_use]
    pub const fn new(arm: ArcadeVelocityScrollArm, route: ArcadeVelocityScrollRoute) -> Self {
        Self {
            arm,
            route,
            duration_ms: 40_000,
            telemetry_secs: 55,
            profiler: match arm {
                ArcadeVelocityScrollArm::Pprof => ArcadeVelocityScrollProfiler::Pprof,
                ArcadeVelocityScrollArm::Pmu => ArcadeVelocityScrollProfiler::Pmu,
                ArcadeVelocityScrollArm::Streamline => ArcadeVelocityScrollProfiler::Streamline,
                _ => ArcadeVelocityScrollProfiler::None,
            },
            input_mode: if matches!(arm, ArcadeVelocityScrollArm::Turbo) {
                ArcadeVelocityScrollInputMode::Turbo
            } else {
                ArcadeVelocityScrollInputMode::Held
            },
        }
    }
}

impl ArcadeVelocityScrollArm {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Turbo => "turbo",
            Self::Pprof => "pprof",
            Self::Pmu => "pmu",
            Self::Streamline => "streamline",
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
    RuntimeEnvironment,
    PlatformManifestAuthority,
    DeviceCrateRootOwnership,
    ExecutableBoundaries,
    DistributionWorkflow,
    KernelWorkflow,
    PlatformWorkflow,
    ArchitectureWorkflow,
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
    WorkingTree,
    Paths(Vec<PathBuf>),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssuranceRequest {
    PrePush { remote: String },
    Plan { scope: Scope },
    CiHostAssurance { scope: Scope },
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
    pub request: AssuranceRequest,
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
