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
    RepairSafeDeviceState,
    RecoverWithOneShotReboot,
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
            Self::DeliverRuntimeTransaction { .. } => "deliver-runtime-transaction",
            Self::DeliverPlatformTransaction { .. } => "deliver-platform-transaction",
            Self::DeliverLocalMainTransaction { .. } => "deliver-local-main-transaction",
            Self::ProfileInstalledScreensaver { .. } => "profile-installed-screensaver",
            Self::ProfileInstalledParticles { .. } => "profile-installed-particles",
            Self::ProfileInstalledParticleCapacity { .. } => "profile-installed-particle-capacity",
            Self::ProfileInstalledParticleDemo40k { .. } => "profile-installed-particle-demo-40k",
            Self::ProfileInstalledParticleStep { .. } => "profile-installed-particle-step",
            Self::ProfileInstalledParticleCpu { .. } => "profile-installed-particle-cpu",
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
            Self::RepairSafeDeviceState => "repair-safe-device-state",
            Self::RecoverWithOneShotReboot => "recover-with-one-shot-reboot",
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
                | Self::VerifyHealth(_)
                | Self::CollectDiagnosticFacts
                | Self::InspectPublicCatalog
                | Self::AwaitLauncherAutomationPresented { .. }
                | Self::ReadLauncherAutomationSnapshot { .. }
        )
    }

    #[must_use]
    pub const fn mutates_device(&self) -> bool {
        !matches!(
            self,
            Self::Discover
                | Self::Status
                | Self::ReadDevelopmentManifest
                | Self::VerifyDevelopmentPlatform
                | Self::VerifyHealth(_)
                | Self::CollectDiagnosticFacts
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
        assert!(
            fake.execute(&DeviceRequest::RecoverWithOneShotReboot)
                .is_err()
        );
        assert_eq!(
            fake.requests(),
            &[
                DeviceRequest::Status,
                DeviceRequest::RecoverWithOneShotReboot
            ]
        );
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
