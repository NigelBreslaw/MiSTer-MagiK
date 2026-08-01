// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};
use mister_tool::transport::{DeviceOperations, DeviceRequest};

pub struct DeviceClient<D = mister_tool::NativeDevice> {
    device: D,
}

pub struct BenchmarkDeviceClient<D = mister_tool::NativeDevice> {
    client: DeviceClient<D>,
}

impl Default for BenchmarkDeviceClient<mister_tool::NativeDevice> {
    fn default() -> Self {
        Self {
            client: DeviceClient::default(),
        }
    }
}

impl<D: DeviceOperations> BenchmarkDeviceClient<D> {
    #[cfg(test)]
    pub const fn new(device: D) -> Self {
        Self {
            client: DeviceClient::new(device),
        }
    }

    pub fn execute(&mut self, request: DeviceRequest) -> AgentResult<String> {
        if !request.allowed_during_benchmark() {
            return Err(format!(
                "benchmark policy rejects device operation {}",
                request.label()
            )
            .into());
        }
        self.client.execute(request)
    }
}

impl Default for DeviceClient<mister_tool::NativeDevice> {
    fn default() -> Self {
        Self {
            device: mister_tool::NativeDevice::default(),
        }
    }
}

impl<D: DeviceOperations> DeviceClient<D> {
    #[must_use]
    pub const fn new(device: D) -> Self {
        Self { device }
    }

    pub fn execute(&mut self, request: DeviceRequest) -> AgentResult<String> {
        self.execute_typed(request)
            .map(|response| response.detail)
            .map_err(AgentError::from)
    }

    pub fn execute_typed(
        &mut self,
        request: DeviceRequest,
    ) -> Result<mister_tool::transport::DeviceResponse, mister_tool::transport::DeviceFailure> {
        self.device.execute(&request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_tool::transport::{DeviceFailure, DeviceResponse, FakeDevice};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct RecordingDevice(Rc<RefCell<Vec<DeviceRequest>>>);

    impl DeviceOperations for RecordingDevice {
        fn execute(&mut self, request: &DeviceRequest) -> Result<DeviceResponse, DeviceFailure> {
            self.0.borrow_mut().push(request.clone());
            Ok(DeviceResponse {
                operation: request.label(),
                detail: "snapshotted".into(),
            })
        }
    }

    #[test]
    fn typed_failures_have_actionable_stable_classifications() {
        let fake =
            FakeDevice::with_results([Err(DeviceFailure::Authentication("bad token".into()))]);
        let error = DeviceClient::new(fake)
            .execute(DeviceRequest::Status)
            .unwrap_err();
        assert_eq!(error.to_string(), "authentication_required: bad token");
    }

    #[test]
    fn typed_success_returns_only_operation_detail() {
        let fake = FakeDevice::with_results([Ok(DeviceResponse {
            operation: "status",
            detail: "healthy".into(),
        })]);
        assert_eq!(
            DeviceClient::new(fake)
                .execute(DeviceRequest::Status)
                .unwrap(),
            "healthy"
        );
    }

    #[test]
    fn typed_requests_are_forwarded_unchanged() {
        let request = DeviceRequest::ProfileInstalledCatalogLifecycle {
            output_dir: "/tmp/catalog-profile".into(),
        };
        let recorded = Rc::new(RefCell::new(Vec::new()));
        let mut client = DeviceClient::new(RecordingDevice(Rc::clone(&recorded)));

        assert_eq!(client.execute(request.clone()).unwrap(), "snapshotted");
        assert_eq!(recorded.borrow().as_slice(), &[request]);
    }

    #[test]
    fn benchmark_client_rejects_platform_mutation_before_transport() {
        let recorded = Rc::new(RefCell::new(Vec::new()));
        let mut client = BenchmarkDeviceClient::new(RecordingDevice(Rc::clone(&recorded)));
        let request = DeviceRequest::DeliverRuntimeTransaction {
            local: "/tmp/runtime".into(),
            remote: "/media/fat/mister-magik-dev/mister-magik-fb".into(),
            manifest_local: "/tmp/manifest".into(),
            manifest_remote: "/media/fat/mister-magik-dev/platform-v3.manifest".into(),
            expected_sha256: "a".repeat(64),
        };

        assert!(client.execute(request).is_err());
        assert!(recorded.borrow().is_empty());
    }

    #[test]
    fn benchmark_client_forwards_installed_runtime_profiles() {
        let recorded = Rc::new(RefCell::new(Vec::new()));
        let mut client = BenchmarkDeviceClient::new(RecordingDevice(Rc::clone(&recorded)));
        let request = DeviceRequest::ProfileInstalledLaunchReturn {
            output_dir: "/tmp/profile".into(),
        };

        assert_eq!(client.execute(request.clone()).unwrap(), "snapshotted");
        assert_eq!(recorded.borrow().as_slice(), &[request]);
    }

    #[test]
    fn every_typed_failure_has_a_stable_classification() {
        let cases = [
            (
                DeviceFailure::Busy("delivery already running".into()),
                "device_busy: delivery already running",
            ),
            (
                DeviceFailure::AccessDenied("local network blocked".into()),
                "device_access_denied: local network blocked",
            ),
            (
                DeviceFailure::Unavailable("offline".into()),
                "device_unavailable: offline",
            ),
            (
                DeviceFailure::Authentication("bad token".into()),
                "authentication_required: bad token",
            ),
            (
                DeviceFailure::InvalidRequest("bad mode".into()),
                "invalid_device_request: bad mode",
            ),
            (
                DeviceFailure::ArtifactMismatch("wrong hash".into()),
                "artifact_mismatch: wrong hash",
            ),
            (
                DeviceFailure::Unhealthy("no process".into()),
                "device_unhealthy: no process",
            ),
            (
                DeviceFailure::OperationFailed("copy failed".into()),
                "device_operation_failed: copy failed",
            ),
            (
                DeviceFailure::RecoveryRequired("rollback failed".into()),
                "recovery_required: rollback failed",
            ),
        ];

        for (failure, expected) in cases {
            assert_eq!(AgentError::from(failure).to_string(), expected);
        }
    }
}
