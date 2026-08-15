// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test-only filesystem fault injection for destructive device experiments.

use std::fmt;
use std::path::Path;

const POINT_ENV: &str = "MISTER_FS_FAULT_POINT";
const ACTION_ENV: &str = "MISTER_FS_FAULT_ACTION";
const DELAY_ENV: &str = "MISTER_FS_FAULT_DELAY_MS";
const SESSION_ENV: &str = "MISTER_FS_FAULT_SESSION";
const DEFAULT_DELAY_MS: u64 = 2_000;
const DIRECT_RESET_NO_SYNC: &str = "direct-reset-no-sync";

/// Evidence describing a publication point that may trigger the attended
/// direct-reset fault. The target is diagnostic context only; implementations
/// own every control endpoint and command spelling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectResetFaultRequest {
    point: String,
    target: String,
}

impl DirectResetFaultRequest {
    pub fn new(point: impl Into<String>, target: &Path) -> Self {
        Self {
            point: point.into(),
            target: target.display().to_string(),
        }
    }

    pub fn point(&self) -> &str {
        &self.point
    }

    pub fn target(&self) -> &str {
        &self.target
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DirectResetFaultOutcome {
    #[default]
    Noop,
    PointMismatch,
    NotArmed,
    UnsupportedAction,
    ResetRequested,
}

/// Narrow destructive-fault capability.
///
/// Callers provide only typed event evidence. Implementations own arming,
/// marker/session paths, Main transport, reset command spelling, delay, and
/// cleanup.
pub trait DirectResetFaultControl {
    fn request_direct_reset(
        &mut self,
        request: &DirectResetFaultRequest,
    ) -> DirectResetFaultOutcome;
}

#[derive(Default)]
pub struct NoopDirectResetFaultControl;

impl DirectResetFaultControl for NoopDirectResetFaultControl {
    fn request_direct_reset(
        &mut self,
        _request: &DirectResetFaultRequest,
    ) -> DirectResetFaultOutcome {
        DirectResetFaultOutcome::Noop
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct FaultConfig {
    point: String,
    action: String,
    delay_ms: u64,
    session: Option<String>,
}

impl FaultConfig {
    /// Capture the compatible destructive-fault controls once at process entry.
    ///
    /// The session token remains volatile and is never serialized by this type.
    pub fn capture_from_process() -> Option<Self> {
        Self::from_compatible_values(
            std::env::var(POINT_ENV).ok().as_deref(),
            std::env::var(ACTION_ENV).ok().as_deref(),
            std::env::var(DELAY_ENV).ok().as_deref(),
            std::env::var(SESSION_ENV).ok().as_deref(),
        )
    }

    pub fn point(&self) -> &str {
        &self.point
    }

    pub fn action(&self) -> &str {
        &self.action
    }

    pub fn is_direct_reset_no_sync(&self) -> bool {
        self.action == DIRECT_RESET_NO_SYNC
    }

    pub fn delay_ms(&self) -> u64 {
        self.delay_ms
    }

    pub fn session_token(&self) -> Option<&str> {
        self.session.as_deref()
    }

    pub fn from_compatible_values(
        point: Option<&str>,
        action: Option<&str>,
        delay_ms: Option<&str>,
        session: Option<&str>,
    ) -> Option<Self> {
        let point = point?;
        if point.trim().is_empty() {
            return None;
        }
        let action = action.unwrap_or(DIRECT_RESET_NO_SYNC).to_string();
        let delay_ms = delay_ms
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_DELAY_MS);
        Some(Self {
            point: point.to_string(),
            action,
            delay_ms,
            session: session
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string),
        })
    }
}

impl fmt::Debug for FaultConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FaultConfig")
            .field("point", &self.point)
            .field("action", &self.action)
            .field("delay_ms", &self.delay_ms)
            .field("session", &self.session.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Notify the injected capability. Catalog code has no control endpoint,
/// command transport, marker, session, or delay access.
pub fn maybe_fault_with_control(
    point: &str,
    target: impl AsRef<Path>,
    control: &mut dyn DirectResetFaultControl,
) -> DirectResetFaultOutcome {
    let target = target.as_ref();
    control.request_direct_reset(&DirectResetFaultRequest::new(point, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingFaultControl {
        requests: Vec<DirectResetFaultRequest>,
        outcome: DirectResetFaultOutcome,
    }

    impl DirectResetFaultControl for RecordingFaultControl {
        fn request_direct_reset(
            &mut self,
            request: &DirectResetFaultRequest,
        ) -> DirectResetFaultOutcome {
            self.requests.push(request.clone());
            self.outcome
        }
    }

    #[test]
    fn config_is_absent_without_point_env() {
        assert!(FaultConfig::from_compatible_values(None, None, None, None).is_none());
    }

    #[test]
    fn portable_fault_control_is_effect_free_by_default_and_fake_is_deterministic() {
        let request = DirectResetFaultRequest::new(
            "catalog.sqlite.after_final_temp_sync",
            Path::new("/media/fat/mister-magik/library.sqlite3"),
        );
        assert_eq!(
            NoopDirectResetFaultControl.request_direct_reset(&request),
            DirectResetFaultOutcome::Noop
        );

        let mut fake = RecordingFaultControl {
            outcome: DirectResetFaultOutcome::ResetRequested,
            ..RecordingFaultControl::default()
        };
        assert_eq!(
            fake.request_direct_reset(&request),
            DirectResetFaultOutcome::ResetRequested
        );
        assert_eq!(fake.requests, vec![request]);
    }

    #[test]
    fn env_config_defaults_to_direct_reset_no_sync() {
        let config =
            FaultConfig::from_compatible_values(Some("settings.after_rename"), None, None, None)
                .expect("config");
        assert_eq!(config.action, DIRECT_RESET_NO_SYNC);
        assert_eq!(config.delay_ms, DEFAULT_DELAY_MS);
        assert_eq!(config.session, None);
    }

    #[test]
    fn captured_config_redacts_the_volatile_session_token() {
        let config = FaultConfig::from_compatible_values(
            Some("settings.after_rename"),
            Some(DIRECT_RESET_NO_SYNC),
            Some("17"),
            Some("secret-session-token"),
        )
        .expect("config");

        assert_eq!(config.point(), "settings.after_rename");
        assert_eq!(config.action(), DIRECT_RESET_NO_SYNC);
        assert_eq!(config.delay_ms(), 17);
        assert_eq!(config.session_token(), Some("secret-session-token"));
        let debug = format!("{config:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret-session-token"));
    }
}
