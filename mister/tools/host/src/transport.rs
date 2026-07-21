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
    SnapshotRuntime {
        remote: String,
    },
    DeployRuntime {
        local: PathBuf,
        remote: String,
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
    CaptureFramebuffer,
}

impl DeviceRequest {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Status => "status",
            Self::SnapshotRuntime { .. } => "snapshot-runtime",
            Self::DeployRuntime { .. } => "deploy-runtime",
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
}
