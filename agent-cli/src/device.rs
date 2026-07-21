// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use mister_tool::transport::{DeviceFailure, DeviceOperations, DeviceRequest};

pub struct DeviceClient<D = mister_tool::NativeDevice> {
    device: D,
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

    pub fn execute(&mut self, request: DeviceRequest) -> Result<String, String> {
        self.device
            .execute(&request)
            .map(|response| response.detail)
            .map_err(render_failure)
    }
}

fn render_failure(failure: DeviceFailure) -> String {
    match failure {
        DeviceFailure::Unavailable(detail) => format!("device_unavailable: {detail}"),
        DeviceFailure::Authentication(detail) => format!("authentication_required: {detail}"),
        DeviceFailure::InvalidRequest(detail) => format!("invalid_device_request: {detail}"),
        DeviceFailure::ArtifactMismatch(detail) => format!("artifact_mismatch: {detail}"),
        DeviceFailure::Unhealthy(detail) => format!("device_unhealthy: {detail}"),
        DeviceFailure::OperationFailed(detail) => format!("device_operation_failed: {detail}"),
        DeviceFailure::RecoveryRequired(detail) => format!("recovery_required: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mister_tool::transport::{DeviceResponse, FakeDevice};

    #[test]
    fn typed_failures_have_actionable_stable_classifications() {
        let fake =
            FakeDevice::with_results([Err(DeviceFailure::Authentication("bad token".into()))]);
        let error = DeviceClient::new(fake)
            .execute(DeviceRequest::Status)
            .unwrap_err();
        assert_eq!(error, "authentication_required: bad token");
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
}
