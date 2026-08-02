// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};
use crate::transport::DeviceFailure;
use std::thread;
use std::time::Duration;

const READ_ONLY_RETRY_DELAY: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct DeviceClient {
    device: crate::NativeDevice,
}

impl DeviceClient {
    pub(crate) fn read<T>(
        &mut self,
        mut operation: impl FnMut(&mut crate::NativeDevice) -> Result<T, DeviceFailure>,
    ) -> AgentResult<T> {
        match operation(&mut self.device) {
            Ok(value) => Ok(value),
            Err(DeviceFailure::Unavailable(_)) => {
                thread::sleep(READ_ONLY_RETRY_DELAY);
                operation(&mut self.device).map_err(AgentError::from)
            }
            Err(error) => Err(AgentError::from(error)),
        }
    }

    pub(crate) fn mutate<T>(
        &mut self,
        operation: impl FnOnce(&mut crate::NativeDevice) -> Result<T, DeviceFailure>,
    ) -> AgentResult<T> {
        operation(&mut self.device).map_err(AgentError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn read_only_unavailability_gets_one_bounded_retry() {
        let calls = Rc::new(Cell::new(0));
        let observed = Rc::clone(&calls);
        let value = DeviceClient::default()
            .read(move |_| {
                observed.set(observed.get() + 1);
                if observed.get() == 1 {
                    Err(DeviceFailure::Unavailable("transient route failure".into()))
                } else {
                    Ok("healthy")
                }
            })
            .unwrap();
        assert_eq!(value, "healthy");
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn mutations_are_never_replayed_after_unavailability() {
        let calls = Rc::new(Cell::new(0));
        let observed = Rc::clone(&calls);
        let error = DeviceClient::default()
            .mutate(move |_| -> Result<(), DeviceFailure> {
                observed.set(observed.get() + 1);
                Err(DeviceFailure::Unavailable("ambiguous timeout".into()))
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "device_unavailable: ambiguous timeout");
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn authentication_failures_are_not_retried() {
        let calls = Rc::new(Cell::new(0));
        let observed = Rc::clone(&calls);
        let error = DeviceClient::default()
            .read(move |_| -> Result<(), DeviceFailure> {
                observed.set(observed.get() + 1);
                Err(DeviceFailure::Authentication("bad token".into()))
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "authentication_required: bad token");
        assert_eq!(calls.get(), 1);
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
