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
pub enum MainSelection {
    Stock,
    Development,
    Public,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BenchmarkScenario {
    LauncherVelocity,
    FramebufferVelocity,
    ScreensaverVelocity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColdBenchmarkScenario {
    CatalogLifecycle,
    PreviewColdStart,
    LibraryPersistence,
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
    SnapshotRuntime {
        remote: String,
    },
    DeployRuntime {
        local: PathBuf,
        remote: String,
    },
    SnapshotRuntimeBundle {
        remote: String,
        manifest: String,
    },
    DeployRuntimeBundle {
        local: PathBuf,
        remote: String,
        manifest_local: PathBuf,
        manifest_remote: String,
    },
    RollbackRuntimeBundle {
        remote: String,
        manifest: String,
    },
    CommitRuntimeBundle {
        remote: String,
        manifest: String,
    },
    RollbackRuntime {
        remote: String,
    },
    CommitRuntime {
        remote: String,
    },
    DeployPlatform {
        stage: PathBuf,
    },
    SnapshotPlatform,
    RollbackPlatform,
    CommitPlatform,
    SelectMain(MainSelection),
    RebootWait,
    VerifyHealth(Layout),
    SmokeDelivery {
        layout: Layout,
        expected_sha256: String,
    },
    PrepareBenchmark(BenchmarkScenario),
    WarmupBenchmark(BenchmarkScenario),
    CaptureBenchmark(BenchmarkScenario),
    RestoreBenchmark,
    SnapshotBenchmarkData(ColdBenchmarkScenario),
    EstablishBenchmarkFixture(ColdBenchmarkScenario),
    ExecuteColdBenchmark(ColdBenchmarkScenario),
    CollectBenchmarkEvents(ColdBenchmarkScenario),
    RestoreBenchmarkData(ColdBenchmarkScenario),
    BeginReleaseQualification,
    QualifyReleaseRuntime,
    QualifyReleaseCatalog,
    QualifyReleaseInputAndHandoff,
    QualifyReleaseDisplay,
    QualifyReleaseRecovery,
    RestoreReleaseQualification,
    CollectDiagnosticFacts,
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
            Self::SnapshotRuntime { .. } => "snapshot-runtime",
            Self::DeployRuntime { .. } => "deploy-runtime",
            Self::SnapshotRuntimeBundle { .. } => "snapshot-runtime-bundle",
            Self::DeployRuntimeBundle { .. } => "deploy-runtime-bundle",
            Self::RollbackRuntimeBundle { .. } => "rollback-runtime-bundle",
            Self::CommitRuntimeBundle { .. } => "commit-runtime-bundle",
            Self::RollbackRuntime { .. } => "rollback-runtime",
            Self::CommitRuntime { .. } => "commit-runtime",
            Self::DeployPlatform { .. } => "deploy-platform",
            Self::SnapshotPlatform => "snapshot-platform",
            Self::RollbackPlatform => "rollback-platform",
            Self::CommitPlatform => "commit-platform",
            Self::SelectMain(_) => "select-main",
            Self::RebootWait => "reboot-wait",
            Self::VerifyHealth(_) => "verify-health",
            Self::SmokeDelivery { .. } => "smoke-delivery",
            Self::PrepareBenchmark(_) => "prepare-benchmark",
            Self::WarmupBenchmark(_) => "warmup-benchmark",
            Self::CaptureBenchmark(_) => "capture-benchmark",
            Self::RestoreBenchmark => "restore-benchmark",
            Self::SnapshotBenchmarkData(_) => "snapshot-benchmark-data",
            Self::EstablishBenchmarkFixture(_) => "establish-benchmark-fixture",
            Self::ExecuteColdBenchmark(_) => "execute-cold-benchmark",
            Self::CollectBenchmarkEvents(_) => "collect-benchmark-events",
            Self::RestoreBenchmarkData(_) => "restore-benchmark-data",
            Self::BeginReleaseQualification => "begin-release-qualification",
            Self::QualifyReleaseRuntime => "qualify-release-runtime",
            Self::QualifyReleaseCatalog => "qualify-release-catalog",
            Self::QualifyReleaseInputAndHandoff => "qualify-release-input-and-handoff",
            Self::QualifyReleaseDisplay => "qualify-release-display",
            Self::QualifyReleaseRecovery => "qualify-release-recovery",
            Self::RestoreReleaseQualification => "restore-release-qualification",
            Self::CollectDiagnosticFacts => "collect-diagnostic-facts",
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
        assert!(fake.execute(&DeviceRequest::RebootWait).is_err());
        assert_eq!(
            fake.requests(),
            &[DeviceRequest::Status, DeviceRequest::RebootWait]
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
            DeviceRequest::RollbackPlatform,
            DeviceRequest::CommitPlatform,
            DeviceRequest::RebootWait,
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
            DeviceRequest::SnapshotRuntime { remote: "r".into() },
            DeviceRequest::DeployRuntime {
                local: "l".into(),
                remote: "r".into(),
            },
            DeviceRequest::SnapshotRuntimeBundle {
                remote: "r".into(),
                manifest: "m".into(),
            },
            DeviceRequest::DeployRuntimeBundle {
                local: "l".into(),
                remote: "r".into(),
                manifest_local: "ml".into(),
                manifest_remote: "m".into(),
            },
            DeviceRequest::RollbackRuntimeBundle {
                remote: "r".into(),
                manifest: "m".into(),
            },
            DeviceRequest::CommitRuntimeBundle {
                remote: "r".into(),
                manifest: "m".into(),
            },
            DeviceRequest::RollbackRuntime { remote: "r".into() },
            DeviceRequest::CommitRuntime { remote: "r".into() },
            DeviceRequest::DeployPlatform { stage: "s".into() },
            DeviceRequest::SnapshotPlatform,
            DeviceRequest::RollbackPlatform,
            DeviceRequest::CommitPlatform,
            DeviceRequest::SelectMain(MainSelection::Stock),
            DeviceRequest::RebootWait,
            DeviceRequest::VerifyHealth(Layout::Public),
            DeviceRequest::SmokeDelivery {
                layout: Layout::Development,
                expected_sha256: "s".into(),
            },
            DeviceRequest::PrepareBenchmark(BenchmarkScenario::LauncherVelocity),
            DeviceRequest::WarmupBenchmark(BenchmarkScenario::FramebufferVelocity),
            DeviceRequest::CaptureBenchmark(BenchmarkScenario::LauncherVelocity),
            DeviceRequest::CaptureBenchmark(BenchmarkScenario::ScreensaverVelocity),
            DeviceRequest::RestoreBenchmark,
            DeviceRequest::SnapshotBenchmarkData(ColdBenchmarkScenario::CatalogLifecycle),
            DeviceRequest::EstablishBenchmarkFixture(ColdBenchmarkScenario::PreviewColdStart),
            DeviceRequest::ExecuteColdBenchmark(ColdBenchmarkScenario::LibraryPersistence),
            DeviceRequest::CollectBenchmarkEvents(ColdBenchmarkScenario::CatalogLifecycle),
            DeviceRequest::RestoreBenchmarkData(ColdBenchmarkScenario::PreviewColdStart),
            DeviceRequest::BeginReleaseQualification,
            DeviceRequest::QualifyReleaseRuntime,
            DeviceRequest::QualifyReleaseCatalog,
            DeviceRequest::QualifyReleaseInputAndHandoff,
            DeviceRequest::QualifyReleaseDisplay,
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
        assert_eq!(labels.len(), 44);
        assert!(labels.iter().all(|label| !label.is_empty()));
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
