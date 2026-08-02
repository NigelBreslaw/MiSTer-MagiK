// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    Development,
    Public,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomationButton {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    Home,
    X,
    Y,
}

impl AutomationButton {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::A => "a",
            Self::B => "b",
            Self::Home => "home",
            Self::X => "x",
            Self::Y => "y",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutomationAction {
    Tap(AutomationButton),
    Hold {
        button: AutomationButton,
        duration_ms: u64,
    },
    ReleaseAll,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlphaCandidateHashes {
    pub platform_manifest: String,
    pub main: String,
    pub gui: String,
    pub manager: String,
    pub scanout_module: String,
    pub scanout_metadata: String,
    pub latch_rbf: String,
    pub latch_metadata: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceRequest {
    Discover,
    Status,
    ReadDevelopmentManifest,
    VerifyDevelopmentPlatform,
    FetchVerifiedDevelopmentManager {
        local: PathBuf,
        expected_sha256: String,
    },
    DeliverRuntimeTransaction {
        local: PathBuf,
        remote: String,
        manifest_local: PathBuf,
        manifest_remote: String,
        expected_sha256: String,
    },
    DeliverPlatformTransaction {
        stage: PathBuf,
        expected_sha256: String,
    },
    DeliverLocalMainTransaction {
        local: PathBuf,
        manifest_local: PathBuf,
        expected_main_sha256: String,
        expected_gui_sha256: String,
    },
    ProfileInstalledScreensaver {
        output_dir: PathBuf,
    },
    ProfileInstalledParticles {
        output_dir: PathBuf,
    },
    ProfileInstalledParticleCapacity {
        output_dir: PathBuf,
    },
    ProfileInstalledParticleDemo40k {
        output_dir: PathBuf,
    },
    ProfileInstalledParticleStep {
        output_dir: PathBuf,
    },
    ProfileInstalledParticleCpu {
        output_dir: PathBuf,
    },
    ProfileInstalledParticleShowcase {
        output_dir: PathBuf,
        demo: u8,
        cpu_profile: bool,
    },
    CaptureInstalledFireworkVisual {
        output_dir: PathBuf,
        demo: u8,
        label: String,
        time_ms: u64,
    },
    CaptureInstalledParticleTechnique {
        output_dir: PathBuf,
        demo: u8,
        label: String,
        hero_secs: u64,
    },
    WatchLiveParticles {
        family: PathBuf,
        demo: u8,
    },
    ProfileInstalledSearch {
        output_dir: PathBuf,
    },
    VerifyInstalledSearchUi {
        output_dir: PathBuf,
    },
    ProfileInstalledCatalogLifecycle {
        output_dir: PathBuf,
    },
    ProfileInstalledLaunchReturn {
        output_dir: PathBuf,
    },
    ProfileInstalledLaunchReturnFallback {
        output_dir: PathBuf,
    },
    ProfileInstalledColdBoot {
        output_dir: PathBuf,
    },
    ProfileInstalledNavigationTransitions {
        output_dir: PathBuf,
    },
    VerifyHealth(Layout),
    BeginReleaseQualification,
    QualifyReleaseRuntime,
    QualifyReleaseCatalog,
    QualifyReleaseInputAndHandoff,
    QualifyReleaseDisplay,
    QualifyReleaseLatchV4Stress,
    QualifyReleaseRecovery,
    RestoreReleaseQualification,
    CollectDiagnosticFacts,
    ClearLatchDiagnostics,
    CollectLatestCrashReport,
    /// Runs one bounded, self-restoring CRT destination-rectangle experiment.
    RunCrtGeometryTrial {
        rectangle: [u16; 4],
    },
    /// Runs the product launcher screensaver for a bounded interval in the active CRT mode.
    RunCrtScreensaverTrial,
    /// Runs the product screensaver trial in each standard CRT mode and restores the original mode.
    RunCrtScreensaverMatrix,
    RepairSafeDeviceState,
    RecoverWithOneShotReboot,
    CaptureFramebuffer,
    InstallAlphaCandidate {
        tag: String,
        hashes: AlphaCandidateHashes,
        restore_on_failure: bool,
    },
    RestoreAlphaHostMode {
        original_main: Option<String>,
    },
    EnsureInstalledAlphaLauncher {
        expected_build_version: String,
        expected_source_revision: String,
    },
    InspectPublicCatalog,
    BeginLauncherAutomation {
        expected_build_version: String,
        expected_source_revision: String,
        expected_main_generation: u64,
        lifetime_seconds: u64,
    },
    SendLauncherAutomationAction {
        nonce: String,
        action: AutomationAction,
    },
    AwaitLauncherAutomationPresented {
        nonce: String,
        action_sequence: u64,
        timeout_ms: u64,
    },
    ReadLauncherAutomationSnapshot {
        nonce: String,
    },
    CaptureLauncherAutomationCheckpoint {
        nonce: String,
        action_sequence: u64,
        label: String,
        output_dir: PathBuf,
    },
    ExerciseLauncherAutomationLaunchReturn {
        nonce: String,
        expected_game_id: String,
        lifetime_seconds: u64,
    },
    EndLauncherAutomation {
        nonce: String,
    },
}

impl DeviceRequest {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Status => "status",
            Self::ReadDevelopmentManifest => "read-development-manifest",
            Self::VerifyDevelopmentPlatform => "verify-development-platform",
            Self::FetchVerifiedDevelopmentManager { .. } => "fetch-verified-development-manager",
            Self::DeliverRuntimeTransaction { .. } => "deliver-runtime-transaction",
            Self::DeliverPlatformTransaction { .. } => "deliver-platform-transaction",
            Self::DeliverLocalMainTransaction { .. } => "deliver-local-main-transaction",
            Self::ProfileInstalledScreensaver { .. } => "profile-installed-screensaver",
            Self::ProfileInstalledParticles { .. } => "profile-installed-particles",
            Self::ProfileInstalledParticleCapacity { .. } => "profile-installed-particle-capacity",
            Self::ProfileInstalledParticleDemo40k { .. } => "profile-installed-particle-demo-40k",
            Self::ProfileInstalledParticleStep { .. } => "profile-installed-particle-step",
            Self::ProfileInstalledParticleCpu { .. } => "profile-installed-particle-cpu",
            Self::ProfileInstalledParticleShowcase {
                cpu_profile: false, ..
            } => "profile-installed-particle-showcase",
            Self::ProfileInstalledParticleShowcase {
                cpu_profile: true, ..
            } => "profile-installed-particle-showcase-cpu",
            Self::CaptureInstalledFireworkVisual { .. } => "capture-installed-firework-visual",
            Self::CaptureInstalledParticleTechnique { .. } => {
                "capture-installed-particle-technique"
            }
            Self::WatchLiveParticles { .. } => "watch-live-particles",
            Self::ProfileInstalledSearch { .. } => "profile-installed-search",
            Self::VerifyInstalledSearchUi { .. } => "verify-installed-search-ui",
            Self::ProfileInstalledCatalogLifecycle { .. } => "profile-installed-catalog-lifecycle",
            Self::ProfileInstalledLaunchReturn { .. } => "profile-installed-launch-return",
            Self::ProfileInstalledLaunchReturnFallback { .. } => {
                "profile-installed-launch-return-fallback"
            }
            Self::ProfileInstalledColdBoot { .. } => "profile-installed-cold-boot",
            Self::ProfileInstalledNavigationTransitions { .. } => {
                "profile-installed-navigation-transitions"
            }
            Self::VerifyHealth(_) => "verify-health",
            Self::BeginReleaseQualification => "begin-release-qualification",
            Self::QualifyReleaseRuntime => "qualify-release-runtime",
            Self::QualifyReleaseCatalog => "qualify-release-catalog",
            Self::QualifyReleaseInputAndHandoff => "qualify-release-input-and-handoff",
            Self::QualifyReleaseDisplay => "qualify-release-display",
            Self::QualifyReleaseLatchV4Stress => "qualify-release-latch-v4-stress",
            Self::QualifyReleaseRecovery => "qualify-release-recovery",
            Self::RestoreReleaseQualification => "restore-release-qualification",
            Self::CollectDiagnosticFacts => "collect-diagnostic-facts",
            Self::ClearLatchDiagnostics => "clear-latch-diagnostics",
            Self::CollectLatestCrashReport => "collect-latest-crash-report",
            Self::RunCrtGeometryTrial { .. } => "run-crt-geometry-trial",
            Self::RunCrtScreensaverTrial => "run-crt-screensaver-trial",
            Self::RunCrtScreensaverMatrix => "run-crt-screensaver-matrix",
            Self::RepairSafeDeviceState => "repair-safe-device-state",
            Self::RecoverWithOneShotReboot => "recover-with-one-shot-reboot",
            Self::CaptureFramebuffer => "capture-framebuffer",
            Self::InstallAlphaCandidate { .. } => "install-alpha-candidate",
            Self::RestoreAlphaHostMode { .. } => "restore-alpha-host-mode",
            Self::EnsureInstalledAlphaLauncher { .. } => "ensure-installed-alpha-launcher",
            Self::InspectPublicCatalog => "inspect-public-catalog",
            Self::BeginLauncherAutomation { .. } => "begin-launcher-automation",
            Self::SendLauncherAutomationAction { .. } => "send-launcher-automation-action",
            Self::AwaitLauncherAutomationPresented { .. } => "await-launcher-automation-presented",
            Self::ReadLauncherAutomationSnapshot { .. } => "read-launcher-automation-snapshot",
            Self::CaptureLauncherAutomationCheckpoint { .. } => {
                "capture-launcher-automation-checkpoint"
            }
            Self::ExerciseLauncherAutomationLaunchReturn { .. } => {
                "exercise-launcher-automation-launch-return"
            }
            Self::EndLauncherAutomation { .. } => "end-launcher-automation",
        }
    }

    #[must_use]
    pub const fn allowed_during_benchmark(&self) -> bool {
        matches!(
            self,
            Self::Discover
                | Self::ReadDevelopmentManifest
                | Self::VerifyDevelopmentPlatform
                | Self::ProfileInstalledScreensaver { .. }
                | Self::ProfileInstalledParticles { .. }
                | Self::ProfileInstalledParticleCapacity { .. }
                | Self::ProfileInstalledParticleDemo40k { .. }
                | Self::ProfileInstalledParticleStep { .. }
                | Self::ProfileInstalledParticleCpu { .. }
                | Self::ProfileInstalledSearch { .. }
                | Self::VerifyInstalledSearchUi { .. }
                | Self::ProfileInstalledCatalogLifecycle { .. }
                | Self::ProfileInstalledLaunchReturn { .. }
                | Self::ProfileInstalledLaunchReturnFallback { .. }
                | Self::ProfileInstalledColdBoot { .. }
                | Self::ProfileInstalledNavigationTransitions { .. }
                | Self::VerifyHealth(Layout::Development)
        )
    }

    #[must_use]
    pub const fn retryable_after_unavailable(&self) -> bool {
        matches!(
            self,
            Self::Discover
                | Self::Status
                | Self::ReadDevelopmentManifest
                | Self::VerifyDevelopmentPlatform
                | Self::FetchVerifiedDevelopmentManager { .. }
                | Self::VerifyHealth(_)
                | Self::CollectDiagnosticFacts
                | Self::CollectLatestCrashReport
                | Self::InspectPublicCatalog
                | Self::AwaitLauncherAutomationPresented { .. }
                | Self::ReadLauncherAutomationSnapshot { .. }
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceResponse {
    pub operation: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceFailure {
    Busy(String),
    AccessDenied(String),
    Unavailable(String),
    Authentication(String),
    InvalidRequest(String),
    ArtifactMismatch(String),
    Unhealthy(String),
    OperationFailed(String),
    RecoveryRequired(String),
}

pub trait DeviceOperations {
    fn execute(&mut self, request: &DeviceRequest) -> Result<DeviceResponse, DeviceFailure>;
}

#[derive(Clone, Debug, Default)]
pub struct FakeDevice {
    responses: VecDeque<Result<DeviceResponse, DeviceFailure>>,
    requests: Vec<DeviceRequest>,
}

impl FakeDevice {
    #[must_use]
    pub fn with_results(
        results: impl IntoIterator<Item = Result<DeviceResponse, DeviceFailure>>,
    ) -> Self {
        Self {
            responses: results.into_iter().collect(),
            requests: Vec::new(),
        }
    }

    #[must_use]
    pub fn requests(&self) -> &[DeviceRequest] {
        &self.requests
    }
}

impl DeviceOperations for FakeDevice {
    fn execute(&mut self, request: &DeviceRequest) -> Result<DeviceResponse, DeviceFailure> {
        self.requests.push(request.clone());
        self.responses.pop_front().unwrap_or_else(|| {
            Err(DeviceFailure::Unavailable(
                "no fake response configured".into(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_records_typed_requests_and_failures() {
        let mut fake = FakeDevice::with_results([
            Ok(DeviceResponse {
                operation: "status",
                detail: "{}".into(),
            }),
            Err(DeviceFailure::Unavailable("offline".into())),
        ]);
        fake.execute(&DeviceRequest::Status).unwrap();
        assert!(fake.execute(&DeviceRequest::CaptureFramebuffer).is_err());
        assert_eq!(
            fake.requests(),
            &[DeviceRequest::Status, DeviceRequest::CaptureFramebuffer]
        );
    }

    #[test]
    fn normal_api_has_no_remote_shell_request() {
        let labels = [
            DeviceRequest::Discover,
            DeviceRequest::Status,
            DeviceRequest::ReadDevelopmentManifest,
            DeviceRequest::VerifyDevelopmentPlatform,
            DeviceRequest::FetchVerifiedDevelopmentManager {
                local: "manager".into(),
                expected_sha256: "a".repeat(64),
            },
            DeviceRequest::DeliverRuntimeTransaction {
                local: "l".into(),
                remote: "r".into(),
                manifest_local: "ml".into(),
                manifest_remote: "m".into(),
                expected_sha256: "a".repeat(64),
            },
            DeviceRequest::DeliverPlatformTransaction {
                stage: "s".into(),
                expected_sha256: "a".repeat(64),
            },
            DeviceRequest::DeliverLocalMainTransaction {
                local: "main".into(),
                manifest_local: "manifest".into(),
                expected_main_sha256: "a".repeat(64),
                expected_gui_sha256: "b".repeat(64),
            },
            DeviceRequest::VerifyHealth(Layout::Development),
            DeviceRequest::CaptureFramebuffer,
        ]
        .map(|request| request.label());
        assert!(!labels.contains(&"run"));
        assert!(!labels.contains(&"shell"));
    }

    #[test]
    fn every_typed_request_has_a_stable_non_shell_label() {
        let requests = vec![
            DeviceRequest::Discover,
            DeviceRequest::Status,
            DeviceRequest::ReadDevelopmentManifest,
            DeviceRequest::VerifyDevelopmentPlatform,
            DeviceRequest::FetchVerifiedDevelopmentManager {
                local: "manager".into(),
                expected_sha256: "a".repeat(64),
            },
            DeviceRequest::DeliverRuntimeTransaction {
                local: "l".into(),
                remote: "r".into(),
                manifest_local: "ml".into(),
                manifest_remote: "m".into(),
                expected_sha256: "a".repeat(64),
            },
            DeviceRequest::DeliverPlatformTransaction {
                stage: "s".into(),
                expected_sha256: "a".repeat(64),
            },
            DeviceRequest::DeliverLocalMainTransaction {
                local: "main".into(),
                manifest_local: "manifest".into(),
                expected_main_sha256: "a".repeat(64),
                expected_gui_sha256: "b".repeat(64),
            },
            DeviceRequest::ProfileInstalledScreensaver {
                output_dir: "profiles".into(),
            },
            DeviceRequest::ProfileInstalledParticles {
                output_dir: "particle-profiles".into(),
            },
            DeviceRequest::ProfileInstalledParticleCapacity {
                output_dir: "particle-capacity-profiles".into(),
            },
            DeviceRequest::ProfileInstalledParticleDemo40k {
                output_dir: "particle-demo-40k-profiles".into(),
            },
            DeviceRequest::ProfileInstalledParticleStep {
                output_dir: "particle-step-profiles".into(),
            },
            DeviceRequest::ProfileInstalledParticleCpu {
                output_dir: "particle-cpu-profiles".into(),
            },
            DeviceRequest::ProfileInstalledParticleShowcase {
                output_dir: "particle-showcase-profiles".into(),
                demo: 1,
                cpu_profile: false,
            },
            DeviceRequest::ProfileInstalledParticleShowcase {
                output_dir: "particle-showcase-cpu-profiles".into(),
                demo: 1,
                cpu_profile: true,
            },
            DeviceRequest::CaptureInstalledFireworkVisual {
                output_dir: "firework-visual".into(),
                demo: 1,
                label: "solar-chrysanthemum".into(),
                time_ms: 2100,
            },
            DeviceRequest::CaptureInstalledParticleTechnique {
                output_dir: "particle-technique".into(),
                demo: 24,
                label: "curl-noise-flow-field".into(),
                hero_secs: 15,
            },
            DeviceRequest::WatchLiveParticles {
                family: "fireworks.json".into(),
                demo: 1,
            },
            DeviceRequest::ProfileInstalledSearch {
                output_dir: "search-profile".into(),
            },
            DeviceRequest::VerifyInstalledSearchUi {
                output_dir: "search-ui".into(),
            },
            DeviceRequest::ProfileInstalledCatalogLifecycle {
                output_dir: "catalog-profile".into(),
            },
            DeviceRequest::ProfileInstalledLaunchReturn {
                output_dir: "launch-return-profile".into(),
            },
            DeviceRequest::ProfileInstalledLaunchReturnFallback {
                output_dir: "launch-return-fallback-profile".into(),
            },
            DeviceRequest::ProfileInstalledColdBoot {
                output_dir: "cold-boot-profile".into(),
            },
            DeviceRequest::ProfileInstalledNavigationTransitions {
                output_dir: "navigation-transition-profile".into(),
            },
            DeviceRequest::VerifyHealth(Layout::Public),
            DeviceRequest::BeginReleaseQualification,
            DeviceRequest::QualifyReleaseRuntime,
            DeviceRequest::QualifyReleaseCatalog,
            DeviceRequest::QualifyReleaseInputAndHandoff,
            DeviceRequest::QualifyReleaseDisplay,
            DeviceRequest::QualifyReleaseLatchV4Stress,
            DeviceRequest::QualifyReleaseRecovery,
            DeviceRequest::RestoreReleaseQualification,
            DeviceRequest::CollectDiagnosticFacts,
            DeviceRequest::CollectLatestCrashReport,
            DeviceRequest::RunCrtGeometryTrial {
                rectangle: [45, 684, 40, 615],
            },
            DeviceRequest::RunCrtScreensaverTrial,
            DeviceRequest::RunCrtScreensaverMatrix,
            DeviceRequest::RepairSafeDeviceState,
            DeviceRequest::RecoverWithOneShotReboot,
            DeviceRequest::CaptureFramebuffer,
            DeviceRequest::RestoreAlphaHostMode {
                original_main: Some("MiSTer_MagiKDev".into()),
            },
            DeviceRequest::EnsureInstalledAlphaLauncher {
                expected_build_version: "0.2.1".into(),
                expected_source_revision: "deadbeef".into(),
            },
            DeviceRequest::InspectPublicCatalog,
            DeviceRequest::BeginLauncherAutomation {
                expected_build_version: "0.2.1".into(),
                expected_source_revision: "deadbeef".into(),
                expected_main_generation: 1,
                lifetime_seconds: 120,
            },
            DeviceRequest::SendLauncherAutomationAction {
                nonce: "a".repeat(64),
                action: AutomationAction::Tap(AutomationButton::A),
            },
            DeviceRequest::AwaitLauncherAutomationPresented {
                nonce: "a".repeat(64),
                action_sequence: 1,
                timeout_ms: 1_000,
            },
            DeviceRequest::ReadLauncherAutomationSnapshot {
                nonce: "a".repeat(64),
            },
            DeviceRequest::CaptureLauncherAutomationCheckpoint {
                nonce: "a".repeat(64),
                action_sequence: 1,
                label: "home".into(),
                output_dir: "checkpoints".into(),
            },
            DeviceRequest::ExerciseLauncherAutomationLaunchReturn {
                nonce: "a".repeat(64),
                expected_game_id: "/media/fat/_Arcade/game.mra".into(),
                lifetime_seconds: 120,
            },
            DeviceRequest::EndLauncherAutomation {
                nonce: "a".repeat(64),
            },
        ];
        let labels: Vec<_> = requests.iter().map(DeviceRequest::label).collect();
        assert_eq!(labels.len(), 53);
        assert!(labels.iter().all(|label| !label.is_empty()));
        assert_eq!(
            labels
                .iter()
                .filter(|label| label.contains("benchmark") || label.contains("profile"))
                .copied()
                .collect::<Vec<_>>(),
            [
                "profile-installed-screensaver",
                "profile-installed-particles",
                "profile-installed-particle-capacity",
                "profile-installed-particle-demo-40k",
                "profile-installed-particle-step",
                "profile-installed-particle-cpu",
                "profile-installed-particle-showcase",
                "profile-installed-particle-showcase-cpu",
                "profile-installed-search",
                "profile-installed-catalog-lifecycle",
                "profile-installed-launch-return",
                "profile-installed-launch-return-fallback",
                "profile-installed-cold-boot",
                "profile-installed-navigation-transitions"
            ]
        );
        assert!(!labels.contains(&"run"));
        assert!(!labels.contains(&"shell"));
    }

    #[test]
    fn benchmark_policy_allows_installed_profiles_but_rejects_delivery() {
        assert!(DeviceRequest::Discover.allowed_during_benchmark());
        assert!(
            DeviceRequest::ProfileInstalledLaunchReturn {
                output_dir: "launch-return".into(),
            }
            .allowed_during_benchmark()
        );
        assert!(
            DeviceRequest::ProfileInstalledLaunchReturnFallback {
                output_dir: "launch-return-fallback".into(),
            }
            .allowed_during_benchmark()
        );
        assert!(
            DeviceRequest::ProfileInstalledColdBoot {
                output_dir: "cold-boot".into(),
            }
            .allowed_during_benchmark()
        );
        assert!(DeviceRequest::VerifyHealth(Layout::Development).allowed_during_benchmark());
        assert!(!DeviceRequest::VerifyHealth(Layout::Public).allowed_during_benchmark());
        assert!(
            !DeviceRequest::DeliverRuntimeTransaction {
                local: "runtime".into(),
                remote: "runtime-remote".into(),
                manifest_local: "manifest".into(),
                manifest_remote: "manifest-remote".into(),
                expected_sha256: "a".repeat(64),
            }
            .allowed_during_benchmark()
        );
        assert!(
            !DeviceRequest::DeliverLocalMainTransaction {
                local: "main".into(),
                manifest_local: "manifest".into(),
                expected_main_sha256: "a".repeat(64),
                expected_gui_sha256: "b".repeat(64),
            }
            .allowed_during_benchmark()
        );
    }

    #[test]
    fn only_read_only_requests_retry_after_unavailability() {
        assert!(DeviceRequest::Discover.retryable_after_unavailable());
        assert!(DeviceRequest::CollectDiagnosticFacts.retryable_after_unavailable());
        assert!(DeviceRequest::ReadDevelopmentManifest.retryable_after_unavailable());
        assert!(!DeviceRequest::RepairSafeDeviceState.retryable_after_unavailable());
        assert!(!DeviceRequest::RecoverWithOneShotReboot.retryable_after_unavailable());
        assert!(
            !DeviceRequest::DeliverPlatformTransaction {
                stage: "stage".into(),
                expected_sha256: "a".repeat(64),
            }
            .retryable_after_unavailable()
        );
    }

    #[test]
    fn fake_without_scripted_result_fails_closed_and_records_request() {
        let mut fake = FakeDevice::default();
        assert_eq!(
            fake.execute(&DeviceRequest::Status),
            Err(DeviceFailure::Unavailable(
                "no fake response configured".into()
            ))
        );
        assert_eq!(fake.requests(), &[DeviceRequest::Status]);
    }
}
