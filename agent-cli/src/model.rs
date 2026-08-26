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
    CatalogBuildRebuild,
    CatalogResumeValidation,
    CatalogFullBuildRebuild,
    CatalogCorpusInventory,
    ArcadeCatalogPrototypeCold,
    CatalogAttributionControl,
    CatalogAttributionPprof,
    CatalogAttributionPmu,
    CatalogAttributionStorage,
    CatalogAttributionFunctionGraph,
    CatalogAttributionStreamline,
    CatalogAttributionReport,
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
    LauncherResponseRetained,
    LauncherResponseAttribution,
    GuiFrameAttribution,
    SettledComposition,
    BridgeModelChurn,
    BridgeModelChurnRetained,
    SchedulerTrace,
    StorageAttribution,
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
    NeonAttribution,
    PmuProfile,
    MediaPackPersistence,
    RomIdentityHashing,
    PreviewWorkAttribution,
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
            Self::CatalogBuildRebuild => "catalog-build-rebuild",
            Self::CatalogResumeValidation => "catalog-resume-validation",
            Self::CatalogFullBuildRebuild => "catalog-full-build-rebuild",
            Self::CatalogCorpusInventory => "catalog-corpus-inventory",
            Self::ArcadeCatalogPrototypeCold => "arcade-catalog-prototype-cold",
            Self::CatalogAttributionControl => "catalog-attribution-control",
            Self::CatalogAttributionPprof => "catalog-attribution-pprof",
            Self::CatalogAttributionPmu => "catalog-attribution-pmu",
            Self::CatalogAttributionStorage => "catalog-attribution-storage",
            Self::CatalogAttributionFunctionGraph => "catalog-attribution-function-graph",
            Self::CatalogAttributionStreamline => "catalog-attribution-streamline",
            Self::CatalogAttributionReport => "catalog-attribution-report",
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
            Self::LauncherResponseRetained => "launcher-response-retained",
            Self::LauncherResponseAttribution => "launcher-response-attribution",
            Self::GuiFrameAttribution => "gui-frame-attribution",
            Self::SettledComposition => "settled-composition",
            Self::BridgeModelChurn => "bridge-model-churn",
            Self::BridgeModelChurnRetained => "bridge-model-churn-retained",
            Self::SchedulerTrace => "scheduler-trace",
            Self::StorageAttribution => "storage-attribution",
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
            Self::NeonAttribution => "neon-attribution",
            Self::PmuProfile => "pmu-profile",
            Self::MediaPackPersistence => "media-pack-persistence",
            Self::RomIdentityHashing => "rom-identity-hashing",
            Self::PreviewWorkAttribution => "preview-work-attribution",
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
    HdmiPortraitRight,
    Hdmi1080Landscape,
    Hdmi1080PortraitLeft,
    Crt240PortraitLeft,
    Crt240PortraitRight,
    Crt288PortraitLeft,
    Crt288PortraitRight,
}

impl ArcadeVelocityScrollRoute {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::HdmiLandscape => "hdmi-landscape",
            Self::HdmiPortraitLeft => "hdmi-portrait-left",
            Self::HdmiPortraitRight => "hdmi-portrait-right",
            Self::Hdmi1080Landscape => "hdmi1080-landscape",
            Self::Hdmi1080PortraitLeft => "hdmi1080-portrait-left",
            Self::Crt240PortraitLeft => "crt240-portrait-left",
            Self::Crt240PortraitRight => "crt240-portrait-right",
            Self::Crt288PortraitLeft => "crt288-portrait-left",
            Self::Crt288PortraitRight => "crt288-portrait-right",
        }
    }

    #[must_use]
    pub const fn display_mode(self) -> Option<&'static str> {
        match self {
            Self::Active => None,
            Self::HdmiLandscape | Self::HdmiPortraitLeft | Self::HdmiPortraitRight => {
                Some("hdmi-1280x720p60")
            }
            Self::Hdmi1080Landscape | Self::Hdmi1080PortraitLeft => Some("hdmi-1920x1080p60"),
            Self::Crt240PortraitLeft | Self::Crt240PortraitRight => Some("crt-240p60"),
            Self::Crt288PortraitLeft | Self::Crt288PortraitRight => Some("crt-288p50"),
        }
    }

    #[must_use]
    pub const fn orientation(self) -> Option<&'static str> {
        match self {
            Self::Active => None,
            Self::HdmiLandscape | Self::Hdmi1080Landscape => Some("normal"),
            Self::HdmiPortraitLeft | Self::Crt240PortraitLeft | Self::Crt288PortraitLeft => {
                Some("monitor-counterclockwise")
            }
            Self::Hdmi1080PortraitLeft => Some("monitor-counterclockwise"),
            Self::HdmiPortraitRight | Self::Crt240PortraitRight | Self::Crt288PortraitRight => {
                Some("monitor-clockwise")
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

    #[must_use]
    pub const fn with_duration_seconds(mut self, duration_seconds: u64) -> Self {
        self.duration_ms = duration_seconds.saturating_mul(1_000);
        self.telemetry_secs = duration_seconds.saturating_add(15);
        self
    }
}

#[cfg(test)]
mod arcade_velocity_scroll_run_spec_tests {
    use super::*;

    #[test]
    fn duration_override_updates_workload_and_telemetry_window() {
        let spec = ArcadeVelocityScrollRunSpec::new(
            ArcadeVelocityScrollArm::Turbo,
            ArcadeVelocityScrollRoute::HdmiPortraitRight,
        )
        .with_duration_seconds(20);

        assert_eq!(spec.duration_ms, 20_000);
        assert_eq!(spec.telemetry_secs, 35);
        assert_eq!(spec.route.display_mode(), Some("hdmi-1280x720p60"));
        assert_eq!(spec.route.orientation(), Some("monitor-clockwise"));
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
