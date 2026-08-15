// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test-only filesystem fault injection for destructive device experiments.

use serde_json::json;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const POINT_ENV: &str = "MISTER_FS_FAULT_POINT";
const ACTION_ENV: &str = "MISTER_FS_FAULT_ACTION";
const DELAY_ENV: &str = "MISTER_FS_FAULT_DELAY_MS";
const SESSION_ENV: &str = "MISTER_FS_FAULT_SESSION";
const DEFAULT_DELAY_MS: u64 = 2_000;
const MARKER_PATH: &str = "/tmp/mister-magik/fs-fault.json";
const SESSION_PATH: &str = "/tmp/mister-magik/fs-fault-session";
const MISTER_CMD: &str = "/dev/MiSTer_cmd";
const DIRECT_RESET_NO_SYNC: &str = "direct-reset-no-sync";
const DIRECT_RESET_NO_SYNC_CMD: &str = "mister_magik_direct_reset_no_sync\n";

pub const DIRECT_RESET_CLEANUP_ARTIFACTS: [&str; 7] = [
    "/media/fat/mister-magik/launcher.env",
    "/media/fat/mister-magik-dev/launcher.env",
    "/tmp/mister-magik/fs-fault-launcher.env",
    "/tmp/mister-magik/fs-fault-session",
    "/tmp/mister-magik/fs-fault.json",
    "/media/fat/mister-magik/rebuild-on-next-boot",
    "/media/fat/mister-magik-dev/rebuild-on-next-boot",
];

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
        Self::from_values(
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

    pub fn delay_ms(&self) -> u64 {
        self.delay_ms
    }

    pub fn session_token(&self) -> Option<&str> {
        self.session.as_deref()
    }

    fn from_values(
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

pub fn maybe_fault(point: &str, target: impl AsRef<Path>) {
    let Some(config) = FaultConfig::capture_from_process() else {
        return;
    };
    let target = target.as_ref();
    let mut runtime = SystemFaultRuntime;
    maybe_fault_with(point, target, &config, &mut runtime);
}

/// Notify the injected capability while the characterized legacy executor
/// remains the sole production effect owner during migration.
pub fn maybe_fault_with_control(
    point: &str,
    target: impl AsRef<Path>,
    control: &mut dyn DirectResetFaultControl,
) -> DirectResetFaultOutcome {
    let target = target.as_ref();
    let outcome = control.request_direct_reset(&DirectResetFaultRequest::new(point, target));
    maybe_fault(point, target);
    outcome
}

fn maybe_fault_with(
    point: &str,
    target: &Path,
    config: &FaultConfig,
    runtime: &mut impl FaultRuntime,
) {
    if config.point != point {
        return;
    }
    if config.action == DIRECT_RESET_NO_SYNC && !session_is_armed(config, runtime) {
        crate::catalog_errln!(
            "fs_fault: direct-reset-no-sync ignored at {point}; volatile session not armed"
        );
        return;
    }
    let _ = runtime.write_marker(MARKER_PATH, &marker_json(point, target, config));
    if config.action == DIRECT_RESET_NO_SYNC {
        let _ = runtime.send_mister_command(MISTER_CMD, DIRECT_RESET_NO_SYNC_CMD);
    } else {
        crate::catalog_errln!(
            "fs_fault: unsupported action {} at point {}",
            config.action,
            point
        );
    }
    runtime.sleep_ms(config.delay_ms);
}

fn marker_json(point: &str, target: &Path, config: &FaultConfig) -> String {
    let value = json!({
        "schema": "mister-magik-fs-fault-v1",
        "point": point,
        "target": target.display().to_string(),
        "action": config.action,
        "delay_ms": config.delay_ms,
        "session": config.session,
        "pid": std::process::id(),
        "ts_unix_ms": unix_ms_now(),
    });
    format!("{value}\n")
}

fn session_is_armed(config: &FaultConfig, runtime: &mut impl FaultRuntime) -> bool {
    let Some(expected) = config.session.as_deref() else {
        return false;
    };
    runtime
        .read_text(SESSION_PATH)
        .map(|actual| actual.trim() == expected)
        .unwrap_or(false)
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

trait FaultRuntime {
    fn read_text(&mut self, path: &str) -> std::io::Result<String>;
    fn write_marker(&mut self, path: &str, json: &str) -> std::io::Result<()>;
    fn send_mister_command(&mut self, path: &str, command: &str) -> std::io::Result<()>;
    fn sleep_ms(&mut self, delay_ms: u64);
}

struct SystemFaultRuntime;

impl FaultRuntime for SystemFaultRuntime {
    fn read_text(&mut self, path: &str) -> std::io::Result<String> {
        fs::read_to_string(path)
    }

    fn write_marker(&mut self, path: &str, json: &str) -> std::io::Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, json)
    }

    fn send_mister_command(&mut self, path: &str, command: &str) -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new().write(true).open(path)?;
        file.write_all(command.as_bytes())?;
        file.flush()
    }

    fn sleep_ms(&mut self, delay_ms: u64) {
        std::thread::sleep(Duration::from_millis(delay_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    #[derive(Default)]
    struct FakeRuntime {
        session: Option<String>,
        marker: Option<(String, String)>,
        command: Option<(String, String)>,
        slept_ms: Vec<u64>,
    }

    impl FaultRuntime for FakeRuntime {
        fn read_text(&mut self, _path: &str) -> std::io::Result<String> {
            self.session
                .clone()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
        }

        fn write_marker(&mut self, path: &str, json: &str) -> std::io::Result<()> {
            self.marker = Some((path.to_string(), json.to_string()));
            Ok(())
        }

        fn send_mister_command(&mut self, path: &str, command: &str) -> std::io::Result<()> {
            self.command = Some((path.to_string(), command.to_string()));
            Ok(())
        }

        fn sleep_ms(&mut self, delay_ms: u64) {
            self.slept_ms.push(delay_ms);
        }
    }

    #[test]
    fn config_is_absent_without_point_env() {
        assert!(FaultConfig::from_values(None, None, None, None).is_none());
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
    fn non_matching_point_is_noop() {
        let config = FaultConfig {
            point: "wanted".into(),
            action: DIRECT_RESET_NO_SYNC.into(),
            delay_ms: 7,
            session: Some("test-session".into()),
        };
        let mut runtime = FakeRuntime::default();
        maybe_fault_with(
            "other",
            &PathBuf::from("/media/fat/mister-magik/library.sqlite3"),
            &config,
            &mut runtime,
        );
        assert!(runtime.marker.is_none());
        assert!(runtime.command.is_none());
        assert!(runtime.slept_ms.is_empty());
    }

    #[test]
    fn matching_point_writes_marker_sends_reset_and_sleeps() {
        let config = FaultConfig {
            point: "catalog.sqlite.after_final_temp_sync".into(),
            action: DIRECT_RESET_NO_SYNC.into(),
            delay_ms: 42,
            session: Some("test-session".into()),
        };
        let mut runtime = FakeRuntime {
            session: Some("test-session".into()),
            ..FakeRuntime::default()
        };
        maybe_fault_with(
            "catalog.sqlite.after_final_temp_sync",
            &PathBuf::from("/media/fat/mister-magik/library.sqlite3"),
            &config,
            &mut runtime,
        );
        let marker = runtime.marker.expect("marker");
        assert_eq!(marker.0, MARKER_PATH);
        assert!(marker.1.contains("catalog.sqlite.after_final_temp_sync"));
        assert!(marker.1.contains("/media/fat/mister-magik/library.sqlite3"));
        assert_eq!(
            runtime.command,
            Some((MISTER_CMD.to_string(), DIRECT_RESET_NO_SYNC_CMD.to_string()))
        );
        assert_eq!(runtime.slept_ms, vec![42]);
    }

    #[test]
    fn matching_direct_reset_without_volatile_session_is_noop() {
        let config = FaultConfig {
            point: "settings.after_temp_write".into(),
            action: DIRECT_RESET_NO_SYNC.into(),
            delay_ms: 42,
            session: Some("test-session".into()),
        };
        let mut runtime = FakeRuntime::default();
        maybe_fault_with(
            "settings.after_temp_write",
            &PathBuf::from("/media/fat/mister-magik/settings.json"),
            &config,
            &mut runtime,
        );
        assert!(runtime.marker.is_none());
        assert!(runtime.command.is_none());
        assert!(runtime.slept_ms.is_empty());
    }

    #[test]
    fn matching_direct_reset_with_wrong_volatile_session_is_noop() {
        let config = FaultConfig {
            point: "settings.after_temp_write".into(),
            action: DIRECT_RESET_NO_SYNC.into(),
            delay_ms: 42,
            session: Some("expected-session".into()),
        };
        let mut runtime = FakeRuntime {
            session: Some("stale-session".into()),
            ..FakeRuntime::default()
        };
        maybe_fault_with(
            "settings.after_temp_write",
            &PathBuf::from("/media/fat/mister-magik/settings.json"),
            &config,
            &mut runtime,
        );
        assert!(runtime.marker.is_none());
        assert!(runtime.command.is_none());
        assert!(runtime.slept_ms.is_empty());
    }

    #[test]
    fn env_config_defaults_to_direct_reset_no_sync() {
        let config = FaultConfig::from_values(Some("settings.after_rename"), None, None, None)
            .expect("config");
        assert_eq!(config.action, DIRECT_RESET_NO_SYNC);
        assert_eq!(config.delay_ms, DEFAULT_DELAY_MS);
        assert_eq!(config.session, None);
    }

    #[test]
    fn captured_config_redacts_the_volatile_session_token() {
        let config = FaultConfig::from_values(
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

    #[test]
    fn direct_reset_cleanup_artifacts_cover_both_layouts_and_volatile_state() {
        assert_eq!(
            DIRECT_RESET_CLEANUP_ARTIFACTS,
            [
                "/media/fat/mister-magik/launcher.env",
                "/media/fat/mister-magik-dev/launcher.env",
                "/tmp/mister-magik/fs-fault-launcher.env",
                "/tmp/mister-magik/fs-fault-session",
                "/tmp/mister-magik/fs-fault.json",
                "/media/fat/mister-magik/rebuild-on-next-boot",
                "/media/fat/mister-magik-dev/rebuild-on-next-boot",
            ]
        );
    }
}
