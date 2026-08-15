// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::transport::DeviceFailure;
use mister_magik_agent_protocol::{FailureCode, FailureMetadata};
use thiserror::Error;

pub type AgentResult<T> = Result<T, AgentError>;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum AgentError {
    #[error("{0}")]
    Message(String),
    #[error("{code}: {detail}")]
    Classified { code: &'static str, detail: String },
    #[error("{phase}: {source}")]
    Phase {
        phase: &'static str,
        #[source]
        source: Box<Self>,
    },
    #[error("cancelled: {0}")]
    Cancelled(Box<Self>),
    #[error("recovery_required: {message}")]
    RecoveryRequired { message: String },
    #[error("{message}")]
    StructuredDevice {
        message: String,
        failure: FailureMetadata,
    },
}

impl AgentError {
    #[must_use]
    pub fn phase(phase: &'static str, source: impl Into<Self>) -> Self {
        Self::Phase {
            phase,
            source: Box::new(source.into()),
        }
    }

    #[must_use]
    pub fn cancelled(source: impl Into<Self>) -> Self {
        Self::Cancelled(Box::new(source.into()))
    }

    #[must_use]
    pub fn recovery_required(context: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::RecoveryRequired {
            message: format!("{}; {}", context.into(), detail.into()),
        }
    }

    #[must_use]
    pub fn structured_device(message: impl Into<String>, failure: FailureMetadata) -> Self {
        Self::StructuredDevice {
            message: message.into(),
            failure,
        }
    }

    #[must_use]
    pub fn structured_failure(&self) -> Option<&FailureMetadata> {
        match self {
            Self::StructuredDevice { failure, .. } => Some(failure),
            Self::Phase { source, .. } | Self::Cancelled(source) => source.structured_failure(),
            Self::Message(_) | Self::Classified { .. } | Self::RecoveryRequired { .. } => None,
        }
    }

    #[must_use]
    pub fn device_failure(&self) -> Option<DeviceFailure> {
        let failure = self.structured_failure()?;
        let detail = failure.detail.clone();
        Some(
            if failure.recovery_required || failure.code == FailureCode::RecoveryRequired {
                DeviceFailure::RecoveryRequired(detail)
            } else {
                match &failure.code {
                    FailureCode::DeviceBusy => DeviceFailure::Busy(detail),
                    FailureCode::AccessDenied => DeviceFailure::AccessDenied(detail),
                    FailureCode::DeviceUnavailable => DeviceFailure::Unavailable(detail),
                    FailureCode::AuthenticationRequired => DeviceFailure::Authentication(detail),
                    FailureCode::UnknownCommand | FailureCode::InvalidRequest => {
                        DeviceFailure::InvalidRequest(detail)
                    }
                    FailureCode::ArtifactMismatch => DeviceFailure::ArtifactMismatch(detail),
                    FailureCode::OperationFailed
                    | FailureCode::Cancelled
                    | FailureCode::Unknown(_)
                    | FailureCode::RecoveryRequired => DeviceFailure::OperationFailed(detail),
                }
            },
        )
    }

    #[must_use]
    pub fn is_recovery_required(&self) -> bool {
        match self {
            Self::RecoveryRequired { .. } => true,
            Self::StructuredDevice { failure, .. } => failure.recovery_required,
            Self::Phase { source, .. } | Self::Cancelled(source) => source.is_recovery_required(),
            Self::Message(_) | Self::Classified { .. } => false,
        }
    }
}

impl From<String> for AgentError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for AgentError {
    fn from(value: &str) -> Self {
        Self::Message(value.to_owned())
    }
}

impl From<DeviceFailure> for AgentError {
    fn from(failure: DeviceFailure) -> Self {
        let (code, detail) = match failure {
            DeviceFailure::Busy(detail) => ("device_busy", detail),
            DeviceFailure::AccessDenied(detail) => ("device_access_denied", detail),
            DeviceFailure::Unavailable(detail) => ("device_unavailable", detail),
            DeviceFailure::Authentication(detail) => ("authentication_required", detail),
            DeviceFailure::InvalidRequest(detail) => ("invalid_device_request", detail),
            DeviceFailure::ArtifactMismatch(detail) => ("artifact_mismatch", detail),
            DeviceFailure::Unhealthy(detail) => ("device_unhealthy", detail),
            DeviceFailure::OperationFailed(detail) => ("device_operation_failed", detail),
            DeviceFailure::RecoveryRequired(detail) => {
                return Self::RecoveryRequired { message: detail };
            }
        };
        Self::Classified { code, detail }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_classification_survives_context() {
        let error = AgentError::phase(
            "smoke",
            AgentError::recovery_required("rollback failed", "device unavailable"),
        );
        assert!(error.is_recovery_required());
        assert!(error.to_string().starts_with("smoke: recovery_required:"));
    }
}
