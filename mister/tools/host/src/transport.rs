// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    Development,
    Public,
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
    LaunchParticleShowcase,
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
    CaptureFramebuffer,
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
            Self::LaunchParticleShowcase => "launch-particle-showcase",
            Self::ProfileInstalledSearch { .. } => "profile-installed-search",
            Self::VerifyInstalledSearchUi { .. } => "verify-installed-search-ui",
            Self::ProfileInstalledCatalogLifecycle { .. } => "profile-installed-catalog-lifecycle",
            Self::ProfileInstalledLaunchReturn { .. } => "profile-installed-launch-return",
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
            Self::CaptureFramebuffer => "capture-framebuffer",
        }
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
            DeviceRequest::CaptureFramebuffer,
        ];
        let labels: Vec<_> = requests.iter().map(DeviceRequest::label).collect();
        assert_eq!(labels.len(), 37);
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
                "profile-installed-navigation-transitions"
            ]
        );
        assert!(!labels.contains(&"run"));
        assert!(!labels.contains(&"shell"));
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
